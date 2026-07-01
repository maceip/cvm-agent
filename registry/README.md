# Registry

A list of Value X entries that have been reviewed and approved by
maintainers. Each entry is a JSON file; each file has a detached
signature sidecar.

## What's in a registry entry

```json
{
  "value_x": "b1b09aae...",        // sha384 of runner source (48 bytes hex)
  "source_commit": "abc123...",    // git commit this was built from
  "platform_measurements": {        // optional: known-good platform hashes
    "nitro_pcr0": "...",
    "tdx_mrtd":   "...",
    "snp_measurement": "..."
  },
  "status": "recommended",          // recommended | deprecated | revoked
  "approved_at": "2026-04-13T00:00:00Z",
  "deprecated_at": null,            // when status changed to deprecated
  "revoked_at":    null,            // when status changed to revoked
  "notes": "free-form text"
}
```

Filename: `<value_x[0..16]>.json` — short prefix of Value X, for easy lookup.

## Signatures

Each entry has a detached signature sidecar: `<value_x[0..16]>.json.sig`.

Sidecar signatures are **verified offline** in `src/registry.rs`: an entry is
`Verified` only if some pinned trusted identity validates its detached
signature, otherwise `Untrusted` — there is no "present, so trust it" path. Both
raw-key (ed25519 / ECDSA P-256) and Sigstore-keyless signers are supported; for
keyless, the Fulcio leaf's SAN URI must glob-match the pinned subject and its
OIDC-issuer extension must equal the pinned issuer (cosign + Fulcio + GitHub
OIDC), so a maintainer never holds a key — the CI workflow identity is what
signs. An entry with no sidecar is `Missing` (informational only).

## Trust model

- **Who can add entries:** whoever controls the Sigstore signing identity
  (the GitHub workflow in `.github/workflows/registry-sign.yml`).
- **Who verifies:** anyone — verification is `cosign verify-blob` against
  the pinned identity.
- **What's in the trust chain:** Sigstore root → Fulcio CA → GitHub OIDC
  issuer → workflow path. All pinned in `src/registry.rs`.

## How it's used

1. `cvm build` in a TEE emits an attestation with a Value X.
2. Maintainer reviews, runs the `registry-sign` workflow, entry is committed.
3. `cvm check https://<domain>` fetches the attestation, verifies
   the TEE quote, looks up Value X in the registry, reports status:
   - `recommended` — green
   - `deprecated`  — yellow (grace period, client policy decides)
   - `revoked`     — red
   - `unknown`     — gray (not in registry at all)

Clients set their own acceptance policy. The registry is the source of
truth; what to do with it is local.

## eat-pass attester policy (VerificationPolicy)

The tool-gate / eat-pass attester uses operator JSON policy for mint authorization.
Keep registry entries and attester policy aligned:

| Registry field | eat-pass policy `allow[]` |
|----------------|---------------------------|
| `platform_measurements.snp_measurement` | `measurement` |
| `platform_measurements.tdx_mrtd` | `measurement` |
| `status` | `registry_status` |

Generate from registry:

```bash
./deploy/gen-eat-pass-policy.sh --registry registry/<prefix>.json --out policy.json
```

Trust-boundary notes (Galanou CVM SoK, Rezabek Proof of Cloud): document in
`notes` that MAA verifies quotes, launch measurement is the policy key, and
quotes do not prove physical location. See `deploy/STACK-POLICY.md`.
