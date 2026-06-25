//! Networking (attested TLS, ACME, vsock bridge, CT) — re-exported from the
//! canonical `unified-quote` crate. The attested-TLS stack is the base layer's;
//! cvm-agent's product code (http_service, llm_*) builds on top of it.

pub use unified_quote::net::*;
