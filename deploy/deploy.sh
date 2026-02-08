#!/usr/bin/env bash
set -euo pipefail

make deploy SERVER="${1:-ubuntu@YOUR_SERVER_IP}"

