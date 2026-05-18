use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};
use std::net::TcpListener;
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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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

fn get_json(client: &reqwest::blocking::Client, url: String) -> Value {
    let body = client.get(url).send().unwrap().text().unwrap();
    serde_json::from_str(&body).unwrap()
}

fn post_json(client: &reqwest::blocking::Client, url: String, body: Value) -> Value {
    let body = client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&body).unwrap())
        .send()
        .unwrap()
        .text()
        .unwrap();
    serde_json::from_str(&body).unwrap()
}

#[test]
fn hosted_site_local_plain_mode_serves_full_user_journey() {
    let port = free_port();
    let base_url = format!("http://127.0.0.1:{port}");
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("store.json");
    let logs = temp.path().join("logs");
    let rate = temp.path().join("rate.json");

    let child = Command::new(env!("CARGO_BIN_EXE_llm-event-service"))
        .args([
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--public-base-url",
            &base_url,
            "--gateway-base-url",
            "http://127.0.0.1:18088",
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
    let _service = ChildGuard(child);
    wait_http_ok(&format!("{base_url}/healthz"));

    let client = reqwest::blocking::Client::new();

    let home = client.get(&base_url).send().unwrap().text().unwrap();
    assert!(home.contains("LLM Attested Events"));
    assert!(home.contains("plain local site"));

    let self_host = get_json(
        &client,
        format!("{base_url}/.well-known/llm-attested/self-host.json"),
    );
    assert_eq!(
        self_host["profile"],
        "https://runcard.dev/llm-self-host-instance/v1"
    );
    assert_eq!(self_host["runtime"]["mode"], "plain");
    assert_eq!(self_host["runtime"]["tee_attested"], false);

    let registry = get_json(
        &client,
        format!("{base_url}/.well-known/runcard/registry.json"),
    );
    assert_eq!(
        registry["profile"],
        "https://runcard.dev/runcard-trust-registry/v1"
    );
    assert!(registry["accepted_tee_platforms"]
        .as_array()
        .unwrap()
        .iter()
        .any(|platform| platform == "tdx"));

    let event = post_json(
        &client,
        format!("{base_url}/events"),
        json!({
            "name": "Local Hosted Site E2E",
            "event_type": "universal"
        }),
    );
    let event_id = event["event_id"].as_str().unwrap();
    assert!(event["join_url"].as_str().unwrap().ends_with("/join"));
    assert!(event["dashboard_url"]
        .as_str()
        .unwrap()
        .ends_with("/dashboard"));

    let join = client
        .get(format!("{base_url}/e/{event_id}/join"))
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(join.contains("llmattest start --team-payload team.json -- your-agent-command"));
    assert!(join.contains("llmattest status --watch"));

    let team = post_json(
        &client,
        format!("{base_url}/e/{event_id}/teams"),
        json!({ "team_name": "Local Solo" }),
    );
    let team_id = team["team_id"].as_str().unwrap();
    assert_eq!(team["team_name"], "Local Solo");
    assert!(team["runcard_url"].as_str().unwrap().contains(team_id));
    assert!(team["openai_compatible"]["OPENAI_BASE_URL"]
        .as_str()
        .unwrap()
        .ends_with("/v1"));

    let dashboard = client
        .get(format!("{base_url}/e/{event_id}/dashboard"))
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(dashboard.contains("Local Solo"));
    assert!(dashboard.contains("Captured events"));

    let runcard_json = get_json(
        &client,
        format!("{base_url}/e/{event_id}/teams/{team_id}/runcard.json"),
    );
    assert_eq!(
        runcard_json["profile"],
        "https://runcard.dev/llm-participant-runcard/v1"
    );
    assert_eq!(runcard_json["stats"]["captured_events"], 0);
    assert!(runcard_json["share"]["embed_url"]
        .as_str()
        .unwrap()
        .ends_with("/runcard.embed"));

    let runcard_page = client
        .get(format!("{base_url}/e/{event_id}/teams/{team_id}/runcard"))
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(runcard_page.contains("summary_large_image"));
    assert!(runcard_page.contains("iframe"));

    let embed = client
        .get(format!(
            "{base_url}/e/{event_id}/teams/{team_id}/runcard.embed"
        ))
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(embed.contains("Live runcard"));

    let svg = client
        .get(format!(
            "{base_url}/e/{event_id}/teams/{team_id}/runcard.svg"
        ))
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(svg.contains("<svg"));
    assert!(svg.contains("scan to verify"));

    let qr = client
        .get(format!(
            "{base_url}/e/{event_id}/teams/{team_id}/runcard.qr.svg"
        ))
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert!(qr.contains("QR code"));

    let proof = get_json(
        &client,
        format!("{base_url}/e/{event_id}/teams/{team_id}/runcard.proof.json"),
    );
    assert_eq!(proof["profile"], "https://runcard.dev/llm-runcard-proof/v1");

    let credential = get_json(
        &client,
        format!("{base_url}/e/{event_id}/teams/{team_id}/runcard.credential.json"),
    );
    assert!(credential["type"]
        .as_array()
        .unwrap()
        .iter()
        .any(|ty| ty == "RuncardCredential"));

    let oembed = get_json(
        &client,
        format!(
            "{base_url}/oembed?url={}",
            team["runcard_url"].as_str().unwrap()
        ),
    );
    assert_eq!(oembed["type"], "rich");
    assert!(oembed["html"].as_str().unwrap().contains("iframe"));
}

#[test]
fn local_site_scripts_are_shell_valid() {
    let packaging = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging");
    let status = Command::new("bash")
        .arg("-n")
        .arg(packaging.join("local-site.sh"))
        .arg(packaging.join("smoke-local-site.sh"))
        .status()
        .unwrap();
    assert!(status.success());
}
