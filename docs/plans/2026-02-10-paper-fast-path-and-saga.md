# Paper Fast Path + Saga Plan (Paper Trading)

**Date:** 2026-02-10

## Summary

Implement a minimal, single-process architecture upgrade so **paper trading reacts immediately** to new wallet activity and **records decisions as a saga-like persisted flow**.

This is intentionally not a full actor system rewrite. It is a thin layer that:

- Separates a **latency-critical fast path** (paper decisions now; live execution later) from background work.
- Provides **coalescing/backpressure semantics** for the fast path (never build an unbounded tick backlog).
- Adds a **durable audit trail** (`paper_events`) and optional **persisted saga state** to make paper decisions replayable and idempotent.

Related docs:
- `docs/ARCHITECTURE.md` (target runtime: fast path, backpressure, saga even for paper)
- `docs/REFERENCE.md` (future tables: `paper_events`, `book_snapshots`, `follower_slippage`)

## Scope

In scope:
- Trigger paper decisioning immediately after ingestion inserts new trades/activity (no waiting for a periodic paper tick).
- Coalesce multiple triggers (latest state wins) so a burst of ingestion does not enqueue 1000 paper ticks.
- Persist paper decision steps as events (risk gate checks, skip reasons, created trade, settlement attempt, etc.).
- Enforce idempotency so repeated triggers do not duplicate paper trades for the same source trade.

Out of scope:
- Multi-process splitting, external queues.
- Orderbook-based slippage (`book_snapshots`) and depth-aware fills.
- Live order placement (but the fast-path design must be compatible with it).

## Design Decisions (locked)

1. **Fast path trigger mechanism:** use a coalescing wakeup (not a buffered queue).
   - Implementation: `tokio::sync::watch` (e.g. `watch::Sender<u64>` “paper_generation” counter).
   - Reason: coalesces naturally; no unbounded buffering; allows “wake once” semantics.

2. **Fallback periodic tick:** keep the existing periodic paper tick as a safety net (e.g. every 60s).
   - The fast path provides low latency when ingestion is active.
   - The periodic tick handles edge cases (missed signals, manual DB edits, etc.).

3. **Saga for paper trading:** implement as “persisted event log + idempotency.”
   - Primary durability: `paper_events` table (append-only).
   - Idempotency: enforce a uniqueness rule for paper trades per source trade.
   - Optional: a lightweight `paper_sagas` table if needed for operational visibility (recommended in this plan).

## Data Model Changes

### Table: `paper_events` (new)

Create an append-only table:
- `id INTEGER PRIMARY KEY AUTOINCREMENT`
- `created_at TEXT NOT NULL DEFAULT (datetime('now'))`
- `proxy_wallet TEXT NOT NULL`
- `condition_id TEXT NOT NULL`
- `triggered_by_trade_id INTEGER` (nullable for periodic runs; otherwise FK to `trades_raw.id`)
- `event_type TEXT NOT NULL`
  - Examples: `SAGA_STARTED`, `RISK_CHECKED`, `SKIP_RISK`, `TRADE_CREATED`, `TRADE_SETTLED`, `SAGA_FAILED`
- `details_json TEXT` (freeform JSON blob; keep stable keys once introduced)

Indexes:
- `(proxy_wallet, created_at)`
- `(triggered_by_trade_id)`

### Table: `paper_sagas` (new; recommended)

Persisted state machine per source trade:
- `id INTEGER PRIMARY KEY AUTOINCREMENT`
- `triggered_by_trade_id INTEGER NOT NULL UNIQUE` (FK to `trades_raw.id`)
- `proxy_wallet TEXT NOT NULL`
- `condition_id TEXT NOT NULL`
- `state TEXT NOT NULL`
  - States: `started`, `skipped`, `trade_created`, `settled`, `failed`
- `last_error TEXT`
- `created_at TEXT NOT NULL DEFAULT (datetime('now'))`
- `updated_at TEXT NOT NULL DEFAULT (datetime('now'))`

Reason: provides a simple “one row per saga” operational view without scanning events.

### Idempotency rule (paper trades)

Add a uniqueness constraint:
- `UNIQUE(triggered_by_trade_id, strategy)`

This prevents duplicated paper trades on repeated processing of the same source trade.

## Implementation Tasks (TDD, bite-sized)

Each task below must follow: failing test -> minimal code -> passing test -> refactor.

### Task A1: Add `paper_events` table migration + schema test
- Update: `crates/common/src/db.rs` migrations SQL.
- Add: `crates/common/src/db.rs` test asserting `paper_events` exists and has expected columns.

### Task A2: Add `paper_sagas` table migration + schema test
- Update: `crates/common/src/db.rs` migrations SQL.
- Add: schema test like above.

### Task A3: Enforce paper trade idempotency (unique constraint) + test
- Update: `crates/common/src/db.rs` migration for `paper_trades` unique constraint.
- Add: test that inserting two paper trades with same `(triggered_by_trade_id, strategy)` fails (or upsert behavior is explicit).

### Task B1: Introduce `PaperFastPath` trigger primitive (watch counter) + unit tests
- Add: `crates/evaluator/src/paper_fast_path.rs`
  - API:
    - `PaperFastPath::new() -> (PaperFastPathTx, PaperFastPathRx)`
    - `PaperFastPathTx::trigger(reason: PaperTriggerReason)` increments generation and stores last reason.
    - `PaperFastPathRx::changed().await` waits for next generation.
- Unit tests:
  - Multiple triggers before receiver polls should coalesce into “at least one wake.”
  - Receiver sees monotonically increasing generation.

### Task B2: Wire ingestion -> fast-path trigger (minimal)
- In `crates/evaluator/src/main.rs` ingestion workers:
  - When `run_trades_ingestion_once` inserts `> 0` rows, call `paper_fast_path.trigger(TradesIngested)`.
  - Optionally do the same for activity ingestion if it is needed for paper logic.
- Add unit test(s) around orchestration wiring by extracting a small “ingestion result handling” function if necessary.

### Task B3: Paper worker listens to fast-path trigger + periodic fallback
- Update the paper worker loop to:
  - Run immediately on `fast_path_rx.changed().await`, and also on the periodic scheduler tick.
  - Ensure runs are serialized (no concurrent `run_paper_tick_once`).
- Add test using Tokio time controls (paused) to ensure:
  - Trigger wakes the worker without waiting 60s.
  - Multiple triggers within a short time do not cause multiple back-to-back redundant runs (coalescing).

### Task C1: Add `paper_events` writes for each decision stage
- In `crates/evaluator/src/paper_trading.rs` (or a new `paper_saga.rs`):
  - On processing a candidate trade:
    - Insert `SAGA_STARTED` (with trade_id).
    - Insert `RISK_CHECKED` with gate results.
    - On skip: `SKIP_RISK` with reason/threshold.
    - On trade creation: `TRADE_CREATED` with `paper_trades.id`.
- Add tests that assert events are written in correct sequence for:
  - “skipped by risk”
  - “created trade”

### Task C2: Add `paper_sagas` state transitions
- On start, insert row `paper_sagas(state='started')` if not exists.
- On skip, update to `skipped`.
- On trade creation, update to `trade_created`.
- On error, update to `failed` + `last_error`.
- Add tests for idempotency:
  - Re-processing same trade does not create a second saga row.
  - Re-processing does not create duplicate paper trade (unique constraint holds).

### Task D1: Latency/backpressure metrics (paper fast path)
- Add Prometheus metrics in `crates/evaluator/src/metrics.rs`:
  - `paper_fast_path_triggers_total{reason=...}`
  - `paper_fast_path_run_duration_seconds`
  - `paper_fast_path_trigger_to_start_seconds` (approx: now - last trigger timestamp)
  - `paper_fast_path_coalesced_triggers_total` (optional)
- Add unit tests that metrics names register (like existing metrics tests).

## Acceptance Criteria

- When new trades are ingested (inserted > 0), paper decisioning runs promptly (no waiting for next scheduled interval).
- A burst of ingestion does not create an unbounded queue of pending paper ticks (coalescing semantics verified by tests).
- Paper decisions are idempotent per `(triggered_by_trade_id, strategy)`.
- Each decision is durably auditable in `paper_events` (and `paper_sagas` shows summarized state).

