//! Delegation leaf — the positive form of "agents own nothing."
//!
//! Instead of minting a persistent agent identity, every action emits a
//! *leaf* that binds two things and nothing else:
//!
//!   - the **human** who authorized it — an Ed25519 principal plus a
//!     signature over the exact scope, expiry, and code they authorized; and
//!   - the **code** that ran it — its Value X, plus the hash of the EAT that
//!     proves that Value X actually ran inside a genuine TEE.
//!
//! The actor is reconstructed at audit time as `(principal x code)`. There is
//! deliberately no `agent_id` field. Cloning the code is a no-op (same Value
//! X), so there is nothing to enroll, discover, or revoke about "the agent":
//! you revoke the human's delegation (let it expire) or rotate the code (a new
//! Value X). Identity is the human's; reputation is the code's; the agent owns
//! nothing.
//!
//! The design mirrors [`crate::eat::EatToken`]: bare CBOR, integrity by hash
//! binding rather than stacked signature layers, content-addressed.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::eat::EatToken;

pub const LEAF_VERSION: u32 = 1;
pub const LEAF_PROFILE: &str = "https://bountynet.dev/leaf/v1";

#[derive(Debug, thiserror::Error)]
pub enum LeafError {
    #[error("CBOR encode failed: {0}")]
    Encode(String),
    #[error("CBOR decode failed: {0}")]
    Decode(String),
    #[error("invalid principal key")]
    BadPrincipal,
    #[error("signature is not 64 bytes")]
    BadSigLen,
    #[error("delegation signature invalid")]
    BadSignature,
    #[error("delegation expired at {0}")]
    Expired(u64),
    #[error("attestation does not match leaf: {0}")]
    AttestationMismatch(&'static str),
}

/// A human's scoped, time-bounded authorization for a specific code
/// measurement to act. The human signs the binding; the agent is issued
/// nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delegation {
    /// The human principal: an Ed25519 public key (32 bytes).
    #[serde(with = "sb32")]
    pub principal: [u8; 32],
    /// Capability scope, e.g. `"answer:rust"` or `"post:brand/acme"`.
    pub scope: String,
    /// Expiry, unix seconds. `0` means no expiry. Revocation is letting it lapse.
    pub not_after: u64,
    /// Ed25519 signature by `principal` over [`Delegation::binding`]. 64 bytes.
    #[serde(with = "serde_bytes")]
    pub sig: Vec<u8>,
}

impl Delegation {
    /// The bytes the human signs: a domain-separated commitment to the scope,
    /// the expiry, the code being authorized, and the attestation that proves
    /// that code is real.
    pub fn binding(
        &self,
        code_value_x: &[u8; 48],
        attestation_hash: &[u8; 32],
        action_hash: &[u8; 32],
    ) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(b"runcard-leaf-v1");
        h.update((self.scope.len() as u32).to_be_bytes());
        h.update(self.scope.as_bytes());
        h.update(self.not_after.to_be_bytes());
        h.update(self.principal);
        h.update(code_value_x);
        h.update(attestation_hash);
        h.update(action_hash);
        h.finalize().into()
    }
}

/// The leaf itself. Note the absence of any agent identity: the only actors
/// named are the human (`delegation.principal`) and the code (`code_value_x`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationLeaf {
    pub version: u32,
    pub profile: String,
    /// Identity lives here — with the human.
    pub delegation: Delegation,
    /// Reputation/identity of the *code* (Value X, sha384). Not the agent.
    #[serde(with = "sb48")]
    pub code_value_x: [u8; 48],
    /// sha256 of the EAT that proves `code_value_x` ran in a genuine TEE.
    #[serde(with = "sb32")]
    pub attestation_hash: [u8; 32],
    /// sha256 of the action/output this leaf accounts for.
    #[serde(with = "sb32")]
    pub action_hash: [u8; 32],
    /// Issued-at, unix seconds.
    pub iat: u64,
    // No `agent_id`. The actor is (delegation.principal x code_value_x).
}

impl DelegationLeaf {
    /// Issue a leaf: the human (`signer`) authorizes `code_value_x` (proven by
    /// `attestation_hash`) to perform `action_hash` under `scope` until
    /// `not_after`. No agent identity is created.
    pub fn issue(
        signer: &SigningKey,
        scope: impl Into<String>,
        not_after: u64,
        code_value_x: [u8; 48],
        attestation_hash: [u8; 32],
        action_hash: [u8; 32],
    ) -> Self {
        let mut delegation = Delegation {
            principal: signer.verifying_key().to_bytes(),
            scope: scope.into(),
            not_after,
            sig: Vec::new(),
        };
        let binding = delegation.binding(&code_value_x, &attestation_hash, &action_hash);
        let sig: Signature = signer.sign(&binding);
        delegation.sig = sig.to_bytes().to_vec();
        Self {
            version: LEAF_VERSION,
            profile: LEAF_PROFILE.to_string(),
            delegation,
            code_value_x,
            attestation_hash,
            action_hash,
            iat: now(),
        }
    }

    /// Content address of this leaf. There is no agent identity; the leaf is
    /// named only by its contents.
    pub fn leaf_id(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(self.version.to_be_bytes());
        h.update(self.delegation.principal);
        h.update(&self.delegation.sig);
        h.update((self.delegation.scope.len() as u32).to_be_bytes());
        h.update(self.delegation.scope.as_bytes());
        h.update(self.delegation.not_after.to_be_bytes());
        h.update(self.code_value_x);
        h.update(self.attestation_hash);
        h.update(self.action_hash);
        h.update(self.iat.to_be_bytes());
        h.finalize().into()
    }

    /// Verify the human's delegation signature and that it has not expired.
    pub fn verify(&self, now_secs: u64) -> Result<(), LeafError> {
        if self.delegation.not_after != 0 && now_secs > self.delegation.not_after {
            return Err(LeafError::Expired(self.delegation.not_after));
        }
        let vk =
            VerifyingKey::from_bytes(&self.delegation.principal).map_err(|_| LeafError::BadPrincipal)?;
        let sig_bytes: [u8; 64] = self
            .delegation
            .sig
            .as_slice()
            .try_into()
            .map_err(|_| LeafError::BadSigLen)?;
        let sig = Signature::from_bytes(&sig_bytes);
        let binding =
            self.delegation
                .binding(&self.code_value_x, &self.attestation_hash, &self.action_hash);
        vk.verify(&binding, &sig).map_err(|_| LeafError::BadSignature)
    }

    /// Tie "the human authorized this code" to "this code provably ran in a
    /// TEE": the EAT must hash to `attestation_hash` and carry the same
    /// `value_x`.
    pub fn verify_against_attestation(&self, eat: &EatToken) -> Result<(), LeafError> {
        let cbor = eat.to_cbor().map_err(|e| LeafError::Encode(e.to_string()))?;
        let h: [u8; 32] = Sha256::digest(&cbor).into();
        if h != self.attestation_hash {
            return Err(LeafError::AttestationMismatch("attestation_hash"));
        }
        if eat.value_x != self.code_value_x {
            return Err(LeafError::AttestationMismatch("value_x"));
        }
        Ok(())
    }

    pub fn to_cbor(&self) -> Result<Vec<u8>, LeafError> {
        let mut out = Vec::new();
        ciborium::ser::into_writer(self, &mut out).map_err(|e| LeafError::Encode(e.to_string()))?;
        Ok(out)
    }

    pub fn from_cbor(bytes: &[u8]) -> Result<Self, LeafError> {
        ciborium::de::from_reader(bytes).map_err(|e| LeafError::Decode(e.to_string()))
    }
}

/// A fresh Ed25519 principal. Seeded from the OS RNG via `rand` (the same
/// crate the build path uses), avoiding a `rand_core` version coupling with
/// `ed25519-dalek::generate`.
pub fn random_principal() -> SigningKey {
    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    SigningKey::from_bytes(&seed)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

mod sb32 {
    use serde::{Deserialize, Serialize, Deserializer, Serializer};
    use serde_bytes::{ByteBuf, Bytes};
    pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        Bytes::new(v.as_slice()).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let b = ByteBuf::deserialize(d)?;
        b.into_vec()
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 32 bytes"))
    }
}

mod sb48 {
    use serde::{Deserialize, Serialize, Deserializer, Serializer};
    use serde_bytes::{ByteBuf, Bytes};
    pub fn serialize<S: Serializer>(v: &[u8; 48], s: S) -> Result<S::Ok, S::Error> {
        Bytes::new(v.as_slice()).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 48], D::Error> {
        let b = ByteBuf::deserialize(d)?;
        b.into_vec()
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 48 bytes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_verify_roundtrip() {
        let k = random_principal();
        let leaf = DelegationLeaf::issue(&k, "answer:rust", 0, [7u8; 48], [9u8; 32], [3u8; 32]);
        assert!(leaf.verify(now()).is_ok());
        let bytes = leaf.to_cbor().unwrap();
        let back = DelegationLeaf::from_cbor(&bytes).unwrap();
        assert_eq!(back.leaf_id(), leaf.leaf_id());
        assert!(back.verify(now()).is_ok());
    }

    #[test]
    fn tampered_scope_fails() {
        let k = random_principal();
        let mut leaf = DelegationLeaf::issue(&k, "answer:rust", 0, [7u8; 48], [9u8; 32], [3u8; 32]);
        leaf.delegation.scope = "answer:everything".to_string();
        assert!(matches!(leaf.verify(now()), Err(LeafError::BadSignature)));
    }

    #[test]
    fn expired_fails() {
        let k = random_principal();
        let leaf = DelegationLeaf::issue(&k, "answer:rust", 1, [7u8; 48], [9u8; 32], [3u8; 32]);
        assert!(matches!(leaf.verify(now()), Err(LeafError::Expired(_))));
    }

    #[test]
    fn no_agent_id_field() {
        // The serialized leaf must not contain an agent identity. This test
        // is the executable form of the thesis.
        let k = random_principal();
        let leaf = DelegationLeaf::issue(&k, "answer:rust", 0, [1u8; 48], [2u8; 32], [3u8; 32]);
        let json = format!("{leaf:?}");
        assert!(!json.contains("agent_id"));
    }
}
