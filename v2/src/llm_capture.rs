//! Capture adapter helpers for LLM Attested.
//!
//! These helpers are intentionally below the scoring layer. Browser adapters,
//! CLI wrappers, workbench launchers, and local proxies all normalize into
//! `ContestEvent` before they reach the event service.

use crate::llm_attested::{random_id, sha256_prefixed, ContestEvent, EventActor, LlmAttestedError};
use ed25519_dalek::SigningKey;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ZERO_HASH: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureReport {
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub hackathon_id: String,
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub gateway_id: String,
    #[serde(default)]
    pub manifest_hash: String,
    #[serde(default)]
    pub capture_method: String,
    #[serde(default)]
    pub assurance: String,
    #[serde(default)]
    pub event_index: u64,
    #[serde(default)]
    pub previous_team_event_hash: String,
    #[serde(default)]
    pub started_at_ms: u64,
    #[serde(default)]
    pub completed_at_ms: u64,
    #[serde(default = "default_actor")]
    pub actor: EventActor,
    #[serde(default)]
    pub subject: BTreeMap<String, String>,
    #[serde(default)]
    pub metrics: BTreeMap<String, i64>,
    #[serde(default)]
    pub hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub evidence: BTreeMap<String, String>,
}

impl CaptureReport {
    pub fn new(
        kind: impl Into<String>,
        hackathon_id: impl Into<String>,
        team_id: impl Into<String>,
        capture_method: impl Into<String>,
        assurance: impl Into<String>,
    ) -> Self {
        let now = now_ms();
        Self {
            kind: kind.into(),
            hackathon_id: hackathon_id.into(),
            team_id: team_id.into(),
            gateway_id: "capture_adapter".to_string(),
            manifest_hash: ZERO_HASH.to_string(),
            capture_method: capture_method.into(),
            assurance: assurance.into(),
            event_index: 0,
            previous_team_event_hash: ZERO_HASH.to_string(),
            started_at_ms: now,
            completed_at_ms: now,
            actor: EventActor {
                kind: "agent".to_string(),
                agent_session_id: "agent_default".to_string(),
            },
            subject: BTreeMap::new(),
            metrics: BTreeMap::new(),
            hashes: BTreeMap::new(),
            evidence: BTreeMap::new(),
        }
    }

    pub fn normalize(mut self) -> Self {
        let now = now_ms();
        if self.kind.is_empty() {
            self.kind = default_kind();
        }
        if self.started_at_ms == 0 {
            self.started_at_ms = now;
        }
        if self.completed_at_ms == 0 {
            self.completed_at_ms = self.started_at_ms;
        }
        if self.previous_team_event_hash.is_empty() {
            self.previous_team_event_hash = ZERO_HASH.to_string();
        }
        if self.manifest_hash.is_empty() {
            self.manifest_hash = ZERO_HASH.to_string();
        }
        if self.gateway_id.is_empty() {
            self.gateway_id = "capture_adapter".to_string();
        }
        self
    }

    pub fn sign(self, signing_key: &SigningKey) -> Result<ContestEvent, LlmAttestedError> {
        let report = self.normalize();
        let mut event = ContestEvent::new(
            random_id("evt"),
            report.kind,
            report.hackathon_id,
            report.team_id,
            report.gateway_id,
            report.manifest_hash,
            report.capture_method,
            report.assurance,
            report.event_index,
            report.previous_team_event_hash,
            report.started_at_ms,
            report.completed_at_ms,
            report.actor,
            report.subject,
            report.metrics,
            report.hashes,
            report.evidence,
        );
        event.sign(signing_key)?;
        Ok(event)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantConfig {
    #[serde(default = "participant_config_version")]
    pub version: u32,
    pub event_id: String,
    #[serde(default)]
    pub issuer: String,
    pub team_id: String,
    pub team_name: String,
    pub team_api_key: String,
    pub gateway_base_url: String,
    pub event_sink_url: String,
    pub bootstrap_url: String,
    pub gateway_manifest_url: String,
    pub runcard_url: String,
    pub runcard_json_url: String,
    #[serde(default)]
    pub runcard_embed_url: String,
    #[serde(default)]
    pub runcard_image_url: String,
    #[serde(default)]
    pub runcard_proof_url: String,
    #[serde(default)]
    pub capture_methods: Vec<String>,
}

impl ParticipantConfig {
    pub fn from_team_payload(value: &Value) -> std::result::Result<Self, String> {
        let runcard_url = required_string(value, "runcard_url")?;
        let cfg = Self {
            version: participant_config_version(),
            event_id: required_string(value, "event_id")?,
            issuer: optional_string(value, "issuer").unwrap_or_default(),
            team_id: required_string(value, "team_id")?,
            team_name: required_string(value, "team_name")?,
            team_api_key: required_string(value, "team_api_key")?,
            gateway_base_url: required_string(value, "gateway_base_url")?,
            event_sink_url: required_string(value, "event_sink_url")?,
            bootstrap_url: required_string(value, "bootstrap_url")?,
            gateway_manifest_url: required_string(value, "gateway_manifest_url")?,
            runcard_json_url: optional_string(value, "runcard_json_url")
                .unwrap_or_else(|| format!("{runcard_url}.json")),
            runcard_embed_url: optional_string(value, "runcard_embed_url")
                .unwrap_or_else(|| format!("{runcard_url}.embed")),
            runcard_image_url: optional_string(value, "runcard_image_url")
                .unwrap_or_else(|| format!("{runcard_url}.svg")),
            runcard_proof_url: optional_string(value, "runcard_proof_url")
                .unwrap_or_else(|| format!("{runcard_url}.proof.json")),
            runcard_url,
            capture_methods: value
                .get("capture_methods")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        };
        Ok(cfg)
    }

    pub fn load(path: &Path) -> std::result::Result<Self, String> {
        let body = std::fs::read_to_string(path)
            .map_err(|e| format!("read participant config {}: {e}", path.display()))?;
        serde_json::from_str(&body)
            .map_err(|e| format!("decode participant config {}: {e}", path.display()))
    }

    pub fn save(&self, path: &Path) -> std::result::Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create config dir {}: {e}", parent.display()))?;
            }
        }
        let body = serde_json::to_vec_pretty(self)
            .map_err(|e| format!("encode participant config: {e}"))?;
        std::fs::write(path, body)
            .map_err(|e| format!("write participant config {}: {e}", path.display()))
    }

    pub fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var("LLMATTEST_CONFIG") {
            return PathBuf::from(path);
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".llmattest").join("config.json");
        }
        PathBuf::from("./llmattest-config.json")
    }

    pub fn openai_base_url(&self) -> String {
        format!("{}/v1", self.gateway_base_url.trim_end_matches('/'))
    }

    pub fn apply_to_report(&self, report: &mut CaptureReport) {
        if report.hackathon_id.is_empty() {
            report.hackathon_id = self.event_id.clone();
        }
        if report.team_id.is_empty() {
            report.team_id = self.team_id.clone();
        }
    }
}

pub fn signing_key_from_seed(seed: Option<&str>) -> SigningKey {
    let mut bytes = [42u8; 32];
    if let Some(seed) = seed {
        let digest = sha256_prefixed(seed.as_bytes());
        if let Ok(decoded) = hex::decode(digest.trim_start_matches("sha256:")) {
            if decoded.len() == 32 {
                bytes.copy_from_slice(&decoded);
            }
        }
    } else {
        rand::thread_rng().fill_bytes(&mut bytes);
    }
    SigningKey::from_bytes(&bytes)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn participant_config_version() -> u32 {
    1
}

fn default_kind() -> String {
    "llm.call".to_string()
}

fn default_actor() -> EventActor {
    EventActor {
        kind: "agent".to_string(),
        agent_session_id: "agent_default".to_string(),
    }
}

fn required_string(value: &Value, key: &str) -> std::result::Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| format!("team payload missing string field {key}"))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
