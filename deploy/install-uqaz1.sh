#!/usr/bin/env bash
# Deploy eat-pass + tool-gate on Azure CVM uqaz1 (attest.secure.build).
set -euo pipefail

HOST="${DEPLOY_HOST:-azureuser@attest.secure.build}"
RG="${AZURE_RG:-UQ-TEE-VALIDATION}"
VM="${AZURE_VM:-uqaz1}"
export ALLOW_MEASUREMENT="${ALLOW_MEASUREMENT:-41f77fe5c1416343f84dbeeded504eb4a2c450861317ed3e4e46cd771c79243a4cbeb3d75ec663e6a7a47bd1f4fab503}"

echo "=== Opening Azure NSG port 8787 ==="
az vm open-port --resource-group "$RG" --name "$VM" --port 8787 --priority 1045 --output none 2>/dev/null || true

SCRIPT="$(mktemp)"
trap 'rm -f "$SCRIPT"' EXIT
cat > "$SCRIPT" <<'REMOTE'
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
STACK="$HOME/tool-gate-stack"
EAT_PASS="$HOME/eat-pass-src"
CVM="$HOME/cvm-agent-src"
UQ="$HOME/unified-quote/target/release/uq"
MEAS="${ALLOW_MEASUREMENT:?}"

mkdir -p "$STACK"

if [[ ! -d "$EAT_PASS/.git" ]]; then
  git clone --depth 1 https://github.com/maceip/eat-pass.git "$EAT_PASS"
else
  git -C "$EAT_PASS" pull --ff-only origin main
fi

if [[ ! -d "$CVM/.git" ]]; then
  git clone --depth 1 https://github.com/maceip/cvm-agent.git "$CVM"
else
  git -C "$CVM" pull --ff-only origin main
fi

echo "=== building eat-pass ==="
( cd "$EAT_PASS" && cargo build --release )

echo "=== building tool-gate + cvm ==="
( cd "$CVM" && cargo build --release --bin tool-gate --bin cvm )

EAT="$EAT_PASS/target/release/eat-pass"
TG="$CVM/target/release/tool-gate"
CVM_BIN="$CVM/target/release/cvm"

if [[ ! -f "$STACK/seeds.env" ]]; then
  {
    echo "EATPASS_ATTESTER_SEED=$(openssl rand -hex 32)"
    echo "EATPASS_KT_SEED=$(openssl rand -hex 32)"
  } > "$STACK/seeds.env"
  chmod 600 "$STACK/seeds.env"
fi
# shellcheck disable=SC1090
source "$STACK/seeds.env"

if [[ ! -f "$STACK/tool-gate.crt" ]]; then
  openssl req -x509 -newkey rsa:2048 -keyout "$STACK/tool-gate.key" -out "$STACK/tool-gate.crt" \
    -days 365 -nodes -subj "/CN=tools.attest.secure.build" 2>/dev/null
  chmod 600 "$STACK/tool-gate.key"
fi

export EATPASS_ATTESTER_SEED
if [[ ! -f "$STACK/attester.pub" ]]; then
  "$EAT" attester --gate azure --allow "$MEAS" --listen 127.0.0.1:18087 --insecure-http &
  ap=$!
  sleep 2
  curl -sf http://127.0.0.1:18087/pubkey | python3 -c "import json,sys; print(json.load(sys.stdin)['pubkey'])" > "$STACK/attester.pub"
  kill "$ap" 2>/dev/null || true
  wait "$ap" 2>/dev/null || true
fi
ATTESTER_PUB=$(cat "$STACK/attester.pub")

export EATPASS_KT_SEED
if [[ ! -f "$STACK/kt.pub" ]]; then
  "$EAT" issuer --attester-pub "$ATTESTER_PUB" --listen 127.0.0.1:19997 --insecure-http \
    > "$STACK/issuer-bootstrap.log" 2>&1 &
  ip=$!
  for _ in $(seq 1 30); do
    grep -q 'kt log pubkey' "$STACK/issuer-bootstrap.log" 2>/dev/null && break
    sleep 0.5
  done
  KT_LOG_PUB=$(grep 'kt log pubkey' "$STACK/issuer-bootstrap.log" | awk '{print $NF}' | head -1)
  kill "$ip" 2>/dev/null || true
  wait "$ip" 2>/dev/null || true
  [[ -n "$KT_LOG_PUB" ]] || { echo "failed to capture KT log pubkey" >&2; cat "$STACK/issuer-bootstrap.log" >&2; exit 1; }
  echo "$KT_LOG_PUB" > "$STACK/kt.pub"
fi
KT_LOG_PUB=$(cat "$STACK/kt.pub")

cat > "$STACK/tool-gate.env" <<ENV
TOOL_GATE_SMTP_HOST=smtp.gmail.com
TOOL_GATE_SMTP_PORT=587
TOOL_GATE_SMTP_USER=agent@secure.build
TOOL_GATE_IMAP_PASSWORD=unused-dry-run
TOOL_GATE_FROM=agent@secure.build
TOOL_GATE_CONTACT_RYAN=ryan@example.com
TOOL_GATE_ALLOWED_TO=ryan@example.com
TOOL_GATE_DRY_RUN=1
ENV
chmod 600 "$STACK/tool-gate.env"

mkdir -p "$HOME/.config/systemd/user"

cat > "$HOME/.config/systemd/user/eat-pass-attester.service" <<UNIT
[Unit]
Description=eat-pass attester
After=network-online.target

[Service]
EnvironmentFile=%h/tool-gate-stack/seeds.env
ExecStart=${EAT} attester --gate azure --allow ${MEAS} --listen 127.0.0.1:8087 --insecure-http
Restart=on-failure

[Install]
WantedBy=default.target
UNIT

cat > "$HOME/.config/systemd/user/eat-pass-issuer.service" <<UNIT
[Unit]
Description=eat-pass issuer
After=eat-pass-attester.service

[Service]
EnvironmentFile=%h/tool-gate-stack/seeds.env
ExecStart=${EAT} issuer --attester-pub ${ATTESTER_PUB} --listen 127.0.0.1:8088 --insecure-http
Restart=on-failure

[Install]
WantedBy=default.target
UNIT

cat > "$HOME/.config/systemd/user/eat-pass-redeemer.service" <<UNIT
[Unit]
Description=eat-pass redeemer

[Service]
ExecStart=${EAT} redeem --listen 127.0.0.1:8100 --insecure-http
Restart=on-failure

[Install]
WantedBy=default.target
UNIT

cat > "$HOME/.config/systemd/user/tool-gate.service" <<UNIT
[Unit]
Description=tool-gate (eat-pass gated email)
After=eat-pass-issuer.service eat-pass-redeemer.service
Wants=eat-pass-attester.service eat-pass-issuer.service eat-pass-redeemer.service

[Service]
EnvironmentFile=%h/tool-gate-stack/tool-gate.env
ExecStart=${TG} --listen 0.0.0.0:8787 --issuer http://127.0.0.1:8088 --redeemer http://127.0.0.1:8100 --attester http://127.0.0.1:8087 --kt-log-pub ${KT_LOG_PUB} --tls-cert ${STACK}/tool-gate.crt --tls-key ${STACK}/tool-gate.key
Restart=on-failure

[Install]
WantedBy=default.target
UNIT

loginctl enable-linger "$USER" 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user enable eat-pass-attester eat-pass-issuer eat-pass-redeemer tool-gate
systemctl --user restart eat-pass-attester eat-pass-issuer eat-pass-redeemer tool-gate
sleep 3

systemctl --user is-active eat-pass-attester eat-pass-issuer eat-pass-redeemer tool-gate
curl -sk https://127.0.0.1:8787/healthz
echo ""
echo "DEPLOYED https://attest.secure.build:8787/"
echo "KT_LOG_PUB=$KT_LOG_PUB"
echo "ATTESTER_PUB=$ATTESTER_PUB"
echo "CVM=$CVM_BIN UQ=$UQ"
REMOTE

chmod +x "$SCRIPT"
scp -q "$SCRIPT" "$HOST:/tmp/install-tool-gate.sh"
ssh "$HOST" "ALLOW_MEASUREMENT='$ALLOW_MEASUREMENT' bash /tmp/install-tool-gate.sh"
rm -f "$SCRIPT"
trap - EXIT

echo "Live: https://attest.secure.build:8787/"
