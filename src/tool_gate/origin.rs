//! eat-pass PrivateToken verification + central redeem for privileged tools.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use eat_pass_core::transparency::verify_log;
use eat_pass_core::{http, IssuerPublicKey, TokenChallenge, Verifier};

use crate::tool_gate::intent::email_redemption_context_bound;
use crate::tool_gate::wire::{KtResponse, RedeemBody};

const MAX_RECENT_CHALLENGES: usize = 128;

struct KeyEntry {
    verifier: Verifier,
    epoch: u32,
}

pub struct EatPassGate {
    issuer_base: String,
    issuer_name: String,
    origin_info: String,
    kt_log_pub: [u8; 32],
    redeemer: String,
    keys: RwLock<HashMap<[u8; 32], Arc<KeyEntry>>>,
    current_pk: IssuerPublicKey,
    recent_challenges: RwLock<Vec<TokenChallenge>>,
    http: reqwest::Client,
}

#[derive(Debug, thiserror::Error)]
pub enum GateReject {
    #[error("missing PrivateToken")]
    MissingToken,
    #[error("malformed token: {0}")]
    Malformed(String),
    #[error("unknown issuer key")]
    UnknownKey,
    #[error("token rejected: {0}")]
    InvalidToken(String),
    #[error("token challenge unknown or expired — fetch a fresh challenge from the gate")]
    UnknownChallenge,
    #[error("token already spent")]
    DoubleSpend,
    #[error("redeemer unavailable")]
    RedeemerDown,
}

enum SpendOutcome {
    Ok,
    DoubleSpend,
    RedeemerDown,
}

impl EatPassGate {
    pub async fn bootstrap(
        issuer_url: &str,
        issuer_name: &str,
        origin_info: &str,
        redeemer_url: &str,
        kt_log_pub: [u8; 32],
        insecure_tls: bool,
    ) -> anyhow::Result<Self> {
        let base = issuer_url.trim_end_matches('/').to_string();
        let mut builder = reqwest::Client::builder();
        if insecure_tls {
            builder = builder.danger_accept_invalid_certs(true);
        }
        let http = builder.build()?;
        let pk: IssuerPublicKey = http
            .get(format!("{base}/keys"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let cur_epoch = pk.key_version;
        let cur_tkid = pk.token_key_id()?;
        let mut keys = HashMap::new();
        keys.insert(
            cur_tkid,
            Arc::new(KeyEntry {
                verifier: Verifier::new(pk.clone()),
                epoch: cur_epoch,
            }),
        );
        Ok(Self {
            issuer_base: base,
            issuer_name: issuer_name.to_string(),
            origin_info: origin_info.to_string(),
            kt_log_pub,
            redeemer: redeemer_url.trim_end_matches('/').to_string(),
            keys: RwLock::new(keys),
            current_pk: pk,
            recent_challenges: RwLock::new(Vec::new()),
            http,
        })
    }

    pub fn issuer_name(&self) -> &str {
        &self.issuer_name
    }

    pub fn origin_info(&self) -> &str {
        &self.origin_info
    }

    pub fn issuer_url(&self) -> &str {
        &self.issuer_base
    }

    pub fn redeemer_url(&self) -> &str {
        &self.redeemer
    }

    pub fn kt_log_pub_hex(&self) -> String {
        hex::encode(self.kt_log_pub)
    }

    pub fn www_authenticate_for(
        &self,
        challenge: &TokenChallenge,
    ) -> Result<String, eat_pass_core::Error> {
        http::www_authenticate(&challenge.to_bytes(), &self.current_pk)
    }

    /// Issue a gate-signed email challenge with server nonce + intent binding.
    pub fn issue_email_challenge(
        &self,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<(TokenChallenge, String), eat_pass_core::Error> {
        let server_nonce = local_nonce();
        let redemption = email_redemption_context_bound(&server_nonce, to, subject, body);
        let ch = TokenChallenge::new(&self.issuer_name, &self.origin_info)
            .with_redemption_context(redemption);
        self.register_challenge(ch.clone());
        let www = http::www_authenticate(&ch.to_bytes(), &self.current_pk)?;
        Ok((ch, www))
    }

    fn register_challenge(&self, ch: TokenChallenge) {
        let mut recent = self.recent_challenges.write().unwrap();
        recent.push(ch);
        if recent.len() > MAX_RECENT_CHALLENGES {
            let drop = recent.len() - MAX_RECENT_CHALLENGES;
            recent.drain(0..drop);
        }
    }

    fn challenge_for_digest(&self, digest: &[u8; 32]) -> Option<TokenChallenge> {
        self.recent_challenges
            .read()
            .unwrap()
            .iter()
            .find(|c| c.digest() == *digest)
            .cloned()
    }

    pub async fn verify_and_spend(
        &self,
        auth_header: Option<&str>,
        challenge_digest: &[u8; 32],
    ) -> Result<[u8; 32], GateReject> {
        let auth = auth_header.ok_or(GateReject::MissingToken)?;
        let token = http::parse_authorization(auth)
            .map_err(|e| GateReject::Malformed(e.to_string()))?;
        if token.challenge_digest != *challenge_digest {
            return Err(GateReject::InvalidToken(
                "token challenge_digest does not match issued challenge".into(),
            ));
        }
        let challenge = self
            .challenge_for_digest(challenge_digest)
            .ok_or(GateReject::UnknownChallenge)?;
        let entry = self
            .key_for(&token.token_key_id)
            .await
            .ok_or(GateReject::UnknownKey)?;
        let nonce = entry
            .verifier
            .verify(&token, &challenge)
            .map_err(|e| GateReject::InvalidToken(e.to_string()))?;
        match self.spend_centrally(entry.epoch, &nonce).await {
            SpendOutcome::Ok => Ok(nonce),
            SpendOutcome::DoubleSpend => Err(GateReject::DoubleSpend),
            SpendOutcome::RedeemerDown => Err(GateReject::RedeemerDown),
        }
    }

    async fn key_for(&self, tkid: &[u8; 32]) -> Option<Arc<KeyEntry>> {
        if let Some(e) = self.keys.read().unwrap().get(tkid).cloned() {
            return Some(e);
        }
        match self.resolve_via_kt(tkid).await {
            Ok(Some(entry)) => {
                self.keys.write().unwrap().insert(*tkid, entry.clone());
                Some(entry)
            }
            Ok(None) => None,
            Err(e) => {
                eprintln!("[tool-gate] key resolve failed: {e}");
                None
            }
        }
    }

    async fn resolve_via_kt(&self, tkid: &[u8; 32]) -> anyhow::Result<Option<Arc<KeyEntry>>> {
        let kt: KtResponse = self
            .http
            .get(format!("{}/kt", self.issuer_base))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let served = hex::decode(kt.log_pub.trim()).unwrap_or_default();
        if served.as_slice() != self.kt_log_pub {
            anyhow::bail!("kt log pubkey mismatch");
        }
        verify_log(&self.kt_log_pub, &kt.records, &kt.signed_head)?;
        let want = hex::encode(tkid);
        let Some(rec) = kt
            .records
            .iter()
            .find(|r| r.token_key_id.eq_ignore_ascii_case(&want))
        else {
            return Ok(None);
        };
        let pk: IssuerPublicKey = self
            .http
            .get(format!("{}/keys/{}", self.issuer_base, rec.key_version))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if pk.token_key_id()? != *tkid {
            anyhow::bail!("issuer key disagrees with transparency log");
        }
        Ok(Some(Arc::new(KeyEntry {
            verifier: Verifier::new(pk),
            epoch: rec.key_version,
        })))
    }

    async fn spend_centrally(&self, key_epoch: u32, nonce: &[u8; 32]) -> SpendOutcome {
        let body = RedeemBody {
            key_epoch,
            nonce: hex::encode(nonce),
        };
        match self
            .http
            .post(format!("{}/redeem", self.redeemer))
            .json(&body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => SpendOutcome::Ok,
            Ok(r) if r.status().as_u16() == 409 => SpendOutcome::DoubleSpend,
            Ok(_) => SpendOutcome::RedeemerDown,
            Err(_) => SpendOutcome::RedeemerDown,
        }
    }
}

fn local_nonce() -> [u8; 32] {
    let mut n = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut n);
    n
}
