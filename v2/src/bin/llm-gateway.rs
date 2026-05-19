//! Minimal LLM Attested gateway.
//!
//! This is the first implementation of the narrow interface in
//! `LLM_ATTESTED.md`: provider-shaped requests go in, upstream responses come
//! back, and a signed `ContestEvent` is appended to the event log.

#![forbid(unsafe_code)]

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use bountynet::http_service::{
    serve_hyper, write_json, write_rate_limited, write_response, write_response_with_headers,
    BufferedResponse as HttpConn, CorsHeaders, HandlerFuture, HttpRequest, HttpServerConfig,
};
use bountynet::llm_attested::{
    build_capture_ra_claim, insert_capture_ra_evidence, random_id, sha256_prefixed, CaptureRaClaim,
    ContestEvent, ContestManifest, EventActor, TrustPolicy,
};
use bountynet::llm_attested_net::{RateLimitPolicy, ServiceRateLimiter};
use ed25519_dalek::SigningKey;
use rand::RngCore;
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

const ZERO_HASH: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone)]
struct Config {
    listen: String,
    upstream_base: String,
    upstream_key_env: String,
    upstream_auth: bool,
    provider: String,
    issuer: String,
    hackathon_id: String,
    gateway_id: String,
    competition_url: String,
    gateway_manifest_url: String,
    runcard_receipt_url: String,
    runcard_receipt_body: Option<Vec<u8>>,
    registry_url: String,
    accepted_tee_platforms: Vec<String>,
    value_x: String,
    runcard_receipt_hash: String,
    policy_hash: String,
    scoring_rule_hash: String,
    enforcement_mode: String,
    start_url: String,
    capture_method: String,
    assurance: String,
    event_sink_url: Option<String>,
    event_log_path: PathBuf,
    teams: HashMap<String, String>,
    rate_state_path: PathBuf,
    max_connections: usize,
    read_timeout_ms: u64,
    ip_requests_per_minute: u32,
    unauth_requests_per_minute: u32,
    team_requests_per_minute: u32,
}

struct GatewayState {
    cfg: Config,
    manifest: ContestManifest,
    manifest_hash: String,
    ra_claim: Option<CaptureRaClaim>,
    signing_key: SigningKey,
    upstream_key: Option<String>,
    http: reqwest::Client,
    chains: Mutex<HashMap<String, TeamChain>>,
    limiter: Arc<ServiceRateLimiter>,
}

#[derive(Debug, Clone)]
struct TeamChain {
    next_index: u64,
    previous_hash: String,
}

#[derive(Debug)]
struct UpstreamResult {
    status: u16,
    content_type: String,
    body: Vec<u8>,
    provider_request_id: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = parse_args()?;
    if let Some(parent) = cfg.event_log_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create event log dir {}", parent.display()))?;
        }
    }

    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);

    let manifest = ContestManifest::new(
        cfg.issuer.clone(),
        cfg.hackathon_id.clone(),
        cfg.gateway_id.clone(),
        cfg.competition_url.clone(),
        cfg.gateway_manifest_url.clone(),
        cfg.runcard_receipt_hash.clone(),
        cfg.value_x.clone(),
        cfg.policy_hash.clone(),
        cfg.scoring_rule_hash.clone(),
        cfg.enforcement_mode.clone(),
        cfg.start_url.clone(),
        vec![cfg.capture_method.clone()],
        TrustPolicy::self_hosted(
            cfg.accepted_tee_platforms.clone(),
            cfg.runcard_receipt_url.clone(),
            cfg.registry_url.clone(),
        ),
        &signing_key.verifying_key(),
        vec!["llm.call".to_string(), "submission".to_string()],
    );
    let manifest_hash = manifest.hash().context("hash manifest")?;
    let ra_claim = if cfg.assurance == "attested" {
        let receipt = cfg.runcard_receipt_body.as_deref().ok_or_else(|| {
            anyhow!(
                "attested assurance requires --runcard-receipt so the gateway can prove its capture path"
            )
        })?;
        Some(
            build_capture_ra_claim(
                &manifest,
                &manifest_hash,
                receipt,
                &cfg.capture_method,
                &cfg.assurance,
                &cfg.enforcement_mode,
            )
            .map_err(|e| anyhow!("remote attestation claim rejected: {e}"))?,
        )
    } else {
        None
    };
    let upstream_key = if cfg.upstream_auth {
        Some(read_upstream_key(&cfg)?)
    } else {
        None
    };

    let limiter = Arc::new(
        ServiceRateLimiter::load(cfg.rate_state_path.clone(), 200_000)
            .context("initialize gateway governor rate limiter")?,
    );

    let state = Arc::new(GatewayState {
        cfg: cfg.clone(),
        manifest,
        manifest_hash,
        ra_claim,
        signing_key,
        upstream_key,
        http: reqwest::Client::builder()
            .build()
            .context("build reqwest client")?,
        chains: Mutex::new(HashMap::new()),
        limiter,
    });

    eprintln!("[llm-gateway] listening on {}", cfg.listen);
    eprintln!("[llm-gateway] upstream {}", cfg.upstream_base);
    eprintln!("[llm-gateway] provider {}", cfg.provider);
    eprintln!("[llm-gateway] hackathon {}", cfg.hackathon_id);
    eprintln!("[llm-gateway] gateway {}", cfg.gateway_id);
    eprintln!("[llm-gateway] manifest {}", state.manifest_hash);
    if let Some(claim) = &state.ra_claim {
        eprintln!(
            "[llm-gateway] remote attestation {} {} {}",
            claim.platform, claim.value_x, claim.runcard_receipt_hash
        );
    }
    eprintln!("[llm-gateway] event log {}", cfg.event_log_path.display());
    eprintln!(
        "[llm-gateway] rate limiter governor with persisted backoff state {}",
        cfg.rate_state_path.display()
    );

    serve_hyper(
        HttpServerConfig {
            service_name: "llm-gateway",
            listen: cfg.listen.clone(),
            max_connections: cfg.max_connections,
            read_timeout_ms: cfg.read_timeout_ms,
            body_limit_bytes: 10 * 1024 * 1024,
            cors: CorsHeaders::new("authorization, content-type, x-llm-attested-agent")
                .with_expose("x-llm-attested-event-id, x-llm-attested-team-root"),
            tls_acceptor: None,
        },
        state,
        gateway_handler,
    )
    .await
}

fn gateway_handler(state: Arc<GatewayState>, peer: SocketAddr, req: HttpRequest) -> HandlerFuture {
    Box::pin(async move {
        let mut response = HttpConn::default();
        handle_connection(&mut response, state, peer, req).await?;
        Ok(response)
    })
}

async fn handle_connection(
    stream: &mut HttpConn,
    state: Arc<GatewayState>,
    peer: SocketAddr,
    req: HttpRequest,
) -> Result<()> {
    let ip_decision = state.limiter.check(
        format!("gateway:ip:{}", peer.ip()),
        RateLimitPolicy::per_minute(
            state.cfg.ip_requests_per_minute,
            state.cfg.ip_requests_per_minute.min(120),
        )
        .with_backoff(1_000, 60_000),
    )?;
    if !ip_decision.allowed {
        return write_rate_limited(stream, ip_decision).await;
    }

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
                    "gateway_id": state.cfg.gateway_id,
                    "manifest_hash": state.manifest_hash,
                }),
            )
            .await
        }
        ("GET", "/.well-known/llm-attested/manifest.cbor") => {
            let body = state.manifest.to_cbor().context("encode manifest")?;
            write_response(stream, 200, "application/cbor", body).await
        }
        ("GET", "/.well-known/llm-attested/manifest.json") => {
            write_json(stream, 200, &state.manifest).await
        }
        ("GET", "/.well-known/runcard/receipt") => {
            let Some(receipt) = &state.cfg.runcard_receipt_body else {
                return write_json(
                    stream,
                    404,
                    &serde_json::json!({"error": "runcard receipt not configured"}),
                )
                .await;
            };
            write_response(stream, 200, "application/cbor", receipt.clone()).await
        }
        _ if req.method == "POST" && req.path.starts_with("/v1/") => {
            let team_id = match authenticate_team(&req, &state.cfg.teams) {
                Some(team_id) => team_id,
                None => {
                    let auth_decision = state.limiter.check(
                        format!("gateway:auth:{}", peer.ip()),
                        RateLimitPolicy::per_minute(
                            state.cfg.unauth_requests_per_minute,
                            state.cfg.unauth_requests_per_minute.min(20),
                        )
                        .with_backoff(2_000, 300_000),
                    )?;
                    if !auth_decision.allowed {
                        return write_rate_limited(stream, auth_decision).await;
                    }
                    return write_json(
                        stream,
                        401,
                        &serde_json::json!({"error": "missing or invalid team API key"}),
                    )
                    .await;
                }
            };
            let team_decision = state.limiter.check(
                format!("gateway:team:{team_id}"),
                RateLimitPolicy::per_minute(
                    state.cfg.team_requests_per_minute,
                    state.cfg.team_requests_per_minute.min(60),
                )
                .with_backoff(1_000, 120_000),
            )?;
            if !team_decision.allowed {
                return write_rate_limited(stream, team_decision).await;
            }
            handle_model_request(stream, state, req, team_id).await
        }
        _ => write_json(stream, 404, &serde_json::json!({"error": "not found"})).await,
    }
}

async fn handle_model_request(
    stream: &mut HttpConn,
    state: Arc<GatewayState>,
    req: HttpRequest,
    team_id: String,
) -> Result<()> {
    let started_at_ms = now_ms();
    let upstream = proxy_upstream(&state, &req).await?;
    let completed_at_ms = now_ms();

    let event = build_llm_event(
        &state,
        &req,
        &upstream,
        team_id,
        started_at_ms,
        completed_at_ms,
    )
    .await?;
    let event_id = event.event_id.clone();
    let team_root = event.event_hash().context("hash event")?;
    if let Err(e) = send_event_sink(&state, &event).await {
        eprintln!("[llm-gateway] event sink failed: {e}");
    }

    let extra_headers = vec![
        ("x-llm-attested-event-id".to_string(), event_id),
        ("x-llm-attested-team-root".to_string(), team_root),
    ];
    write_response_with_headers(
        stream,
        upstream.status,
        &upstream.content_type,
        extra_headers,
        upstream.body,
    )
    .await
}

async fn send_event_sink(state: &GatewayState, event: &ContestEvent) -> Result<()> {
    let Some(url) = &state.cfg.event_sink_url else {
        return Ok(());
    };
    let body = event.to_cbor().context("encode event for sink")?;
    let resp = state
        .http
        .post(url)
        .header("content-type", "application/cbor")
        .body(body)
        .send()
        .await
        .context("send event to sink")?;
    if !resp.status().is_success() {
        anyhow::bail!("event sink returned {}", resp.status());
    }
    Ok(())
}

async fn build_llm_event(
    state: &GatewayState,
    req: &HttpRequest,
    upstream: &UpstreamResult,
    team_id: String,
    started_at_ms: u64,
    completed_at_ms: u64,
) -> Result<ContestEvent> {
    let request_hash = sha256_prefixed(&req.body);
    let response_hash = sha256_prefixed(&upstream.body);

    let request_json = serde_json::from_slice::<Value>(&req.body).ok();
    let response_json = serde_json::from_slice::<Value>(&upstream.body).ok();

    let mut subject = BTreeMap::new();
    subject.insert("provider".to_string(), state.cfg.provider.clone());
    subject.insert(
        "provider_request_id".to_string(),
        upstream.provider_request_id.clone(),
    );
    subject.insert("api_family".to_string(), api_family(&req.path).to_string());
    subject.insert(
        "model".to_string(),
        request_json
            .as_ref()
            .and_then(|v| v.get("model"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
    );

    let mut metrics = extract_usage_metrics(response_json.as_ref());
    metrics.insert(
        "latency_ms".to_string(),
        completed_at_ms.saturating_sub(started_at_ms) as i64,
    );
    metrics.insert(
        "tool_call_count".to_string(),
        count_tool_calls(response_json.as_ref()) as i64,
    );

    let mut hashes = BTreeMap::new();
    hashes.insert("request".to_string(), request_hash);
    hashes.insert("response".to_string(), response_hash);

    let mut evidence = BTreeMap::new();
    if let Some(usage) = response_json.as_ref().and_then(|v| v.get("usage")) {
        evidence.insert("provider_usage".to_string(), usage.to_string());
    }
    evidence.insert("gateway_usage".to_string(), "provider-reported".to_string());
    evidence.insert(
        "enforcement_mode".to_string(),
        state.cfg.enforcement_mode.clone(),
    );
    if let Some(claim) = &state.ra_claim {
        insert_capture_ra_evidence(&mut evidence, claim);
    }

    let mut chains = state.chains.lock().await;
    let chain = chains.entry(team_id.clone()).or_insert_with(|| TeamChain {
        next_index: 0,
        previous_hash: ZERO_HASH.to_string(),
    });

    let mut event = ContestEvent::new(
        random_id("evt"),
        "llm.call".to_string(),
        state.cfg.hackathon_id.clone(),
        team_id,
        state.cfg.gateway_id.clone(),
        state.manifest_hash.clone(),
        state.cfg.capture_method.clone(),
        state.cfg.assurance.clone(),
        chain.next_index,
        chain.previous_hash.clone(),
        started_at_ms,
        completed_at_ms,
        EventActor {
            kind: "agent".to_string(),
            agent_session_id: req
                .headers
                .get("x-llm-attested-agent")
                .cloned()
                .unwrap_or_else(|| "agent_default".to_string()),
        },
        subject,
        metrics,
        hashes,
        evidence,
    );
    event
        .sign(&state.signing_key)
        .context("sign contest event")?;
    let event_hash = event.event_hash().context("hash signed contest event")?;
    append_event(&state.cfg.event_log_path, &event)?;

    chain.previous_hash = event_hash;
    chain.next_index += 1;

    Ok(event)
}

async fn proxy_upstream(state: &GatewayState, req: &HttpRequest) -> Result<UpstreamResult> {
    let method = reqwest::Method::from_bytes(req.method.as_bytes()).context("parse method")?;
    let url = format!(
        "{}{}",
        state.cfg.upstream_base.trim_end_matches('/'),
        req.path
    );

    let mut builder = state.http.request(method, url).body(req.body.clone());
    if let Some(upstream_key) = &state.upstream_key {
        builder = builder.bearer_auth(upstream_key);
    }

    for (name, value) in &req.headers {
        if should_forward_header(name) {
            if let (Ok(header_name), Ok(header_value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                builder = builder.header(header_name, header_value);
            }
        }
    }

    let resp = builder.send().await.context("send upstream request")?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let provider_request_id = resp
        .headers()
        .get("x-request-id")
        .or_else(|| resp.headers().get("openai-request-id"))
        .or_else(|| resp.headers().get("request-id"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = resp
        .bytes()
        .await
        .context("read upstream response")?
        .to_vec();

    Ok(UpstreamResult {
        status,
        content_type,
        body,
        provider_request_id,
    })
}

fn should_forward_header(name: &str) -> bool {
    !matches!(
        name,
        "authorization"
            | "host"
            | "content-length"
            | "connection"
            | "accept-encoding"
            | "x-llm-attested-agent"
    )
}

fn authenticate_team(req: &HttpRequest, teams: &HashMap<String, String>) -> Option<String> {
    let auth = req.headers.get("authorization")?;
    let token = auth.strip_prefix("Bearer ")?;
    teams.get(token).cloned()
}

fn append_event(path: &PathBuf, event: &ContestEvent) -> Result<()> {
    let cbor = event.to_cbor().context("encode event")?;
    let line = BASE64_STANDARD.encode(cbor);
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open event log {}", path.display()))?;
    writeln!(file, "{line}").with_context(|| format!("append event log {}", path.display()))?;
    Ok(())
}

fn extract_usage_metrics(response: Option<&Value>) -> BTreeMap<String, i64> {
    let usage = response.and_then(|v| v.get("usage"));
    let input = usage
        .and_then(|u| u.get("prompt_tokens").or_else(|| u.get("input_tokens")))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output = usage
        .and_then(|u| {
            u.get("completion_tokens")
                .or_else(|| u.get("output_tokens"))
        })
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let total = usage
        .and_then(|u| u.get("total_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(input + output);

    let mut metrics = BTreeMap::new();
    metrics.insert("input_tokens".to_string(), input);
    metrics.insert("output_tokens".to_string(), output);
    metrics.insert("total_tokens".to_string(), total);
    metrics
}

fn count_tool_calls(response: Option<&Value>) -> usize {
    let Some(response) = response else {
        return 0;
    };
    let mut count = 0;
    if let Some(choices) = response.get("choices").and_then(Value::as_array) {
        for choice in choices {
            count += choice
                .get("message")
                .and_then(|m| m.get("tool_calls"))
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
        }
    }
    if let Some(output) = response.get("output").and_then(Value::as_array) {
        count += output
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .count();
    }
    count
}

fn api_family(path: &str) -> &'static str {
    if path.starts_with("/v1/responses") {
        "openai-responses"
    } else if path.starts_with("/v1/chat/completions") {
        "openai-chat-completions"
    } else {
        "openai-compatible"
    }
}

fn parse_args() -> Result<Config> {
    let mut listen = "127.0.0.1:8088".to_string();
    let mut upstream_base = std::env::var("LLM_UPSTREAM_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com".to_string());
    let mut upstream_key_env = "LLM_UPSTREAM_API_KEY".to_string();
    let mut upstream_auth = true;
    let mut provider = "generic".to_string();
    let mut issuer = "self-hosted".to_string();
    let mut hackathon_id = "hk_dev".to_string();
    let mut gateway_id = random_id("gw");
    let mut competition_url: Option<String> = None;
    let mut gateway_manifest_url: Option<String> = None;
    let mut runcard_receipt_url: Option<String> = None;
    let mut runcard_receipt_body: Option<Vec<u8>> = None;
    let mut registry_url: Option<String> = None;
    let mut accepted_tee_platforms = vec![
        "tdx".to_string(),
        "sev-snp".to_string(),
        "nitro".to_string(),
    ];
    let mut value_x = std::env::var("BOUNTYNET_VALUE_X").unwrap_or_else(|_| "unknown".to_string());
    let mut runcard_receipt_hash =
        std::env::var("RUNCARD_RECEIPT_HASH").unwrap_or_else(|_| ZERO_HASH.to_string());
    let mut policy_hash = sha256_prefixed(b"allow-all-dev");
    let mut scoring_rule_hash = sha256_prefixed(b"min-total-tokens");
    let mut enforcement_mode = "routed".to_string();
    let mut start_url = "http://localhost:8080/events/local/join".to_string();
    let mut capture_method = "gateway_proxy".to_string();
    let mut assurance = "routed".to_string();
    let mut event_sink_url: Option<String> = None;
    let mut event_log_path = PathBuf::from("./llm-events.cborl");
    let mut teams = HashMap::new();
    let mut rate_state_path = PathBuf::from("./llm-gateway-rate-state.json");
    let mut max_connections = 512usize;
    let mut read_timeout_ms = 5_000u64;
    let mut ip_requests_per_minute = 600u32;
    let mut unauth_requests_per_minute = 60u32;
    let mut team_requests_per_minute = 120u32;

    let args: Vec<String> = std::env::args().collect();
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
            "--upstream" => {
                i += 1;
                upstream_base = arg_value(&args, i, "--upstream")?;
            }
            "--upstream-key-env" => {
                i += 1;
                upstream_key_env = arg_value(&args, i, "--upstream-key-env")?;
            }
            "--upstream-no-auth" => {
                upstream_auth = false;
            }
            "--credential-env" => {
                i += 1;
                upstream_key_env = arg_value(&args, i, "--credential-env")?;
            }
            "--provider" => {
                i += 1;
                provider = arg_value(&args, i, "--provider")?;
            }
            "--issuer" => {
                i += 1;
                issuer = arg_value(&args, i, "--issuer")?;
            }
            "--hackathon-id" => {
                i += 1;
                hackathon_id = arg_value(&args, i, "--hackathon-id")?;
            }
            "--gateway-id" => {
                i += 1;
                gateway_id = arg_value(&args, i, "--gateway-id")?;
            }
            "--competition-url" => {
                i += 1;
                competition_url = Some(arg_value(&args, i, "--competition-url")?);
            }
            "--gateway-manifest-url" => {
                i += 1;
                gateway_manifest_url = Some(arg_value(&args, i, "--gateway-manifest-url")?);
            }
            "--runcard-receipt-url" => {
                i += 1;
                runcard_receipt_url = Some(arg_value(&args, i, "--runcard-receipt-url")?);
            }
            "--registry-url" => {
                i += 1;
                registry_url = Some(arg_value(&args, i, "--registry-url")?);
            }
            "--accepted-tee-platforms" => {
                i += 1;
                accepted_tee_platforms = arg_value(&args, i, "--accepted-tee-platforms")?
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string)
                    .collect();
            }
            "--value-x" => {
                i += 1;
                value_x = arg_value(&args, i, "--value-x")?;
            }
            "--runcard-receipt-hash" => {
                i += 1;
                runcard_receipt_hash = arg_value(&args, i, "--runcard-receipt-hash")?;
            }
            "--runcard-receipt" => {
                i += 1;
                let path = PathBuf::from(arg_value(&args, i, "--runcard-receipt")?);
                let bytes = std::fs::read(&path)
                    .with_context(|| format!("read receipt {}", path.display()))?;
                runcard_receipt_hash = sha256_prefixed(&bytes);
                runcard_receipt_body = Some(bytes);
            }
            "--policy" => {
                i += 1;
                let path = PathBuf::from(arg_value(&args, i, "--policy")?);
                let bytes = std::fs::read(&path)
                    .with_context(|| format!("read policy {}", path.display()))?;
                policy_hash = sha256_prefixed(&bytes);
            }
            "--scoring" => {
                i += 1;
                let path = PathBuf::from(arg_value(&args, i, "--scoring")?);
                let bytes = std::fs::read(&path)
                    .with_context(|| format!("read scoring rule {}", path.display()))?;
                scoring_rule_hash = sha256_prefixed(&bytes);
            }
            "--enforcement-mode" => {
                i += 1;
                enforcement_mode = arg_value(&args, i, "--enforcement-mode")?;
            }
            "--start-url" => {
                i += 1;
                start_url = arg_value(&args, i, "--start-url")?;
            }
            "--capture-method" => {
                i += 1;
                capture_method = arg_value(&args, i, "--capture-method")?;
            }
            "--assurance" => {
                i += 1;
                assurance = arg_value(&args, i, "--assurance")?;
            }
            "--event-sink-url" => {
                i += 1;
                event_sink_url = Some(arg_value(&args, i, "--event-sink-url")?);
            }
            "--events" => {
                i += 1;
                event_log_path = PathBuf::from(arg_value(&args, i, "--events")?);
            }
            "--rate-state" => {
                i += 1;
                rate_state_path = PathBuf::from(arg_value(&args, i, "--rate-state")?);
            }
            "--max-connections" => {
                i += 1;
                max_connections = parse_arg(&args, i, "--max-connections")?;
            }
            "--read-timeout-ms" => {
                i += 1;
                read_timeout_ms = parse_arg(&args, i, "--read-timeout-ms")?;
            }
            "--ip-requests-per-minute" => {
                i += 1;
                ip_requests_per_minute = parse_arg(&args, i, "--ip-requests-per-minute")?;
            }
            "--unauth-requests-per-minute" => {
                i += 1;
                unauth_requests_per_minute = parse_arg(&args, i, "--unauth-requests-per-minute")?;
            }
            "--team-requests-per-minute" => {
                i += 1;
                team_requests_per_minute = parse_arg(&args, i, "--team-requests-per-minute")?;
            }
            "--team" => {
                i += 1;
                let pair = arg_value(&args, i, "--team")?;
                let (team_id, key) = pair
                    .split_once(':')
                    .ok_or_else(|| anyhow!("--team must be team_id:api_key"))?;
                teams.insert(key.to_string(), team_id.to_string());
            }
            other => return Err(anyhow!("unknown argument {other}")),
        }
        i += 1;
    }

    if teams.is_empty() {
        teams.insert("llma_dev".to_string(), "team_dev".to_string());
    }

    let gateway_base_url = default_gateway_base_url(&listen);
    let competition_url =
        competition_url.unwrap_or_else(|| "http://localhost:8080/e/local".to_string());
    let gateway_manifest_url = gateway_manifest_url
        .unwrap_or_else(|| format!("{gateway_base_url}/.well-known/llm-attested/manifest.cbor"));
    let runcard_receipt_url = runcard_receipt_url
        .unwrap_or_else(|| format!("{gateway_base_url}/.well-known/runcard/receipt"));
    let registry_url = registry_url
        .unwrap_or_else(|| "http://localhost:8080/.well-known/runcard/registry.json".to_string());

    Ok(Config {
        listen,
        upstream_base,
        upstream_key_env,
        upstream_auth,
        provider,
        issuer,
        hackathon_id,
        gateway_id,
        competition_url,
        gateway_manifest_url,
        runcard_receipt_url,
        runcard_receipt_body,
        registry_url,
        accepted_tee_platforms,
        value_x,
        runcard_receipt_hash,
        policy_hash,
        scoring_rule_hash,
        enforcement_mode,
        start_url,
        capture_method,
        assurance,
        event_sink_url,
        event_log_path,
        teams,
        rate_state_path,
        max_connections: max_connections.max(1),
        read_timeout_ms: read_timeout_ms.max(1),
        ip_requests_per_minute: ip_requests_per_minute.max(1),
        unauth_requests_per_minute: unauth_requests_per_minute.max(1),
        team_requests_per_minute: team_requests_per_minute.max(1),
    })
}

fn read_upstream_key(cfg: &Config) -> Result<String> {
    if let Ok(value) = std::env::var(&cfg.upstream_key_env) {
        return Ok(value);
    }
    if cfg.upstream_key_env == "LLM_UPSTREAM_API_KEY" {
        if let Ok(value) = std::env::var("OPENAI_API_KEY") {
            return Ok(value);
        }
        if let Ok(value) = std::env::var("OPENROUTER_API_KEY") {
            return Ok(value);
        }
    }
    Err(anyhow!(
        "missing upstream credential env {}; set it or pass --credential-env",
        cfg.upstream_key_env
    ))
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

fn default_gateway_base_url(listen: &str) -> String {
    let host_port = listen
        .strip_prefix("0.0.0.0:")
        .map(|port| format!("127.0.0.1:{port}"))
        .unwrap_or_else(|| listen.to_string());
    format!("http://{host_port}")
}

fn print_usage() {
    eprintln!("llm-gateway - attested LLM proxy and contest event logger");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  llm-gateway [--listen 127.0.0.1:8088] [--team team_id:api_key]");
    eprintln!();
    eprintln!("Important options:");
    eprintln!("  --upstream https://provider.example");
    eprintln!("  --credential-env LLM_UPSTREAM_API_KEY");
    eprintln!("  --upstream-no-auth");
    eprintln!("  --provider provider-label");
    eprintln!("  --issuer https://events.example");
    eprintln!("  --competition-url https://events.example/e/<event>");
    eprintln!(
        "  --gateway-manifest-url https://gateway.example/.well-known/llm-attested/manifest.cbor"
    );
    eprintln!("  --runcard-receipt-url https://gateway.example/.well-known/runcard/receipt");
    eprintln!("  --registry-url https://events.example/.well-known/runcard/registry.json");
    eprintln!("  --accepted-tee-platforms tdx,sev-snp,nitro");
    eprintln!("  --start-url https://events.example/e/<event>/join");
    eprintln!("  --event-sink-url https://events.example/e/<event>/events");
    eprintln!("  --capture-method gateway_proxy");
    eprintln!("  --assurance routed");
    eprintln!("  --events ./llm-events.cborl");
    eprintln!("  --rate-state ./llm-gateway-rate-state.json  (deprecated compatibility; governor is in-process)");
    eprintln!("  --max-connections 512");
    eprintln!("  --read-timeout-ms 5000");
    eprintln!("  --ip-requests-per-minute 600");
    eprintln!("  --unauth-requests-per-minute 60");
    eprintln!("  --team-requests-per-minute 120");
    eprintln!("  --policy ./policy.json");
    eprintln!("  --scoring ./scoring.json");
    eprintln!("  --runcard-receipt ./stage1-attestation.cbor");
}

#[cfg(test)]
mod tests {
    use super::default_gateway_base_url;

    #[test]
    fn default_gateway_base_url_follows_listen_addr() {
        assert_eq!(
            default_gateway_base_url("127.0.0.1:18088"),
            "http://127.0.0.1:18088"
        );
        assert_eq!(
            default_gateway_base_url("0.0.0.0:8088"),
            "http://127.0.0.1:8088"
        );
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
