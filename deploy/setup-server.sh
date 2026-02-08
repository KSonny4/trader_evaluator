#!/usr/bin/env bash
set -euo pipefail

# Run ON the server after first SSH connection.
# Creates directories, installs Grafana Alloy and node_exporter.
# No Rust toolchain needed — binaries are cross-compiled on macOS.

echo "=== Evaluator Server Setup ==="

# 1. System packages (minimal — no build tools, binaries are cross-compiled on macOS)
echo "Installing system packages..."
sudo apt-get update -qq
sudo apt-get install -y -qq curl wget

# 2. Create directories
echo "Creating directories..."
mkdir -p ~/evaluator/data
mkdir -p ~/evaluator/config

# 3. Install Grafana Alloy (for Prometheus remote write to Grafana Cloud)
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

# 4. Install node_exporter (for EC2 system metrics)
if ! command -v node_exporter &>/dev/null; then
    echo "Installing node_exporter..."
    NODE_EXPORTER_VERSION="1.8.2"
    wget -q "https://github.com/prometheus/node_exporter/releases/download/v${NODE_EXPORTER_VERSION}/node_exporter-${NODE_EXPORTER_VERSION}.linux-amd64.tar.gz" -O /tmp/node_exporter.tar.gz
    tar xzf /tmp/node_exporter.tar.gz -C /tmp
    sudo mv "/tmp/node_exporter-${NODE_EXPORTER_VERSION}.linux-amd64/node_exporter" /usr/local/bin/
    rm -rf /tmp/node_exporter*

    # Create systemd service
    sudo tee /etc/systemd/system/node_exporter.service > /dev/null <<'SERVICEEOF'
[Unit]
Description=Prometheus Node Exporter
After=network.target

[Service]
Type=simple
User=ubuntu
ExecStart=/usr/local/bin/node_exporter
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
SERVICEEOF

    sudo systemctl daemon-reload
    sudo systemctl enable node_exporter
    sudo systemctl start node_exporter
    echo "node_exporter installed and running on :9100"
else
    echo "node_exporter already installed"
fi

echo ""
echo "=== Server setup complete ==="
echo "Next: run deploy.sh from your local machine"
