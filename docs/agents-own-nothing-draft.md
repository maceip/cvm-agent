# Agents should own nothing: a CROPS argument against ERC-8004's Identity Registry

*Draft. Not a new standard. This keeps ERC-8004's Validation Registry and builds on
delegation work already shipping (HDP, UCAN). It is one claim: trust should attach to
the human and to the code, never to the agent — so agents should not be given identity.
Agents are knives, not citizens.*

## Judge it by CROPS

Vitalik just defined the EF's mandate as **CROPS** — Censorship resistance, Resistance
to capture, Openness, Privacy, Security — and called maximizing throughput "a route to
mediocrity." Fine. Let's hold agent-trust designs to that same yardstick, because one of
them fails it.

## The Identity Registry fails CROPS

ERC-8004's Identity Registry mints a **transferable ERC-721 NFT per agent**:
`register(agentURI) → agentId`, "browsable and transferable with NFT-compliant apps,"
ownership via standard ERC-721. The Reputation Registry then keys feedback to that same
`agentId`.

Put those together and an agent identity is a **transferable, reputation-bearing asset**.
Transfer the token, the accumulated reputation goes with it. That is a tradeable
reputation market — and you don't need a TEE or any encumbrance trick to lease an
identity, just an NFT sale. This is the on-chain vote-buying / credential-lease problem
(Dark DAOs; TEEvil) that Complete Knowledge formalized, except 8004 makes it native and
frictionless.

By the CROPS yardstick the EF just published, that's a direct hit on **Resistance to
capture** (reputation becomes purchasable) and **Openness** (standing becomes an owned,
gated asset). The Identity Registry is the wrong primitive — not because the team is
wrong about everything, but because a transferable identity object is anti-CROPS by
construction.

## Trust already lives somewhere else — the field is showing it

An agent already has identity on Ethereum: its key. It can sign. A second, transferable
identity layer on top is both redundant and the footgun. The two things you actually
need:

- **Identity is the human's.** The agent carries a scoped, revocable capability derived
  from the human's existing credential. This is what the delegation work is already
  building — HDP (now an IETF draft), UCAN, OAuth Token Exchange. Don't issue the agent
  anything.
- **Reputation is the code's.** Address it by code measurement, not by an agent token.
  You can't Sybil a measurement — and a clone has the identical measurement, so it
  correctly shares the history. Cloning becomes a no-op, which is the point.

The agent in the middle owns nothing. It is a disposable hand that leaves a record:
*code measurement M, acting under delegation D from human H, did A at T.* That record is
content-addressed, appended to H's log and M's log, and discarded.

This is not a lone opinion; it's where the whole stack is already heading. In the last
90 days: delegation chains without registries (HDP), Merkle-logged agent execution
history (Right to History), receipts instead of binary verification (Tool Receipts, Not
Zero-Knowledge Proofs), and WebPKI itself replacing per-certificate identity with
Merkle-logged history (IETF PLANTS / Merkle Tree Certificates). Even Lean Ethereum is
Merkle/SNARK/post-quantum. Everyone is building **logged history plus delegation**. The
transferable agent identity is the one piece swimming upstream.

## The shortcut (the part that isn't just talk)

If you actually believe agents own nothing, you get to *delete* an entire subsystem
everyone else is building: no agent enrollment, no agent keypair, no agent
namespace/directory, no agent reputation registry, no credential lifecycle or revocation
list. There is nothing to revoke because the agent holds nothing; you revoke the human's
delegation or rotate the code, both of which already exist.

We built it this way (runcards). You point it at the credentials you already have plus
the code you're running, and leaves come out. No agent was registered. There was never
an agent to register. The adoption-friction delta is the whole argument: we are not
faster at agent identity, we removed it.

## What we are not claiming

Attestation is not magic, and we won't pretend it is. A hardware quote proves reviewed
code ran in real hardware — not that the code is honest, not that it wasn't hijacked at
runtime, not that there is no hidden second path. So the record is **tiered**: each
verification method is labeled by what it still cannot prove (this is the "trust signals,
not binary verified/unverified" point from Tool Receipts). TEE.Fail (2026) is a
physical-access attack — out of scope unless your threat model includes a malicious
operator. State the scope; don't oversell it.

## The ask

Keep the Validation Registry — it's the right idea, and it's where attestation belongs.
Reconsider the **Identity Registry as a transferable, reputation-bearing object**: by
CROPS, that's a capture surface. Treat agent "identity" as an ephemeral (human × code)
tuple, recorded as history, not as an owned NFT.

We have a running implementation and no token, no raise, nothing to sell — which is the
only reason we can comfortably say the unprofitable thing: stop minting agent identities.
Corrections welcome.

---

### References
- CROPS / EF mandate (Buterin, May 2026): coindesk.com/web3/2026/05/25/buterin-says-ethereum-foundation-will-shrink-sell-less-eth-and-focus-on-crops
- ERC-8004 spec (Identity/Reputation/Validation registries): eips.ethereum.org/EIPS/eip-8004
- Complete Knowledge (encumbrance, TEEvil): eprint.iacr.org/2023/044
- HDP — Human Delegation Provenance: arxiv.org/abs/2604.04522
- Right to History (Merkle-logged agent execution): arxiv.org/abs/2602.20214
- Tool Receipts, Not Zero-Knowledge Proofs: arxiv.org/abs/2603.10060
- Merkle Tree Certificates (IETF PLANTS): datatracker.ietf.org/doc/draft-ietf-plants-merkle-tree-certs/
