#!/usr/bin/env bash
# Run ON THE SERVER as root or with sudo.
# Clears persona classification state (exclusions, personas, traits) so the next
# persona run re-evaluates all wallets with the current config (e.g. 45-day window).
# Use after deploying a new threshold so "young" etc. are re-evaluated.
#
# 1) Stops evaluator (and web) so no concurrent writes.
# 2) DELETE FROM wallet_exclusions; wallet_personas; wallet_persona_traits;
# 3) Starts evaluator and web. Next persona run (hourly or on start) will reclassify everyone.
#
# Usage on server:
#   sudo bash /opt/evaluator/scripts/prod_clear_persona_state.sh

set -e

DB="/opt/evaluator/data/evaluator.db"

if [[ ! -f "$DB" ]]; then
  echo "DB not found: $DB"
  exit 1
fi

echo "=== 1) Stop evaluator and web ==="
sudo systemctl stop evaluator
sudo systemctl stop web
sleep 2

echo "=== 2) Clear persona state (exclusions, personas, traits) ==="
sudo -u evaluator sqlite3 "$DB" "DELETE FROM wallet_exclusions;"
sudo -u evaluator sqlite3 "$DB" "DELETE FROM wallet_personas;"
sudo -u evaluator sqlite3 "$DB" "DELETE FROM wallet_persona_traits;" 2>/dev/null || true
echo "Cleared."

echo "=== 3) Start evaluator and web ==="
sudo systemctl start evaluator
sudo systemctl start web
sleep 2
sudo systemctl is-active evaluator && echo "evaluator is active" || (echo "evaluator failed"; exit 1)
sudo systemctl is-active web && echo "web is active" || (echo "web failed"; exit 1)
echo "Done. Next persona run (on schedule or already triggered) will re-evaluate all wallets with current config."
