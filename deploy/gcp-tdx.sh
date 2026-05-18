#!/bin/bash
set -euo pipefail

# ===========================================================================
# Deploy bountynet-genesis runner on GCP TDX Confidential VM
#
# Prerequisites:
#   - gcloud CLI authenticated
#   - GITHUB_TOKEN with repo + admin:org scope (for runner registration)
#   - Docker image built and pushed to GHCR
#
# Usage:
#   GITHUB_TOKEN=ghp_xxx GITHUB_REPO=maceip/bountynet-genesis ./deploy/gcp-tdx.sh
# ===========================================================================

: "${GITHUB_TOKEN:?GITHUB_TOKEN is required}"
: "${GITHUB_REPO:?GITHUB_REPO is required (e.g., maceip/bountynet-genesis)}"

PROJECT="${GCP_PROJECT:-lowkey-b7136}"
ZONE="${GCP_ZONE:-us-central1-a}"
REGION="${GCP_REGION:-${ZONE%-*}}"
INSTANCE_NAME="${INSTANCE_NAME:-bountynet-tdx-runner}"
MACHINE_TYPE="${MACHINE_TYPE:-c3-standard-4}"
IMAGE_TAG="${IMAGE_TAG:-latest}"
REGISTRY="ghcr.io"
IMAGE="${REGISTRY}/${GITHUB_REPO}:${IMAGE_TAG}"
STATIC_IP_NAME="${STATIC_IP_NAME:-${INSTANCE_NAME}-ip}"

echo "=== Deploying bountynet-genesis TDX runner ==="
echo "  Instance:  ${INSTANCE_NAME}"
echo "  Zone:      ${ZONE}"
echo "  Region:    ${REGION}"
echo "  Machine:   ${MACHINE_TYPE} (TDX Confidential VM)"
echo "  Image:     ${IMAGE}"
echo "  Repo:      ${GITHUB_REPO}"
echo ""

# Create startup script that pulls and runs the container
STARTUP_SCRIPT=$(cat <<'STARTUP'
#!/bin/bash
set -euo pipefail

# Install Docker
if ! command -v docker &>/dev/null; then
    curl -fsSL https://get.docker.com | sh
fi

# Wait for Docker
systemctl start docker
sleep 2

# Login to GHCR (public image, but login helps with rate limits)
echo "${GHCR_TOKEN}" | docker login ghcr.io -u "${GHCR_USER}" --password-stdin 2>/dev/null || true

# Pull the attested runner image
docker pull "${RUNNER_IMAGE}"

# Run the container with TDX device access
docker run -d \
    --name bountynet-runner \
    --restart unless-stopped \
    --device /dev/tdx_guest:/dev/tdx_guest \
    -v /sys/kernel/config/tsm:/sys/kernel/config/tsm \
    --privileged \
    -p 9384:9384 \
    -e GITHUB_TOKEN="${GITHUB_TOKEN}" \
    -e GITHUB_REPO="${GITHUB_REPO}" \
    -e RUNNER_NAME="bountynet-tdx-$(hostname | cut -c1-8)" \
    -e RUNNER_LABELS="self-hosted,bountynet,tee,tdx" \
    "${RUNNER_IMAGE}"

echo "bountynet-genesis runner started"
STARTUP
)

# Check if instance already exists
if gcloud compute instances describe "${INSTANCE_NAME}" --zone="${ZONE}" --project="${PROJECT}" &>/dev/null; then
    echo "Instance ${INSTANCE_NAME} already exists. Deleting..."
    gcloud compute instances delete "${INSTANCE_NAME}" --zone="${ZONE}" --project="${PROJECT}" --quiet
fi

if ! gcloud compute addresses describe "${STATIC_IP_NAME}" --region="${REGION}" --project="${PROJECT}" &>/dev/null; then
    echo "Reserving regional static external IP ${STATIC_IP_NAME}..."
    gcloud compute addresses create "${STATIC_IP_NAME}" \
        --region="${REGION}" \
        --project="${PROJECT}"
fi

EXTERNAL_IP=$(gcloud compute addresses describe "${STATIC_IP_NAME}" \
    --region="${REGION}" --project="${PROJECT}" \
    --format='get(address)')

echo "Creating TDX Confidential VM..."
gcloud compute instances create "${INSTANCE_NAME}" \
    --project="${PROJECT}" \
    --zone="${ZONE}" \
    --machine-type="${MACHINE_TYPE}" \
    --confidential-compute-type=TDX \
    --image-family=ubuntu-2404-lts-amd64 \
    --image-project=ubuntu-os-cloud \
    --maintenance-policy=TERMINATE \
    --boot-disk-size=50GB \
    --metadata=startup-script="${STARTUP_SCRIPT}" \
    --metadata=GITHUB_TOKEN="${GITHUB_TOKEN}" \
    --metadata=GITHUB_REPO="${GITHUB_REPO}" \
    --metadata=RUNNER_IMAGE="${IMAGE}" \
    --metadata=GHCR_TOKEN="${GITHUB_TOKEN}" \
    --metadata=GHCR_USER="$(echo ${GITHUB_REPO} | cut -d/ -f1)" \
    --address="${EXTERNAL_IP}" \
    --tags=bountynet-runner \
    --scopes=default

echo ""
echo "=== Deployment Complete ==="
echo "  Instance: ${INSTANCE_NAME}"
echo "  Static external IP: ${EXTERNAL_IP}"
echo "  Attestation endpoint: http://${EXTERNAL_IP}:9384/attest"
echo ""
echo "  Wait ~2-3 minutes for Docker install and runner startup, then:"
echo "    curl http://${EXTERNAL_IP}:9384/health"
echo "    cargo run --bin verify-runner -- http://${EXTERNAL_IP}:9384 --expected-platform Tdx"
echo ""
echo "  The IP/DNS only routes you to an endpoint. Trust starts after verify-runner"
echo "  checks a fresh nonce-bound hardware quote, Value X, and measurements."
echo ""
echo "  To check runner status:"
echo "    gcloud compute ssh ${INSTANCE_NAME} --zone=${ZONE} -- docker logs bountynet-runner"
echo ""
echo "  To tear down:"
echo "    gcloud compute instances delete ${INSTANCE_NAME} --zone=${ZONE} --quiet"
