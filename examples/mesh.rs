//! cvm-agent meshing the stack.
//!
//! cvm-agent is the top layer: it gates privilege on a verified hardware
//! receipt — *no proof, no privilege*. This example shows it standing on the
//! base layer for real: it pulls in `unified-quote` (a cargo dependency, not a
//! copy) and uses that crate's verifier to check a real, hardware-rooted EAT
//! receipt — the same receipt format attestation-service issues and
//! attested-workload serves over attested TLS.
//!
//! Run:
//!   cargo run --example mesh                                   # bundled tdx chain
//!   cargo run --example mesh -- examples/fixtures/nitro_stage0.cbor
//!
//! The receipt bytes are real captured hardware quotes (see
//! unified-quote/v2/testdata/chain), verified offline against pinned vendor
//! roots. Exit code is non-zero if verification fails.

use unified_quote::eat::EatToken;
use unified_quote::quote::{verify::verify_platform_quote, Platform};

fn platform_name(p: Platform) -> &'static str {
    match p {
        Platform::Nitro => "nitro",
        Platform::SevSnp => "snp",
        Platform::Tdx => "tdx",
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "examples/fixtures/tdx_stage1.cbor".to_string());

    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("could not read receipt {path}: {e}");
        std::process::exit(2);
    });

    let token = EatToken::from_cbor(&bytes).unwrap_or_else(|e| {
        eprintln!("not a valid unified-quote EAT: {e}");
        std::process::exit(2);
    });

    // Walk the chain runtime-first, back to the build (stage 0).
    let mut chain = vec![token.clone()];
    let mut cursor = token.clone();
    while let Ok(Some(prev)) = cursor.decode_previous() {
        chain.push(prev.clone());
        cursor = prev;
    }

    println!("cvm-agent → unified-quote mesh check");
    println!("  receipt : {path}");
    println!("  format  : eat v{} · {} stage(s)", token.version, chain.len());

    let mut all_ok = true;
    for (i, t) in chain.iter().enumerate() {
        let stage = match (i, chain.len()) {
            (_, 1) => "stage0",
            (0, _) => "stage1 (runtime)",
            _ => "stage0 (build)",
        };
        match t.platform_enum() {
            Some(p) if !t.platform_quote.is_empty() => {
                match verify_platform_quote(p, &t.platform_quote, &t.binding_bytes()) {
                    Ok(meas) => {
                        println!(
                            "  [{stage}] {} → VERIFIED against pinned {} vendor root",
                            platform_name(p),
                            platform_name(p)
                        );
                        for (k, v) in meas.iter().take(2) {
                            let short = &v[..v.len().min(16)];
                            println!("           {k} = {}…", hex::encode(short));
                        }
                    }
                    Err(e) => {
                        all_ok = false;
                        println!("  [{stage}] {} → FAILED: {e}", platform_name(p));
                    }
                }
            }
            _ => println!("  [{stage}] software witness (no hardware quote)"),
        }
    }

    // value_x must be stable across the chain: the runtime is the built source.
    let value_x_stable = chain.windows(2).all(|w| w[0].value_x == w[1].value_x);
    println!("  value_x stable across chain: {value_x_stable}");

    let verdict = all_ok && value_x_stable;
    println!(
        "  verdict : {}",
        if verdict { "VERIFIED — privilege may be released" } else { "FAILED — no privilege" }
    );
    if !verdict {
        std::process::exit(1);
    }
}
