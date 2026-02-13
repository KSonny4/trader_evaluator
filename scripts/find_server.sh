#!/usr/bin/env bash
set -euo pipefail

# Resolve the default evaluator deploy SSH target.
#
# Priority:
# 1. TRADING_SERVER_IP env var (explicit override for humans/CI)
# 2. AWS EC2 lookup (running instance with tag Name=trading-bot)
# 3. Fallback placeholder (keeps `make -n` and help output sane)

if [[ -n "${TRADING_SERVER_IP:-}" ]]; then
  echo "ubuntu@${TRADING_SERVER_IP}"
  exit 0
fi

if command -v aws >/dev/null 2>&1; then
  region="$(aws configure get region 2>/dev/null || true)"
  if [[ -z "${region}" ]]; then
    region="eu-west-2"
  fi

  ip="$(
    AWS_DEFAULT_REGION="${region}" aws ec2 describe-instances \
      --filters "Name=instance-state-name,Values=running" "Name=tag:Name,Values=trading-bot" \
      --query 'Reservations[].Instances[].PublicIpAddress | [0]' \
      --output text 2>/dev/null || true
  )"

  # AWS prints "None" for nulls in some output modes.
  if [[ -n "${ip}" && "${ip}" != "None" ]]; then
    echo "ubuntu@${ip}"
    exit 0
  fi
fi

echo "ubuntu@YOUR_SERVER_IP"

