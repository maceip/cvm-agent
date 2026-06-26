# cvm-agent

proof before privilege for agents in confidential vms.

an agent runtime that runs inside a confidential vm and gates secrets, tokens,
and tool access on a verified hardware receipt. before an app gives an agent,
mcp server, deploy job, or service real power, it can ask one question:

> is this live thing really the reviewed source it claims to be?

no proof, no privilege. `cvm` is the developer-facing product surface.

## verify before release

```ts
const verdict = await cvm.verify("https://agent.example.com", {
  source: "github.com/acme/support-flow",
  policy: "reviewed-main-only",
});
if (verdict.ok) await secrets.release("PROD_SUPPORT_TOKEN");
```

## where it fits

- agentic workflows: prove the workflow before it gets a github, linear, stripe, or cloud token.
- mcp tool servers: require a receipt before model-controlled shell, db, browser, or filesystem tools.
- secret release: kms/vault release material only to a verified runtime.
- deploy gates: attestation as a pass/fail check in a pr or release job.

## the stack

- agent platform — **cvm-agent** (here)
- attestation service — [attestation-service](https://github.com/maceip/attestation-service)
- quote format — [unified-quote](https://github.com/maceip/unified-quote)
- in-tee runtime — [attested-workload](https://github.com/maceip/attested-workload)

each lower layer stands on its own; cvm-agent pulls them in to show the mesh.

## the mesh

cvm-agent depends on the base layer for real — `unified-quote` is a cargo
dependency, not a vendored copy:

```toml
unified-quote = { git = "https://github.com/maceip/unified-quote", package = "unified-quote" }
```

`examples/mesh.rs` uses that crate's verifier to check a real, hardware-rooted
receipt — the same format attestation-service issues and attested-workload
serves over attested tls:

```
$ cargo run --example mesh
cvm-agent → unified-quote mesh check
  receipt : examples/fixtures/tdx_stage1.cbor
  format  : eat v2 · 2 stage(s)
  [stage1 (runtime)] tdx → VERIFIED against pinned tdx vendor root
  [stage0 (build)]   tdx → VERIFIED against pinned tdx vendor root
  value_x stable across chain: true
  verdict : VERIFIED — privilege may be released
```

the receipts are real captured quotes (tdx build→runtime chain, aws nitro),
verified offline against pinned vendor roots. the verifier is the base layer's
code — no fork, no copy.

the mesh is live across three clouds-rooted nodes, each remotely re-verifiable
with no tee of your own: **aws sev-snp** (milan, vlek → ark-milan), **aws nitro**
(enclave pcr0 → aws root), and **azure sev-snp** (vTPM-exposed snp report →
ark-milan, no MAA in the trust path). status + verification commands:
https://maceip.github.io/unified-quote/live.html

pages: https://maceip.github.io/cvm-agent/

<!-- agentic-canon -->
## agentic canon

<table>
<tr>
<td width="200" valign="top"><img src="docs/assets/canon-scroll.png" width="180" alt="agentic canon" /></td>
<td valign="top">

**no proof, no privilege.**

1. **make behavior enforceable.** replace conventions with hardware quotes, attested gates, and runtime checks.
2. **turn failures into evolution.** each failed verification hardens the shared verifier, not just one deployment.
3. **compose through proofs.** every layer declares what it accepts, returns, and can prove.
4. **carry trust forward.** a proof from one stage becomes the ground the next stands on.

</td>
</tr>
</table>
