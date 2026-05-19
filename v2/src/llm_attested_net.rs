//! Network-hardening glue for LLM Attested services.
//!
//! Rate decisions are delegated to `governor` so the project does not own a
//! token-bucket implementation. This module only maps project policies and
//! caller keys to governor limiters and response metadata.

use anyhow::{Context, Result};
use governor::clock::{Clock, DefaultClock};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitPolicy {
    pub capacity: u32,
    pub refill_per_minute: u32,
    pub base_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl RateLimitPolicy {
    pub fn per_minute(refill_per_minute: u32, burst: u32) -> Self {
        Self {
            capacity: burst.max(1),
            refill_per_minute: refill_per_minute.max(1),
            base_backoff_ms: 1_000,
            max_backoff_ms: 60_000,
        }
    }

    pub fn with_backoff(mut self, base_backoff_ms: u64, max_backoff_ms: u64) -> Self {
        self.base_backoff_ms = base_backoff_ms.max(1);
        self.max_backoff_ms = max_backoff_ms.max(self.base_backoff_ms);
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RateLimitDecision {
    pub allowed: bool,
    pub retry_after_ms: u64,
    pub remaining: u32,
    pub limit: u32,
}

#[derive(Debug)]
pub struct ServiceRateLimiter {
    path: PathBuf,
    max_sessions: usize,
    idle_ttl_ms: u64,
    save_interval_ms: u64,
    state: Mutex<LimiterState>,
}

#[derive(Debug)]
struct LimiterState {
    sessions: BTreeMap<String, LimiterEntry>,
    dirty: bool,
    last_saved_ms: u64,
}

#[derive(Debug)]
struct LimiterEntry {
    policy: RateLimitPolicy,
    limiter: Arc<DefaultDirectRateLimiter>,
    blocked_until_ms: u64,
    violations: u32,
    last_seen_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedLimiterState {
    sessions: BTreeMap<String, PersistedSession>,
    #[serde(default)]
    last_saved_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSession {
    blocked_until_ms: u64,
    violations: u32,
    last_seen_ms: u64,
}

impl ServiceRateLimiter {
    pub fn load(path: PathBuf, max_sessions: usize) -> Result<Self> {
        let now = now_ms();
        let persisted = if path.exists() {
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("read rate limit state {}", path.display()))?;
            serde_json::from_str::<PersistedLimiterState>(&body)
                .with_context(|| format!("decode rate limit state {}", path.display()))?
        } else {
            PersistedLimiterState::default()
        };
        let default_policy = RateLimitPolicy::per_minute(1, 1);
        let sessions = persisted
            .sessions
            .into_iter()
            .map(|(key, session)| {
                (
                    key,
                    LimiterEntry {
                        policy: default_policy,
                        limiter: Arc::new(RateLimiter::direct(quota(default_policy))),
                        blocked_until_ms: session.blocked_until_ms,
                        violations: session.violations,
                        last_seen_ms: session.last_seen_ms,
                    },
                )
            })
            .collect();
        Ok(Self {
            path,
            max_sessions: max_sessions.max(1),
            idle_ttl_ms: 24 * 60 * 60 * 1_000,
            save_interval_ms: 5_000,
            state: Mutex::new(LimiterState {
                sessions,
                dirty: false,
                last_saved_ms: now,
            }),
        })
    }

    pub fn check(
        &self,
        key: impl Into<String>,
        policy: RateLimitPolicy,
    ) -> Result<RateLimitDecision> {
        let key = key.into();
        let now = now_ms();
        let mut state = self.state.lock().expect("rate limiter mutex poisoned");
        if state.sessions.len() > self.max_sessions {
            prune_sessions(&mut state.sessions, now, self.idle_ttl_ms);
            state.dirty = true;
        }
        let entry = state.sessions.entry(key).or_insert_with(|| LimiterEntry {
            policy,
            limiter: Arc::new(RateLimiter::direct(quota(policy))),
            blocked_until_ms: 0,
            violations: 0,
            last_seen_ms: now,
        });
        if entry.policy != policy {
            let blocked_until_ms = entry.blocked_until_ms;
            let violations = entry.violations;
            *entry = LimiterEntry {
                policy,
                limiter: Arc::new(RateLimiter::direct(quota(policy))),
                blocked_until_ms,
                violations,
                last_seen_ms: now,
            };
        }
        entry.last_seen_ms = now;
        let limit = policy.capacity;
        let decision = if now < entry.blocked_until_ms {
            RateLimitDecision {
                allowed: false,
                retry_after_ms: entry.blocked_until_ms.saturating_sub(now),
                remaining: 0,
                limit,
            }
        } else {
            match entry.limiter.check() {
                Ok(()) => {
                    if entry.violations > 0 {
                        entry.violations -= 1;
                        state.dirty = true;
                    }
                    RateLimitDecision {
                        allowed: true,
                        retry_after_ms: 0,
                        remaining: limit.saturating_sub(1),
                        limit,
                    }
                }
                Err(until) => {
                    let wait = until.wait_time_from(DefaultClock::default().now());
                    entry.violations = entry.violations.saturating_add(1).min(20);
                    let retry_after_ms = (wait.as_millis().min(u64::MAX as u128) as u64)
                        .max(exponential_backoff_ms(policy, entry.violations));
                    entry.blocked_until_ms = now.saturating_add(retry_after_ms);
                    state.dirty = true;
                    RateLimitDecision {
                        allowed: false,
                        retry_after_ms,
                        remaining: 0,
                        limit,
                    }
                }
            }
        };
        if decision.allowed {
            self.save_if_due(&mut state, now)?;
        } else {
            self.save_locked(&mut state, now)?;
        }
        Ok(decision)
    }

    pub fn flush(&self) -> Result<()> {
        let mut state = self.state.lock().expect("rate limiter mutex poisoned");
        self.save_locked(&mut state, now_ms())
    }

    fn save_if_due(&self, state: &mut LimiterState, now: u64) -> Result<()> {
        if state.dirty && now.saturating_sub(state.last_saved_ms) >= self.save_interval_ms {
            self.save_locked(state, now)?;
        }
        Ok(())
    }

    fn save_locked(&self, state: &mut LimiterState, now: u64) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create rate limit dir {}", parent.display()))?;
            }
        }
        let persisted = PersistedLimiterState {
            sessions: state
                .sessions
                .iter()
                .map(|(key, session)| {
                    (
                        key.clone(),
                        PersistedSession {
                            blocked_until_ms: session.blocked_until_ms,
                            violations: session.violations,
                            last_seen_ms: session.last_seen_ms,
                        },
                    )
                })
                .collect(),
            last_saved_ms: now,
        };
        let body = serde_json::to_vec_pretty(&persisted).context("encode rate limit state")?;
        std::fs::write(&self.path, body)
            .with_context(|| format!("write rate limit state {}", self.path.display()))?;
        state.dirty = false;
        state.last_saved_ms = now;
        Ok(())
    }
}

fn exponential_backoff_ms(policy: RateLimitPolicy, violations: u32) -> u64 {
    let shift = violations.saturating_sub(1).min(16);
    policy
        .base_backoff_ms
        .saturating_mul(1u64 << shift)
        .min(policy.max_backoff_ms)
}

fn quota(policy: RateLimitPolicy) -> Quota {
    let replenish = NonZeroU32::new(policy.refill_per_minute).expect("refill is nonzero");
    let burst = NonZeroU32::new(policy.capacity).expect("capacity is nonzero");
    Quota::per_minute(replenish).allow_burst(burst)
}

fn prune_sessions(state: &mut BTreeMap<String, LimiterEntry>, now: u64, idle_ttl_ms: u64) {
    state.retain(|_, session| now.saturating_sub(session.last_seen_ms) <= idle_ttl_ms);
}

pub fn retry_after_secs(retry_after_ms: u64) -> u64 {
    retry_after_ms.div_ceil(1_000).max(1)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_blocks_and_reports_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let limiter = ServiceRateLimiter::load(dir.path().join("rate.json"), 64).unwrap();
        let policy = RateLimitPolicy::per_minute(60, 1).with_backoff(250, 1_000);

        assert!(limiter.check("ip:127.0.0.1", policy).unwrap().allowed);
        let blocked = limiter.check("ip:127.0.0.1", policy).unwrap();
        assert!(!blocked.allowed);
        assert!(blocked.retry_after_ms > 0);
    }

    #[test]
    fn limiter_persists_active_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rate.json");
        let policy = RateLimitPolicy::per_minute(60, 1).with_backoff(750, 1_000);
        {
            let limiter = ServiceRateLimiter::load(path.clone(), 64).unwrap();
            assert!(limiter.check("team:one", policy).unwrap().allowed);
            assert!(!limiter.check("team:one", policy).unwrap().allowed);
        }

        let restored = ServiceRateLimiter::load(path, 64).unwrap();
        let blocked = restored.check("team:one", policy).unwrap();
        assert!(!blocked.allowed);
        assert!(blocked.retry_after_ms > 0);
    }
}
