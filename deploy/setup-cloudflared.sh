#!/usr/bin/env bash
set -euo pipefail

# Install cloudflared and create a tunnel for the evaluator dashboard.
# Run this on the target server.
#
# Prerequisites:
#   - Must be run interactively (cloudflared login opens a browser URL)
#
# Usage:
#   bash deploy/setup-cloudflared.sh

TUNNEL_NAME="${TUNNEL_NAME:-evaluator-dashboard}"
HOSTNAME="${CF_HOSTNAME:-sniper.pkubelka.cz}"
LOCAL_SERVICE="http://localhost:8080"

echo "=== Installing cloudflared ==="
# Add Cloudflare's GPG key and repo
sudo mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg | sudo tee /usr/share/keyrings/cloudflare-main.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared $(lsb_release -cs) main" | \
    sudo tee /etc/apt/sources.list.d/cloudflared.list
sudo apt-get update -y
sudo apt-get install -y cloudflared

echo ""
echo "=== Authenticating with Cloudflare ==="
echo "This will open a URL — paste it in your browser to authorize."
cloudflared tunnel login

echo ""
echo "=== Creating tunnel: $TUNNEL_NAME ==="
cloudflared tunnel create "$TUNNEL_NAME"

# Get the tunnel ID
TUNNEL_ID=$(cloudflared tunnel list --name "$TUNNEL_NAME" --output json | python3 -c "import sys,json; print(json.load(sys.stdin)[0]['id'])")
CRED_FILE="$HOME/.cloudflared/${TUNNEL_ID}.json"

echo "Tunnel ID: $TUNNEL_ID"
echo "Credentials: $CRED_FILE"

echo ""
echo "=== Writing config ==="
sudo mkdir -p /etc/cloudflared
sudo tee /etc/cloudflared/config.yml > /dev/null <<EOF
tunnel: ${TUNNEL_ID}
credentials-file: ${CRED_FILE}

ingress:
  - hostname: ${HOSTNAME}
    service: ${LOCAL_SERVICE}
  - service: http_status:404
EOF

echo ""
echo "=== Creating DNS route ==="
cloudflared tunnel route dns "$TUNNEL_NAME" "$HOSTNAME"

echo ""
echo "=== Installing as systemd service ==="
sudo cloudflared service install
sudo systemctl enable cloudflared
sudo systemctl start cloudflared

echo ""
echo "=== Done ==="
echo "Tunnel $TUNNEL_NAME is running."
echo "Dashboard should be accessible at https://$HOSTNAME"
echo ""
echo "Verify with: sudo systemctl status cloudflared"
echo "Logs: sudo journalctl -u cloudflared -f"
