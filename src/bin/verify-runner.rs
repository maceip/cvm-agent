//! Verify a live bountynet runner attestation endpoint.
//!
//! The URL is only a routing hint. Identity comes from a fresh
//! nonce-bound UnifiedQuote whose hardware quote verifies against the
//! platform root and whose report_data binds the quote signing key to
//! Value X.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context};
use rand::RngCore;
use serde_json::json;

use bountynet_shim::quote::{verify::verify_unified_quote, Platform, UnifiedQuote};

#[derive(Debug)]
struct Config {
    url: String,
    expected_value_x: Option<[u8; 48]>,
    expected_platform: Option<Platform>,
    expected_measurements: BTreeMap<String, Vec<u8>>,
    max_age_secs: u64,
}

fn main() -> anyhow::Result<()> {
    let cfg = parse_args()?;
    let nonce = random_nonce();
    let nonce_hex = hex::encode(nonce);
    let attest_url = full_quote_url(&cfg.url);

    eprintln!("[bountynet] === runner identity check ===");
    eprintln!("[bountynet] Endpoint: {attest_url}");
    eprintln!("[bountynet] Challenge nonce: {nonce_hex}");

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let quote: UnifiedQuote = client
        .post(&attest_url)
        .json(&json!({ "nonce": nonce_hex }))
        .send()
        .with_context(|| format!("POST {attest_url}"))?
        .error_for_status()
        .with_context(|| format!("non-success response from {attest_url}"))?
        .json()
        .context("decode UnifiedQuote JSON")?;

    if quote.nonce != nonce {
        bail!(
            "freshness check failed: quote nonce {} did not match challenge {}",
            hex::encode(quote.nonce),
            hex::encode(nonce)
        );
    }
    eprintln!("[bountynet] Freshness: PASS");

    if let Some(expected) = cfg.expected_platform {
        if quote.platform != expected {
            bail!(
                "platform mismatch: expected {:?}, got {:?}",
                expected,
                quote.platform
            );
        }
    }

    let result = verify_unified_quote(&quote, cfg.expected_value_x.as_ref())?;
    if !result.signature_valid || !result.platform_valid {
        bail!("quote verification returned a non-valid result");
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let age = now.saturating_sub(quote.timestamp);
    if age > cfg.max_age_secs {
        bail!(
            "quote is stale: age={}s exceeds max_age_secs={}",
            age,
            cfg.max_age_secs
        );
    }

    let measurement_map: BTreeMap<String, Vec<u8>> = result.measurements.iter().cloned().collect();
    for (name, expected) in &cfg.expected_measurements {
        let actual = measurement_map
            .get(name)
            .ok_or_else(|| anyhow!("expected measurement '{name}' was not present in quote"))?;
        if actual != expected {
            bail!(
                "measurement mismatch for {name}: expected {}, got {}",
                hex::encode(expected),
                hex::encode(actual)
            );
        }
    }

    eprintln!("[bountynet] Signature: PASS");
    eprintln!("[bountynet] Platform quote: PASS");
    eprintln!("[bountynet] Platform: {:?}", result.platform);
    eprintln!("[bountynet] Value X: {}", hex::encode(result.value_x));
    eprintln!("[bountynet] Quote age: {age}s");
    for (name, value) in &result.measurements {
        eprintln!("[bountynet]   {name}: {}", hex::encode(value));
    }
    eprintln!("[bountynet] === Identity Accepted ===");

    Ok(())
}

fn parse_args() -> anyhow::Result<Config> {
    let mut url = None;
    let mut expected_value_x = None;
    let mut expected_platform = None;
    let mut expected_measurements = BTreeMap::new();
    let mut max_age_secs = 300u64;

    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--expected-value-x" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow!("--expected-value-x requires a 48-byte hex value"))?;
                let bytes = hex::decode(raw.trim_start_matches("0x"))
                    .context("decode --expected-value-x")?;
                if bytes.len() != 48 {
                    bail!("--expected-value-x must decode to 48 bytes");
                }
                let mut arr = [0u8; 48];
                arr.copy_from_slice(&bytes);
                expected_value_x = Some(arr);
            }
            "--expected-platform" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow!("--expected-platform requires Nitro, SevSnp, or Tdx"))?;
                expected_platform = Some(parse_platform(&raw)?);
            }
            "--expect-measurement" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow!("--expect-measurement requires NAME=HEX"))?;
                let (name, hex_value) = raw
                    .split_once('=')
                    .ok_or_else(|| anyhow!("--expect-measurement requires NAME=HEX"))?;
                expected_measurements.insert(
                    name.to_string(),
                    hex::decode(hex_value.trim_start_matches("0x"))
                        .with_context(|| format!("decode measurement {name}"))?,
                );
            }
            "--max-age-secs" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow!("--max-age-secs requires a number"))?;
                max_age_secs = raw.parse().context("parse --max-age-secs")?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ if arg.starts_with("--") => bail!("unknown option: {arg}"),
            _ => {
                if url.is_some() {
                    bail!("unexpected extra argument: {arg}");
                }
                url = Some(arg);
            }
        }
    }

    let url = url.ok_or_else(|| {
        print_usage();
        anyhow!("missing runner attestation endpoint URL")
    })?;

    Ok(Config {
        url,
        expected_value_x,
        expected_platform,
        expected_measurements,
        max_age_secs,
    })
}

fn parse_platform(raw: &str) -> anyhow::Result<Platform> {
    match raw.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
        "nitro" => Ok(Platform::Nitro),
        "sevsnp" | "snp" => Ok(Platform::SevSnp),
        "tdx" => Ok(Platform::Tdx),
        _ => bail!("unknown platform: {raw}"),
    }
}

fn random_nonce() -> [u8; 32] {
    let mut nonce = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut nonce);
    nonce
}

fn full_quote_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/attest/full") {
        trimmed.to_string()
    } else if trimmed.ends_with("/attest") {
        format!("{trimmed}/full")
    } else {
        format!("{trimmed}/attest/full")
    }
}

fn print_usage() {
    eprintln!(
        "Usage: verify-runner <http://host:9384> [--expected-platform Tdx] \\
         [--expected-value-x HEX] [--expect-measurement NAME=HEX] \\
         [--max-age-secs 300]"
    );
}
