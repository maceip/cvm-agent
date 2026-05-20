# Core Invariant

Everything in this repo exists to solve one problem:

**A verifier can confirm that a specific set of software files is running
inside genuine TEE hardware, without trusting the operator.**

This breaks down into exactly three checks. If any one fails, the
attestation is worthless:

1. **Platform measurement matches expected value.**
   MRTD (TDX), MEASUREMENT (SNP), or PCR0 (Nitro) from the hardware
   quote matches a known-good value. This proves the boot image is
   genuine. The shim is part of the boot image — the CPU measured it.

2. **Value X matches expected value.**
   sha384 of the runner files (NOT the shim — that's in #1) matches
   a known-good value. This proves the application payload is genuine.
   Value X is computed by the shim, which is trusted because of #1.

3. **The pubkey in the quote was generated inside the TEE.**
   report_data in the hardware quote contains a hash that binds the
   signing key to this specific TEE instance. This proves the
   UnifiedQuote was produced by the attested environment, not a proxy.

If you are adding code and it does not directly serve one of these
three checks, stop and ask why.

## What is NOT the core problem

- Token formats (EAT, JWT, CBOR) — these are transport, not trust
- On-chain contracts — these are storage, not verification
- HTTP endpoints — these are delivery, not proof
- Ecosystem compatibility — this is adoption, not security
- Update governance — this is lifecycle, not attestation

These things matter but they are secondary. Do not build them until
the three checks above work end-to-end with no gaps.

**Status (April 2026):** All three checks work end-to-end on Nitro,
SNP, and TDX. Binding hash verified, TEE signature chain verified
to pinned vendor root CAs, platform measurements extracted and
compared. The secondary concerns are now appropriate to build.
