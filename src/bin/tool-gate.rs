#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use cvm_agent::http_service::{
    serve_hyper, write_json, write_response, write_response_with_headers, BufferedResponse as HttpConn,
    CorsHeaders, HandlerFuture, HttpRequest, HttpServerConfig,
};
use cvm_agent::tool_gate::{
    EmailSendRequest, EmailSendResponse, EatPassGate, GateReject, Mailer, SmtpConfig,
    ToolChallengeInfo, TOOL_EMAIL_SEND,
};
use eat_pass_core::http;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

struct ToolGateState {
    gate: EatPassGate,
    mailer: Mailer,
    attester_url: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = parse_args()?;
    let kt_log_pub = parse_hex32(&cfg.kt_log_pub, "kt-log-pub")?;

    let gate = EatPassGate::bootstrap(
        &cfg.issuer,
        &cfg.issuer_name,
        &cfg.origin_info,
        &cfg.redeemer,
        kt_log_pub,
        cfg.insecure_tls,
    )
    .await
    .context("bootstrap eat-pass gate")?;

    let smtp = SmtpConfig::from_env()?;
    eprintln!("[tool-gate] SMTP {}:{} as {}", smtp.host, smtp.port, smtp.from);
    if smtp.dry_run {
        eprintln!("[tool-gate] TOOL_GATE_DRY_RUN=1 — emails are logged, not sent");
    }
    if !smtp.allowed_to.is_empty() {
        eprintln!(
            "[tool-gate] recipient allowlist: {}",
            smtp.allowed_to.join(", ")
        );
    }

    eprintln!("[tool-gate] issuer     {}", gate.issuer_url());
    eprintln!("[tool-gate] redeemer   {}", gate.redeemer_url());
    eprintln!("[tool-gate] origin     {}", gate.origin_info());
    eprintln!(
        "[tool-gate] kt pin     {}",
        gate.kt_log_pub_hex()
    );
    eprintln!(
        "[tool-gate] listen     {}{}",
        cfg.listen,
        if cfg.tls_cert.is_some() {
            " (TLS)"
        } else {
            ""
        }
    );
    eprintln!("[tool-gate] tool       POST {TOOL_EMAIL_SEND}  (PrivateToken + one-time spend → SMTP send)");

    let tls_acceptor = match (&cfg.tls_cert, &cfg.tls_key) {
        (Some(c), Some(k)) => Some(
            cvm_agent::http_service::rustls_acceptor_from_paths(Some(c), Some(k), None)?
                .ok_or_else(|| anyhow::anyhow!("tls config"))?,
        ),
        (None, None) => {
            let addr: SocketAddr = cfg.listen.parse().context("listen address")?;
            if !addr.ip().is_loopback() {
                anyhow::bail!(
                    "non-loopback bind requires --tls-cert and --tls-key (mailbox credentials must not travel in cleartext)"
                );
            }
            None
        }
        _ => anyhow::bail!("--tls-cert and --tls-key must be set together"),
    };

    let state = Arc::new(ToolGateState {
        gate,
        mailer: Mailer::new(smtp),
        attester_url: cfg.attester,
    });

    serve_hyper(
        HttpServerConfig {
            service_name: "tool-gate",
            listen: cfg.listen.clone(),
            max_connections: 256,
            read_timeout_ms: 30_000,
            body_limit_bytes: 256 * 1024,
            cors: CorsHeaders::new("authorization, content-type"),
            tls_acceptor,
        },
        state,
        tool_gate_handler,
    )
    .await
}

fn tool_gate_handler(state: Arc<ToolGateState>, peer: std::net::SocketAddr, req: HttpRequest) -> HandlerFuture {
    Box::pin(async move {
        let mut response = HttpConn::default();
        handle_request(&mut response, state, peer, req).await?;
        Ok(response)
    })
}

async fn handle_request(
    stream: &mut HttpConn,
    state: Arc<ToolGateState>,
    _peer: std::net::SocketAddr,
    req: HttpRequest,
) -> Result<()> {
    if req.method == "OPTIONS" {
        return write_response(stream, 204, "text/plain", Vec::new()).await;
    }

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/healthz") => {
            write_json(
                stream,
                200,
                &serde_json::json!({
                    "ok": true,
                    "service": "tool-gate",
                    "tool": TOOL_EMAIL_SEND,
                }),
            )
            .await
        }
        ("GET", "/") => {
            write_response(
                stream,
                200,
                "text/html; charset=utf-8",
                landing_html().as_bytes().to_vec(),
            )
            .await
        }
        ("GET", path) if path.starts_with("/.well-known/eat-pass/challenge") => {
            challenge_response(stream, &state, &req.path).await
        }
        ("GET", TOOL_EMAIL_SEND) => challenge_response(stream, &state, &req.path).await,
        ("POST", TOOL_EMAIL_SEND) => email_send(stream, state, req).await,
        _ => write_json(stream, 404, &serde_json::json!({"error": "not found"})).await,
    }
}

async fn challenge_response(
    stream: &mut HttpConn,
    state: &Arc<ToolGateState>,
    path_and_query: &str,
) -> Result<()> {
    let (to, subject, body) = match challenge_query(path_and_query) {
        Some(q) => q,
        None => {
            return write_json(
                stream,
                400,
                &serde_json::json!({
                    "error": "challenge requires query params: to, subject, body",
                    "example": "/.well-known/eat-pass/challenge?to=a@b.c&subject=hi&body=hello"
                }),
            )
            .await;
        }
    };

    let (challenge, www) = state
        .gate
        .issue_email_challenge(&to, &subject, &body)
        .map_err(|e| anyhow::anyhow!("issue challenge: {e}"))?;

    let info = ToolChallengeInfo {
        tool: "email.send".into(),
        issuer_name: state.gate.issuer_name().into(),
        origin_info: state.gate.origin_info().into(),
        issuer_url: state.gate.issuer_url().into(),
        redeemer_url: state.gate.redeemer_url().into(),
        kt_log_pub: state.gate.kt_log_pub_hex(),
        attester_url: state.attester_url.clone(),
        note: "Mint a PrivateToken whose challenge binds to this email intent + gate nonce. \
               Fetch this URL before minting; POST the token to /v1/tools/email.send with the same to/subject/body."
            .into(),
    };

    write_response_with_headers(
        stream,
        401,
        "application/json",
        vec![
            ("www-authenticate".into(), www),
            (
                "x-tool-gate-proof".into(),
                "attested-build + eat-pass token required".into(),
            ),
            (
                "x-challenge-digest".into(),
                hex::encode(challenge.digest()),
            ),
        ],
        serde_json::to_vec(&info)?,
    )
    .await
}

fn challenge_query(path_and_query: &str) -> Option<(String, String, String)> {
    let fake = format!("http://tool-gate.local{path_and_query}");
    let url = reqwest::Url::parse(&fake).ok()?;
    let mut to = None;
    let mut subject = None;
    let mut body = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "to" => to = Some(v.into_owned()),
            "subject" => subject = Some(v.into_owned()),
            "body" => body = Some(v.into_owned()),
            _ => {}
        }
    }
    Some((to?, subject?, body?))
}

async fn email_send(stream: &mut HttpConn, state: Arc<ToolGateState>, req: HttpRequest) -> Result<()> {
    let body: EmailSendRequest = match serde_json::from_slice(&req.body) {
        Ok(b) => b,
        Err(e) => {
            return write_json(
                stream,
                400,
                &serde_json::json!({"error": format!("invalid JSON body: {e}")}),
            )
            .await;
        }
    };

    if body.to.trim().is_empty() || body.subject.trim().is_empty() {
        return write_json(
            stream,
            400,
            &serde_json::json!({"error": "to and subject are required"}),
        )
        .await;
    }

    let to_resolved = match cvm_agent::tool_gate::resolve_recipient(&body.to) {
        Ok(t) => t,
        Err(e) => {
            return write_json(stream, 400, &serde_json::json!({"error": e.to_string()})).await;
        }
    };

    let _auth = req
        .headers
        .get("authorization")
        .map(|s| s.as_str());

    let token = match _auth.and_then(|h| http::parse_authorization(h).ok()) {
        Some(t) => t,
        None => {
            return challenge_response(stream, &state, &format!(
                "{TOOL_EMAIL_SEND}?to={}&subject={}&body={}",
                url_encode(body.to.trim()),
                url_encode(&body.subject),
                url_encode(&body.body),
            ))
            .await;
        }
    };

    match state
        .gate
        .verify_and_spend(_auth, &token.challenge_digest)
        .await
    {
        Ok(nonce) => {
            let message_id = state
                .mailer
                .send(&body.to, &body.subject, &body.body)
                .await
                .map_err(|e| anyhow::anyhow!("send failed: {e}"))?;
            let resp = EmailSendResponse {
                ok: true,
                message_id: message_id.clone(),
                to: to_resolved,
                subject: body.subject,
                proof: format!(
                    "eat-pass nonce {} spent; mail sent from gated mailbox",
                    hex::encode(nonce)
                ),
            };
            write_json(stream, 200, &resp).await
        }
        Err(GateReject::MissingToken) | Err(GateReject::Malformed(_)) => {
            challenge_response(
                stream,
                &state,
                &format!(
                    "{TOOL_EMAIL_SEND}?to={}&subject={}&body={}",
                    url_encode(body.to.trim()),
                    url_encode(&body.subject),
                    url_encode(&body.body),
                ),
            )
            .await
        }
        Err(GateReject::UnknownChallenge) => {
            write_json(
                stream,
                403,
                &serde_json::json!({
                    "error": "token challenge unknown or expired — fetch GET /.well-known/eat-pass/challenge with the same to/subject/body before minting"
                }),
            )
            .await
        }
        Err(GateReject::InvalidToken(e)) => {
            write_json(stream, 403, &serde_json::json!({"error": e})).await
        }
        Err(GateReject::UnknownKey) => {
            write_json(
                stream,
                403,
                &serde_json::json!({"error": "issuer key not in transparency log"}),
            )
            .await
        }
        Err(GateReject::DoubleSpend) => {
            write_json(
                stream,
                409,
                &serde_json::json!({"error": "token already spent — one email per capability"}),
            )
            .await
        }
        Err(GateReject::RedeemerDown) => {
            write_json(
                stream,
                503,
                &serde_json::json!({"error": "redeemer unavailable (fail-closed)"}),
            )
            .await
        }
    }
}

fn landing_html() -> &'static str {
    r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>tool-gate</title>
<style>
  body{font-family:ui-monospace,monospace;max-width:52rem;margin:3rem auto;padding:0 1rem;line-height:1.5}
  h1{font-size:1.4rem} .dim{color:#666} code{background:#f4f4f4;padding:.1rem .3rem}
</style></head>
<body>
<h1>tool-gate</h1>
<p>The <strong>IMAP/SMTP mailbox password lives here</strong> — not in your agent, not in the LLM, not in env vars on the CVM client.</p>
<p>An attested agent mints a one-time <code>PrivateToken</code> (eat-pass) bound to the exact email intent, spends it here, and this gate sends the mail.</p>
<p class="dim">Tool: <code>POST /v1/tools/email.send</code> · Challenge: <code>GET /.well-known/eat-pass/challenge?to=&amp;subject=&amp;body=</code></p>
<p class="dim">Agent: <code>cvm tool send-email --to ryan --subject "…" --body "…"</code></p>
</body></html>"#
}

fn url_encode(s: &str) -> String {
    let mut url = reqwest::Url::parse("http://local/").expect("base url");
    url.query_pairs_mut().append_pair("q", s);
    url.query()
        .and_then(|q| q.strip_prefix("q="))
        .map(|v| v.to_string())
        .unwrap_or_else(|| s.to_string())
}

struct Config {
    listen: String,
    issuer: String,
    redeemer: String,
    issuer_name: String,
    origin_info: String,
    kt_log_pub: String,
    attester: Option<String>,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
    insecure_tls: bool,
}

fn parse_args() -> Result<Config> {
    let mut listen = "127.0.0.1:8787".to_string();
    let mut issuer = String::new();
    let mut redeemer = String::new();
    let mut issuer_name = "issuer.eat-pass.dev".to_string();
    let mut origin_info = "tool-gate.secure.build/v1/tools/email.send".to_string();
    let mut kt_log_pub = String::new();
    let mut attester = None;
    let mut tls_cert = None;
    let mut tls_key = None;
    let mut insecure_tls = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--listen" => listen = args.next().context("--listen")?,
            "--issuer" => issuer = args.next().context("--issuer")?,
            "--redeemer" => redeemer = args.next().context("--redeemer")?,
            "--issuer-name" => issuer_name = args.next().context("--issuer-name")?,
            "--origin-info" => origin_info = args.next().context("--origin-info")?,
            "--kt-log-pub" => kt_log_pub = args.next().context("--kt-log-pub")?,
            "--attester" => attester = Some(args.next().context("--attester")?),
            "--tls-cert" => tls_cert = Some(PathBuf::from(args.next().context("--tls-cert")?)),
            "--tls-key" => tls_key = Some(PathBuf::from(args.next().context("--tls-key")?)),
            "--insecure-tls" => insecure_tls = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    if issuer.is_empty() || redeemer.is_empty() || kt_log_pub.is_empty() {
        anyhow::bail!("--issuer, --redeemer, and --kt-log-pub are required");
    }

    Ok(Config {
        listen,
        issuer,
        redeemer,
        issuer_name,
        origin_info,
        kt_log_pub,
        attester,
        tls_cert,
        tls_key,
        insecure_tls,
    })
}

fn parse_hex32(s: &str, what: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(s.trim()).with_context(|| format!("{what}: hex"))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("{what}: expected 32 bytes"))
}

fn print_help() {
    eprintln!(
        "tool-gate — attested agent tool gate (mailbox credentials stay here)\n\n\
         Usage: tool-gate --issuer URL --redeemer URL --kt-log-pub HEX \\\n\
                [--listen HOST:PORT] [--tls-cert PEM --tls-key PEM]\n\n\
         Secrets (env): TOOL_GATE_SMTP_* or TOOL_GATE_IMAP_* + TOOL_GATE_FROM\n\
         Optional: TOOL_GATE_CONTACT_RYAN=ryan@example.com, TOOL_GATE_ALLOWED_TO, TOOL_GATE_DRY_RUN=1"
    );
}
