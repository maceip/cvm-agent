//! Attested agent tool gate — privileged tools (email, secrets, …) gated on
//! eat-pass PrivateToken. Credentials stay on the gate; agents prove attested
//! build + one-time capability.

pub mod email;
pub mod intent;
pub mod mint;
pub mod origin;
pub mod wire;

pub use email::{Mailer, SmtpConfig};
pub use intent::{email_redemption_context, resolve_recipient};
pub use mint::{mint_email_send_token, MintConfig, MintedToolToken};
pub use origin::{EatPassGate, GateReject};
pub use wire::{EmailSendRequest, EmailSendResponse, ToolChallengeInfo};

pub const TOOL_EMAIL_SEND: &str = "/v1/tools/email.send";
