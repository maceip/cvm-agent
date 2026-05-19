# Runcard v2

`v2/` is the active LLM Attested implementation. It combines the original
Runcard quote-verification work with a hackathon/event layer that records LLM
use through multiple capture paths and publishes team and individual run cards.

## Binaries

- `llmattest`: participant CLI and one-binary entrypoint. It probes local
  configuration, starts the needed local helpers, and emits normalized capture
  events.
- `llm-event-service`: organizer-facing web service. It creates events, serves
  join/dashboard/card pages, accepts captured events, and supports direct
  Rustls for local or TEE tests. Production deployment is expected to run behind
  Caddy, which terminates public TLS and forwards HTTP to this service.
- `llm-gateway`: OpenAI-compatible gateway/local proxy. It records routed model
  calls, token usage reported by the provider response, model/provider metadata,
  tool-call counts, and signed event-chain headers.
- `llm-capture-daemon`: local companion for browser-extension and manual
  capture reports.

## Shared Service Layer

Network services use `src/http_service.rs`, a narrow Hyper HTTP/1 adapter with:

- bounded connection concurrency
- bounded request bodies
- read timeout enforcement
- shared CORS/header response handling
- optional direct Rustls termination with caller-provided certificate, key, and
  client CA paths

Rate limiting and backoff use `governor` through `src/llm_attested_net.rs`.
The service binaries forbid unsafe Rust; low-level unsafe remains isolated in
TEE and vsock modules where kernel/device interfaces require it.

## Capture Paths

Adapter ownership is documented in `capture/README.md`. Non-Rust adapter files
live under `capture/`; Rust service and CLI modes live under `src/bin/`.

## Local Validation

Focused checks:

```sh
cargo check --bin llm-event-service --bin llm-gateway --bin llm-capture-daemon
cargo test --test llm_hosted_site_e2e -- --test-threads=1
cargo test --test llm_capture_paths_e2e -- --test-threads=1
```

Full local test sweep:

```sh
cargo test --tests -- --test-threads=1
```
