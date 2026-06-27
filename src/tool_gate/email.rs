//! SMTP send using credentials that never leave the tool gate.

use lettre::message::{header, Message, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use sha2::{Digest, Sha256};

use crate::tool_gate::intent::resolve_recipient;

#[derive(Clone, Debug)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
    pub allowed_to: Vec<String>,
    pub dry_run: bool,
}

impl SmtpConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let host = std::env::var("TOOL_GATE_SMTP_HOST")
            .map_err(|_| anyhow::anyhow!("TOOL_GATE_SMTP_HOST is required"))?;
        let port = std::env::var("TOOL_GATE_SMTP_PORT")
            .unwrap_or_else(|_| "587".into())
            .parse()
            .map_err(|e| anyhow::anyhow!("TOOL_GATE_SMTP_PORT: {e}"))?;
        let username = std::env::var("TOOL_GATE_SMTP_USER")
            .or_else(|_| std::env::var("TOOL_GATE_IMAP_USER"))
            .map_err(|_| {
                anyhow::anyhow!("TOOL_GATE_SMTP_USER (or TOOL_GATE_IMAP_USER) is required")
            })?;
        let password = std::env::var("TOOL_GATE_SMTP_PASSWORD")
            .or_else(|_| std::env::var("TOOL_GATE_IMAP_PASSWORD"))
            .map_err(|_| {
                anyhow::anyhow!(
                    "TOOL_GATE_SMTP_PASSWORD (or TOOL_GATE_IMAP_PASSWORD) is required — \
                     the mailbox secret stays on the tool gate, never in the agent"
                )
            })?;
        let from = std::env::var("TOOL_GATE_FROM")
            .map_err(|_| anyhow::anyhow!("TOOL_GATE_FROM is required"))?;
        let allowed_to = std::env::var("TOOL_GATE_ALLOWED_TO")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let dry_run = std::env::var("TOOL_GATE_DRY_RUN")
            .ok()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        Ok(Self {
            host,
            port,
            username,
            password,
            from,
            allowed_to,
            dry_run,
        })
    }

    fn check_allowed(&self, to: &str) -> anyhow::Result<()> {
        if self.allowed_to.is_empty() {
            return Ok(());
        }
        if self
            .allowed_to
            .iter()
            .any(|a| a.eq_ignore_ascii_case(to))
        {
            Ok(())
        } else {
            anyhow::bail!(
                "recipient {to} is not in TOOL_GATE_ALLOWED_TO allowlist ({})",
                self.allowed_to.join(", ")
            )
        }
    }
}

pub struct Mailer {
    cfg: SmtpConfig,
}

impl Mailer {
    pub fn new(cfg: SmtpConfig) -> Self {
        Self { cfg }
    }

    pub async fn send(&self, to_raw: &str, subject: &str, body: &str) -> anyhow::Result<String> {
        let to = resolve_recipient(to_raw)?;
        self.cfg.check_allowed(&to)?;

        if self.cfg.dry_run {
            let id = format!(
                "dry-run-{}",
                hex::encode(&Sha256::digest(format!("{to}{subject}{body}"))[..8])
            );
            eprintln!(
                "[tool-gate] DRY RUN email → {to} subject={subject:?} (would use SMTP {})",
                self.cfg.host
            );
            return Ok(id);
        }

        let email = Message::builder()
            .from(
                self.cfg
                    .from
                    .parse()
                    .map_err(|e| anyhow::anyhow!("TOOL_GATE_FROM invalid: {e}"))?,
            )
            .to(to
                .parse()
                .map_err(|e| anyhow::anyhow!("recipient invalid: {e}"))?)
            .subject(subject)
            .header(header::ContentType::TEXT_PLAIN)
            .singlepart(SinglePart::plain(body.to_string()))
            .map_err(|e| anyhow::anyhow!("build message: {e}"))?;

        let creds = Credentials::new(self.cfg.username.clone(), self.cfg.password.clone());
        let mailer = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.cfg.host)
            .map_err(|e| anyhow::anyhow!("smtp relay {}: {e}", self.cfg.host))?
            .port(self.cfg.port)
            .credentials(creds)
            .build();

        mailer
            .send(email)
            .await
            .map_err(|e| anyhow::anyhow!("smtp send: {e}"))?;

        Ok(format!(
            "sent-{}",
            hex::encode(&Sha256::digest(format!("{to}|{subject}|{body}"))[..12])
        ))
    }
}
