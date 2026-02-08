#!/usr/bin/env bash
set -euo pipefail

# One-time server setup for the evaluator service.
# Expected to be run on the target Ubuntu box.

REMOTE_DIR="${REMOTE_DIR:-/opt/evaluator}"

sudo useradd --system --home "$REMOTE_DIR" --shell /usr/sbin/nologin evaluator 2>/dev/null || true

sudo mkdir -p "$REMOTE_DIR"/{data,config}
sudo chown -R evaluator:evaluator "$REMOTE_DIR"

sudo apt-get update -y
sudo apt-get install -y sqlite3 ca-certificates

sudo install -m 0644 -o root -g root deploy/systemd/evaluator.service /etc/systemd/system/evaluator.service
sudo systemctl daemon-reload
sudo systemctl enable evaluator

echo "OK: evaluator user + directories + systemd service installed."
echo "Next: copy binary/config to $REMOTE_DIR and run: sudo systemctl start evaluator"

