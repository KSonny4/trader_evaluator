# On-Demand Feature Computation

**Date:** 2026-02-13
**Status:** Approved
**Owner:** Claude + User

## Problem

Newly discovered wallets wait up to 24 hours for feature computation (daily batch job at 86400s interval). This delays persona classification and prevents immediate evaluation of promising wallets.

**Current flow:**
```
wallet_discovery (continuous/scheduled)
    ↓ inserts to wallets table
    ↓ waits up to 24h
wallet_scoring (daily)
    ↓ computes features + WScore
    ↓ waits up to 1h
persona_classification (hourly)
    ↓ classifies wallet
```

**Desired:** Features computed immediately when wallet discovered, enabling classification within the next hourly run (~1h max vs ~25h max).

## Solution: Dual Computation Strategy

### 1. Daily Batch (Existing - No Changes)

`run_wallet_scoring_once()` continues to run every 24h:
- Computes features for ALL active wallets
- All 3 windows: 7d, 30d, 90d
- Computes WScore components
- Source of truth for feature data

### 2. On-Demand (New - Fast Path)

When `wallet_discovery` inserts a new wallet:
- Spawn background tokio task (non-blocking)
- Compute features for 30d window only (classification signal)
- Silent failure if insufficient data
- Hourly persona job picks up features within ~1h

## Architecture

### Data Flow

```
wallet_discovery inserts new wallet
    ↓
tokio::spawn (async, non-blocking)
    ↓
compute_features_for_wallet(wallet, window_days=30)
    ↓
INSERT/UPDATE wallet_features_snapshots
    ↓
next hourly persona_classification
    ↓
classify wallet (INFORMED_SPECIALIST, CONSISTENT_GENERALIST, etc.)
```

### Key Invariants

1. **Daily batch is authoritative**: On-demand provides fast signal, daily batch provides complete data
2. **Non-blocking discovery**: Spawned tasks don't slow discovery loop
3. **Idempotent writes**: `UNIQUE(proxy_wallet, feature_date, window_days)` prevents duplicates
4. **Silent failures**: Insufficient data → log warning, let scheduled jobs retry

## Implementation

### Trigger Point

**File:** `crates/evaluator/src/jobs/pipeline_jobs.rs`
**Function:** `run_wallet_discovery_once()`

After wallet insertion, spawn feature computation:

```rust
// After discover_wallets_for_market() inserts new wallets
for wallet in newly_discovered_wallets {
    let db = db.clone();
    let cfg = cfg.clone();
    tokio::spawn(async move {
        let span = tracing::info_span!("on_demand_features", wallet = %wallet);
        let _g = span.enter();
        match compute_features_for_wallet(&db, &cfg, &wallet, 30).await {
            Ok(()) => tracing::info!("on-demand features computed"),
            Err(e) => tracing::warn!(error=%e, "on-demand features failed, will retry in batch"),
        }
    });
}
```

### New Function

**File:** `crates/evaluator/src/wallet_features.rs`
**Function:** `compute_features_for_wallet(db, cfg, wallet, window_days)`

```rust
pub async fn compute_features_for_wallet(
    db: &AsyncDb,
    cfg: &Config,
    proxy_wallet: &str,
    window_days: i64,
) -> Result<()> {
    // Reuse existing save_wallet_features() logic
    // Check for minimum settled trades (≥5)
    // Return early if insufficient data
    // Log structured error on failure
}
```

Wraps existing `save_wallet_features()` to:
- Accept single wallet + single window
- Return `Result` for error handling
- Check settled trade count gate (≥5)

### Error Handling

| Condition | Behavior |
|-----------|----------|
| Insufficient trades (<5 settled) | Log warning, skip, hourly persona job retries |
| DB error (query/insert fails) | Log error, skip, daily batch retries |
| Wallet not found | Log warning, skip |

**No retries**: Rely on scheduled jobs (hourly persona, daily batch) for natural retry.

## Testing

### Unit Tests

1. `test_on_demand_features_computed_after_discovery`
   - Mock wallet discovery
   - Verify feature row inserted with feature_date=today, window_days=30

2. `test_on_demand_features_silent_skip_insufficient_trades`
   - Insert wallet with 2 trades
   - Verify no feature row, warning logged

3. `test_on_demand_features_idempotent`
   - Compute features twice for same wallet/date
   - Verify only 1 row (UNIQUE constraint)

### Integration Test

- Discover new wallet
- Wait 2 seconds (async spawn)
- Query `wallet_features_snapshots`
- Assert row exists with window_days=30

## Rollout

1. Implement `compute_features_for_wallet()` wrapper
2. Add spawn call to `run_wallet_discovery_once()`
3. Add unit tests
4. Test in dev with manual wallet discovery
5. Deploy to production
6. Monitor logs for "on_demand_features" span

## Metrics

- `evaluator_on_demand_features_total{status="success|failure"}` — counter
- `evaluator_on_demand_features_duration_seconds` — histogram
- Log ratio of success vs "insufficient trades" warnings

## Future Enhancements

If discovery volume increases (>100 wallets/run), upgrade to **Approach B: Bounded Channel Worker**:
- Discovery pushes to `mpsc::channel`
- Dedicated worker with `Semaphore::new(10)` for concurrency control
- Prevents task spawn explosion

## Alternatives Considered

### Approach B: Bounded Channel Worker
Pros: Controlled concurrency, backpressure
Cons: More code, another worker
**Decision:** Overkill for current scale (10-50 wallets/run)

### Approach C: Piggyback on Persona Job
Pros: Minimal code change
Cons: Still 5-10min latency, couples feature computation to persona logic
**Decision:** Doesn't meet "immediate" requirement

## References

- `docs/STRATEGY_BIBLE.md` — Persona taxonomy, classification gates
- `docs/EVALUATION_STRATEGY.md` — Phase 2 wallet classification requirements
- `crates/evaluator/src/wallet_features.rs` — Existing feature computation
- `crates/evaluator/src/jobs/pipeline_jobs.rs` — Pipeline orchestration
