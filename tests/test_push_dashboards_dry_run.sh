#!/usr/bin/env bash
set -euo pipefail

# Verifies deploy/push-dashboards.sh:
# - blocks accidental DASHBOARDS_DIR override by default
# - supports explicit DASHBOARDS_DIR override when ALLOW_DASHBOARDS_DIR_OVERRIDE=1
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

# By default, DASHBOARDS_DIR overrides are ignored to prevent cross-project uploads.
DASHBOARDS_DIR="$TMP_DIR/dashboards" \
GRAFANA_URL="https://example.grafana.net" \
GRAFANA_SA_TOKEN="glsa_dummy" \
DRY_RUN=1 \
  ./deploy/push-dashboards.sh >"$OUT" 2>&1

# It must warn and fall back to the repo default dashboards folder.
grep -F "WARNING: DASHBOARDS_DIR is set but override is disabled" "$OUT" >/dev/null
grep -F "Dashboards: $(cd deploy && pwd)/../dashboards" "$OUT" >/dev/null
# If dry run worked, it should not attempt to hit the network.
grep -F "DRY RUN" "$OUT" >/dev/null

# When explicitly enabled, the override must work.
OUT2="$TMP_DIR/out2.txt"
ALLOW_DASHBOARDS_DIR_OVERRIDE=1 \
DASHBOARDS_DIR="$TMP_DIR/dashboards" \
GRAFANA_URL="https://example.grafana.net" \
GRAFANA_SA_TOKEN="glsa_dummy" \
DRY_RUN=1 \
  ./deploy/push-dashboards.sh >"$OUT2" 2>&1
grep -F "Dashboards: $TMP_DIR/dashboards" "$OUT2" >/dev/null
grep -F "DRY RUN" "$OUT2" >/dev/null
