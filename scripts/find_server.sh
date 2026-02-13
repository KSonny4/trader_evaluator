#!/usr/bin/env bash
set -euo pipefail

# Resolve the default evaluator deploy SSH target.
#
# Priority:
# 1. TRADING_SERVER_IP env var (explicit override for humans/CI)
# 2. deploy/.server-info file (static config)
# 3. Fallback placeholder (keeps `make -n` and help output sane)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVER_INFO="$SCRIPT_DIR/../deploy/.server-info"

# Default values
SSH_USER="ksonny"
SERVER_IP=""

# Load from .server-info if it exists
if [ -f "$SERVER_INFO" ]; then
    # shellcheck disable=SC1090
    source "$SERVER_INFO"
fi

# Env var override
if [[ -n "${TRADING_SERVER_IP:-}" ]]; then
    SERVER_IP="$TRADING_SERVER_IP"
fi

if [[ -n "$SERVER_IP" ]]; then
    echo "${SSH_USER}@${SERVER_IP}"
else
    echo "${SSH_USER}@YOUR_SERVER_IP"
fi
