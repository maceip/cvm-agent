//! Quote format + verifier — re-exported from the canonical `unified-quote`
//! crate. This re-exports the platform enum and the `roots` / `verify`
//! submodules, so existing `crate::quote::verify::…` and `crate::quote::roots`
//! paths resolve to the base-layer implementation (no copy).

pub use unified_quote::quote::*;
