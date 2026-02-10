#!/usr/bin/env bash
set -euo pipefail

# Verifies deploy/push-dashboards.sh:
# - supports DASHBOARDS_DIR override
# - supports DRY_RUN=1 (no network calls)

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

mkdir -p "$TMP_DIR/dashboards"
cat > "$TMP_DIR/dashboards/test.json" <<'JSON'
{
  "title": "Test Dashboard",
  "schemaVersion": 39,
  "panels": []
}
JSON

OUT="$TMP_DIR/out.txt"

DASHBOARDS_DIR="$TMP_DIR/dashboards" \
GRAFANA_URL="https://example.grafana.net" \
GRAFANA_SA_TOKEN="glsa_dummy" \
DRY_RUN=1 \
  ./deploy/push-dashboards.sh >"$OUT" 2>&1

# If override worked, the script must print the overridden path.
grep -F "Dashboards: $TMP_DIR/dashboards" "$OUT" >/dev/null
# If dry run worked, it should not attempt to hit the network.
grep -F "DRY RUN" "$OUT" >/dev/null
