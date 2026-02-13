SHELL := /bin/bash

.PHONY: build test build-linux deploy check status check-tables skills-sync setup-hooks coverage worktree worktree-clean check-file-length
.PHONY: todo todo-check todo-test reset-and-run

# === Local enforcement ===
setup-hooks:
	ln -sf ../../hooks/pre-push .git/hooks/pre-push
	ln -sf ../../hooks/pre-commit .git/hooks/pre-commit
	@echo "Git hooks installed:"
	@echo "  pre-push:   blocks direct pushes to main"
	@echo "  pre-commit: blocks feature/* commits outside worktrees"

skills-sync:
	./scripts/check_skills_sync.sh

# === TODO guardrails ===
todo:
	python3 ./scripts/todo_guard.py list

todo-check:
	python3 ./scripts/todo_guard.py check

todo-test:
	python3 -m unittest -q scripts.tests.test_todo_guard

# === Coverage ===
coverage:
	cargo llvm-cov --workspace --fail-under-lines 70
	@echo "Coverage passed (>= 70% line coverage)"

# === Git worktrees ===
worktree:
	@if [ -z "$(NAME)" ]; then echo "Usage: make worktree NAME=<feature-name>"; exit 1; fi
	@git worktree add .worktrees/$(NAME) -b feature/$(NAME)
	@echo "Worktree created at .worktrees/$(NAME) on branch feature/$(NAME)"
	@echo "cd .worktrees/$(NAME) to start working"

worktree-clean:
	@if [ -z "$(NAME)" ]; then echo "Usage: make worktree-clean NAME=<feature-name>"; exit 1; fi
	git worktree remove .worktrees/$(NAME) --force 2>/dev/null || true
	git branch -D feature/$(NAME) 2>/dev/null || true
	@echo "Cleaned up worktree and branch for $(NAME)"

# === Architecture enforcement ===
MAX_FILE_LINES ?= 500
# Files that currently exceed the limit — each must have a tracking issue to split them.
# Remove entries as files are refactored below the threshold.
OVERLENGTH_ALLOWLIST := \
	crates/common/src/config.rs \
	crates/evaluator/src/jobs/pipeline_jobs.rs \
	crates/web/src/main.rs \
	crates/web/src/models.rs \
	crates/web/src/queries.rs \
	crates/evaluator/src/ingestion.rs \
	crates/evaluator/src/paper_trading.rs \
	crates/evaluator/src/persona_classification.rs \
	crates/evaluator/src/wallet_features.rs \
	crates/common/src/db.rs \
	crates/common/src/polymarket.rs

check-file-length:
	@echo "=== Checking .rs files for >$(MAX_FILE_LINES) lines ==="
	@fail=0; \
	for f in $$(find crates/ -name '*.rs' -not -path '*/target/*'); do \
		lines=$$(wc -l < "$$f"); \
		if [ "$$lines" -gt $(MAX_FILE_LINES) ]; then \
			allowed=0; \
			for a in $(OVERLENGTH_ALLOWLIST); do \
				if [ "$$f" = "$$a" ]; then allowed=1; break; fi; \
			done; \
			if [ "$$allowed" -eq 1 ]; then \
				echo "WARN: $$f has $$lines lines (allowlisted — needs refactoring)"; \
			else \
				echo "FAIL: $$f has $$lines lines (max $(MAX_FILE_LINES))"; \
				fail=1; \
			fi; \
		fi; \
	done; \
	if [ "$$fail" -eq 1 ]; then \
		echo ""; \
		echo "Files exceed $(MAX_FILE_LINES) line limit. Split them into modules."; \
		echo "If this is a known issue, add to OVERLENGTH_ALLOWLIST in Makefile."; \
		exit 1; \
	fi
	@echo "OK: all files within $(MAX_FILE_LINES) line limit (or allowlisted)"

# === Build ===
build:
	cargo build --release

test: skills-sync check-file-length
	cargo test --all
	cargo clippy --all-targets -- -D warnings
	cargo fmt --check

build-linux:
	cross build --release --target x86_64-unknown-linux-musl

# === Deploy ===
# Default deploy target is discovered via AWS (tag Name=trading-bot).
# Override with `SERVER=ubuntu@x.x.x.x` or `TRADING_SERVER_IP=x.x.x.x`.
SERVER ?= $(shell ./scripts/aws_find_server.sh)
SSH_KEY ?= ~/git_projects/trading/trading-bot.pem
REMOTE_DIR ?= /opt/evaluator
DB = $(REMOTE_DIR)/data/evaluator.db
SSH = ssh -i $(SSH_KEY)
SCP = scp -i $(SSH_KEY)
DB_CMD = $(SSH) $(SERVER) 'sqlite3 $(DB)'

deploy: test build-linux
	$(SCP) target/x86_64-unknown-linux-musl/release/evaluator $(SERVER):/tmp/evaluator.new
	$(SCP) target/x86_64-unknown-linux-musl/release/web $(SERVER):/tmp/web.new
	$(SCP) config/default.toml $(SERVER):/tmp/default.toml
	$(SSH) $(SERVER) 'sudo mv /tmp/evaluator.new $(REMOTE_DIR)/evaluator && sudo mv /tmp/web.new $(REMOTE_DIR)/web && sudo mv /tmp/default.toml $(REMOTE_DIR)/config/default.toml && sudo chown evaluator:evaluator $(REMOTE_DIR)/evaluator $(REMOTE_DIR)/web $(REMOTE_DIR)/config/default.toml || true && sudo systemctl restart evaluator && sudo systemctl restart web'
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
	@echo "=== Phase 1: Event Discovery ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM markets" | \
		awk '{if ($$1 == 0) {print "FAIL: no markets"; exit 1} else print "OK: " $$1 " markets"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM market_scores WHERE score_date = date('now')" | \
		awk '{if ($$1 == 0) {print "FAIL: no market scores today"; exit 1} else print "OK: " $$1 " market scores today"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM market_scores WHERE score_date = date('now') AND rank <= 50" | \
		awk '{print "OK: " $$1 " top-50 events selected"}'
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
	@$(DB_CMD) "SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='paper_events') THEN (SELECT COUNT(*) FROM paper_events) ELSE -1 END" | \
		awk '{if ($$1 == -1) print "SKIP: paper_events table not yet created"; else print "OK: " $$1 " paper events (gate checks, skips, breakers)"}'
	@$(DB_CMD) "SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='follower_slippage') THEN (SELECT COUNT(*) FROM follower_slippage) ELSE -1 END" | \
		awk '{if ($$1 == -1) print "SKIP: follower_slippage table not yet created"; else print "OK: " $$1 " slippage measurements"}'
	@$(DB_CMD) "SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='book_snapshots') THEN (SELECT COUNT(*) FROM book_snapshots) ELSE -1 END" | \
		awk '{if ($$1 == -1) print "SKIP: book_snapshots table not yet created"; else print "OK: " $$1 " book snapshots"}'
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

# === Local run ===
# Start evaluator in background, then web in foreground. Ctrl+C stops web (evaluator keeps running).
LOCAL_DB ?= data/evaluator.db

run:
	@mkdir -p data
	@echo "Starting evaluator in background..."
	@cargo run -p evaluator &
	@sleep 3
	@echo "Starting web (Ctrl+C to stop)..."
	@cargo run -p web

# === Local reset and run ===
# Keep DB, delete everything except markets and wallets, then start evaluator and web.
reset-and-run:
	@mkdir -p data
	@./scripts/reset_db_keep_markets_wallets.sh $(LOCAL_DB)
	@echo "Starting evaluator in background..."
	@cargo run -p evaluator &
	@sleep 3
	@echo "Starting web (Ctrl+C to stop)..."
	@cargo run -p web

# === Status (human-friendly pipeline overview) ===
status:
	@echo "=== Pipeline Status ==="
	@$(DB_CMD) "SELECT 'markets:            ' || COUNT(*) FROM markets"
	@$(DB_CMD) "SELECT 'market scores today: ' || COUNT(*) FROM market_scores WHERE score_date = date('now')"
	@$(DB_CMD) "SELECT 'wallets:            ' || COUNT(*) FROM wallets"
	@$(DB_CMD) "SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='wallet_personas') THEN 'wallet personas:    ' || (SELECT COUNT(*) FROM wallet_personas) ELSE 'wallet personas:    table not created' END"
	@$(DB_CMD) "SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='wallet_exclusions') THEN 'wallet exclusions:  ' || (SELECT COUNT(*) FROM wallet_exclusions) ELSE 'wallet exclusions:  table not created' END"
	@$(DB_CMD) "SELECT 'trades:             ' || COUNT(*) FROM trades_raw"
	@$(DB_CMD) "SELECT 'activities:         ' || COUNT(*) FROM activity_raw"
	@$(DB_CMD) "SELECT 'position snapshots: ' || COUNT(*) FROM positions_snapshots"
	@$(DB_CMD) "SELECT 'holder snapshots:   ' || COUNT(*) FROM holders_snapshots"
	@$(DB_CMD) "SELECT 'raw API responses:  ' || COUNT(*) FROM raw_api_responses"
	@$(DB_CMD) "SELECT 'paper trades:       ' || COUNT(*) FROM paper_trades"
	@$(DB_CMD) "SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='paper_events') THEN 'paper events:       ' || (SELECT COUNT(*) FROM paper_events) ELSE 'paper events:       table not created' END"
	@$(DB_CMD) "SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='book_snapshots') THEN 'book snapshots:     ' || (SELECT COUNT(*) FROM book_snapshots) ELSE 'book snapshots:     table not created' END"
	@$(DB_CMD) "SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='follower_slippage') THEN 'follower slippage:  ' || (SELECT COUNT(*) FROM follower_slippage) ELSE 'follower slippage:  table not created' END"
	@$(DB_CMD) "SELECT 'wallet scores today:' || COUNT(*) FROM wallet_scores_daily WHERE score_date = date('now')"
	@$(DB_CMD) "SELECT 'last trade ingested: ' || COALESCE(MAX(ingested_at), 'never') FROM trades_raw"
	@$(DB_CMD) "SELECT 'DB size:            ' || (page_count * page_size / 1024 / 1024) || ' MB' FROM pragma_page_count(), pragma_page_size()"

# === Reclassify (after persona logic changes) ===
# Clears persona state so the next pipeline run re-evaluates all wallets with current code/config.

# Local: clear tables in data/evaluator.db. Restart evaluator (and web) yourself if they're running.
reclassify-local:
	@test -f $(LOCAL_DB) || (echo "DB not found: $(LOCAL_DB)"; exit 1)
	@sqlite3 $(LOCAL_DB) "DELETE FROM wallet_exclusions; DELETE FROM wallet_personas; DELETE FROM wallet_persona_traits;" 2>/dev/null || true
	@echo "Cleared persona state in $(LOCAL_DB). Restart evaluator (and web) so next run reclassifies."

# Server: run prod script (stops services, clears, starts services).
reclassify:
	@echo "Server: sudo bash /opt/evaluator/scripts/prod_clear_persona_state.sh"
	@echo "Local:  make reclassify-local   (then restart evaluator/web if running)"
