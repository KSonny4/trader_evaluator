#!/usr/bin/env bash
# Run ON THE SERVER as root or with sudo.
# 1) Backs up /opt/evaluator/data/evaluator.db to evaluator.db.bak.<date>
# 2) Stops evaluator
# 3) Resets DB (keep markets, wallets, market_scores, scoring_stats, trades_raw, activity_raw)
# 4) Starts evaluator
#
# Usage on server:
#   sudo bash /opt/evaluator/scripts/prod_backup_and_reset_db.sh
# Or copy this script to the server and run it.

set -e

DB="/opt/evaluator/data/evaluator.db"
BACKUP="${DB}.bak.$(date +%Y%m%d-%H%M%S)"

if [[ ! -f "$DB" ]]; then
  echo "DB not found: $DB"
  exit 1
fi

echo "=== 1) Backup ==="
sudo -u evaluator cp "$DB" "$BACKUP"
echo "Backed up to $BACKUP"
ls -la /opt/evaluator/data/

echo "=== 2) Stop evaluator ==="
sudo systemctl stop evaluator
sleep 2

echo "=== 3) Reset DB (keep markets, wallets, market_scores, scoring_stats, trades_raw, activity_raw) ==="
for t in copy_fidelity_events follower_slippage wallet_rules_events wallet_rules_state \
  wallet_persona_traits wallet_exclusions wallet_personas wallet_scores_daily \
  paper_positions paper_trades wallet_features_daily \
  holders_snapshots positions_snapshots raw_api_responses; do
  sudo -u evaluator sqlite3 "$DB" "DELETE FROM $t;" 2>/dev/null || true
done
sudo -u evaluator sqlite3 "$DB" "VACUUM;"
echo "Reset done."

echo "=== 4) Start evaluator ==="
sudo systemctl start evaluator
sleep 2
sudo systemctl is-active evaluator && echo "evaluator is active" || (echo "evaluator failed to start"; exit 1)

# Report: table may be market_scores (after migration) or market_scores_daily (old)
EVENTS=$(sudo -u evaluator sqlite3 "$DB" "SELECT COUNT(DISTINCT COALESCE(m.event_slug, m.condition_id)) FROM market_scores ms JOIN markets m ON m.condition_id = ms.condition_id WHERE ms.score_date = (SELECT MAX(score_date) FROM market_scores);" 2>/dev/null || sudo -u evaluator sqlite3 "$DB" "SELECT COUNT(DISTINCT COALESCE(m.event_slug, m.condition_id)) FROM market_scores_daily ms JOIN markets m ON m.condition_id = ms.condition_id WHERE ms.score_date = (SELECT MAX(score_date) FROM market_scores_daily);" 2>/dev/null || echo "0")
TRADES=$(sudo -u evaluator sqlite3 "$DB" "SELECT COUNT(*) FROM trades_raw;" 2>/dev/null || echo "0")
echo "Done. markets: $(sudo -u evaluator sqlite3 "$DB" 'SELECT COUNT(*) FROM markets'), wallets: $(sudo -u evaluator sqlite3 "$DB" 'SELECT COUNT(*) FROM wallets'), events kept: $EVENTS, trades kept: $TRADES"
