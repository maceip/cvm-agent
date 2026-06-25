//! CLI wrapper for SDK scripts, strict workbench launches, TEE workspace
//! launches, and manual imports.

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use cvm_agent::llm_attested::{sha256_prefixed, EventActor};
use cvm_agent::llm_capture::{
    now_ms, signing_key_from_seed, CaptureReport, ParticipantConfig, ZERO_HASH,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[allow(dead_code)]
#[path = "llm-capture-daemon.rs"]
mod capture_daemon_bin;

#[derive(Debug, Clone)]
struct RunConfig {
    event_sink_url: String,
    hackathon_id: String,
    team_id: String,
    gateway_id: String,
    manifest_hash: String,
    capture_method: String,
    assurance: String,
    enforcement: String,
    agent_session_id: String,
    signing_seed: Option<String>,
    openai_base_url: Option<String>,
    openai_api_key: Option<String>,
    cvm_url: Option<String>,
    local_capture_base_url: Option<String>,
    command: Vec<String>,
}

impl RunConfig {
    fn from_participant(
        participant: &ParticipantConfig,
        capture_method: &str,
        assurance: &str,
        enforcement: &str,
    ) -> Self {
        let mut cfg = Self {
            event_sink_url: String::new(),
            hackathon_id: "hk_dev".to_string(),
            team_id: "team_dev".to_string(),
            gateway_id: "llmattest".to_string(),
            manifest_hash: ZERO_HASH.to_string(),
            capture_method: capture_method.to_string(),
            assurance: assurance.to_string(),
            enforcement: enforcement.to_string(),
            agent_session_id: "agent_cli".to_string(),
            signing_seed: None,
            openai_base_url: None,
            openai_api_key: None,
            cvm_url: None,
            local_capture_base_url: None,
            command: Vec::new(),
        };
        apply_participant_config_to_run(participant, &mut cfg);
        cfg
    }
}

#[derive(Debug, Clone)]
struct ParticipantStatus {
    config_path: PathBuf,
    cfg: ParticipantConfig,
    bootstrap_ok: bool,
    gateway_ok: bool,
    manifest_ok: bool,
    cvm_ok: bool,
    captured_events: u64,
    cvm_json: Option<Value>,
}

#[derive(Debug, Clone)]
struct StatusArgs {
    config_path: PathBuf,
    watch: bool,
    interval_ms: u64,
    iterations: Option<u64>,
}

#[derive(Debug)]
struct LocalDaemon {
    child: Child,
    base_url: String,
    events_path: PathBuf,
    pending_events_path: PathBuf,
    rate_state_path: PathBuf,
}

#[derive(Debug, Clone)]
struct LocalCompanionStatus {
    base_url: String,
    events_path: Option<PathBuf>,
    pending_events_path: Option<PathBuf>,
    rate_state_path: Option<PathBuf>,
}

impl Drop for LocalDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl LocalDaemon {
    fn companion_status(&self) -> LocalCompanionStatus {
        LocalCompanionStatus {
            base_url: self.base_url.clone(),
            events_path: Some(self.events_path.clone()),
            pending_events_path: Some(self.pending_events_path.clone()),
            rate_state_path: Some(self.rate_state_path.clone()),
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("__capture-daemon") => capture_daemon_bin::main_from_args(
            std::iter::once("llmattest __capture-daemon".to_string())
                .chain(args[2..].iter().cloned())
                .collect(),
        ),
        Some("start") => start_capture(&args[2..]),
        Some("init") => init_config(&args[2..]),
        Some("status") => print_status(&args[2..], false),
        Some("doctor") => print_status(&args[2..], true),
        Some("smoke") => smoke_capture(&args[2..]),
        Some("emit") => emit_report(&args[2..]),
        Some("notary-import") => import_notary_proof(&args[2..]),
        Some("run") => run_command(parse_run(
            &args[2..],
            "sdk_cli_wrapper",
            "participant_controlled",
            "routed",
        )?),
        Some("workbench-run") => run_command(parse_run(
            &args[2..],
            "strict_workbench",
            "managed",
            "strict_workbench",
        )?),
        Some("tee-run") => run_command(parse_run(
            &args[2..],
            "tee_workspace",
            "attested",
            "full_tee",
        )?),
        Some("-h" | "--help") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => Err(anyhow!("unknown subcommand {other}")),
    }
}

fn start_capture(args: &[String]) -> Result<()> {
    let mut team_payload_path: Option<String> = None;
    let mut config_path = ParticipantConfig::default_path();
    let mut signing_seed = None;
    let mut smoke = true;
    let mut command = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--" => {
                command = args[i + 1..].to_vec();
                break;
            }
            "--team-payload" | "--team-json" => {
                i += 1;
                team_payload_path = Some(arg_value(args, i, "--team-payload")?);
            }
            "--config" => {
                i += 1;
                config_path = PathBuf::from(arg_value(args, i, "--config")?);
            }
            "--signing-seed" => {
                i += 1;
                signing_seed = Some(arg_value(args, i, "--signing-seed")?);
            }
            "--no-smoke" => {
                smoke = false;
            }
            other => return Err(anyhow!("unknown start argument {other}")),
        }
        i += 1;
    }

    if let Some(team_payload_path) = team_payload_path {
        let body = read_text_arg(&team_payload_path).context("read team payload")?;
        let value: Value = serde_json::from_str(&body).context("decode team payload json")?;
        let cfg = ParticipantConfig::from_team_payload(&value).map_err(anyhow::Error::msg)?;
        cfg.save(&config_path).map_err(anyhow::Error::msg)?;
    }

    let cfg = ParticipantConfig::load(&config_path).map_err(anyhow::Error::msg)?;
    let mut daemon = start_local_daemon(&config_path, signing_seed.as_deref())?;
    let daemon_status = daemon.companion_status();
    let before = status_for_config(config_path.clone(), cfg.clone())?;
    let smoke_event = if smoke {
        Some(emit_smoke_event(
            &cfg,
            &config_path,
            signing_seed.as_deref(),
        )?)
    } else {
        None
    };
    let after = status_for_config(config_path.clone(), cfg.clone())?;
    let capability_probes = collect_capability_probes(&after, Some(&daemon_status));
    let selected_capture_path = select_capture_path(&capability_probes, !command.is_empty());

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "started": true,
            "config_path": config_path,
            "event_id": cfg.event_id,
            "team_id": cfg.team_id,
            "team_name": cfg.team_name,
            "cvm_url": cfg.cvm_url,
            "capture_runtime": {
                "mode": "llmattest_start",
                "supervisor": "child_process",
                "local_daemon": {
                    "status": "running",
                    "pid": daemon.child.id(),
                    "base_url": daemon.base_url,
                    "health_url": format!("{}/healthz", daemon.base_url),
                    "browser_capture_url": format!("{}/capture/browser", daemon.base_url),
                    "manual_capture_url": format!("{}/capture/manual", daemon.base_url),
                    "events_path": daemon.events_path,
                    "pending_events_path": daemon.pending_events_path,
                    "rate_state_path": daemon.rate_state_path
                }
            },
            "capability_probes": capability_probes,
            "selected_capture_path": selected_capture_path,
            "checks_before": {
                "bootstrap": before.bootstrap_ok,
                "gateway_health": before.gateway_ok,
                "gateway_manifest": before.manifest_ok,
                "cvm": before.cvm_ok,
                "captured_events": before.captured_events
            },
            "checks_after": {
                "bootstrap": after.bootstrap_ok,
                "gateway_health": after.gateway_ok,
                "gateway_manifest": after.manifest_ok,
                "cvm": after.cvm_ok,
                "captured_events": after.captured_events
            },
            "smoke_event_id": smoke_event.as_ref().map(|event| event.event_id.clone()),
            "openai_compatible": {
                "OPENAI_BASE_URL": cfg.openai_base_url(),
                "OPENAI_API_KEY": cfg.team_api_key,
                "LLM_ATTESTED_AGENT": "your-agent-or-ide-name",
                "LLM_ATTESTED_LOCAL_CAPTURE_URL": daemon.base_url
            }
        }))?
    );

    if !command.is_empty() {
        let mut run = RunConfig::from_participant(
            &cfg,
            "sdk_cli_wrapper",
            "participant_controlled",
            "routed",
        );
        run.signing_seed = signing_seed;
        run.local_capture_base_url = Some(daemon.base_url.clone());
        run.command = command;
        run_command(run)?;
    } else {
        eprintln!(
            "[llmattest] local capture daemon is running at {}. Press Ctrl-C to stop.",
            daemon.base_url
        );
        let status = daemon
            .child
            .wait()
            .context("wait for local capture daemon")?;
        if !status.success() {
            anyhow::bail!("local capture daemon exited with {status}");
        }
    }
    Ok(())
}

fn start_local_daemon(config_path: &Path, signing_seed: Option<&str>) -> Result<LocalDaemon> {
    let listen =
        std::env::var("LLMATTEST_DAEMON_LISTEN").unwrap_or_else(|_| "127.0.0.1:8090".to_string());
    let base_url = format!("http://{listen}");
    let state_dir = daemon_state_dir(config_path)?;
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("create llmattest state dir {}", state_dir.display()))?;
    let events_path = std::env::var("LLMATTEST_DAEMON_EVENTS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| state_dir.join("capture-events.cborl"));
    let pending_events_path = std::env::var("LLMATTEST_DAEMON_PENDING_EVENTS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| state_dir.join("capture-pending.cborl"));
    let rate_state_path = std::env::var("LLMATTEST_DAEMON_RATE_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| state_dir.join("capture-rate-state.json"));

    let mut command = Command::new(std::env::current_exe().context("resolve llmattest path")?);
    command
        .arg("__capture-daemon")
        .arg("--listen")
        .arg(&listen)
        .arg("--config")
        .arg(config_path)
        .arg("--events")
        .arg(&events_path)
        .arg("--pending-events")
        .arg(&pending_events_path)
        .arg("--rate-state")
        .arg(&rate_state_path)
        .arg("--requests-per-minute")
        .arg("10000");
    if let Some(signing_seed) = signing_seed {
        command.args(["--signing-seed", signing_seed]);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut child = command.spawn().context("start local capture daemon")?;
    wait_for_daemon(&mut child, &format!("{base_url}/healthz"))?;

    Ok(LocalDaemon {
        child,
        base_url,
        events_path,
        pending_events_path,
        rate_state_path,
    })
}

fn daemon_state_dir(config_path: &Path) -> Result<PathBuf> {
    if let Some(parent) = config_path.parent() {
        if !parent.as_os_str().is_empty() {
            return Ok(parent.to_path_buf());
        }
    }
    std::env::current_dir().context("resolve current directory for daemon state")
}

fn wait_for_daemon(child: &mut Child, health_url: &str) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_millis(250))
        .build()
        .context("build daemon health client")?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if let Some(status) = child.try_wait().context("poll local capture daemon")? {
            anyhow::bail!("local capture daemon exited during startup with {status}");
        }
        if get_success(&client, health_url) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    anyhow::bail!("local capture daemon did not become healthy at {health_url}");
}

fn collect_capability_probes(
    status: &ParticipantStatus,
    local_companion: Option<&LocalCompanionStatus>,
) -> Vec<Value> {
    let mut probes = Vec::new();
    probes.push(serde_json::json!({
        "id": "attested_gateway",
        "label": "Attested gateway",
        "available": status.gateway_ok && status.manifest_ok,
        "capture_method": "gateway",
        "assurance": "attested",
        "enforcement": "routed",
        "details": {
            "health_url": format!("{}/healthz", status.cfg.gateway_base_url.trim_end_matches('/')),
            "manifest_url": status.cfg.gateway_manifest_url
        }
    }));

    if let Some(companion) = local_companion {
        probes.push(serde_json::json!({
            "id": "local_capture_companion",
            "label": "Local browser/manual capture companion",
            "available": true,
            "capture_method": "browser_extension_or_manual",
            "assurance": "participant_controlled",
            "enforcement": "routed",
            "details": {
                "base_url": companion.base_url,
                "browser_capture_url": format!("{}/capture/browser", companion.base_url),
                "manual_capture_url": format!("{}/capture/manual", companion.base_url),
                "events_path": companion.events_path.clone(),
                "pending_events_path": companion.pending_events_path.clone(),
                "rate_state_path": companion.rate_state_path.clone()
            }
        }));
    }

    probes.push(serde_json::json!({
        "id": "sdk_cli_wrapper",
        "label": "SDK and CLI command wrapper",
        "available": true,
        "capture_method": "sdk_cli_wrapper",
        "assurance": "participant_controlled",
        "enforcement": "routed",
        "details": {
            "wrapper": "llmattest start -- <command>"
        }
    }));

    probes.push(env_probe(
        "managed_workbench",
        "Managed workbench",
        "LLM_ATTESTED_WORKBENCH",
        "strict_workbench",
        "managed",
        "strict_workbench",
    ));
    probes.push(tee_probe());

    for (id, label, env_name, default_url) in [
        (
            "ollama",
            "Ollama",
            "LLMATTEST_PROBE_OLLAMA_URL",
            "http://127.0.0.1:11434/api/tags",
        ),
        (
            "lm_studio",
            "LM Studio",
            "LLMATTEST_PROBE_LM_STUDIO_URL",
            "http://127.0.0.1:1234/v1/models",
        ),
        (
            "omlx",
            "oMLX",
            "LLMATTEST_PROBE_OMLX_URL",
            "http://127.0.0.1:8000/v1/models",
        ),
        (
            "mlx_omni_server",
            "MLX Omni Server",
            "LLMATTEST_PROBE_MLX_OMNI_SERVER_URL",
            "http://127.0.0.1:8000/v1/models",
        ),
        (
            "llama_cpp",
            "llama.cpp server",
            "LLMATTEST_PROBE_LLAMA_CPP_URL",
            "http://127.0.0.1:8080/v1/models",
        ),
        (
            "vllm",
            "vLLM OpenAI server",
            "LLMATTEST_PROBE_VLLM_URL",
            "http://127.0.0.1:8000/v1/models",
        ),
    ] {
        let url = std::env::var(env_name).unwrap_or_else(|_| default_url.to_string());
        probes.push(local_runtime_probe(id, label, url));
    }
    probes
}

fn select_capture_path(probes: &[Value], command_present: bool) -> Value {
    let preferred = if command_present {
        [
            "tee_workspace",
            "attested_gateway",
            "managed_workbench",
            "sdk_cli_wrapper",
            "local_capture_companion",
        ]
    } else {
        [
            "tee_workspace",
            "attested_gateway",
            "managed_workbench",
            "local_capture_companion",
            "sdk_cli_wrapper",
        ]
    };

    for id in preferred {
        let Some(probe) = probes
            .iter()
            .find(|probe| probe.get("id").and_then(Value::as_str) == Some(id))
        else {
            continue;
        };
        if probe
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return serde_json::json!({
                "id": id,
                "label": probe.get("label").cloned().unwrap_or(Value::Null),
                "capture_method": probe.get("capture_method").cloned().unwrap_or(Value::Null),
                "assurance": probe.get("assurance").cloned().unwrap_or(Value::Null),
                "enforcement": probe.get("enforcement").cloned().unwrap_or(Value::Null),
                "reason": selection_reason(id),
                "user_configurable": false
            });
        }
    }

    serde_json::json!({
        "id": "none",
        "available": false,
        "reason": "no supported capture path was available",
        "user_configurable": false
    })
}

fn selection_reason(id: &str) -> &'static str {
    match id {
        "tee_workspace" => "TEE evidence was detected on this host",
        "attested_gateway" => "event gateway health and manifest checks passed",
        "managed_workbench" => "managed workbench marker was present",
        "sdk_cli_wrapper" => "a wrapped command was provided to llmattest start",
        "local_capture_companion" => "local browser/manual capture companion is running",
        _ => "first available capture path in policy order",
    }
}

fn env_probe(
    id: &str,
    label: &str,
    env_name: &str,
    capture_method: &str,
    assurance: &str,
    enforcement: &str,
) -> Value {
    let value = std::env::var(env_name).ok();
    serde_json::json!({
        "id": id,
        "label": label,
        "available": value.as_deref().map(env_truthy).unwrap_or(false),
        "capture_method": capture_method,
        "assurance": assurance,
        "enforcement": enforcement,
        "details": {
            "env": env_name
        }
    })
}

fn tee_probe() -> Value {
    let mut evidence = Vec::new();
    for env_name in [
        "CVM_TEE_QUOTE_PATH",
        "LLM_ATTESTED_TEE_QUOTE_PATH",
        "TDX_REPORT_PATH",
        "SNP_REPORT_PATH",
    ] {
        if let Ok(path) = std::env::var(env_name) {
            if Path::new(&path).exists() {
                evidence.push(format!("{env_name}={path}"));
            }
        }
    }
    for path in ["/dev/tdx_guest", "/dev/sev-guest", "/dev/nsm"] {
        if Path::new(path).exists() {
            evidence.push(path.to_string());
        }
    }
    serde_json::json!({
        "id": "tee_workspace",
        "label": "TEE workspace",
        "available": !evidence.is_empty(),
        "capture_method": "tee_workspace",
        "assurance": "attested",
        "enforcement": "full_tee",
        "details": {
            "evidence": evidence
        }
    })
}

fn local_runtime_probe(id: &str, label: &str, url: String) -> Value {
    serde_json::json!({
        "id": id,
        "label": label,
        "available": probe_http(&url, Duration::from_millis(150)),
        "capture_method": "local_proxy",
        "assurance": "participant_controlled",
        "enforcement": "routed",
        "details": {
            "probe_url": url
        }
    })
}

fn probe_http(url: &str, timeout: Duration) -> bool {
    let Ok(client) = Client::builder().timeout(timeout).build() else {
        return false;
    };
    client
        .get(url)
        .send()
        .map(|resp| resp.status().is_success())
        .unwrap_or(false)
}

fn env_truthy(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

fn init_config(args: &[String]) -> Result<()> {
    let mut team_payload_path: Option<String> = None;
    let mut config_path = ParticipantConfig::default_path();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--team-payload" | "--team-json" => {
                i += 1;
                team_payload_path = Some(arg_value(args, i, "--team-payload")?);
            }
            "--config" => {
                i += 1;
                config_path = PathBuf::from(arg_value(args, i, "--config")?);
            }
            other => return Err(anyhow!("unknown init argument {other}")),
        }
        i += 1;
    }
    let team_payload_path =
        team_payload_path.ok_or_else(|| anyhow!("--team-payload is required"))?;
    let body = read_text_arg(&team_payload_path).context("read team payload")?;
    let value: Value = serde_json::from_str(&body).context("decode team payload json")?;
    let cfg = ParticipantConfig::from_team_payload(&value).map_err(anyhow::Error::msg)?;
    cfg.save(&config_path).map_err(anyhow::Error::msg)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "initialized": true,
            "config_path": config_path,
            "event_id": cfg.event_id,
            "team_id": cfg.team_id,
            "team_name": cfg.team_name,
            "cvm_url": cfg.cvm_url,
            "openai_compatible": {
                "OPENAI_BASE_URL": cfg.openai_base_url(),
                "OPENAI_API_KEY": cfg.team_api_key,
                "LLM_ATTESTED_AGENT": "your-agent-or-ide-name"
            }
        }))?
    );
    Ok(())
}

fn print_status(args: &[String], strict: bool) -> Result<()> {
    let args = parse_status_args(args)?;
    let mut emitted = 0u64;
    loop {
        let status = status_for_path(args.config_path.clone())?;
        let document = participant_status_document(&status)?;
        if args.watch {
            println!("{}", serde_json::to_string(&document)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&document)?);
        }

        let ready = document
            .get("ready")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let strict_ready = document
            .get("strict_ready")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if strict && !strict_ready {
            anyhow::bail!("capture doctor failed");
        }
        if !args.watch {
            break;
        }
        emitted = emitted.saturating_add(1);
        if args
            .iterations
            .map(|limit| emitted >= limit)
            .unwrap_or(false)
        {
            break;
        }
        if !ready {
            std::thread::sleep(Duration::from_millis(args.interval_ms.min(10_000)));
        } else {
            std::thread::sleep(Duration::from_millis(args.interval_ms));
        }
    }
    Ok(())
}

fn parse_status_args(args: &[String]) -> Result<StatusArgs> {
    let mut parsed = StatusArgs {
        config_path: ParticipantConfig::default_path(),
        watch: false,
        interval_ms: 2_000,
        iterations: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                i += 1;
                parsed.config_path = PathBuf::from(arg_value(args, i, "--config")?);
            }
            "--watch" => {
                parsed.watch = true;
            }
            "--interval-ms" => {
                i += 1;
                parsed.interval_ms = parse_u64_arg(args, i, "--interval-ms")?.max(100);
            }
            "--iterations" => {
                i += 1;
                parsed.iterations = Some(parse_u64_arg(args, i, "--iterations")?.max(1));
            }
            other => return Err(anyhow!("unknown status argument {other}")),
        }
        i += 1;
    }
    Ok(parsed)
}

fn participant_status_document(status: &ParticipantStatus) -> Result<Value> {
    let ready = status.bootstrap_ok && status.cvm_ok;
    let local_companion = discover_local_companion(&status.config_path)?;
    let capability_probes = collect_capability_probes(status, local_companion.as_ref());
    let active_path = select_capture_path(&capability_probes, false);
    let client = Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .context("build status document client")?;
    let latest_event = latest_event_document(&client, status.cvm_json.as_ref());
    let actions = status_actions(status, local_companion.as_ref());

    Ok(serde_json::json!({
        "ready": ready,
        "strict_ready": ready && status.gateway_ok && status.manifest_ok,
        "config_path": status.config_path,
        "event_id": status.cfg.event_id,
        "team_id": status.cfg.team_id,
        "team_name": status.cfg.team_name,
        "cvm_url": status.cfg.cvm_url,
        "captured_events": status.captured_events,
        "active_capture_path": active_path,
        "capability_probes": capability_probes,
        "latest_event": latest_event,
        "actions": actions,
        "checks": {
            "bootstrap": status.bootstrap_ok,
            "gateway_health": status.gateway_ok,
            "gateway_manifest": status.manifest_ok,
            "cvm": status.cvm_ok,
            "local_companion": local_companion.is_some()
        },
        "live": {
            "cvm_url": status.cfg.cvm_url,
            "cvm_json_url": status.cfg.cvm_json_url,
            "proof_url": status.cvm_json.as_ref().and_then(|card| card.pointer("/share/proof_url")).cloned(),
            "receipts_url": status.cvm_json.as_ref().and_then(|card| card.pointer("/share/receipts_url")).cloned(),
            "event_count": status.cvm_json.as_ref().and_then(|card| card.pointer("/integrity/event_count")).cloned().unwrap_or_else(|| serde_json::json!(status.captured_events)),
            "latest_event_hash": status.cvm_json.as_ref().and_then(|card| card.pointer("/integrity/latest_event_hash")).cloned()
        },
        "openai_compatible": {
            "OPENAI_BASE_URL": status.cfg.openai_base_url(),
            "OPENAI_API_KEY": status.cfg.team_api_key,
            "LLM_ATTESTED_AGENT": "your-agent-or-ide-name"
        }
    }))
}

fn smoke_capture(args: &[String]) -> Result<()> {
    let mut config_path = ParticipantConfig::default_path();
    let mut signing_seed = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                i += 1;
                config_path = PathBuf::from(arg_value(args, i, "--config")?);
            }
            "--signing-seed" => {
                i += 1;
                signing_seed = Some(arg_value(args, i, "--signing-seed")?);
            }
            other => return Err(anyhow!("unknown smoke argument {other}")),
        }
        i += 1;
    }
    let cfg = ParticipantConfig::load(&config_path).map_err(anyhow::Error::msg)?;
    let event = emit_smoke_event(&cfg, &config_path, signing_seed.as_deref())?;
    let status = status_for_config(config_path, cfg)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "accepted": true,
            "event_id": event.event_id,
            "event_hash": event.event_hash().unwrap_or_else(|_| "sha256:unknown".to_string()),
            "cvm_url": status.cfg.cvm_url,
            "captured_events": status.captured_events
        }))?
    );
    Ok(())
}

fn emit_smoke_event(
    cfg: &ParticipantConfig,
    config_path: &std::path::Path,
    signing_seed: Option<&str>,
) -> Result<cvm_agent::llm_attested::ContestEvent> {
    let now = now_ms();
    let mut subject = BTreeMap::new();
    subject.insert("provider".to_string(), "llmattest".to_string());
    subject.insert("api_family".to_string(), "capture-smoke".to_string());
    subject.insert("model".to_string(), "none".to_string());

    let mut evidence = BTreeMap::new();
    evidence.insert("enforcement_mode".to_string(), "setup_smoke".to_string());
    evidence.insert("config_path".to_string(), config_path.display().to_string());
    evidence.insert("smoke_test".to_string(), "true".to_string());

    let report = CaptureReport {
        kind: "capture.health".to_string(),
        hackathon_id: cfg.event_id.clone(),
        team_id: cfg.team_id.clone(),
        gateway_id: "llmattest".to_string(),
        manifest_hash: ZERO_HASH.to_string(),
        capture_method: "setup_smoke".to_string(),
        assurance: "participant_controlled".to_string(),
        event_index: 0,
        previous_team_event_hash: ZERO_HASH.to_string(),
        started_at_ms: now,
        completed_at_ms: now,
        actor: EventActor {
            kind: "participant".to_string(),
            agent_session_id: "llmattest_smoke".to_string(),
        },
        subject,
        metrics: BTreeMap::new(),
        hashes: BTreeMap::new(),
        evidence,
    };
    let key = signing_key_from_seed(signing_seed);
    let event = report.sign(&key).context("sign smoke event")?;
    post_event(&cfg.event_sink_url, &event)?;
    Ok(event)
}

fn emit_report(args: &[String]) -> Result<()> {
    let mut event_sink_url = String::new();
    let mut report_path: Option<String> = None;
    let mut signing_seed = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--event-sink-url" => {
                i += 1;
                event_sink_url = arg_value(args, i, "--event-sink-url")?;
            }
            "--report" => {
                i += 1;
                report_path = Some(arg_value(args, i, "--report")?);
            }
            "--signing-seed" => {
                i += 1;
                signing_seed = Some(arg_value(args, i, "--signing-seed")?);
            }
            other => return Err(anyhow!("unknown emit argument {other}")),
        }
        i += 1;
    }
    if event_sink_url.is_empty() {
        anyhow::bail!("--event-sink-url is required");
    }
    let body = match report_path.as_deref() {
        Some("-") | None => {
            let mut body = String::new();
            std::io::stdin()
                .read_to_string(&mut body)
                .context("read report from stdin")?;
            body
        }
        Some(path) => std::fs::read_to_string(path).with_context(|| format!("read {path}"))?,
    };
    let report: CaptureReport = serde_json::from_str(&body).context("decode capture report")?;
    let key = signing_key_from_seed(signing_seed.as_deref());
    let event = report.sign(&key).context("sign capture report")?;
    post_event(&event_sink_url, &event)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "accepted": true,
            "event_id": event.event_id,
            "capture_method": event.capture_method,
            "assurance": event.assurance,
        }))?
    );
    Ok(())
}

fn status_for_path(config_path: PathBuf) -> Result<ParticipantStatus> {
    let cfg = ParticipantConfig::load(&config_path).map_err(anyhow::Error::msg)?;
    status_for_config(config_path, cfg)
}

fn status_for_config(config_path: PathBuf, cfg: ParticipantConfig) -> Result<ParticipantStatus> {
    let client = Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .context("build status client")?;
    let bootstrap_ok = get_success(&client, &cfg.bootstrap_url);
    let gateway_ok = get_success(
        &client,
        &format!("{}/healthz", cfg.gateway_base_url.trim_end_matches('/')),
    );
    let manifest_ok = get_success(&client, &cfg.gateway_manifest_url);
    let cvm = get_json_value(&client, &cfg.cvm_json_url);
    let cvm_ok = cvm.is_some();
    let captured_events = cvm
        .as_ref()
        .and_then(|value| value.get("stats"))
        .and_then(|value| value.get("captured_events"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Ok(ParticipantStatus {
        config_path,
        cfg,
        bootstrap_ok,
        gateway_ok,
        manifest_ok,
        cvm_ok,
        captured_events,
        cvm_json: cvm,
    })
}

fn discover_local_companion(config_path: &Path) -> Result<Option<LocalCompanionStatus>> {
    let listen =
        std::env::var("LLMATTEST_DAEMON_LISTEN").unwrap_or_else(|_| "127.0.0.1:8090".to_string());
    let base_url = format!("http://{listen}");
    if !probe_http(&format!("{base_url}/healthz"), Duration::from_millis(250)) {
        return Ok(None);
    }
    let state_dir = daemon_state_dir(config_path)?;
    Ok(Some(LocalCompanionStatus {
        base_url,
        events_path: Some(
            std::env::var("LLMATTEST_DAEMON_EVENTS")
                .map(PathBuf::from)
                .unwrap_or_else(|_| state_dir.join("capture-events.cborl")),
        ),
        pending_events_path: Some(
            std::env::var("LLMATTEST_DAEMON_PENDING_EVENTS")
                .map(PathBuf::from)
                .unwrap_or_else(|_| state_dir.join("capture-pending.cborl")),
        ),
        rate_state_path: Some(
            std::env::var("LLMATTEST_DAEMON_RATE_STATE")
                .map(PathBuf::from)
                .unwrap_or_else(|_| state_dir.join("capture-rate-state.json")),
        ),
    }))
}

fn latest_event_document(client: &Client, cvm: Option<&Value>) -> Value {
    let Some(card) = cvm else {
        return serde_json::json!({
            "available": false,
            "reason": "cvm unavailable"
        });
    };
    let latest_hash = card
        .pointer("/integrity/latest_event_hash")
        .and_then(Value::as_str)
        .unwrap_or("sha256:empty")
        .to_string();
    let receipts_url = card.pointer("/share/receipts_url").and_then(Value::as_str);
    let Some(receipts_url) = receipts_url else {
        return serde_json::json!({
            "available": latest_hash != "sha256:empty",
            "event_hash": latest_hash,
            "reason": "receipts URL unavailable"
        });
    };
    let Some(receipts) = get_json_value(client, receipts_url) else {
        return serde_json::json!({
            "available": latest_hash != "sha256:empty",
            "event_hash": latest_hash,
            "receipts_url": receipts_url,
            "reason": "receipts unavailable"
        });
    };
    let latest = receipts
        .get("receipts")
        .and_then(Value::as_array)
        .and_then(|items| items.last())
        .cloned();
    match latest {
        Some(receipt) => serde_json::json!({
            "available": true,
            "event_id": receipt.get("event_id").cloned(),
            "kind": receipt.get("kind").cloned(),
            "event_hash": receipt.get("event_hash").cloned().unwrap_or_else(|| serde_json::json!(latest_hash)),
            "receipts_url": receipts_url
        }),
        None => serde_json::json!({
            "available": false,
            "event_hash": latest_hash,
            "receipts_url": receipts_url,
            "reason": "no captured receipts yet"
        }),
    }
}

fn status_actions(
    status: &ParticipantStatus,
    local_companion: Option<&LocalCompanionStatus>,
) -> Vec<Value> {
    let mut actions = Vec::new();
    if !status.bootstrap_ok {
        actions.push(serde_json::json!({
            "severity": "error",
            "code": "bootstrap_unreachable",
            "message": "Event bootstrap is unreachable.",
            "next_step": "Check the join URL or self-hosted event service URL, then rerun llmattest status."
        }));
    }
    if !status.cvm_ok {
        actions.push(serde_json::json!({
            "severity": "error",
            "code": "cvm_unreachable",
            "message": "The live cvm is unreachable.",
            "next_step": "Regenerate the team payload from the join page or verify the event service is online."
        }));
    }
    if !status.gateway_ok || !status.manifest_ok {
        actions.push(serde_json::json!({
            "severity": "warning",
            "code": "attested_gateway_unavailable",
            "message": "The attested gateway path is not currently available.",
            "next_step": "Wrapper and local companion capture can still work; organizer-managed attested gateway scoring should wait for gateway health and manifest checks to pass."
        }));
    }
    if local_companion.is_none() {
        actions.push(serde_json::json!({
            "severity": "info",
            "code": "local_companion_not_running",
            "message": "The local browser/manual capture companion is not running.",
            "next_step": "Run llmattest start with the team payload before using browser or manual capture paths."
        }));
    }
    if status.captured_events == 0 {
        actions.push(serde_json::json!({
            "severity": "info",
            "code": "no_events_yet",
            "message": "No captured events are visible on the cvm yet.",
            "next_step": "Run llmattest smoke or start an agent through llmattest start."
        }));
    }
    actions
}

fn import_notary_proof(args: &[String]) -> Result<()> {
    let mut event_sink_url = String::new();
    let mut hackathon_id = "hk_dev".to_string();
    let mut team_id = "team_dev".to_string();
    let mut provider = "provider".to_string();
    let mut target_origin = String::new();
    let mut notary_url = String::new();
    let mut account_scope = "participant_selected_account".to_string();
    let mut proof_path: Option<String> = None;
    let mut claim_path: Option<String> = None;
    let mut signing_seed = None;
    let mut total_tokens: Option<i64> = None;
    let mut llm_calls: Option<i64> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--event-sink-url" => {
                i += 1;
                event_sink_url = arg_value(args, i, "--event-sink-url")?;
            }
            "--hackathon-id" => {
                i += 1;
                hackathon_id = arg_value(args, i, "--hackathon-id")?;
            }
            "--team-id" => {
                i += 1;
                team_id = arg_value(args, i, "--team-id")?;
            }
            "--provider" => {
                i += 1;
                provider = arg_value(args, i, "--provider")?;
            }
            "--target-origin" => {
                i += 1;
                target_origin = arg_value(args, i, "--target-origin")?;
            }
            "--notary-url" => {
                i += 1;
                notary_url = arg_value(args, i, "--notary-url")?;
            }
            "--account-scope" => {
                i += 1;
                account_scope = arg_value(args, i, "--account-scope")?;
            }
            "--proof" => {
                i += 1;
                proof_path = Some(arg_value(args, i, "--proof")?);
            }
            "--claim" => {
                i += 1;
                claim_path = Some(arg_value(args, i, "--claim")?);
            }
            "--total-tokens" => {
                i += 1;
                total_tokens = Some(parse_i64_arg(args, i, "--total-tokens")?);
            }
            "--llm-calls" => {
                i += 1;
                llm_calls = Some(parse_i64_arg(args, i, "--llm-calls")?);
            }
            "--signing-seed" => {
                i += 1;
                signing_seed = Some(arg_value(args, i, "--signing-seed")?);
            }
            other => return Err(anyhow!("unknown notary-import argument {other}")),
        }
        i += 1;
    }

    if event_sink_url.is_empty() {
        anyhow::bail!("--event-sink-url is required");
    }
    let proof_path = proof_path.ok_or_else(|| anyhow!("--proof is required"))?;
    let proof = std::fs::read(&proof_path).with_context(|| format!("read proof {proof_path}"))?;
    let proof_hash = sha256_prefixed(&proof);
    let claim = match claim_path.as_deref() {
        Some(path) => {
            let body = std::fs::read_to_string(path).with_context(|| format!("read {path}"))?;
            let value: Value = serde_json::from_str(&body).context("decode notary claim json")?;
            Some((path.to_string(), value, sha256_prefixed(body.as_bytes())))
        }
        None => None,
    };

    let now = now_ms();
    let mut subject = BTreeMap::new();
    subject.insert("provider".to_string(), provider.clone());
    subject.insert(
        "api_family".to_string(),
        "provider_account_usage_snapshot".to_string(),
    );
    subject.insert("account_scope".to_string(), account_scope.clone());
    if !target_origin.is_empty() {
        subject.insert("target_origin".to_string(), target_origin.clone());
    }
    if let Some((_, claim, _)) = &claim {
        merge_string_object(&mut subject, claim.get("subject"));
        if let Some(model) = claim.get("model").and_then(Value::as_str) {
            subject.insert("model".to_string(), model.to_string());
        }
    }

    let mut metrics = BTreeMap::new();
    if let Some((_, claim, _)) = &claim {
        merge_i64_object(&mut metrics, claim.get("metrics"));
    }
    if let Some(total_tokens) = total_tokens {
        metrics.insert("total_tokens".to_string(), total_tokens);
    }
    if let Some(llm_calls) = llm_calls {
        metrics.insert("llm_calls".to_string(), llm_calls);
    }

    let mut hashes = BTreeMap::new();
    hashes.insert("tls_notary_proof".to_string(), proof_hash.clone());
    if let Some((_, _, claim_hash)) = &claim {
        hashes.insert("provider_usage_claim".to_string(), claim_hash.clone());
    }

    let mut evidence = BTreeMap::new();
    evidence.insert(
        "enforcement_mode".to_string(),
        "retrospective_proof".to_string(),
    );
    evidence.insert(
        "notary_protocol".to_string(),
        "tlsnotary-mpc-tls".to_string(),
    );
    evidence.insert("notary_proof_hash".to_string(), proof_hash);
    evidence.insert("account_scope".to_string(), account_scope);
    evidence.insert(
        "completeness".to_string(),
        "provider_account_scoped_not_global".to_string(),
    );
    evidence.insert(
        "limitation".to_string(),
        "does_not_prove_absence_of_other_accounts_devices_or_providers".to_string(),
    );
    if !notary_url.is_empty() {
        evidence.insert("notary_url".to_string(), notary_url);
    }
    if !target_origin.is_empty() {
        evidence.insert("target_origin".to_string(), target_origin);
    }
    if let Some((claim_path, _, claim_hash)) = &claim {
        evidence.insert("claim_path".to_string(), claim_path.clone());
        evidence.insert("claim_hash".to_string(), claim_hash.clone());
    }

    let report = CaptureReport {
        kind: "provider.usage_snapshot".to_string(),
        hackathon_id,
        team_id,
        gateway_id: "tlsnotary_import".to_string(),
        manifest_hash: ZERO_HASH.to_string(),
        capture_method: "provider_tls_notary".to_string(),
        assurance: "participant_controlled".to_string(),
        event_index: 0,
        previous_team_event_hash: ZERO_HASH.to_string(),
        started_at_ms: now,
        completed_at_ms: now,
        actor: EventActor {
            kind: "participant".to_string(),
            agent_session_id: "provider_account_snapshot".to_string(),
        },
        subject,
        metrics,
        hashes,
        evidence,
    };
    let key = signing_key_from_seed(signing_seed.as_deref());
    let event = report.sign(&key).context("sign notary import")?;
    post_event(&event_sink_url, &event)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "accepted": true,
            "event_id": event.event_id,
            "capture_method": event.capture_method,
            "assurance": event.assurance,
            "enforcement": event.evidence.get("enforcement_mode"),
        }))?
    );
    Ok(())
}

fn run_command(cfg: RunConfig) -> Result<()> {
    if cfg.command.is_empty() {
        anyhow::bail!("command required after --");
    }
    let started = now_ms();
    let mut command = Command::new(&cfg.command[0]);
    command.args(&cfg.command[1..]);
    command.env("LLM_ATTESTED_TEAM_ID", &cfg.team_id);
    command.env("LLM_ATTESTED_AGENT", &cfg.agent_session_id);
    command.env("LLM_ATTESTED_CAPTURE_METHOD", &cfg.capture_method);
    command.env("LLM_ATTESTED_ASSURANCE", &cfg.assurance);
    command.env("LLM_ATTESTED_ENFORCEMENT", &cfg.enforcement);
    if let Some(openai_base_url) = &cfg.openai_base_url {
        command.env("OPENAI_BASE_URL", openai_base_url);
    }
    if let Some(openai_api_key) = &cfg.openai_api_key {
        command.env("OPENAI_API_KEY", openai_api_key);
    }
    if let Some(cvm_url) = &cfg.cvm_url {
        command.env("LLM_ATTESTED_CVM_URL", cvm_url);
    }
    if let Some(local_capture_base_url) = &cfg.local_capture_base_url {
        command.env("LLM_ATTESTED_LOCAL_CAPTURE_URL", local_capture_base_url);
        command.env(
            "LLM_ATTESTED_BROWSER_CAPTURE_URL",
            format!("{local_capture_base_url}/capture/browser"),
        );
        command.env(
            "LLM_ATTESTED_MANUAL_CAPTURE_URL",
            format!("{local_capture_base_url}/capture/manual"),
        );
    }
    let status = command
        .status()
        .with_context(|| format!("run {}", cfg.command.join(" ")))?;
    let completed = now_ms();

    let mut subject = BTreeMap::new();
    subject.insert("command".to_string(), cfg.command[0].clone());
    subject.insert(
        "command_line_hash".to_string(),
        sha256_prefixed(cfg.command.join("\0").as_bytes()),
    );

    let mut metrics = BTreeMap::new();
    metrics.insert("exit_code".to_string(), status.code().unwrap_or(-1) as i64);
    metrics.insert(
        "duration_ms".to_string(),
        completed.saturating_sub(started) as i64,
    );

    let mut evidence = BTreeMap::new();
    evidence.insert("enforcement_mode".to_string(), cfg.enforcement.clone());
    evidence.insert("wrapper".to_string(), "llmattest".to_string());
    if cfg.capture_method == "tee_workspace" {
        evidence.insert("tee_workspace_required".to_string(), "true".to_string());
    }

    let report = CaptureReport {
        kind: "agent.session".to_string(),
        hackathon_id: cfg.hackathon_id.clone(),
        team_id: cfg.team_id.clone(),
        gateway_id: cfg.gateway_id.clone(),
        manifest_hash: cfg.manifest_hash.clone(),
        capture_method: cfg.capture_method.clone(),
        assurance: cfg.assurance.clone(),
        event_index: 0,
        previous_team_event_hash: ZERO_HASH.to_string(),
        started_at_ms: started,
        completed_at_ms: completed,
        actor: EventActor {
            kind: "agent".to_string(),
            agent_session_id: cfg.agent_session_id.clone(),
        },
        subject,
        metrics,
        hashes: BTreeMap::new(),
        evidence,
    };
    let key = signing_key_from_seed(cfg.signing_seed.as_deref());
    let event = report.sign(&key).context("sign wrapper event")?;
    post_event(&cfg.event_sink_url, &event)?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn parse_run(
    args: &[String],
    capture_method: &str,
    assurance: &str,
    enforcement: &str,
) -> Result<RunConfig> {
    let mut cfg = RunConfig {
        event_sink_url: String::new(),
        hackathon_id: "hk_dev".to_string(),
        team_id: "team_dev".to_string(),
        gateway_id: "llmattest".to_string(),
        manifest_hash: ZERO_HASH.to_string(),
        capture_method: capture_method.to_string(),
        assurance: assurance.to_string(),
        enforcement: enforcement.to_string(),
        agent_session_id: "agent_cli".to_string(),
        signing_seed: None,
        openai_base_url: None,
        openai_api_key: None,
        cvm_url: None,
        local_capture_base_url: None,
        command: Vec::new(),
    };
    if let Some(config_path) = find_arg_value(args, "--config")? {
        let participant =
            ParticipantConfig::load(&PathBuf::from(config_path)).map_err(anyhow::Error::msg)?;
        apply_participant_config_to_run(&participant, &mut cfg);
    }
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--" => {
                cfg.command = args[i + 1..].to_vec();
                break;
            }
            "--event-sink-url" => {
                i += 1;
                cfg.event_sink_url = arg_value(args, i, "--event-sink-url")?;
            }
            "--config" => {
                i += 1;
                let _ = arg_value(args, i, "--config")?;
            }
            "--hackathon-id" => {
                i += 1;
                cfg.hackathon_id = arg_value(args, i, "--hackathon-id")?;
            }
            "--team-id" => {
                i += 1;
                cfg.team_id = arg_value(args, i, "--team-id")?;
            }
            "--gateway-id" => {
                i += 1;
                cfg.gateway_id = arg_value(args, i, "--gateway-id")?;
            }
            "--manifest-hash" => {
                i += 1;
                cfg.manifest_hash = arg_value(args, i, "--manifest-hash")?;
            }
            "--capture-method" => {
                i += 1;
                cfg.capture_method = arg_value(args, i, "--capture-method")?;
            }
            "--assurance" => {
                i += 1;
                cfg.assurance = arg_value(args, i, "--assurance")?;
            }
            "--enforcement" => {
                i += 1;
                cfg.enforcement = arg_value(args, i, "--enforcement")?;
            }
            "--agent-session-id" => {
                i += 1;
                cfg.agent_session_id = arg_value(args, i, "--agent-session-id")?;
            }
            "--signing-seed" => {
                i += 1;
                cfg.signing_seed = Some(arg_value(args, i, "--signing-seed")?);
            }
            other => return Err(anyhow!("unknown run argument {other}")),
        }
        i += 1;
    }
    if cfg.event_sink_url.is_empty() {
        anyhow::bail!("--event-sink-url is required");
    }
    Ok(cfg)
}

fn apply_participant_config_to_run(participant: &ParticipantConfig, cfg: &mut RunConfig) {
    cfg.event_sink_url = participant.event_sink_url.clone();
    cfg.hackathon_id = participant.event_id.clone();
    cfg.team_id = participant.team_id.clone();
    cfg.openai_base_url = Some(participant.openai_base_url());
    cfg.openai_api_key = Some(participant.team_api_key.clone());
    cfg.cvm_url = Some(participant.cvm_url.clone());
}

fn parse_i64_arg(args: &[String], index: usize, name: &str) -> Result<i64> {
    arg_value(args, index, name)?
        .parse()
        .map_err(|e| anyhow!("invalid value for {name}: {e}"))
}

fn parse_u64_arg(args: &[String], index: usize, name: &str) -> Result<u64> {
    arg_value(args, index, name)?
        .parse()
        .map_err(|e| anyhow!("invalid value for {name}: {e}"))
}

fn merge_string_object(out: &mut BTreeMap<String, String>, value: Option<&Value>) {
    let Some(map) = value.and_then(Value::as_object) else {
        return;
    };
    for (key, value) in map {
        if let Some(value) = value.as_str() {
            out.insert(key.clone(), value.to_string());
        }
    }
}

fn merge_i64_object(out: &mut BTreeMap<String, i64>, value: Option<&Value>) {
    let Some(map) = value.and_then(Value::as_object) else {
        return;
    };
    for (key, value) in map {
        if let Some(value) = value.as_i64() {
            out.insert(key.clone(), value);
        }
    }
}

fn post_event(url: &str, event: &cvm_agent::llm_attested::ContestEvent) -> Result<()> {
    let body = event.to_cbor().context("encode event")?;
    let resp = Client::new()
        .post(url)
        .header(CONTENT_TYPE, "application/cbor")
        .body(body)
        .send()
        .context("post event")?;
    if !resp.status().is_success() {
        anyhow::bail!("event sink returned {}", resp.status());
    }
    Ok(())
}

fn read_text_arg(path: &str) -> Result<String> {
    if path == "-" {
        let mut body = String::new();
        std::io::stdin()
            .read_to_string(&mut body)
            .context("read stdin")?;
        Ok(body)
    } else {
        std::fs::read_to_string(path).with_context(|| format!("read {path}"))
    }
}

fn get_success(client: &Client, url: &str) -> bool {
    client
        .get(url)
        .send()
        .map(|resp| resp.status().is_success())
        .unwrap_or(false)
}

fn get_json_value(client: &Client, url: &str) -> Option<Value> {
    let resp = client.get(url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().ok()?;
    serde_json::from_str(&body).ok()
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

fn arg_value(args: &[String], index: usize, name: &str) -> Result<String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| anyhow!("{name} requires a value"))
}

fn print_usage() {
    eprintln!("llmattest - capture wrapper and manual import tool");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  llmattest init --team-payload team.json");
    eprintln!("  llmattest status --config ~/.llmattest/config.json");
    eprintln!("  llmattest status --watch --interval-ms 2000");
    eprintln!("  llmattest doctor --config ~/.llmattest/config.json");
    eprintln!("  llmattest smoke --config ~/.llmattest/config.json");
    eprintln!("  llmattest emit --event-sink-url URL --report report.json");
    eprintln!(
        "  llmattest notary-import --event-sink-url URL --proof tlsnotary.tlsn --team-id team"
    );
    eprintln!("  llmattest run --config ~/.llmattest/config.json -- command args...");
    eprintln!("  llmattest workbench-run --event-sink-url URL -- command args...");
    eprintln!("  llmattest tee-run --event-sink-url URL -- command args...");
}
