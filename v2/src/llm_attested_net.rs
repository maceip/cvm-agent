//! Small network-hardening primitives for LLM Attested services.
//!
//! The services stay deliberately simple, but they should fail closed under
//! load: bounded concurrency, rate limits, and persistent cooldowns.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const TOKEN_SCALE: u64 = 1_000;

#[derive(Debug, Clone, Copy)]
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
pub struct PersistentRateLimiter {
    path: PathBuf,
    max_sessions: usize,
    idle_ttl_ms: u64,
    save_interval_ms: u64,
    state: Mutex<LimiterState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LimiterState {
    sessions: BTreeMap<String, RateLimitSession>,
    #[serde(default)]
    dirty: bool,
    #[serde(default)]
    last_saved_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RateLimitSession {
    tokens_scaled: u64,
    last_refill_ms: u64,
    blocked_until_ms: u64,
    violations: u32,
    last_seen_ms: u64,
}

impl PersistentRateLimiter {
    pub fn load(path: PathBuf, max_sessions: usize) -> Result<Self> {
        let now = now_ms();
        let mut state = if path.exists() {
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("read rate limit state {}", path.display()))?;
            serde_json::from_str::<LimiterState>(&body)
                .with_context(|| format!("decode rate limit state {}", path.display()))?
        } else {
            LimiterState::default()
        };
        state.last_saved_ms = now;
        Ok(Self {
            path,
            max_sessions: max_sessions.max(1),
            idle_ttl_ms: 24 * 60 * 60 * 1_000,
            save_interval_ms: 5_000,
            state: Mutex::new(state),
        })
    }

    pub fn check(
        &self,
        key: impl Into<String>,
        policy: RateLimitPolicy,
    ) -> Result<RateLimitDecision> {
        let now = now_ms();
        let key = key.into();
        let mut state = self.state.lock().expect("rate limiter mutex poisoned");
        if state.sessions.len() > self.max_sessions {
            prune_sessions(&mut state, now, self.idle_ttl_ms);
        }

        let session = state
            .sessions
            .entry(key)
            .or_insert_with(|| RateLimitSession {
                tokens_scaled: policy.capacity as u64 * TOKEN_SCALE,
                last_refill_ms: now,
                blocked_until_ms: 0,
                violations: 0,
                last_seen_ms: now,
            });
        refill(session, policy, now);
        session.last_seen_ms = now;

        let cost = TOKEN_SCALE;
        let decision = if now < session.blocked_until_ms {
            RateLimitDecision {
                allowed: false,
                retry_after_ms: session.blocked_until_ms.saturating_sub(now),
                remaining: scaled_to_tokens(session.tokens_scaled),
                limit: policy.capacity,
            }
        } else if session.tokens_scaled >= cost {
            session.tokens_scaled -= cost;
            if session.violations > 0
                && session.tokens_scaled > (policy.capacity as u64 * TOKEN_SCALE / 2)
            {
                session.violations -= 1;
            }
            RateLimitDecision {
                allowed: true,
                retry_after_ms: 0,
                remaining: scaled_to_tokens(session.tokens_scaled),
                limit: policy.capacity,
            }
        } else {
            session.violations = session.violations.saturating_add(1).min(20);
            let refill_wait_ms = refill_wait_ms(session.tokens_scaled, policy);
            let backoff_ms = exponential_backoff_ms(policy, session.violations);
            let retry_after_ms = refill_wait_ms.max(backoff_ms);
            session.blocked_until_ms = now.saturating_add(retry_after_ms);
            RateLimitDecision {
                allowed: false,
                retry_after_ms,
                remaining: scaled_to_tokens(session.tokens_scaled),
                limit: policy.capacity,
            }
        };

        state.dirty = true;
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
        let body = serde_json::to_vec_pretty(state).context("encode rate limit state")?;
        std::fs::write(&self.path, body)
            .with_context(|| format!("write rate limit state {}", self.path.display()))?;
        state.dirty = false;
        state.last_saved_ms = now;
        Ok(())
    }
}

fn refill(session: &mut RateLimitSession, policy: RateLimitPolicy, now: u64) {
    let elapsed_ms = now.saturating_sub(session.last_refill_ms);
    if elapsed_ms == 0 {
        return;
    }
    let refill = elapsed_ms
        .saturating_mul(policy.refill_per_minute as u64)
        .saturating_mul(TOKEN_SCALE)
        / 60_000;
    let capacity = policy.capacity as u64 * TOKEN_SCALE;
    session.tokens_scaled = session.tokens_scaled.saturating_add(refill).min(capacity);
    session.last_refill_ms = now;
}

fn refill_wait_ms(tokens_scaled: u64, policy: RateLimitPolicy) -> u64 {
    let missing = TOKEN_SCALE.saturating_sub(tokens_scaled);
    let refill_per_ms = policy.refill_per_minute as u64 * TOKEN_SCALE;
    missing
        .saturating_mul(60_000)
        .div_ceil(refill_per_ms.max(1))
}

fn exponential_backoff_ms(policy: RateLimitPolicy, violations: u32) -> u64 {
    let shift = violations.saturating_sub(1).min(16);
    policy
        .base_backoff_ms
        .saturating_mul(1u64 << shift)
        .min(policy.max_backoff_ms)
}

fn prune_sessions(state: &mut LimiterState, now: u64, idle_ttl_ms: u64) {
    state
        .sessions
        .retain(|_, session| now.saturating_sub(session.last_seen_ms) <= idle_ttl_ms);
}

fn scaled_to_tokens(tokens_scaled: u64) -> u32 {
    (tokens_scaled / TOKEN_SCALE).min(u32::MAX as u64) as u32
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
        let limiter = PersistentRateLimiter::load(dir.path().join("rate.json"), 64).unwrap();
        let policy = RateLimitPolicy::per_minute(60, 1).with_backoff(250, 1_000);

        assert!(limiter.check("ip:127.0.0.1", policy).unwrap().allowed);
        let blocked = limiter.check("ip:127.0.0.1", policy).unwrap();
        assert!(!blocked.allowed);
        assert!(blocked.retry_after_ms >= 250);
    }
}
