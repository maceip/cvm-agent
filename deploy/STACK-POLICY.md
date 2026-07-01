# TEE-stack policy map

Who enforces what across repositories (RFC 9334 / 9711 roles + literature pull-ins).

## Layers

| Layer | Repo | Enforces |
|-------|------|----------|
| Evidence | `unified-quote`, `cvm-agent`, `attestation-service` | Hardware-rooted EAT / quote |
| Crypto verify | `eat-pass-gate`, `unified-quote` verify | Signature chain + channel binding |
| Appraisal policy | `eat-pass-policy` on **attester only** | Allow/deny after crypto verify |
| Authorization token | `eat-pass` issuer | PoMFRIT blind-sign after FAEST-128f attester auth |
| RP / tool gate | `eat-pass origin`, `cvm-agent tool-gate` | Token + challenge + central redeem |

**Measurement is not in the PrivateToken.** The attester checks policy once at
`/authorize`; the token proves attested mint eligibility, not which build.

## Operator surfaces

1. **cvm-agent `registry/*.json`** — Value-X approval (Sigstore sidecar). Used by `cvm check`.
2. **eat-pass `VerificationPolicy` JSON** — attester allow list. Generate from registry:

   ```bash
   ./deploy/gen-eat-pass-policy.sh --registry registry/<prefix>.json --out policy.json
   ```

3. **Signed policy sidecar** (CoRIM-style, Ferro ITASEC 2024): `policy.json.sig` +
   `EATPASS_POLICY_TRUSTED_PUB` on attester.

## Deploy wiring

| Script | Attester flags |
|--------|----------------|
| `deploy/run-tool-gate-stack.sh` | `--policy "$POLICY_FILE"` |
| `deploy/install-uqaz1.sh` | `--policy "$STACK/attester-policy.json"` |

Env: see `deploy/tool-gate.env.example` (`POLICY_FILE`, `ALLOW_VALUE_X`, or `REGISTRY_ENTRY`).

## Challenge freshness (Hanff CCS 2025)

| Service | Behavior |
|---------|----------|
| `eat-pass origin` | Fresh 32-byte `redemption_context` per 401 |
| `cvm-agent tool-gate` | `GET /.well-known/eat-pass/challenge?to=&subject=&body=` before mint |
| `cvm tool send-email` | Fetches gate challenge, then mints |

## EAR-shaped results (Fossati draft-ietf-rats-ear)

Attester `POST /authorize` returns `appraisal`. `cvm tool` logs `policy_id` and
`class_label` after mint.

## dev-sim vs production

| Mode | Policy |
|------|--------|
| Shipped `eat-pass` binary | Real gate + `--policy` required |
| `cargo test --features dev-sim` | Inline test allow list (tests only) |

There is no production flag to skip measurement checks.
