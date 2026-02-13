#!/usr/bin/env bash
set -euo pipefail

# Add an evaluator dashboard route to an EXISTING cloudflared tunnel.
# Run this on the target server (which already has cloudflared running).
#
# Prerequisites:
#   - cloudflared already installed and running (e.g., for ai_wars)
#   - An existing tunnel configured in /etc/cloudflared/config.yml
#
# Usage:
#   bash deploy/setup-cloudflared.sh [TUNNEL_NAME]

TUNNEL_NAME="${1:-${TUNNEL_NAME:-}}"
ROUTE_HOSTNAME="${CF_HOSTNAME:-sniper.pkubelka.cz}"
LOCAL_SERVICE="http://localhost:8080"
CONFIG_FILE="/etc/cloudflared/config.yml"

# Verify cloudflared is installed
if ! command -v cloudflared &>/dev/null; then
    echo "ERROR: cloudflared is not installed."
    echo "Install it first or check that the server has an existing tunnel."
    exit 1
fi

# Verify config exists
if [ ! -f "$CONFIG_FILE" ]; then
    echo "ERROR: $CONFIG_FILE not found."
    echo "This script expects an existing cloudflared tunnel."
    exit 1
fi

# Check if the route already exists
if grep -q "$ROUTE_HOSTNAME" "$CONFIG_FILE"; then
    echo "Route for $ROUTE_HOSTNAME already exists in $CONFIG_FILE"
    echo "Nothing to do."
    exit 0
fi

# Auto-detect tunnel name if not provided
if [ -z "$TUNNEL_NAME" ]; then
    TUNNEL_NAME=$(grep '^tunnel:' "$CONFIG_FILE" | awk '{print $2}')
    if [ -z "$TUNNEL_NAME" ]; then
        echo "ERROR: Could not detect tunnel name from $CONFIG_FILE"
        echo "Usage: $0 <TUNNEL_NAME>"
        exit 1
    fi
    echo "Auto-detected tunnel: $TUNNEL_NAME"
fi

echo "=== Adding evaluator route to existing tunnel ==="
echo "Tunnel:   $TUNNEL_NAME"
echo "Hostname: $ROUTE_HOSTNAME"
echo "Service:  $LOCAL_SERVICE"
echo ""

# Add the route before the catch-all 404 rule
echo "Updating $CONFIG_FILE..."
sudo sed -i "/- service: http_status:404/i\\
  - hostname: ${ROUTE_HOSTNAME}\\
    service: ${LOCAL_SERVICE}" "$CONFIG_FILE"

# Verify the route was actually inserted
if ! grep -q "$ROUTE_HOSTNAME" "$CONFIG_FILE"; then
    echo "ERROR: Failed to insert route into $CONFIG_FILE"
    echo "The catch-all rule '- service: http_status:404' may not exist."
    echo "Please add the route manually:"
    echo "  - hostname: $ROUTE_HOSTNAME"
    echo "    service: $LOCAL_SERVICE"
    exit 1
fi

echo "Config updated. New ingress rules:"
grep -A1 'hostname:' "$CONFIG_FILE" || true
echo ""

# Add DNS route
echo "Adding DNS route..."
cloudflared tunnel route dns "$TUNNEL_NAME" "$ROUTE_HOSTNAME" 2>/dev/null || echo "DNS route may already exist (that's OK)"

# Restart cloudflared
echo "Restarting cloudflared..."
sudo systemctl restart cloudflared

echo ""
echo "=== Done ==="
echo "Evaluator dashboard should be accessible at https://$ROUTE_HOSTNAME"
echo ""
echo "Verify with: sudo systemctl status cloudflared"
echo "Logs: sudo journalctl -u cloudflared -f"
