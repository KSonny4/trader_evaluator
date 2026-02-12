#!/usr/bin/env bash
# Reset DB: delete all data except markets and wallets.
# Usage: ./scripts/reset_db_keep_markets_wallets.sh [db_path]
# Default: data/evaluator.db

set -e

DB="${1:-data/evaluator.db}"

if [[ ! -f "$DB" ]]; then
  echo "DB not found: $DB"
  exit 1
fi

echo "Resetting DB (keeping markets, wallets, market_scores, scoring_stats): $DB"

# Tables to clear (everything except markets, wallets, market_scores, scoring_stats, trades_raw, activity_raw)
# market_scores + scoring_stats kept so funnel "Events" stage shows count after reset
# trades_raw + activity_raw kept so persona classification can run (needs trade age and counts for Stage 1)
for t in copy_fidelity_events follower_slippage wallet_rules_events wallet_rules_state \
  wallet_persona_traits wallet_exclusions wallet_personas wallet_scores_daily \
  paper_positions paper_trades wallet_features_daily \
  holders_snapshots positions_snapshots raw_api_responses; do
  sqlite3 "$DB" "DELETE FROM $t;" 2>/dev/null || true
done

# VACUUM to reclaim space
sqlite3 "$DB" "VACUUM;"

EVENTS=$(sqlite3 "$DB" "
  SELECT COUNT(DISTINCT COALESCE(m.event_slug, m.condition_id))
  FROM market_scores ms
  JOIN markets m ON m.condition_id = ms.condition_id
  WHERE ms.score_date = (SELECT MAX(score_date) FROM market_scores)
")
TRADES=$(sqlite3 "$DB" "SELECT COUNT(*) FROM trades_raw;" 2>/dev/null || echo "0")
echo "Done. markets: $(sqlite3 "$DB" 'SELECT COUNT(*) FROM markets'), wallets: $(sqlite3 "$DB" 'SELECT COUNT(*) FROM wallets'), events kept: $EVENTS, trades kept: $TRADES"
