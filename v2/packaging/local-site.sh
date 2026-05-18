#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
V2_DIR="${ROOT_DIR}/v2"

MODE="${LLMATTEST_SITE_MODE:-auto}"
LISTEN="${LLMATTEST_SITE_LISTEN:-127.0.0.1:8080}"
PUBLIC_BASE_URL="${LLMATTEST_SITE_PUBLIC_BASE_URL:-http://${LISTEN}}"
GATEWAY_BASE_URL="${LLMATTEST_SITE_GATEWAY_BASE_URL:-http://127.0.0.1:8088}"
STATE_DIR="${LLMATTEST_SITE_STATE_DIR:-${ROOT_DIR}/.local/llm-site}"
CREATE_DEMO=1
OPEN_BROWSER=0
EXTRA_ARGS=()

usage() {
  cat <<'USAGE'
Usage: v2/packaging/local-site.sh [options] [-- extra llm-event-service args]

Runs the hosted LLM Attested event website locally.

Options:
  --mode auto|plain|tee      auto uses TEE mode when a quote device is visible
  --listen HOST:PORT         default: 127.0.0.1:8080
  --public-base-url URL      default: http://HOST:PORT
  --gateway-base-url URL     default: http://127.0.0.1:8088
  --state-dir PATH           default: .local/llm-site
  --no-demo                  do not create a demo event/team
  --open                     open the demo runcard or home page in the OS browser
  -h, --help                 show this help

Examples:
  v2/packaging/local-site.sh
  v2/packaging/local-site.sh --mode plain --listen 127.0.0.1:8099
  v2/packaging/local-site.sh --mode tee --public-base-url https://events.example
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)
      MODE="${2:?--mode requires auto, plain, or tee}"
      shift 2
      ;;
    --listen)
      LISTEN="${2:?--listen requires HOST:PORT}"
      shift 2
      ;;
    --public-base-url)
      PUBLIC_BASE_URL="${2:?--public-base-url requires URL}"
      shift 2
      ;;
    --gateway-base-url)
      GATEWAY_BASE_URL="${2:?--gateway-base-url requires URL}"
      shift 2
      ;;
    --state-dir)
      STATE_DIR="${2:?--state-dir requires PATH}"
      shift 2
      ;;
    --no-demo)
      CREATE_DEMO=0
      shift
      ;;
    --open)
      OPEN_BROWSER=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      EXTRA_ARGS+=("$@")
      break
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "${MODE}" in
  auto|plain|tee) ;;
  *)
    echo "--mode must be auto, plain, or tee" >&2
    exit 2
    ;;
esac

mkdir -p "${STATE_DIR}/event-logs" "${STATE_DIR}/attest-out"

STORE_PATH="${STATE_DIR}/store.json"
RATE_STATE_PATH="${STATE_DIR}/rate-state.json"
EVENT_LOG_DIR="${STATE_DIR}/event-logs"
SITE_MANIFEST="${STATE_DIR}/local-site.json"

health_base_url() {
  case "${LISTEN}" in
    0.0.0.0:*) printf 'http://127.0.0.1:%s' "${LISTEN##*:}" ;;
    \[::\]:*) printf 'http://127.0.0.1:%s' "${LISTEN##*:}" ;;
    *) printf 'http://%s' "${LISTEN}" ;;
  esac
}

tee_device_visible() {
  [[ -e /dev/tdx_guest ]] && return 0
  [[ -e /dev/tdx-guest ]] && return 0
  [[ -e /dev/sev-guest ]] && return 0
  [[ -e /dev/nsm ]] && return 0
  return 1
}

if [[ "${MODE}" == "auto" ]]; then
  if tee_device_visible; then
    MODE="tee"
  else
    MODE="plain"
  fi
fi

echo "[local-site] building v2 binaries"
cargo build --manifest-path "${V2_DIR}/Cargo.toml" --bins

EVENT_BIN="${V2_DIR}/target/debug/llm-event-service"
RUNCARD_BIN="${V2_DIR}/target/debug/runcard"
HEALTH_BASE_URL="$(health_base_url)"

SERVICE_ARGS=(
  --listen "${LISTEN}"
  --public-base-url "${PUBLIC_BASE_URL}"
  --gateway-base-url "${GATEWAY_BASE_URL}"
  --issuer "${PUBLIC_BASE_URL}"
  --store "${STORE_PATH}"
  --event-log-dir "${EVENT_LOG_DIR}"
  --rate-state "${RATE_STATE_PATH}"
  --public-requests-per-minute 10000
  --mutation-requests-per-minute 10000
  --ingest-requests-per-minute 10000
)
if [[ ${#EXTRA_ARGS[@]} -gt 0 ]]; then
  SERVICE_ARGS+=("${EXTRA_ARGS[@]}")
fi

quote_cmd() {
  printf '%q ' "$@"
}

privileged_env() {
  if [[ "${EUID}" -eq 0 ]]; then
    env "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo env "$@"
  else
    echo "[local-site] TEE mode needs root access to quote devices and port 443, but sudo is not installed" >&2
    exit 1
  fi
}

SERVICE_PID=""
cleanup() {
  if [[ -n "${SERVICE_PID}" ]]; then
    kill "${SERVICE_PID}" >/dev/null 2>&1 || true
    wait "${SERVICE_PID}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

start_plain() {
  echo "[local-site] starting plain local website at ${PUBLIC_BASE_URL}"
  "${EVENT_BIN}" "${SERVICE_ARGS[@]}" &
  SERVICE_PID="$!"
}

start_tee() {
  echo "[local-site] starting TEE-backed website at ${PUBLIC_BASE_URL}"
  privileged_env "PATH=${PATH}" \
    "RUSTUP_HOME=${RUSTUP_HOME:-${HOME}/.rustup}" \
    "CARGO_HOME=${CARGO_HOME:-${HOME}/.cargo}" \
    "${RUNCARD_BIN}" build "${V2_DIR}" \
    --cmd "cargo build --bins" \
    --output "${STATE_DIR}/attest-out"

  local workload
  workload="$(quote_cmd "${EVENT_BIN}" "${SERVICE_ARGS[@]}")"
  privileged_env "PATH=${PATH}" \
    "RUSTUP_HOME=${RUSTUP_HOME:-${HOME}/.rustup}" \
    "CARGO_HOME=${CARGO_HOME:-${HOME}/.cargo}" \
    "${RUNCARD_BIN}" run "${V2_DIR}" \
    --attestation "${STATE_DIR}/attest-out/attestation.cbor" \
    --cmd "${workload}" &
  SERVICE_PID="$!"
}

case "${MODE}" in
  plain) start_plain ;;
  tee) start_tee ;;
esac

for _ in $(seq 1 200); do
  if curl -fsS "${HEALTH_BASE_URL}/healthz" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "${SERVICE_PID}" >/dev/null 2>&1; then
    echo "[local-site] service exited before health check passed" >&2
    wait "${SERVICE_PID}"
  fi
  sleep 0.1
done

curl -fsS "${HEALTH_BASE_URL}/healthz" >/dev/null

HOME_URL="${PUBLIC_BASE_URL}"
JOIN_URL=""
DASHBOARD_URL=""
RUNCARD_URL=""

if [[ "${CREATE_DEMO}" == "1" ]]; then
  EVENT_JSON="$(curl -fsS -X POST "${HEALTH_BASE_URL}/events" \
    -H 'content-type: application/json' \
    --data '{"name":"Local LLM Attested Demo","event_type":"universal"}')"
  EVENT_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["event_id"])' "${EVENT_JSON}")"
  JOIN_URL="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["join_url"])' "${EVENT_JSON}")"
  DASHBOARD_URL="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["dashboard_url"])' "${EVENT_JSON}")"
  TEAM_JSON="$(curl -fsS -X POST "${HEALTH_BASE_URL}/e/${EVENT_ID}/teams" \
    -H 'content-type: application/json' \
    --data '{"team_name":"Local Solo Team"}')"
  RUNCARD_URL="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["runcard_url"])' "${TEAM_JSON}")"
  printf '%s\n' "${TEAM_JSON}" > "${STATE_DIR}/team.json"
fi

python3 - <<PY > "${SITE_MANIFEST}"
import json
manifest = {
    "mode": "${MODE}",
    "home_url": "${HOME_URL}",
    "health_url": "${HEALTH_BASE_URL}/healthz",
    "self_host_url": "${PUBLIC_BASE_URL}/.well-known/llm-attested/self-host.json",
    "registry_url": "${PUBLIC_BASE_URL}/.well-known/runcard/registry.json",
    "join_url": "${JOIN_URL}",
    "dashboard_url": "${DASHBOARD_URL}",
    "runcard_url": "${RUNCARD_URL}",
    "team_payload_path": "${STATE_DIR}/team.json" if "${CREATE_DEMO}" == "1" else None,
    "state_dir": "${STATE_DIR}",
}
print(json.dumps(manifest, indent=2))
PY

echo
echo "[local-site] ready"
echo "  mode:          ${MODE}"
echo "  home:          ${HOME_URL}"
echo "  self-host doc: ${PUBLIC_BASE_URL}/.well-known/llm-attested/self-host.json"
echo "  registry:      ${PUBLIC_BASE_URL}/.well-known/runcard/registry.json"
if [[ "${CREATE_DEMO}" == "1" ]]; then
  echo "  join:          ${JOIN_URL}"
  echo "  dashboard:     ${DASHBOARD_URL}"
  echo "  runcard:       ${RUNCARD_URL}"
  echo "  team payload:  ${STATE_DIR}/team.json"
fi
echo "  manifest:      ${SITE_MANIFEST}"
echo
echo "[local-site] press Ctrl+C to stop"

if [[ "${OPEN_BROWSER}" == "1" ]]; then
  OPEN_URL="${RUNCARD_URL:-${HOME_URL}}"
  if command -v open >/dev/null 2>&1; then
    open "${OPEN_URL}" >/dev/null 2>&1 || true
  elif command -v xdg-open >/dev/null 2>&1; then
    xdg-open "${OPEN_URL}" >/dev/null 2>&1 || true
  fi
fi

wait "${SERVICE_PID}"
