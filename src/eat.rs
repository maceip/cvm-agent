//! EAT token format — re-exported from the canonical `unified-quote` crate.
//!
//! cvm-agent does not carry its own copy of the format/verifier engine; it
//! depends on the base layer (see Cargo.toml) and re-exports it here so the
//! rest of this crate (and existing `crate::eat::…` paths) use exactly the
//! base-layer code.

pub use unified_quote::eat::*;
