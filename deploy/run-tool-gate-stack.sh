#!/usr/bin/env bash
# Start the full attested agent tool-gate stack:
#   eat-pass attester → issuer → redeemer → tool-gate (SMTP/IMAP secrets here)
#
# Usage:
#   cp deploy/tool-gate.env.example deploy/tool-gate.env
#   # edit deploy/tool-gate.env
#   ./deploy/run-tool-gate-stack.sh
#
# Agent (inside attested CVM):
#   cvm tool send-email --to ryan --subject "Hi" --body "…" \
#     --kt-log-pub "$KT_LOG_PUB" --gate http://127.0.0.1:8787 \
#     --issuer http://127.0.0.1:8088 --attester http://127.0.0.1:8087

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${ENV_FILE:-$ROOT/deploy/tool-gate.env}"

if [[ ! -f "$ENV_FILE" ]]; then
  echo "missing $ENV_FILE — copy deploy/tool-gate.env.example and edit" >&2
  exit 1
fi

# shellcheck disable=SC1090
source "$ENV_FILE"

EAT_PASS_BIN="${EAT_PASS_BIN:-$(command -v eat-pass 2>/dev/null || echo "$ROOT/../eat-pass/target/release/eat-pass")}"
TOOL_GATE_BIN="${TOOL_GATE_BIN:-$(command -v tool-gate 2>/dev/null || echo "$ROOT/target/release/tool-gate")}"
GEN_POLICY="$ROOT/deploy/gen-eat-pass-policy.sh"

for var in EATPASS_ATTESTER_SEED EATPASS_KT_SEED ATTESTER_PUB KT_LOG_PUB \
  TOOL_GATE_SMTP_HOST TOOL_GATE_SMTP_USER TOOL_GATE_IMAP_PASSWORD TOOL_GATE_FROM; do
  if [[ -z "${!var:-}" ]]; then
    echo "set $var in $ENV_FILE" >&2
    exit 1
  fi
done

POLICY_FILE="${POLICY_FILE:-}"
if [[ -z "$POLICY_FILE" ]]; then
  if [[ -z "${ALLOW_VALUE_X:-}" && -z "${REGISTRY_ENTRY:-}" ]]; then
    echo "set POLICY_FILE, or ALLOW_VALUE_X (SNP launch measurement hex), or REGISTRY_ENTRY in $ENV_FILE" >&2
    exit 1
  fi
  POLICY_FILE="${POLICY_DIR:-$ROOT/deploy/policies}/.generated-tool-gate.json"
  if [[ -n "${REGISTRY_ENTRY:-}" ]]; then
    "$GEN_POLICY" --registry "$REGISTRY_ENTRY" --gate "${ATTESTER_GATE:-azure}" \
      --id "${POLICY_ID:-tool-gate-local}" --out "$POLICY_FILE"
  else
    "$GEN_POLICY" --measurement "$ALLOW_VALUE_X" --gate "${ATTESTER_GATE:-azure}" \
      --id "${POLICY_ID:-tool-gate-local}" --out "$POLICY_FILE"
  fi
fi

if [[ ! -f "$POLICY_FILE" ]]; then
  echo "policy file not found: $POLICY_FILE" >&2
  exit 1
fi

if [[ -x "$EAT_PASS_BIN" ]]; then
  "$EAT_PASS_BIN" policy validate --file "$POLICY_FILE"
else
  echo "warning: eat-pass not built; skipping policy validate" >&2
fi

LISTEN_ATTESTER="${LISTEN_ATTESTER:-127.0.0.1:8087}"
LISTEN_ISSUER="${LISTEN_ISSUER:-127.0.0.1:8088}"
LISTEN_REDEEMER="${LISTEN_REDEEMER:-127.0.0.1:8100}"
LISTEN_TOOL_GATE="${LISTEN_TOOL_GATE:-127.0.0.1:8787}"
TOOL_GATE_SMTP_PORT="${TOOL_GATE_SMTP_PORT:-587}"
export TOOL_GATE_SMTP_PORT

TLS_ARGS=()
GATE_TLS_ARGS=()
if [[ -n "${TLS_CERT:-}" && -n "${TLS_KEY:-}" ]]; then
  TLS_ARGS=(--tls-cert "$TLS_CERT" --tls-key "$TLS_KEY")
  GATE_TLS_ARGS=(--tls-cert "$TLS_CERT" --tls-key "$TLS_KEY")
  GATE_SCHEME=https
else
  TLS_ARGS=(--insecure-http)
  GATE_SCHEME=http
  echo "no TLS_CERT/TLS_KEY — starting with --insecure-http (loopback only)"
fi

export EATPASS_ATTESTER_SEED EATPASS_KT_SEED
export EATPASS_POLICY_TRUSTED_PUB="${EATPASS_POLICY_TRUSTED_PUB:-}"
export TOOL_GATE_SMTP_HOST TOOL_GATE_SMTP_USER TOOL_GATE_IMAP_PASSWORD TOOL_GATE_FROM
export TOOL_GATE_CONTACT_RYAN TOOL_GATE_ALLOWED_TO TOOL_GATE_DRY_RUN

ISSUER_URL="${GATE_SCHEME}://$LISTEN_ISSUER"
REDEEMER_URL="${GATE_SCHEME}://$LISTEN_REDEEMER"
ATTESTER_URL="${GATE_SCHEME}://$LISTEN_ATTESTER"
GATE_URL="${GATE_SCHEME}://$LISTEN_TOOL_GATE"

PIDS=()
cleanup() {
  for pid in "${PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
  done
}
trap cleanup EXIT

echo "starting eat-pass attester on $LISTEN_ATTESTER (policy=$POLICY_FILE)"
"$EAT_PASS_BIN" attester --gate "${ATTESTER_GATE:-azure}" --policy "$POLICY_FILE" \
  --listen "$LISTEN_ATTESTER" "${TLS_ARGS[@]}" &
PIDS+=($!)

echo "starting eat-pass issuer on $LISTEN_ISSUER"
"$EAT_PASS_BIN" issuer --attester-pub "$ATTESTER_PUB" \
  --listen "$LISTEN_ISSUER" "${TLS_ARGS[@]}" &
PIDS+=($!)

echo "starting eat-pass redeemer on $LISTEN_REDEEMER"
"$EAT_PASS_BIN" redeem --listen "$LISTEN_REDEEMER" "${TLS_ARGS[@]}" &
PIDS+=($!)

sleep 1

echo "starting tool-gate on $LISTEN_TOOL_GATE"
GATE_CMD=(
  "$TOOL_GATE_BIN"
  --listen "$LISTEN_TOOL_GATE"
  --issuer "$ISSUER_URL"
  --redeemer "$REDEEMER_URL"
  --kt-log-pub "$KT_LOG_PUB"
  --attester "$ATTESTER_URL"
)
if ((${#GATE_TLS_ARGS[@]})); then
  GATE_CMD+=("${GATE_TLS_ARGS[@]}")
fi
"${GATE_CMD[@]}" &
PIDS+=($!)

echo
echo "stack up. demo agent command:"
echo "  cvm tool send-email --to ryan --subject \"Proof before privilege\" \\"
echo "    --body \"Sent through eat-pass gated mailbox\" \\"
echo "    --kt-log-pub $KT_LOG_PUB \\"
echo "    --gate $GATE_URL \\"
echo "    --issuer $ISSUER_URL --attester $ATTESTER_URL"
echo
echo "landing page: $GATE_URL/"
echo "press Ctrl+C to stop"

wait
