# Trust Bootstrap

The network address is not the trust root.

For a self-hosted GitHub runner or a public attestation endpoint, IP, DNS, and
the GitHub runner name only get traffic to a candidate machine. The machine is
accepted only after it answers a verifier challenge with evidence rooted in TEE
hardware.

## What "the right machine" means

Runcard should not mean "we reached the IP we expected." It should mean:

1. The endpoint returned a fresh quote containing the verifier's nonce.
2. The hardware quote verifies against the vendor root for TDX, SEV-SNP, or
   Nitro.
3. The quote binds the runner signing key and Value X in report data.
4. The Value X matches the expected runner or workload identity.
5. The platform measurement matches a known-good TEE image measurement.

If any of those checks is missing, the result is routing, not identity.

## GitHub Actions runner path

GitHub does not dial the GCP VM by public IP. The runner starts, requests a
GitHub registration token, and opens an outbound runner session to GitHub. Jobs
select it by labels such as:

```yaml
runs-on: [self-hosted, tdx, bountynet]
```

That label match is an availability and scheduling mechanism. It is not a trust
decision. A workflow must treat the runner as untrusted until the job produces
and verifies a hardware-rooted Runcard receipt.

## Public attestation endpoint path

The GCP deploy helper exposes the legacy runner attestation endpoint on
`:9384`. The deploy script now reserves a regional static external IP so the
operator has a stable routing target, but the IP still does not prove identity.

Verify the endpoint with a fresh challenge:

```bash
cargo run --bin verify-runner -- http://<ip-or-dns>:9384 \
  --expected-platform Tdx \
  --expected-value-x <48-byte-hex> \
  --expect-measurement MRTD=<tdx-mrtd-hex>
```

The verifier sends a 32-byte nonce to `/attest/full`. The quote is accepted only
if the returned nonce matches, the UnifiedQuote signature verifies, the embedded
hardware quote verifies, and any expected Value X or measurement constraints
match.

## Attested TLS runtime path

For the v2 runtime flow, use:

```bash
runcard check https://<domain>/
```

The TLS certificate is intentionally authenticated by attestation instead of a
normal CA chain. `runcard check` extracts the EAT receipt from the certificate,
verifies the hardware quote, checks the report-data binding, and verifies that
the certificate SPKI hash is the same key the TEE committed to. That is what
prevents a relay or man-in-the-middle from turning "attestation over TLS" into a
false proof.

## Policy

Do not accept a GitHub runner status, a DNS name, a static IP, or an HTTP 200 as
proof. They are useful only after a verifier has checked the current quote
against the expected trust policy.
