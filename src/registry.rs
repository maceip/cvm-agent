//! Registry of approved Value X entries.
//!
//! A registry entry says: "this Value X has been reviewed and is approved
//! for some status (recommended / deprecated / revoked)." Entries live as
//! JSON files in a `registry/` directory — same-repo today.
//!
//! ## Trust roots
//!
//! Entries are signed. There is no single global signer — this is not
//! a CI-only system. A `TrustRoot` is a configuration that says "these
//! signer identities are trusted." Our own project ships with a default
//! `TrustRoot` pointing at our GitHub workflow, but a downstream user
//! (e.g., a JS developer running their webserver in a TEE) configures
//! their own `TrustRoot` that trusts their signer. The registry format
//! does not change; only the set of accepted identities does.
//!
//! Signatures are verified offline (see `sigverify`): an entry is `Verified`
//! only if a pinned trusted identity validates its detached signature, else
//! `Untrusted`. There is no path that accepts an unverified sidecar.
//!
//! See `registry/README.md` for the schema and trust model.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Recommended,
    Deprecated,
    Revoked,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlatformMeasurements {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nitro_pcr0: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tdx_mrtd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snp_measurement: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub value_x: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
    #[serde(default)]
    pub platform_measurements: PlatformMeasurements,
    pub status: Status,
    pub approved_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// Result of looking up a Value X in the registry.
#[derive(Debug, Clone)]
pub enum Lookup {
    /// Entry found and signature verified (or signature verification skipped).
    Found {
        entry: Entry,
        signature: SignatureState,
    },
    /// Value X is not in the registry at all.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureState {
    /// Sidecar present and signature valid against a pinned trusted identity.
    Verified,
    /// Sidecar present but no trusted identity verified it (bad signature,
    /// unknown signer, or an empty trust root). NOT trustworthy — fail closed.
    Untrusted,
    /// Sidecar missing.
    Missing,
}

/// Identity of a trusted signer. Describes *who* is allowed to sign
/// registry entries for a given consumer of the registry. Having this
/// as data (not a hardcoded constant) is the hinge that keeps the
/// system from collapsing into "GitHub CI only."
#[derive(Debug, Clone)]
pub enum TrustedIdentity {
    /// Sigstore keyless signer, pinned by Fulcio cert subject.
    /// Matches any workflow identity matching (issuer, subject_pattern).
    /// `subject_pattern` is a glob — e.g.,
    ///   `https://github.com/maceip/cvm-agent/.github/workflows/registry-sign.yml@refs/heads/main`
    /// or a looser match for downstream users.
    SigstoreKeyless {
        issuer: String,
        subject_pattern: String,
    },
    /// Raw public key (ed25519 or ecdsa). For offline / YubiKey signers
    /// who don't want GitHub or Sigstore in their trust chain.
    RawPublicKey {
        algorithm: String, // "ed25519" | "ecdsa-p256"
        spki_der: Vec<u8>,
        label: String, // human-readable, shown on verify
    },
}

/// A set of trusted identities. An entry is accepted if *any* identity
/// in the trust root successfully verifies its signature. The set can
/// be empty — in which case the registry is informational only and
/// every entry comes back as `SignatureState::Missing`.
#[derive(Debug, Clone, Default)]
pub struct TrustRoot {
    pub identities: Vec<TrustedIdentity>,
}

impl TrustRoot {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with(mut self, id: TrustedIdentity) -> Self {
        self.identities.push(id);
        self
    }

    /// The project's own default trust root: our GitHub workflow signing
    /// via Sigstore keyless. Downstream users should NOT use this — they
    /// should build their own `TrustRoot` pointing at their own signers.
    /// This exists so `cvm check` of our own runner works out of
    /// the box without a config file.
    pub fn cvms_default() -> Self {
        Self::empty().with(TrustedIdentity::SigstoreKeyless {
            issuer: "https://token.actions.githubusercontent.com".to_string(),
            subject_pattern:
                "https://github.com/maceip/cvm-agent/.github/workflows/registry-sign.yml@refs/heads/main"
                    .to_string(),
        })
    }
}

pub struct Registry {
    entries: HashMap<String, (Entry, SignatureState)>,
    trust_root: TrustRoot,
}

impl Registry {
    /// Load every `*.json` in the given directory as a registry entry.
    /// Files named `README.md` or `*.sig` are skipped. Signatures are
    /// checked against the provided `trust_root`.
    pub fn load(dir: &Path, trust_root: TrustRoot) -> anyhow::Result<Self> {
        let mut entries = HashMap::new();
        if !dir.exists() {
            return Ok(Self {
                entries,
                trust_root,
            });
        }
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let body = std::fs::read_to_string(&path)?;
            let entry: Entry = serde_json::from_str(&body)
                .map_err(|err| anyhow::anyhow!("{}: {err}", path.display()))?;
            let sig_state = Self::check_sidecar(&path, &body, &trust_root)?;
            entries.insert(entry.value_x.clone(), (entry, sig_state));
        }
        Ok(Self {
            entries,
            trust_root,
        })
    }

    /// Default load: look for the project's registry directory and use
    /// the project's default `TrustRoot`. This is the path
    /// `cvm check` takes when no config is provided.
    pub fn load_default() -> anyhow::Result<Self> {
        let trust_root = TrustRoot::cvms_default();
        let mut merged = Self {
            entries: HashMap::new(),
            trust_root: trust_root.clone(),
        };

        for c in [PathBuf::from("registry")] {
            if c.exists() && c.is_dir() {
                let loaded = Self::load(&c, trust_root.clone())?;
                merged.entries.extend(loaded.entries);
            }
        }

        // Older releases kept a root `registry.json` containing `{ entries: [...] }`.
        // Keep reading it for compatibility with cloned historical trees.
        for c in [PathBuf::from("registry.json")] {
            if c.exists() && c.is_file() {
                merged.load_legacy_registry_json(&c)?;
            }
        }

        Ok(merged)
    }

    pub fn lookup(&self, value_x_hex: &str) -> Lookup {
        match self.entries.get(value_x_hex) {
            Some((entry, sig)) => Lookup::Found {
                entry: entry.clone(),
                signature: *sig,
            },
            None => Lookup::Unknown,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn trust_root(&self) -> &TrustRoot {
        &self.trust_root
    }

    fn check_sidecar(
        json_path: &Path,
        body: &str,
        trust_root: &TrustRoot,
    ) -> anyhow::Result<SignatureState> {
        let sig_path = {
            let mut p = json_path.as_os_str().to_owned();
            p.push(".sig");
            PathBuf::from(p)
        };
        if !sig_path.exists() {
            return Ok(SignatureState::Missing);
        }
        let raw = std::fs::read(&sig_path)?;
        let sidecar = match sigverify::Sidecar::parse(&raw) {
            Some(s) => s,
            None => {
                eprintln!(
                    "[cvm] Registry: {} present but unparseable",
                    sig_path.display()
                );
                return Ok(SignatureState::Untrusted);
            }
        };
        // An entry is accepted only if SOME identity in the trust root verifies
        // it. Otherwise it is Untrusted — there is no "present, so trust it".
        for id in &trust_root.identities {
            if sigverify::verify_against(id, body.as_bytes(), &sidecar) {
                return Ok(SignatureState::Verified);
            }
        }
        Ok(SignatureState::Untrusted)
    }

    fn load_legacy_registry_json(&mut self, path: &Path) -> anyhow::Result<()> {
        let body = std::fs::read_to_string(path)?;
        let root: serde_json::Value = serde_json::from_str(&body)
            .map_err(|err| anyhow::anyhow!("{}: {err}", path.display()))?;
        let entries = root
            .get("entries")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("{}: missing entries array", path.display()))?;

        for raw in entries {
            let value_x = raw
                .get("value_x")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("{}: entry missing value_x", path.display()))?
                .to_string();
            let deprecated = raw
                .get("deprecated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let recommended = raw
                .get("recommended")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let status = if deprecated {
                Status::Deprecated
            } else if recommended {
                Status::Recommended
            } else {
                Status::Revoked
            };
            let pm = raw
                .get("platform_measurements")
                .cloned()
                .unwrap_or_default();
            let entry = Entry {
                value_x: value_x.clone(),
                source_commit: raw
                    .get("source_commit")
                    .or_else(|| raw.get("git_commit"))
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string),
                platform_measurements: PlatformMeasurements {
                    nitro_pcr0: pm
                        .get("nitro_pcr0")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string),
                    tdx_mrtd: pm
                        .get("tdx_mrtd")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string),
                    snp_measurement: pm
                        .get("snp_measurement")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string),
                },
                status,
                approved_at: raw
                    .get("approved_at")
                    .or_else(|| raw.get("registered_at"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                deprecated_at: None,
                revoked_at: None,
                notes: raw
                    .get("notes")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            };

            self.entries
                .entry(value_x)
                .or_insert((entry, SignatureState::Missing));
        }

        Ok(())
    }
}

/// Human-readable summary of a lookup result. Used by `cvm check`.
pub fn describe(lookup: &Lookup) -> String {
    match lookup {
        Lookup::Found { entry, signature } => {
            let sig = match signature {
                SignatureState::Verified => "signed",
                SignatureState::Untrusted => "signature present but UNTRUSTED",
                SignatureState::Missing => "UNSIGNED",
            };
            let status = match entry.status {
                Status::Recommended => "RECOMMENDED",
                Status::Deprecated => "DEPRECATED",
                Status::Revoked => "REVOKED",
            };
            format!("{status} ({sig}) — approved {}", entry.approved_at)
        }
        Lookup::Unknown => "UNKNOWN (not in registry)".to_string(),
    }
}

/// Detached-signature sidecar verification.
///
/// Verification is fully offline. For `SigstoreKeyless` identities we bind the
/// leaf certificate's identity (SAN URI + Fulcio OIDC-issuer extension) to the
/// pinned `(issuer, subject_pattern)` and verify the detached signature with
/// the leaf key. A sidecar that does not verify against any pinned identity is
/// `Untrusted` — there is no path that accepts an unverified signature.
mod sigverify {
    use super::TrustedIdentity;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use der::{Decode, Encode};

    /// Parsed sidecar: a detached signature plus optional algorithm hint and a
    /// leaf certificate (PEM) for keyless identities.
    pub struct Sidecar {
        pub sig: Vec<u8>,
        pub cert_pem: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct SidecarJson {
        sig: String,
        #[serde(default)]
        cert: Option<String>,
    }

    impl Sidecar {
        /// Parse either the JSON object form or a bare base64 detached signature.
        pub fn parse(raw: &[u8]) -> Option<Self> {
            let text = std::str::from_utf8(raw).ok()?.trim();
            if text.starts_with('{') {
                let j: SidecarJson = serde_json::from_str(text).ok()?;
                let sig = B64.decode(j.sig.trim()).ok()?;
                return Some(Sidecar {
                    sig,
                    cert_pem: j.cert,
                });
            }
            let sig = B64.decode(text).ok()?;
            Some(Sidecar {
                sig,
                cert_pem: None,
            })
        }
    }

    /// Does `id` verify `sidecar` over `msg` (the exact entry json bytes)?
    pub fn verify_against(id: &TrustedIdentity, msg: &[u8], sidecar: &Sidecar) -> bool {
        match id {
            TrustedIdentity::RawPublicKey { spki_der, .. } => {
                verify_sig_with_spki(spki_der, msg, &sidecar.sig)
            }
            TrustedIdentity::SigstoreKeyless {
                issuer,
                subject_pattern,
            } => {
                let pem = match &sidecar.cert_pem {
                    Some(p) => p,
                    None => return false,
                };
                let der = match pem_to_der(pem) {
                    Some(d) => d,
                    None => return false,
                };
                let cert = match x509_cert::Certificate::from_der(&der) {
                    Ok(c) => c,
                    Err(_) => return false,
                };
                let spki = match cert.tbs_certificate.subject_public_key_info.to_der() {
                    Ok(s) => s,
                    Err(_) => return false,
                };
                if !verify_sig_with_spki(&spki, msg, &sidecar.sig) {
                    return false;
                }
                cert_identity_matches(&cert, issuer, subject_pattern)
            }
        }
    }

    /// Auto-detect the key type from the SPKI algorithm OID and verify a
    /// detached signature over `msg`. Supports ed25519 and ECDSA P-256.
    fn verify_sig_with_spki(spki_der: &[u8], msg: &[u8], sig: &[u8]) -> bool {
        let spki = match spki::SubjectPublicKeyInfoRef::from_der(spki_der) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let oid = spki.algorithm.oid.to_string();
        let key = match spki.subject_public_key.as_bytes() {
            Some(k) => k,
            None => return false,
        };
        match oid.as_str() {
            "1.3.101.112" => verify_ed25519(key, msg, sig),
            "1.2.840.10045.2.1" => verify_ecdsa_p256(key, msg, sig),
            _ => false,
        }
    }

    fn verify_ed25519(key: &[u8], msg: &[u8], sig: &[u8]) -> bool {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let key: [u8; 32] = match key.try_into() {
            Ok(k) => k,
            Err(_) => return false,
        };
        let vk = match VerifyingKey::from_bytes(&key) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let sig: [u8; 64] = match sig.try_into() {
            Ok(s) => s,
            Err(_) => return false,
        };
        vk.verify(msg, &Signature::from_bytes(&sig)).is_ok()
    }

    fn verify_ecdsa_p256(sec1_point: &[u8], msg: &[u8], sig: &[u8]) -> bool {
        use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
        let vk = match VerifyingKey::from_sec1_bytes(sec1_point) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let parsed = Signature::from_der(sig).or_else(|_| Signature::from_slice(sig));
        match parsed {
            Ok(s) => vk.verify(msg, &s).is_ok(),
            Err(_) => false,
        }
    }

    /// PEM (`-----BEGIN CERTIFICATE-----`) to DER.
    fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
        let mut b64 = String::new();
        let mut in_block = false;
        for line in pem.lines() {
            let t = line.trim();
            if t.starts_with("-----BEGIN") {
                in_block = true;
                continue;
            }
            if t.starts_with("-----END") {
                break;
            }
            if in_block {
                b64.push_str(t);
            }
        }
        if b64.is_empty() {
            return None;
        }
        B64.decode(b64).ok()
    }

    /// Match a Fulcio leaf certificate against the pinned `(issuer,
    /// subject_pattern)`: the SAN URI must glob-match `subject_pattern`, and
    /// the OIDC issuer extension must equal `issuer`.
    fn cert_identity_matches(
        cert: &x509_cert::Certificate,
        issuer: &str,
        subject_pattern: &str,
    ) -> bool {
        let exts = match &cert.tbs_certificate.extensions {
            Some(e) => e,
            None => return false,
        };
        let mut san_ok = false;
        let mut issuer_ok = false;
        for ext in exts.iter() {
            match ext.extn_id.to_string().as_str() {
                "2.5.29.17" => {
                    if let Ok(san) =
                        x509_cert::ext::pkix::SubjectAltName::from_der(ext.extn_value.as_bytes())
                    {
                        for gn in san.0.iter() {
                            if let x509_cert::ext::pkix::name::GeneralName::UniformResourceIdentifier(
                                uri,
                            ) = gn
                            {
                                if glob_match(subject_pattern, uri.as_str()) {
                                    san_ok = true;
                                }
                            }
                        }
                    }
                }
                "1.3.6.1.4.1.57264.1.1" => {
                    if std::str::from_utf8(ext.extn_value.as_bytes())
                        .map(|s| s == issuer)
                        .unwrap_or(false)
                    {
                        issuer_ok = true;
                    }
                }
                "1.3.6.1.4.1.57264.1.8" => {
                    if let Ok(s) = der::asn1::Utf8StringRef::from_der(ext.extn_value.as_bytes()) {
                        if s.as_str() == issuer {
                            issuer_ok = true;
                        }
                    }
                }
                _ => {}
            }
        }
        san_ok && issuer_ok
    }

    /// Minimal glob: `*` matches any run of characters. Registry signer
    /// patterns only ever use `*`.
    pub fn glob_match(pattern: &str, value: &str) -> bool {
        fn rec(p: &[u8], v: &[u8]) -> bool {
            if p.is_empty() {
                return v.is_empty();
            }
            if p[0] == b'*' {
                let mut i = 1;
                while i < p.len() && p[i] == b'*' {
                    i += 1;
                }
                let rest = &p[i..];
                if rest.is_empty() {
                    return true;
                }
                for k in 0..=v.len() {
                    if rec(rest, &v[k..]) {
                        return true;
                    }
                }
                false
            } else if !v.is_empty() && p[0] == v[0] {
                rec(&p[1..], &v[1..])
            } else {
                false
            }
        }
        rec(pattern.as_bytes(), value.as_bytes())
    }

    /// SPKI DER for an ed25519 raw public key (RFC 8410 prefix + key).
    #[cfg(test)]
    pub fn ed25519_spki(pubkey: &[u8; 32]) -> Vec<u8> {
        let mut der = vec![
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ];
        der.extend_from_slice(pubkey);
        der
    }
}

#[cfg(test)]
mod sig_tests {
    use super::sigverify::{ed25519_spki, glob_match, verify_against, Sidecar};
    use super::TrustedIdentity;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use ed25519_dalek::{Signer, SigningKey};

    fn raw_id(spki: Vec<u8>) -> TrustedIdentity {
        TrustedIdentity::RawPublicKey {
            algorithm: "ed25519".into(),
            spki_der: spki,
            label: "test".into(),
        }
    }

    #[test]
    fn raw_ed25519_verifies_and_rejects_tamper() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let body = b"registry entry bytes";
        let sig = sk.sign(body).to_bytes().to_vec();
        let sidecar = Sidecar::parse(B64.encode(&sig).as_bytes()).unwrap();
        let id = raw_id(ed25519_spki(&sk.verifying_key().to_bytes()));
        assert!(verify_against(&id, body, &sidecar));
        assert!(!verify_against(&id, b"different bytes", &sidecar));
    }

    #[test]
    fn glob_matches_workflow_subjects() {
        assert!(glob_match("https://github.com/org/*/.github/*", "https://github.com/org/repo/.github/workflows/sign.yml"));
        assert!(!glob_match("https://github.com/org/*", "https://evil.example/org/x"));
    }
}
