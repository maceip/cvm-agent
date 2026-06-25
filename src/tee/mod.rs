//! TEE quote collection (detect/snp/nitro/tdx/tpm/kds) — re-exported from the
//! canonical `unified-quote` crate. cvm-agent runs inside the TEE and collects
//! quotes using the base layer's collectors rather than a forked copy.

pub use unified_quote::tee::*;
