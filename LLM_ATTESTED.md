# LLM Attested

LLM Attested is a Cvm application for hackathons where model and agent use
can be captured, scored, and audited without forcing teams onto a specific
model, provider, IDE, or agent.

Hackathons are the first wedge, not the whole product. The same system should
also support a generic `universal` event profile where any individual can start
capturing AI activity and publish a personal cvm. That cvm can be used
as a public proof-of-work or proof-of-usage card: across web, CLI, local model
servers, agents, and workbenches, with explicit visibility controls for what
the person wants to show.

The hosted product starts with an event service. An organizer creates an event,
gets a dashboard URL and a join URL, and shares the join URL in their own
hackathon channels. Teams self-serve from there. The organizer should not have
to distribute API keys, explain attestation, or debug agent configuration.

The product promise:

> This app was built under this hackathon policy, by these teams and agent
> sessions, using these captured model actions, with these token totals, under
> this assurance level.

The important word is "captured." Cvm can prove attested gateway code and
the events it signed. It cannot magically prove that a contestant on an
unrestricted laptop did not paste code from an outside chatbot. Strong "all LLM
actions" claims require an enforced environment: managed credentials, egress
controls, or a hosted workbench.

## Non-Negotiables

1. **No required LLM.** OpenAI, Anthropic, Gemini, OpenRouter, local models,
   and future providers must all fit.
2. **No required agent.** Codex, Cursor, Claude Code, aider, shell scripts,
   browser chat, custom agents, and raw API users must all have a path.
3. **Organizer shares one URL.** Event creation returns a join URL and a
   dashboard URL. Team onboarding is self-service.
4. **Assurance is labeled.** Broad compatibility is welcome, but the dashboard
   must show whether an event came from an attested gateway, local proxy,
   browser extension, hosted workbench, or self-reported path.
5. **Universal cvms exist.** A hackathon is just one event type. A person
   should be able to create a persistent cvm for their own AI activity and
   link it from a profile, portfolio, or social account.

## Shape

LLM Attested should be a hosted event system, not an agent framework and not a
model router monoculture. The gateway is one capture adapter. The browser
extension, local proxy, hosted workbench, and future integrations are other
adapters. They all emit the same signed event shape.

There should be one stable core and several thin shells.

The stable core is:

1. **An attested event log.** Every competition-relevant action becomes a
   signed event in one append-only log.
2. **A contest manifest.** The manifest names the policy, scoring rule,
   gateway receipt, enforcement mode, and public verification keys.
3. **A scorer.** A scorer is a pure function over the verified manifest, event
   log, and final submission.

Everything else is an adapter:

- **Hosted event service.** Creates events, dashboards, join URLs, and
   self-service team start credentials.
- **Attested LLM gateway.** Provider-compatible APIs that proxy requests,
   meter usage, enforce policy, and sign receipts from inside a TEE.
- **Organizer dashboard.** Live leaderboard, per-team receipts, audit views,
   model/provider breakdowns, and final submission cards.
- **Contestant setup.** One command that gives a team API key and prints the
   environment variables for common agents.
- **Strict workbench.** Optional devcontainer, VM, or hosted runner whose
   outbound network can only reach the attested gateway and approved developer
   services.
- **GitHub integration.** A final build receipt ties the code submission,
   GitHub commit, Cvm Value X, and LLM event log root together.

This keeps the product from turning into a nest of special cases. New
competition types should not require new gateway APIs. They should require new
event kinds or new scorers.

## Architecture

```text
organizer
   |
   | POST /events
   v
hosted event service -----------------------> dashboard / leaderboard
   |
   | join URL shared in Discord, website, email, QR code
   v
teams
   |
   | choose easiest capture method for their tools
   v
gateway / local proxy / browser extension / hosted workbench
   |
   | signed ContestEvent
   v
append-only event ledger
   |
   | optional provider request
   v
any LLM provider, model, local runtime, or agent workflow
```

Capture adapters are the trust boundary for the events they emit. Upstream
providers are sources of model output and provider-reported usage; they are not
the attestation root.

The dashboard verifies the gateway once through Cvm attested TLS, caches
the gateway boot receipt, then verifies cheap event signatures from that
gateway. This follows the existing "bootstrap once, then cheap verification"
pattern in `DESIGN.md`.

## Hosted and Self-Hosted

The hosted product and a self-hosted government or enterprise deployment should
use the same client protocol. The only thing that changes is the URL teams
start from and the trust registry named by that URL.

A self-hosted organizer runs:

- `llm-event-service` on the organizer's domain.
- One or more `llm-gateway` instances under the organizer's control.
- Their chosen Cvm verifier root and policy for TDX, SEV-SNP, Nitro, or
  any future verifier profile the deployment explicitly accepts.
- Their own provider credentials, egress policy, log retention policy, and
  dashboard access controls.

The self-hosted service publishes:

```http
GET /.well-known/llm-attested/self-host.json
GET /.well-known/cvm/registry.json
GET /e/<event_id>/bootstrap.json
```

Each event bootstrap names the gateway manifest URL, event sink URL, capture
methods, and `trust_policy`. A client does not need to know whether the
competition is hosted by Cvm or by an agency. It follows the bootstrap,
fetches the gateway manifest, fetches the Cvm receipt named by the trust
policy, verifies the EAT against the named registry, and then routes model
traffic through the advertised capture method.

The gateway publishes:

```http
GET /.well-known/cvm/receipt
GET /.well-known/llm-attested/manifest.cbor
GET /.well-known/llm-attested/manifest.json
```

`accepted_tee_platforms` is a policy label, not a hardcoded product lock. The
current Cvm verifier path already covers the supported roots in this repo;
SGX can be added as another accepted platform/profile when verifier support is
available. Clients must display the actual verified platform and assurance
level instead of collapsing everything into a generic "secure" badge.

## Enforcement Modes

There should be three explicit modes. Mixing them would make the product look
stronger than it is.

| Mode | Contestant change | What it proves | Best for |
| --- | --- | --- | --- |
| Routed | Set base URL and API key | Calls through the gateway are real and metered | Demos, casual hackathons |
| Managed keys | Use only organizer-issued model credentials | All billable provider calls using organizer keys went through the gateway | Token-budget contests |
| Strict workbench | Build inside hosted/devcontainer environment with egress locked to the gateway | LLM API traffic from that environment went through the gateway | Serious prizes, adversarial scoring |

The dashboard should display the mode on every team card. A "least tokens"
winner from Routed mode is fun, but a "least tokens" winner from Strict mode is
credible.

## Compatibility Strategy

The system should avoid per-agent integrations for the hot path. Capture
methods should meet teams where they already are:

1. **Hosted gateway.** Best when an agent can set a base URL or provider key.
2. **Local proxy.** Best when a team wants to keep using their own keys and
   tools while still emitting signed events.
3. **Browser extension.** Best for ChatGPT, Claude, Gemini, or other web UIs.
4. **Hosted workbench.** Best for serious prizes where egress and environment
   controls matter.
5. **Manual/event import.** Best as a low-assurance fallback rather than
   excluding a team.

All ingress protocols normalize into `ContestEvent`. Competition logic never
depends on which capture method produced the event.

Supported capture paths:

| Capture path | Implementation | Assurance label | Enforcement label |
| --- | --- | --- | --- |
| Attested gateway | `llm-gateway` with event signing and Cvm manifest/receipt endpoints | `attested` | `routed` or `managed_keys` |
| Local capture proxy | `llm-gateway --upstream-no-auth --capture-method local_proxy` pointing at Ollama, LM Studio, oMLX, vLLM, etc. | `participant_controlled` | `routed` |
| Browser extension | `capture/browser-extension` plus `llm-capture-daemon` companion | `participant_controlled` | `routed` |
| SDK/CLI wrapper | `llmattest run` | `participant_controlled` | `routed` |
| Organizer-hosted local/open model gateway | `llm-gateway --upstream-no-auth` against organizer model infrastructure | `managed` | `managed_keys` |
| Strict workbench | `llmattest workbench-run` inside a locked-down workspace image/profile | `managed` | `strict_workbench` |
| Full TEE workspace | `llmattest tee-run` inside the Cvm/TEE workspace launcher | `attested` | `full_tee` |
| Manual import | `llmattest emit` or `llm-capture-daemon /capture/manual` | `self_reported` | usually `routed` or `none` |
| Provider TLS/MPC notary fallback | `llmattest notary-import` with a TLSNotary/MPC-TLS proof of a provider usage page or API response | `participant_controlled` | `retrospective_proof` |

The implementation rule is simple: every path emits `ContestEvent` CBOR to the
event service. The dashboard and scorer see one event shape, while the cvm
shows the capture path and assurance differences clearly.

### Retrospective Provider Proofs

There should be a mercy lane for teams that forgot to install the capture stack
or were working in a weird environment. TLSNotary-style MPC-TLS proofs can help
there: the participant proves that a specific HTTPS response came from a
provider account page or usage API, and then imports that proof into their
cvm.

The path is intentionally labeled differently:

```sh
llmattest notary-import \
  --event-sink-url "$EVENT_SINK_URL" \
  --hackathon-id "$EVENT_ID" \
  --team-id "$TEAM_ID" \
  --provider anthropic \
  --target-origin https://console.anthropic.com \
  --proof anthropic-usage.tlsn \
  --claim anthropic-usage.json
```

This emits a `provider.usage_snapshot` event with:

- `capture_method = provider_tls_notary`
- `assurance = participant_controlled`
- `enforcement_mode = retrospective_proof`
- `completeness = provider_account_scoped_not_global`

What it can prove:

- A usage snapshot or API response was authentic for the named HTTPS origin.
- The imported token/call totals match the participant's disclosed claim.
- The proof file hash and claim hash are tied into the cvm receipt chain.

What it cannot prove:

- The participant did not use another provider account.
- The participant did not use another provider, device, browser profile, or
  teammate account.
- The participant disclosed every relevant field from the provider response.

So this should be allowed as a late-submission or audit aid, not as the same
assurance class as gateway, managed-key, workbench, or TEE capture.

These paths are an internal capability matrix, not a setup menu. Participants
should not decide between "browser extension," "local proxy," "workbench," or
"TEE workspace" by hand. The participant-facing product should be one
`llmattest` binary for macOS, Linux, and Windows. Mobile can use the same
protocol through a native app or browser-extension companion if the event
requires mobile/web capture.

The one-binary participant flow:

1. Participant opens the join URL and creates a team.
2. The page gives one command or download for `llmattest`.
3. `llmattest start --team-payload team.json -- <agent command>` saves the
   event/team payload to `~/.llmattest/config.json`, starts the local browser
   and manual capture companion, emits a smoke event, and launches the wrapped
   tool with capture environment variables.
4. `llmattest status` checks the bootstrap URL, cvm URL, gateway health,
   and manifest URL.
5. `llmattest smoke` emits a `capture.health` event so the participant and
   organizer can see the cvm move before real work starts.
6. `llmattest` probes the machine and event policy:
   - Is this an organizer-managed workbench?
   - Is this a TEE workspace?
   - Is the attested gateway reachable?
   - Is the agent using OpenAI-compatible, Anthropic-compatible, or
     Ollama-compatible settings?
   - Are local runtimes such as Ollama, LM Studio, oMLX, MLX Studio, llama.cpp,
     or vLLM reachable?
   - Is browser capture needed for the active workflow?
7. `llmattest` chooses the strongest working path allowed by the event policy.
8. `llmattest run -- ...` or the command passed to `llmattest start` injects
   `OPENAI_BASE_URL`, `OPENAI_API_KEY`, `LLM_ATTESTED_TEAM_ID`,
   `LLM_ATTESTED_CVM_URL`, and local capture URLs for tools it wraps.
9. The participant sees one status: connected, capturing, and current cvm.

`llmattest start` reports these probes as a single `capability_probes` array.
That array is the narrow handoff between environment discovery and capture
selection; new competition types should add policy/scoring on top of the probe
facts instead of adding participant-facing setup branches.

The selected path is reported separately as `selected_capture_path` with
`user_configurable: false`. Participants can see why a path was selected, but
they do not self-assign assurance or enforcement labels.

Local capture durability is part of the one-binary contract. The companion
appends every accepted capture to an event log before delivery. If the event
service is unavailable, it returns `queued: true`, writes the event to a pending
queue, and retries with backoff. The hosted/self-hosted event service treats
replayed event hashes idempotently so retry storms do not inflate cvms.

Participant status is also one command:

```sh
llmattest status --watch
```

The watch payload includes the live cvm URL, selected capture path, latest
event hash/receipt, capability probes, and actionable recovery steps. This is
what support staff should ask teams to paste before escalating a setup issue.

Cross-platform packaging lives in `packaging/`:

- `package-llmattest.sh` builds release archives for a requested Rust target.
- `install.sh` installs or updates macOS/Linux from a release URL or local
  archive.
- `install.ps1` installs or updates Windows from a release URL or local archive.
- `.github/workflows/llmattest-release.yml` produces macOS, Linux, and Windows
  archives and SHA-256 sidecars.

`llmattest start` runs the browser/manual companion from the same executable.
The standalone companion remains available for development and compatibility:

```sh
llm-capture-daemon --config ~/.llmattest/config.json
```

That keeps setup support centered around one file and one participant identity
instead of asking people to copy event IDs, sink URLs, team IDs, and keys into
several tools.

The user should not configure `capture_method`, `assurance`, or `enforcement`.
Those labels are assigned by the platform from measured facts: where the code
is running, what network path is active, what policy the event requires, and
what evidence the capture adapter can actually produce.

Smallest useful protocol surface:

- OpenAI-compatible `/v1/chat/completions`
- OpenAI-compatible `/v1/responses`
- Streaming responses for OpenAI SSE
- OpenRouter upstream routing for broad provider coverage

Nice-to-have after the first demo:

- OpenAI-compatible `/v1/embeddings`
- Anthropic-compatible `/v1/messages`
- Anthropic-compatible `/v1/messages/count_tokens`
- Gemini `generateContent` and streaming APIs
- Ollama-compatible local model routing for offline events
- MCP server wrapper that exposes the gateway config to agent tools

Local model support should target API shape before product identity. The first
adapter should work with any local server that exposes OpenAI-compatible,
Anthropic-compatible, or Ollama-compatible HTTP APIs, and then add named
setup recipes for popular runtimes.

Important local serving targets:

- Cross-platform/common: Ollama, LM Studio, llama.cpp server, vLLM, LocalAI,
  Jan, and GPT4All.
- Apple Silicon / MLX: oMLX, MLXHub, Pico AI Server, macMLX, MLX Studio,
  Ai Keeper, vMLX, MLX Omni Server, mlx-serve, mlx-lm server, and similar
  MLX-based OpenAI-compatible servers.

The dashboard should label these as `provider = local` with separate runtime
metadata, for example `runtime = omlx`, `mlxhub`, `pico-mlx`, `ollama`,
`lm-studio`, or `llama.cpp`. A participant-hosted local runtime is still lower
assurance unless it runs inside a strict workbench or the organizer hosts the
runtime behind an attested gateway.

For agents that support `OPENAI_BASE_URL`, `ANTHROPIC_BASE_URL`, or an
equivalent setting, setup is a tiny config change. For teams using browser UIs
or hardcoded endpoints, the join page should offer the browser extension,
local proxy, or hosted workbench. Transparent TLS interception is not a good
default: it is brittle, invasive, and often breaks certificate pinning.

## Receipts

There are two receipt layers, but only one public application manifest. Avoid
separate boot JSON, JWKS, policy, and dashboard config APIs until there is a
real caller that needs them independently.

### Gateway Boot Receipt

The boot receipt proves the gateway service itself is genuine. It is produced
by the existing Cvm runtime flow:

- Stage 0 builds the gateway.
- Stage 1 runs the gateway inside a TEE.
- Attested TLS binds the live TLS key to the Cvm EAT.
- The dashboard verifies the EAT, quote binding, hardware root, and Value X.

The gateway should then serve:

```http
GET /.well-known/cvm/receipt
GET /.well-known/llm-attested/manifest.cbor
```

`manifest.cbor` is fetched over verified attested TLS and logged by the
dashboard. It contains:

```json
{
  "version": 1,
  "profile": "https://cvm.dev/llm-contest-manifest/v1",
  "issuer": "https://events.example",
  "hackathon_id": "hk_...",
  "gateway_id": "gw_...",
  "competition_url": "https://events.example/e/hk_...",
  "gateway_manifest_url": "https://gateway.example/.well-known/llm-attested/manifest.cbor",
  "cvm_receipt_hash": "sha256:...",
  "value_x": "sha384:...",
  "policy_hash": "sha256:...",
  "scoring_rule_hash": "sha256:...",
  "enforcement_mode": "routed",
  "start_url": "https://events.example/e/hk_.../join",
  "capture_methods": [
    "gateway_proxy",
    "local_proxy",
    "browser_extension",
    "hosted_workbench"
  ],
  "trust_policy": {
    "required": true,
    "accepted_tee_platforms": ["tdx", "sev-snp", "nitro"],
    "cvm_receipt_url": "https://gateway.example/.well-known/cvm/receipt",
    "registry_url": "https://events.example/.well-known/cvm/registry.json",
    "verifier_profile": "cvm-eat-v2"
  },
  "event_signing_jwk": {},
  "event_kinds": ["llm.call", "agent.session", "submission"]
}
```

For a hardened version, add a generic `app_claims_hash` or
`runtime_claims_hash` to a future EAT profile so the event signing key and
policy hash are committed directly into the hardware quote. Do not block the
MVP on that schema change; attested TLS plus immediate dashboard logging is
enough for the first demo.

### Event Receipt

Every model request gets a signed event, but the envelope should be generic.
Tokens are just one metric. This is what lets future competitions score tool
use, build time, test runs, network calls, agent count, human intervention, or
final code size without changing the log format.

```json
{
  "version": 1,
  "profile": "https://cvm.dev/contest-event/v1",
  "event_id": "evt_...",
  "kind": "llm.call",
  "hackathon_id": "hk_...",
  "team_id": "team_...",
  "gateway_id": "gw_...",
  "manifest_hash": "sha256:...",
  "capture_method": "gateway_proxy",
  "assurance": "routed",
  "event_index": 42,
  "previous_team_event_hash": "sha256:...",
  "started_at_ms": 1779105600000,
  "completed_at_ms": 1779105603000,
  "actor": {
    "agent_session_id": "agent_...",
    "kind": "agent"
  },
  "subject": {
    "provider": "openai",
    "provider_request_id": "req_...",
    "model": "gpt-...",
    "api_family": "openai-responses"
  },
  "metrics": {
    "input_tokens": 1234,
    "output_tokens": 567,
    "total_tokens": 1801,
    "tool_call_count": 3,
    "latency_ms": 3000
  },
  "hashes": {
    "request": "sha256:...",
    "response": "sha256:..."
  },
  "evidence": {
    "provider_usage": {},
    "gateway_usage": {
      "tokenizer": "provider-compatible-name-or-version"
    },
    "transcript_ciphertext_ref": "optional"
  },
  "signature": "base64url..."
}
```

The signed bytes should be canonical CBOR, even if the dashboard also exposes
JSON. The event signature key lives inside the attested gateway and is bound to
the boot receipt.

The gateway should return receipt handles in response headers:

```http
x-llm-attested-event-id: evt_...
x-llm-attested-receipt: https://events.example/hk/.../evt_...
x-llm-attested-team-root: sha256:...
```

For streaming responses, the receipt is finalized after the stream closes. The
gateway can emit a final SSE metadata event when the client protocol tolerates
it, but headers plus dashboard lookup are enough for compatibility.

Hosted capture adapters should also POST the signed CBOR event to:

```http
POST /e/<event_id>/events
Content-Type: application/cbor
```

The event service stores the raw signed event first. Verification and scoring
can run asynchronously from the append-only bytes.

## Ledger

The event ledger should be append-only and easy to verify offline.

MVP:

- Store events in Postgres or SQLite.
- Maintain a per-team hash chain using `previous_team_event_hash`.
- Periodically publish a hackathon root hash.
- Export `events.cborl` plus `checkpoint.json` at the end.

Hardened path:

- Merkle tree over all events.
- Signed checkpoints.
- Public inclusion proofs.
- Optional anchoring into Rekor, GitHub releases, object storage with
  immutable retention, or a chain.

The dashboard should never need raw provider secrets. It verifies signatures,
checks hash chain continuity, and computes scoring from signed event fields.

## Narrow Interfaces

The public interface should be smaller than the number of moving parts.

### Gateway

The gateway has one job:

```text
provider-shaped request -> upstream response + signed contest event
```

Do not expose provider adapters as a public plugin system in the first version.
They are internal compatibility functions that all emit the same event
envelope.

### Contest

A competition type should be a scorer:

```text
score(manifest, verified_events, submission) -> score_report
```

The scorer must not call providers, parse raw transcripts by default, or know
which ingress protocol produced an event. It receives verified events and
returns a deterministic report.

This single interface supports:

- minimum tokens: minimize `sum(metrics.total_tokens)`
- maximum tokens: maximize `sum(metrics.total_tokens)` under budget
- most agents: count distinct `actor.agent_session_id`
- smallest model: restrict or weight `subject.model`
- freeze compliance: reject events after a cutoff
- most tool use: score `tool.call` events
- fastest build: score `build.receipt` or `ci.run` events
- smallest final app: score `submission` metadata
- best local-model app: require `subject.provider = local`

### Dashboard

The dashboard should read the same `score_report` shape for every competition.
It should not get one custom leaderboard implementation per challenge.

The narrow dashboard contract:

```json
{
  "team_id": "team_...",
  "rank": 1,
  "score": 123,
  "unit": "tokens",
  "status": "eligible",
  "explanations": ["1801 total tokens", "strict workbench verified"],
  "receipt_refs": ["evt_...", "sub_..."]
}
```

## Dashboard

The dashboard should feel like a tournament control room, not a cryptography
demo.

Core views:

- **Leaderboard:** team ranking by current scoring rule.
- **Team card:** token totals, model mix, agent sessions, cost, latest event,
  final GitHub commit, final Cvm Value X, enforcement mode.
- **Attestation card:** gateway Value X, platform, quote status, policy hash,
  event signing key, verified-at time.
- **Event stream:** recent model calls with provider, model, tokens, agent,
  latency, and receipt status.
- **Audit drawer:** request/response hashes, transcript disclosure status,
  inclusion proof, signature verification, upstream request ID.
- **Submission page:** final commit, build receipt, event log root, score,
  disqualification flags.
- **Participant cvm:** per-team shareable cvm with capture methods,
  assurance labels, enforcement labels, token totals, min/max call tokens,
  model mix, provider mix, and agent-session counts.

Challenge types this unlocks:

- App built with minimum tokens.
- App built with maximum tokens under a cost ceiling.
- Most distinct agent sessions used.
- Best app using only small or local models.
- Best app with no calls after feature freeze.
- Best app with the smallest "human intervention" window, if the workbench
  logs agent sessions.
- Provider diversity challenge.
- Model escalation challenge: start with small models, justify every upgrade.

## Organizer Flow

1. Organizer opens the hosted event service and creates an event.
2. Service returns:
   - organizer URL
   - dashboard URL
   - team join URL
3. Organizer shares only the team join URL.
4. Each team opens the join URL, creates a team, and gets setup options:
   - API/base URL config for compatible agents
   - local proxy download or one-liner
   - browser extension link
   - hosted workbench link when enabled
5. Teams use any model, provider, IDE, or agent that fits a capture method.
6. Captured events appear live with `capture_method` and `assurance` labels.
7. Final submission runs a Cvm build or check.
8. Dashboard links final commit, final Value X, event log root, score, and
   assurance level.
9. Judges can verify the whole package offline.

## Participant Cvms

Each team receives a participant cvm generated from the captured event log:

```http
GET /e/<event_id>/teams/<team_id>/cvm.json
GET /e/<event_id>/cvms.json
```

The cvm must never flatten assurance into a single "attested" badge. It
shows the three dimensions separately:

```text
Capture method: Gateway / Local Proxy / Browser Extension / Workbench / TEE Workspace / Manual Import
Assurance: Attested / Managed / Participant Controlled / Self Reported
Enforcement: Routed / Managed Keys / Strict Workbench / Full TEE
```

It also shows the stats people want to share:

- total captured events
- total LLM calls
- total tokens
- minimum tokens for a single call
- maximum tokens for a single call
- model mix
- provider/runtime mix
- distinct agent sessions
- per-agent session counts

This lets the product say, precisely: "this team won minimum tokens under
Managed Keys" or "this team used local models through Participant Controlled
Local Proxy." Both can be fun. They are not the same security claim.

### Universal Cvms

Universal cvms remove the hackathon wrapper. The event type is
`universal`, the "team" is a person or project identity, and the goal is public
proof of selected AI activity rather than contest scoring.

Examples:

- "I used 40M captured tokens this month."
- "Here is my public model mix across CLI, web, and local inference."
- "Here is my agent-session history for this project."
- "Here are only the stats I elected to show: token totals and models, but not
  transcript hashes or exact agent sessions."

The same cvm schema applies, but visibility controls can hide or reveal:

- capture labels
- token stats
- models
- providers/runtimes
- agent sessions

The honest claim is still "captured activity," not "all possible activity."
The value is that the card is signed, timestamped, and backed by the same event
ledger as competitions.

### Shareable Cvms Are Live Objects

A cvm should not be only a PNG badge. The share URL is the canonical object,
and image/card artifacts are renderings of it.

Current event-service surfaces:

```http
GET /e/<event_id>/teams/<team_id>/cvm
GET /e/<event_id>/teams/<team_id>/cvm.json
GET /e/<event_id>/teams/<team_id>/cvm.embed
GET /e/<event_id>/teams/<team_id>/cvm.svg
GET /e/<event_id>/teams/<team_id>/cvm.qr.svg
GET /e/<event_id>/teams/<team_id>/cvm.proof.json
GET /e/<event_id>/teams/<team_id>/cvm.receipts.json
GET /e/<event_id>/teams/<team_id>/cvm.credential.json
GET /oembed?url=<cvm_url>
```

The user-facing page includes Open Graph and oEmbed metadata so social sites,
blogs, docs, and portfolio pages can render a preview. The iframe surface is a
live card that can refresh in place, so someone embedding it on a profile or
project page is not freezing the claim at screenshot time. The SVG surface is a
fallback for link previews and badge images. It includes a QR code back to the
canonical cvm URL, because screenshots and reposts will always happen.

The proof bundle is the narrow verifier interface:

- `cvm_json_hash`
- `event_log_root`
- ordered signed event hashes
- receipt export URL
- gateway manifest URL
- Cvm receipt URL
- registry URL

That keeps the marketing artifact and the verifier artifact connected without
making every consumer understand every capture adapter.

#### Provenance Standards

C2PA / Content Credentials are the right mental model for exported media:
tamper-evident provenance bound to a file. They are useful for future exported
PNG/SVG/PDF cvms, but they do not replace the live cvm URL because many
social and messaging platforms strip metadata, and a copied screenshot loses the
manifest. Cvm should use C2PA as an export layer, not as the canonical data
model.

W3C Verifiable Credentials and Open Badges 3.0 are a better fit for portable
achievement and identity claims. The current `cvm.credential.json` is a
draft credential-shaped export that points back to the proof bundle and signed
receipts. A later version should sign this with a standards-compatible VC proof
format such as Data Integrity or JOSE/COSE once the issuer key lifecycle is
settled.

Digital Credentials API support is a future browser/wallet presentation path:
a viewer could ask the holder to present a cvm credential directly from a
wallet. That is not required for the first product, and it should not block the
simple share URL / iframe / QR path.

#### Attested Rendering

The strongest public flex is a cvm URL rendered by a service whose TLS
certificate is itself bound to a Cvm EAT. In that mode the iframe is not
"some HTML we hosted"; it is a live renderer running a known cvm renderer
Value X inside TDX, SEV-SNP, Nitro, or another supported root.

The browser still will not natively show a TEE lock icon, so the page should
make verification easy:

- `/.well-known/cvm/registry.json` names the verifier policy.
- The cvm links its proof bundle and Cvm receipt.
- A future `POST /proof/challenge` can return a fresh nonce-bound quote for the
  renderer, gateway, or strict workspace.
- The QR code always lands on the canonical URL, not on an image CDN copy.

This gives hackers something they can inspect and build around today, while
leaving room for C2PA exports, wallet credentials, and browser-native
credential presentation as the ecosystem matures.

## Security Properties

What the system can prove:

- The gateway code was the reviewed gateway source, running inside genuine TEE
  hardware, verified by Cvm.
- The gateway saw and proxied specific model calls.
- Token/cost metrics in receipts were computed by the gateway code, with
  provider usage copied where available.
- Receipts came from the attested gateway's event signing key.
- The event log for a team was not edited without breaking the hash chain.
- In Strict mode, LLM API traffic from the controlled workbench had to route
  through the gateway.
- In lower-assurance modes, exactly which capture method produced each event.

What the system cannot prove by itself:

- That a contestant did not use an outside chatbot on another device.
- That a self-declared `agent_id` on an unmanaged laptop corresponds to a real
  separate agent.
- That provider-reported token counts are always semantically identical across
  model families.
- That prompt or response content is honest if the gateway only stores hashes
  and no encrypted transcript.
- That browser-extension or local-proxy events have the same strength as
  attested gateway or strict workbench events.

Those limits are product features if they are labeled clearly. They let
organizers choose the right enforcement level for the prize.

## Abuse Controls

The gateway holds valuable provider keys, so treat it like the shadow service:
adversarial by default.

- Per-team budgets and hard spend caps.
- Per-provider and per-model allowlists.
- Rate limits by team, agent session, and IP/workbench.
- Persistent cooldown state so restarts do not clear abusive sessions.
- Exponential backoff with `429 Too Many Requests` and `Retry-After`.
- Bounded connection concurrency and short request-read timeouts.
- Append-only event ingest logs instead of rewriting the full event store for
  every model call.
- No arbitrary outbound network from the gateway except provider endpoints and
  ledger storage.
- Provider API keys injected only inside the TEE.
- Request and response size limits.
- Prompt logging mode chosen by organizer: hashes only, encrypted transcript,
  or full disclosure.
- Signed manifest hash in every event.
- Clear disqualification flags instead of silent denial.

## Implementation Plan

### Phase 1: Hosted Event Service + Demo Gateway

Build the self-service event service and a single attested gateway capture
adapter. The gateway supports OpenAI-compatible chat, responses, streaming, and
configurable upstream routing.

The first code cut may buffer upstream responses while still preserving the
OpenAI-compatible response shape. True SSE pass-through is the next
compatibility step once the signed event path is stable.

Deliverables:

- `llm-event-service` binary for hosted event creation.
- `POST /events` that returns organizer, dashboard, and join URLs.
- `POST /e/<event>/teams` that self-issues team start config.
- `POST /e/<event>/events` that stores signed event CBOR from capture adapters.
- `GET /.well-known/llm-attested/self-host.json` for self-host discovery.
- `GET /.well-known/cvm/registry.json` for deployment trust policy.
- `GET /e/<event>/bootstrap.json` for client bootstrapping.
- Join page with install/update commands, team payload creation, and one
  `llmattest start --team-payload team.json -- <agent>` command.
- `llm-gateway` binary that can run under `cvm run`.
- `llmattest` CLI for one-command startup, SDK wrapper, browser/manual local
  capture, strict workbench launcher, TEE workspace launcher, and manual event
  import.
- `llm-capture-daemon` compatibility binary for browser-extension and manual
  local capture.
- Team API keys and hackathon policy config.
- Event signing key generated on boot.
- Cvm receipt and `manifest.cbor` served by the gateway.
- Generic contest event CBOR receipts with `kind = "llm.call"`.
- Postgres or SQLite event store.
- One scorer interface and two built-in scorers:
  - minimum tokens
  - maximum tokens under budget
- Minimal dashboard that renders `score_report` and receipt verification.
- Participant cvms that expose capture method, assurance, enforcement,
  token stats, model mix, provider mix, and agent-session counts.

This is enough for an organizer to create an event, share one URL, and run
"minimum tokens" or "maximum tokens" events for teams using the gateway capture
path. Other teams can be onboarded as lower-assurance capture methods land.

### Phase 2: Compatibility

Add provider adapters and contestant setup tooling.

Deliverables:

- Local proxy capture adapter.
- Browser extension capture adapter for common hosted chat UIs.
- Anthropic `/v1/messages` adapter.
- Gemini adapter if needed by target agents.
- `llmattest login` that writes env snippets and tests the gateway.
- Agent session labels and per-agent API keys.
- GitHub check summary for final submission receipts.
- Additional event kinds only when a scorer needs them.

### Phase 3: Strict Workbench

Make serious competitions credible.

Deliverables:

- Devcontainer or VM image with egress restricted to the gateway, GitHub, and
  approved package registries.
- Agent launcher that assigns signed session IDs.
- Optional screen/terminal activity metadata, if organizers want it.
- Final Cvm build that embeds or references the event log root.
- Dashboard enforcement badges: Routed, Managed Keys, Strict Workbench.

### Phase 4: Hardened Attestation

Move from demo credibility to durable infrastructure.

Deliverables:

- Future EAT profile with `app_claims_hash` or `runtime_claims_hash`.
- Merkle transparency log and inclusion proofs.
- Offline verifier CLI:
  `llmattest verify --submission submission.json`.
- Multi-gateway trust domain with gateway rotation.
- Disaster controls for leaked team keys and provider key rotation.

## First Code Cut

The first implementation should not modify the core quote verifier. Build it as
an application on top of attested TLS:

1. Add a new gateway crate or binary.
2. Add a hosted event-service binary that creates events and team start URLs.
3. Reuse `EatToken`, attested TLS, and quote verification as-is.
4. Define `ContestManifest`, `ContestEvent`, and `ScoreReport` as
   application-level CBOR.
5. Make the dashboard verify the Cvm EAT once, then verify event receipts
   and score reports.
6. Keep provider adapters internal until at least three real agents need the
   same extension point.
7. Only consider an EAT schema change after the demo proves the event signing
   key and policy hash need offline quote-level binding.

That keeps Cvm's core invariant clean: the quote proves the running gateway
code. The LLM product proves what that gateway observed and signed.
