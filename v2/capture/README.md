# Capture Adapters

This directory only contains capture adapters that are not Rust binaries.
Right now that means the browser extension.

The other capture paths are implemented in the Rust entrypoints because they
share signing, event-envelope, rate-limit, and delivery code:

| Capture path | Implementation |
| --- | --- |
| Attested gateway | `src/bin/llm-gateway.rs` |
| Local proxy | `src/bin/llm-gateway.rs` with `--capture-method local_proxy` |
| Browser extension | `capture/browser-extension/` plus `src/bin/llm-capture-daemon.rs` |
| CLI / SDK wrapper | `src/bin/llmattest.rs` |
| Strict workbench | `src/bin/llmattest.rs` workbench mode plus gateway/event service |
| TEE workspace | `src/bin/llm-gateway.rs` with attested receipt evidence |
| Provider TLS notary import | `src/bin/llmattest.rs` notary import mode |
| Manual import | `src/bin/llmattest.rs` emit mode or `src/bin/llm-capture-daemon.rs` |

End-to-end coverage for these paths lives in:

- `tests/llm_capture_paths_e2e.rs`
- `tests/llm_remote_attestation_e2e.rs`

If an adapter needs non-Rust deployable files, put them in this directory.
If it is a Rust service or CLI mode, keep the implementation under `src/bin/`
and add a row here.
