# Paper Tick Gating Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Ensure `paper_tick` only mirrors wallets that are currently followable, and stops mirroring immediately if a wallet becomes excluded.

**Architecture:** Gate `paper_tick` at selection time using DB EXISTS/NOT EXISTS checks (followable persona present, no exclusion present). Add a per-trade exclusion re-check inside the processing loop to stop mid-batch if an exclusion appears concurrently.

**Tech Stack:** Rust, tokio, rusqlite, in-process SQLite.

---

### Task 1: Add Regression Tests For Followable Gating

**Files:**
- Modify: `crates/evaluator/src/jobs/pipeline_jobs.rs`

**Step 1: Write the failing test**
- Add a test that inserts a `trades_raw` row for a wallet with no `wallet_personas` row.
- Assert `run_paper_tick_once()` inserts `0` paper trades.
- Then insert a followable persona row and assert the previously-unprocessed trade is mirrored.

**Step 2: Run test to verify it fails**
Run: `cargo test -p evaluator test_run_paper_tick_only_mirrors_followable_wallets_and_backfills_when_becoming_followable`
Expected: FAIL (paper_tick mirrors even without followable classification).

### Task 2: Add Regression Test For Immediate Stop On Exclusion

**Files:**
- Modify: `crates/evaluator/src/jobs/pipeline_jobs.rs`

**Step 1: Write the failing test**
- Insert a followable persona row for a wallet.
- Insert 2 `trades_raw` rows for that wallet.
- Create a SQLite trigger that inserts into `wallet_exclusions` after the first insert into `paper_trades`.
- Assert only the first trade is mirrored.

**Step 2: Run test to verify it fails**
Run: `cargo test -p evaluator test_run_paper_tick_stops_mirroring_immediately_when_wallet_becomes_excluded`
Expected: FAIL (paper_tick mirrors the second trade even after exclusion exists).

### Task 3: Implement Gating In `run_paper_tick_once`

**Files:**
- Modify: `crates/evaluator/src/jobs/pipeline_jobs.rs`

**Step 1: Filter selection query**
- Update the `paper_tick.select_unprocessed_trades` SQL to:
  - require `EXISTS wallet_personas` for the trade wallet
  - require `NOT EXISTS wallet_exclusions` for the trade wallet

**Step 2: Re-check exclusion per trade**
- Before mirroring each trade, query `wallet_exclusions` for that wallet.
- If excluded, skip mirroring.

**Step 3: Run full test suite**
Run: `cargo test`
Expected: PASS.
