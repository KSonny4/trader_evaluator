#!/usr/bin/env bash
set -euo pipefail

# Move dashboards that don't belong in the trader_evaluator Grafana folder.
#
# This exists because it's easy to accidentally upload dashboards from another repo
# when using a shared `.env.agent` or shell environment.
#
# Default behavior is DRY RUN. To apply changes, set APPLY=1.
#
# Usage:
#   source .env.agent
#   ./deploy/cleanup-grafana-folder-mixup.sh
#
# Environment:
#   GRAFANA_URL
#   GRAFANA_SA_TOKEN
# Optional:
#   SRC_FOLDER_TITLE   (default: trader-evaluator)
#   DST_FOLDER_TITLE   (default: trading)
#   APPLY=1            to actually move dashboards

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

SRC_FOLDER_TITLE="${SRC_FOLDER_TITLE:-trader-evaluator}"
DST_FOLDER_TITLE="${DST_FOLDER_TITLE:-trading}"

if [ -z "${GRAFANA_URL:-}" ] || [ -z "${GRAFANA_SA_TOKEN:-}" ]; then
  if [ -f "$PROJECT_DIR/.env.agent" ]; then
    # shellcheck disable=SC1091
    source "$PROJECT_DIR/.env.agent"
  fi
fi

: "${GRAFANA_URL:?Set GRAFANA_URL (e.g. https://your-slug.grafana.net)}"
: "${GRAFANA_SA_TOKEN:?Set GRAFANA_SA_TOKEN (Grafana Service Account token)}"

GRAFANA_URL="${GRAFANA_URL%/}"

curl_json() {
  curl -sS --max-time 30 \
    -H "Authorization: Bearer $GRAFANA_SA_TOKEN" \
    -H "Content-Type: application/json" \
    "$@"
}

ensure_folder() {
  local title="$1"
  local folder_json
  folder_json=$(curl_json "$GRAFANA_URL/api/folders" | jq -c --arg t "$title" '.[] | select(.title==$t) | {id,uid,title}' | head -n 1 || true)
  if [ -n "$folder_json" ]; then
    echo "$folder_json"
    return 0
  fi

  local create_resp create_code create_body
  create_resp=$(curl -sS -w "\n%{http_code}" --max-time 30 \
    -X POST \
    -H "Authorization: Bearer $GRAFANA_SA_TOKEN" \
    -H "Content-Type: application/json" \
    -d "{\"title\":\"$title\"}" \
    "$GRAFANA_URL/api/folders")
  create_code=$(echo "$create_resp" | tail -1)
  create_body=$(echo "$create_resp" | sed '$d')
  if [ "$create_code" != "200" ]; then
    echo "ERROR: failed to create folder '$title' (HTTP $create_code): $create_body" >&2
    exit 1
  fi
  echo "$create_body" | jq -c '{id,uid,title}'
}

echo "=== Grafana Folder Cleanup (mixup) ==="
echo "Grafana: $GRAFANA_URL"
echo "Source folder: $SRC_FOLDER_TITLE"
echo "Dest folder: $DST_FOLDER_TITLE"
echo "Mode: ${APPLY:-0}"
echo ""

SRC_FOLDER=$(ensure_folder "$SRC_FOLDER_TITLE")
DST_FOLDER=$(ensure_folder "$DST_FOLDER_TITLE")

SRC_UID=$(echo "$SRC_FOLDER" | jq -r '.uid')
DST_UID=$(echo "$DST_FOLDER" | jq -r '.uid')

echo "Source folder uid=$SRC_UID"
echo "Dest folder uid=$DST_UID"
echo ""

# NOTE: In Grafana Cloud, folder IDs can be very large (64-bit) and the search
# API's `folderIds=` filter can behave unexpectedly. `folderUIDs=` works reliably.
SEARCH=$(curl_json "$GRAFANA_URL/api/search?type=dash-db&folderUIDs=$SRC_UID&limit=5000")
COUNT=$(echo "$SEARCH" | jq 'length')
echo "Found $COUNT dashboards in '$SRC_FOLDER_TITLE'"

MOVE=0
SKIP=0

while IFS= read -r row; do
  uid=$(echo "$row" | jq -r '.uid')
  title=$(echo "$row" | jq -r '.title')
  tags=$(echo "$row" | jq -c '.tags // []')

  # Heuristic: dashboards tagged "trading" don't belong under trader-evaluator.
  should_move=$(echo "$tags" | jq 'index("trading") != null')
  if [ "$should_move" != "true" ]; then
    SKIP=$((SKIP + 1))
    continue
  fi

  MOVE=$((MOVE + 1))
  echo "Move: $title (uid=$uid) tags=$tags -> folder '$DST_FOLDER_TITLE'"

  if [ "${APPLY:-0}" != "1" ]; then
    continue
  fi

  dash=$(curl_json "$GRAFANA_URL/api/dashboards/uid/$uid" | jq '.dashboard')
  payload=$(jq -n --arg folderUid "$DST_UID" --arg message "Moved by cleanup-grafana-folder-mixup.sh" --argjson dash "$dash" '{
    dashboard: $dash,
    folderUid: $folderUid,
    overwrite: true,
    message: $message
  }')

  resp=$(curl -sS -w "\n%{http_code}" --max-time 30 \
    -X POST \
    -H "Authorization: Bearer $GRAFANA_SA_TOKEN" \
    -H "Content-Type: application/json" \
    -d "$payload" \
    "$GRAFANA_URL/api/dashboards/db")
  code=$(echo "$resp" | tail -1)
  body=$(echo "$resp" | sed '$d')

  if [ "$code" != "200" ]; then
    echo "  FAILED (HTTP $code): $body" >&2
    exit 1
  fi
done < <(echo "$SEARCH" | jq -c '.[]')

echo ""
echo "Summary:"
echo "  to-move (tag=trading): $MOVE"
echo "  kept: $SKIP"
if [ "${APPLY:-0}" != "1" ]; then
  echo ""
  echo "Dry-run only. Re-run with APPLY=1 to move dashboards."
fi
