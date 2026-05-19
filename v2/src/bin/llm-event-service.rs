//! Hosted self-service event service for LLM Attested.
//!
//! Organizers should not hand-edit gateway configs or distribute team keys by
//! hand. This service creates an event, returns shareable URLs, and lets teams
//! self-issue start instructions.

#![forbid(unsafe_code)]

use anyhow::{anyhow, Context, Result};
use bountynet::http_service::{
    rustls_acceptor_from_paths, serve_hyper, write_json, write_rate_limited, write_response,
    write_response_with_headers, BufferedResponse as HttpConn, CorsHeaders, HandlerFuture,
    HttpRequest, HttpServerConfig,
};
use bountynet::llm_attested::{
    build_capture_ra_claim, event_matches_capture_ra_claim, random_id, sha256_prefixed,
    ContestEvent, ContestManifest, TrustPolicy,
};
use bountynet::llm_attested_net::{RateLimitPolicy, ServiceRateLimiter};
use qrcodegen::{QrCode, QrCodeEcc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
struct Config {
    listen: String,
    public_base_url: String,
    gateway_base_url: String,
    issuer: String,
    registry_url: String,
    runcard_receipt_url: String,
    gateway_manifest_url: String,
    participant_release_base_url: String,
    accepted_tee_platforms: Vec<String>,
    runtime_mode: String,
    runtime_value_x: Option<String>,
    runtime_domain: Option<String>,
    store_path: PathBuf,
    event_log_dir: PathBuf,
    rate_state_path: PathBuf,
    tls_cert_path: Option<PathBuf>,
    tls_key_path: Option<PathBuf>,
    tls_client_ca_path: Option<PathBuf>,
    max_connections: usize,
    read_timeout_ms: u64,
    public_requests_per_minute: u32,
    mutation_requests_per_minute: u32,
    ingest_requests_per_minute: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Store {
    events: BTreeMap<String, EventRecord>,
    teams: BTreeMap<String, Vec<TeamRecord>>,
    #[serde(default)]
    captured_events: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    captured_event_counts: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EventRecord {
    event_id: String,
    #[serde(default = "default_event_type")]
    event_type: String,
    name: String,
    organizer_token: String,
    created_at_ms: u64,
    gateway_base_url: String,
    join_url: String,
    dashboard_url: String,
    organizer_url: String,
    #[serde(default)]
    issuer: String,
    #[serde(default)]
    event_sink_url: String,
    #[serde(default)]
    bootstrap_url: String,
    #[serde(default)]
    gateway_manifest_url: String,
    #[serde(default)]
    participant_release_base_url: String,
    #[serde(default)]
    trust_policy: TrustPolicy,
    default_scoring_rule_hash: String,
    capture_methods: Vec<String>,
    #[serde(default)]
    runcard_visibility: RuncardVisibility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuncardVisibility {
    show_capture_labels: bool,
    show_token_stats: bool,
    show_models: bool,
    show_providers: bool,
    show_agent_sessions: bool,
}

impl Default for RuncardVisibility {
    fn default() -> Self {
        Self {
            show_capture_labels: true,
            show_token_stats: true,
            show_models: true,
            show_providers: true,
            show_agent_sessions: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TeamRecord {
    event_id: String,
    team_id: String,
    team_name: String,
    api_key: String,
    created_at_ms: u64,
}

#[derive(Debug, Deserialize, Default)]
struct CreateEventRequest {
    name: Option<String>,
    event_type: Option<String>,
    gateway_base_url: Option<String>,
    gateway_manifest_url: Option<String>,
    runcard_receipt_url: Option<String>,
    scoring: Option<String>,
    runcard_visibility: Option<RuncardVisibility>,
}

#[derive(Debug, Deserialize, Default)]
struct CreateTeamRequest {
    team_name: Option<String>,
}

struct AppState {
    cfg: Config,
    store: Mutex<Store>,
    limiter: Arc<ServiceRateLimiter>,
    http: reqwest::Client,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = parse_args()?;
    let store = load_store(&cfg.store_path)?;
    std::fs::create_dir_all(&cfg.event_log_dir)
        .with_context(|| format!("create event log dir {}", cfg.event_log_dir.display()))?;
    let limiter = Arc::new(
        ServiceRateLimiter::load(cfg.rate_state_path.clone(), 200_000)
            .context("initialize event service governor rate limiter")?,
    );
    let state = Arc::new(AppState {
        cfg: cfg.clone(),
        store: Mutex::new(store),
        limiter,
        http: reqwest::Client::builder()
            .build()
            .context("build event service http client")?,
    });

    let tls_acceptor = rustls_acceptor_from_paths(
        cfg.tls_cert_path.as_ref(),
        cfg.tls_key_path.as_ref(),
        cfg.tls_client_ca_path.as_ref(),
    )?;
    let transport = if tls_acceptor.is_some() {
        "rustls"
    } else {
        "http"
    };

    eprintln!(
        "[llm-event-service] listening on {} ({transport})",
        cfg.listen
    );
    eprintln!("[llm-event-service] public base {}", cfg.public_base_url);
    eprintln!(
        "[llm-event-service] default gateway {}",
        cfg.gateway_base_url
    );
    eprintln!("[llm-event-service] store {}", cfg.store_path.display());
    eprintln!(
        "[llm-event-service] event log dir {}",
        cfg.event_log_dir.display()
    );
    eprintln!(
        "[llm-event-service] rate limiter governor with persisted backoff state {}",
        cfg.rate_state_path.display()
    );
    if let Some(path) = &cfg.tls_client_ca_path {
        eprintln!("[llm-event-service] rustls client CA {}", path.display());
    }
    if tls_acceptor.is_none() {
        eprintln!("[llm-event-service] expected public deployment: Caddy terminates TLS and forwards HTTP here");
    }

    serve_hyper(
        HttpServerConfig {
            service_name: "llm-event-service",
            listen: cfg.listen.clone(),
            max_connections: cfg.max_connections,
            read_timeout_ms: cfg.read_timeout_ms,
            body_limit_bytes: 1024 * 1024,
            cors: CorsHeaders::new("content-type"),
            tls_acceptor,
        },
        state,
        event_service_handler,
    )
    .await
}

fn event_service_handler(
    state: Arc<AppState>,
    peer: SocketAddr,
    req: HttpRequest,
) -> HandlerFuture {
    Box::pin(async move {
        let mut response = HttpConn::default();
        handle_connection(&mut response, state, peer, req).await?;
        Ok(response)
    })
}

async fn handle_connection(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    peer: SocketAddr,
    req: HttpRequest,
) -> Result<()> {
    let ip = peer.ip().to_string();
    if !enforce_rate(
        stream,
        &state,
        format!("event-service:ip:{ip}"),
        RateLimitPolicy::per_minute(
            state.cfg.public_requests_per_minute,
            state.cfg.public_requests_per_minute.min(120),
        )
        .with_backoff(1_000, 60_000),
    )
    .await?
    {
        return Ok(());
    }

    let route = req
        .path
        .split('?')
        .next()
        .unwrap_or(req.path.as_str())
        .to_string();
    let parts: Vec<String> = route
        .trim_start_matches('/')
        .split('/')
        .map(ToString::to_string)
        .collect();

    match (req.method.as_str(), route.as_str()) {
        ("OPTIONS", _) => write_response(stream, 204, "text/plain", Vec::new()).await,
        ("GET", "/healthz") => write_json(stream, 200, &serde_json::json!({"ok": true})).await,
        ("GET", "/") => write_response(stream, 200, "text/html", home_page(&state.cfg)).await,
        ("GET", "/.well-known/llm-attested/self-host.json") => {
            write_json(stream, 200, &self_host_document(&state.cfg)).await
        }
        ("GET", "/.well-known/runcard/registry.json") => {
            write_json(stream, 200, &registry_document(&state.cfg)).await
        }
        ("GET", "/oembed") => oembed_document(stream, state, &req.path).await,
        ("POST", "/events") => {
            if !enforce_rate(
                stream,
                &state,
                format!("event-service:create-event:{ip}"),
                RateLimitPolicy::per_minute(
                    state.cfg.mutation_requests_per_minute.min(10),
                    state.cfg.mutation_requests_per_minute.min(10),
                )
                .with_backoff(2_000, 300_000),
            )
            .await?
            {
                return Ok(());
            }
            create_event(stream, state, req).await
        }
        _ if req.method == "GET" && parts.len() == 3 && parts[0] == "e" && parts[2] == "join" => {
            join_page(stream, state, &parts[1]).await
        }
        _ if req.method == "GET"
            && parts.len() == 3
            && parts[0] == "e"
            && parts[2] == "bootstrap.json" =>
        {
            bootstrap_document(stream, state, &parts[1]).await
        }
        _ if req.method == "GET"
            && parts.len() == 3
            && parts[0] == "e"
            && parts[2] == "dashboard" =>
        {
            dashboard_page(stream, state, &parts[1]).await
        }
        _ if req.method == "GET"
            && parts.len() == 3
            && parts[0] == "e"
            && parts[2] == "runcards.json" =>
        {
            event_runcards(stream, state, &parts[1]).await
        }
        _ if req.method == "GET"
            && parts.len() == 5
            && parts[0] == "e"
            && parts[2] == "teams"
            && parts[4] == "runcard" =>
        {
            team_runcard_page(stream, state, &parts[1], &parts[3]).await
        }
        _ if req.method == "GET"
            && parts.len() == 5
            && parts[0] == "e"
            && parts[2] == "teams"
            && parts[4] == "runcard.embed" =>
        {
            team_runcard_embed_page(stream, state, &parts[1], &parts[3]).await
        }
        _ if req.method == "GET"
            && parts.len() == 5
            && parts[0] == "e"
            && parts[2] == "teams"
            && parts[4] == "runcard.svg" =>
        {
            team_runcard_svg(stream, state, &parts[1], &parts[3]).await
        }
        _ if req.method == "GET"
            && parts.len() == 5
            && parts[0] == "e"
            && parts[2] == "teams"
            && parts[4] == "runcard.signal.svg" =>
        {
            let agent = query_param(&req.path, "agent");
            team_individual_signal_svg(stream, state, &parts[1], &parts[3], agent).await
        }
        _ if req.method == "GET"
            && parts.len() == 5
            && parts[0] == "e"
            && parts[2] == "teams"
            && parts[4] == "runcard.signal.json" =>
        {
            let agent = query_param(&req.path, "agent");
            team_individual_signal(stream, state, &parts[1], &parts[3], agent).await
        }
        _ if req.method == "GET"
            && parts.len() == 5
            && parts[0] == "e"
            && parts[2] == "teams"
            && parts[4] == "runcard.qr.svg" =>
        {
            team_runcard_qr_svg(stream, state, &parts[1], &parts[3]).await
        }
        _ if req.method == "GET"
            && parts.len() == 5
            && parts[0] == "e"
            && parts[2] == "teams"
            && parts[4] == "runcard.proof.json" =>
        {
            team_runcard_proof(stream, state, &parts[1], &parts[3]).await
        }
        _ if req.method == "GET"
            && parts.len() == 5
            && parts[0] == "e"
            && parts[2] == "teams"
            && parts[4] == "runcard.receipts.json" =>
        {
            team_runcard_receipts(stream, state, &parts[1], &parts[3]).await
        }
        _ if req.method == "GET"
            && parts.len() == 5
            && parts[0] == "e"
            && parts[2] == "teams"
            && parts[4] == "runcard.credential.json" =>
        {
            team_runcard_credential(stream, state, &parts[1], &parts[3]).await
        }
        _ if req.method == "GET"
            && parts.len() == 5
            && parts[0] == "e"
            && parts[2] == "teams"
            && parts[4] == "runcard.json" =>
        {
            team_runcard(stream, state, &parts[1], &parts[3]).await
        }
        _ if req.method == "POST" && parts.len() == 3 && parts[0] == "e" && parts[2] == "teams" => {
            if !enforce_rate(
                stream,
                &state,
                format!("event-service:create-team:{}:{ip}", parts[1]),
                RateLimitPolicy::per_minute(
                    state.cfg.mutation_requests_per_minute,
                    state.cfg.mutation_requests_per_minute.min(30),
                )
                .with_backoff(1_000, 120_000),
            )
            .await?
            {
                return Ok(());
            }
            create_team(stream, state, &parts[1], req).await
        }
        _ if req.method == "POST"
            && parts.len() == 3
            && parts[0] == "e"
            && parts[2] == "events" =>
        {
            if !enforce_rate(
                stream,
                &state,
                format!("event-service:ingest:{}:{ip}", parts[1]),
                RateLimitPolicy::per_minute(
                    state.cfg.ingest_requests_per_minute,
                    state.cfg.ingest_requests_per_minute.min(300),
                )
                .with_backoff(500, 60_000),
            )
            .await?
            {
                return Ok(());
            }
            ingest_event(stream, state, &parts[1], req).await
        }
        _ => write_json(stream, 404, &serde_json::json!({"error": "not found"})).await,
    }
}

async fn enforce_rate(
    stream: &mut HttpConn,
    state: &AppState,
    key: String,
    policy: RateLimitPolicy,
) -> Result<bool> {
    let decision = state.limiter.check(key, policy)?;
    if decision.allowed {
        return Ok(true);
    }
    write_rate_limited(stream, decision).await?;
    Ok(false)
}

async fn create_event(stream: &mut HttpConn, state: Arc<AppState>, req: HttpRequest) -> Result<()> {
    let body: CreateEventRequest = if req.body.is_empty() {
        CreateEventRequest::default()
    } else {
        serde_json::from_slice(&req.body).context("decode create event request")?
    };

    let CreateEventRequest {
        name,
        event_type,
        gateway_base_url,
        gateway_manifest_url,
        runcard_receipt_url,
        scoring,
        runcard_visibility,
    } = body;

    let event_id = random_id("hk");
    let organizer_token = random_id("org");
    let event_type = event_type.unwrap_or_else(|| "hackathon".to_string());
    let name = name.unwrap_or_else(|| {
        if event_type == "universal" {
            format!("Universal Runcard {}", &event_id)
        } else {
            format!("LLM Attested {}", &event_id)
        }
    });
    let gateway_base_url_overridden = gateway_base_url.is_some();
    let gateway_base_url = gateway_base_url.unwrap_or_else(|| state.cfg.gateway_base_url.clone());
    let gateway_base_url = gateway_base_url.trim_end_matches('/').to_string();
    let gateway_manifest_url = gateway_manifest_url.unwrap_or_else(|| {
        if gateway_base_url_overridden {
            format!("{gateway_base_url}/.well-known/llm-attested/manifest.cbor")
        } else {
            state.cfg.gateway_manifest_url.clone()
        }
    });
    let runcard_receipt_url = runcard_receipt_url.unwrap_or_else(|| {
        if gateway_base_url_overridden {
            format!("{gateway_base_url}/.well-known/runcard/receipt")
        } else {
            state.cfg.runcard_receipt_url.clone()
        }
    });
    let join_url = format!("{}/e/{event_id}/join", state.cfg.public_base_url);
    let dashboard_url = format!("{}/e/{event_id}/dashboard", state.cfg.public_base_url);
    let event_sink_url = format!("{}/e/{event_id}/events", state.cfg.public_base_url);
    let bootstrap_url = format!("{}/e/{event_id}/bootstrap.json", state.cfg.public_base_url);
    let organizer_url = format!(
        "{}/e/{event_id}/dashboard?organizer_token={organizer_token}",
        state.cfg.public_base_url
    );
    let scoring = scoring.unwrap_or_else(|| "min-total-tokens".to_string());

    let record = EventRecord {
        event_id: event_id.clone(),
        event_type,
        name,
        organizer_token,
        created_at_ms: now_ms(),
        gateway_base_url,
        join_url,
        dashboard_url,
        organizer_url,
        issuer: state.cfg.issuer.clone(),
        event_sink_url,
        bootstrap_url,
        gateway_manifest_url,
        participant_release_base_url: state.cfg.participant_release_base_url.clone(),
        trust_policy: TrustPolicy::self_hosted(
            state.cfg.accepted_tee_platforms.clone(),
            runcard_receipt_url,
            state.cfg.registry_url.clone(),
        ),
        default_scoring_rule_hash: sha256_prefixed(scoring.as_bytes()),
        capture_methods: vec![
            "gateway_proxy".to_string(),
            "local_proxy".to_string(),
            "browser_extension".to_string(),
            "hosted_workbench".to_string(),
            "provider_tls_notary".to_string(),
        ],
        runcard_visibility: runcard_visibility.unwrap_or_default(),
    };

    {
        let mut store = state.store.lock().await;
        store.events.insert(event_id.clone(), record.clone());
        store.teams.entry(event_id.clone()).or_default();
        store.captured_events.entry(event_id.clone()).or_default();
        store
            .captured_event_counts
            .entry(event_id.clone())
            .or_default();
        save_store(&state.cfg.store_path, &store)?;
    }

    write_json(
        stream,
        201,
        &serde_json::json!({
            "event_id": event_id,
            "event_type": record.event_type,
            "name": record.name,
            "join_url": record.join_url,
            "dashboard_url": record.dashboard_url,
            "organizer_url": record.organizer_url,
            "issuer": record.issuer,
            "event_sink_url": record.event_sink_url,
            "bootstrap_url": record.bootstrap_url,
            "gateway_base_url": record.gateway_base_url,
            "gateway_manifest_url": record.gateway_manifest_url,
            "participant_release_base_url": state.cfg.participant_release_base_url,
            "trust_policy": record.trust_policy,
            "capture_methods": record.capture_methods,
            "runcard_visibility": record.runcard_visibility,
            "organizer_next_step": "Share join_url in your hackathon chat, website, or email.",
        }),
    )
    .await
}

async fn create_team(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    req: HttpRequest,
) -> Result<()> {
    let body: CreateTeamRequest = if req.body.is_empty() {
        CreateTeamRequest::default()
    } else {
        serde_json::from_slice(&req.body).context("decode create team request")?
    };

    let mut store = state.store.lock().await;
    let Some(event) = store.events.get(event_id).cloned() else {
        return write_json(
            stream,
            404,
            &serde_json::json!({"error": "event not found"}),
        )
        .await;
    };

    let team = TeamRecord {
        event_id: event_id.to_string(),
        team_id: random_id("team"),
        team_name: body.team_name.unwrap_or_else(|| "Unnamed team".to_string()),
        api_key: random_id("llma"),
        created_at_ms: now_ms(),
    };

    store
        .teams
        .entry(event_id.to_string())
        .or_default()
        .push(team.clone());
    save_store(&state.cfg.store_path, &store)?;

    write_json(stream, 201, &team_start_payload(&event, &team)).await
}

async fn ingest_event(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    req: HttpRequest,
) -> Result<()> {
    if req.body.is_empty() {
        return write_json(
            stream,
            400,
            &serde_json::json!({"error": "empty event body"}),
        )
        .await;
    }

    let captured = match ContestEvent::from_cbor(&req.body) {
        Ok(event) => event,
        Err(e) => {
            return write_json(
                stream,
                400,
                &serde_json::json!({"error": format!("invalid contest event: {e}")}),
            )
            .await;
        }
    };

    let event_record = {
        let store = state.store.lock().await;
        store.events.get(event_id).cloned()
    };
    let Some(event_record) = event_record else {
        return write_json(
            stream,
            404,
            &serde_json::json!({"error": "event not found"}),
        )
        .await;
    };

    if let Err(e) = validate_attested_capture_event(&state, &event_record, &captured).await {
        return write_json(
            stream,
            422,
            &serde_json::json!({"error": format!("attested capture rejected: {e}")}),
        )
        .await;
    }

    let event_hash = sha256_prefixed(&req.body);
    let mut store = state.store.lock().await;
    if !store.events.contains_key(event_id) {
        return write_json(
            stream,
            404,
            &serde_json::json!({"error": "event not found"}),
        )
        .await;
    }
    let duplicate = store
        .captured_events
        .entry(event_id.to_string())
        .or_default()
        .iter()
        .any(|hash| hash == &event_hash);
    let count = if duplicate {
        *store.captured_event_counts.get(event_id).unwrap_or(&0)
    } else {
        let event_b64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &req.body);
        append_captured_event(&state.cfg.event_log_dir, event_id, &event_b64)?;
        store
            .captured_events
            .entry(event_id.to_string())
            .or_default()
            .push(event_hash.clone());
        let count = store
            .captured_event_counts
            .entry(event_id.to_string())
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
        *count
    };
    save_store(&state.cfg.store_path, &store)?;

    write_json(
        stream,
        202,
        &serde_json::json!({
            "accepted": true,
            "duplicate": duplicate,
            "event_hash": event_hash,
            "count": count
        }),
    )
    .await
}

async fn validate_attested_capture_event(
    state: &AppState,
    event: &EventRecord,
    captured: &ContestEvent,
) -> Result<()> {
    if captured.assurance != "attested" {
        return Ok(());
    }
    let enforcement = captured
        .evidence
        .get("enforcement_mode")
        .ok_or_else(|| anyhow!("missing enforcement_mode evidence"))?;
    let manifest_bytes = state
        .http
        .get(&event.gateway_manifest_url)
        .send()
        .await
        .with_context(|| format!("fetch gateway manifest {}", event.gateway_manifest_url))?
        .error_for_status()
        .with_context(|| {
            format!(
                "gateway manifest returned error {}",
                event.gateway_manifest_url
            )
        })?
        .bytes()
        .await
        .context("read gateway manifest")?;
    let manifest = ContestManifest::from_cbor(&manifest_bytes)
        .map_err(|e| anyhow!("decode gateway manifest: {e}"))?;
    let manifest_hash = manifest
        .hash()
        .map_err(|e| anyhow!("hash gateway manifest: {e}"))?;
    if captured.manifest_hash != manifest_hash {
        anyhow::bail!(
            "event manifest hash {} does not match gateway manifest {}",
            captured.manifest_hash,
            manifest_hash
        );
    }
    let verifying_key = manifest
        .event_verifying_key()
        .map_err(|e| anyhow!("decode event signing key: {e}"))?;
    captured
        .verify(&verifying_key)
        .map_err(|e| anyhow!("event signature is not from the attested gateway key: {e}"))?;

    let receipt_bytes = state
        .http
        .get(&manifest.trust_policy.runcard_receipt_url)
        .send()
        .await
        .with_context(|| {
            format!(
                "fetch runcard receipt {}",
                manifest.trust_policy.runcard_receipt_url
            )
        })?
        .error_for_status()
        .with_context(|| {
            format!(
                "runcard receipt returned error {}",
                manifest.trust_policy.runcard_receipt_url
            )
        })?
        .bytes()
        .await
        .context("read runcard receipt")?;
    let claim = build_capture_ra_claim(
        &manifest,
        &manifest_hash,
        &receipt_bytes,
        &captured.capture_method,
        &captured.assurance,
        enforcement,
    )
    .map_err(|e| anyhow!("verify capture RA claim: {e}"))?;
    if !event_matches_capture_ra_claim(captured, &claim) {
        anyhow::bail!("event RA evidence does not match verified gateway receipt");
    }
    Ok(())
}

async fn bootstrap_document(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
) -> Result<()> {
    let store = state.store.lock().await;
    let Some(event) = store.events.get(event_id) else {
        return write_json(
            stream,
            404,
            &serde_json::json!({"error": "event not found"}),
        )
        .await;
    };
    write_json(stream, 200, &event_bootstrap_document(event)).await
}

async fn join_page(stream: &mut HttpConn, state: Arc<AppState>, event_id: &str) -> Result<()> {
    let store = state.store.lock().await;
    let Some(event) = store.events.get(event_id) else {
        return write_response(stream, 404, "text/html", b"event not found".to_vec()).await;
    };

    let body = format!(
        r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>{name}</title></head>
<body>
<h1>{name}</h1>
<p>This event does not require a specific LLM, provider, IDE, or agent.</p>
<h2>1. Install or update llmattest</h2>
<pre>curl -fsSL {release_base}/install.sh | sh</pre>
<p>Windows PowerShell:</p>
<pre>irm {release_base}/install.ps1 | iex</pre>
<h2>2. Create your team payload</h2>
<pre>curl -sS -X POST {base}/e/{event_id}/teams \
  -H 'content-type: application/json' \
  --data '{{"team_name":"YOUR TEAM"}}' &gt; team.json</pre>
<h2>3. Start your agent through the one binary</h2>
<pre>llmattest start --team-payload team.json -- your-agent-command</pre>
<h2>4. Keep status visible</h2>
<pre>llmattest status --watch</pre>
<p>The runcard shows capture method, assurance, enforcement, token stats, model stats, and proof links.</p>
<p><a href="{dashboard}">Dashboard</a></p>
</body>
</html>"#,
        name = html_escape(&event.name),
        base = state.cfg.public_base_url,
        release_base = html_escape(&state.cfg.participant_release_base_url),
        event_id = event.event_id,
        dashboard = event.dashboard_url
    );
    write_response(stream, 200, "text/html", body.into_bytes()).await
}

async fn dashboard_page(stream: &mut HttpConn, state: Arc<AppState>, event_id: &str) -> Result<()> {
    let store = state.store.lock().await;
    let Some(event) = store.events.get(event_id) else {
        return write_response(stream, 404, "text/html", b"event not found".to_vec()).await;
    };
    let teams = store.teams.get(event_id).cloned().unwrap_or_default();
    let captured_count = store
        .captured_event_counts
        .get(event_id)
        .copied()
        .or_else(|| {
            store
                .captured_events
                .get(event_id)
                .map(|events| events.len() as u64)
        })
        .unwrap_or(0);
    let captured_count = captured_count as usize;
    let mut rows = String::new();
    for team in teams {
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td><a href=\"/e/{}/teams/{}/runcard.json\">runcard</a></td></tr>",
            html_escape(&team.team_name),
            html_escape(&team.team_id),
            team.created_at_ms,
            html_escape(event_id),
            html_escape(&team.team_id)
        ));
    }

    let body = format!(
        r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>{name} dashboard</title></head>
<body>
<h1>{name}</h1>
<p>Join URL: <a href="{join}">{join}</a></p>
<p>Gateway: {gateway}</p>
<p>Captured events: {captured_count}</p>
<table>
<thead><tr><th>Team</th><th>ID</th><th>Created</th><th>Runcard</th></tr></thead>
<tbody>{rows}</tbody>
</table>
</body>
</html>"#,
        name = html_escape(&event.name),
        join = event.join_url,
        gateway = html_escape(&event.gateway_base_url),
        captured_count = captured_count,
        rows = rows
    );
    write_response(stream, 200, "text/html", body.into_bytes()).await
}

fn team_start_payload(event: &EventRecord, team: &TeamRecord) -> Value {
    let base = event.gateway_base_url.trim_end_matches('/');
    let card_url = runcard_url(event, &team.team_id);
    serde_json::json!({
        "event_id": event.event_id,
        "issuer": event.issuer,
        "team_id": team.team_id,
        "team_name": team.team_name,
        "team_api_key": team.api_key,
        "runcard_url": card_url,
        "runcard_json_url": format!("{card_url}.json"),
        "runcard_embed_url": format!("{card_url}.embed"),
        "runcard_image_url": format!("{card_url}.svg"),
        "runcard_proof_url": format!("{card_url}.proof.json"),
        "individual_signal_image_url": format!("{card_url}.signal.svg"),
        "individual_signal_json_url": format!("{card_url}.signal.json"),
        "gateway_base_url": event.gateway_base_url,
        "event_sink_url": event.event_sink_url,
        "bootstrap_url": event.bootstrap_url,
        "gateway_manifest_url": event.gateway_manifest_url,
        "trust_policy": event.trust_policy,
        "openai_compatible": {
            "OPENAI_BASE_URL": format!("{base}/v1"),
            "OPENAI_API_KEY": team.api_key,
            "LLM_ATTESTED_AGENT": "your-agent-or-ide-name"
        },
        "llmattest": {
            "start": "llmattest start --team-payload team.json -- your-agent-command",
            "init": "llmattest init --team-payload team.json",
            "status": "llmattest status",
            "status_watch": "llmattest status --watch",
            "smoke": "llmattest smoke",
            "run": "llmattest run -- your-agent-command"
        },
        "install": {
            "mac_linux": format!("curl -fsSL {}/install.sh | sh", event_service_release_base(event).trim_end_matches('/')),
            "windows_powershell": format!("irm {}/install.ps1 | iex", event_service_release_base(event).trim_end_matches('/')),
            "release_base_url": event_service_release_base(event)
        },
        "participant_next_step": "Save this response as team.json, then run: llmattest start --team-payload team.json -- your-agent-command",
        "curl_test": format!(
            "curl -sS {base}/v1/chat/completions -H 'authorization: Bearer {}' -H 'content-type: application/json' --data '{{\"model\":\"any-compatible-model\",\"messages\":[{{\"role\":\"user\",\"content\":\"hello\"}}]}}'",
            team.api_key
        ),
        "capture_methods": event.capture_methods,
        "note": "Use any LLM or agent that can route through one of the capture methods. Assurance is shown separately from score."
    })
}

async fn event_runcards(stream: &mut HttpConn, state: Arc<AppState>, event_id: &str) -> Result<()> {
    let store = state.store.lock().await;
    let Some(event) = store.events.get(event_id).cloned() else {
        return write_json(
            stream,
            404,
            &serde_json::json!({"error": "event not found"}),
        )
        .await;
    };
    let teams = store.teams.get(event_id).cloned().unwrap_or_default();
    drop(store);

    let events = load_captured_events(&state.cfg.event_log_dir, event_id)?;
    let cards: Vec<Value> = teams
        .iter()
        .map(|team| team_runcard_document(&event, team, &events))
        .collect();
    write_json(
        stream,
        200,
        &serde_json::json!({
            "event_id": event.event_id,
            "name": event.name,
            "runcards": cards
        }),
    )
    .await
}

struct RuncardContext {
    event: EventRecord,
    team: TeamRecord,
    events: Vec<ContestEvent>,
}

async fn load_runcard_context(
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
) -> Result<Option<RuncardContext>> {
    let store = state.store.lock().await;
    let Some(event) = store.events.get(event_id).cloned() else {
        return Ok(None);
    };
    let team = store
        .teams
        .get(event_id)
        .and_then(|teams| teams.iter().find(|team| team.team_id == team_id))
        .cloned();
    let Some(team) = team else {
        return Ok(None);
    };
    drop(store);

    let events = load_captured_events(&state.cfg.event_log_dir, event_id)?;
    Ok(Some(RuncardContext {
        event,
        team,
        events,
    }))
}

async fn team_runcard(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
) -> Result<()> {
    let Some(ctx) = load_runcard_context(state, event_id, team_id).await? else {
        return write_json(
            stream,
            404,
            &serde_json::json!({"error": "runcard not found"}),
        )
        .await;
    };
    write_json(
        stream,
        200,
        &team_runcard_document(&ctx.event, &ctx.team, &ctx.events),
    )
    .await
}

async fn team_runcard_page(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
) -> Result<()> {
    let Some(ctx) = load_runcard_context(state, event_id, team_id).await? else {
        return write_response(stream, 404, "text/html", b"runcard not found".to_vec()).await;
    };
    let card = team_runcard_document(&ctx.event, &ctx.team, &ctx.events);
    let stats = &card["stats"];
    let labels = &card["labels"];
    let share = &card["share"];
    let url = share["url"].as_str().unwrap_or("");
    let embed_url = share["embed_url"].as_str().unwrap_or("");
    let image_url = share["image_url"].as_str().unwrap_or("");
    let signal_image_url = share["individual_signal_image_url"].as_str().unwrap_or("");
    let proof_url = share["proof_url"].as_str().unwrap_or("");
    let credential_url = share["credential_url"].as_str().unwrap_or("");
    let description = card["share"]["headline"].as_str().unwrap_or("");
    let embed_code = format!(
        r#"<iframe src="{embed_url}" width="420" height="520" loading="lazy" referrerpolicy="no-referrer"></iframe>"#
    );
    let profile_markdown = format!("![Runcard signal]({signal_image_url})");
    let body = format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{team} runcard</title>
<meta name="description" content="{description}">
<meta property="og:type" content="profile">
<meta property="og:title" content="{team} runcard">
<meta property="og:description" content="{description}">
<meta property="og:url" content="{url}">
<meta property="og:image" content="{image_url}">
<meta name="twitter:card" content="summary_large_image">
<link rel="alternate" type="application/json" href="{json_url}">
<link rel="alternate" type="application/ld+json" href="{credential_url}">
<link rel="alternate" type="application/json+oembed" href="{oembed_url}">
<style>
:root {{ color-scheme: light; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
body {{ margin: 0; background: #f6f7f2; color: #141414; }}
main {{ max-width: 920px; margin: 0 auto; padding: 40px 20px; }}
.hero {{ display: grid; grid-template-columns: minmax(0, 1fr) 220px; gap: 28px; align-items: center; }}
.proof {{ margin-top: 28px; display: grid; grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); gap: 12px; }}
.stat {{ border: 1px solid #d7dbc9; background: #fff; border-radius: 8px; padding: 14px; }}
.label {{ color: #5d6356; font-size: 12px; text-transform: uppercase; letter-spacing: .08em; }}
.value {{ font-size: 24px; font-weight: 760; margin-top: 4px; word-break: break-word; }}
.links {{ margin-top: 28px; display: flex; flex-wrap: wrap; gap: 10px; }}
a, button {{ color: #0b5f6a; }}
code, textarea {{ font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }}
code {{ overflow-wrap: anywhere; word-break: break-word; font-size: 12px; }}
textarea {{ width: 100%; min-height: 84px; margin-top: 12px; border: 1px solid #cfd5c2; border-radius: 8px; padding: 12px; box-sizing: border-box; background: #fff; }}
@media (max-width: 680px) {{ .hero {{ grid-template-columns: 1fr; }} }}
</style>
<script type="application/ld+json">{json_ld}</script>
</head>
<body>
<main>
<section class="hero">
<div>
<p class="label">{event_name}</p>
<h1>{team}</h1>
<p>{headline}</p>
</div>
<img alt="QR code for this runcard" src="{qr_url}" width="220" height="220">
</section>
<section class="proof">
  <div class="stat"><div class="label">Captured events</div><div class="value">{captured_events}</div></div>
  <div class="stat"><div class="label">LLM calls</div><div class="value">{llm_calls}</div></div>
  <div class="stat"><div class="label">Total tokens</div><div class="value">{total_tokens}</div></div>
  <div class="stat"><div class="label">Assurance</div><div class="value">{assurance}</div></div>
</section>
<section class="proof">
  <div class="stat"><div class="label">Capture</div><div>{capture}</div></div>
  <div class="stat"><div class="label">Enforcement</div><div>{enforcement}</div></div>
  <div class="stat"><div class="label">Event log root</div><code>{event_log_root}</code></div>
</section>
<div class="links">
  <a href="{json_url}">JSON</a>
  <a href="{proof_url}">Proof bundle</a>
  <a href="{credential_url}">Credential JSON</a>
  <a href="{image_url}">SVG card</a>
  <a href="{signal_image_url}">Individual signal SVG</a>
  <a href="{embed_url}">Live iframe</a>
</div>
<textarea readonly>{embed_code}</textarea>
<textarea readonly>{profile_markdown}</textarea>
</main>
</body>
</html>"#,
        team = html_escape(&ctx.team.team_name),
        event_name = html_escape(&ctx.event.name),
        headline = html_escape(description),
        description = html_escape(description),
        url = html_escape(url),
        image_url = html_escape(image_url),
        signal_image_url = html_escape(signal_image_url),
        json_url = html_escape(share["json_url"].as_str().unwrap_or("runcard.json")),
        credential_url = html_escape(credential_url),
        proof_url = html_escape(proof_url),
        embed_url = html_escape(embed_url),
        oembed_url = html_escape(share["oembed_url"].as_str().unwrap_or("")),
        qr_url = html_escape(share["qr_svg_url"].as_str().unwrap_or("")),
        captured_events = stats["captured_events"].as_u64().unwrap_or(0),
        llm_calls = stats["llm_calls"].as_u64().unwrap_or(0),
        total_tokens = stats["total_tokens"].as_i64().unwrap_or(0),
        capture = html_escape(&compact_counts(&labels["capture_methods"])),
        assurance = html_escape(&compact_counts(&labels["assurance"])),
        enforcement = html_escape(&compact_counts(&labels["enforcement"])),
        event_log_root = html_escape(
            card["integrity"]["event_log_root"]
                .as_str()
                .unwrap_or("sha256:empty")
        ),
        embed_code = html_escape(&embed_code),
        profile_markdown = html_escape(&profile_markdown),
        json_ld = script_escape(&runcard_json_ld(&card)),
    );
    write_response(stream, 200, "text/html", body.into_bytes()).await
}

async fn team_runcard_embed_page(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
) -> Result<()> {
    let Some(ctx) = load_runcard_context(state, event_id, team_id).await? else {
        return write_response(stream, 404, "text/html", b"runcard not found".to_vec()).await;
    };
    let card = team_runcard_document(&ctx.event, &ctx.team, &ctx.events);
    let stats = &card["stats"];
    let labels = &card["labels"];
    let share = &card["share"];
    let body = format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="30">
<base target="_blank">
<style>
:root {{ color-scheme: light; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
body {{ margin: 0; background: transparent; color: #141414; }}
a {{ color: inherit; text-decoration: none; }}
.card {{ box-sizing: border-box; width: 100%; min-height: 100vh; border: 1px solid #cfd5c2; border-radius: 8px; background: #fbfcf7; padding: 18px; display: grid; gap: 14px; }}
.eyebrow {{ color: #5d6356; font-size: 11px; text-transform: uppercase; letter-spacing: .08em; }}
h1 {{ margin: 0; font-size: 24px; line-height: 1.08; }}
.headline {{ margin: 0; color: #35382f; }}
.grid {{ display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }}
.stat {{ background: #fff; border: 1px solid #e0e4d5; border-radius: 8px; padding: 10px; min-width: 0; }}
.label {{ color: #687060; font-size: 10px; text-transform: uppercase; letter-spacing: .08em; }}
.value {{ margin-top: 3px; font-weight: 760; font-size: 18px; word-break: break-word; }}
.small {{ font-size: 12px; color: #4b5045; word-break: break-word; }}
footer {{ display: flex; justify-content: space-between; gap: 10px; align-items: center; }}
</style>
</head>
<body>
<a href="{url}" class="card" aria-label="Open runcard">
<header>
  <div class="eyebrow">{event_name}</div>
  <h1>{team}</h1>
</header>
<p class="headline">{headline}</p>
<section class="grid">
  <div class="stat"><div class="label">Tokens</div><div class="value">{total_tokens}</div></div>
  <div class="stat"><div class="label">Calls</div><div class="value">{llm_calls}</div></div>
  <div class="stat"><div class="label">Events</div><div class="value">{captured_events}</div></div>
</section>
<section class="stat">
  <div class="label">Assurance</div>
  <div class="small">{assurance}</div>
</section>
<section class="stat">
  <div class="label">Enforcement</div>
  <div class="small">{enforcement}</div>
</section>
<footer>
  <span class="small">Live runcard</span>
  <img alt="" src="{qr_url}" width="64" height="64">
</footer>
</a>
</body>
</html>"#,
        url = html_escape(share["url"].as_str().unwrap_or("")),
        event_name = html_escape(&ctx.event.name),
        team = html_escape(&ctx.team.team_name),
        headline = html_escape(share["headline"].as_str().unwrap_or("")),
        total_tokens = stats["total_tokens"].as_i64().unwrap_or(0),
        llm_calls = stats["llm_calls"].as_u64().unwrap_or(0),
        captured_events = stats["captured_events"].as_u64().unwrap_or(0),
        assurance = html_escape(&compact_counts(&labels["assurance"])),
        enforcement = html_escape(&compact_counts(&labels["enforcement"])),
        qr_url = html_escape(share["qr_svg_url"].as_str().unwrap_or("")),
    );
    write_response_with_headers(
        stream,
        200,
        "text/html",
        vec![
            (
                "Content-Security-Policy".to_string(),
                "default-src 'self'; img-src 'self' data:; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors *"
                    .to_string(),
            ),
        ],
        body.into_bytes(),
    )
    .await
}

async fn team_runcard_svg(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
) -> Result<()> {
    let Some(ctx) = load_runcard_context(state, event_id, team_id).await? else {
        return write_response(stream, 404, "image/svg+xml", b"not found".to_vec()).await;
    };
    let card = team_runcard_document(&ctx.event, &ctx.team, &ctx.events);
    let svg = runcard_card_svg(&card)?;
    write_response_with_headers(
        stream,
        200,
        "image/svg+xml",
        vec![(
            "Cache-Control".to_string(),
            "public, max-age=30".to_string(),
        )],
        svg.into_bytes(),
    )
    .await
}

async fn team_individual_signal(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
    agent: Option<String>,
) -> Result<()> {
    let Some(ctx) = load_runcard_context(state, event_id, team_id).await? else {
        return write_json(
            stream,
            404,
            &serde_json::json!({"error": "runcard not found"}),
        )
        .await;
    };
    write_json(
        stream,
        200,
        &individual_signal_document(&ctx.event, &ctx.team, &ctx.events, agent.as_deref()),
    )
    .await
}

async fn team_individual_signal_svg(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
    agent: Option<String>,
) -> Result<()> {
    let Some(ctx) = load_runcard_context(state, event_id, team_id).await? else {
        return write_response(stream, 404, "image/svg+xml", b"not found".to_vec()).await;
    };
    let card = individual_signal_document(&ctx.event, &ctx.team, &ctx.events, agent.as_deref());
    let svg = individual_signal_card_svg(&card)?;
    write_response_with_headers(
        stream,
        200,
        "image/svg+xml",
        vec![(
            "Cache-Control".to_string(),
            "public, max-age=30".to_string(),
        )],
        svg.into_bytes(),
    )
    .await
}

async fn team_runcard_qr_svg(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
) -> Result<()> {
    let Some(ctx) = load_runcard_context(state, event_id, team_id).await? else {
        return write_response(stream, 404, "image/svg+xml", b"not found".to_vec()).await;
    };
    let url = runcard_url(&ctx.event, &ctx.team.team_id);
    let svg = qr_svg(&url, 8)?;
    write_response_with_headers(
        stream,
        200,
        "image/svg+xml",
        vec![(
            "Cache-Control".to_string(),
            "public, max-age=300".to_string(),
        )],
        svg.into_bytes(),
    )
    .await
}

async fn team_runcard_proof(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
) -> Result<()> {
    let Some(ctx) = load_runcard_context(state, event_id, team_id).await? else {
        return write_json(
            stream,
            404,
            &serde_json::json!({"error": "runcard not found"}),
        )
        .await;
    };
    let card = team_runcard_document(&ctx.event, &ctx.team, &ctx.events);
    write_json(
        stream,
        200,
        &runcard_proof_document(&ctx.event, &ctx.team, &ctx.events, &card),
    )
    .await
}

async fn team_runcard_receipts(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
) -> Result<()> {
    let Some(ctx) = load_runcard_context(state, event_id, team_id).await? else {
        return write_json(
            stream,
            404,
            &serde_json::json!({"error": "runcard not found"}),
        )
        .await;
    };
    write_json(
        stream,
        200,
        &runcard_receipts_document(&ctx.event, &ctx.team, &ctx.events),
    )
    .await
}

async fn team_runcard_credential(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    event_id: &str,
    team_id: &str,
) -> Result<()> {
    let Some(ctx) = load_runcard_context(state, event_id, team_id).await? else {
        return write_json(
            stream,
            404,
            &serde_json::json!({"error": "runcard not found"}),
        )
        .await;
    };
    let card = team_runcard_document(&ctx.event, &ctx.team, &ctx.events);
    write_json(
        stream,
        200,
        &runcard_credential_document(&ctx.event, &ctx.team, &ctx.events, &card),
    )
    .await
}

async fn oembed_document(
    stream: &mut HttpConn,
    state: Arc<AppState>,
    request_path: &str,
) -> Result<()> {
    let Some(url) = query_param(request_path, "url") else {
        return write_json(stream, 400, &serde_json::json!({"error": "missing url"})).await;
    };
    let public_base = state.cfg.public_base_url.trim_end_matches('/');
    if !url.starts_with(public_base) {
        return write_json(
            stream,
            400,
            &serde_json::json!({"error": "url must belong to this runcard host"}),
        )
        .await;
    }
    let embed_url = format!("{}.embed", url.trim_end_matches(".json"));
    let html = format!(
        r#"<iframe src="{}" width="420" height="520" loading="lazy" referrerpolicy="no-referrer"></iframe>"#,
        html_escape(&embed_url)
    );
    write_json(
        stream,
        200,
        &serde_json::json!({
            "version": "1.0",
            "type": "rich",
            "provider_name": "Runcard",
            "provider_url": state.cfg.public_base_url,
            "title": "LLM Attested runcard",
            "html": html,
            "width": 420,
            "height": 520,
            "cache_age": 30
        }),
    )
    .await
}

fn team_runcard_document(event: &EventRecord, team: &TeamRecord, events: &[ContestEvent]) -> Value {
    let mut team_events: Vec<&ContestEvent> = events
        .iter()
        .filter(|captured| captured.team_id == team.team_id)
        .collect();
    team_events.sort_by(|a, b| {
        a.event_index
            .cmp(&b.event_index)
            .then_with(|| a.started_at_ms.cmp(&b.started_at_ms))
            .then_with(|| a.event_id.cmp(&b.event_id))
    });
    let mut capture_methods = BTreeMap::<String, u64>::new();
    let mut assurance = BTreeMap::<String, u64>::new();
    let mut enforcement = BTreeMap::<String, u64>::new();
    let mut remote_attestation = BTreeMap::<String, u64>::new();
    let mut ra_platforms = BTreeMap::<String, u64>::new();
    let mut models = BTreeMap::<String, u64>::new();
    let mut providers = BTreeMap::<String, u64>::new();
    let mut agent_sessions = BTreeMap::<String, u64>::new();
    let mut total_tokens = 0i64;
    let mut llm_calls = 0i64;
    let mut min_tokens: Option<i64> = None;
    let mut max_tokens: Option<i64> = None;

    for captured in &team_events {
        *capture_methods
            .entry(captured.capture_method.clone())
            .or_default() += 1;
        *assurance.entry(captured.assurance.clone()).or_default() += 1;
        if let Some(mode) = captured.evidence.get("enforcement_mode") {
            *enforcement.entry(mode.clone()).or_default() += 1;
        }
        if let Some(profile) = captured.evidence.get("ra_profile") {
            *remote_attestation.entry(profile.clone()).or_default() += 1;
        }
        if let Some(platform) = captured.evidence.get("ra_platform") {
            *ra_platforms.entry(platform.clone()).or_default() += 1;
        }
        if let Some(model) = captured.subject.get("model") {
            *models.entry(model.clone()).or_default() += 1;
        }
        if let Some(provider) = captured.subject.get("provider") {
            *providers.entry(provider.clone()).or_default() += 1;
        }
        *agent_sessions
            .entry(captured.actor.agent_session_id.clone())
            .or_default() += 1;
        let tokens = captured.total_tokens();
        total_tokens += tokens;
        if captured.kind == "llm.call" {
            llm_calls += 1;
        } else if let Some(calls) = captured.metrics.get("llm_calls") {
            llm_calls += (*calls).max(0);
        }
        min_tokens = Some(min_tokens.map_or(tokens, |current| current.min(tokens)));
        max_tokens = Some(max_tokens.map_or(tokens, |current| current.max(tokens)));
    }

    let mut stats = serde_json::Map::new();
    stats.insert(
        "captured_events".to_string(),
        serde_json::json!(team_events.len()),
    );
    stats.insert("llm_calls".to_string(), serde_json::json!(llm_calls));
    if event.runcard_visibility.show_token_stats {
        stats.insert("total_tokens".to_string(), serde_json::json!(total_tokens));
        stats.insert(
            "min_call_tokens".to_string(),
            serde_json::json!(min_tokens.unwrap_or(0)),
        );
        stats.insert(
            "max_call_tokens".to_string(),
            serde_json::json!(max_tokens.unwrap_or(0)),
        );
    }
    if event.runcard_visibility.show_models {
        stats.insert("models".to_string(), serde_json::json!(models));
    }
    if event.runcard_visibility.show_providers {
        stats.insert("providers".to_string(), serde_json::json!(providers));
    }
    if event.runcard_visibility.show_agent_sessions {
        stats.insert(
            "agent_sessions".to_string(),
            serde_json::json!(agent_sessions.len()),
        );
        stats.insert(
            "agent_session_counts".to_string(),
            serde_json::json!(agent_sessions),
        );
    }

    let labels = if event.runcard_visibility.show_capture_labels {
        serde_json::json!({
            "capture_methods": capture_methods,
            "assurance": assurance,
            "enforcement": enforcement,
            "remote_attestation": remote_attestation,
            "ra_platforms": ra_platforms
        })
    } else {
        serde_json::json!({})
    };

    let event_hashes = event_hashes_for_team(&team_events);
    let event_log_root = event_log_root(&event_hashes);
    let card_url = runcard_url(event, &team.team_id);
    let json_url = format!("{card_url}.json");
    let embed_url = format!("{card_url}.embed");
    let image_url = format!("{card_url}.svg");
    let qr_svg_url = format!("{card_url}.qr.svg");
    let proof_url = format!("{card_url}.proof.json");
    let receipts_url = format!("{card_url}.receipts.json");
    let credential_url = format!("{card_url}.credential.json");
    let oembed_url = format!(
        "{}/oembed?url={}",
        service_base_url(event),
        url_encode_component(&card_url)
    );

    serde_json::json!({
        "version": 1,
        "profile": "https://runcard.dev/llm-participant-runcard/v1",
        "event_id": event.event_id,
        "event_type": event.event_type,
        "event_name": event.name,
        "team_id": team.team_id,
        "team_name": team.team_name,
        "visibility": event.runcard_visibility,
        "labels": labels,
        "stats": stats,
        "integrity": {
            "event_log_root": event_log_root,
            "event_count": event_hashes.len(),
            "latest_event_hash": event_hashes.last().cloned().unwrap_or_else(|| "sha256:empty".to_string()),
            "proof_url": proof_url.clone(),
            "receipts_url": receipts_url.clone(),
            "trust_registry_url": event.trust_policy.registry_url.clone(),
            "runcard_receipt_url": event.trust_policy.runcard_receipt_url.clone(),
            "gateway_manifest_url": event.gateway_manifest_url.clone()
        },
        "share": {
            "headline": format!("{} used {} captured tokens across {} LLM events", team.team_name, total_tokens, llm_calls),
            "url": card_url,
            "json_url": json_url,
            "embed_url": embed_url,
            "image_url": image_url,
            "individual_signal_json_url": format!("{}.signal.json", card_url),
            "individual_signal_image_url": format!("{}.signal.svg", card_url),
            "qr_svg_url": qr_svg_url,
            "proof_url": proof_url,
            "receipts_url": receipts_url,
            "credential_url": credential_url,
            "oembed_url": oembed_url,
            "dashboard_url": event.dashboard_url
        }
    })
}

fn individual_signal_document(
    event: &EventRecord,
    team: &TeamRecord,
    events: &[ContestEvent],
    requested_agent: Option<&str>,
) -> Value {
    let requested_agent = requested_agent
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
        .map(ToString::to_string);
    let all_team_events = team_events(team, events);
    let latest_team_agent = all_team_events
        .last()
        .map(|captured| captured.actor.agent_session_id.clone());
    let agent_session_id = requested_agent.clone().or(latest_team_agent);
    let selected_events: Vec<&ContestEvent> = if let Some(agent_session_id) = &agent_session_id {
        all_team_events
            .iter()
            .copied()
            .filter(|captured| captured.actor.agent_session_id == *agent_session_id)
            .collect()
    } else {
        Vec::new()
    };
    let latest = selected_events.last().copied();
    let day_start_ms = (now_ms() / 86_400_000) * 86_400_000;
    let mut total_tokens = 0i64;
    let mut llm_calls = 0i64;
    let mut today_tokens = 0i64;
    let mut today_llm_calls = 0i64;
    let mut capture_methods = BTreeMap::<String, u64>::new();
    let mut assurance = BTreeMap::<String, u64>::new();
    let mut enforcement = BTreeMap::<String, u64>::new();

    for captured in &selected_events {
        let calls = captured_llm_calls(captured);
        let tokens = captured.total_tokens();
        total_tokens += tokens;
        llm_calls += calls;
        if captured.completed_at_ms >= day_start_ms {
            today_tokens += tokens;
            today_llm_calls += calls;
        }
        *capture_methods
            .entry(captured.capture_method.clone())
            .or_default() += 1;
        *assurance.entry(captured.assurance.clone()).or_default() += 1;
        if let Some(mode) = captured.evidence.get("enforcement_mode") {
            *enforcement.entry(mode.clone()).or_default() += 1;
        }
    }

    let signal_base_url = individual_signal_base_url(event, &team.team_id);
    let image_url = format!("{}.svg", signal_base_url);
    let json_url = format!("{}.json", signal_base_url);
    let image_url = with_optional_agent_query(&image_url, requested_agent.as_deref());
    let json_url = with_optional_agent_query(&json_url, requested_agent.as_deref());
    let event_hashes = event_hashes_for_team(&selected_events);
    let active_path = latest
        .map(|captured| captured.capture_method.clone())
        .unwrap_or_else(|| "not connected".to_string());
    let latest_event = latest
        .map(latest_signal_event_label)
        .unwrap_or_else(|| "none yet".to_string());
    let latest_age = latest
        .map(|captured| format_age(captured.completed_at_ms))
        .unwrap_or_else(|| "none yet".to_string());
    let latest_assurance = latest
        .map(|captured| captured.assurance.clone())
        .unwrap_or_else(|| "pending".to_string());
    let latest_enforcement = latest
        .and_then(|captured| captured.evidence.get("enforcement_mode").cloned())
        .unwrap_or_else(|| "unrouted".to_string());
    let token_source = latest
        .map(token_source_label)
        .unwrap_or_else(|| "no usage receipts yet".to_string());
    let failure = signal_failure_label(latest);
    let participant_agent_session_id = agent_session_id;
    let display_name = participant_agent_session_id
        .clone()
        .unwrap_or_else(|| team.team_name.clone());
    let headline =
        format!("{display_name} has {llm_calls} captured calls and {total_tokens} tokens");

    serde_json::json!({
        "version": 1,
        "profile": "https://runcard.dev/llm-individual-signal/v1",
        "event_id": event.event_id,
        "event_type": event.event_type,
        "event_name": event.name,
        "team_id": team.team_id,
        "team_name": team.team_name,
        "participant": {
            "display_name": display_name.clone(),
            "agent_session_id": participant_agent_session_id
        },
        "stats": {
            "captured_events": selected_events.len(),
            "llm_calls": llm_calls,
            "total_tokens": total_tokens,
            "today_llm_calls": today_llm_calls,
            "today_total_tokens": today_tokens
        },
        "labels": {
            "capture_methods": capture_methods,
            "assurance": assurance,
            "enforcement": enforcement,
            "active_path": active_path,
            "latest_event": latest_event,
            "latest_age": latest_age,
            "latest_assurance": latest_assurance,
            "latest_enforcement": latest_enforcement,
            "token_source": token_source,
            "actionable_failure": failure
        },
        "integrity": {
            "event_log_root": event_log_root(&event_hashes),
            "event_count": event_hashes.len(),
            "latest_event_hash": event_hashes.last().cloned().unwrap_or_else(|| "sha256:empty".to_string()),
            "team_runcard_url": runcard_url(event, &team.team_id),
            "trust_registry_url": event.trust_policy.registry_url.clone(),
            "runcard_receipt_url": event.trust_policy.runcard_receipt_url.clone(),
            "gateway_manifest_url": event.gateway_manifest_url.clone()
        },
        "share": {
            "headline": headline,
            "url": json_url,
            "json_url": json_url,
            "image_url": image_url,
            "team_runcard_url": runcard_url(event, &team.team_id)
        }
    })
}

fn captured_llm_calls(captured: &ContestEvent) -> i64 {
    if captured.kind == "llm.call" {
        1
    } else {
        captured
            .metrics
            .get("llm_calls")
            .copied()
            .unwrap_or(0)
            .max(0)
    }
}

fn latest_signal_event_label(captured: &ContestEvent) -> String {
    let provider = captured
        .subject
        .get("provider")
        .map(String::as_str)
        .unwrap_or("unknown");
    let model = captured
        .subject
        .get("model")
        .map(String::as_str)
        .unwrap_or("unknown");
    format!("{} via {} {}", captured.kind, provider, model)
}

fn format_age(completed_at_ms: u64) -> String {
    let elapsed_ms = now_ms().saturating_sub(completed_at_ms);
    let elapsed_secs = elapsed_ms / 1_000;
    if elapsed_secs < 60 {
        format!("{elapsed_secs}s ago")
    } else if elapsed_secs < 3_600 {
        format!("{}m ago", elapsed_secs / 60)
    } else if elapsed_secs < 86_400 {
        format!("{}h ago", elapsed_secs / 3_600)
    } else {
        format!("{}d ago", elapsed_secs / 86_400)
    }
}

fn token_source_label(captured: &ContestEvent) -> String {
    if let Some(source) = captured.evidence.get("token_source") {
        return source.clone();
    }
    if let Some(source) = captured.evidence.get("gateway_usage") {
        return source.clone();
    }
    if captured.metrics.contains_key("total_tokens") {
        "captured usage".to_string()
    } else {
        "usage missing".to_string()
    }
}

fn signal_failure_label(latest: Option<&ContestEvent>) -> String {
    let Some(latest) = latest else {
        return "waiting for first capture".to_string();
    };
    if latest.kind == "llm.call" && !latest.metrics.contains_key("total_tokens") {
        return "token usage missing".to_string();
    }
    if latest.kind == "llm.call" && latest.total_tokens() == 0 {
        return "zero-token LLM call".to_string();
    }
    "0 actionable failures".to_string()
}

fn service_base_url(event: &EventRecord) -> String {
    let suffix = format!("/e/{}/join", event.event_id);
    event
        .join_url
        .strip_suffix(&suffix)
        .unwrap_or_else(|| event.join_url.trim_end_matches("/join"))
        .trim_end_matches('/')
        .to_string()
}

fn event_service_release_base(event: &EventRecord) -> String {
    if !event.participant_release_base_url.is_empty() {
        return event
            .participant_release_base_url
            .trim_end_matches('/')
            .to_string();
    }
    format!("{}/downloads", service_base_url(event))
}

fn runcard_url(event: &EventRecord, team_id: &str) -> String {
    format!(
        "{}/e/{}/teams/{}/runcard",
        service_base_url(event),
        event.event_id,
        team_id
    )
}

fn individual_signal_base_url(event: &EventRecord, team_id: &str) -> String {
    format!("{}.signal", runcard_url(event, team_id))
}

fn with_optional_agent_query(url: &str, agent: Option<&str>) -> String {
    let Some(agent) = agent else {
        return url.to_string();
    };
    format!("{url}?agent={}", url_encode_component(agent))
}

fn event_hashes_for_team(events: &[&ContestEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| {
            event
                .event_hash()
                .unwrap_or_else(|_| sha256_prefixed(event.event_id.as_bytes()))
        })
        .collect()
}

fn event_log_root(event_hashes: &[String]) -> String {
    if event_hashes.is_empty() {
        return "sha256:empty".to_string();
    }
    sha256_prefixed(event_hashes.join("\n").as_bytes())
}

fn team_events<'a>(team: &TeamRecord, events: &'a [ContestEvent]) -> Vec<&'a ContestEvent> {
    let mut selected: Vec<&ContestEvent> = events
        .iter()
        .filter(|event| event.team_id == team.team_id)
        .collect();
    selected.sort_by(|a, b| {
        a.event_index
            .cmp(&b.event_index)
            .then_with(|| a.started_at_ms.cmp(&b.started_at_ms))
            .then_with(|| a.event_id.cmp(&b.event_id))
    });
    selected
}

fn runcard_proof_document(
    event: &EventRecord,
    team: &TeamRecord,
    events: &[ContestEvent],
    card: &Value,
) -> Value {
    let selected = team_events(team, events);
    let event_hashes = event_hashes_for_team(&selected);
    let card_json = serde_json::to_vec(card).unwrap_or_default();
    serde_json::json!({
        "version": 1,
        "profile": "https://runcard.dev/llm-runcard-proof/v1",
        "event_id": event.event_id,
        "event_type": event.event_type,
        "team_id": team.team_id,
        "runcard_url": runcard_url(event, &team.team_id),
        "runcard_json_hash": sha256_prefixed(&card_json),
        "event_log_root": event_log_root(&event_hashes),
        "event_hashes": event_hashes,
        "receipts_url": format!("{}.receipts.json", runcard_url(event, &team.team_id)),
        "credential_url": format!("{}.credential.json", runcard_url(event, &team.team_id)),
        "trust": {
            "issuer": event.issuer,
            "registry_url": event.trust_policy.registry_url,
            "gateway_manifest_url": event.gateway_manifest_url,
            "runcard_receipt_url": event.trust_policy.runcard_receipt_url,
            "verifier_profile": event.trust_policy.verifier_profile,
            "accepted_tee_platforms": event.trust_policy.accepted_tee_platforms
        },
        "verification": {
            "mode": "recompute-card-from-signed-events",
            "note": "Fetch receipts_url, verify each ContestEvent signature against the gateway manifest, recompute stats, then compare runcard_json_hash and event_log_root."
        }
    })
}

fn runcard_receipts_document(
    event: &EventRecord,
    team: &TeamRecord,
    events: &[ContestEvent],
) -> Value {
    let selected = team_events(team, events);
    let receipts: Vec<Value> = selected
        .iter()
        .map(|captured| {
            let cbor = captured.to_cbor().unwrap_or_default();
            serde_json::json!({
                "event_id": captured.event_id,
                "kind": captured.kind,
                "event_hash": captured.event_hash().unwrap_or_else(|_| sha256_prefixed(captured.event_id.as_bytes())),
                "receipt_cbor_b64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &cbor)
            })
        })
        .collect();
    let hashes: Vec<String> = receipts
        .iter()
        .filter_map(|receipt| receipt["event_hash"].as_str().map(ToString::to_string))
        .collect();
    serde_json::json!({
        "version": 1,
        "profile": "https://runcard.dev/llm-runcard-receipts/v1",
        "event_id": event.event_id,
        "team_id": team.team_id,
        "event_log_root": event_log_root(&hashes),
        "receipts": receipts
    })
}

fn runcard_credential_document(
    event: &EventRecord,
    team: &TeamRecord,
    events: &[ContestEvent],
    card: &Value,
) -> Value {
    let selected = team_events(team, events);
    let event_hashes = event_hashes_for_team(&selected);
    let event_log_root = event_log_root(&event_hashes);
    let card_url = runcard_url(event, &team.team_id);
    serde_json::json!({
        "@context": [
            "https://www.w3.org/ns/credentials/v2",
            "https://purl.imsglobal.org/spec/ob/v3p0/context-3.0.3.json"
        ],
        "id": format!("{card_url}.credential.json"),
        "type": ["VerifiableCredential", "OpenBadgeCredential", "RuncardCredential"],
        "name": format!("{} LLM Attested Runcard", team.team_name),
        "description": card["share"]["headline"],
        "issuer": {
            "id": event.issuer,
            "name": "Runcard LLM Attested"
        },
        "validFrom": now_ms(),
        "credentialSubject": {
            "id": card_url,
            "name": team.team_name,
            "event": {
                "id": event.event_id,
                "name": event.name,
                "type": event.event_type
            },
            "achievement": {
                "id": format!("{}/achievement/llm-attested-capture", service_base_url(event)),
                "type": ["Achievement"],
                "name": "LLM Attested Capture",
                "description": "Captured LLM and agent activity represented by signed runcard receipts."
            },
            "result": card["stats"],
            "evidence": [{
                "id": format!("{card_url}.proof.json"),
                "type": ["Evidence"],
                "name": "Runcard proof bundle",
                "eventLogRoot": event_log_root
            }]
        },
        "proof": {
            "type": "RuncardEventReceiptProof2026",
            "proofPurpose": "assertionMethod",
            "verificationMethod": event.gateway_manifest_url,
            "proofValue": event_log_root,
            "receiptService": format!("{card_url}.receipts.json")
        }
    })
}

fn runcard_json_ld(card: &Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "@context": "https://schema.org",
        "@type": "CreativeWork",
        "name": format!("{} runcard", card["team_name"].as_str().unwrap_or("Participant")),
        "description": card["share"]["headline"],
        "url": card["share"]["url"],
        "encoding": [{
            "@type": "MediaObject",
            "encodingFormat": "application/json",
            "contentUrl": card["share"]["json_url"]
        }, {
            "@type": "MediaObject",
            "encodingFormat": "image/svg+xml",
            "contentUrl": card["share"]["image_url"]
        }],
        "identifier": card["integrity"]["event_log_root"]
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

fn compact_counts(value: &Value) -> String {
    let Some(map) = value.as_object() else {
        return "none".to_string();
    };
    if map.is_empty() {
        return "none".to_string();
    }
    map.iter()
        .map(|(name, count)| format!("{name} x{}", count.as_u64().unwrap_or(0)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn runcard_card_svg(card: &Value) -> Result<String> {
    let share = &card["share"];
    let stats = &card["stats"];
    let labels = &card["labels"];
    let metadata = svg_card_metadata(card)?;
    let url = share["url"].as_str().unwrap_or("");
    let qr = QrCode::encode_text(url, QrCodeEcc::Medium)
        .map_err(|e| anyhow!("encode runcard qr: {e:?}"))?;
    let qr_path = qr_path_data(&qr, 4);
    let qr_size = (qr.size() + 8).max(1) as f64;
    let qr_scale = 172.0 / qr_size;
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="630" viewBox="0 0 1200 630" role="img" aria-label="{team} runcard">
{metadata}
<rect width="1200" height="630" fill="#f6f7f2"/>
<rect x="52" y="52" width="1096" height="526" rx="18" fill="#fbfcf7" stroke="#cfd5c2" stroke-width="2"/>
<text x="92" y="118" fill="#5d6356" font-family="Inter, system-ui, sans-serif" font-size="24" letter-spacing="3">{event_name}</text>
<text x="92" y="178" fill="#141414" font-family="Inter, system-ui, sans-serif" font-size="54" font-weight="800">{team}</text>
<text x="92" y="232" fill="#33372d" font-family="Inter, system-ui, sans-serif" font-size="28">{headline}</text>
<g transform="translate(92 292)">
  <rect width="220" height="112" rx="12" fill="#ffffff" stroke="#dce2d0"/>
  <text x="22" y="40" fill="#687060" font-family="Inter, system-ui, sans-serif" font-size="18" letter-spacing="2">TOKENS</text>
  <text x="22" y="86" fill="#141414" font-family="Inter, system-ui, sans-serif" font-size="42" font-weight="800">{tokens}</text>
</g>
<g transform="translate(336 292)">
  <rect width="220" height="112" rx="12" fill="#ffffff" stroke="#dce2d0"/>
  <text x="22" y="40" fill="#687060" font-family="Inter, system-ui, sans-serif" font-size="18" letter-spacing="2">CALLS</text>
  <text x="22" y="86" fill="#141414" font-family="Inter, system-ui, sans-serif" font-size="42" font-weight="800">{calls}</text>
</g>
<g transform="translate(580 292)">
  <rect width="220" height="112" rx="12" fill="#ffffff" stroke="#dce2d0"/>
  <text x="22" y="40" fill="#687060" font-family="Inter, system-ui, sans-serif" font-size="18" letter-spacing="2">EVENTS</text>
  <text x="22" y="86" fill="#141414" font-family="Inter, system-ui, sans-serif" font-size="42" font-weight="800">{events}</text>
</g>
<text x="92" y="466" fill="#5d6356" font-family="Inter, system-ui, sans-serif" font-size="20">Assurance: {assurance}</text>
<text x="92" y="504" fill="#5d6356" font-family="Inter, system-ui, sans-serif" font-size="20">Enforcement: {enforcement}</text>
<rect x="906" y="294" width="212" height="212" rx="12" fill="#ffffff" stroke="#dce2d0"/>
<path d="{qr_path}" fill="#141414" transform="translate(926 314) scale({qr_scale})"/>
<text x="906" y="542" fill="#5d6356" font-family="Inter, system-ui, sans-serif" font-size="18">scan to verify</text>
</svg>"##,
        team = xml_escape(card["team_name"].as_str().unwrap_or("Participant")),
        event_name = xml_escape(card["event_name"].as_str().unwrap_or("LLM Attested")),
        headline = xml_escape(share["headline"].as_str().unwrap_or("")),
        tokens = stats["total_tokens"].as_i64().unwrap_or(0),
        calls = stats["llm_calls"].as_u64().unwrap_or(0),
        events = stats["captured_events"].as_u64().unwrap_or(0),
        assurance = xml_escape(&truncate_text(&compact_counts(&labels["assurance"]), 64)),
        enforcement = xml_escape(&truncate_text(&compact_counts(&labels["enforcement"]), 64)),
        metadata = metadata,
        qr_path = qr_path,
        qr_scale = qr_scale
    ))
}

fn individual_signal_card_svg(card: &Value) -> Result<String> {
    let share = &card["share"];
    let stats = &card["stats"];
    let labels = &card["labels"];
    let participant = &card["participant"];
    let metadata = svg_card_metadata(card)?;
    let url = share["json_url"]
        .as_str()
        .or_else(|| share["url"].as_str())
        .unwrap_or("");
    let qr = QrCode::encode_text(url, QrCodeEcc::Medium)
        .map_err(|e| anyhow!("encode individual signal qr: {e:?}"))?;
    let qr_path = qr_path_data(&qr, 4);
    let qr_size = (qr.size() + 8).max(1) as f64;
    let qr_scale = 98.0 / qr_size;
    let captured_events = stats["captured_events"].as_u64().unwrap_or(0);
    let status = if captured_events > 0 {
        "LIVE"
    } else {
        "WAITING"
    };
    let status_fill = if captured_events > 0 {
        "#59ffd6"
    } else {
        "#ffd166"
    };
    let display_name = truncate_text(
        participant["display_name"].as_str().unwrap_or("Individual"),
        28,
    );
    let active_path = labels["active_path"]
        .as_str()
        .unwrap_or("not connected")
        .replace('_', " ");
    let latest_age = labels["latest_age"].as_str().unwrap_or("none yet");
    let latest_assurance = labels["latest_assurance"].as_str().unwrap_or("pending");
    let latest_enforcement = labels["latest_enforcement"].as_str().unwrap_or("unrouted");
    let token_source = labels["token_source"].as_str().unwrap_or("usage unknown");
    let failure = labels["actionable_failure"]
        .as_str()
        .unwrap_or("0 actionable failures");
    let today_calls = stats["today_llm_calls"].as_i64().unwrap_or(0);
    let today_tokens = stats["today_total_tokens"].as_i64().unwrap_or(0);
    let today_label = if today_calls == 1 {
        "1 call".to_string()
    } else {
        format!("{today_calls} calls")
    };
    let signal_line = truncate_text(
        &format!("signal: prompts hidden | receipts public | {token_source}"),
        82,
    );
    let health_line = truncate_text(
        &format!("capture health: {latest_enforcement} | {failure} | {today_tokens} tokens today"),
        88,
    );
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="630" viewBox="0 0 1200 630" role="img" aria-label="{name} individual signal runcard">
{metadata}
<rect width="1200" height="630" fill="#06201c"/>
<rect x="44" y="42" width="1112" height="546" rx="24" fill="#092b25" stroke="#d8fff6" stroke-width="2"/>
<text x="76" y="104" fill="#59ffd6" font-family="Inter, system-ui, sans-serif" font-size="26" font-weight="900">LIVE RUNCARD</text>
<text x="76" y="174" fill="#f7fff4" font-family="Inter, system-ui, sans-serif" font-size="74" font-weight="950">{name}</text>
<rect x="756" y="84" width="128" height="34" rx="17" fill="{status_fill}"/>
<text x="820" y="107" fill="#06201c" font-family="Inter, system-ui, sans-serif" font-size="18" font-weight="800" text-anchor="middle">{status}</text>
<rect x="902" y="84" width="170" height="34" rx="17" fill="#ffd166"/>
<text x="987" y="107" fill="#06201c" font-family="Inter, system-ui, sans-serif" font-size="18" font-weight="800" text-anchor="middle">{assurance}</text>
<line x1="78" y1="314" x2="990" y2="314" stroke="#23685c" stroke-width="2" stroke-dasharray="10 12"/>
<path d="M78 304L126 268L174 334L222 283L270 352L318 277L366 326L414 294L462 370L510 285L558 318L606 274L654 342L702 302L750 329L798 279L846 354L894 309L942 296L990 332" fill="none" stroke="#59ffd6" stroke-width="8" stroke-linejoin="round"/>
<path d="M78 304L126 268L174 334L222 283L270 352L318 277L366 326L414 294L462 370L510 285L558 318L606 274L654 342L702 302L750 329L798 279L846 354L894 309L942 296L990 332" fill="none" stroke="#ffd166" stroke-width="2" stroke-linejoin="round"/>
<text x="78" y="400" fill="#d8fff6" font-family="Inter, system-ui, sans-serif" font-size="21" font-weight="800">{signal_line}</text>
<rect x="80" y="416" width="258" height="92" rx="12" fill="#0d3932" stroke="#23685c" stroke-width="2"/>
<text x="104" y="453" fill="#59ffd6" font-family="Inter, system-ui, sans-serif" font-size="18" font-weight="850">active path</text>
<text x="104" y="486" fill="#f7fff4" font-family="Inter, system-ui, sans-serif" font-size="30" font-weight="950">{active_path}</text>
<rect x="368" y="416" width="258" height="92" rx="12" fill="#0d3932" stroke="#23685c" stroke-width="2"/>
<text x="392" y="453" fill="#59ffd6" font-family="Inter, system-ui, sans-serif" font-size="18" font-weight="850">latest event</text>
<text x="392" y="486" fill="#f7fff4" font-family="Inter, system-ui, sans-serif" font-size="30" font-weight="950">{latest_age}</text>
<rect x="656" y="416" width="258" height="92" rx="12" fill="#0d3932" stroke="#23685c" stroke-width="2"/>
<text x="680" y="453" fill="#59ffd6" font-family="Inter, system-ui, sans-serif" font-size="18" font-weight="850">today</text>
<text x="680" y="486" fill="#f7fff4" font-family="Inter, system-ui, sans-serif" font-size="30" font-weight="950">{today_label}</text>
<rect x="960" y="400" width="112" height="112" rx="8" fill="#d8fff6"/>
<path d="{qr_path}" fill="#06201c" transform="translate(967 407) scale({qr_scale})"/>
<text x="80" y="560" fill="#a8d9ce" font-family="Inter, system-ui, sans-serif" font-size="19" font-weight="800">{health_line}</text>
</svg>"##,
        name = xml_escape(&display_name),
        status_fill = status_fill,
        status = status,
        assurance = xml_escape(&truncate_text(latest_assurance, 18)),
        signal_line = xml_escape(&signal_line),
        active_path = xml_escape(&truncate_text(&active_path, 16)),
        latest_age = xml_escape(&truncate_text(latest_age, 16)),
        today_label = xml_escape(&today_label),
        qr_path = qr_path,
        qr_scale = qr_scale,
        health_line = xml_escape(&health_line),
        metadata = metadata
    ))
}

fn svg_card_metadata(card: &Value) -> Result<String> {
    let metadata_json = serde_json::to_string(card).context("encode svg card metadata")?;
    let metadata_hash = sha256_prefixed(metadata_json.as_bytes());
    let profile = card["profile"]
        .as_str()
        .unwrap_or("https://runcard.dev/runcard/v1");
    Ok(format!(
        r#"<metadata id="runcard-card-json" data-content-type="application/json" data-profile="{profile}" data-sha256="{metadata_hash}">{metadata_json}</metadata>"#,
        profile = xml_escape(profile),
        metadata_hash = xml_escape(&metadata_hash),
        metadata_json = xml_escape(&metadata_json)
    ))
}

fn qr_svg(text: &str, module_px: i32) -> Result<String> {
    let qr = QrCode::encode_text(text, QrCodeEcc::Medium)
        .map_err(|e| anyhow!("encode runcard qr: {e:?}"))?;
    let border = 4;
    let size = qr.size() + border * 2;
    let width = size * module_px;
    let path = qr_path_data(&qr, border);
    Ok(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{width}" viewBox="0 0 {size} {size}" role="img" aria-label="QR code">
<rect width="{size}" height="{size}" fill="#ffffff"/>
<path d="{path}" fill="#141414"/>
</svg>"##
    ))
}

fn qr_path_data(qr: &QrCode, border: i32) -> String {
    let mut path = String::new();
    for y in 0..qr.size() {
        for x in 0..qr.size() {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{} {}h1v1h-1z", x + border, y + border));
            }
        }
    }
    path
}

fn truncate_text(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut out: String = value.chars().take(limit.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

fn xml_escape(value: &str) -> String {
    html_escape(value).replace('\'', "&apos;")
}

fn script_escape(value: &str) -> String {
    value
        .replace("</", "<\\/")
        .replace("<!--", "<\\!--")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn query_param(path: &str, name: &str) -> Option<String> {
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(key) == name {
            return Some(percent_decode(value));
        }
    }
    None
}

fn url_encode_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn event_bootstrap_document(event: &EventRecord) -> Value {
    serde_json::json!({
        "version": 1,
        "profile": "https://runcard.dev/llm-self-host-bootstrap/v1",
        "issuer": event.issuer,
        "event_id": event.event_id,
        "name": event.name,
        "join_url": event.join_url,
        "dashboard_url": event.dashboard_url,
        "gateway_base_url": event.gateway_base_url,
        "gateway_manifest_url": event.gateway_manifest_url,
        "event_sink_url": event.event_sink_url,
        "participant_release_base_url": event_service_release_base(event),
        "trust_policy": event.trust_policy,
        "capture_methods": event.capture_methods
    })
}

fn self_host_document(cfg: &Config) -> Value {
    serde_json::json!({
        "version": 1,
        "profile": "https://runcard.dev/llm-self-host-instance/v1",
        "issuer": cfg.issuer,
        "public_base_url": cfg.public_base_url,
        "gateway_base_url": cfg.gateway_base_url,
        "gateway_manifest_url": cfg.gateway_manifest_url,
        "participant_release_base_url": cfg.participant_release_base_url,
        "runtime": {
            "mode": cfg.runtime_mode,
            "tee_attested": cfg.runtime_mode == "tee",
            "value_x": cfg.runtime_value_x,
            "domain": cfg.runtime_domain
        },
        "trust_policy": TrustPolicy::self_hosted(
            cfg.accepted_tee_platforms.clone(),
            cfg.runcard_receipt_url.clone(),
            cfg.registry_url.clone(),
        )
    })
}

fn registry_document(cfg: &Config) -> Value {
    serde_json::json!({
        "version": 1,
        "profile": "https://runcard.dev/runcard-trust-registry/v1",
        "issuer": cfg.issuer,
        "verifier_profile": "runcard-eat-v2",
        "accepted_tee_platforms": cfg.accepted_tee_platforms,
        "runcard_receipt_url": cfg.runcard_receipt_url,
        "gateway_manifest_url": cfg.gateway_manifest_url
    })
}

fn home_page(cfg: &Config) -> Vec<u8> {
    let runtime_label = if cfg.runtime_mode == "tee" {
        "TEE-backed local site"
    } else {
        "plain local site"
    };
    format!(
        r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>LLM Attested Events</title></head>
<body>
<h1>LLM Attested Events</h1>
<p>Runtime: <strong>{runtime_label}</strong></p>
<p><a href="/.well-known/llm-attested/self-host.json">Self-host document</a></p>
<p><a href="/.well-known/runcard/registry.json">Trust registry</a></p>
<p>Create an event:</p>
<pre>curl -sS -X POST /events -H 'content-type: application/json' --data '{{"name":"My Hackathon"}}'</pre>
</body>
</html>"#,
        runtime_label = runtime_label
    )
    .into_bytes()
}

fn load_store(path: &PathBuf) -> Result<Store> {
    if !path.exists() {
        return Ok(Store::default());
    }
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("read event service store {}", path.display()))?;
    serde_json::from_str(&body).with_context(|| format!("decode store {}", path.display()))
}

fn save_store(path: &PathBuf, store: &Store) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create store dir {}", parent.display()))?;
        }
    }
    let body = serde_json::to_vec_pretty(store).context("encode event service store")?;
    std::fs::write(path, body).with_context(|| format!("write store {}", path.display()))
}

fn append_captured_event(dir: &PathBuf, event_id: &str, event_b64: &str) -> Result<()> {
    if !event_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        anyhow::bail!("invalid event id for event log path");
    }
    std::fs::create_dir_all(dir)
        .with_context(|| format!("create event log dir {}", dir.display()))?;
    let path = dir.join(format!("{event_id}.cborl"));
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open event log {}", path.display()))?;
    writeln!(file, "{event_b64}").with_context(|| format!("append event log {}", path.display()))
}

fn load_captured_events(dir: &PathBuf, event_id: &str) -> Result<Vec<ContestEvent>> {
    if !event_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        anyhow::bail!("invalid event id for event log path");
    }
    let path = dir.join(format!("{event_id}.cborl"));
    if !path.exists() {
        return Ok(Vec::new());
    }
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("read captured event log {}", path.display()))?;
    let mut events = Vec::new();
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            line.trim().as_bytes(),
        )
        .context("decode captured event base64")?;
        if let Ok(event) = ContestEvent::from_cbor(&bytes) {
            events.push(event);
        }
    }
    Ok(events)
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn default_event_type() -> String {
    "hackathon".to_string()
}

fn parse_args() -> Result<Config> {
    let mut listen = "127.0.0.1:8080".to_string();
    let mut public_base_url = "http://127.0.0.1:8080".to_string();
    let mut gateway_base_url = "http://127.0.0.1:8088".to_string();
    let mut issuer = "self-hosted".to_string();
    let mut registry_url: Option<String> = None;
    let mut runcard_receipt_url: Option<String> = None;
    let mut gateway_manifest_url: Option<String> = None;
    let mut participant_release_base_url: Option<String> = None;
    let mut accepted_tee_platforms = vec![
        "tdx".to_string(),
        "sev-snp".to_string(),
        "nitro".to_string(),
    ];
    let mut store_path = PathBuf::from("./llm-events-service.json");
    let mut event_log_dir = PathBuf::from("./llm-event-captures");
    let mut rate_state_path = PathBuf::from("./llm-event-service-rate-state.json");
    let mut tls_cert_path: Option<PathBuf> = None;
    let mut tls_key_path: Option<PathBuf> = None;
    let mut tls_client_ca_path: Option<PathBuf> = None;
    let mut max_connections = 512usize;
    let mut read_timeout_ms = 5_000u64;
    let mut public_requests_per_minute = 1_200u32;
    let mut mutation_requests_per_minute = 60u32;
    let mut ingest_requests_per_minute = 3_000u32;

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
            "--public-base-url" => {
                i += 1;
                public_base_url = arg_value(&args, i, "--public-base-url")?;
            }
            "--gateway-base-url" => {
                i += 1;
                gateway_base_url = arg_value(&args, i, "--gateway-base-url")?;
            }
            "--issuer" => {
                i += 1;
                issuer = arg_value(&args, i, "--issuer")?;
            }
            "--registry-url" => {
                i += 1;
                registry_url = Some(arg_value(&args, i, "--registry-url")?);
            }
            "--runcard-receipt-url" => {
                i += 1;
                runcard_receipt_url = Some(arg_value(&args, i, "--runcard-receipt-url")?);
            }
            "--gateway-manifest-url" => {
                i += 1;
                gateway_manifest_url = Some(arg_value(&args, i, "--gateway-manifest-url")?);
            }
            "--participant-release-base-url" => {
                i += 1;
                participant_release_base_url =
                    Some(arg_value(&args, i, "--participant-release-base-url")?);
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
            "--store" => {
                i += 1;
                store_path = PathBuf::from(arg_value(&args, i, "--store")?);
            }
            "--event-log-dir" => {
                i += 1;
                event_log_dir = PathBuf::from(arg_value(&args, i, "--event-log-dir")?);
            }
            "--rate-state" => {
                i += 1;
                rate_state_path = PathBuf::from(arg_value(&args, i, "--rate-state")?);
            }
            "--tls-cert" => {
                i += 1;
                tls_cert_path = Some(PathBuf::from(arg_value(&args, i, "--tls-cert")?));
            }
            "--tls-key" => {
                i += 1;
                tls_key_path = Some(PathBuf::from(arg_value(&args, i, "--tls-key")?));
            }
            "--tls-client-ca" => {
                i += 1;
                tls_client_ca_path = Some(PathBuf::from(arg_value(&args, i, "--tls-client-ca")?));
            }
            "--max-connections" => {
                i += 1;
                max_connections = parse_arg(&args, i, "--max-connections")?;
            }
            "--read-timeout-ms" => {
                i += 1;
                read_timeout_ms = parse_arg(&args, i, "--read-timeout-ms")?;
            }
            "--public-requests-per-minute" => {
                i += 1;
                public_requests_per_minute = parse_arg(&args, i, "--public-requests-per-minute")?;
            }
            "--mutation-requests-per-minute" => {
                i += 1;
                mutation_requests_per_minute =
                    parse_arg(&args, i, "--mutation-requests-per-minute")?;
            }
            "--ingest-requests-per-minute" => {
                i += 1;
                ingest_requests_per_minute = parse_arg(&args, i, "--ingest-requests-per-minute")?;
            }
            other => return Err(anyhow!("unknown argument {other}")),
        }
        i += 1;
    }

    let public_base_url = public_base_url.trim_end_matches('/').to_string();
    let gateway_base_url = gateway_base_url.trim_end_matches('/').to_string();
    let registry_url = registry_url
        .unwrap_or_else(|| format!("{public_base_url}/.well-known/runcard/registry.json"));
    let runcard_receipt_url = runcard_receipt_url
        .unwrap_or_else(|| format!("{gateway_base_url}/.well-known/runcard/receipt"));
    let gateway_manifest_url = gateway_manifest_url
        .unwrap_or_else(|| format!("{gateway_base_url}/.well-known/llm-attested/manifest.cbor"));
    let participant_release_base_url = participant_release_base_url
        .unwrap_or_else(|| format!("{public_base_url}/downloads"))
        .trim_end_matches('/')
        .to_string();
    let runtime_stage = std::env::var("BOUNTYNET_STAGE").unwrap_or_default();
    let runtime_mode = if runtime_stage == "1" {
        "tee".to_string()
    } else {
        "plain".to_string()
    };
    let runtime_value_x = std::env::var("BOUNTYNET_VALUE_X").ok();
    let runtime_domain = std::env::var("BOUNTYNET_DOMAIN").ok();

    Ok(Config {
        listen,
        public_base_url,
        gateway_base_url,
        issuer,
        registry_url,
        runcard_receipt_url,
        gateway_manifest_url,
        participant_release_base_url,
        accepted_tee_platforms,
        runtime_mode,
        runtime_value_x,
        runtime_domain,
        store_path,
        event_log_dir,
        rate_state_path,
        tls_cert_path,
        tls_key_path,
        tls_client_ca_path,
        max_connections: max_connections.max(1),
        read_timeout_ms: read_timeout_ms.max(1),
        public_requests_per_minute: public_requests_per_minute.max(1),
        mutation_requests_per_minute: mutation_requests_per_minute.max(1),
        ingest_requests_per_minute: ingest_requests_per_minute.max(1),
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

fn print_usage() {
    eprintln!("llm-event-service - hosted self-service event creation");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  llm-event-service --public-base-url https://events.example");
    eprintln!();
    eprintln!("Important options:");
    eprintln!("  --listen 127.0.0.1:8080");
    eprintln!("  --gateway-base-url https://gateway.example");
    eprintln!("  --issuer https://events.example");
    eprintln!("  --registry-url https://events.example/.well-known/runcard/registry.json");
    eprintln!("  --runcard-receipt-url https://gateway.example/.well-known/runcard/receipt");
    eprintln!(
        "  --gateway-manifest-url https://gateway.example/.well-known/llm-attested/manifest.cbor"
    );
    eprintln!("  --participant-release-base-url https://events.example/downloads");
    eprintln!("  --accepted-tee-platforms tdx,sev-snp,nitro");
    eprintln!("  --store ./llm-events-service.json");
    eprintln!("  --event-log-dir ./llm-event-captures");
    eprintln!("  --rate-state ./llm-event-service-rate-state.json  (deprecated compatibility; governor is in-process)");
    eprintln!("  --tls-cert ./server.pem");
    eprintln!("  --tls-key ./server-key.pem");
    eprintln!("  --tls-client-ca ./client-ca.pem");
    eprintln!("  --max-connections 512");
    eprintln!("  --read-timeout-ms 5000");
    eprintln!("  --public-requests-per-minute 1200");
    eprintln!("  --mutation-requests-per-minute 60");
    eprintln!("  --ingest-requests-per-minute 3000");
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
