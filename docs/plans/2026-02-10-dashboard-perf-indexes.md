# Dashboard Perf Indexes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the web dashboard and evaluator pipeline queries reliably fast on large SQLite DBs by adding missing, query-driven indexes.

**Architecture:** Add `CREATE INDEX IF NOT EXISTS ...` statements to the SQLite `SCHEMA` string in `crates/common/src/db.rs`. Verify via unit tests that migrations create the new indexes.

**Tech Stack:** Rust, rusqlite, SQLite

---

### Task 1: Add Failing Tests For New Indexes

**Files:**
- Modify: `crates/common/src/db.rs`
- Test: `crates/common/src/db.rs`

**Step 1: Write the failing test**

Add a unit test that opens an in-memory DB, runs migrations, and asserts the new index names exist in `sqlite_master`.

Expected new indexes (names may be adjusted if conflicts):
- `idx_paper_trades_triggered_by_trade_id`
- `idx_paper_trades_created_at`
- `idx_wallets_discovered_at`
- `idx_wallet_scores_date_window_wscore`
- `idx_market_scores_date_rank`
- `idx_trades_raw_ingested_at`
- `idx_activity_raw_ingested_at`
- `idx_positions_snapshots_snapshot_at`
- `idx_holders_snapshots_snapshot_at`

**Step 2: Run tests to verify it fails**

Run: `cargo test -q -p common`
Expected: FAIL because the index names are not present in the current `SCHEMA`.

### Task 2: Implement Indexes In Schema

**Files:**
- Modify: `crates/common/src/db.rs`

**Step 1: Add minimal schema changes**

Add `CREATE INDEX IF NOT EXISTS ...` statements for the new indexes.

**Step 2: Run tests to verify it passes**

Run: `cargo test -q -p common`
Expected: PASS.

### Task 3: Whole-Workspace Verification

**Files:**
- None

**Step 1: Run full test suite**

Run: `cargo test -q`
Expected: PASS.
