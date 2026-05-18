# Hosted Site Local Testing

The hosted event website is `llm-event-service`. It serves the organizer flow,
participant join page, team payloads, dashboards, runcards, embeds, SVG cards,
QR codes, proof bundles, credentials, and trust registry.

Run it locally:

```bash
v2/packaging/local-site.sh
```

The script builds the v2 binaries, starts the site, creates a demo universal
event and team, writes `team.json` under `.local/llm-site`, and prints the local
join, dashboard, and runcard URLs.

Run a self-contained smoke test:

```bash
v2/packaging/smoke-local-site.sh --mode plain
```

On a TEE host, the same smoke test exercises the attested runtime path:

```bash
v2/packaging/smoke-local-site.sh --mode tee
```

The smoke test starts the website, verifies the health endpoint, self-host
document, trust registry, generated join/dashboard/runcard pages, and runcard
JSON labels, then stops the site.

## Modes

```bash
v2/packaging/local-site.sh --mode auto
v2/packaging/local-site.sh --mode plain
v2/packaging/local-site.sh --mode tee
```

`auto` is the default. On a normal laptop it runs the website directly in plain
local mode. On a TDX, SEV-SNP, or Nitro machine with a visible quote device, it
uses the existing Runcard chain:

```text
runcard build v2 -> attestation.cbor
runcard run v2 --attestation attestation.cbor -> llm-event-service
```

The same HTTP website is served in both modes. The difference is visible in:

```http
GET /.well-known/llm-attested/self-host.json
```

Plain local mode reports:

```json
{ "runtime": { "mode": "plain", "tee_attested": false } }
```

TEE mode reports:

```json
{
  "runtime": {
    "mode": "tee",
    "tee_attested": true,
    "value_x": "...",
    "domain": "..."
  }
}
```

The network URL is still only a routing hint. A deployment is trusted only after
the relevant Runcard receipt or attested TLS check verifies the hardware quote,
Value X, and measurement policy.

## Useful Options

```bash
v2/packaging/local-site.sh --listen 127.0.0.1:8099
v2/packaging/local-site.sh --public-base-url http://127.0.0.1:8099
v2/packaging/local-site.sh --gateway-base-url http://127.0.0.1:8088
v2/packaging/local-site.sh --state-dir /tmp/runcard-site
v2/packaging/local-site.sh --no-demo
```

Extra `llm-event-service` arguments can be passed after `--`.

```bash
v2/packaging/local-site.sh -- --max-connections 32
```

## Test

```bash
cargo test --manifest-path v2/Cargo.toml --test llm_hosted_site_e2e -- --test-threads=1
v2/packaging/smoke-local-site.sh --mode plain
```

This test starts the hosted site outside a TEE and checks the full high-level
user journey: home, self-host document, registry, event creation, join page,
team payload, dashboard, runcard JSON, runcard page, embed, SVG, QR, proof,
credential, and oEmbed.
