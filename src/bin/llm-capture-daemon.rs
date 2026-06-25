//! Local capture companion for browser extensions and manual reports.
//!
//! The daemon accepts normalized JSON capture reports, signs them locally, and
//! forwards the same `ContestEvent` CBOR envelope used by the gateway. This is
//! intentionally lower-assurance than an attested gateway, but it keeps browser
//! and manual paths on the same ledger.

#![forbid(unsafe_code)]

use anyhow::{anyhow, Context, Result};
use reqwest::header::CONTENT_TYPE;
use cvm_agent::http_service::{
    serve_hyper, write_json, write_rate_limited, write_response, BufferedResponse as HttpConn,
    CorsHeaders, HandlerFuture, HttpRequest, HttpServerConfig,
};
use cvm_agent::llm_attested::sha256_prefixed;
use cvm_agent::llm_attested_net::{RateLimitPolicy, ServiceRateLimiter};
use cvm_agent::llm_capture::{signing_key_from_seed, CaptureReport, ParticipantConfig};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
struct Config {
    listen: String,
    event_sink_url: String,
    hackathon_id: String,
    team_id: String,
    gateway_id: String,
    manifest_hash: String,
    signing_seed: Option<String>,
    event_log_path: PathBuf,
    pending_log_path: PathBuf,
    rate_state_path: PathBuf,
    max_connections: usize,
    requests_per_minute: u32,
    read_timeout_ms: u64,
}

#[derive(Debug)]
struct State {
    cfg: Config,
    signing_key: ed25519_dalek::SigningKey,
    http: reqwest::Client,
    limiter: Arc<ServiceRateLimiter>,
}

fn main() -> Result<()> {
    main_from_args(std::env::args().collect())
}

pub fn main_from_args(args: Vec<String>) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build capture daemon runtime")?
        .block_on(run_from_args(args))
}

pub async fn run_from_args(args: Vec<String>) -> Result<()> {
    let cfg = parse_args_from(&args)?;
    run_server(cfg).await
}

async fn run_server(cfg: Config) -> Result<()> {
    if let Some(parent) = cfg.event_log_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create event log dir {}", parent.display()))?;
        }
    }
    let limiter = Arc::new(
        ServiceRateLimiter::load(cfg.rate_state_path.clone(), 100_000)
            .context("initialize capture daemon governor rate limiter")?,
    );
    let state = Arc::new(State {
        signing_key: signing_key_from_seed(cfg.signing_seed.as_deref()),
        cfg: cfg.clone(),
        http: reqwest::Client::new(),
        limiter,
    });
    tokio::spawn(replay_loop(state.clone()));

    eprintln!("[llm-capture-daemon] listening on {}", cfg.listen);
    eprintln!("[llm-capture-daemon] event sink {}", cfg.event_sink_url);
    eprintln!(
        "[llm-capture-daemon] rate limiter governor with persisted backoff state {}",
        cfg.rate_state_path.display()
    );
    serve_hyper(
        HttpServerConfig {
            service_name: "llm-capture-daemon",
            listen: cfg.listen.clone(),
            max_connections: cfg.max_connections,
            read_timeout_ms: cfg.read_timeout_ms,
            body_limit_bytes: 1024 * 1024,
            cors: CorsHeaders::new("content-type"),
            tls_acceptor: None,
        },
        state,
        capture_daemon_handler,
    )
    .await
}

fn capture_daemon_handler(state: Arc<State>, peer: SocketAddr, req: HttpRequest) -> HandlerFuture {
    Box::pin(async move {
        let mut response = HttpConn::default();
        handle_connection(&mut response, state, peer, req).await?;
        Ok(response)
    })
}

async fn handle_connection(
    stream: &mut HttpConn,
    state: Arc<State>,
    peer: SocketAddr,
    req: HttpRequest,
) -> Result<()> {
    let decision = state.limiter.check(
        format!("capture-daemon:ip:{}", peer.ip()),
        RateLimitPolicy::per_minute(
            state.cfg.requests_per_minute,
            state.cfg.requests_per_minute.min(120),
        )
        .with_backoff(1_000, 60_000),
    )?;
    if !decision.allowed {
        return write_rate_limited(stream, decision).await;
    }

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/healthz") => write_json(stream, 200, &serde_json::json!({"ok": true})).await,
        ("OPTIONS", _) => write_response(stream, 204, "text/plain", Vec::new()).await,
        ("POST", "/capture/browser") => {
            capture_report(
                stream,
                state,
                req,
                "browser_extension",
                "participant_controlled",
            )
            .await
        }
        ("POST", "/capture/manual") => {
            capture_report(stream, state, req, "manual_import", "self_reported").await
        }
        _ => write_json(stream, 404, &serde_json::json!({"error": "not found"})).await,
    }
}

async fn capture_report(
    stream: &mut HttpConn,
    state: Arc<State>,
    req: HttpRequest,
    default_capture_method: &str,
    default_assurance: &str,
) -> Result<()> {
    let mut report: CaptureReport =
        serde_json::from_slice(&req.body).context("decode capture report")?;
    fill_report_defaults(
        &mut report,
        &state.cfg,
        default_capture_method,
        default_assurance,
    );
    let event = report
        .sign(&state.signing_key)
        .context("sign capture report")?;
    let event_hash = event.event_hash().context("hash capture event")?;
    append_event(&state.cfg.event_log_path, &event)?;
    let delivered = match send_event_with_backoff(&state, &event).await {
        Ok(()) => true,
        Err(e) => {
            eprintln!(
                "[llm-capture-daemon] queued {} after event sink failure: {e}",
                event.event_id
            );
            false
        }
    };
    if !delivered {
        append_event(&state.cfg.pending_log_path, &event)?;
    }
    write_json(
        stream,
        202,
        &serde_json::json!({
            "accepted": true,
            "queued": !delivered,
            "event_id": event.event_id,
            "event_hash": event_hash,
            "capture_method": event.capture_method,
            "assurance": event.assurance,
        }),
    )
    .await
}

fn fill_report_defaults(
    report: &mut CaptureReport,
    cfg: &Config,
    capture_method: &str,
    assurance: &str,
) {
    if report.hackathon_id.is_empty() {
        report.hackathon_id = cfg.hackathon_id.clone();
    }
    if report.team_id.is_empty() {
        report.team_id = cfg.team_id.clone();
    }
    if report.gateway_id.is_empty() {
        report.gateway_id = cfg.gateway_id.clone();
    }
    if report.manifest_hash.is_empty() {
        report.manifest_hash = cfg.manifest_hash.clone();
    }
    if report.capture_method.is_empty() {
        report.capture_method = capture_method.to_string();
    }
    if report.assurance.is_empty() {
        report.assurance = assurance.to_string();
    }
    report
        .evidence
        .entry("capture_daemon".to_string())
        .or_insert_with(|| "llm-capture-daemon".to_string());
}

async fn send_event(state: &State, event: &cvm_agent::llm_attested::ContestEvent) -> Result<()> {
    let body = event.to_cbor().context("encode capture event")?;
    send_event_body(state, body).await
}

async fn send_event_with_backoff(
    state: &State,
    event: &cvm_agent::llm_attested::ContestEvent,
) -> Result<()> {
    let mut delay = Duration::from_millis(200);
    let mut last_error = None;
    for _ in 0..3 {
        match send_event(state, event).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_error = Some(e);
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(2));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("event delivery failed")))
}

async fn send_event_body(state: &State, body: Vec<u8>) -> Result<()> {
    let resp = state
        .http
        .post(&state.cfg.event_sink_url)
        .header(CONTENT_TYPE, "application/cbor")
        .body(body)
        .send()
        .await
        .context("post capture event")?;
    if !resp.status().is_success() {
        anyhow::bail!("event sink returned {}", resp.status());
    }
    Ok(())
}

async fn replay_loop(state: Arc<State>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        match replay_pending_events(&state).await {
            Ok(delivered) if delivered > 0 => {
                eprintln!("[llm-capture-daemon] replayed {delivered} queued capture events");
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[llm-capture-daemon] queued replay failed: {e}");
            }
        }
    }
}

async fn replay_pending_events(state: &State) -> Result<usize> {
    if !state.cfg.pending_log_path.exists() {
        return Ok(0);
    }
    let body = std::fs::read_to_string(&state.cfg.pending_log_path)
        .with_context(|| format!("read pending log {}", state.cfg.pending_log_path.display()))?;
    let mut delivered = 0usize;
    let mut remaining = Vec::new();
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            line.trim().as_bytes(),
        )
        .context("decode queued event")?;
        if send_event_body(state, bytes).await.is_ok() {
            delivered += 1;
        } else {
            remaining.push(line.trim().to_string());
        }
    }
    if remaining.is_empty() {
        let _ = std::fs::remove_file(&state.cfg.pending_log_path);
    } else {
        std::fs::write(
            &state.cfg.pending_log_path,
            format!("{}\n", remaining.join("\n")),
        )
        .with_context(|| {
            format!(
                "rewrite pending log {}",
                state.cfg.pending_log_path.display()
            )
        })?;
    }
    Ok(delivered)
}

fn append_event(path: &PathBuf, event: &cvm_agent::llm_attested::ContestEvent) -> Result<()> {
    let cbor = event.to_cbor().context("encode event")?;
    let line = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, cbor);
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open event log {}", path.display()))?;
    writeln!(file, "{line}").with_context(|| format!("append event log {}", path.display()))
}

fn parse_args_from(args: &[String]) -> Result<Config> {
    let mut listen = "127.0.0.1:8090".to_string();
    let mut event_sink_url = String::new();
    let mut hackathon_id = "hk_dev".to_string();
    let mut team_id = "team_dev".to_string();
    let mut gateway_id = "capture_daemon".to_string();
    let mut manifest_hash =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string();
    let mut signing_seed = None;
    let mut event_log_path = PathBuf::from("./llm-capture-events.cborl");
    let mut pending_log_path = PathBuf::from("./llm-capture-pending.cborl");
    let mut rate_state_path = PathBuf::from("./llm-capture-rate-state.json");
    let mut max_connections = 128usize;
    let mut requests_per_minute = 600u32;
    let mut read_timeout_ms = 5_000u64;

    if let Some(config_path) = find_arg_value(&args[1..], "--config")? {
        let participant =
            ParticipantConfig::load(&PathBuf::from(config_path)).map_err(anyhow::Error::msg)?;
        event_sink_url = participant.event_sink_url;
        hackathon_id = participant.event_id;
        team_id = participant.team_id;
    }
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "--listen" => {
                i += 1;
                listen = arg_value(&args, i, "--listen")?;
            }
            "--event-sink-url" => {
                i += 1;
                event_sink_url = arg_value(&args, i, "--event-sink-url")?;
            }
            "--config" => {
                i += 1;
                let _ = arg_value(&args, i, "--config")?;
            }
            "--hackathon-id" => {
                i += 1;
                hackathon_id = arg_value(&args, i, "--hackathon-id")?;
            }
            "--team-id" => {
                i += 1;
                team_id = arg_value(&args, i, "--team-id")?;
            }
            "--gateway-id" => {
                i += 1;
                gateway_id = arg_value(&args, i, "--gateway-id")?;
            }
            "--manifest-hash" => {
                i += 1;
                manifest_hash = arg_value(&args, i, "--manifest-hash")?;
            }
            "--signing-seed" => {
                i += 1;
                signing_seed = Some(arg_value(&args, i, "--signing-seed")?);
            }
            "--events" => {
                i += 1;
                event_log_path = PathBuf::from(arg_value(&args, i, "--events")?);
            }
            "--pending-events" => {
                i += 1;
                pending_log_path = PathBuf::from(arg_value(&args, i, "--pending-events")?);
            }
            "--rate-state" => {
                i += 1;
                rate_state_path = PathBuf::from(arg_value(&args, i, "--rate-state")?);
            }
            "--max-connections" => {
                i += 1;
                max_connections = parse_arg(&args, i, "--max-connections")?;
            }
            "--requests-per-minute" => {
                i += 1;
                requests_per_minute = parse_arg(&args, i, "--requests-per-minute")?;
            }
            "--read-timeout-ms" => {
                i += 1;
                read_timeout_ms = parse_arg(&args, i, "--read-timeout-ms")?;
            }
            other => return Err(anyhow!("unknown argument {other}")),
        }
        i += 1;
    }
    if event_sink_url.is_empty() {
        anyhow::bail!("--event-sink-url is required");
    }
    if !manifest_hash.starts_with("sha256:") {
        manifest_hash = sha256_prefixed(manifest_hash.as_bytes());
    }

    Ok(Config {
        listen,
        event_sink_url,
        hackathon_id,
        team_id,
        gateway_id,
        manifest_hash,
        signing_seed,
        event_log_path,
        pending_log_path,
        rate_state_path,
        max_connections: max_connections.max(1),
        requests_per_minute: requests_per_minute.max(1),
        read_timeout_ms: read_timeout_ms.max(1),
    })
}

fn arg_value(args: &[String], index: usize, name: &str) -> Result<String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| anyhow!("{name} requires a value"))
}

fn parse_arg<T>(args: &[String], index: usize, name: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    arg_value(args, index, name)?
        .parse()
        .map_err(|e| anyhow!("invalid value for {name}: {e}"))
}

fn find_arg_value(args: &[String], name: &str) -> Result<Option<String>> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            return Ok(Some(arg_value(args, i + 1, name)?));
        }
        i += 1;
    }
    Ok(None)
}

fn print_usage() {
    eprintln!("llm-capture-daemon - browser/manual capture companion");
    eprintln!("Usage:");
    eprintln!("  llm-capture-daemon --config ~/.llmattest/config.json");
    eprintln!("  llm-capture-daemon --event-sink-url http://events/e/<event>/events");
    eprintln!("Important options:");
    eprintln!("  --listen 127.0.0.1:8090");
    eprintln!("  --hackathon-id hk_...");
    eprintln!("  --team-id team_...");
    eprintln!("  --signing-seed stable-team-seed");
    eprintln!("  --events ./llm-capture-events.cborl");
    eprintln!("  --pending-events ./llm-capture-pending.cborl");
}
