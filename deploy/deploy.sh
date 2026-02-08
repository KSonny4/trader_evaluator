#!/usr/bin/env bash
set -euo pipefail

# Deploy evaluator to the remote server.
# Usage: ./deploy/deploy.sh [SERVER_IP]
#
# Cross-compiles static Linux binaries on macOS via `cross`, then uploads
# the binaries to the server. No Rust toolchain needed on the server.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET="x86_64-unknown-linux-musl"
BINARIES=(evaluator web)

# Load server info
if [ -f "$SCRIPT_DIR/.server-info" ]; then
    source "$SCRIPT_DIR/.server-info"
fi

SERVER_IP="${1:-${SERVER_IP:-}}"
KEY_FILE="${KEY_FILE:-trading-bot.pem}"
SSH_USER="${SSH_USER:-ubuntu}"

if [ -z "$SERVER_IP" ]; then
    echo "Usage: ./deploy/deploy.sh <SERVER_IP>"
    echo "  or run provision.sh first to create .server-info"
    exit 1
fi

SSH_CMD="ssh -i $KEY_FILE -o StrictHostKeyChecking=no $SSH_USER@$SERVER_IP"
SCP_CMD="scp -i $KEY_FILE -o StrictHostKeyChecking=no"

echo "=== Deploying to $SERVER_IP ==="

# 1. Cross-compile static Linux binaries locally
# Export GIT_SHA so build.rs can pick it up (git isn't available inside cross Docker)
export GIT_SHA
GIT_SHA="$(cd "$PROJECT_DIR" && git rev-parse --short HEAD)"
echo "Cross-compiling for $TARGET (git_sha=$GIT_SHA)..."
PACKAGES=""
for bin in "${BINARIES[@]}"; do
    PACKAGES="$PACKAGES -p $bin"
done
(cd "$PROJECT_DIR" && CROSS_CONTAINER_OPTS="--env GIT_SHA=$GIT_SHA" cross build --release --target "$TARGET" $PACKAGES)

# 2. Verify binaries exist
for bin in "${BINARIES[@]}"; do
    BINARY="$PROJECT_DIR/target/$TARGET/release/$bin"
    if [ ! -f "$BINARY" ]; then
        echo "ERROR: Binary not found: $BINARY"
        exit 1
    fi
    echo "  Built: $bin ($(du -h "$BINARY" | cut -f1))"
done

# 3. Create remote directories and stop running services before uploading
$SSH_CMD "mkdir -p ~/evaluator/data ~/evaluator/config"
echo "Stopping services before upload..."
$SSH_CMD "sudo systemctl stop evaluator 2>/dev/null || true"
$SSH_CMD "sudo systemctl stop evaluator-web 2>/dev/null || true"

# 4. Upload binaries
echo "Uploading binaries..."
for bin in "${BINARIES[@]}"; do
    $SCP_CMD "$PROJECT_DIR/target/$TARGET/release/$bin" "$SSH_USER@$SERVER_IP:~/evaluator/$bin"
    $SSH_CMD "chmod +x ~/evaluator/$bin"
done

# 5. Copy config and .env
echo "Copying config..."
$SCP_CMD "$PROJECT_DIR/config/default.toml" "$SSH_USER@$SERVER_IP:~/evaluator/config/default.toml"
if [ -f "$PROJECT_DIR/.env" ]; then
    echo "Copying .env..."
    $SCP_CMD "$PROJECT_DIR/.env" "$SSH_USER@$SERVER_IP:~/evaluator/.env"
fi

# 6. Install systemd services
echo "Installing systemd services..."
$SCP_CMD "$SCRIPT_DIR/systemd/evaluator.service" "$SSH_USER@$SERVER_IP:/tmp/evaluator.service"
$SCP_CMD "$SCRIPT_DIR/systemd/web.service" "$SSH_USER@$SERVER_IP:/tmp/web.service"
$SSH_CMD "sudo mv /tmp/evaluator.service /etc/systemd/system/"
$SSH_CMD "sudo mv /tmp/web.service /etc/systemd/system/"
$SSH_CMD "sudo systemctl daemon-reload"

# 7. Install Alloy config if Grafana Cloud is configured
if $SSH_CMD "grep -q 'GRAFANA_CLOUD' ~/evaluator/.env 2>/dev/null"; then
    echo "Configuring Grafana Alloy..."
    $SCP_CMD "$SCRIPT_DIR/alloy-config.alloy" "$SSH_USER@$SERVER_IP:/tmp/config.alloy"
    $SSH_CMD "sudo mv /tmp/config.alloy /etc/alloy/config.alloy"
    # Inject Grafana Cloud env vars into Alloy's environment file (remove old ones first to avoid duplicates)
    $SSH_CMD "sudo sed -i '/^GRAFANA_CLOUD/d' /etc/default/alloy"
    $SSH_CMD "grep '^GRAFANA_CLOUD' ~/evaluator/.env | sudo tee -a /etc/default/alloy > /dev/null"
    $SSH_CMD "sudo systemctl enable alloy && sudo systemctl restart alloy"
fi

# 8. Push dashboards to Grafana Cloud (if credentials available)
if [ -f "$PROJECT_DIR/.env.agent" ]; then
    source "$PROJECT_DIR/.env.agent"
fi
if [ -n "${GRAFANA_URL:-}" ] && [ -n "${GRAFANA_SA_TOKEN:-}" ]; then
    echo "Pushing dashboards to Grafana Cloud..."
    "$SCRIPT_DIR/push-dashboards.sh"
else
    echo "Skipping dashboard push (GRAFANA_URL / GRAFANA_SA_TOKEN not set)"
fi

# 9. Restart services
echo "Restarting services..."
$SSH_CMD "sudo systemctl restart evaluator"
$SSH_CMD "sudo systemctl restart evaluator-web"

echo ""
echo "=== Deploy complete ==="
echo ""
echo "Check status:"
echo "  $SSH_CMD 'sudo systemctl status evaluator'"
echo "  $SSH_CMD 'sudo systemctl status evaluator-web'"
echo ""
echo "View logs:"
echo "  $SSH_CMD 'sudo journalctl -u evaluator -f'"
echo "  $SSH_CMD 'sudo journalctl -u evaluator-web -f'"
echo ""
echo "Stop everything:"
echo "  $SSH_CMD 'sudo systemctl stop evaluator evaluator-web'"
