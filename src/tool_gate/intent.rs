//! Per-request intent binding for tool calls.

use sha2::{Digest, Sha256};

const EMAIL_INTENT_DOMAIN: &[u8] = b"cvm/tool-gate/email/v1\0";

/// 32-byte redemption context for a specific email send intent.
pub fn email_redemption_context(to: &str, subject: &str, body: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(EMAIL_INTENT_DOMAIN);
    h.update(to.as_bytes());
    h.update([0u8]);
    h.update(subject.as_bytes());
    h.update([0u8]);
    h.update(body.as_bytes());
    h.finalize().into()
}

/// Resolve `ryan` → `ryan@company.com` via `TOOL_GATE_CONTACT_RYAN`.
pub fn resolve_recipient(raw: &str) -> anyhow::Result<String> {
    let trimmed = raw.trim();
    if trimmed.contains('@') {
        return Ok(trimmed.to_string());
    }
    let key = format!(
        "TOOL_GATE_CONTACT_{}",
        trimmed.to_ascii_uppercase().replace('-', "_")
    );
    if let Ok(email) = std::env::var(&key) {
        if !email.trim().is_empty() {
            return Ok(email.trim().to_string());
        }
    }
    anyhow::bail!(
        "recipient '{trimmed}' is not an email address and no env {key} is set \
         (set TOOL_GATE_CONTACT_RYAN=ryan@example.com for named contacts)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_hash_is_stable() {
        let a = email_redemption_context("a@b.c", "hi", "body");
        let b = email_redemption_context("a@b.c", "hi", "body");
        assert_eq!(a, b);
        assert_ne!(a, email_redemption_context("a@b.c", "hi", "other"));
    }
}
