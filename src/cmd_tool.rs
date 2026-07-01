//! `cvm tool send-email` — mint eat-pass token inside attested CVM, call tool-gate.

use anyhow::{Context, Result};
use cvm_agent::tool_gate::{mint_email_send_token, EmailSendRequest, MintConfig, TOOL_EMAIL_SEND};
use eat_pass_cli::client::{EvidenceInput, DEFAULT_UQ_COLLECT_CMD};

pub async fn cmd_tool(args: &[String]) -> Result<()> {
    if args.is_empty() {
        print_tool_usage();
        anyhow::bail!("missing tool subcommand");
    }
    match args[0].as_str() {
        "send-email" => cmd_send_email(&args[1..]).await,
        other => {
            print_tool_usage();
            anyhow::bail!("unknown tool subcommand: {other}");
        }
    }
}

async fn cmd_send_email(args: &[String]) -> Result<()> {
    let mut to = None;
    let mut subject = None;
    let mut body = None;
    let mut gate = "http://127.0.0.1:8787".to_string();
    let mut issuer = "http://127.0.0.1:8088".to_string();
    let mut attester = "http://127.0.0.1:8087".to_string();
    let mut issuer_name = "issuer.eat-pass.dev".to_string();
    let mut origin_info = "tool-gate.secure.build/v1/tools/email.send".to_string();
    let mut kt_log_pub = String::new();
    let mut evidence = None;
    let mut uq_collect = None;
    let mut insecure_tls = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--to" => {
                i += 1;
                to = Some(args.get(i).context("--to")?.clone());
            }
            "--subject" => {
                i += 1;
                subject = Some(args.get(i).context("--subject")?.clone());
            }
            "--body" => {
                i += 1;
                body = Some(args.get(i).context("--body")?.clone());
            }
            "--gate" => {
                i += 1;
                gate = args.get(i).context("--gate")?.clone();
            }
            "--issuer" => {
                i += 1;
                issuer = args.get(i).context("--issuer")?.clone();
            }
            "--attester" => {
                i += 1;
                attester = args.get(i).context("--attester")?.clone();
            }
            "--issuer-name" => {
                i += 1;
                issuer_name = args.get(i).context("--issuer-name")?.clone();
            }
            "--origin-info" => {
                i += 1;
                origin_info = args.get(i).context("--origin-info")?.clone();
            }
            "--kt-log-pub" => {
                i += 1;
                kt_log_pub = args.get(i).context("--kt-log-pub")?.clone();
            }
            "--evidence" => {
                i += 1;
                evidence = Some(args.get(i).context("--evidence")?.clone());
            }
            "--uq-collect" => {
                i += 1;
                uq_collect = Some(args.get(i).context("--uq-collect")?.clone());
            }
            "--insecure-tls" => insecure_tls = true,
            "--help" | "-h" => {
                print_send_email_usage();
                return Ok(());
            }
            other => anyhow::bail!("unknown flag: {other}"),
        }
        i += 1;
    }

    let to = to.context("--to is required (email or contact name like ryan)")?;
    let subject = subject.unwrap_or_else(|| "Message from attested agent".into());
    let body = body.unwrap_or_else(|| "Sent via cvm tool-gate (eat-pass gated).".into());
    if kt_log_pub.is_empty() {
        anyhow::bail!("--kt-log-pub is required (64 hex chars, pin issuer transparency log key)");
    }
    let kt = parse_hex32(&kt_log_pub, "kt-log-pub")?;

    eprintln!("[cvm tool] minting eat-pass capability for email intent…");
    eprintln!("[cvm tool]   to={to} subject={subject:?}");

    let evidence = evidence
        .map(|path| EvidenceInput::File(path.into()))
        .unwrap_or_else(|| EvidenceInput::AzureCommand {
            cmd: uq_collect.unwrap_or_else(|| DEFAULT_UQ_COLLECT_CMD.into()),
        });

    let minted = mint_email_send_token(
        &MintConfig {
            gate_url: gate.clone(),
            issuer_url: issuer,
            attester_url: attester,
            issuer_name,
            origin_info,
            kt_log_pub: kt,
            evidence,
            insecure_tls,
        },
        &to,
        &subject,
        &body,
    )
    .await
    .context("mint eat-pass token")?;

    eprintln!("[cvm tool] token minted (binding={})", minted.binding_hex);

    let gate_base = gate.trim_end_matches('/');
    let url = format!("{gate_base}{TOOL_EMAIL_SEND}");
    let req_body = EmailSendRequest { to, subject, body };

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(insecure_tls)
        .build()?;

    let resp = client
        .post(&url)
        .header("authorization", &minted.authorization_header)
        .header("content-type", "application/json")
        .json(&req_body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("tool-gate rejected send ({status}): {text}");
    }

    println!("{text}");
    eprintln!("[cvm tool] email sent through gated mailbox (one-time token spent)");
    Ok(())
}

fn parse_hex32(s: &str, what: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(s.trim()).with_context(|| format!("{what}: hex"))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("{what}: expected 32 bytes (64 hex chars)"))
}

fn print_tool_usage() {
    eprintln!("Usage: cvm tool send-email --to <email|name> --kt-log-pub HEX [options]");
    print_send_email_usage();
}

fn print_send_email_usage() {
    eprintln!(
        "\nSend email through the attested tool gate (IMAP/SMTP password never on agent):\n\
         \n\
           cvm tool send-email --to ryan --subject \"Hi\" --body \"…\" \\\n\
             --kt-log-pub <64-hex> \\\n\
             [--gate URL] [--issuer URL] [--attester URL] [--evidence FILE|-] [--uq-collect CMD]\n\
         \n\
         Named contacts: TOOL_GATE_CONTACT_RYAN=ryan@example.com on the gate host."
    );
}
