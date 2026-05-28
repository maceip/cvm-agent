# Attestation is not pass/fail: honest trust tiers for ERC-8004's Validation Registry

*Draft. Not a new standard. This builds on ERC-8004 and on work already shipped by
Phala, Automata, and Oasis. It is a way to talk about what attestation does and
does not prove, written so the boundaries are honest.*

## The problem with "verified"

Most attestation gets used as a single bit: verified or not. In practice that bit
hides the part that matters. A TEE attestation proves something narrow — that a
specific piece of code ran inside genuine hardware. It does not prove the code is
honest, that it was not fed malicious input, or that the operator has no other way
to act. Collapsing all of that into "verified" is how you end up trusting the word
instead of the proof.

## A tier is a map of what you cannot prove

ERC-8004's Validation Registry already lists reputation, crypto-economic
re-execution, ZK, and TEE attestation, and frames them as "security proportional to
value at risk." That framing is right. The missing piece is honesty about the
boundary of each one:

- **Reputation** proves past behavior. It cold-starts at zero and is gameable.
- **Crypto-economic** proves stake is at risk. It says nothing about correctness if
  the verifier itself is wrong.
- **TEE attestation** proves reviewed code ran in real hardware. It does not prove
  the code was honest or uncompromised at runtime.
- **ZK** proves a statement is true. It is expensive and you have to have written
  the circuit for exactly the claim you care about.

We are not proposing new tiers. We are proposing honest labels on the ones that
already exist: for each, state what it still fails to prove.

## Containment, not honesty

A concrete case from running attested LLM workflows (e.g. a competition that meters
token use): you cannot prove a participant did not also ask a model you do not
control. That is unprovable. What you *can* prove is that their work never left the
attested path, and that leaving it drops them to a lower tier.

So the honest top tier is not "trustless." It is "contained, with the scope stated."
You convert an unprovable claim (did they cheat) into a provable one (did they stay
inside the envelope). Say what the envelope covers and what it does not.

## Someone has to require the tier

A tier only means something if the relying party requires it for the stakes. SPIFFE
/ SPIRE showed the pricing that actually works: attestation gates access. A KMS or
gateway releases the production credential only to a sufficient tier. That is proof
before privilege — tiers priced in what they unlock, not in dollars. If nobody
requires a tier, everyone self-reports at the floor and the hierarchy collapses to
nothing.

## The honest limits

- **TEE.Fail (2025)** showed physical extraction of attestation keys from DDR5. If
  your threat model excludes datacenter physical access, this is out of scope; if it
  includes a malicious operator, it is in scope. State which — do not wave it away.
- **Attestation proves reviewed code ran, not that it was not hijacked at runtime.**
  Build attestation and runtime memory snapshots do not catch prompt injection. That
  is a different layer. Name it; do not claim it.
- **The composite-prover problem.** If the prover has a hidden second path — another
  model, another device, a second key — attestation of one path proves nothing about
  the whole. You can prove what is revealed, not the absence of what is hidden.

## The footgun: containment has an abuse mirror

The rigorous version of all of this is **Complete Knowledge** (Kelkar, Babel, Daian,
Austgen, Buterin, Juels, CCS 2024). It shows that standard attestation cannot tell
whether a party freely controls a secret or has *encumbered* it under someone else's
policy. The same "I can prove my execution is bound to a policy" that gives you a
clean top tier is what enables vote-buying (Dark DAOs) and credential lease (TEEvil).
A tier model has to be honest that the strong tier is also a dangerous capability.

Their CK Registry is prior art for the exact shape proposed here: a registry of
multiple verification methods, with revocation, and explicit strong vs. lightweight
tiers. The point is to build on it, not reinvent it.

## Why bother now

A narrow bet, not a manifesto. Attestation has only ever been adopted at scale in one
place: machine identity (SPIFFE/SPIRE), because that is where you have an ephemeral,
zero-history entity that needs authority on first contact. Agents are the same shape,
one layer up. When agent-to-agent commerce gets real, the first-contact trust
decision is where attestation beats reputation (which starts at zero) and staking
(which is capital-heavy). That day is not here. The point of writing this down now is
that the honest tier framing is what it will need when it arrives.

## What this is grounded in

The runtime side of this is real, not hypothetical: an attested build (source and
artifact bound into a hardware quote) chained to an attested runtime, proven on Intel
TDX, AMD SEV-SNP, and AWS Nitro. The contribution here is not that engine — it is the
honest framing around what such proofs are worth at each tier, and where they stop.

Corrections welcome. The goal is to get the boundaries right, not to claim more than
attestation delivers.
