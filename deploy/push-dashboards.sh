#!/usr/bin/env bash
set -euo pipefail

# Push Grafana dashboards in deploy/dashboards/ to Grafana Cloud/OSS via HTTP API.
#
# Required env:
# - GRAFANA_URL: e.g. https://<stack>.grafana.net
# - GRAFANA_SA_TOKEN: Grafana service account token

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DASH_DIR="$SCRIPT_DIR/dashboards"

if [ -z "${GRAFANA_URL:-}" ] || [ -z "${GRAFANA_SA_TOKEN:-}" ]; then
  echo "ERROR: GRAFANA_URL and GRAFANA_SA_TOKEN must be set"
  exit 1
fi

if [ ! -d "$DASH_DIR" ]; then
  echo "No dashboards dir found at $DASH_DIR"
  exit 0
fi

shopt -s nullglob
files=("$DASH_DIR"/*.json)
if [ ${#files[@]} -eq 0 ]; then
  echo "No dashboard JSON files found in $DASH_DIR"
  exit 0
fi

for f in "${files[@]}"; do
  echo "Pushing dashboard: $(basename "$f")"
  curl -sS -X POST \
    -H "Authorization: Bearer ${GRAFANA_SA_TOKEN}" \
    -H "Content-Type: application/json" \
    --data @"$f" \
    "${GRAFANA_URL%/}/api/dashboards/db" \
    >/dev/null
done

echo "Dashboards pushed."

