//! LLM Attested application primitives.
//!
//! The core interface is intentionally small:
//! - `ContestManifest` commits to the policy, scorer, gateway, and event key.
//! - `ContestEvent` records one competition-relevant action.
//! - `ScoreReport` is the dashboard-facing output of a scorer.
//!
//! Provider adapters and dashboards should depend on these types, not on each
//! other. That keeps new competition formats as pure scorers over verified
//! events.

use crate::eat::EatToken;
use crate::quote::{verify::verify_platform_quote, Platform};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MANIFEST_VERSION: u32 = 1;
pub const MANIFEST_PROFILE: &str = "https://cvm.dev/llm-contest-manifest/v1";
pub const EVENT_VERSION: u32 = 1;
pub const EVENT_PROFILE: &str = "https://cvm.dev/contest-event/v1";
pub const CAPTURE_RA_PROFILE: &str = "https://cvm.dev/llm-capture-ra/v1";

#[derive(Debug, thiserror::Error)]
pub enum LlmAttestedError {
    #[error("CBOR encode failed: {0}")]
    Encode(String),
    #[error("CBOR decode failed: {0}")]
    Decode(String),
    #[error("signature decode failed: {0}")]
    SignatureDecode(String),
    #[error("signature verify failed")]
    SignatureVerify,
    #[error("invalid public key")]
    InvalidPublicKey,
    #[error("event version mismatch: expected {expected}, got {got}")]
    EventVersionMismatch { expected: u32, got: u32 },
    #[error("event profile mismatch: expected {expected}, got {got}")]
    EventProfileMismatch { expected: String, got: String },
    #[error("manifest version mismatch: expected {expected}, got {got}")]
    ManifestVersionMismatch { expected: u32, got: u32 },
    #[error("manifest profile mismatch: expected {expected}, got {got}")]
    ManifestProfileMismatch { expected: String, got: String },
}

pub type Result<T> = std::result::Result<T, LlmAttestedError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttestedCapturePathSpec {
    pub capture_method: &'static str,
    pub assurance: &'static str,
    pub enforcement: &'static str,
    pub requirement: &'static str,
}

pub const REMOTE_ATTESTED_CAPTURE_PATHS: &[AttestedCapturePathSpec] = &[
    AttestedCapturePathSpec {
        capture_method: "gateway_proxy",
        assurance: "attested",
        enforcement: "routed",
        requirement: "gateway endpoint serves manifest and model API from a Cvm-verified TEE",
    },
    AttestedCapturePathSpec {
        capture_method: "gateway_proxy",
        assurance: "attested",
        enforcement: "managed_keys",
        requirement: "organizer-managed provider keys live behind a Cvm-verified TEE gateway",
    },
    AttestedCapturePathSpec {
        capture_method: "local_proxy",
        assurance: "attested",
        enforcement: "routed",
        requirement: "local-model proxy runs in a Cvm-verified TEE sidecar or workspace",
    },
    AttestedCapturePathSpec {
        capture_method: "sdk_cli_wrapper",
        assurance: "attested",
        enforcement: "routed",
        requirement: "wrapper event signer runs inside a Cvm-verified TEE workspace",
    },
    AttestedCapturePathSpec {
        capture_method: "browser_extension",
        assurance: "attested",
        enforcement: "attested_broker",
        requirement: "browser observer is mediated by a Cvm-verified broker; desktop extension alone is insufficient",
    },
    AttestedCapturePathSpec {
        capture_method: "strict_workbench",
        assurance: "attested",
        enforcement: "strict_workbench",
        requirement: "workbench launcher and event signer run inside a Cvm-verified TEE",
    },
    AttestedCapturePathSpec {
        capture_method: "tee_workspace",
        assurance: "attested",
        enforcement: "full_tee",
        requirement: "participant workspace and event signer run inside a Cvm-verified TEE",
    },
];

#[derive(Debug, thiserror::Error)]
pub enum CaptureRaError {
    #[error("capture path {capture_method}/{enforcement} is not a remote-attested path")]
    UnsupportedCapturePath {
        capture_method: String,
        enforcement: String,
    },
    #[error("EAT decode failed: {0}")]
    EatDecode(String),
    #[error("EAT has unknown platform discriminant {0}")]
    UnknownPlatform(u8),
    #[error("TEE platform {platform} is not accepted by policy {accepted:?}")]
    PlatformNotAccepted {
        platform: String,
        accepted: Vec<String>,
    },
    #[error("TEE quote is empty")]
    EmptyQuote,
    #[error("platform quote verification failed: {0}")]
    QuoteVerify(String),
    #[error("attestation chain value_x drift: leaf {leaf}, previous {previous}")]
    ChainValueXDrift { leaf: String, previous: String },
    #[error("attestation chain is deeper than {0} stages")]
    ChainTooDeep(usize),
    #[error("cvm receipt hash mismatch: expected {expected}, got {got}")]
    ReceiptHashMismatch { expected: String, got: String },
    #[error("manifest value_x mismatch: expected {expected}, got {got}")]
    ValueXMismatch { expected: String, got: String },
    #[error("manifest does not advertise capture method {0}")]
    CaptureMethodNotAdvertised(String),
    #[error("manifest hash failed: {0}")]
    ManifestHash(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifiedCvmReceipt {
    pub verifier_profile: String,
    pub platform: String,
    pub value_x: String,
    pub platform_measurement: String,
    pub quote_binding_hash: String,
    pub receipt_hash: String,
    pub chain_depth: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureRaClaim {
    pub profile: String,
    pub capture_method: String,
    pub assurance: String,
    pub enforcement: String,
    pub requirement: String,
    pub verifier_profile: String,
    pub platform: String,
    pub value_x: String,
    pub platform_measurement: String,
    pub quote_binding_hash: String,
    pub cvm_receipt_hash: String,
    pub manifest_hash: String,
    pub event_signing_key_hash: String,
    pub chain_depth: u32,
}

pub fn attested_capture_path_spec(
    capture_method: &str,
    assurance: &str,
    enforcement: &str,
) -> Option<&'static AttestedCapturePathSpec> {
    if assurance != "attested" {
        return None;
    }
    REMOTE_ATTESTED_CAPTURE_PATHS
        .iter()
        .find(|spec| spec.capture_method == capture_method && spec.enforcement == enforcement)
}

pub fn verify_cvm_receipt(
    receipt_cbor: &[u8],
    accepted_tee_platforms: &[String],
    verifier_profile: &str,
) -> std::result::Result<VerifiedCvmReceipt, CaptureRaError> {
    let receipt_hash = sha256_prefixed(receipt_cbor);
    let eat =
        EatToken::from_cbor(receipt_cbor).map_err(|e| CaptureRaError::EatDecode(e.to_string()))?;
    let leaf_value_x = eat.value_x;
    let (platform, platform_measurement, binding, depth) =
        verify_eat_chain(&eat, accepted_tee_platforms, &leaf_value_x, 0)?;
    Ok(VerifiedCvmReceipt {
        verifier_profile: verifier_profile.to_string(),
        platform: platform_label(platform).to_string(),
        value_x: format!("sha384:{}", hex::encode(leaf_value_x)),
        platform_measurement: hex::encode(platform_measurement),
        quote_binding_hash: format!("sha256:{}", hex::encode(binding)),
        receipt_hash,
        chain_depth: depth,
    })
}

pub fn build_capture_ra_claim(
    manifest: &ContestManifest,
    manifest_hash: &str,
    receipt_cbor: &[u8],
    capture_method: &str,
    assurance: &str,
    enforcement: &str,
) -> std::result::Result<CaptureRaClaim, CaptureRaError> {
    let Some(spec) = attested_capture_path_spec(capture_method, assurance, enforcement) else {
        return Err(CaptureRaError::UnsupportedCapturePath {
            capture_method: capture_method.to_string(),
            enforcement: enforcement.to_string(),
        });
    };
    if !manifest
        .capture_methods
        .iter()
        .any(|method| method == capture_method)
    {
        return Err(CaptureRaError::CaptureMethodNotAdvertised(
            capture_method.to_string(),
        ));
    }
    let verified = verify_cvm_receipt(
        receipt_cbor,
        &manifest.trust_policy.accepted_tee_platforms,
        &manifest.trust_policy.verifier_profile,
    )?;
    if manifest.cvm_receipt_hash != verified.receipt_hash {
        return Err(CaptureRaError::ReceiptHashMismatch {
            expected: manifest.cvm_receipt_hash.clone(),
            got: verified.receipt_hash,
        });
    }
    if normalize_value_x(&manifest.value_x) != normalize_value_x(&verified.value_x) {
        return Err(CaptureRaError::ValueXMismatch {
            expected: manifest.value_x.clone(),
            got: verified.value_x,
        });
    }
    Ok(CaptureRaClaim {
        profile: CAPTURE_RA_PROFILE.to_string(),
        capture_method: capture_method.to_string(),
        assurance: assurance.to_string(),
        enforcement: enforcement.to_string(),
        requirement: spec.requirement.to_string(),
        verifier_profile: verified.verifier_profile,
        platform: verified.platform,
        value_x: verified.value_x,
        platform_measurement: verified.platform_measurement,
        quote_binding_hash: verified.quote_binding_hash,
        cvm_receipt_hash: verified.receipt_hash,
        manifest_hash: manifest_hash.to_string(),
        event_signing_key_hash: sha256_prefixed(manifest.event_signing_jwk.x.as_bytes()),
        chain_depth: verified.chain_depth,
    })
}

pub fn insert_capture_ra_evidence(evidence: &mut BTreeMap<String, String>, claim: &CaptureRaClaim) {
    evidence.insert("ra_profile".to_string(), claim.profile.clone());
    evidence.insert(
        "ra_capture_method".to_string(),
        claim.capture_method.clone(),
    );
    evidence.insert("ra_assurance".to_string(), claim.assurance.clone());
    evidence.insert("ra_enforcement".to_string(), claim.enforcement.clone());
    evidence.insert("ra_requirement".to_string(), claim.requirement.clone());
    evidence.insert(
        "ra_verifier_profile".to_string(),
        claim.verifier_profile.clone(),
    );
    evidence.insert("ra_platform".to_string(), claim.platform.clone());
    evidence.insert("ra_value_x".to_string(), claim.value_x.clone());
    evidence.insert(
        "ra_platform_measurement".to_string(),
        claim.platform_measurement.clone(),
    );
    evidence.insert(
        "ra_quote_binding_hash".to_string(),
        claim.quote_binding_hash.clone(),
    );
    evidence.insert(
        "ra_cvm_receipt_hash".to_string(),
        claim.cvm_receipt_hash.clone(),
    );
    evidence.insert("ra_manifest_hash".to_string(), claim.manifest_hash.clone());
    evidence.insert(
        "ra_event_signing_key_hash".to_string(),
        claim.event_signing_key_hash.clone(),
    );
    evidence.insert("ra_chain_depth".to_string(), claim.chain_depth.to_string());
}

pub fn event_matches_capture_ra_claim(event: &ContestEvent, claim: &CaptureRaClaim) -> bool {
    event.capture_method == claim.capture_method
        && event.assurance == claim.assurance
        && event.manifest_hash == claim.manifest_hash
        && event
            .evidence
            .get("enforcement_mode")
            .map(|value| value == &claim.enforcement)
            .unwrap_or(false)
        && event
            .evidence
            .get("ra_profile")
            .map(|value| value == &claim.profile)
            .unwrap_or(false)
        && event
            .evidence
            .get("ra_cvm_receipt_hash")
            .map(|value| value == &claim.cvm_receipt_hash)
            .unwrap_or(false)
        && event
            .evidence
            .get("ra_event_signing_key_hash")
            .map(|value| value == &claim.event_signing_key_hash)
            .unwrap_or(false)
}

fn verify_eat_chain(
    eat: &EatToken,
    accepted_tee_platforms: &[String],
    leaf_value_x: &[u8; 48],
    depth: u32,
) -> std::result::Result<(Platform, Vec<u8>, [u8; 32], u32), CaptureRaError> {
    if depth > 16 {
        return Err(CaptureRaError::ChainTooDeep(16));
    }
    if &eat.value_x != leaf_value_x {
        return Err(CaptureRaError::ChainValueXDrift {
            leaf: format!("sha384:{}", hex::encode(leaf_value_x)),
            previous: format!("sha384:{}", hex::encode(eat.value_x)),
        });
    }
    let platform = eat
        .platform_enum()
        .ok_or(CaptureRaError::UnknownPlatform(eat.platform))?;
    let platform_name = platform_label(platform).to_string();
    if !platform_is_accepted(platform, accepted_tee_platforms) {
        return Err(CaptureRaError::PlatformNotAccepted {
            platform: platform_name,
            accepted: accepted_tee_platforms.to_vec(),
        });
    }
    if eat.platform_quote.is_empty() {
        return Err(CaptureRaError::EmptyQuote);
    }
    let binding = eat.binding_bytes();
    let measurements = verify_platform_quote(platform, &eat.platform_quote, &binding)
        .map_err(|e| CaptureRaError::QuoteVerify(e.to_string()))?;
    let measurement = measurements
        .first()
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| eat.platform_measurement.clone());
    if let Some(previous) = eat
        .decode_previous()
        .map_err(|e| CaptureRaError::EatDecode(e.to_string()))?
    {
        let (_, _, _, previous_depth) =
            verify_eat_chain(&previous, accepted_tee_platforms, leaf_value_x, depth + 1)?;
        Ok((platform, measurement, binding, previous_depth))
    } else {
        Ok((platform, measurement, binding, depth + 1))
    }
}

fn platform_is_accepted(platform: Platform, accepted_tee_platforms: &[String]) -> bool {
    let platform = normalize_platform_label(platform_label(platform));
    accepted_tee_platforms
        .iter()
        .any(|accepted| normalize_platform_label(accepted) == platform)
}

pub fn platform_label(platform: Platform) -> &'static str {
    match platform {
        Platform::Nitro => "nitro",
        Platform::SevSnp => "sev-snp",
        Platform::Tdx => "tdx",
    }
}

fn normalize_platform_label(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "tdx" => "tdx".to_string(),
        "nitro" | "aws-nitro" => "nitro".to_string(),
        "snp" | "sev-snp" | "sevsnp" | "amd-sev-snp" => "sev-snp".to_string(),
        other => other.to_string(),
    }
}

fn normalize_value_x(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("sha384:")
        .to_ascii_lowercase()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicJwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
}

impl PublicJwk {
    pub fn from_verifying_key(key: &VerifyingKey) -> Self {
        Self {
            kty: "OKP".to_string(),
            crv: "Ed25519".to_string(),
            x: URL_SAFE_NO_PAD.encode(key.to_bytes()),
        }
    }

    pub fn to_verifying_key(&self) -> Result<VerifyingKey> {
        if self.kty != "OKP" || self.crv != "Ed25519" {
            return Err(LlmAttestedError::InvalidPublicKey);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(self.x.as_bytes())
            .map_err(|e| LlmAttestedError::SignatureDecode(e.to_string()))?;
        let array: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| LlmAttestedError::InvalidPublicKey)?;
        VerifyingKey::from_bytes(&array).map_err(|_| LlmAttestedError::InvalidPublicKey)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustPolicy {
    pub required: bool,
    pub accepted_tee_platforms: Vec<String>,
    pub cvm_receipt_url: String,
    pub registry_url: String,
    pub verifier_profile: String,
}

impl TrustPolicy {
    pub fn self_hosted(
        accepted_tee_platforms: Vec<String>,
        cvm_receipt_url: String,
        registry_url: String,
    ) -> Self {
        Self {
            required: true,
            accepted_tee_platforms,
            cvm_receipt_url,
            registry_url,
            verifier_profile: "cvm-eat-v2".to_string(),
        }
    }
}

impl Default for TrustPolicy {
    fn default() -> Self {
        Self {
            required: true,
            accepted_tee_platforms: vec![
                "tdx".to_string(),
                "sev-snp".to_string(),
                "nitro".to_string(),
            ],
            cvm_receipt_url: String::new(),
            registry_url: String::new(),
            verifier_profile: "cvm-eat-v2".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContestManifest {
    pub version: u32,
    pub profile: String,
    pub issuer: String,
    pub hackathon_id: String,
    pub gateway_id: String,
    pub competition_url: String,
    pub gateway_manifest_url: String,
    pub cvm_receipt_hash: String,
    pub value_x: String,
    pub policy_hash: String,
    pub scoring_rule_hash: String,
    pub enforcement_mode: String,
    pub start_url: String,
    pub capture_methods: Vec<String>,
    pub trust_policy: TrustPolicy,
    pub event_signing_jwk: PublicJwk,
    pub event_kinds: Vec<String>,
}

impl ContestManifest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        issuer: String,
        hackathon_id: String,
        gateway_id: String,
        competition_url: String,
        gateway_manifest_url: String,
        cvm_receipt_hash: String,
        value_x: String,
        policy_hash: String,
        scoring_rule_hash: String,
        enforcement_mode: String,
        start_url: String,
        capture_methods: Vec<String>,
        trust_policy: TrustPolicy,
        event_signing_key: &VerifyingKey,
        event_kinds: Vec<String>,
    ) -> Self {
        Self {
            version: MANIFEST_VERSION,
            profile: MANIFEST_PROFILE.to_string(),
            issuer,
            hackathon_id,
            gateway_id,
            competition_url,
            gateway_manifest_url,
            cvm_receipt_hash,
            value_x,
            policy_hash,
            scoring_rule_hash,
            enforcement_mode,
            start_url,
            capture_methods,
            trust_policy,
            event_signing_jwk: PublicJwk::from_verifying_key(event_signing_key),
            event_kinds,
        }
    }

    pub fn to_cbor(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        ciborium::ser::into_writer(self, &mut out)
            .map_err(|e| LlmAttestedError::Encode(e.to_string()))?;
        Ok(out)
    }

    pub fn from_cbor(bytes: &[u8]) -> Result<Self> {
        let manifest: Self = ciborium::de::from_reader(bytes)
            .map_err(|e| LlmAttestedError::Decode(e.to_string()))?;
        if manifest.version != MANIFEST_VERSION {
            return Err(LlmAttestedError::ManifestVersionMismatch {
                expected: MANIFEST_VERSION,
                got: manifest.version,
            });
        }
        if manifest.profile != MANIFEST_PROFILE {
            return Err(LlmAttestedError::ManifestProfileMismatch {
                expected: MANIFEST_PROFILE.to_string(),
                got: manifest.profile,
            });
        }
        Ok(manifest)
    }

    pub fn hash(&self) -> Result<String> {
        Ok(sha256_prefixed(&self.to_cbor()?))
    }

    pub fn event_verifying_key(&self) -> Result<VerifyingKey> {
        self.event_signing_jwk.to_verifying_key()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventActor {
    pub kind: String,
    pub agent_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContestEvent {
    pub version: u32,
    pub profile: String,
    pub event_id: String,
    pub kind: String,
    pub hackathon_id: String,
    pub team_id: String,
    pub gateway_id: String,
    pub manifest_hash: String,
    pub capture_method: String,
    pub assurance: String,
    pub event_index: u64,
    pub previous_team_event_hash: String,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    pub actor: EventActor,
    pub subject: BTreeMap<String, String>,
    pub metrics: BTreeMap<String, i64>,
    pub hashes: BTreeMap<String, String>,
    pub evidence: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature: String,
}

impl ContestEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_id: String,
        kind: String,
        hackathon_id: String,
        team_id: String,
        gateway_id: String,
        manifest_hash: String,
        capture_method: String,
        assurance: String,
        event_index: u64,
        previous_team_event_hash: String,
        started_at_ms: u64,
        completed_at_ms: u64,
        actor: EventActor,
        subject: BTreeMap<String, String>,
        metrics: BTreeMap<String, i64>,
        hashes: BTreeMap<String, String>,
        evidence: BTreeMap<String, String>,
    ) -> Self {
        Self {
            version: EVENT_VERSION,
            profile: EVENT_PROFILE.to_string(),
            event_id,
            kind,
            hackathon_id,
            team_id,
            gateway_id,
            manifest_hash,
            capture_method,
            assurance,
            event_index,
            previous_team_event_hash,
            started_at_ms,
            completed_at_ms,
            actor,
            subject,
            metrics,
            hashes,
            evidence,
            signature: String::new(),
        }
    }

    pub fn to_cbor(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        ciborium::ser::into_writer(self, &mut out)
            .map_err(|e| LlmAttestedError::Encode(e.to_string()))?;
        Ok(out)
    }

    pub fn from_cbor(bytes: &[u8]) -> Result<Self> {
        let event: Self = ciborium::de::from_reader(bytes)
            .map_err(|e| LlmAttestedError::Decode(e.to_string()))?;
        if event.version != EVENT_VERSION {
            return Err(LlmAttestedError::EventVersionMismatch {
                expected: EVENT_VERSION,
                got: event.version,
            });
        }
        if event.profile != EVENT_PROFILE {
            return Err(LlmAttestedError::EventProfileMismatch {
                expected: EVENT_PROFILE.to_string(),
                got: event.profile,
            });
        }
        Ok(event)
    }

    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        let mut unsigned = self.clone();
        unsigned.signature.clear();
        unsigned.to_cbor()
    }

    pub fn sign(&mut self, key: &SigningKey) -> Result<()> {
        let sig = key.sign(&self.signing_bytes()?);
        self.signature = URL_SAFE_NO_PAD.encode(sig.to_bytes());
        Ok(())
    }

    pub fn verify(&self, key: &VerifyingKey) -> Result<()> {
        let sig_bytes = URL_SAFE_NO_PAD
            .decode(self.signature.as_bytes())
            .map_err(|e| LlmAttestedError::SignatureDecode(e.to_string()))?;
        let sig_array: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| LlmAttestedError::SignatureDecode("expected 64 bytes".to_string()))?;
        let sig = Signature::from_bytes(&sig_array);
        key.verify(&self.signing_bytes()?, &sig)
            .map_err(|_| LlmAttestedError::SignatureVerify)
    }

    pub fn event_hash(&self) -> Result<String> {
        Ok(sha256_prefixed(&self.to_cbor()?))
    }

    pub fn total_tokens(&self) -> i64 {
        self.metrics.get("total_tokens").copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScoreReport {
    pub team_id: String,
    pub rank: u32,
    pub score: i64,
    pub unit: String,
    pub status: String,
    pub explanations: Vec<String>,
    pub receipt_refs: Vec<String>,
}

pub fn score_min_tokens(events: &[ContestEvent]) -> Vec<ScoreReport> {
    let mut totals = totals_by_team(events);
    totals.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    totals
        .into_iter()
        .enumerate()
        .map(|(idx, (team_id, total))| ScoreReport {
            team_id,
            rank: (idx + 1) as u32,
            score: total,
            unit: "tokens".to_string(),
            status: "eligible".to_string(),
            explanations: vec![format!("{total} total tokens")],
            receipt_refs: Vec::new(),
        })
        .collect()
}

pub fn score_max_tokens_under_budget(events: &[ContestEvent], budget: i64) -> Vec<ScoreReport> {
    let mut totals = totals_by_team(events);
    totals.sort_by(|a, b| {
        let a_ok = a.1 <= budget;
        let b_ok = b.1 <= budget;
        b_ok.cmp(&a_ok)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.cmp(&b.0))
    });

    totals
        .into_iter()
        .enumerate()
        .map(|(idx, (team_id, total))| {
            let status = if total <= budget {
                "eligible"
            } else {
                "ineligible"
            };
            ScoreReport {
                team_id,
                rank: (idx + 1) as u32,
                score: total,
                unit: "tokens".to_string(),
                status: status.to_string(),
                explanations: vec![format!("{total} total tokens; budget {budget}")],
                receipt_refs: Vec::new(),
            }
        })
        .collect()
}

pub fn distinct_agent_count(events: &[ContestEvent], team_id: &str) -> usize {
    events
        .iter()
        .filter(|e| e.team_id == team_id)
        .map(|e| e.actor.agent_session_id.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

pub fn sha256_prefixed(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn random_id(prefix: &str) -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{prefix}_{}", hex::encode(bytes))
}

fn totals_by_team(events: &[ContestEvent]) -> Vec<(String, i64)> {
    let mut totals = BTreeMap::<String, i64>::new();
    for event in events.iter().filter(|e| e.kind == "llm.call") {
        *totals.entry(event.team_id.clone()).or_default() += event.total_tokens();
    }
    totals.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn sample_manifest(key: &VerifyingKey) -> ContestManifest {
        ContestManifest::new(
            "https://events.example".to_string(),
            "hk_test".to_string(),
            "gw_test".to_string(),
            "https://events.example/e/hk_test".to_string(),
            "https://gateway.example/.well-known/llm-attested/manifest.cbor".to_string(),
            "sha256:receipt".to_string(),
            "sha384:valuex".to_string(),
            "sha256:policy".to_string(),
            "sha256:scorer".to_string(),
            "routed".to_string(),
            "https://events.example/e/hk_test/join".to_string(),
            vec!["gateway_proxy".to_string()],
            TrustPolicy::self_hosted(
                vec![
                    "tdx".to_string(),
                    "sev-snp".to_string(),
                    "nitro".to_string(),
                ],
                "https://gateway.example/.well-known/cvm/receipt".to_string(),
                "https://events.example/.well-known/cvm/registry.json".to_string(),
            ),
            key,
            vec!["llm.call".to_string()],
        )
    }

    fn sample_event(team_id: &str, total_tokens: i64) -> ContestEvent {
        let mut metrics = BTreeMap::new();
        metrics.insert("total_tokens".to_string(), total_tokens);

        ContestEvent::new(
            random_id("evt"),
            "llm.call".to_string(),
            "hk_test".to_string(),
            team_id.to_string(),
            "gw_test".to_string(),
            "sha256:manifest".to_string(),
            "gateway_proxy".to_string(),
            "routed".to_string(),
            0,
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            1,
            2,
            EventActor {
                kind: "agent".to_string(),
                agent_session_id: "agent_test".to_string(),
            },
            BTreeMap::new(),
            metrics,
            BTreeMap::new(),
            BTreeMap::new(),
        )
    }

    #[test]
    fn manifest_cbor_roundtrips_and_hashes() {
        let key = signing_key();
        let manifest = sample_manifest(&key.verifying_key());
        let cbor = manifest.to_cbor().unwrap();
        let decoded = ContestManifest::from_cbor(&cbor).unwrap();
        assert_eq!(manifest, decoded);
        assert!(manifest.hash().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn event_signs_and_verifies() {
        let key = signing_key();
        let mut event = sample_event("team_a", 42);
        event.sign(&key).unwrap();
        event.verify(&key.verifying_key()).unwrap();

        let mut tampered = event.clone();
        tampered.metrics.insert("total_tokens".to_string(), 43);
        assert!(tampered.verify(&key.verifying_key()).is_err());
    }

    #[test]
    fn min_token_scorer_orders_lowest_first() {
        let events = vec![sample_event("team_b", 10), sample_event("team_a", 5)];
        let scores = score_min_tokens(&events);
        assert_eq!(scores[0].team_id, "team_a");
        assert_eq!(scores[0].score, 5);
        assert_eq!(scores[1].team_id, "team_b");
    }
}
