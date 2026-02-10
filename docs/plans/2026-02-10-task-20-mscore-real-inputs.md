# Task 20: MScore Real Inputs (Density, Whale Concentration)

Date: 2026-02-10

## Goal

Replace hardcoded MScore inputs in market scoring with values computed from the local DB:

- `trades_24h` from `trades_raw`
- `unique_traders_24h` from `trades_raw`
- `top_holder_concentration` from the latest `holders_snapshots` snapshot

## Scope

- Add DB query helpers in `crates/evaluator/src/jobs/pipeline_jobs.rs`
- Wire helpers into `run_market_scoring_once` so ranking uses real signals
- Add focused unit/integration tests for the DB-derived inputs

## Implementation Steps

1. Add tests for:
   - counting `trades_24h` and `unique_traders_24h`
   - computing whale concentration from holders snapshots
   - market scoring uses DB-derived density + whale concentration (mscore differs)
2. Implement the DB query helpers.
3. Wire the computed values into the market scoring job.
4. Verify with `cargo test -p evaluator`.

## Verification

- `cargo test -p evaluator`
