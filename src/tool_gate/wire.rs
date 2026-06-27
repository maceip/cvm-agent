//! HTTP wire types shared by the tool gate and eat-pass issuer/redeemer.

use eat_pass_core::transparency::{KeyRecord, SignedHead};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct KtResponse {
    pub log_pub: String,
    pub records: Vec<KeyRecord>,
    pub signed_head: SignedHead,
}

#[derive(Clone, Debug, Serialize)]
pub struct RedeemBody {
    pub key_epoch: u32,
    pub nonce: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AuthorizeBody {
    pub eat_b64: String,
    pub binding: String,
    pub max_batch: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AuthorizeResponse {
    pub authorization_b64: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignBody {
    pub req: eat_pass_core::SignRequest,
    pub authorization_b64: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EmailSendRequest {
    pub to: String,
    pub subject: String,
    pub body: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct EmailSendResponse {
    pub ok: bool,
    pub message_id: String,
    pub to: String,
    pub subject: String,
    pub proof: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolChallengeInfo {
    pub tool: String,
    pub issuer_name: String,
    pub origin_info: String,
    pub issuer_url: String,
    pub redeemer_url: String,
    pub kt_log_pub: String,
    pub attester_url: Option<String>,
    pub note: String,
}
