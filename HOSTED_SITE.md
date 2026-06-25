# Hosted Site Local Testing

The hosted event website is `llm-event-service`. It serves the organizer flow,
participant join page, team payloads, dashboards, cvms, embeds, SVG cards,
QR codes, proof bundles, credentials, and trust registry.

Run it locally:

```bash
packaging/local-site.sh
```

The script builds the Cvms binaries, starts the site, creates a demo universal
event and team, writes `team.json` under `.local/llm-site`, and prints the local
join, dashboard, and cvm URLs.

Run a self-contained smoke test:

```bash
packaging/smoke-local-site.sh --mode plain
```

On a TEE host, the same smoke test exercises the attested runtime path:

```bash
packaging/smoke-local-site.sh --mode tee
```

The smoke test starts the website, verifies the health endpoint, self-host
document, trust registry, generated join/dashboard/cvm pages, and cvm
JSON labels, then stops the site.

## Modes

```bash
packaging/local-site.sh --mode auto
packaging/local-site.sh --mode plain
packaging/local-site.sh --mode tee
```

`auto` is the default. On a normal laptop it runs the website directly in plain
local mode. On a TDX, SEV-SNP, or Nitro machine with a visible quote device, it
uses the existing Cvm chain:

```text
cvm build . -> attestation.cbor
cvm run . --attestation attestation.cbor -> llm-event-service
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
the relevant Cvm receipt or attested TLS check verifies the hardware quote,
Value X, and measurement policy.

## Useful Options

```bash
packaging/local-site.sh --listen 127.0.0.1:8099
packaging/local-site.sh --public-base-url http://127.0.0.1:8099
packaging/local-site.sh --gateway-base-url http://127.0.0.1:8088
packaging/local-site.sh --state-dir /tmp/cvm-site
packaging/local-site.sh --no-demo
```

Extra `llm-event-service` arguments can be passed after `--`.

```bash
packaging/local-site.sh -- --max-connections 32
```

## Test

```bash
cargo test --manifest-path Cargo.toml --test llm_hosted_site_e2e -- --test-threads=1
packaging/smoke-local-site.sh --mode plain
```

This test starts the hosted site outside a TEE and checks the full high-level
user journey: home, self-host document, registry, event creation, join page,
team payload, dashboard, cvm JSON, cvm page, embed, SVG, QR, proof,
credential, and oEmbed.
