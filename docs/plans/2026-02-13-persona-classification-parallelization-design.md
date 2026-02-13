# Persona Classification Parallelization Design

**Date:** 2026-02-13
**Status:** Approved
**Goal:** Reduce persona classification time from 40 minutes to 12-15 minutes (2-3x speedup)

## Problem

Persona classification processes 21k+ wallets serially, taking 40+ minutes. Feature computation (reading trades_raw and calculating metrics) is the bottleneck, consuming ~80% of the time. This is CPU and I/O bound work that can be parallelized.

## Solution: Chunk-Level Parallelization

Split each 100-wallet chunk into mini-batches and compute features in parallel using tokio tasks, while keeping classification writes serial.

### Architecture

**Current Flow (Serial):**
```
For each 100-wallet chunk:
  ├─ Read wallet metadata from DB
  ├─ For each wallet (serial):
  │   ├─ Read trades_raw (SQL query)
  │   ├─ Compute features (CPU-bound calculations)
  │   └─ Classify + write to DB
  └─ Update progress
```

**New Flow (Parallel):**
```
For each 100-wallet chunk:
  ├─ Read wallet metadata from DB
  ├─ Split into 8 mini-batches (~12 wallets each)
  ├─ Parallel Phase (tokio tasks):
  │   ├─ Task 1: Read trades + compute features for wallets 0-12
  │   ├─ Task 2: Read trades + compute features for wallets 13-25
  │   ├─ ... (6 more tasks)
  │   └─ Task 8: Read trades + compute features for wallets 88-99
  ├─ Await all tasks → collect features
  └─ Serial Phase:
      ├─ Single DB transaction
      ├─ Classify all 100 wallets (using pre-computed features)
      └─ Write personas/exclusions/traits
```

**Key Insight:** SQLite WAL mode allows multiple concurrent readers (feature computation) but only one writer (classification). We exploit this by doing heavy reads in parallel, then batching all writes together.

## Implementation Details

### New Function: `compute_features_parallel()`

```rust
async fn compute_features_parallel(
    db: &AsyncDb,
    wallets: Vec<(String, u32, u32, u32)>,  // (wallet, age, trades, days_since_last)
    window_days: u32,
    now_epoch: i64,
    parallel_tasks: usize,  // default: 8
) -> Vec<(String, u32, Result<WalletFeatures>)>
```

- Takes a chunk of 100 wallets
- Splits into `parallel_tasks` mini-batches (~12 wallets each)
- Spawns tokio task for each mini-batch
- Each task calls `db.call()` to compute features for its wallets
- Returns all results (wallet, age, features)

### Modified: `process_wallet_chunk()`

Split into three phases:
1. **Stage 1 Filter** (serial, fast) - Apply stage1 checks
2. **Feature Computation** (parallel) - Call `compute_features_parallel()` for filtered wallets
3. **Stage 2 Classification** (serial) - Classify with pre-computed features, write to DB

### Configuration Guard

```rust
if cfg.personas.parallel_enabled {
    // Use parallel path
    compute_features_parallel(db, wallets, ...).await
} else {
    // Use existing serial path (current code)
}
```

## Configuration

**File:** `config/default.toml`

```toml
[personas]
# Existing config...
stage1_min_total_trades = 10
stage1_min_wallet_age_days = 30

# Parallelization
parallel_enabled = true   # Enable parallel feature computation
parallel_tasks = 8        # Number of concurrent tasks
```

**File:** `crates/common/src/config.rs`

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PersonasConfig {
    // ... existing fields ...

    /// Enable parallel classification (default: true)
    #[serde(default = "default_parallel_enabled")]
    pub parallel_enabled: bool,

    /// Number of parallel tasks per chunk (default: 8)
    #[serde(default = "default_parallel_tasks")]
    pub parallel_tasks: usize,
}

fn default_parallel_enabled() -> bool { true }
fn default_parallel_tasks() -> usize { 8 }
```

## Safety Mechanisms

1. **Feature Flag (`parallel_enabled`)**
   - Defaults to `true` (enabled by default)
   - Allows instant rollback by toggling config
   - Serial path remains unchanged - zero regression risk

2. **Bounded Concurrency (`parallel_tasks`)**
   - Default: 8 tasks (conservative starting point)
   - Prevents resource exhaustion
   - Can tune up/down based on server capacity

3. **Graceful Degradation**
   - If parallel path errors, log and fall back to serial
   - Each mini-batch failure isolated (doesn't crash entire chunk)
   - Failed wallets logged with wallet ID for debugging

4. **Memory Bounds**
   - Still processing 100 wallets per chunk (unchanged)
   - Each task processes ~12 wallets (~120KB of features)
   - Total parallel memory: ~1MB (8 tasks × 120KB)

## Error Handling

### Task Spawn Failures
- If `tokio::spawn()` fails → log error, fall back to serial path
- Ensures classification still completes even if parallelization fails

### Feature Computation Errors
- Each task returns `Result<WalletFeatures>`
- Failed features logged with wallet ID
- Failed wallets skipped (same as current behavior)
- Doesn't block other wallets in chunk

### Database Connection Errors
- `db.call()` failures bubble up as errors
- Logged with context (wallet ID, error message)
- Chunk processing continues with remaining wallets

## Testing Strategy

### Unit Tests
- `test_compute_features_parallel_success` - 100 wallets, all succeed
- `test_compute_features_parallel_partial_failure` - Some wallets fail, others succeed
- `test_compute_features_parallel_respects_task_count` - Verify batching logic

### Integration Tests
- `test_persona_classification_parallel_enabled` - End-to-end with parallel_enabled=true
- `test_persona_classification_parallel_disabled` - End-to-end with parallel_enabled=false (serial path)
- `test_persona_classification_results_identical` - Compare serial vs parallel outputs (determinism check)

### Performance Tests (manual)
- Time classification with parallel_enabled=false (baseline)
- Time classification with parallel_enabled=true, parallel_tasks=4
- Time classification with parallel_enabled=true, parallel_tasks=8
- Monitor memory usage, CPU utilization

## Expected Outcomes

### Performance
- **Baseline:** 40 minutes (21k wallets, serial)
- **Target:** 12-15 minutes (2-3x speedup)
- **Bottleneck:** SQLite writer (only one concurrent write transaction)

### Resource Usage
- **Memory:** +1MB per chunk (8 tasks × 120KB features)
- **CPU:** Better utilization (parallel I/O + computation)
- **Database:** WAL allows concurrent reads, single writer unchanged

### Rollout
- Deploy with `parallel_enabled = true`
- Monitor metrics: classification time, memory, CPU
- Can disable via config if issues arise
- No code changes needed for rollback

## Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| Parallel reads stress SQLite | Start with 8 tasks (conservative); monitor with `PRAGMA wal_checkpoint` |
| Memory usage spike | Bounded at 1MB per chunk; chunk size stays 100 wallets |
| Feature computation differs | Extensive testing; determinism checks (compare serial vs parallel) |
| Task spawn failures | Fall back to serial path; log errors |

## Files Modified

| File | Changes |
|------|---------|
| `crates/evaluator/src/jobs/pipeline_jobs.rs` | Add `compute_features_parallel()`, modify `process_wallet_chunk()` |
| `crates/common/src/config.rs` | Add `parallel_enabled` and `parallel_tasks` config fields |
| `config/default.toml` | Add parallelization config section |

## Future Enhancements

1. **Dynamic task sizing** - Adjust parallel_tasks based on server load
2. **Per-wallet timing** - Track feature computation time to identify slow wallets
3. **Connection pool** - Enable multi-chunk parallelization (requires more complex coordination)
4. **Adaptive batching** - Larger batches for fast wallets, smaller for slow ones
