#!/usr/bin/env bash
# Run ON THE SERVER as root or with sudo.
# 1) Stops evaluator and web (so no process holds the DB).
# 2) Backs up /opt/evaluator/data/evaluator.db to evaluator.db.bak.<date>
# 3) Resets DB: keeps ONLY wallets. Clears markets, market_scores, trades_raw,
#    activity_raw, and all derived tables so bootstrap repopulates events and
#    wallet_discovery can add new wallets from top events.
# 4) Starts evaluator and web.
#
# Usage on server:
#   sudo bash /opt/evaluator/scripts/prod_backup_and_reset_keep_wallets_only.sh
#
# After run: bootstrap will repopulate events (market_scores, markets) and
# wallet_discovery will add new wallets; existing wallets are preserved.

set -e

DB="/opt/evaluator/data/evaluator.db"
BACKUP="${DB}.bak.$(date +%Y%m%d-%H%M%S)"

if [[ ! -f "$DB" ]]; then
  echo "DB not found: $DB"
  exit 1
fi

echo "=== 1) Stop evaluator and web ==="
sudo systemctl stop evaluator
sudo systemctl stop web
sleep 3

echo "=== 2) Backup ==="
sudo -u evaluator cp "$DB" "$BACKUP"
echo "Backed up to $BACKUP"
ls -la /opt/evaluator/data/

echo "=== 3) Reset DB (keep ONLY wallets) ==="
# Clear everything except wallets so we repopulate events and get new wallets from API
for t in markets market_scores scoring_stats discovery_scheduler_state \
  trades_raw activity_raw raw_api_responses \
  copy_fidelity_events follower_slippage wallet_rules_events wallet_rules_state \
  wallet_persona_traits wallet_exclusions wallet_personas wallet_scores_daily \
  paper_positions paper_trades wallet_features_daily \
  holders_snapshots positions_snapshots; do
  sudo -u evaluator sqlite3 "$DB" "DELETE FROM $t;" 2>/dev/null || true
done
# Handle old schema name if migration not yet run
sudo -u evaluator sqlite3 "$DB" "DELETE FROM market_scores_daily;" 2>/dev/null || true

echo "VACUUM (may take several minutes on large DB)..."
sudo -u evaluator sqlite3 "$DB" "VACUUM;"
echo "Reset done."

echo "=== 4) Start evaluator and web ==="
sudo systemctl start evaluator
sudo systemctl start web
sleep 3
sudo systemctl is-active evaluator && echo "evaluator is active" || (echo "evaluator failed to start"; exit 1)
sudo systemctl is-active web && echo "web is active" || (echo "web failed to start"; exit 1)

WALLETS=$(sudo -u evaluator sqlite3 "$DB" "SELECT COUNT(*) FROM wallets;" 2>/dev/null || echo "0")
echo "Done. wallets kept: $WALLETS (events and rest will repopulate from API)"
