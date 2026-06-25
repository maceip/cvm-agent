# cvm-agent

proof before privilege for agents in confidential vms.

an agent runtime that runs inside a confidential vm and gates secrets, tokens,
and tool access on a verified hardware receipt. before an app gives an agent,
mcp server, deploy job, or service real power, it can ask one question:

> is this live thing really the reviewed source it claims to be?

no proof, no privilege. **runcard** is the developer-facing product surface.

## verify before release

```ts
const verdict = await runcard.verify("https://agent.example.com", {
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
