use base64::Engine as _;
use cvm_agent::eat::EatToken;
use cvm_agent::llm_attested::{ContestEvent, CAPTURE_RA_PROFILE};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct EventFixture {
    _service: ChildGuard,
    base_url: String,
    event_id: String,
    event_sink_url: String,
    team_id: String,
    team_key: String,
    team_payload: Value,
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start_event_fixture(name: &str) -> EventFixture {
    start_event_fixture_with_gateway(name, None)
}

fn start_event_fixture_with_gateway(name: &str, gateway_base_url: Option<String>) -> EventFixture {
    let port = free_port();
    let base_url = format!("http://127.0.0.1:{port}");
    let temp = tempfile::tempdir().unwrap().keep();
    let store = temp.join("store.json");
    let logs = temp.join("logs");
    let rate = temp.join("rate.json");
    let child = Command::new(env!("CARGO_BIN_EXE_llm-event-service"))
        .args([
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--public-base-url",
            &base_url,
            "--gateway-base-url",
            "http://127.0.0.1:9",
            "--store",
            store.to_str().unwrap(),
            "--event-log-dir",
            logs.to_str().unwrap(),
            "--rate-state",
            rate.to_str().unwrap(),
            "--public-requests-per-minute",
            "10000",
            "--mutation-requests-per-minute",
            "10000",
            "--ingest-requests-per-minute",
            "10000",
        ])
        .spawn()
        .unwrap();
    let service = ChildGuard(child);
    wait_http_ok(&format!("{base_url}/healthz"));

    let client = reqwest::blocking::Client::new();
    let mut event_body = json!({ "name": name });
    if let Some(gateway_base_url) = gateway_base_url {
        event_body["gateway_base_url"] = json!(gateway_base_url);
    }
    let event: Value = client
        .post(format!("{base_url}/events"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&event_body).unwrap())
        .send()
        .unwrap()
        .text()
        .map(|body| serde_json::from_str(&body).unwrap())
        .unwrap();
    let event_id = event["event_id"].as_str().unwrap().to_string();
    let event_sink_url = event["event_sink_url"].as_str().unwrap().to_string();
    let team: Value = client
        .post(format!("{base_url}/e/{event_id}/teams"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&json!({ "team_name": "Solo Team" })).unwrap())
        .send()
        .unwrap()
        .text()
        .map(|body| serde_json::from_str(&body).unwrap())
        .unwrap();

    EventFixture {
        _service: service,
        base_url,
        event_id,
        event_sink_url,
        team_id: team["team_id"].as_str().unwrap().to_string(),
        team_key: team["team_api_key"].as_str().unwrap().to_string(),
        team_payload: team,
    }
}

fn create_team(fixture: &EventFixture, team_name: &str) -> Value {
    reqwest::blocking::Client::new()
        .post(format!("{}/e/{}/teams", fixture.base_url, fixture.event_id))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&json!({ "team_name": team_name })).unwrap())
        .send()
        .unwrap()
        .text()
        .map(|body| serde_json::from_str(&body).unwrap())
        .unwrap()
}

fn wait_http_ok(url: &str) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if reqwest::blocking::get(url)
            .map(|resp| resp.status().is_success())
            .unwrap_or(false)
        {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {url}");
}

fn wait_cvm_count(fixture: &EventFixture, expected: u64) -> Value {
    wait_team_cvm_count(fixture, &fixture.team_id, expected)
}

fn wait_team_cvm_count(fixture: &EventFixture, team_id: &str, expected: u64) -> Value {
    let url = format!(
        "{}/e/{}/teams/{}/cvm.json",
        fixture.base_url, fixture.event_id, team_id
    );
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        let body = reqwest::blocking::get(&url).unwrap().text().unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        if value["stats"]["captured_events"].as_u64().unwrap_or(0) >= expected {
            return value;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for cvm count {expected}");
}

fn get_json(client: &reqwest::blocking::Client, url: &str) -> Value {
    let body = client.get(url).send().unwrap().text().unwrap();
    serde_json::from_str(&body).unwrap()
}

fn with_receipts(mut card: Value) -> Value {
    let receipts_url = card["share"]["receipts_url"]
        .as_str()
        .expect("cvm should expose receipts_url");
    let receipts = get_json(&reqwest::blocking::Client::new(), receipts_url);
    card.as_object_mut()
        .unwrap()
        .insert("_test_receipts".to_string(), receipts);
    card
}

fn contest_events_from_card(card: &Value) -> Vec<ContestEvent> {
    card["_test_receipts"]["receipts"]
        .as_array()
        .expect("receipts should be attached to test card")
        .iter()
        .map(|receipt| {
            let event_cbor = base64::engine::general_purpose::STANDARD
                .decode(receipt["receipt_cbor_b64"].as_str().unwrap().as_bytes())
                .unwrap();
            ContestEvent::from_cbor(&event_cbor).unwrap()
        })
        .collect()
}

fn single_event_from_card(card: &Value) -> ContestEvent {
    let events = contest_events_from_card(card);
    assert_eq!(events.len(), 1);
    events.into_iter().next().unwrap()
}

fn event_by_capture_method(card: &Value, capture_method: &str) -> ContestEvent {
    contest_events_from_card(card)
        .into_iter()
        .find(|event| event.capture_method == capture_method)
        .unwrap_or_else(|| panic!("missing event with capture_method={capture_method}"))
}

fn assert_hash(value: &str) {
    assert!(
        value.starts_with("sha256:"),
        "expected sha256-prefixed hash, got {value}"
    );
}

fn assert_no_remote_attestation(event: &ContestEvent) {
    assert!(
        !event.evidence.contains_key("ra_profile"),
        "non-attested capture path unexpectedly carried RA evidence: {:?}",
        event.evidence
    );
}

fn assert_gateway_call_event(
    event: &ContestEvent,
    capture_method: &str,
    assurance: &str,
    enforcement: &str,
    provider: &str,
) {
    assert_eq!(event.kind, "llm.call");
    assert_eq!(event.capture_method, capture_method);
    assert_eq!(event.assurance, assurance);
    assert_eq!(event.actor.kind, "agent");
    assert_eq!(
        event.actor.agent_session_id,
        format!("agent_{capture_method}")
    );
    assert_eq!(event.subject["provider"], provider);
    assert_eq!(event.subject["provider_request_id"], "req_test");
    assert_eq!(event.subject["api_family"], "openai-chat-completions");
    assert_eq!(event.subject["model"], "test-model");
    assert_eq!(event.metrics["input_tokens"], 3);
    assert_eq!(event.metrics["output_tokens"], 4);
    assert_eq!(event.metrics["total_tokens"], 7);
    assert_eq!(event.metrics["tool_call_count"], 0);
    assert!(*event.metrics.get("latency_ms").unwrap() >= 0);
    assert_hash(&event.hashes["request"]);
    assert_hash(&event.hashes["response"]);
    assert_eq!(event.evidence["gateway_usage"], "provider-reported");
    assert_eq!(event.evidence["enforcement_mode"], enforcement);
    assert!(event.evidence["provider_usage"].contains("\"total_tokens\":7"));
    assert!(!event.signature.is_empty());
}

fn assert_wrapper_event(
    event: &ContestEvent,
    subcommand: &str,
    capture_method: &str,
    assurance: &str,
    enforcement: &str,
) {
    assert_eq!(event.kind, "agent.session");
    assert_eq!(event.capture_method, capture_method);
    assert_eq!(event.assurance, assurance);
    assert_eq!(event.actor.kind, "agent");
    assert_eq!(event.actor.agent_session_id, format!("agent_{subcommand}"));
    assert_eq!(event.subject["command"], "/usr/bin/true");
    assert_hash(&event.subject["command_line_hash"]);
    assert_eq!(event.metrics["exit_code"], 0);
    assert!(*event.metrics.get("duration_ms").unwrap() >= 0);
    assert_eq!(event.evidence["wrapper"], "llmattest");
    assert_eq!(event.evidence["enforcement_mode"], enforcement);
    assert!(!event.signature.is_empty());
}

fn start_openai_like_upstream() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let mut stream = stream.unwrap();
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let body = json!({
                "id": "cmpl_test",
                "choices": [{ "message": { "role": "assistant", "content": "hello" } }],
                "usage": { "prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7 }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nx-request-id: req_test\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (port, handle)
}

fn start_anthropic_like_upstream() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let mut stream = stream.unwrap();
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let body = json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "model": "claude-3-5-sonnet-latest",
                "content": [{ "type": "text", "text": "implemented" }],
                "usage": { "input_tokens": 42, "output_tokens": 18 }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nx-request-id: req_claude_cli\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (port, handle)
}

fn start_model_list_server() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let mut stream = stream.unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body = json!({
                "object": "list",
                "data": [{ "id": "mlx-community/test", "object": "model" }]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (port, handle)
}

fn tdx_stage1_receipt_fixture() -> (PathBuf, String) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/chain/tdx_stage1.cbor");
    let bytes = std::fs::read(&path).unwrap();
    let eat = EatToken::from_cbor(&bytes).unwrap();
    (path, format!("sha384:{}", hex::encode(eat.value_x)))
}

fn start_gateway_process(
    fixture: &EventFixture,
    gateway_port: u16,
    upstream_port: u16,
    capture_method: &str,
    assurance: &str,
    enforcement: &str,
    provider: &str,
    teams: &[(String, String)],
) -> ChildGuard {
    let temp = tempfile::tempdir().unwrap().keep();
    let events = temp.join("gateway.cborl");
    let rate = temp.join("rate.json");
    let gateway_base_url = format!("http://127.0.0.1:{gateway_port}");
    let mut args: Vec<String> = vec![
        "--listen".to_string(),
        format!("127.0.0.1:{gateway_port}"),
        "--upstream".to_string(),
        format!("http://127.0.0.1:{upstream_port}"),
        "--upstream-no-auth".to_string(),
        "--provider".to_string(),
        provider.to_string(),
        "--hackathon-id".to_string(),
        fixture.event_id.clone(),
        "--event-sink-url".to_string(),
        fixture.event_sink_url.clone(),
        "--capture-method".to_string(),
        capture_method.to_string(),
        "--assurance".to_string(),
        assurance.to_string(),
        "--enforcement-mode".to_string(),
        enforcement.to_string(),
        "--gateway-manifest-url".to_string(),
        format!("{gateway_base_url}/.well-known/llm-attested/manifest.cbor"),
        "--events".to_string(),
        events.to_str().unwrap().to_string(),
        "--rate-state".to_string(),
        rate.to_str().unwrap().to_string(),
        "--ip-requests-per-minute".to_string(),
        "10000".to_string(),
        "--team-requests-per-minute".to_string(),
        "10000".to_string(),
    ];
    for (team_id, team_key) in teams {
        args.extend(["--team".to_string(), format!("{team_id}:{team_key}")]);
    }
    let receipt;
    let value_x;
    if assurance == "attested" {
        let receipt_fixture = tdx_stage1_receipt_fixture();
        receipt = receipt_fixture.0;
        value_x = receipt_fixture.1;
        args.extend([
            "--cvm-receipt".to_string(),
            receipt.to_str().unwrap().to_string(),
            "--value-x".to_string(),
            value_x,
            "--accepted-tee-platforms".to_string(),
            "tdx".to_string(),
        ]);
    }
    let gateway = Command::new(env!("CARGO_BIN_EXE_llm-gateway"))
        .args(args)
        .spawn()
        .unwrap();
    ChildGuard(gateway)
}

fn gateway_path(capture_method: &str, assurance: &str, enforcement: &str, provider: &str) -> Value {
    let (upstream_port, _upstream) = start_openai_like_upstream();
    let gateway_port = free_port();
    let gateway_base_url = format!("http://127.0.0.1:{gateway_port}");
    let fixture = start_event_fixture_with_gateway(capture_method, Some(gateway_base_url.clone()));
    let _gateway = start_gateway_process(
        &fixture,
        gateway_port,
        upstream_port,
        capture_method,
        assurance,
        enforcement,
        provider,
        &[(fixture.team_id.clone(), fixture.team_key.clone())],
    );
    wait_http_ok(&format!("http://127.0.0.1:{gateway_port}/healthz"));

    let resp = reqwest::blocking::Client::new()
        .post(format!(
            "http://127.0.0.1:{gateway_port}/v1/chat/completions"
        ))
        .bearer_auth(&fixture.team_key)
        .header("x-llm-attested-agent", format!("agent_{capture_method}"))
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(
                &json!({ "model": "test-model", "messages": [{ "role": "user", "content": "hi" }] }),
            )
            .unwrap(),
        )
        .send()
        .unwrap();
    assert!(resp.status().is_success(), "{:?}", resp.text());
    with_receipts(wait_cvm_count(&fixture, 1))
}

#[test]
fn attested_gateway_capture_path_e2e() {
    let card = gateway_path("gateway_proxy", "attested", "routed", "openai-test");
    assert_eq!(card["labels"]["capture_methods"]["gateway_proxy"], 1);
    assert_eq!(card["labels"]["assurance"]["attested"], 1);
    assert_eq!(card["labels"]["enforcement"]["routed"], 1);
    assert_eq!(card["labels"]["remote_attestation"][CAPTURE_RA_PROFILE], 1);
    assert_eq!(card["labels"]["ra_platforms"]["tdx"], 1);
    assert_eq!(card["stats"]["total_tokens"], 7);
    assert_eq!(card["stats"]["models"]["test-model"], 1);

    let event = single_event_from_card(&card);
    assert_gateway_call_event(&event, "gateway_proxy", "attested", "routed", "openai-test");
    assert_eq!(event.evidence["ra_profile"], CAPTURE_RA_PROFILE);
    assert_eq!(event.evidence["ra_platform"], "tdx");
    assert!(event.evidence["ra_cvm_receipt_hash"].starts_with("sha256:"));
}

#[test]
fn event_service_rejects_unverified_attested_capture_e2e() {
    let fixture = start_event_fixture("fake-attested");
    let temp = tempfile::tempdir().unwrap().keep();
    let report_path = temp.join("fake-attested.json");
    std::fs::write(
        &report_path,
        serde_json::to_vec(&base_report(&fixture, "gateway_proxy", "attested")).unwrap(),
    )
    .unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "emit",
            "--event-sink-url",
            &fixture.event_sink_url,
            "--report",
            report_path.to_str().unwrap(),
            "--signing-seed",
            "fake-attested-test",
        ])
        .status()
        .unwrap();
    assert!(!status.success());
    let card = wait_cvm_count(&fixture, 0);
    assert_eq!(card["stats"]["captured_events"], 0);
}

#[test]
fn local_proxy_capture_path_e2e() {
    let card = gateway_path(
        "local_proxy",
        "participant_controlled",
        "routed",
        "local-ollama",
    );
    assert_eq!(card["labels"]["capture_methods"]["local_proxy"], 1);
    assert_eq!(card["labels"]["assurance"]["participant_controlled"], 1);
    assert_eq!(card["stats"]["providers"]["local-ollama"], 1);
    let event = single_event_from_card(&card);
    assert_gateway_call_event(
        &event,
        "local_proxy",
        "participant_controlled",
        "routed",
        "local-ollama",
    );
    assert_no_remote_attestation(&event);
}

#[test]
fn claude_cli_anthropic_messages_gateway_usage_e2e() {
    let (upstream_port, _upstream) = start_anthropic_like_upstream();
    let gateway_port = free_port();
    let gateway_base_url = format!("http://127.0.0.1:{gateway_port}");
    let fixture = start_event_fixture_with_gateway("claude-cli", Some(gateway_base_url));
    let _gateway = start_gateway_process(
        &fixture,
        gateway_port,
        upstream_port,
        "gateway_proxy",
        "participant_controlled",
        "routed",
        "anthropic",
        &[(fixture.team_id.clone(), fixture.team_key.clone())],
    );
    wait_http_ok(&format!("http://127.0.0.1:{gateway_port}/healthz"));

    let resp = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{gateway_port}/v1/messages"))
        .bearer_auth(&fixture.team_key)
        .header("x-llm-attested-agent", "claude-cli")
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(&json!({
                "model": "claude-3-5-sonnet-latest",
                "max_tokens": 2000,
                "messages": [{
                    "role": "user",
                    "content": "edit the project and run tests"
                }]
            }))
            .unwrap(),
        )
        .send()
        .unwrap();
    let status = resp.status();
    let body = resp.text().unwrap();
    assert!(status.is_success(), "{body}");
    let upstream_body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(upstream_body["id"], "msg_test");

    let card = with_receipts(wait_cvm_count(&fixture, 1));
    assert_eq!(card["labels"]["capture_methods"]["gateway_proxy"], 1);
    assert_eq!(card["labels"]["assurance"]["participant_controlled"], 1);
    assert_eq!(card["labels"]["enforcement"]["routed"], 1);
    assert_eq!(card["stats"]["providers"]["anthropic"], 1);
    assert_eq!(card["stats"]["models"]["claude-3-5-sonnet-latest"], 1);
    assert_eq!(card["stats"]["total_tokens"], 60);

    let event = single_event_from_card(&card);
    assert_eq!(event.kind, "llm.call");
    assert_eq!(event.capture_method, "gateway_proxy");
    assert_eq!(event.assurance, "participant_controlled");
    assert_eq!(event.actor.kind, "agent");
    assert_eq!(event.actor.agent_session_id, "claude-cli");
    assert_eq!(event.subject["provider"], "anthropic");
    assert_eq!(event.subject["provider_request_id"], "req_claude_cli");
    assert_eq!(event.subject["api_family"], "openai-compatible");
    assert_eq!(event.subject["model"], "claude-3-5-sonnet-latest");
    assert_eq!(event.metrics["input_tokens"], 42);
    assert_eq!(event.metrics["output_tokens"], 18);
    assert_eq!(event.metrics["total_tokens"], 60);
    assert_eq!(event.metrics["tool_call_count"], 0);
    assert!(*event.metrics.get("latency_ms").unwrap() >= 0);
    assert_hash(&event.hashes["request"]);
    assert_hash(&event.hashes["response"]);
    assert_eq!(event.evidence["gateway_usage"], "provider-reported");
    assert_eq!(event.evidence["enforcement_mode"], "routed");
    assert!(event.evidence["provider_usage"].contains("\"input_tokens\":42"));
    assert!(event.evidence["provider_usage"].contains("\"output_tokens\":18"));
    assert_no_remote_attestation(&event);
}

#[test]
fn gateway_cross_team_credentials_cannot_credit_another_team_e2e() {
    let (upstream_port, _upstream) = start_openai_like_upstream();
    let gateway_port = free_port();
    let gateway_base_url = format!("http://127.0.0.1:{gateway_port}");
    let fixture = start_event_fixture_with_gateway("cross-team", Some(gateway_base_url));
    let other_team = create_team(&fixture, "Other Team");
    let other_team_id = other_team["team_id"].as_str().unwrap().to_string();
    let other_team_key = other_team["team_api_key"].as_str().unwrap().to_string();
    let _gateway = start_gateway_process(
        &fixture,
        gateway_port,
        upstream_port,
        "gateway_proxy",
        "participant_controlled",
        "routed",
        "openai-test",
        &[
            (fixture.team_id.clone(), fixture.team_key.clone()),
            (other_team_id.clone(), other_team_key.clone()),
        ],
    );
    wait_http_ok(&format!("http://127.0.0.1:{gateway_port}/healthz"));

    let resp = reqwest::blocking::Client::new()
        .post(format!(
            "http://127.0.0.1:{gateway_port}/v1/chat/completions"
        ))
        .bearer_auth(&other_team_key)
        .header("x-llm-attested-agent", "claimed-first-team-agent")
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(
                &json!({ "model": "test-model", "messages": [{ "role": "user", "content": "credit team one" }] }),
            )
            .unwrap(),
        )
        .send()
        .unwrap();
    let status = resp.status();
    let body = resp.text().unwrap();
    assert!(status.is_success(), "{body}");

    let other_card = with_receipts(wait_team_cvm_count(&fixture, &other_team_id, 1));
    let first_card = wait_cvm_count(&fixture, 0);
    assert_eq!(first_card["stats"]["captured_events"], 0);
    assert_eq!(other_card["stats"]["captured_events"], 1);
    let event = single_event_from_card(&other_card);
    assert_eq!(event.team_id, other_team_id);
    assert_ne!(event.team_id, fixture.team_id);
    assert_eq!(event.actor.agent_session_id, "claimed-first-team-agent");
    assert_eq!(event.subject["provider"], "openai-test");
    assert_eq!(event.metrics["total_tokens"], 7);
}

#[test]
fn organizer_hosted_model_gateway_capture_path_e2e() {
    let card = gateway_path(
        "gateway_proxy",
        "managed",
        "managed_keys",
        "organizer-local",
    );
    assert_eq!(card["labels"]["assurance"]["managed"], 1);
    assert_eq!(card["labels"]["enforcement"]["managed_keys"], 1);
    assert_eq!(card["stats"]["providers"]["organizer-local"], 1);
    let event = single_event_from_card(&card);
    assert_gateway_call_event(
        &event,
        "gateway_proxy",
        "managed",
        "managed_keys",
        "organizer-local",
    );
    assert_no_remote_attestation(&event);
}

#[test]
fn organizer_join_page_and_team_payload_are_one_command_e2e() {
    let fixture = start_event_fixture("one-command-join");
    let join = reqwest::blocking::get(format!("{}/e/{}/join", fixture.base_url, fixture.event_id))
        .unwrap()
        .text()
        .unwrap();
    assert!(join.contains("llmattest start --team-payload team.json -- your-agent-command"));
    assert!(join.contains("llmattest status --watch"));
    assert!(join.contains("install.sh"));
    assert!(join.contains("install.ps1"));

    assert_eq!(
        fixture.team_payload["llmattest"]["start"],
        "llmattest start --team-payload team.json -- your-agent-command"
    );
    assert_eq!(
        fixture.team_payload["llmattest"]["status_watch"],
        "llmattest status --watch"
    );
    assert!(fixture.team_payload["install"]["mac_linux"]
        .as_str()
        .unwrap()
        .contains("install.sh"));
    assert!(fixture.team_payload["participant_next_step"]
        .as_str()
        .unwrap()
        .contains("llmattest start"));
}

#[test]
fn gateway_rejects_bad_team_credentials_e2e() {
    let fixture = start_event_fixture("bad-credentials");
    let (upstream_port, _upstream) = start_openai_like_upstream();
    let gateway_port = free_port();
    let temp = tempfile::tempdir().unwrap().keep();
    let gateway = Command::new(env!("CARGO_BIN_EXE_llm-gateway"))
        .args([
            "--listen",
            &format!("127.0.0.1:{gateway_port}"),
            "--upstream",
            &format!("http://127.0.0.1:{upstream_port}"),
            "--upstream-no-auth",
            "--provider",
            "openai-test",
            "--hackathon-id",
            &fixture.event_id,
            "--event-sink-url",
            &fixture.event_sink_url,
            "--events",
            temp.join("bad-creds.cborl").to_str().unwrap(),
            "--rate-state",
            temp.join("rate.json").to_str().unwrap(),
            "--team",
            &format!("{}:{}", fixture.team_id, fixture.team_key),
            "--ip-requests-per-minute",
            "10000",
            "--team-requests-per-minute",
            "10000",
        ])
        .spawn()
        .unwrap();
    let _gateway = ChildGuard(gateway);
    wait_http_ok(&format!("http://127.0.0.1:{gateway_port}/healthz"));

    let resp = reqwest::blocking::Client::new()
        .post(format!(
            "http://127.0.0.1:{gateway_port}/v1/chat/completions"
        ))
        .bearer_auth("wrong-team-key")
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(
                &json!({ "model": "test-model", "messages": [{ "role": "user", "content": "hi" }] }),
            )
            .unwrap(),
        )
        .send()
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let card = wait_cvm_count(&fixture, 0);
    assert_eq!(card["stats"]["captured_events"], 0);
}

#[test]
fn participant_config_status_smoke_and_run_e2e() {
    let fixture = start_event_fixture("participant-config");
    let temp = tempfile::tempdir().unwrap().keep();
    let team_payload_path = temp.join("team.json");
    let config_path = temp.join("llmattest.json");
    std::fs::write(
        &team_payload_path,
        serde_json::to_vec(&fixture.team_payload).unwrap(),
    )
    .unwrap();

    let init = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "init",
            "--team-payload",
            team_payload_path.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let init_json: Value = serde_json::from_slice(&init.stdout).unwrap();
    assert_eq!(init_json["team_id"], fixture.team_id);
    assert!(config_path.exists());

    let status = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args(["status", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["ready"], true);
    assert_eq!(status_json["checks"]["bootstrap"], true);
    assert_eq!(status_json["checks"]["cvm"], true);

    let smoke = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "smoke",
            "--config",
            config_path.to_str().unwrap(),
            "--signing-seed",
            "participant-config-smoke",
        ])
        .output()
        .unwrap();
    assert!(
        smoke.status.success(),
        "{}",
        String::from_utf8_lossy(&smoke.stderr)
    );
    let card = wait_cvm_count(&fixture, 1);
    assert_eq!(card["labels"]["capture_methods"]["setup_smoke"], 1);
    assert_eq!(card["labels"]["enforcement"]["setup_smoke"], 1);

    let run = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "run",
            "--config",
            config_path.to_str().unwrap(),
            "--signing-seed",
            "participant-config-run",
            "--",
            "/bin/sh",
            "-c",
            "test -n \"$OPENAI_BASE_URL\" && test -n \"$OPENAI_API_KEY\" && test -n \"$LLM_ATTESTED_CVM_URL\"",
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let card = wait_cvm_count(&fixture, 2);
    assert_eq!(card["labels"]["capture_methods"]["sdk_cli_wrapper"], 1);

    let watched = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "status",
            "--config",
            config_path.to_str().unwrap(),
            "--watch",
            "--iterations",
            "1",
            "--interval-ms",
            "100",
        ])
        .output()
        .unwrap();
    assert!(
        watched.status.success(),
        "{}",
        String::from_utf8_lossy(&watched.stderr)
    );
    let watched_line = String::from_utf8(watched.stdout).unwrap();
    let watched_json: Value = serde_json::from_str(watched_line.trim()).unwrap();
    assert_eq!(watched_json["ready"], true);
    assert_eq!(watched_json["live"]["event_count"], 2);
    assert_eq!(watched_json["latest_event"]["available"], true);
    assert_eq!(
        watched_json["active_capture_path"]["user_configurable"],
        false
    );
    assert!(watched_json["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| { action["code"] == "attested_gateway_unavailable" }));
}

#[test]
fn participant_status_reports_browser_companion_disconnect_e2e() {
    let fixture = start_event_fixture("browser-disconnect");
    let temp = tempfile::tempdir().unwrap().keep();
    let team_payload_path = temp.join("team.json");
    let config_path = temp.join("llmattest.json");
    std::fs::write(
        &team_payload_path,
        serde_json::to_vec(&fixture.team_payload).unwrap(),
    )
    .unwrap();
    let init = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "init",
            "--team-payload",
            team_payload_path.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(init.success());

    let status = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .env(
            "LLMATTEST_DAEMON_LISTEN",
            format!("127.0.0.1:{}", free_port()),
        )
        .args(["status", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["checks"]["local_companion"], false);
    assert!(status_json["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| { action["code"] == "local_companion_not_running" }));
}

#[test]
fn participant_status_detects_local_model_runtime_e2e() {
    let fixture = start_event_fixture("local-runtime-detect");
    let (model_port, _server) = start_model_list_server();
    let temp = tempfile::tempdir().unwrap().keep();
    let team_payload_path = temp.join("team.json");
    let config_path = temp.join("llmattest.json");
    std::fs::write(
        &team_payload_path,
        serde_json::to_vec(&fixture.team_payload).unwrap(),
    )
    .unwrap();
    let init = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "init",
            "--team-payload",
            team_payload_path.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(init.success());

    let status = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .env(
            "LLMATTEST_PROBE_OMLX_URL",
            format!("http://127.0.0.1:{model_port}/v1/models"),
        )
        .args(["status", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(status_json["capability_probes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|probe| probe["id"] == "omlx" && probe["available"] == true));
}

#[test]
fn participant_start_single_entrypoint_e2e() {
    let fixture = start_event_fixture("participant-start");
    let daemon_port = free_port();
    let temp = tempfile::tempdir().unwrap().keep();
    let team_payload_path = temp.join("team.json");
    let config_path = temp.join("llmattest.json");
    std::fs::write(
        &team_payload_path,
        serde_json::to_vec(&fixture.team_payload).unwrap(),
    )
    .unwrap();

    let start = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .env(
            "LLMATTEST_DAEMON_LISTEN",
            format!("127.0.0.1:{daemon_port}"),
        )
        .args([
            "start",
            "--team-payload",
            team_payload_path.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
            "--signing-seed",
            "participant-start",
            "--",
            "/bin/sh",
            "-c",
            "test -n \"$OPENAI_BASE_URL\" && test -n \"$OPENAI_API_KEY\" && test -n \"$LLM_ATTESTED_CVM_URL\" && test -n \"$LLM_ATTESTED_LOCAL_CAPTURE_URL\"",
        ])
        .output()
        .unwrap();
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    let start_json: Value = serde_json::from_slice(&start.stdout).unwrap();
    assert_eq!(start_json["started"], true);
    assert_eq!(start_json["capture_runtime"]["mode"], "llmattest_start");
    assert_eq!(start_json["capture_runtime"]["supervisor"], "child_process");
    assert_eq!(
        start_json["capture_runtime"]["local_daemon"]["base_url"],
        format!("http://127.0.0.1:{daemon_port}")
    );
    assert!(start_json["capability_probes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|probe| probe["id"] == "local_capture_companion" && probe["available"] == true));
    assert_eq!(start_json["selected_capture_path"]["id"], "sdk_cli_wrapper");
    assert_eq!(
        start_json["selected_capture_path"]["user_configurable"],
        false
    );
    assert!(config_path.exists());

    let card = wait_cvm_count(&fixture, 2);
    assert_eq!(card["labels"]["capture_methods"]["setup_smoke"], 1);
    assert_eq!(card["labels"]["capture_methods"]["sdk_cli_wrapper"], 1);
}

#[test]
fn participant_start_supervises_local_daemon_e2e() {
    let fixture = start_event_fixture("participant-start-daemon");
    let daemon_port = free_port();
    let temp = tempfile::tempdir().unwrap().keep();
    let team_payload_path = temp.join("team.json");
    let config_path = temp.join("llmattest.json");
    std::fs::write(
        &team_payload_path,
        serde_json::to_vec(&fixture.team_payload).unwrap(),
    )
    .unwrap();

    let start = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .env(
            "LLMATTEST_DAEMON_LISTEN",
            format!("127.0.0.1:{daemon_port}"),
        )
        .args([
            "start",
            "--team-payload",
            team_payload_path.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
            "--signing-seed",
            "participant-start-daemon",
            "--no-smoke",
            "--",
            "/bin/sh",
            "-c",
            "sleep 2",
        ])
        .spawn()
        .unwrap();
    let mut start = ChildGuard(start);
    wait_http_ok(&format!("http://127.0.0.1:{daemon_port}/healthz"));

    let resp = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{daemon_port}/capture/manual"))
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(&json!({
                "subject": {
                    "provider": "manual-while-started",
                    "model": "unknown"
                },
                "metrics": {
                    "total_tokens": 11
                },
                "evidence": {
                    "enforcement_mode": "none"
                }
            }))
            .unwrap(),
        )
        .send()
        .unwrap();
    let body = resp.text().unwrap();
    let accepted: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(accepted["queued"], false);
    assert_eq!(accepted["capture_method"], "manual_import");
    assert_eq!(accepted["assurance"], "self_reported");
    assert_hash(accepted["event_hash"].as_str().unwrap());

    let status = start.0.wait().unwrap();
    assert!(status.success());
    let card = with_receipts(wait_cvm_count(&fixture, 1));
    assert_eq!(card["labels"]["capture_methods"]["manual_import"], 1);
    assert_eq!(card["stats"]["total_tokens"], 11);
    let event = event_by_capture_method(&card, "manual_import");
    assert_eq!(event.capture_method, "manual_import");
    assert_eq!(event.assurance, "self_reported");
    assert_eq!(event.subject["provider"], "manual-while-started");
    assert_eq!(event.subject["model"], "unknown");
    assert_eq!(event.metrics["total_tokens"], 11);
    assert_eq!(event.evidence["capture_daemon"], "llm-capture-daemon");
    assert_eq!(event.evidence["enforcement_mode"], "none");
}

#[test]
fn cvm_share_surfaces_e2e() {
    let fixture = start_event_fixture("share-surfaces");
    let temp = tempfile::tempdir().unwrap().keep();
    let report_path = temp.join("share.json");
    std::fs::write(
        &report_path,
        serde_json::to_vec(&base_report(&fixture, "manual_import", "self_reported")).unwrap(),
    )
    .unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "emit",
            "--event-sink-url",
            &fixture.event_sink_url,
            "--report",
            report_path.to_str().unwrap(),
            "--signing-seed",
            "share-surfaces-test",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let card = wait_cvm_count(&fixture, 1);
    assert!(card["share"]["embed_url"]
        .as_str()
        .unwrap()
        .ends_with("/cvm.embed"));
    assert!(card["share"]["image_url"]
        .as_str()
        .unwrap()
        .ends_with("/cvm.svg"));
    assert!(card["share"]["individual_signal_image_url"]
        .as_str()
        .unwrap()
        .ends_with("/cvm.signal.svg"));
    assert!(card["integrity"]["event_log_root"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    let client = reqwest::blocking::Client::new();
    let embed = client
        .get(card["share"]["embed_url"].as_str().unwrap())
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(embed.contains("Live cvm"));

    let svg = client
        .get(card["share"]["image_url"].as_str().unwrap())
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("scan to verify"));
    assert!(svg.contains("cvm-card-json"));
    assert!(svg.contains("data-sha256=\"sha256:"));

    let signal_json = get_json(
        &client,
        card["share"]["individual_signal_json_url"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(
        signal_json["profile"],
        "https://cvm.dev/llm-individual-signal/v1"
    );
    assert_eq!(
        signal_json["participant"]["agent_session_id"],
        "agent_capture_test"
    );
    assert_eq!(signal_json["stats"]["today_llm_calls"], 1);
    assert_eq!(signal_json["stats"]["today_total_tokens"], 7);

    let signal_svg = client
        .get(
            card["share"]["individual_signal_image_url"]
                .as_str()
                .unwrap(),
        )
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(signal_svg.starts_with("<svg"));
    assert!(signal_svg.contains("agent_capture_test"));
    assert!(signal_svg.contains("LIVE CVM"));
    assert!(signal_svg.contains("cvm-card-json"));
    assert!(signal_svg.contains("llm-individual-signal/v1"));

    let qr = client
        .get(card["share"]["qr_svg_url"].as_str().unwrap())
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(qr.starts_with("<svg"));

    let proof = get_json(&client, card["share"]["proof_url"].as_str().unwrap());
    assert_eq!(proof["event_log_root"], card["integrity"]["event_log_root"]);
    assert_eq!(proof["event_hashes"].as_array().unwrap().len(), 1);

    let receipts = get_json(&client, card["share"]["receipts_url"].as_str().unwrap());
    assert_eq!(receipts["receipts"].as_array().unwrap().len(), 1);

    let credential = get_json(&client, card["share"]["credential_url"].as_str().unwrap());
    assert!(credential["type"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "CvmCredential"));

    let oembed = get_json(&client, card["share"]["oembed_url"].as_str().unwrap());
    assert_eq!(oembed["type"], "rich");
    assert!(oembed["html"].as_str().unwrap().contains("<iframe"));
}

#[test]
fn browser_extension_capture_path_e2e() {
    let fixture = start_event_fixture("browser");
    let daemon_port = free_port();
    let temp = tempfile::tempdir().unwrap().keep();
    let daemon = Command::new(env!("CARGO_BIN_EXE_llm-capture-daemon"))
        .args([
            "--listen",
            &format!("127.0.0.1:{daemon_port}"),
            "--event-sink-url",
            &fixture.event_sink_url,
            "--hackathon-id",
            &fixture.event_id,
            "--team-id",
            &fixture.team_id,
            "--signing-seed",
            "browser-test",
            "--events",
            temp.join("browser.cborl").to_str().unwrap(),
            "--rate-state",
            temp.join("rate.json").to_str().unwrap(),
            "--requests-per-minute",
            "10000",
        ])
        .spawn()
        .unwrap();
    let _daemon = ChildGuard(daemon);
    wait_http_ok(&format!("http://127.0.0.1:{daemon_port}/healthz"));

    let report = base_report(&fixture, "browser_extension", "participant_controlled");
    let resp = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{daemon_port}/capture/browser"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&report).unwrap())
        .send()
        .unwrap();
    let body = resp.text().unwrap();
    let accepted: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(accepted["queued"], false);
    assert_eq!(accepted["capture_method"], "browser_extension");
    assert_eq!(accepted["assurance"], "participant_controlled");
    assert_hash(accepted["event_hash"].as_str().unwrap());

    let card = with_receipts(wait_cvm_count(&fixture, 1));
    assert_eq!(card["labels"]["capture_methods"]["browser_extension"], 1);
    assert_eq!(card["labels"]["assurance"]["participant_controlled"], 1);
    assert_eq!(card["stats"]["total_tokens"], 7);
    assert_eq!(card["stats"]["models"]["browser-model"], 1);
    let event = single_event_from_card(&card);
    assert_eq!(event.kind, "llm.call");
    assert_eq!(event.capture_method, "browser_extension");
    assert_eq!(event.assurance, "participant_controlled");
    assert_eq!(event.actor.agent_session_id, "agent_capture_test");
    assert_eq!(event.subject["provider"], "browser-test");
    assert_eq!(event.subject["model"], "browser-model");
    assert_eq!(event.subject["api_family"], "browser-ui");
    assert_eq!(event.metrics["input_tokens"], 2);
    assert_eq!(event.metrics["output_tokens"], 5);
    assert_eq!(event.metrics["total_tokens"], 7);
    assert_eq!(event.hashes["request"], "sha256:req");
    assert_eq!(event.hashes["response"], "sha256:resp");
    assert_eq!(event.evidence["capture_daemon"], "llm-capture-daemon");
    assert_eq!(event.evidence["enforcement_mode"], "routed");
    assert_no_remote_attestation(&event);
}

#[test]
fn codex_web_browser_capture_records_visible_session_without_attestation_e2e() {
    let fixture = start_event_fixture("codex-web-browser");
    let daemon_port = free_port();
    let temp = tempfile::tempdir().unwrap().keep();
    let daemon = Command::new(env!("CARGO_BIN_EXE_llm-capture-daemon"))
        .args([
            "--listen",
            &format!("127.0.0.1:{daemon_port}"),
            "--event-sink-url",
            &fixture.event_sink_url,
            "--hackathon-id",
            &fixture.event_id,
            "--team-id",
            &fixture.team_id,
            "--signing-seed",
            "codex-web-browser-test",
            "--events",
            temp.join("codex-web.cborl").to_str().unwrap(),
            "--rate-state",
            temp.join("rate.json").to_str().unwrap(),
            "--requests-per-minute",
            "10000",
        ])
        .spawn()
        .unwrap();
    let _daemon = ChildGuard(daemon);
    wait_http_ok(&format!("http://127.0.0.1:{daemon_port}/healthz"));

    let report = json!({
        "kind": "llm.call",
        "hackathon_id": fixture.event_id,
        "team_id": fixture.team_id,
        "gateway_id": "codex_web_extension",
        "manifest_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "capture_method": "browser_extension",
        "assurance": "participant_controlled",
        "actor": {
            "kind": "human",
            "agent_session_id": "codex-web-tab-1"
        },
        "subject": {
            "provider": "openai",
            "surface": "codex-web",
            "origin": "https://chatgpt.com",
            "conversation_id_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "model": "gpt-5-codex",
            "api_family": "browser-ui"
        },
        "metrics": {
            "input_tokens": 1200,
            "output_tokens": 340,
            "total_tokens": 1540,
            "llm_calls": 1
        },
        "hashes": {
            "dom_snapshot": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
            "visible_prompt": "sha256:3333333333333333333333333333333333333333333333333333333333333333",
            "visible_response": "sha256:4444444444444444444444444444444444444444444444444444444444444444"
        },
        "evidence": {
            "enforcement_mode": "browser_observed",
            "capture_surface": "codex_web",
            "visibility": "participant_browser_dom",
            "limitation": "browser_extension_cannot_prove_absence_of_other_tabs_or_devices"
        }
    });
    let resp = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{daemon_port}/capture/browser"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&report).unwrap())
        .send()
        .unwrap();
    let body = resp.text().unwrap();
    let accepted: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(accepted["queued"], false);
    assert_eq!(accepted["capture_method"], "browser_extension");
    assert_eq!(accepted["assurance"], "participant_controlled");

    let card = with_receipts(wait_cvm_count(&fixture, 1));
    assert_eq!(card["labels"]["capture_methods"]["browser_extension"], 1);
    assert_eq!(card["labels"]["assurance"]["participant_controlled"], 1);
    assert_eq!(card["labels"]["enforcement"]["browser_observed"], 1);
    assert_eq!(card["stats"]["providers"]["openai"], 1);
    assert_eq!(card["stats"]["models"]["gpt-5-codex"], 1);
    assert_eq!(card["stats"]["total_tokens"], 1540);
    assert_eq!(card["stats"]["llm_calls"], 1);
    let event = single_event_from_card(&card);
    assert_eq!(event.kind, "llm.call");
    assert_eq!(event.capture_method, "browser_extension");
    assert_eq!(event.assurance, "participant_controlled");
    assert_eq!(event.actor.kind, "human");
    assert_eq!(event.actor.agent_session_id, "codex-web-tab-1");
    assert_eq!(event.subject["provider"], "openai");
    assert_eq!(event.subject["surface"], "codex-web");
    assert_eq!(event.subject["origin"], "https://chatgpt.com");
    assert_eq!(event.subject["model"], "gpt-5-codex");
    assert_eq!(event.subject["api_family"], "browser-ui");
    assert_eq!(event.metrics["input_tokens"], 1200);
    assert_eq!(event.metrics["output_tokens"], 340);
    assert_eq!(event.metrics["total_tokens"], 1540);
    assert_eq!(event.metrics["llm_calls"], 1);
    assert_hash(&event.hashes["dom_snapshot"]);
    assert_hash(&event.hashes["visible_prompt"]);
    assert_hash(&event.hashes["visible_response"]);
    assert_eq!(event.evidence["capture_daemon"], "llm-capture-daemon");
    assert_eq!(event.evidence["enforcement_mode"], "browser_observed");
    assert_eq!(event.evidence["capture_surface"], "codex_web");
    assert_eq!(event.evidence["visibility"], "participant_browser_dom");
    assert_eq!(
        event.evidence["limitation"],
        "browser_extension_cannot_prove_absence_of_other_tabs_or_devices"
    );
    assert_no_remote_attestation(&event);
}

#[test]
fn browser_extension_cannot_self_upgrade_to_attested_e2e() {
    let fixture = start_event_fixture("browser-attested-cheat");
    let daemon_port = free_port();
    let temp = tempfile::tempdir().unwrap().keep();
    let pending_path = temp.join("pending.cborl");
    let daemon = Command::new(env!("CARGO_BIN_EXE_llm-capture-daemon"))
        .args([
            "--listen",
            &format!("127.0.0.1:{daemon_port}"),
            "--event-sink-url",
            &fixture.event_sink_url,
            "--hackathon-id",
            &fixture.event_id,
            "--team-id",
            &fixture.team_id,
            "--signing-seed",
            "browser-attested-cheat-test",
            "--events",
            temp.join("browser-attested-cheat.cborl").to_str().unwrap(),
            "--pending-events",
            pending_path.to_str().unwrap(),
            "--rate-state",
            temp.join("rate.json").to_str().unwrap(),
            "--requests-per-minute",
            "10000",
        ])
        .spawn()
        .unwrap();
    let _daemon = ChildGuard(daemon);
    wait_http_ok(&format!("http://127.0.0.1:{daemon_port}/healthz"));

    let mut report = base_report(&fixture, "browser_extension", "attested");
    report["evidence"]["enforcement_mode"] = json!("attested_runtime");
    report["evidence"]["claimed_runtime"] = json!("modified_browser_extension");
    let resp = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{daemon_port}/capture/browser"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&report).unwrap())
        .send()
        .unwrap();
    let body = resp.text().unwrap();
    let accepted: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(accepted["queued"], true);
    assert_eq!(accepted["capture_method"], "browser_extension");
    assert_eq!(accepted["assurance"], "attested");

    let card = wait_cvm_count(&fixture, 0);
    assert_eq!(card["stats"]["captured_events"], 0);
    let pending_body = std::fs::read_to_string(pending_path).unwrap();
    let pending_event = ContestEvent::from_cbor(
        &base64::engine::general_purpose::STANDARD
            .decode(pending_body.lines().next().unwrap().as_bytes())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(pending_event.capture_method, "browser_extension");
    assert_eq!(pending_event.assurance, "attested");
    assert_eq!(
        pending_event.evidence["capture_daemon"],
        "llm-capture-daemon"
    );
    assert_eq!(
        pending_event.evidence["enforcement_mode"],
        "attested_runtime"
    );
    assert_eq!(
        pending_event.evidence["claimed_runtime"],
        "modified_browser_extension"
    );
}

#[test]
fn capture_daemon_config_and_minimal_report_e2e() {
    let fixture = start_event_fixture("daemon-config");
    let daemon_port = free_port();
    let temp = tempfile::tempdir().unwrap().keep();
    let config_path = temp.join("llmattest.json");
    let daemon_events = temp.join("daemon.cborl");
    std::fs::write(
        &config_path,
        serde_json::to_vec_pretty(&fixture.team_payload).unwrap(),
    )
    .unwrap();
    let init = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "init",
            "--team-payload",
            config_path.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(init.success());

    let daemon = Command::new(env!("CARGO_BIN_EXE_llm-capture-daemon"))
        .args([
            "--listen",
            &format!("127.0.0.1:{daemon_port}"),
            "--config",
            config_path.to_str().unwrap(),
            "--signing-seed",
            "daemon-config-test",
            "--events",
            daemon_events.to_str().unwrap(),
            "--rate-state",
            temp.join("rate.json").to_str().unwrap(),
            "--requests-per-minute",
            "10000",
        ])
        .spawn()
        .unwrap();
    let _daemon = ChildGuard(daemon);
    wait_http_ok(&format!("http://127.0.0.1:{daemon_port}/healthz"));

    let resp = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{daemon_port}/capture/manual"))
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(&json!({
                "subject": {
                    "provider": "manual-minimal",
                    "model": "unknown"
                },
                "metrics": {
                    "total_tokens": 5
                },
                "evidence": {
                    "enforcement_mode": "none"
                }
            }))
            .unwrap(),
        )
        .send()
        .unwrap();
    let body = resp.text().unwrap();
    let accepted: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(accepted["accepted"], true);
    assert_eq!(accepted["queued"], false);
    assert_eq!(accepted["capture_method"], "manual_import");
    assert_eq!(accepted["assurance"], "self_reported");
    assert_hash(accepted["event_hash"].as_str().unwrap());
    let card = with_receipts(wait_cvm_count(&fixture, 1));
    assert_eq!(card["labels"]["capture_methods"]["manual_import"], 1);
    assert_eq!(card["labels"]["assurance"]["self_reported"], 1);
    assert_eq!(card["stats"]["total_tokens"], 5);
    let event = single_event_from_card(&card);
    assert_eq!(event.capture_method, "manual_import");
    assert_eq!(event.assurance, "self_reported");
    assert_eq!(event.subject["provider"], "manual-minimal");
    assert_eq!(event.metrics["total_tokens"], 5);
    assert_eq!(event.evidence["capture_daemon"], "llm-capture-daemon");
    assert_eq!(event.evidence["enforcement_mode"], "none");

    let event_b64 = std::fs::read_to_string(&daemon_events)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    let event_cbor = base64::engine::general_purpose::STANDARD
        .decode(event_b64.as_bytes())
        .unwrap();
    let logged_event = ContestEvent::from_cbor(&event_cbor).unwrap();
    assert_eq!(logged_event.event_id, event.event_id);
    let duplicate: Value = reqwest::blocking::Client::new()
        .post(&fixture.event_sink_url)
        .header("content-type", "application/cbor")
        .body(event_cbor)
        .send()
        .unwrap()
        .text()
        .map(|body| serde_json::from_str(&body).unwrap())
        .unwrap();
    assert_eq!(duplicate["accepted"], true);
    assert_eq!(duplicate["duplicate"], true);
    assert_eq!(duplicate["count"], 1);
    let card = wait_cvm_count(&fixture, 1);
    assert_eq!(card["stats"]["captured_events"], 1);
}

#[test]
fn capture_daemon_queues_when_sink_down_e2e() {
    let daemon_port = free_port();
    let sink_port = free_port();
    let temp = tempfile::tempdir().unwrap().keep();
    let events_path = temp.join("events.cborl");
    let pending_path = temp.join("pending.cborl");
    let daemon = Command::new(env!("CARGO_BIN_EXE_llm-capture-daemon"))
        .args([
            "--listen",
            &format!("127.0.0.1:{daemon_port}"),
            "--event-sink-url",
            &format!("http://127.0.0.1:{sink_port}/e/hk_queue/events"),
            "--hackathon-id",
            "hk_queue",
            "--team-id",
            "team_queue",
            "--signing-seed",
            "queue-test",
            "--events",
            events_path.to_str().unwrap(),
            "--pending-events",
            pending_path.to_str().unwrap(),
            "--rate-state",
            temp.join("rate.json").to_str().unwrap(),
            "--requests-per-minute",
            "10000",
        ])
        .spawn()
        .unwrap();
    let _daemon = ChildGuard(daemon);
    wait_http_ok(&format!("http://127.0.0.1:{daemon_port}/healthz"));

    let body = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{daemon_port}/capture/manual"))
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(&json!({
                "subject": {
                    "provider": "offline",
                    "model": "unknown"
                },
                "metrics": {
                    "total_tokens": 3
                },
                "evidence": {
                    "enforcement_mode": "none"
                }
            }))
            .unwrap(),
        )
        .send()
        .unwrap()
        .text()
        .unwrap();
    let resp: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(resp["accepted"], true);
    assert_eq!(resp["queued"], true);
    assert_eq!(resp["capture_method"], "manual_import");
    assert_eq!(resp["assurance"], "self_reported");
    assert_hash(resp["event_hash"].as_str().unwrap());
    assert!(events_path.exists());
    assert!(pending_path.exists());
    let pending_body = std::fs::read_to_string(pending_path).unwrap();
    assert!(!pending_body.is_empty());
    let pending_event = ContestEvent::from_cbor(
        &base64::engine::general_purpose::STANDARD
            .decode(pending_body.lines().next().unwrap().as_bytes())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(pending_event.hackathon_id, "hk_queue");
    assert_eq!(pending_event.team_id, "team_queue");
    assert_eq!(pending_event.capture_method, "manual_import");
    assert_eq!(pending_event.assurance, "self_reported");
    assert_eq!(pending_event.subject["provider"], "offline");
    assert_eq!(pending_event.metrics["total_tokens"], 3);
    assert_eq!(
        pending_event.evidence["capture_daemon"],
        "llm-capture-daemon"
    );
}

#[test]
fn manual_import_capture_path_e2e() {
    let fixture = start_event_fixture("manual");
    let temp = tempfile::tempdir().unwrap().keep();
    let report_path = temp.join("manual.json");
    std::fs::write(
        &report_path,
        serde_json::to_vec(&base_report(&fixture, "manual_import", "self_reported")).unwrap(),
    )
    .unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "emit",
            "--event-sink-url",
            &fixture.event_sink_url,
            "--report",
            report_path.to_str().unwrap(),
            "--signing-seed",
            "manual-test",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let card = with_receipts(wait_cvm_count(&fixture, 1));
    assert_eq!(card["labels"]["capture_methods"]["manual_import"], 1);
    assert_eq!(card["labels"]["assurance"]["self_reported"], 1);
    assert_eq!(card["stats"]["total_tokens"], 7);
    let event = single_event_from_card(&card);
    assert_eq!(event.capture_method, "manual_import");
    assert_eq!(event.assurance, "self_reported");
    assert_eq!(event.actor.agent_session_id, "agent_capture_test");
    assert_eq!(event.subject["provider"], "browser-test");
    assert_eq!(event.subject["model"], "browser-model");
    assert_eq!(event.metrics["total_tokens"], 7);
    assert_eq!(event.hashes["request"], "sha256:req");
    assert_eq!(event.hashes["response"], "sha256:resp");
    assert_eq!(event.evidence["enforcement_mode"], "routed");
    assert_no_remote_attestation(&event);
}

#[test]
fn provider_tls_notary_fallback_capture_path_e2e() {
    let fixture = start_event_fixture("tls-notary");
    let temp = tempfile::tempdir().unwrap().keep();
    let proof_path = temp.join("anthropic-usage.tlsn");
    let claim_path = temp.join("anthropic-usage.json");
    std::fs::write(&proof_path, b"opaque tlsnotary proof bytes").unwrap();
    std::fs::write(
        &claim_path,
        serde_json::to_vec(&json!({
            "subject": {
                "provider": "anthropic",
                "account_scope": "participant_selected_account"
            },
            "metrics": {
                "total_tokens": 1234,
                "llm_calls": 17
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            "notary-import",
            "--event-sink-url",
            &fixture.event_sink_url,
            "--hackathon-id",
            &fixture.event_id,
            "--team-id",
            &fixture.team_id,
            "--provider",
            "anthropic",
            "--target-origin",
            "https://console.anthropic.com",
            "--notary-url",
            "https://notary.cvm.example",
            "--proof",
            proof_path.to_str().unwrap(),
            "--claim",
            claim_path.to_str().unwrap(),
            "--signing-seed",
            "tls-notary-test",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let card = with_receipts(wait_cvm_count(&fixture, 1));
    assert_eq!(card["labels"]["capture_methods"]["provider_tls_notary"], 1);
    assert_eq!(card["labels"]["assurance"]["participant_controlled"], 1);
    assert_eq!(card["labels"]["enforcement"]["retrospective_proof"], 1);
    assert_eq!(card["stats"]["total_tokens"], 1234);
    assert_eq!(card["stats"]["llm_calls"], 17);
    let event = single_event_from_card(&card);
    assert_eq!(event.kind, "provider.usage_snapshot");
    assert_eq!(event.capture_method, "provider_tls_notary");
    assert_eq!(event.assurance, "participant_controlled");
    assert_eq!(event.actor.kind, "participant");
    assert_eq!(event.actor.agent_session_id, "provider_account_snapshot");
    assert_eq!(event.subject["provider"], "anthropic");
    assert_eq!(
        event.subject["account_scope"],
        "participant_selected_account"
    );
    assert_eq!(
        event.subject["target_origin"],
        "https://console.anthropic.com"
    );
    assert_eq!(event.metrics["total_tokens"], 1234);
    assert_eq!(event.metrics["llm_calls"], 17);
    assert_hash(&event.hashes["tls_notary_proof"]);
    assert_hash(&event.hashes["provider_usage_claim"]);
    assert_eq!(event.evidence["enforcement_mode"], "retrospective_proof");
    assert_eq!(event.evidence["notary_protocol"], "tlsnotary-mpc-tls");
    assert_eq!(
        event.evidence["notary_url"],
        "https://notary.cvm.example"
    );
    assert_eq!(
        event.evidence["completeness"],
        "provider_account_scoped_not_global"
    );
    assert_eq!(
        event.evidence["limitation"],
        "does_not_prove_absence_of_other_accounts_devices_or_providers"
    );
    assert_no_remote_attestation(&event);

    let proof = get_json(
        &reqwest::blocking::Client::new(),
        card["share"]["proof_url"].as_str().unwrap(),
    );
    assert_eq!(proof["event_hashes"].as_array().unwrap().len(), 1);
}

#[test]
fn sdk_cli_wrapper_capture_path_e2e() {
    let card = wrapper_path("run", "sdk_cli_wrapper", "participant_controlled", "routed");
    assert_eq!(card["labels"]["capture_methods"]["sdk_cli_wrapper"], 1);
    assert_eq!(card["labels"]["enforcement"]["routed"], 1);
    let event = single_event_from_card(&card);
    assert_wrapper_event(
        &event,
        "run",
        "sdk_cli_wrapper",
        "participant_controlled",
        "routed",
    );
    assert_no_remote_attestation(&event);
}

#[test]
fn strict_workbench_capture_path_e2e() {
    let card = wrapper_path(
        "workbench-run",
        "strict_workbench",
        "managed",
        "strict_workbench",
    );
    assert_eq!(card["labels"]["capture_methods"]["strict_workbench"], 1);
    assert_eq!(card["labels"]["enforcement"]["strict_workbench"], 1);
    let event = single_event_from_card(&card);
    assert_wrapper_event(
        &event,
        "workbench-run",
        "strict_workbench",
        "managed",
        "strict_workbench",
    );
    assert_no_remote_attestation(&event);
}

#[test]
fn tee_workspace_capture_path_e2e() {
    let card = gateway_path("tee_workspace", "attested", "full_tee", "tee-workspace");
    assert_eq!(card["labels"]["capture_methods"]["tee_workspace"], 1);
    assert_eq!(card["labels"]["assurance"]["attested"], 1);
    assert_eq!(card["labels"]["enforcement"]["full_tee"], 1);
    let event = single_event_from_card(&card);
    assert_gateway_call_event(
        &event,
        "tee_workspace",
        "attested",
        "full_tee",
        "tee-workspace",
    );
    assert_eq!(event.evidence["ra_profile"], CAPTURE_RA_PROFILE);
    assert_eq!(event.evidence["ra_platform"], "tdx");
    assert!(event.evidence["ra_cvm_receipt_hash"].starts_with("sha256:"));
}

fn wrapper_path(
    subcommand: &str,
    capture_method: &str,
    assurance: &str,
    enforcement: &str,
) -> Value {
    let fixture = start_event_fixture(subcommand);
    let status = Command::new(env!("CARGO_BIN_EXE_llmattest"))
        .args([
            subcommand,
            "--event-sink-url",
            &fixture.event_sink_url,
            "--hackathon-id",
            &fixture.event_id,
            "--team-id",
            &fixture.team_id,
            "--capture-method",
            capture_method,
            "--assurance",
            assurance,
            "--enforcement",
            enforcement,
            "--agent-session-id",
            &format!("agent_{subcommand}"),
            "--signing-seed",
            subcommand,
            "--",
            "/usr/bin/true",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    with_receipts(wait_cvm_count(&fixture, 1))
}

fn base_report(fixture: &EventFixture, capture_method: &str, assurance: &str) -> Value {
    json!({
        "kind": "llm.call",
        "hackathon_id": fixture.event_id,
        "team_id": fixture.team_id,
        "gateway_id": "capture_test",
        "manifest_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "capture_method": capture_method,
        "assurance": assurance,
        "actor": { "kind": "agent", "agent_session_id": "agent_capture_test" },
        "subject": {
            "provider": "browser-test",
            "model": "browser-model",
            "api_family": "browser-ui"
        },
        "metrics": {
            "input_tokens": 2,
            "output_tokens": 5,
            "total_tokens": 7
        },
        "hashes": {
            "request": "sha256:req",
            "response": "sha256:resp"
        },
        "evidence": {
            "enforcement_mode": "routed"
        }
    })
}
