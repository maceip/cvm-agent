#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

MODE="${LLMATTEST_SITE_MODE:-auto}"
LISTEN="${LLMATTEST_SITE_LISTEN:-}"
PUBLIC_BASE_URL="${LLMATTEST_SITE_PUBLIC_BASE_URL:-}"
STATE_DIR="${LLMATTEST_SITE_STATE_DIR:-}"
TIMEOUT_SECONDS="${LLMATTEST_SITE_SMOKE_TIMEOUT:-90}"
EXTRA_ARGS=()

usage() {
  cat <<'USAGE'
Usage: packaging/smoke-local-site.sh [options] [-- extra local-site args]

Starts the hosted LLM Attested website, verifies the generated local
organizer/participant flow, then stops it.

Options:
  --mode auto|plain|tee      default: auto
  --listen HOST:PORT         default: random 127.0.0.1 port
  --public-base-url URL      default: http://HOST:PORT
  --state-dir PATH           default: temporary directory
  --timeout SECONDS          default: 90
  -h, --help                 show this help

Examples:
  packaging/smoke-local-site.sh --mode plain
  packaging/smoke-local-site.sh --mode tee --listen 127.0.0.1:8099
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
    --state-dir)
      STATE_DIR="${2:?--state-dir requires PATH}"
      shift 2
      ;;
    --timeout)
      TIMEOUT_SECONDS="${2:?--timeout requires seconds}"
      shift 2
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

if [[ -z "${LISTEN}" ]]; then
  PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"
  LISTEN="127.0.0.1:${PORT}"
fi

if [[ -z "${PUBLIC_BASE_URL}" ]]; then
  PUBLIC_BASE_URL="http://${LISTEN}"
fi

if [[ -z "${STATE_DIR}" ]]; then
  STATE_DIR="$(mktemp -d /tmp/cvms-site-smoke.XXXXXX)"
fi

LOG_PATH="${STATE_DIR}/local-site.log"
MANIFEST_PATH="${STATE_DIR}/local-site.json"
mkdir -p "${STATE_DIR}"

SITE_PID=""
cleanup() {
  if [[ -n "${SITE_PID}" ]]; then
    kill "${SITE_PID}" >/dev/null 2>&1 || true
    wait "${SITE_PID}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

LOCAL_SITE_CMD=(
  "${ROOT_DIR}/packaging/local-site.sh"
  --mode "${MODE}"
  --listen "${LISTEN}"
  --public-base-url "${PUBLIC_BASE_URL}"
  --state-dir "${STATE_DIR}"
)
if [[ ${#EXTRA_ARGS[@]} -gt 0 ]]; then
  LOCAL_SITE_CMD+=("${EXTRA_ARGS[@]}")
fi

"${LOCAL_SITE_CMD[@]}" >"${LOG_PATH}" 2>&1 &
SITE_PID="$!"

deadline=$((SECONDS + TIMEOUT_SECONDS))
while [[ "${SECONDS}" -lt "${deadline}" ]]; do
  if [[ -s "${MANIFEST_PATH}" ]]; then
    break
  fi
  if ! kill -0 "${SITE_PID}" >/dev/null 2>&1; then
    cat "${LOG_PATH}" >&2 || true
    exit 1
  fi
  sleep 0.25
done

if [[ ! -s "${MANIFEST_PATH}" ]]; then
  cat "${LOG_PATH}" >&2 || true
  echo "[smoke-local-site] timed out waiting for ${MANIFEST_PATH}" >&2
  exit 1
fi

python3 - "${MODE}" "${MANIFEST_PATH}" <<'PY'
import json
import sys
import urllib.request

requested_mode = sys.argv[1]
manifest_path = sys.argv[2]
with open(manifest_path, "r", encoding="utf-8") as f:
    manifest = json.load(f)


def fetch(url, accept=None):
    req = urllib.request.Request(url)
    if accept:
        req.add_header("accept", accept)
    with urllib.request.urlopen(req, timeout=10) as resp:
        return resp.read().decode("utf-8")


health = fetch(manifest["health_url"]).strip()
if health == "ok":
    pass
else:
    try:
        parsed_health = json.loads(health)
    except json.JSONDecodeError:
        parsed_health = {}
    if parsed_health.get("ok") is not True:
        raise SystemExit(f"health check returned {health!r}")

self_host = json.loads(fetch(manifest["self_host_url"], "application/json"))
registry = json.loads(fetch(manifest["registry_url"], "application/json"))

actual_mode = manifest["mode"]
runtime = self_host.get("runtime", {})
if runtime.get("mode") != actual_mode:
    raise SystemExit(f"runtime mode {runtime.get('mode')!r} != manifest mode {actual_mode!r}")

if requested_mode == "tee" and not runtime.get("tee_attested"):
    raise SystemExit("TEE smoke expected tee_attested=true")
if requested_mode == "plain" and runtime.get("tee_attested"):
    raise SystemExit("plain smoke expected tee_attested=false")

if "gateway_base_url" not in self_host:
    raise SystemExit("self-host document missing gateway_base_url")
if "accepted_tee_platforms" not in self_host.get("trust_policy", {}):
    raise SystemExit("self-host document missing trust_policy.accepted_tee_platforms")
if registry.get("profile") != "https://cvm.dev/cvm-trust-registry/v1":
    raise SystemExit("registry document has unexpected profile")
if "accepted_tee_platforms" not in registry:
    raise SystemExit("registry missing accepted_tee_platforms")

for key in ("join_url", "dashboard_url", "cvm_url"):
    url = manifest.get(key)
    if not url:
        raise SystemExit(f"manifest missing {key}")
    body = fetch(url)
    if len(body) < 100:
        raise SystemExit(f"{key} response was unexpectedly small")

cvm_json = json.loads(fetch(manifest["cvm_url"] + ".json", "application/json"))
labels = cvm_json.get("labels", {})
if "capture_methods" not in labels or "assurance" not in labels or "enforcement" not in labels:
    raise SystemExit("cvm JSON missing assurance labels")

print(json.dumps({
    "ok": True,
    "mode": actual_mode,
    "tee_attested": runtime.get("tee_attested"),
    "home_url": manifest["home_url"],
    "dashboard_url": manifest["dashboard_url"],
    "cvm_url": manifest["cvm_url"],
}, indent=2))
PY
