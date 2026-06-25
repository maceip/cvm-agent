# plans.md — confidential-compute stack (tracking doc)

Single source of truth for the four-project stack, their linkage, current
status, designs, and the roadmap. Lives in **cvm-agent** because cvm-agent is
the capstone that depends on the other three. Update this whenever scope,
naming, or status changes.

Last updated: 2026-06-25.

---

## 1. The stack (architecture)

The product (cvm-agent) stands on three lower layers. unified-quote is the
foundation everything else speaks.

```
cvm-agent            agent platform — the product capstone
   ├─ attested-workload     in-tee runtime: runs + serves a workload, attested TLS
   ├─ attestation-service   issues + verifies EAT receipts for source/builds
   └─ unified-quote         the quote format + verifier (the base layer)
```

Read bottom-up:

1. **unified-quote** defines one EAT-based receipt + verifier that behaves
   identically on AWS Nitro, AMD SEV-SNP, and Intel TDX. Trust is rooted in the
   CPU vendor, not a cloud provider.
2. **attested-workload** boots a workload inside a TEE and serves it over
   attested TLS, binding the TLS cert SPKI into the hardware quote.
3. **attestation-service** issues stage0 (build) and stage1 (runtime) EAT
   receipts and verifies whole chains against pinned vendor roots. Runs *inside*
   attested-workload.
4. **cvm-agent** gates secrets / tools / privilege on a verified receipt:
   *no proof, no privilege.*

### Repos

| Layer | Repo | Pages |
|---|---|---|
| agent platform | https://github.com/maceip/cvm-agent | https://maceip.github.io/cvm-agent/ |
| in-tee runtime | https://github.com/maceip/attested-workload | https://maceip.github.io/attested-workload/ |
| attestation service | https://github.com/maceip/attestation-service | https://maceip.github.io/attestation-service/ |
| quote format (base) | https://github.com/maceip/unified-quote | https://maceip.github.io/unified-quote/ |

### Local working tree

All four are cloned, isolated, under **`/Users/mac/tee-stack/`** (NOT the
phantom `qwen-webgpu-lora` workspace label). Everything pushes to
`github.com/maceip/*`.

---

## 2. North star (end-state)

All four layers deployed on **AWS** and **Azure** (and **GCP** later), each
**remotely attestable**, with **validated per-layer measurements reported on
each repo's Pages page**. A visitor should be able to open any layer's Pages
page and see live, independently re-verified attestation for that layer on each
cloud.

---

## 3. Naming (locked)

The product CLI must not be "runcard" anymore. Decided CLI names:

| Tool | Binary | Belongs to | Verbs |
|---|---|---|---|
| product runtime | **`cvm`** | cvm-agent | `cvm run`, `cvm verify`, `cvm gate` |
| quote verifier | **`uq`** | unified-quote | `uq build`, `uq verify`, `uq check`, `uq run`, `uq enclave` |
| workload runner | **`aw`** | attested-workload | `aw run`, `aw check` |
| attestation API | (service, no CLI) | attestation-service | HTTP `/v1/*` |

Legacy names being retired: **runcard** (old product), **bountynet / tenet**
(old genesis branding — keep out of READMEs and user-facing surfaces).

---

## 4. Current status (per project)

### unified-quote — base layer  ✅ solid
- Canonical crate: `v2/` (internal package `bountynet`, binary **`uq`**).
- Fixed cross-platform compile break (`sockaddr_vm.svm_family` u8/u16) so the
  verifier builds on non-Linux dev hosts.
- Renamed CLI `runcard` → `uq` across code, GitHub Action, deploy scripts,
  workflows, docs; dist artifact → `uq-linux-x86_64`; dropped off-layer product
  cards (badge/card SVG, `runcard.js`); `runcard.css` → `uq.css`.
- **Build + full test suite green** (26 unit + integration; the 2 SNP signature
  tests are `#[ignore]`d because they need live AMD KDS).
- Pinned vendor roots: `v2/src/quote/roots.rs`. Real captured quotes:
  `v2/testdata/chain/`. EAT schema: `v2/src/eat.rs`. Verify:
  `v2/src/quote/verify.rs`.
- Pages live: Zurich-style redesign + agentic-canon scroll (hover shimmer) +
  links to the other three.
- Open: dual-crate wart (top-level `bountynet-shim` vs `v2/`) — decide whether
  to delete the top-level shim or promote `v2/` to repo root for a clean
  `cargo build` at the top.

### attestation-service — service layer  ✅ working demo
- Rust + axum. Endpoints: `POST /v1/attest`, `POST /v1/verify`,
  `GET /v1/receipt/{id}`, `GET /v1/info`, `GET /healthz`.
- `stackcore/` vendors unified-quote's EAT + quote-verify core (byte-for-byte
  wire-compatible): `eat.rs`, `value_x.rs`, `quote/{mod,roots,verify}.rs`,
  `kds.rs`.
- Stage0 (build) / stage1 (runtime) EAT chaining via `x-previous-eat` header;
  `/v1/verify` walks the chain and asserts `value_x` stability.
- **Software-witness mode** for tee-less demos (honest, never faked);
  `AS_QUOTE_CMD` + `AS_PLATFORM` to collect real hardware quotes bound into
  `report_data`.
- Bundled real fixtures (tdx stage0→stage1, nitro, snp). **CI green**:
  `tests/verify_fixtures.rs` verifies tdx + nitro offline.
- `scripts/demo.sh`, `deploy/attested-workload.md`.
- Open: rename stray `runcard` strings (receipt/verify/eat) → cvm/uq/receipt;
  deeper per-cloud quote-source hooks.

### attested-workload — runtime layer  🚧
- In-TEE runtime: runs an http service inside a cloud TEE, serves over attested
  TLS, binds cert SPKI into the quote; one engine for nitro/snp/tdx. CLI `aw`.
- Pages live with canon scroll.
- Open: rename `runcard` refs (`src/main.rs`, `src/registry.rs`, docs) → `aw`;
  production-quality TEE paths.

### cvm-agent — product capstone  🚧
- Agent runtime that gates privilege on a verified receipt. CLI `cvm`.
- Currently still carries legacy content (llm* binaries, card-lab, registries)
  and **extensive `runcard` references** (~50 files) to be renamed → `cvm`.
- Pages live with canon scroll.
- Open: the big rename + trimming legacy surface to the cvm-agent story.

---

## 5. Per-layer measurement dashboard (design)

Each repo's Pages page gets a live measurements panel. Shared schema written by
a scheduled re-verification workflow to `docs/status/nodes.json`:

```json
{
  "updated_at": "2026-06-25T17:00:00Z",
  "layer": "unified-quote",
  "nodes": [
    {
      "cloud": "aws",
      "platform": "nitro",
      "endpoint": "https://...",
      "verdict": "verified",
      "measurement": { "PCR0": "..." },
      "pinned_root": "aws nitro root ca",
      "value_x": "….",
      "last_verified": "2026-06-25T16:59:12Z",
      "chain": ["stage0", "stage1"]
    }
  ]
}
```

Pages table columns (per layer): **cloud · platform · measurement · pinned root
→ verdict · value_x (short) · last-verified**. A scheduled GitHub Action runs
the verifier (`uq check` / service `/v1/verify`) against each registered node,
writes the JSON, and republishes — so the dashboard shows live re-verification,
not a static claim. unified-quote already has the skeleton
(`deploy/report_nodes.py`, `docs/live.html`, `report-live-quotes.yml`); extend
to all four with the per-layer schema.

---

## 6. Cloud deployment plan

**Approach: build-first.** Build deploy automation + dashboards now (zero cost);
provision real VMs only in a separate, explicitly-approved step.

### Per-cloud TEE feasibility

| Cloud | Platform | Instance (example) | Evidence path | Status / notes |
|---|---|---|---|---|
| AWS | Nitro Enclaves | m5.xlarge + enclave | `/dev/nsm` (NSM) | proven stage0; stage1 needs 2nd enclave |
| AWS | AMD SEV-SNP | c6a.xlarge | `/dev/sev-guest` / configfs-tsm | verifier proven; full sig needs AMD KDS |
| Azure | AMD SEV-SNP CVM | Standard_DCas_v5 | vTOM paravisor — **no raw `/dev/sev-guest`** | blocked: needs MAA / vTOM collector |
| GCP | Intel TDX | c3-standard-4 | configfs-tsm / `/dev/tdx-guest` | proven stage0→stage1 (later) |

**Azure caveat (known):** the DCas_v5 CVM provisions with AMD SEV memory
encryption but does not expose a raw SNP evidence device through its vTOM
paravisor. Plan: add an Azure MAA (Microsoft Azure Attestation) / vTOM collector
to unified-quote so Azure becomes *verified*, not just *tested*.

### Deploy sequencing (gated/paid)

1. **AWS** first (Nitro + SEV-SNP): provision, deploy all four layers, collect
   real quotes, remotely verify, publish measurements to each Pages page.
2. **Azure** (SNP CVM via MAA/vTOM collector once built).
3. **GCP** (TDX) later.

Provisioning is **not** started until explicitly approved (creds + budget +
region + leave-running policy). Default intent from prior discussion: leave VMs
running after verification for teardown later.

---

## 7. Pages design + agentic canon

- Shared visual style: Zurich-like type, dark "security + zappos" palette,
  demure enter/exit animations, elevation on cards.
- **Agentic canon scroll** (`docs/assets/canon-scroll.png`) on all four Pages
  and at the bottom of all four READMEs. On Pages it shimmers on hover: silver
  sheen across the text, rainbow glow around the edge, red glow on the wax seal.
- Canon principles (TEE-adapted): **no proof, no privilege.**
  1. make behavior enforceable — replace conventions with hardware quotes,
     attested gates, runtime checks.
  2. turn failures into evolution — each failed verification hardens the shared
     verifier, not just one deployment.
  3. compose through proofs — every layer declares what it accepts, returns, and
     can prove.
  4. carry trust forward — a proof from one stage becomes the ground the next
     stands on.

---

## 8. Roadmap (phases + checklist)

### Phase 0 — consolidation + base solidity  ✅ (mostly done)
- [x] Consolidate all four repos under `/Users/mac/tee-stack/`.
- [x] unified-quote: fix compile, rename `runcard`→`uq`, green tests, push.
- [x] attestation-service: real `/v1/attest` + `/v1/verify`, stage chaining,
      fixtures, CI green.

### Phase 1 — finish rename + dashboards (now, zero cost)
- [ ] cvm-agent: rename `runcard`→`cvm` (binary, docs, badges, workflows,
      registries); trim legacy surface to the cvm-agent story.
- [ ] attested-workload: rename `runcard`→`aw`.
- [ ] attestation-service: purge stray `runcard` strings.
- [ ] Per-layer measurement dashboard schema + Pages tables across all four.
- [ ] Deploy automation scripts per cloud (no provisioning yet).
- [ ] Redeploy all four Pages + READMEs after renames.

### Phase 2 — attested cloud deployments (gated/paid)
- [ ] AWS: deploy all four layers (Nitro + SEV-SNP), remote attest, publish
      measurements to Pages.
- [ ] Azure: MAA/vTOM collector in unified-quote, then deploy + attest.
- [ ] GCP: TDX deploy + attest.

---

## 9. Open decisions / risks

- **Dual crate in unified-quote** (`bountynet-shim` top-level vs `v2/`): pick one
  layout for a clean root `cargo build`.
- **Azure raw SNP evidence** via vTOM: requires MAA/vTOM collector work.
- **cvm-agent legacy surface**: how much llm*/card-lab content to keep vs trim
  for the interview story.
- **Cloud budget / credentials**: needed before Phase 2 provisioning.
