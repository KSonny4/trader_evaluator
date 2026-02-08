#!/usr/bin/env bash
set -euo pipefail

# Purge raw_api_responses and reclaim disk space on the evaluator server.
# Run on server: nohup bash /tmp/purge-raw.sh > /tmp/purge-raw.log 2>&1 &
# Monitor:       tail -f /tmp/purge-raw.log

DB="/opt/evaluator/data/evaluator.db"
LOG="/tmp/purge-raw.log"

echo "$(date) — Starting purge of raw_api_responses"

# 1. Stop evaluator so nothing holds the DB
echo "$(date) — Stopping evaluator..."
sudo systemctl stop evaluator
sleep 2
if sudo systemctl is-active --quiet evaluator; then
    echo "$(date) — FAIL: evaluator still running after stop"
    exit 1
fi

# 2. Kill any lingering sqlite3 processes on this DB
echo "$(date) — Killing lingering sqlite3 processes..."
sudo fuser -k "$DB" 2>/dev/null || true
sudo fuser -k "$DB-wal" 2>/dev/null || true
sleep 2

# 3. Count rows before
COUNT=$(sudo -u evaluator sqlite3 "$DB" "SELECT COUNT(*) FROM raw_api_responses;")
echo "$(date) — raw_api_responses has $COUNT rows"

# 4. Delete all rows
echo "$(date) — Deleting all rows from raw_api_responses..."
sudo -u evaluator sqlite3 "$DB" "DELETE FROM raw_api_responses;"
echo "$(date) — Delete complete"

# 5. Checkpoint WAL to fold it back into main DB
echo "$(date) — Checkpointing WAL..."
sudo -u evaluator sqlite3 "$DB" "PRAGMA wal_checkpoint(TRUNCATE);"
echo "$(date) — WAL checkpoint complete"

# 6. Show sizes after cleanup (skip VACUUM — too slow on t3.micro with 1GB RAM)
echo "$(date) — DB files after cleanup:"
ls -lh "$DB"*

# 7. Restart evaluator
echo "$(date) — Starting evaluator..."
sudo systemctl start evaluator
sleep 2
sudo systemctl status evaluator --no-pager | head -5

echo "$(date) — Done! Disk space reclaimed (VACUUM skipped — freed space reused internally by SQLite)."
echo "$(date) — If you want to shrink the file on disk too, run later when server is idle:"
echo "           sudo systemctl stop evaluator && sudo -u evaluator sqlite3 $DB 'VACUUM;' && sudo systemctl start evaluator"
