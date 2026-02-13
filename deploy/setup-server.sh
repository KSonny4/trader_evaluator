#!/usr/bin/env bash
set -euo pipefail

# Run ON the server after first SSH connection.
# Creates evaluator user/group, directories, and installs dependencies.
# No Rust toolchain needed — binaries are cross-compiled on macOS.

echo "=== Evaluator Server Setup ==="

# 1. System packages (minimal — no build tools, binaries are cross-compiled on macOS)
echo "Installing system packages..."
sudo apt-get update -qq
sudo apt-get install -y -qq curl sqlite3

# 2. Create evaluator system user/group for service isolation
if ! id evaluator &>/dev/null; then
    echo "Creating evaluator system user..."
    sudo useradd --system --no-create-home --shell /usr/sbin/nologin evaluator
    echo "evaluator user created"
else
    echo "evaluator user already exists"
fi

# 3. Create directories
echo "Creating directories..."
sudo mkdir -p /opt/evaluator/data /opt/evaluator/config
sudo chown -R evaluator:evaluator /opt/evaluator

# 4. Install Grafana Alloy (for Prometheus remote write to Grafana Cloud)
if ! command -v alloy &>/dev/null; then
    echo "Installing Grafana Alloy..."
    sudo mkdir -p /etc/apt/keyrings/
    wget -q -O - https://apt.grafana.com/gpg.key | gpg --dearmor | sudo tee /etc/apt/keyrings/grafana.gpg > /dev/null
    echo "deb [signed-by=/etc/apt/keyrings/grafana.gpg] https://apt.grafana.com stable main" | sudo tee /etc/apt/sources.list.d/grafana.list
    sudo apt-get update -qq
    sudo apt-get install -y -qq alloy
    echo "Alloy installed"
else
    echo "Grafana Alloy already installed"
fi

echo ""
echo "=== Server setup complete ==="
echo "Next: run deploy.sh from your local machine"
