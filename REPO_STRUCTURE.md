# Repository Structure

This repo has two generations of code:

- `v2/` is the current implementation. Use this for LLM Attested, hosted event
  service work, capture paths, run cards, attestation verification, packaging,
  and tests.
- `src/`, `stage0/`, `contracts/`, `deploy/`, `testdata/`, and `tests/` are
  the original Runcard build/quote proof surface. Keep changes here focused on
  legacy build attestation unless a v2 feature explicitly needs shared code.

Current product surface:

- `v2/src/bin/llmattest.rs`: participant CLI and one-binary orchestration.
- `v2/src/bin/llm-event-service.rs`: organizer web service, join pages,
  dashboards, hosted/self-hosted event setup, and live run cards.
- `v2/src/bin/llm-gateway.rs`: OpenAI-compatible capture gateway and local
  proxy mode.
- `v2/src/bin/llm-capture-daemon.rs`: local companion service for browser and
  manual capture reports.
- `v2/src/http_service.rs`: shared Hyper HTTP/1 server, bounded concurrency,
  body limits, CORS, and optional direct Rustls termination.
- `v2/src/llm_attested.rs`: event, manifest, claim, and run-card data model.
- `v2/src/llm_attested_net.rs`: shared request limiting and backoff policy.
- `v2/src/llm_capture.rs`: normalized participant-side capture reports.
- `v2/capture/`: non-Rust capture adapter assets, currently the browser
  extension.
- `v2/tests/`: local end-to-end coverage for hosted service, capture paths,
  attestation fixtures, and TLS flows.

Web and card assets:

- `docs/index.html`: GitHub Pages site.
- `docs/assets/`: static site assets and published team/individual SVG cards.
- `docs/card-lab/`: generated card concept lab and source script.

Generated/local-only directories:

- `target/`, `v2/target/`, and `dist/` are build outputs.
- `.DS_Store` and editor swap files are ignored.
