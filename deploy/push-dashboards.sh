#!/usr/bin/env bash
set -euo pipefail

# Push local Grafana dashboard JSON files to Grafana Cloud.
#
# Usage:
#   source .env.agent && ./deploy/push-dashboards.sh
#
# Environment variables:
#   GRAFANA_URL       - Grafana Cloud instance URL (e.g. https://meowlabs.grafana.net)
#   GRAFANA_SA_TOKEN  - Grafana Service Account token (Editor or Admin)
#
# Note: GRAFANA_CLOUD_API_KEY from .env is an Alloy/Prometheus/Loki/Tempo token and
# does NOT have permissions to manage dashboards.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# Allow overriding dashboards location (e.g. reuse dashboards from another repo).
DASHBOARDS_DIR_DEFAULT="$SCRIPT_DIR/../dashboards"
DASHBOARDS_DIR="${DASHBOARDS_DIR:-$DASHBOARDS_DIR_DEFAULT}"
FOLDER_TITLE="trader-evaluator"

# Convenience: allow running without manually sourcing env files.
# Prefer .env.agent, fallback to .env. Both are optional.
if [ -z "${GRAFANA_URL:-}" ] || [ -z "${GRAFANA_SA_TOKEN:-}" ]; then
  if [ -f "$PROJECT_DIR/.env.agent" ]; then
    # shellcheck disable=SC1091
    source "$PROJECT_DIR/.env.agent"
  elif [ -f "$PROJECT_DIR/.env" ]; then
    # .env commonly contains KEY=VALUE (not exported); export while sourcing.
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_DIR/.env"
    set +a
  fi
fi

if [ -z "${GRAFANA_URL:-}" ]; then
  echo "ERROR: GRAFANA_URL is not set (e.g. https://your-slug.grafana.net)" >&2
  echo "Tip: cp .env.agent.example .env.agent && edit it, then run: source .env.agent && ./deploy/push-dashboards.sh" >&2
  exit 1
fi

if [ -z "${GRAFANA_SA_TOKEN:-}" ]; then
  echo "ERROR: GRAFANA_SA_TOKEN is not set (Grafana Service Account token, Editor/Admin)" >&2
  echo "Tip: this is NOT GRAFANA_CLOUD_API_KEY. Create a Grafana Service Account token and set GRAFANA_SA_TOKEN." >&2
  exit 1
fi

GRAFANA_URL="${GRAFANA_URL%/}"

echo "=== Grafana Dashboard Pusher ==="
echo "Target: $GRAFANA_URL"
echo "Dashboards: $DASHBOARDS_DIR"
echo "Folder: $FOLDER_TITLE"
echo ""

if [ ! -d "$DASHBOARDS_DIR" ]; then
  echo "ERROR: dashboards directory not found: $DASHBOARDS_DIR" >&2
  exit 1
fi

if [ "${DRY_RUN:-}" = "1" ]; then
  echo "DRY RUN: validating dashboards only (no Grafana API calls)"
  for DASHBOARD_FILE in "$DASHBOARDS_DIR"/*.json; do
    FILENAME=$(basename "$DASHBOARD_FILE")
    # Validate JSON is parseable and can be wrapped into the API payload shape.
    jq -e '.' "$DASHBOARD_FILE" >/dev/null
    jq -n --slurpfile dash "$DASHBOARD_FILE" --arg folderUid "dry-run" '{
      dashboard: $dash[0],
      folderUid: $folderUid,
      overwrite: true,
      message: "Updated via push-dashboards.sh"
    }' >/dev/null
    echo "DRY RUN: would push $FILENAME"
  done
  exit 0
fi

echo "Testing connection..."
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 \
  -H "Authorization: Bearer $GRAFANA_SA_TOKEN" \
  "$GRAFANA_URL/api/health")

if [ "$HTTP_CODE" != "200" ]; then
  echo "ERROR: Cannot reach Grafana API (HTTP $HTTP_CODE)" >&2
  exit 1
fi
echo "Connection OK"
echo ""

echo "Ensuring folder exists..."
FOLDER_UID=$(curl -s --max-time 10 \
  -H "Authorization: Bearer $GRAFANA_SA_TOKEN" \
  "$GRAFANA_URL/api/folders" | jq -r --arg t "$FOLDER_TITLE" '.[] | select(.title==$t) | .uid' | head -n 1)

if [ -z "${FOLDER_UID:-}" ] || [ "$FOLDER_UID" = "null" ]; then
  CREATE_RESP=$(curl -s -w "\n%{http_code}" --max-time 10 \
    -X POST \
    -H "Authorization: Bearer $GRAFANA_SA_TOKEN" \
    -H "Content-Type: application/json" \
    -d "{\"title\":\"$FOLDER_TITLE\"}" \
    "$GRAFANA_URL/api/folders")
  CREATE_CODE=$(echo "$CREATE_RESP" | tail -1)
  CREATE_BODY=$(echo "$CREATE_RESP" | sed '$d')
  if [ "$CREATE_CODE" != "200" ]; then
    echo "ERROR: Failed to create folder (HTTP $CREATE_CODE): $CREATE_BODY" >&2
    exit 1
  fi
  FOLDER_UID=$(echo "$CREATE_BODY" | jq -r '.uid')
fi

echo "Folder UID: $FOLDER_UID"
echo ""

ERRORS=0
for DASHBOARD_FILE in "$DASHBOARDS_DIR"/*.json; do
  FILENAME=$(basename "$DASHBOARD_FILE")
  echo "Pushing $FILENAME ..."

  PAYLOAD=$(jq -n --slurpfile dash "$DASHBOARD_FILE" --arg folderUid "$FOLDER_UID" '{
    dashboard: $dash[0],
    folderUid: $folderUid,
    overwrite: true,
    message: "Updated via push-dashboards.sh"
  }')

  RESPONSE=$(curl -s -w "\n%{http_code}" --max-time 30 \
    -X POST \
    -H "Authorization: Bearer $GRAFANA_SA_TOKEN" \
    -H "Content-Type: application/json" \
    -d "$PAYLOAD" \
    "$GRAFANA_URL/api/dashboards/db")

  CODE=$(echo "$RESPONSE" | tail -1)
  BODY=$(echo "$RESPONSE" | sed '$d')

  if [ "$CODE" = "200" ]; then
    URL=$(echo "$BODY" | jq -r '.url // "unknown"')
    echo "  OK -> $GRAFANA_URL$URL"
  else
    echo "  FAILED (HTTP $CODE): $BODY"
    ERRORS=$((ERRORS + 1))
  fi
done

echo ""
if [ "$ERRORS" -gt 0 ]; then
  echo "Done with $ERRORS error(s)" >&2
  exit 1
fi

echo "All dashboards pushed successfully"
