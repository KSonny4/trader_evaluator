# On-Demand Feature Computation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable immediate feature computation when wallets are discovered, reducing classification latency from ~25h to ~1h.

**Architecture:** Spawn tokio tasks after wallet_discovery inserts new wallets. Each task computes features for 30d window only. Hourly persona job picks up computed features.

**Tech Stack:** Rust, tokio async, SQLite, tracing

---

## Task 1: Create compute_features_for_wallet wrapper function

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs` (add new public async function after line 444)
- Test: `crates/evaluator/src/wallet_features.rs` (tests module at end of file)

**Step 1: Write the failing test**

Add to test module at end of `wallet_features.rs`:

```rust
#[tokio::test]
async fn test_compute_features_for_wallet_success() {
    let cfg = common::config::Config::from_toml_str(
        include_str!("../../../config/default.toml")
    ).unwrap();
    let db = common::db::AsyncDatabase::open(":memory:").await.unwrap();

    // Insert wallet with 5+ settled trades
    db.call(|conn| {
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xtest', 'HOLDER', 1)",
            [],
        )?;
        for i in 0..5 {
            conn.execute(
                "INSERT INTO trades_raw (tx_hash, proxy_wallet, condition_id, side, size, price, bucket, timestamp)
                 VALUES (?1, '0xtest', '0xcond', 'BUY', 100, 0.5, 'crypto', datetime('now', ?2))",
                rusqlite::params![format!("0xtx{}", i), format!("-{} days", i)],
            )?;
            conn.execute(
                "INSERT INTO trades_raw (tx_hash, proxy_wallet, condition_id, side, size, price, bucket, timestamp)
                 VALUES (?1, '0xtest', '0xcond', 'SELL', 100, 0.6, 'crypto', datetime('now', ?2))",
                rusqlite::params![format!("0xtxsell{}", i), format!("-{} days", i)],
            )?;
        }
        Ok(())
    }).await.unwrap();

    // Call on-demand feature computation
    let result = compute_features_for_wallet(&db, &cfg, "0xtest", 30).await;
    assert!(result.is_ok(), "should compute features successfully");

    // Verify features row inserted
    let count: i64 = db.call(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM wallet_features_daily WHERE proxy_wallet = '0xtest' AND window_days = 30",
            [],
            |row| row.get(0),
        )?)
    }).await.unwrap();
    assert_eq!(count, 1, "should have 1 feature row");
}

#[tokio::test]
async fn test_compute_features_for_wallet_insufficient_trades() {
    let cfg = common::config::Config::from_toml_str(
        include_str!("../../../config/default.toml")
    ).unwrap();
    let db = common::db::AsyncDatabase::open(":memory:").await.unwrap();

    // Insert wallet with only 2 trades (below threshold)
    db.call(|conn| {
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xfew', 'HOLDER', 1)",
            [],
        )?;
        for i in 0..2 {
            conn.execute(
                "INSERT INTO trades_raw (tx_hash, proxy_wallet, condition_id, side, size, price, bucket, timestamp)
                 VALUES (?1, '0xfew', '0xcond', 'BUY', 100, 0.5, 'crypto', datetime('now', ?2))",
                rusqlite::params![format!("0xtx{}", i), format!("-{} days", i)],
            )?;
        }
        Ok(())
    }).await.unwrap();

    // Call on-demand feature computation
    let result = compute_features_for_wallet(&db, &cfg, "0xfew", 30).await;
    assert!(result.is_err(), "should fail with insufficient trades");
    assert!(result.unwrap_err().to_string().contains("insufficient"), "error should mention insufficient trades");

    // Verify no features row inserted
    let count: i64 = db.call(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM wallet_features_daily WHERE proxy_wallet = '0xfew'",
            [],
            |row| row.get(0),
        )?)
    }).await.unwrap();
    assert_eq!(count, 0, "should have 0 feature rows");
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test -p evaluator compute_features_for_wallet
```

Expected: FAIL with "cannot find function `compute_features_for_wallet`"

**Step 3: Write minimal implementation**

Add after `save_wallet_features()` function (around line 490):

```rust
/// Compute and save features for a single wallet and window.
///
/// This is a wrapper around the batch feature computation logic,
/// designed for on-demand computation when wallets are first discovered.
///
/// # Errors
/// Returns error if:
/// - Wallet has <5 settled trades (insufficient data)
/// - Database query/insert fails
pub async fn compute_features_for_wallet(
    db: &common::db::AsyncDatabase,
    cfg: &common::config::Config,
    proxy_wallet: &str,
    window_days: i64,
) -> anyhow::Result<()> {
    use chrono::Utc;

    let wallet = proxy_wallet.to_string();
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let min_trades = 5_u32;

    db.call_named("on_demand_features.compute", move |conn| {
        // Check settled trade count (same gate as daily batch)
        let settled_count: i64 = conn.query_row(
            "
            SELECT COUNT(DISTINCT t1.tx_hash)
            FROM trades_raw t1
            WHERE t1.proxy_wallet = ?1
              AND EXISTS (
                  SELECT 1 FROM trades_raw t2
                  WHERE t2.proxy_wallet = t1.proxy_wallet
                    AND t2.condition_id = t1.condition_id
                    AND t2.side != t1.side
                    AND t2.timestamp >= t1.timestamp
              )
            ",
            [&wallet],
            |row| row.get(0),
        )?;

        if settled_count < min_trades as i64 {
            return Err(anyhow::anyhow!(
                "insufficient settled trades: {} < {}",
                settled_count,
                min_trades
            ));
        }

        // Compute features (reuse existing logic)
        let features = compute_wallet_features(conn, &wallet, window_days)?;

        if features.trade_count < min_trades {
            return Err(anyhow::anyhow!(
                "insufficient total trades: {} < {}",
                features.trade_count,
                min_trades
            ));
        }

        // Persist
        save_wallet_features(conn, &features, &today)?;

        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("on-demand feature computation failed: {}", e))
}
```

**Step 4: Run tests to verify they pass**

```bash
cargo test -p evaluator compute_features_for_wallet
```

Expected: PASS (2 tests)

**Step 5: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "feat: add compute_features_for_wallet wrapper for on-demand computation

- Async wrapper around existing feature computation logic
- Checks ≥5 settled trades gate before computing
- Returns Result for error handling in spawned tasks
- Tests: success case + insufficient trades case

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Track newly inserted wallets in discovery

**Files:**
- Modify: `crates/evaluator/src/jobs/pipeline_jobs.rs:758-778` (wallet insertion loop)

**Step 1: Write the failing test**

Add to test module at end of `pipeline_jobs.rs`:

```rust
#[tokio::test]
async fn test_wallet_discovery_tracks_new_wallets() {
    let cfg = common::config::Config::from_toml_str(
        include_str!("../../../../config/default.toml")
    ).unwrap();
    let db = common::db::AsyncDatabase::open(":memory:").await.unwrap();

    // Insert market score
    db.call(|conn| {
        conn.execute(
            "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES ('0xmarket', date('now'), 0.9, 1)",
            [],
        )
    }).await.unwrap();

    // Pre-insert one wallet as "existing"
    db.call(|conn| {
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xold', 'HOLDER', 1)",
            [],
        )
    }).await.unwrap();

    let holders = FakeHoldersFetcher {
        wallets: vec![
            HolderWallet { proxy_wallet: "0xold".to_string() },   // existing
            HolderWallet { proxy_wallet: "0xnew1".to_string() },  // new
        ],
    };
    let trades = FakeMarketTradesFetcher { wallets: vec![
        TradeWallet { proxy_wallet: "0xnew2".to_string(), total_trades: 10 },  // new
    ]};

    // Discovery should insert 2 new wallets (0xnew1, 0xnew2)
    let inserted = run_wallet_discovery_once(&db, &holders, &trades, &cfg).await.unwrap();
    assert_eq!(inserted, 2, "should insert 2 new wallets");

    // TODO: Verify that new_wallets list contains 0xnew1 and 0xnew2
    // This will be validated when we add the spawn logic in next task
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p evaluator test_wallet_discovery_tracks_new_wallets
```

Expected: PASS (test passes but doesn't validate new_wallets list yet - placeholder for next task)

**Step 3: Modify wallet insertion to track new wallets**

Replace the `wallet_discovery.insert_wallets_page` call (lines 758-778) with:

```rust
        let cid = condition_id.clone();
        let (page_inserted, new_wallets): (u64, Vec<String>) = db
            .call_named("wallet_discovery.insert_wallets_page", move |conn| {
                let tx = conn.transaction()?;

                let mut ins = 0_u64;
                let mut newly_inserted = Vec::new();
                for (proxy_wallet, discovered_from) in wallets_to_insert {
                    let changed = tx.execute(
                        "
                        INSERT OR IGNORE INTO wallets
                            (proxy_wallet, discovered_from, discovered_market, is_active)
                        VALUES
                            (?1, ?2, ?3, 1)
                        ",
                        rusqlite::params![&proxy_wallet, &discovered_from, &cid],
                    )?;
                    if changed > 0 {
                        newly_inserted.push(proxy_wallet);
                        ins += 1;
                    }
                }
                tx.commit()?;
                Ok((ins, newly_inserted))
            })
            .await?;

        inserted += page_inserted;
        all_new_wallets.extend(new_wallets);
```

And add `let mut all_new_wallets = Vec::new();` before the market loop (around line 700).

**Step 4: Run tests to verify they pass**

```bash
cargo test -p evaluator test_wallet_discovery
```

Expected: PASS (all wallet_discovery tests)

**Step 5: Commit**

```bash
git add crates/evaluator/src/jobs/pipeline_jobs.rs
git commit -m "feat: track newly inserted wallets in discovery

- Return (inserted_count, new_wallet_list) from DB call
- Collect new wallets across all markets in discovery run
- Preparation for spawning on-demand feature computation

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Spawn on-demand feature computation after discovery

**Files:**
- Modify: `crates/evaluator/src/jobs/pipeline_jobs.rs:800-802` (after discovery loop, before metrics)

**Step 1: Write the failing integration test**

Add to test module:

```rust
#[tokio::test]
async fn test_on_demand_features_spawned_after_discovery() {
    let cfg = common::config::Config::from_toml_str(
        include_str!("../../../../config/default.toml")
    ).unwrap();
    let db = common::db::AsyncDatabase::open(":memory:").await.unwrap();

    // Insert market score
    db.call(|conn| {
        conn.execute(
            "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES ('0xmarket', date('now'), 0.9, 1)",
            [],
        )
    }).await.unwrap();

    // Insert trades for new wallet (≥5 for feature computation)
    db.call(|conn| {
        for i in 0..6 {
            conn.execute(
                "INSERT INTO trades_raw (tx_hash, proxy_wallet, condition_id, side, size, price, bucket, timestamp)
                 VALUES (?1, '0xnewwallet', '0xmarket', 'BUY', 100, 0.5, 'crypto', datetime('now', ?2))",
                rusqlite::params![format!("0xtx_buy_{}", i), format!("-{} days", i)],
            )?;
            conn.execute(
                "INSERT INTO trades_raw (tx_hash, proxy_wallet, condition_id, side, size, price, bucket, timestamp)
                 VALUES (?1, '0xnewwallet', '0xmarket', 'SELL', 100, 0.6, 'crypto', datetime('now', ?2))",
                rusqlite::params![format!("0xtx_sell_{}", i), format!("-{} days", i)],
            )?;
        }
        Ok(())
    }).await.unwrap();

    let holders = FakeHoldersFetcher {
        wallets: vec![HolderWallet { proxy_wallet: "0xnewwallet".to_string() }],
    };
    let trades = FakeMarketTradesFetcher { wallets: vec![] };

    // Run discovery
    let inserted = run_wallet_discovery_once(&db, &holders, &trades, &cfg).await.unwrap();
    assert_eq!(inserted, 1, "should discover 1 new wallet");

    // Wait for spawned tasks to complete (tokio tasks are async)
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Verify features computed
    let count: i64 = db.call(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM wallet_features_daily WHERE proxy_wallet = '0xnewwallet' AND window_days = 30",
            [],
            |row| row.get(0),
        )?)
    }).await.unwrap();
    assert_eq!(count, 1, "on-demand features should be computed for new wallet");
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p evaluator test_on_demand_features_spawned_after_discovery
```

Expected: FAIL with assertion "on-demand features should be computed" (count = 0)

**Step 3: Add tokio spawn logic**

After the discovery loop ends and before metrics (replace lines 800-802):

```rust
    // Spawn on-demand feature computation for newly discovered wallets
    for wallet in all_new_wallets {
        let db = db.clone();
        let cfg = cfg.clone();
        tokio::spawn(async move {
            let span = tracing::info_span!("on_demand_features", wallet = %wallet);
            let _g = span.enter();
            match crate::wallet_features::compute_features_for_wallet(&db, &cfg, &wallet, 30).await {
                Ok(()) => {
                    tracing::info!("on-demand features computed");
                    metrics::counter!("evaluator_on_demand_features_total", "status" => "success").increment(1);
                }
                Err(e) => {
                    tracing::warn!(error=%e, "on-demand features failed, will retry in batch");
                    metrics::counter!("evaluator_on_demand_features_total", "status" => "failure").increment(1);
                }
            }
        });
    }

    metrics::counter!("evaluator_wallets_discovered_total").increment(inserted);
```

**Step 4: Run tests to verify they pass**

```bash
cargo test -p evaluator test_on_demand_features_spawned_after_discovery
```

Expected: PASS

**Step 5: Run all discovery tests**

```bash
cargo test -p evaluator test_run_wallet_discovery
```

Expected: PASS (all tests)

**Step 6: Commit**

```bash
git add crates/evaluator/src/jobs/pipeline_jobs.rs
git commit -m "feat: spawn on-demand feature computation after discovery

- After inserting new wallets, spawn tokio task per wallet
- Compute 30d window features immediately (non-blocking)
- Log success/failure with structured tracing
- Emit metrics: evaluator_on_demand_features_total{status}

Reduces wallet classification latency from ~25h to ~1h.

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Add idempotency test

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs` (test module)

**Step 1: Write the test**

Add to test module:

```rust
#[tokio::test]
async fn test_compute_features_for_wallet_idempotent() {
    let cfg = common::config::Config::from_toml_str(
        include_str!("../../../config/default.toml")
    ).unwrap();
    let db = common::db::AsyncDatabase::open(":memory:").await.unwrap();

    // Insert wallet with sufficient trades
    db.call(|conn| {
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xidempotent', 'HOLDER', 1)",
            [],
        )?;
        for i in 0..6 {
            conn.execute(
                "INSERT INTO trades_raw (tx_hash, proxy_wallet, condition_id, side, size, price, bucket, timestamp)
                 VALUES (?1, '0xidempotent', '0xcond', 'BUY', 100, 0.5, 'crypto', datetime('now', ?2))",
                rusqlite::params![format!("0xtx_buy_{}", i), format!("-{} days", i)],
            )?;
            conn.execute(
                "INSERT INTO trades_raw (tx_hash, proxy_wallet, condition_id, side, size, price, bucket, timestamp)
                 VALUES (?1, '0xidempotent', '0xcond', 'SELL', 100, 0.6, 'crypto', datetime('now', ?2))",
                rusqlite::params![format!("0xtx_sell_{}", i), format!("-{} days", i)],
            )?;
        }
        Ok(())
    }).await.unwrap();

    // Call twice
    compute_features_for_wallet(&db, &cfg, "0xidempotent", 30).await.unwrap();
    compute_features_for_wallet(&db, &cfg, "0xidempotent", 30).await.unwrap();

    // Verify only 1 row (INSERT OR REPLACE with same date/window)
    let count: i64 = db.call(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM wallet_features_daily WHERE proxy_wallet = '0xidempotent' AND window_days = 30",
            [],
            |row| row.get(0),
        )?)
    }).await.unwrap();
    assert_eq!(count, 1, "should have exactly 1 row due to UNIQUE constraint");
}
```

**Step 2: Run test to verify it passes**

```bash
cargo test -p evaluator test_compute_features_for_wallet_idempotent
```

Expected: PASS (UNIQUE constraint on wallet_features_daily ensures idempotency)

**Step 3: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "test: verify on-demand feature computation is idempotent

- Compute features twice for same wallet/date/window
- Verify only 1 row persisted (INSERT OR REPLACE)
- Validates UNIQUE(proxy_wallet, feature_date, window_days)

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Run full test suite and verify

**Step 1: Run all tests**

```bash
make test
```

Expected: ALL PASS (cargo test + clippy + fmt + file-length)

**Step 2: Check coverage (optional)**

```bash
make coverage
```

Expected: ≥70% coverage maintained

**Step 3: Manual verification (optional)**

Start evaluator locally, trigger discovery, check logs for "on_demand_features" span:

```bash
cargo run -p evaluator 2>&1 | grep on_demand_features
```

Expected: Log lines with "on-demand features computed" or "failed" for new wallets

---

## Task 6: Update documentation

**Files:**
- Modify: `docs/HOW_IT_WORKS.md` (add note about on-demand features)
- Modify: `docs/plans/2026-02-13-on-demand-features.md` (mark status as "Implemented")

**Step 1: Update HOW_IT_WORKS.md**

Find the "Wallet Scoring" section and add:

```markdown
#### On-Demand Feature Computation

When `wallet_discovery` inserts a new wallet, it spawns a background tokio task to compute 30d window features immediately. This enables classification within the next hourly persona run (~1h latency vs ~25h).

- Silent failure if wallet has <5 settled trades
- Daily batch scoring remains authoritative (computes all 3 windows)
- Metrics: `evaluator_on_demand_features_total{status="success|failure"}`
```

**Step 2: Update design doc status**

Change line 4 in `docs/plans/2026-02-13-on-demand-features.md`:

```markdown
**Status:** Implemented
```

**Step 3: Commit**

```bash
git add docs/HOW_IT_WORKS.md docs/plans/2026-02-13-on-demand-features.md
git commit -m "docs: update for on-demand feature computation

- HOW_IT_WORKS.md: explain on-demand computation in wallet scoring
- Design doc: mark status as Implemented

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Verification Checklist

- [ ] Task 1: `compute_features_for_wallet()` function passes 3 tests
- [ ] Task 2: Discovery tracks new wallets (returned from DB call)
- [ ] Task 3: Tokio tasks spawned for new wallets, integration test passes
- [ ] Task 4: Idempotency test passes
- [ ] Task 5: `make test` passes (all tests + lint)
- [ ] Task 6: Documentation updated

## Deployment Notes

After merging:
1. Deploy to production: `make deploy`
2. Monitor logs for "on_demand_features" span: `ssh <server> journalctl -u evaluator -f | grep on_demand`
3. Check Prometheus metrics: `evaluator_on_demand_features_total{status="success"}` should increment as new wallets discovered
4. Verify persona classification latency drops from ~25h to ~1h by checking `wallet_personas.classified_at` timestamps vs `wallets.discovered_at`

## References

- Design doc: `docs/plans/2026-02-13-on-demand-features.md`
- Existing feature computation: `crates/evaluator/src/wallet_features.rs`
- Daily batch scoring: `crates/evaluator/src/jobs/pipeline_jobs.rs::run_wallet_scoring_once()`
- Persona classification: `crates/evaluator/src/persona_classification.rs`
