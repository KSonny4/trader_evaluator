SHELL := /bin/bash

.PHONY: build test build-linux deploy check status check-tables skills-sync

# === Local enforcement ===
skills-sync:
	./scripts/check_skills_sync.sh

# === Build ===
build:
	cargo build --release

test: skills-sync
	cargo test --all
	cargo clippy --all-targets -- -D warnings
	cargo fmt --check

build-linux:
	cargo build --release --target x86_64-unknown-linux-musl

# === Deploy ===
SERVER ?= ubuntu@YOUR_SERVER_IP
REMOTE_DIR ?= /opt/evaluator
DB = $(REMOTE_DIR)/data/evaluator.db
DB_CMD = ssh $(SERVER) 'sqlite3 $(DB)'

deploy: test build-linux
	scp target/x86_64-unknown-linux-musl/release/evaluator $(SERVER):$(REMOTE_DIR)/evaluator.new
	scp target/x86_64-unknown-linux-musl/release/web $(SERVER):$(REMOTE_DIR)/web.new
	scp config/default.toml $(SERVER):$(REMOTE_DIR)/config/default.toml
	ssh $(SERVER) 'mv $(REMOTE_DIR)/evaluator.new $(REMOTE_DIR)/evaluator && mv $(REMOTE_DIR)/web.new $(REMOTE_DIR)/web && sudo systemctl restart evaluator && sudo systemctl restart web'
	@echo "Deployed. Waiting 10s for startup..."
	@sleep 10
	$(MAKE) check

# === Sanity Checks (the single source of truth) ===
check: check-tables
	@echo "=== Basic sanity check passed ==="

check-phase-0: check-tables
	@echo "=== Phase 0: Foundation ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM sqlite_master WHERE type='table'" | \
		awk '{if ($$1 < 12) {print "FAIL: only " $$1 " tables (need 12+)"; exit 1} else print "OK: " $$1 " tables"}'
	@echo "Phase 0: PASSED"

check-phase-1: check-phase-0
	@echo "=== Phase 1: Market Discovery ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM markets" | \
		awk '{if ($$1 == 0) {print "FAIL: no markets"; exit 1} else print "OK: " $$1 " markets"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM market_scores_daily WHERE score_date = date('now')" | \
		awk '{if ($$1 == 0) {print "FAIL: no market scores today"; exit 1} else print "OK: " $$1 " market scores today"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM market_scores_daily WHERE score_date = date('now') AND rank <= 20" | \
		awk '{print "OK: " $$1 " top-20 markets selected"}'
	@echo "Phase 1: PASSED"

check-phase-2: check-phase-1
	@echo "=== Phase 2: Wallet Discovery ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM wallets" | \
		awk '{if ($$1 == 0) {print "FAIL: no wallets"; exit 1} else print "OK: " $$1 " wallets discovered"}'
	@$(DB_CMD) "SELECT COUNT(DISTINCT discovered_from) FROM wallets" | \
		awk '{if ($$1 < 2) {print "WARN: only " $$1 " discovery sources"} else print "OK: " $$1 " discovery sources"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM wallet_exclusions" | \
		awk '{print "OK: " $$1 " exclusion decisions recorded"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM wallet_personas" | \
		awk '{if ($$1 == 0) {print "FAIL: no personas classified"; exit 1} else print "OK: " $$1 " wallets classified"}'
	@echo "Phase 2: PASSED"

check-phase-3: check-phase-2
	@echo "=== Phase 3: Long-Term Tracking ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM trades_raw" | \
		awk '{if ($$1 == 0) {print "FAIL: no trades ingested"; exit 1} else print "OK: " $$1 " trades"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM activity_raw" | \
		awk '{print "OK: " $$1 " activity events"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM positions_snapshots" | \
		awk '{if ($$1 == 0) {print "FAIL: no position snapshots"; exit 1} else print "OK: " $$1 " position snapshots"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM holders_snapshots" | \
		awk '{print "OK: " $$1 " holder snapshots"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM raw_api_responses" | \
		awk '{print "OK: " $$1 " raw API responses saved"}'
	@$(DB_CMD) "SELECT CAST((julianday('now') - julianday(MAX(ingested_at))) * 24 AS INTEGER) FROM trades_raw" | \
		awk '{if ($$1 > 2) {print "WARN: trades " $$1 "h stale (target <2h)"} else print "OK: trades " $$1 "h fresh"}'
	@echo "Phase 3: PASSED"

check-phase-4: check-phase-3
	@echo "=== Phase 4: Paper Trading ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM paper_trades" | \
		awk '{if ($$1 == 0) {print "FAIL: no paper trades"; exit 1} else print "OK: " $$1 " paper trades"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM paper_events" | \
		awk '{print "OK: " $$1 " paper events (gate checks, skips, breakers)"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM follower_slippage" | \
		awk '{print "OK: " $$1 " slippage measurements"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM book_snapshots" | \
		awk '{print "OK: " $$1 " book snapshots"}'
	@echo "Phase 4: PASSED"

check-phase-5: check-phase-4
	@echo "=== Phase 5: Wallet Ranking ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM wallet_scores_daily WHERE score_date = date('now')" | \
		awk '{if ($$1 == 0) {print "FAIL: no wallet scores today"; exit 1} else print "OK: " $$1 " wallet scores today"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM wallet_scores_daily WHERE score_date = date('now') AND recommended_follow_mode IS NOT NULL" | \
		awk '{print "OK: " $$1 " wallets with follow-mode recommendation"}'
	@echo "Phase 5: PASSED"

check-tables:
	@$(DB_CMD) ".tables" | grep -q markets || (echo "FAIL: markets table missing" && exit 1)
	@$(DB_CMD) ".tables" | grep -q wallets || (echo "FAIL: wallets table missing" && exit 1)
	@$(DB_CMD) ".tables" | grep -q trades_raw || (echo "FAIL: trades_raw table missing" && exit 1)
	@$(DB_CMD) ".tables" | grep -q raw_api_responses || (echo "FAIL: raw_api_responses table missing" && exit 1)
	@echo "OK: core tables exist"

# === Status (human-friendly pipeline overview) ===
status:
	@echo "=== Pipeline Status ==="
	@$(DB_CMD) "SELECT 'markets:            ' || COUNT(*) FROM markets"
	@$(DB_CMD) "SELECT 'market scores today: ' || COUNT(*) FROM market_scores_daily WHERE score_date = date('now')"
	@$(DB_CMD) "SELECT 'wallets:            ' || COUNT(*) FROM wallets"
	@$(DB_CMD) "SELECT 'wallet personas:    ' || COUNT(*) FROM wallet_personas"
	@$(DB_CMD) "SELECT 'wallet exclusions:  ' || COUNT(*) FROM wallet_exclusions"
	@$(DB_CMD) "SELECT 'trades:             ' || COUNT(*) FROM trades_raw"
	@$(DB_CMD) "SELECT 'activities:         ' || COUNT(*) FROM activity_raw"
	@$(DB_CMD) "SELECT 'position snapshots: ' || COUNT(*) FROM positions_snapshots"
	@$(DB_CMD) "SELECT 'holder snapshots:   ' || COUNT(*) FROM holders_snapshots"
	@$(DB_CMD) "SELECT 'raw API responses:  ' || COUNT(*) FROM raw_api_responses"
	@$(DB_CMD) "SELECT 'paper trades:       ' || COUNT(*) FROM paper_trades"
	@$(DB_CMD) "SELECT 'paper events:       ' || COUNT(*) FROM paper_events"
	@$(DB_CMD) "SELECT 'book snapshots:     ' || COUNT(*) FROM book_snapshots"
	@$(DB_CMD) "SELECT 'follower slippage:  ' || COUNT(*) FROM follower_slippage"
	@$(DB_CMD) "SELECT 'wallet scores today:' || COUNT(*) FROM wallet_scores_daily WHERE score_date = date('now')"
	@$(DB_CMD) "SELECT 'last trade ingested: ' || COALESCE(MAX(ingested_at), 'never') FROM trades_raw"
	@$(DB_CMD) "SELECT 'DB size:            ' || (page_count * page_size / 1024 / 1024) || ' MB' FROM pragma_page_count(), pragma_page_size()"

