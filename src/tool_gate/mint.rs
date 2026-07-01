//! Agent-side eat-pass minting for a specific tool intent.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use eat_pass_core::transparency::{verify_inclusion, verify_log};
use eat_pass_core::{http, Client, IssuerPublicKey, SignResponse, TokenChallenge, Token};

use crate::tool_gate::wire::{AuthorizeBody, AuthorizeResponse, KtResponse, SignBody};

pub struct MintConfig {
    pub gate_url: String,
    pub issuer_url: String,
    pub attester_url: String,
    pub issuer_name: String,
    pub origin_info: String,
    pub kt_log_pub: [u8; 32],
    pub uq_collect_cmd: String,
    pub insecure_tls: bool,
}

pub struct MintedToolToken {
    pub token: Token,
    pub authorization_header: String,
    pub binding_hex: String,
    pub challenge_digest_hex: String,
}

pub async fn mint_email_send_token(
    cfg: &MintConfig,
    to: &str,
    subject: &str,
    body: &str,
) -> anyhow::Result<MintedToolToken> {
    let http = http_client(cfg.insecure_tls)?;
    let gate_base = cfg.gate_url.trim_end_matches('/');
    let issuer_base = cfg.issuer_url.trim_end_matches('/');
    let attester_base = cfg.attester_url.trim_end_matches('/');

    let challenge = fetch_email_challenge(&http, gate_base, to, subject, body).await?;
    eprintln!(
        "[cvm tool] gate challenge digest={} redemption_context={}",
        hex::encode(challenge.digest()),
        challenge.has_redemption_context()
    );

    let pk: IssuerPublicKey = http
        .get(format!("{issuer_base}/keys"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let tkid = pk.token_key_id()?;

    let kt: KtResponse = http
        .get(format!("{issuer_base}/kt"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let served = hex::decode(kt.log_pub.trim()).unwrap_or_default();
    if served.as_slice() != cfg.kt_log_pub {
        anyhow::bail!("issuer kt log pubkey does not match pinned key");
    }
    verify_log(&cfg.kt_log_pub, &kt.records, &kt.signed_head)?;
    verify_inclusion(&kt.records, &tkid)?;

    let (req, pending) = Client::begin(&pk, &challenge, 1)?;
    let binding = req.binding();
    let eat = collect_azure_eat(&cfg.uq_collect_cmd, &binding)?;

    let auth_resp: AuthorizeResponse = http
        .post(format!("{attester_base}/authorize"))
        .json(&AuthorizeBody {
            eat_b64: B64.encode(eat),
            binding: hex::encode(binding),
            max_batch: 1,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if let Some(ref appraisal) = auth_resp.appraisal {
        eprintln!(
            "[cvm tool] attester appraisal pass={} policy={} class={}",
            appraisal.pass, appraisal.policy_id, appraisal.class_label
        );
        if let Some(n) = &appraisal.notes {
            eprintln!("[cvm tool]   notes: {n}");
        }
    }

    let sign_resp: SignResponse = http
        .post(format!("{issuer_base}/sign"))
        .json(&SignBody {
            req,
            authorization_b64: auth_resp.authorization_b64,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let tokens = pending.finalize(&pk, &sign_resp)?;
    let token = tokens
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("issuer returned no token"))?;

    Ok(MintedToolToken {
        authorization_header: http::authorization(&token),
        binding_hex: hex::encode(binding),
        challenge_digest_hex: hex::encode(token.challenge_digest),
        token,
    })
}

async fn fetch_email_challenge(
    http: &reqwest::Client,
    gate_base: &str,
    to: &str,
    subject: &str,
    body: &str,
) -> anyhow::Result<TokenChallenge> {
    let mut url = reqwest::Url::parse(&format!("{gate_base}/.well-known/eat-pass/challenge"))
        .map_err(|e| anyhow::anyhow!("gate url: {e}"))?;
    url.query_pairs_mut()
        .append_pair("to", to)
        .append_pair("subject", subject)
        .append_pair("body", body);
    let r = http.get(url.clone()).send().await?;
    if r.status() != reqwest::StatusCode::UNAUTHORIZED {
        anyhow::bail!(
            "expected 401 challenge from gate at {}, got {}",
            url,
            r.status()
        );
    }
    let hdr = r
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("gate challenge missing WWW-Authenticate"))?;
    http::parse_www_authenticate(hdr).map_err(|e| anyhow::anyhow!("parse gate challenge: {e}"))
}

fn http_client(insecure_tls: bool) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();
    if insecure_tls {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().map_err(|e| anyhow::anyhow!("http client: {e}"))
}

fn collect_azure_eat(cmd: &str, binding: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    let mut rnd = [0u8; 8];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut rnd);
    let out = std::env::temp_dir().join(format!("cvm-tool-{}.json", hex::encode(rnd)));
    let out_str = out.to_string_lossy();
    let mut argv = cmd.split_whitespace().collect::<Vec<_>>();
    if argv.is_empty() {
        anyhow::bail!("uq collect command is empty");
    }
    let prog = argv.remove(0);
    let binding_hex = hex::encode(binding);
    let status = std::process::Command::new(prog)
        .args(&argv)
        .args(["--value-x", &binding_hex, "-o", &out_str])
        .status()
        .map_err(|e| anyhow::anyhow!("spawn `{cmd}`: {e}"))?;
    if !status.success() {
        anyhow::bail!("`{cmd}` failed with {status}");
    }
    let bytes = std::fs::read(&out)?;
    let _ = std::fs::remove_file(&out);
    Ok(bytes)
}
