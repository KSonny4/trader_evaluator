# Realized + Unrealized PnL Tracking - Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Separate realized (closed positions) from unrealized (open positions) PnL and fix Stage 1/1.5 gates to use realized PnL only.

**Architecture:** Enhance existing FIFO pairing logic to track realized PnL sum and open positions. Integrate Polymarket positions API to value unrealized positions. Update classification gates to use realized PnL. Update dashboard to show breakdown.

**Tech Stack:** Rust, SQLite, reqwest (HTTP client), Polymarket Data API

**Design Doc:** `docs/plans/2026-02-14-realized-unrealized-pnl-tracking.md`

**Critical Bug Being Fixed:** Wallets with negative realized PnL (-$95.92) but positive unrealized gains (+$1,097) are currently classified as followable. They should be EXCLUDED.

---

## Task 1: Database Migration - Add New PnL Columns

**Files:**
- Modify: `crates/common/src/db.rs` (migrations section)

**Step 1: Add migration for new columns**

Add to the migrations array in `db.rs`:

```rust
// Add after existing wallet_features_daily table creation
r#"
-- Migration: Add realized/unrealized PnL tracking columns
ALTER TABLE wallet_features_daily ADD COLUMN IF NOT EXISTS cashflow_pnl REAL NOT NULL DEFAULT 0.0;
ALTER TABLE wallet_features_daily ADD COLUMN IF NOT EXISTS fifo_realized_pnl REAL NOT NULL DEFAULT 0.0;
ALTER TABLE wallet_features_daily ADD COLUMN IF NOT EXISTS unrealized_pnl REAL NOT NULL DEFAULT 0.0;
ALTER TABLE wallet_features_daily ADD COLUMN IF NOT EXISTS total_pnl REAL NOT NULL DEFAULT 0.0;
ALTER TABLE wallet_features_daily ADD COLUMN IF NOT EXISTS open_positions_count INTEGER NOT NULL DEFAULT 0;

-- Migrate existing data: copy old realized_pnl to cashflow_pnl
UPDATE wallet_features_daily
SET cashflow_pnl = realized_pnl
WHERE cashflow_pnl = 0.0 AND realized_pnl != 0.0;
"#,
```

**Step 2: Run migration test**

Run: `cargo test -p common migrations`
Expected: Migrations create all expected columns

**Step 3: Verify schema**

Run:
```bash
sqlite3 data/evaluator.db ".schema wallet_features_daily" | grep -E "cashflow|fifo|unrealized|total_pnl|open_positions"
```
Expected: All 5 new columns present

**Step 4: Commit**

```bash
git add crates/common/src/db.rs
git commit -m "feat: Add realized/unrealized PnL columns to wallet_features_daily

- Add cashflow_pnl, fifo_realized_pnl, unrealized_pnl, total_pnl columns
- Add open_positions_count column
- Migrate existing realized_pnl data to cashflow_pnl

Part of realized/unrealized PnL tracking implementation.

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Add OpenPosition Struct and Enhanced PairedStats

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs` (around line 30, before PairedStats)

**Step 1: Add OpenPosition struct**

Add before `struct PairedStats`:

```rust
/// Represents an open position (unmatched buys) in a single market
#[derive(Debug, Clone)]
struct OpenPosition {
    condition_id: String,
    total_size: f64,           // Sum of unmatched buy sizes
    weighted_cost_basis: f64,  // Weighted average buy price
    oldest_buy_timestamp: i64,
}
```

**Step 2: Enhance PairedStats struct**

Update `struct PairedStats`:

```rust
/// Paired round-trip stats: wins, losses, and hold durations (seconds) for each closed position.
struct PairedStats {
    wins: u32,
    losses: u32,
    hold_seconds: Vec<f64>,
    closed_pnls: Vec<(i64, f64)>,
    /// Number of markets where total paired PnL > 0
    profitable_markets: u32,

    // NEW FIELDS
    /// Sum of all FIFO-paired realized PnL (closed positions only)
    total_fifo_realized_pnl: f64,
    /// Open positions (unmatched buys) per market
    open_positions: Vec<OpenPosition>,
}
```

**Step 3: Compile check**

Run: `cargo check -p evaluator`
Expected: Compile errors in `paired_trade_stats` return statement (missing new fields)

**Step 4: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "feat: Add OpenPosition struct and enhance PairedStats

- Add OpenPosition struct for tracking unmatched buys
- Add total_fifo_realized_pnl field to PairedStats
- Add open_positions vector to PairedStats

Does not compile yet - fields not populated.

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Write Tests for Enhanced FIFO Pairing (TDD - RED)

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs` (test module at end)

**Step 1: Write test for realized PnL sum**

Add to test module:

```rust
#[test]
fn test_paired_stats_sums_realized_pnl() {
    let db = setup_db_with_trades(&[
        ("0xtest", "mkt1", "BUY", 100.0, 0.40, 1000),
        ("0xtest", "mkt1", "SELL", 80.0, 0.60, 2000),  // +16.00 realized
        ("0xtest", "mkt2", "BUY", 50.0, 0.50, 3000),
        ("0xtest", "mkt2", "SELL", 50.0, 0.55, 4000),  // +2.50 realized
    ]);

    let stats = paired_trade_stats(&db.conn, "0xtest", 0).unwrap();

    // Total realized: 16.00 + 2.50 = 18.50
    assert!((stats.total_fifo_realized_pnl - 18.50).abs() < 0.01);
}
```

**Step 2: Write test for open positions tracking**

```rust
#[test]
fn test_paired_stats_tracks_open_positions() {
    let db = setup_db_with_trades(&[
        ("0xtest", "mkt1", "BUY", 100.0, 0.40, 1000),  // Cost: $40
        ("0xtest", "mkt1", "BUY", 50.0, 0.50, 1500),   // Cost: $25
        ("0xtest", "mkt1", "SELL", 80.0, 0.60, 2000),  // Matches first 80 from first buy
        // Remaining: 20 @ $0.40 + 50 @ $0.50 = 70 shares, cost basis ~$0.457
    ]);

    let stats = paired_trade_stats(&db.conn, "0xtest", 0).unwrap();

    assert_eq!(stats.open_positions.len(), 1);
    let open = &stats.open_positions[0];
    assert_eq!(open.condition_id, "mkt1");
    assert!((open.total_size - 70.0).abs() < 0.01);
    assert!((open.weighted_cost_basis - 0.457).abs() < 0.01);
}
```

**Step 3: Write test for multiple markets with mixed positions**

```rust
#[test]
fn test_paired_stats_multiple_markets_mixed() {
    let db = setup_db_with_trades(&[
        // Market 1: fully closed
        ("0xtest", "mkt1", "BUY", 100.0, 0.40, 1000),
        ("0xtest", "mkt1", "SELL", 100.0, 0.50, 2000),  // +10.00 realized

        // Market 2: open position
        ("0xtest", "mkt2", "BUY", 50.0, 0.60, 3000),    // Open: 50 @ $0.60

        // Market 3: partial close
        ("0xtest", "mkt3", "BUY", 100.0, 0.30, 4000),
        ("0xtest", "mkt3", "SELL", 60.0, 0.40, 5000),   // +6.00 realized, 40 open
    ]);

    let stats = paired_trade_stats(&db.conn, "0xtest", 0).unwrap();

    // Realized: 10.00 + 6.00 = 16.00
    assert!((stats.total_fifo_realized_pnl - 16.00).abs() < 0.01);

    // Open positions: mkt2 (50 shares) + mkt3 (40 shares) = 2 markets
    assert_eq!(stats.open_positions.len(), 2);
}
```

**Step 4: Run tests to verify they FAIL**

Run: `cargo test -p evaluator paired_stats`
Expected: 3 new tests FAIL (fields not populated yet)

**Step 5: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "test: Add failing tests for enhanced FIFO pairing (TDD red)

- test_paired_stats_sums_realized_pnl
- test_paired_stats_tracks_open_positions
- test_paired_stats_multiple_markets_mixed

Tests fail as expected - implementation next.

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Implement Enhanced FIFO Pairing Logic (TDD - GREEN)

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs` (fn paired_trade_stats, around line 60-130)

**Step 1: Update paired_trade_stats to compute realized PnL and track open positions**

Replace the existing `paired_trade_stats` function body with:

```rust
fn paired_trade_stats(conn: &Connection, proxy_wallet: &str, cutoff: i64) -> Result<PairedStats> {
    #[derive(Debug)]
    struct Trade {
        condition_id: String,
        side: String,
        size: f64,
        price: f64,
        timestamp: i64,
    }

    let rows: Vec<Trade> = conn
        .prepare(
            "SELECT condition_id, side, size, price, timestamp
             FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2
             ORDER BY condition_id, timestamp",
        )?
        .query_map(rusqlite::params![proxy_wallet, cutoff], |row| {
            Ok(Trade {
                condition_id: row.get(0)?,
                side: row.get(1)?,
                size: row.get(2)?,
                price: row.get(3)?,
                timestamp: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut wins = 0u32;
    let mut losses = 0u32;
    let mut hold_seconds: Vec<f64> = Vec::new();
    let mut closed_pnls: Vec<(i64, f64)> = Vec::new();
    let mut total_fifo_realized_pnl = 0.0;
    let mut open_positions_vec: Vec<OpenPosition> = Vec::new();

    let mut by_market: std::collections::HashMap<String, MarketBuysSells> =
        std::collections::HashMap::new();
    for t in &rows {
        let (buys, sells) = by_market
            .entry(t.condition_id.clone())
            .or_insert_with(|| (Vec::new(), Vec::new()));
        if t.side == "BUY" {
            buys.push((t.size, t.price, t.timestamp));
        } else if t.side == "SELL" {
            sells.push((t.size, t.price, t.timestamp));
        }
    }

    let mut profitable_markets = 0u32;
    for (cid, (mut buys, sells)) in by_market {
        let n = buys.len().min(sells.len());
        let mut market_pnl = 0.0f64;

        // FIFO pairing
        for i in 0..n {
            let (buy_size, buy_price, buy_ts) = buys[i];
            let (sell_size, sell_price, sell_ts) = sells[i];
            let size = buy_size.min(sell_size);
            if size <= 0.0 {
                continue;
            }
            let pnl = (sell_price - buy_price) * size;
            market_pnl += pnl;
            total_fifo_realized_pnl += pnl;  // NEW: sum realized PnL

            if pnl > 0.0 {
                wins += 1;
            } else {
                losses += 1;
            }
            hold_seconds.push((sell_ts - buy_ts) as f64);
            closed_pnls.push((sell_ts, pnl));
        }

        if market_pnl > 0.0 {
            profitable_markets += 1;
        }

        // NEW: Track open positions (unmatched buys)
        // After FIFO pairing, remaining buys are open
        let remaining_buys: Vec<_> = buys.iter().skip(n).copied().collect();

        if !remaining_buys.is_empty() {
            let total_size: f64 = remaining_buys.iter().map(|(size, _, _)| size).sum();
            let total_cost: f64 = remaining_buys.iter().map(|(size, price, _)| size * price).sum();
            let weighted_cost_basis = if total_size > 0.0 {
                total_cost / total_size
            } else {
                0.0
            };
            let oldest_timestamp = remaining_buys.iter().map(|(_, _, ts)| ts).min().copied().unwrap_or(0);

            open_positions_vec.push(OpenPosition {
                condition_id: cid.clone(),
                total_size,
                weighted_cost_basis,
                oldest_buy_timestamp: oldest_timestamp,
            });
        }
    }

    Ok(PairedStats {
        wins,
        losses,
        hold_seconds,
        closed_pnls,
        profitable_markets,
        total_fifo_realized_pnl,
        open_positions: open_positions_vec,
    })
}
```

**Step 2: Run tests to verify they PASS**

Run: `cargo test -p evaluator paired_stats`
Expected: 3 new tests PASS (green phase)

**Step 3: Run all tests to check for regressions**

Run: `cargo test -p evaluator`
Expected: All tests pass (no regressions from changes)

**Step 4: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "feat: Enhance paired_trade_stats to compute realized PnL and track open positions (TDD green)

- Sum total_fifo_realized_pnl from all closed positions
- Track remaining unmatched buys as OpenPosition structs
- Compute weighted average cost basis for open positions
- All 3 new tests pass

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Add Polymarket Positions API Client (TDD)

**Files:**
- Modify: `crates/common/src/polymarket.rs` (add new function)
- Modify: `crates/evaluator/src/wallet_features.rs` (add import)

**Step 1: Write failing test for positions API**

Add to test module in `crates/common/src/polymarket.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_positions_parses_response() {
        // This will be an integration test that hits real API
        // For now, just test the function signature exists
        let client = reqwest::Client::new();
        let result = fetch_wallet_positions(&client, "0xtest").await;
        // Will fail until function exists
        assert!(result.is_ok() || result.is_err()); // Either outcome is valid
    }
}
```

**Step 2: Add PolymarketPosition struct**

Add to `crates/common/src/polymarket.rs`:

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PolymarketPosition {
    #[serde(rename = "conditionId")]
    pub condition_id: String,
    pub size: f64,
    #[serde(rename = "marketPrice")]
    pub market_price: f64,
}
```

**Step 3: Implement fetch_wallet_positions function**

Add to `crates/common/src/polymarket.rs`:

```rust
/// Fetch current positions for a wallet from Polymarket Data API
pub async fn fetch_wallet_positions(
    client: &reqwest::Client,
    proxy_wallet: &str,
) -> Result<Vec<PolymarketPosition>> {
    let url = format!(
        "https://data-api.polymarket.com/positions?user={}",
        proxy_wallet
    );

    // Rate limiting: 200ms delay
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Positions API request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Positions API returned status {}",
            response.status()
        ));
    }

    let positions: Vec<PolymarketPosition> = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse positions response: {}", e))?;

    Ok(positions)
}
```

**Step 4: Run test**

Run: `cargo test -p common fetch_positions`
Expected: Test passes (or skips if API unreachable)

**Step 5: Commit**

```bash
git add crates/common/src/polymarket.rs
git commit -m "feat: Add Polymarket positions API client

- Add PolymarketPosition struct
- Add fetch_wallet_positions() function
- Includes rate limiting (200ms delay)
- Returns current positions with market prices

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Add Unrealized PnL Computation (TDD)

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs`

**Step 1: Write test for unrealized PnL calculation (RED)**

```rust
#[test]
fn test_compute_unrealized_pnl_positive_gains() {
    let open_positions = vec![
        OpenPosition {
            condition_id: "mkt1".to_string(),
            total_size: 100.0,
            weighted_cost_basis: 0.40,
            oldest_buy_timestamp: 1000,
        },
    ];

    let current_positions = vec![
        PolymarketPosition {
            condition_id: "mkt1".to_string(),
            size: 100.0,
            market_price: 0.55,  // Up from $0.40 cost basis
        },
    ];

    let (unrealized, count) = compute_unrealized_pnl(&open_positions, &current_positions);

    // Unrealized: 100 * ($0.55 - $0.40) = +$15.00
    assert!((unrealized - 15.0).abs() < 0.01);
    assert_eq!(count, 1);
}

#[test]
fn test_compute_unrealized_pnl_negative_losses() {
    let open_positions = vec![
        OpenPosition {
            condition_id: "mkt1".to_string(),
            total_size: 100.0,
            weighted_cost_basis: 0.60,
            oldest_buy_timestamp: 1000,
        },
    ];

    let current_positions = vec![
        PolymarketPosition {
            condition_id: "mkt1".to_string(),
            size: 100.0,
            market_price: 0.45,  // Down from $0.60 cost basis
        },
    ];

    let (unrealized, count) = compute_unrealized_pnl(&open_positions, &current_positions);

    // Unrealized: 100 * ($0.45 - $0.60) = -$15.00
    assert!((unrealized - (-15.0)).abs() < 0.01);
    assert_eq!(count, 1);
}

#[test]
fn test_compute_unrealized_pnl_missing_current_position() {
    let open_positions = vec![
        OpenPosition {
            condition_id: "mkt1".to_string(),
            total_size: 100.0,
            weighted_cost_basis: 0.40,
            oldest_buy_timestamp: 1000,
        },
    ];

    let current_positions = vec![]; // Position closed since last sync

    let (unrealized, count) = compute_unrealized_pnl(&open_positions, &current_positions);

    // Should skip missing positions
    assert_eq!(unrealized, 0.0);
    assert_eq!(count, 0);
}
```

**Step 2: Run tests to verify FAIL**

Run: `cargo test -p evaluator compute_unrealized_pnl`
Expected: 3 tests FAIL (function doesn't exist)

**Step 3: Implement compute_unrealized_pnl function (GREEN)**

Add to `crates/evaluator/src/wallet_features.rs`:

```rust
use common::polymarket::PolymarketPosition;

/// Compute unrealized PnL for open positions using current market prices
fn compute_unrealized_pnl(
    open_positions: &[OpenPosition],
    current_positions: &[PolymarketPosition],
) -> (f64, u32) {
    let mut unrealized_pnl = 0.0;
    let mut matched_count = 0;

    for open_pos in open_positions {
        // Find matching current position
        if let Some(current) = current_positions
            .iter()
            .find(|p| p.condition_id == open_pos.condition_id)
        {
            // Unrealized PnL = (current_price - cost_basis) * size
            let pnl = (current.market_price - open_pos.weighted_cost_basis) * open_pos.total_size;
            unrealized_pnl += pnl;
            matched_count += 1;
        } else {
            // Position closed since our last trade sync, or data mismatch
            tracing::warn!(
                condition_id = %open_pos.condition_id,
                "Open position not found in current positions API - may have been closed"
            );
        }
    }

    (unrealized_pnl, matched_count)
}
```

**Step 4: Run tests to verify PASS**

Run: `cargo test -p evaluator compute_unrealized_pnl`
Expected: 3 tests PASS

**Step 5: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "feat: Implement unrealized PnL computation (TDD green)

- Add compute_unrealized_pnl() function
- Matches open positions with current API data
- Handles missing positions gracefully (log warning, skip)
- All 3 tests pass

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Update WalletFeatures Struct and Computation

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs` (struct definition around line 10, compute_wallet_features around line 150)

**Step 1: Update WalletFeatures struct definition**

Find `pub struct WalletFeatures` and update:

```rust
pub struct WalletFeatures {
    pub proxy_wallet: String,
    pub window_days: u32,
    pub trade_count: u32,
    pub win_count: u32,
    pub loss_count: u32,
    pub total_pnl: f64,  // CHANGED: Now = fifo_realized_pnl + unrealized_pnl
    pub avg_position_size: f64,
    pub unique_markets: u32,
    pub avg_hold_time_hours: f64,
    pub max_drawdown_pct: f64,
    pub trades_per_week: f64,
    pub trades_per_day: f64,
    pub sharpe_ratio: f64,
    pub active_positions: u32,
    pub concentration_ratio: f64,
    pub avg_trade_size_usdc: f64,
    pub size_cv: f64,
    pub buy_sell_balance: f64,
    pub mid_fill_ratio: f64,
    pub extreme_price_ratio: f64,
    pub burstiness_top_1h_ratio: f64,
    pub top_domain: Option<String>,
    pub top_domain_ratio: f64,
    pub profitable_markets: u32,

    // RENAMED (breaking change)
    /// Cashflow PnL: total sell proceeds minus total buy costs.
    /// Captures ALL capital deployed (includes unrealized positions).
    pub cashflow_pnl: f64,  // Was: realized_pnl

    // NEW FIELDS
    /// FIFO-paired realized PnL: sum of all closed positions
    pub fifo_realized_pnl: f64,
    /// Unrealized PnL: open positions valued at current market price (from Polymarket API)
    pub unrealized_pnl: f64,
    /// Number of markets with open positions (unmatched buys)
    pub open_positions_count: u32,
}
```

**Step 2: Update compute_wallet_features to populate new fields**

In `compute_wallet_features` function, after calling `paired_trade_stats`:

```rust
pub fn compute_wallet_features(
    conn: &Connection,
    proxy_wallet: &str,
    window_days: u32,
    now_epoch: i64,
) -> Result<WalletFeatures> {
    let cutoff = now_epoch - i64::from(window_days) * 86400;

    // ... existing queries for trade_count, unique_markets, etc. ...

    // Get paired stats (now includes realized PnL and open positions)
    let paired_stats = paired_trade_stats(conn, proxy_wallet, cutoff)?;

    // ... existing drawdown/sharpe calculation ...

    // Compute cashflow PnL (old "realized_pnl" logic)
    let (total_buy_cost, total_sell_proceeds): (f64, f64) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN side = 'BUY' THEN size * price ELSE 0.0 END), 0.0),
                COALESCE(SUM(CASE WHEN side = 'SELL' THEN size * price ELSE 0.0 END), 0.0)
             FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2",
            rusqlite::params![proxy_wallet, cutoff],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0.0, 0.0));

    let cashflow_pnl = total_sell_proceeds - total_buy_cost;

    // NEW: Get fifo_realized_pnl and open positions from paired_stats
    let fifo_realized_pnl = paired_stats.total_fifo_realized_pnl;
    let open_positions_count = paired_stats.open_positions.len() as u32;

    // NEW: Unrealized PnL will be computed separately with API (set 0.0 for now)
    let unrealized_pnl = 0.0;  // Populated in separate function with API call

    // NEW: Total PnL = realized + unrealized
    let total_pnl = fifo_realized_pnl + unrealized_pnl;

    Ok(WalletFeatures {
        proxy_wallet: proxy_wallet.to_string(),
        window_days,
        trade_count,
        win_count: paired_stats.wins,
        loss_count: paired_stats.losses,
        total_pnl,  // Changed from old total_pnl calculation
        avg_position_size,
        unique_markets,
        avg_hold_time_hours,
        max_drawdown_pct,
        trades_per_week,
        trades_per_day,
        sharpe_ratio,
        active_positions,
        concentration_ratio,
        avg_trade_size_usdc,
        size_cv,
        buy_sell_balance,
        mid_fill_ratio,
        extreme_price_ratio,
        burstiness_top_1h_ratio,
        top_domain,
        top_domain_ratio,
        profitable_markets: paired_stats.profitable_markets,

        // NEW AND RENAMED FIELDS
        cashflow_pnl,
        fifo_realized_pnl,
        unrealized_pnl,
        open_positions_count,
    })
}
```

**Step 3: Update save_wallet_features to save new columns**

Find `save_wallet_features` function and update the INSERT statement:

```rust
pub fn save_wallet_features(
    conn: &Connection,
    features: &WalletFeatures,
    today: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO wallet_features_daily
            (proxy_wallet, feature_date, window_days, trade_count, win_count, loss_count,
             total_pnl, avg_position_size, unique_markets, avg_hold_time_hours, max_drawdown_pct,
             trades_per_day, avg_trade_size_usdc, size_cv, buy_sell_balance,
             mid_fill_ratio, extreme_price_ratio, burstiness_top_1h_ratio,
             top_category, top_category_ratio, top_domain, top_domain_ratio,
             profitable_markets, sharpe_ratio, concentration_ratio, active_positions,
             resolved_wins, resolved_losses, realized_pnl,
             cashflow_pnl, fifo_realized_pnl, unrealized_pnl, open_positions_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                 ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29,
                 ?30, ?31, ?32, ?33)",
        rusqlite::params![
            features.proxy_wallet,
            today,
            features.window_days,
            features.trade_count,
            features.win_count,
            features.loss_count,
            features.total_pnl,
            features.avg_position_size,
            features.unique_markets,
            features.avg_hold_time_hours,
            features.max_drawdown_pct,
            features.trades_per_day,
            features.avg_trade_size_usdc,
            features.size_cv,
            features.buy_sell_balance,
            features.mid_fill_ratio,
            features.extreme_price_ratio,
            features.burstiness_top_1h_ratio,
            "", // top_category (deprecated)
            0.0, // top_category_ratio
            features.top_domain.as_deref().unwrap_or(""),
            features.top_domain_ratio,
            features.profitable_markets,
            features.sharpe_ratio,
            features.concentration_ratio,
            features.active_positions,
            features.win_count,  // resolved_wins
            features.loss_count,  // resolved_losses
            features.cashflow_pnl,  // realized_pnl (old column, keep for compat)
            // NEW COLUMNS
            features.cashflow_pnl,
            features.fifo_realized_pnl,
            features.unrealized_pnl,
            features.open_positions_count,
        ],
    )?;
    Ok(())
}
```

**Step 4: Run existing feature tests**

Run: `cargo test -p evaluator compute_features`
Expected: May have failures due to struct changes (fix in next step)

**Step 5: Fix existing tests**

Update all tests that construct `WalletFeatures` to include new fields. Search for test functions and add:
```rust
cashflow_pnl: 0.0,
fifo_realized_pnl: 0.0,
unrealized_pnl: 0.0,
open_positions_count: 0,
```

**Step 6: Run tests to verify PASS**

Run: `cargo test -p evaluator`
Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "feat: Update WalletFeatures with realized/unrealized fields

- Rename realized_pnl → cashflow_pnl (breaking change)
- Add fifo_realized_pnl, unrealized_pnl, total_pnl, open_positions_count
- Update compute_wallet_features to populate new fields
- Update save_wallet_features to save new columns
- Fix all existing tests to use new struct

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Integrate Unrealized PnL with API Call

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs` (compute_wallet_features)

**Step 1: Add async wrapper for unrealized PnL computation**

Add new async function:

```rust
/// Compute wallet features with unrealized PnL from Polymarket API
pub async fn compute_wallet_features_with_unrealized(
    db: &AsyncDb,
    config: &Config,
    proxy_wallet: &str,
    window_days: u32,
) -> Result<WalletFeatures> {
    let now_epoch = chrono::Utc::now().timestamp();
    let today = chrono::Utc::now().date_naive().to_string();

    // Compute base features (includes fifo_realized_pnl and open positions)
    let mut features = db
        .call_named("compute_features", move |conn| {
            compute_wallet_features(conn, proxy_wallet, window_days, now_epoch)
        })
        .await?;

    // If we have open positions, fetch current prices and compute unrealized
    if features.open_positions_count > 0 {
        // Get open positions from paired_stats again (we need the data)
        let cutoff = now_epoch - i64::from(window_days) * 86400;
        let open_positions = db
            .call_named("get_open_positions", move |conn| {
                let stats = paired_trade_stats(conn, proxy_wallet, cutoff)?;
                Ok(stats.open_positions)
            })
            .await?;

        // Fetch current positions from Polymarket API
        let client = reqwest::Client::new();
        match common::polymarket::fetch_wallet_positions(&client, proxy_wallet).await {
            Ok(current_positions) => {
                let (unrealized, _count) = compute_unrealized_pnl(&open_positions, &current_positions);
                features.unrealized_pnl = unrealized;
                features.total_pnl = features.fifo_realized_pnl + unrealized;
            }
            Err(e) => {
                tracing::warn!(
                    proxy_wallet = %proxy_wallet,
                    error = %e,
                    "Failed to fetch current positions - unrealized PnL will be 0.0"
                );
                // Keep unrealized_pnl = 0.0 (already set in compute_wallet_features)
            }
        }
    }

    // Save features with unrealized PnL
    db.call_named("save_features", move |conn| {
        save_wallet_features(conn, &features, &today)
    })
    .await?;

    Ok(features)
}
```

**Step 2: Compile check**

Run: `cargo check -p evaluator`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "feat: Add async wrapper for unrealized PnL with API integration

- Add compute_wallet_features_with_unrealized() async function
- Fetches current positions from Polymarket API
- Computes unrealized PnL for open positions
- Graceful fallback if API fails (unrealized = 0.0)

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Fix Stage 1 Gate to Use Realized PnL (TDD)

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs` (compute_all_time_roi function)
- Modify: `crates/evaluator/src/jobs/pipeline_jobs.rs` (Stage 1 gate logic)

**Step 1: Write test for Stage 1 gate with realized PnL (RED)**

Add to test module:

```rust
#[test]
fn test_compute_all_time_roi_uses_realized_not_cashflow() {
    let db = setup_db_with_trades(&[
        // Closed position: loss
        ("0xloser", "mkt1", "BUY", 100.0, 0.60, 1000),
        ("0xloser", "mkt1", "SELL", 100.0, 0.50, 2000),  // -$10 realized

        // Open position with paper gain (not counted in realized)
        ("0xloser", "mkt2", "BUY", 1000.0, 0.40, 3000),  // $400 cost, if price is now $0.50 = +$100 unrealized

        // Cashflow PnL = (50 + 0) - (60 + 400) = -410 (negative)
        // But with unrealized gain: -10 + 100 = +90 total
        // Realized PnL = -10 (only closed position counts)
    ]);

    let roi = compute_all_time_roi(&db.conn, "0xloser").unwrap();

    // ROI based on realized PnL only: -10 / 460 = -0.0217 (-2.17%)
    // Should be negative (exclude this wallet)
    assert!(roi < 0.0, "ROI should be negative based on realized loss");
}
```

**Step 2: Run test to verify FAIL**

Run: `cargo test -p evaluator compute_all_time_roi_uses_realized`
Expected: FAIL (currently uses cashflow PnL)

**Step 3: Update compute_all_time_roi to use FIFO realized PnL (GREEN)**

Replace the `compute_all_time_roi` function:

```rust
/// Compute all-time ROI using FIFO-paired realized PnL (closed positions only).
/// Unrealized gains don't count - only proven profits from closed positions.
pub fn compute_all_time_roi(conn: &Connection, proxy_wallet: &str) -> Result<f64> {
    // Get FIFO-paired realized PnL for ALL time (cutoff = 0)
    let paired_stats = paired_trade_stats(conn, proxy_wallet, 0)?;
    let realized_pnl = paired_stats.total_fifo_realized_pnl;

    // Get total capital deployed (denominator for ROI)
    let total_buy_cost: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(CASE WHEN side = 'BUY' THEN size * price ELSE 0.0 END), 0.0)
             FROM trades_raw
             WHERE proxy_wallet = ?1",
            rusqlite::params![proxy_wallet],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    if total_buy_cost == 0.0 {
        return Ok(0.0);
    }

    // ROI = realized_pnl / capital_deployed
    Ok(realized_pnl / total_buy_cost)
}
```

**Step 4: Run test to verify PASS**

Run: `cargo test -p evaluator compute_all_time_roi_uses_realized`
Expected: PASS

**Step 5: Run all existing ROI tests**

Run: `cargo test -p evaluator compute_all_time_roi`
Expected: All ROI tests pass (may need to update test data)

**Step 6: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "feat: Fix Stage 1 gate to use FIFO realized PnL (TDD green)

- Update compute_all_time_roi() to use paired_stats.total_fifo_realized_pnl
- No longer uses cashflow PnL (which includes unrealized)
- Add test: wallet with realized loss but unrealized gain is correctly excluded
- All ROI tests pass

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Fix Stage 1.5 Gate to Use Realized PnL (TDD)

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs` (compute_recent_pnl function)

**Step 1: Write test for Stage 1.5 with realized PnL (RED)**

```rust
#[test]
fn test_compute_recent_pnl_uses_realized_not_cashflow() {
    let now = 1_700_000_000i64;
    let days_30_ago = now - (30 * 86400);

    let db = setup_db_with_trades(&[
        // Recent closed position: loss
        ("0xrecent", "mkt1", "BUY", 100.0, 0.55, days_30_ago + 1000),
        ("0xrecent", "mkt1", "SELL", 100.0, 0.50, days_30_ago + 2000),  // -$5 realized

        // Recent open position with paper gain
        ("0xrecent", "mkt2", "BUY", 1000.0, 0.40, days_30_ago + 3000),  // $400 cost
        // If current price is $0.50: +$100 unrealized (not counted)

        // Old trade (outside 30-day window)
        ("0xrecent", "mkt3", "BUY", 100.0, 0.30, days_30_ago - 10000),
        ("0xrecent", "mkt3", "SELL", 100.0, 0.50, days_30_ago - 5000),  // +$20 realized (old)
    ]);

    let recent_pnl = compute_recent_pnl(&db.conn, "0xrecent", 30, now).unwrap();

    // Should only count recent realized: -$5 (not the old +$20 or unrealized +$100)
    assert!((recent_pnl - (-5.0)).abs() < 0.01);
}
```

**Step 2: Run test to verify FAIL**

Run: `cargo test -p evaluator compute_recent_pnl_uses_realized`
Expected: FAIL (currently uses cashflow)

**Step 3: Update compute_recent_pnl to use FIFO realized PnL (GREEN)**

Replace `compute_recent_pnl` function:

```rust
/// Compute recent FIFO-paired realized PnL over last N days.
/// Uses closed positions only, not unrealized gains.
pub fn compute_recent_pnl(
    conn: &Connection,
    proxy_wallet: &str,
    window_days: u32,
    now_epoch: i64,
) -> Result<f64> {
    let cutoff = now_epoch - (i64::from(window_days) * 86400);

    // Use FIFO-paired realized PnL in window
    let paired_stats = paired_trade_stats(conn, proxy_wallet, cutoff)?;
    Ok(paired_stats.total_fifo_realized_pnl)
}
```

**Step 4: Run test to verify PASS**

Run: `cargo test -p evaluator compute_recent_pnl_uses_realized`
Expected: PASS

**Step 5: Run all recent PnL tests**

Run: `cargo test -p evaluator compute_recent_pnl`
Expected: All 6 tests pass (5 old + 1 new)

**Step 6: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "feat: Fix Stage 1.5 gate to use FIFO realized PnL (TDD green)

- Update compute_recent_pnl() to use paired_stats.total_fifo_realized_pnl
- No longer uses cashflow PnL (which includes unrealized)
- Add test: recent realized loss detected even with unrealized gains
- All 6 recent PnL tests pass

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Update Dashboard Queries for New Fields

**Files:**
- Modify: `crates/web/src/queries.rs` (WalletFeaturesSnapshot struct and wallet_features_latest function)
- Modify: `crates/web/src/models.rs` (if WalletFeaturesSnapshot is defined there)

**Step 1: Find WalletFeaturesSnapshot struct**

Run:
```bash
grep -n "struct WalletFeaturesSnapshot" crates/web/src/*.rs
```

**Step 2: Add new fields to WalletFeaturesSnapshot**

Update the struct:

```rust
pub struct WalletFeaturesSnapshot {
    pub feature_date: String,
    pub total_pnl: f64,  // Now = fifo_realized + unrealized
    pub win_count: i64,
    pub loss_count: i64,
    pub max_drawdown_pct: f64,
    pub sharpe_ratio: f64,
    pub trades_per_day: f64,
    pub trade_count: i64,
    pub unique_markets: i64,
    pub profitable_markets: i64,
    pub concentration_ratio: f64,
    pub avg_trade_size: f64,
    pub size_cv: f64,
    pub buy_sell_balance: f64,
    pub burstiness_top_1h_ratio: f64,
    pub top_domain: String,
    pub top_domain_ratio: f64,
    pub mid_fill_ratio: f64,
    pub extreme_price_ratio: f64,
    pub active_positions: i64,
    pub avg_position_size: f64,
    pub roi_pct: f64,

    // NEW FIELDS
    pub cashflow_pnl: f64,
    pub fifo_realized_pnl: f64,
    pub unrealized_pnl: f64,
    pub open_positions_count: i64,
}
```

**Step 3: Update wallet_features_latest query**

In `wallet_features_latest` function, update the SELECT statement to include new columns:

```rust
"SELECT feature_date, COALESCE(total_pnl, 0), COALESCE(win_count, 0), COALESCE(loss_count, 0),
        /* ... existing columns ... */
        COALESCE(avg_position_size, 0),
        COALESCE(cashflow_pnl, 0),
        COALESCE(fifo_realized_pnl, 0),
        COALESCE(unrealized_pnl, 0),
        COALESCE(open_positions_count, 0)
 FROM wallet_features_daily
 WHERE proxy_wallet = ?1 AND window_days = 30
 ORDER BY feature_date DESC
 LIMIT 1"
```

Update the tuple extraction to include new fields (add at end):
```rust
let cashflow_pnl = row.get::<_, f64>(N)?;  // Update N to correct index
let fifo_realized_pnl = row.get::<_, f64>(N+1)?;
let unrealized_pnl = row.get::<_, f64>(N+2)?;
let open_positions_count = row.get::<_, i64>(N+3)?;
```

**Step 4: Update struct construction**

Add to the `WalletFeaturesSnapshot` construction:
```rust
cashflow_pnl,
fifo_realized_pnl,
unrealized_pnl,
open_positions_count,
```

**Step 5: Compile and test**

Run: `cargo test -p web`
Expected: All web tests pass

**Step 6: Commit**

```bash
git add crates/web/src/queries.rs crates/web/src/models.rs
git commit -m "feat: Add realized/unrealized PnL to dashboard queries

- Add cashflow_pnl, fifo_realized_pnl, unrealized_pnl, open_positions_count to WalletFeaturesSnapshot
- Update wallet_features_latest query to read new columns
- All web tests pass

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Update Dashboard Template to Show Breakdown

**Files:**
- Find and modify: Scorecard HTML template (likely in `crates/web/src/main.rs` or templates/)

**Step 1: Find scorecard template**

Run:
```bash
grep -r "ScorecardTemplate\|scorecard.*template" crates/web/
```

**Step 2: Update template to show realized/unrealized breakdown**

Add performance breakdown section to the scorecard template:

```html
<div class="performance-summary">
    <h3>Performance Summary</h3>
    <table>
        <tr>
            <td>Realized PnL:</td>
            <td class="{{ if features.fifo_realized_pnl >= 0.0 }}positive{{ else }}negative{{ end }}">
                ${{ features.fifo_realized_pnl | format_currency }}
                {{ if features.fifo_realized_pnl >= 0.0 }}✅{{ else }}❌{{ end }}
            </td>
        </tr>
        <tr>
            <td>Unrealized PnL:</td>
            <td class="{{ if features.unrealized_pnl >= 0.0 }}positive{{ else }}negative{{ end }}">
                ${{ features.unrealized_pnl | format_currency }}
                📈
            </td>
        </tr>
        <tr class="total-row">
            <td><strong>Total PnL:</strong></td>
            <td><strong>${{ features.total_pnl | format_currency }}</strong></td>
        </tr>
        <tr>
            <td>Open Positions:</td>
            <td>{{ features.open_positions_count }} markets</td>
        </tr>
        <tr>
            <td>Closed Trades:</td>
            <td>{{ features.win_count + features.loss_count }} round-trips</td>
        </tr>
    </table>
</div>
```

**Step 3: Test locally**

Run:
```bash
cargo run -p web --release &
sleep 3
curl http://localhost:8080/wallet/0xf983feb22d5eabfc7697b426ec1040b8038a651c | grep -i "realized\|unrealized"
```
Expected: See realized/unrealized breakdown in HTML

**Step 4: Commit**

```bash
git add crates/web/src/main.rs
git commit -m "feat: Add realized/unrealized PnL breakdown to dashboard

- Show realized PnL with ✅/❌ indicator
- Show unrealized PnL with 📈 indicator
- Show total PnL and open positions count
- Visual breakdown matches Polymarket presentation

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Update Strategy Bible Documentation

**Files:**
- Modify: `docs/STRATEGY_BIBLE.md`

**Step 1: Update Stage 1 gate description**

Find the Stage 1 table (around line 174) and update the All-time ROI row:

```markdown
| All-time ROI | `stage1_min_all_time_roi` | 0.00 (0%) | Wallets must have positive **FIFO-paired realized PnL** (closed positions only). Unrealized gains don't count - only proven profits from closed positions demonstrate true edge. Formula: sum of (sell_price - buy_price) * size for all FIFO-matched pairs. |
```

**Step 2: Update Stage 1.5 gate description**

Find Stage 1.5 section (around line 183) and update:

```markdown
| Recent profitability | `stage1_require_recent_profit` | true | Exclude wallets with negative **realized PnL** in last 30 days. Uses FIFO-paired closed positions, not cashflow or unrealized gains. Strategy deterioration must show in realized losses. |
```

**Step 3: Add PnL Metrics Glossary**

Add new section after Stage 2:

```markdown
### PnL Metrics Glossary

The system tracks multiple PnL metrics for different purposes:

| Metric | Formula | What it Measures | Used For |
|--------|---------|------------------|----------|
| **Cashflow PnL** | Total sell proceeds - Total buy costs | All capital flow (includes unrealized positions still open) | Analytics, capital deployed tracking, cashflow analysis |
| **FIFO Realized PnL** | Sum of (sell_price - buy_price) * size for FIFO-matched closed positions | Proven profits/losses from positions that have been fully closed | **Stage 1/1.5 gates**, classification decisions, ROI calculations |
| **Unrealized PnL** | (current_price - cost_basis) * size for open positions | Paper gains/losses on positions not yet closed (mark-to-market) | Dashboard display, risk monitoring, total value calculation |
| **Total PnL** | FIFO Realized + Unrealized | Complete current value (realized gains + mark-to-market of open positions) | Dashboard summary, portfolio valuation |

**Why gates use realized PnL only:** A wallet with -$95.92 in realized losses but +$1,097 in unrealized gains has proven they **lose money when they close positions**. Unrealized gains are paper profits that may never materialize. Classification decisions must be based on proven, closed performance.

**Example:**
- Wallet closes 10 positions: loses -$100 (realized)
- Wallet has 5 open positions: showing +$500 unrealized gain
- Cashflow PnL: +$400 (would PASS old gates ❌)
- Realized PnL: -$100 (correctly EXCLUDED by new gates ✅)
```

**Step 4: Commit**

```bash
git add docs/STRATEGY_BIBLE.md
git commit -m "docs: Update Strategy Bible with realized PnL clarifications

- Update Stage 1 gate: now explicitly uses FIFO realized PnL
- Update Stage 1.5 gate: now explicitly uses realized PnL
- Add PnL Metrics Glossary explaining all 4 metrics
- Add example showing why realized PnL matters for classification

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: Integration Test - Full Pipeline with Realized PnL

**Files:**
- Modify: `crates/evaluator/src/jobs/pipeline_jobs.rs` (test module)

**Step 1: Write integration test (RED)**

Add comprehensive integration test:

```rust
#[tokio::test]
async fn test_classification_uses_realized_pnl_not_unrealized() {
    let mut cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
    cfg.personas.stage1_min_all_time_roi = 0.0;  // 0% threshold

    let db = AsyncDb::open(":memory:").await.unwrap();
    db.call(|conn| {
        let now = chrono::Utc::now().timestamp();

        // Wallet with negative realized PnL but positive unrealized
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xloser', 'HOLDER', 1)",
            [],
        )?;

        // Closed position: realized loss -$10
        insert_trade(conn, "0xloser", "mkt1", "BUY", 100.0, 0.60, now - 10000)?;
        insert_trade(conn, "0xloser", "mkt1", "SELL", 100.0, 0.50, now - 5000)?;

        // Open position: $400 cost (would be +$100 unrealized if price is $0.50)
        insert_trade(conn, "0xloser", "mkt2", "BUY", 1000.0, 0.40, now - 3000)?;

        // Cashflow PnL = (50 + 0) - (60 + 400) = -$410
        // But if unrealized is +$100: total = -$310 or with more positions could be positive
        // Realized PnL = -$10 (should fail Stage 1)

        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap();

    // Run classification
    let (_processed, _no_trades, all_time_roi_excluded, _recent_loss, _other, _stage2, suitable) =
        run_persona_classification_once(&db, &cfg).await.unwrap();

    // Wallet should be EXCLUDED by Stage 1 (negative realized PnL)
    assert_eq!(all_time_roi_excluded, 1, "Wallet with negative realized PnL should be excluded");
    assert_eq!(suitable, 0, "No wallets should be suitable");

    // Verify exclusion reason
    let reason: Option<String> = db.call(|conn| {
        conn.query_row(
            "SELECT reason FROM wallet_exclusions WHERE proxy_wallet = '0xloser'",
            [],
            |row| row.get(0),
        )
        .optional()
    })
    .await
    .unwrap();

    assert!(reason.is_some());
    assert!(reason.unwrap().contains("all_time_roi"));
}
```

**Step 2: Run test to verify current behavior**

Run: `cargo test -p evaluator classification_uses_realized`
Expected: FAIL initially (may pass or fail depending on current state)

**Step 3: Verify test passes with our changes**

Run: `cargo test -p evaluator classification_uses_realized`
Expected: PASS (wallet correctly excluded)

**Step 4: Commit**

```bash
git add crates/evaluator/src/jobs/pipeline_jobs.rs
git commit -m "test: Add integration test for realized PnL classification

- Test wallet with negative realized but positive unrealized
- Verify Stage 1 gate excludes based on realized PnL
- Test passes with new realized PnL logic

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: Full Verification & Final Testing

**Step 1: Run complete test suite**

Run: `make test`
Expected: All 356+ tests pass, 0 clippy warnings, code formatted

**Step 2: Clear database and reclassify**

```bash
sqlite3 data/evaluator.db "
DELETE FROM wallet_personas;
DELETE FROM wallet_exclusions;
DELETE FROM wallet_features_daily;
"

cargo run -p evaluator --release -- classify
```

Expected:
- Wallet `0xf983feb...` is EXCLUDED
- Suitable count drops significantly (122 → ~60-80)

**Step 3: Verify problematic wallet is excluded**

```bash
sqlite3 data/evaluator.db "
SELECT reason, ROUND(metric_value, 2)
FROM wallet_exclusions
WHERE proxy_wallet = '0xf983feb22d5eabfc7697b426ec1040b8038a651c';
"
```

Expected: Shows "STAGE1_ALL_TIME_ROI" or "STAGE1_REALIZED_LOSS" with negative value

**Step 4: Check exclusion counts**

```bash
sqlite3 data/evaluator.db "
SELECT
  SUM(CASE WHEN reason LIKE '%all_time_roi%' THEN 1 ELSE 0 END) as stage1_roi_exclusions,
  SUM(CASE WHEN reason LIKE '%RECENT_LOSS%' THEN 1 ELSE 0 END) as stage15_recent_exclusions,
  COUNT(*) as total_exclusions
FROM wallet_exclusions;
"
```

Expected: Higher exclusion counts than before

**Step 5: Start web server and verify dashboard**

```bash
cargo run -p web --release &
sleep 5

# Check that realized/unrealized shows correctly
curl http://localhost:8080/wallet/0xf983feb22d5eabfc7697b426ec1040b8038a651c
```

Expected: Dashboard shows realized PnL breakdown

**Step 6: Document verification results**

Create file: `docs/verification-2026-02-14-realized-pnl.txt`

```txt
Verification Results - Realized PnL Implementation

Date: 2026-02-14

Test Suite:
- Total tests: [NUMBER] (all passing)
- New tests added: 9
- Coverage: [PERCENTAGE]%

Classification Results:
- Wallets processed: [NUMBER]
- Suitable wallets: [NUMBER] (down from 122)
- Stage 1 ROI exclusions: [NUMBER]
- Stage 1.5 recent loss exclusions: [NUMBER]

Problematic Wallet Verification:
- Wallet: 0xf983feb22d5eabfc7697b426ec1040b8038a651c
- Status: EXCLUDED ✅
- Reason: [EXCLUSION_REASON]
- Realized PnL: -$95.92
- Unrealized PnL: [VALUE]
- Previously: PATIENT_ACCUMULATOR (incorrectly classified)

Dashboard:
- Realized/unrealized breakdown: WORKING ✅
- Visual indicators (✅/❌): PRESENT ✅
- Open positions count: SHOWING ✅
```

**Step 7: Commit verification**

```bash
git add docs/verification-2026-02-14-realized-pnl.txt
git commit -m "docs: Add verification results for realized PnL implementation

All tests pass, problematic wallet correctly excluded.

Co-Authored-By: Claude Sonnet 4.5 (1M context) <noreply@anthropic.com>"
```

---

## Success Criteria Checklist

Before creating PR, verify:

- [ ] All 356+ tests pass
- [ ] New tests: 9+ added (FIFO pairing, unrealized calc, gates, integration)
- [ ] 0 clippy warnings
- [ ] Code formatted (cargo fmt)
- [ ] Wallet `0xf983feb...` EXCLUDED by Stage 1
- [ ] Dashboard shows realized/unrealized breakdown
- [ ] Strategy Bible updated with PnL glossary
- [ ] Suitable wallet count reduced (122 → ~60-80 expected)
- [ ] No wallets with negative realized PnL classified as followable

---

## Estimated Time

- Tasks 1-2: 10 min (migrations + structs)
- Tasks 3-4: 25 min (TDD for FIFO enhancements)
- Task 5: 10 min (Polymarket API client)
- Task 6: 20 min (unrealized PnL computation with TDD)
- Tasks 7-8: 15 min (WalletFeatures updates)
- Tasks 9-10: 20 min (integrate API call)
- Tasks 11-12: 15 min (fix gates with TDD)
- Tasks 13-14: 15 min (dashboard updates)
- Task 15: 10 min (Strategy Bible)
- Task 16: 10 min (integration test)
- Task 17: 10 min (verification)

**Total: ~2.5 hours** for complete, tested implementation

---

## Post-Implementation

After all tasks complete:

1. **Create PR** with full summary
2. **Request code review** using superpowers:requesting-code-review
3. **Merge to main** using superpowers:rapid-merge-workflow
4. **Deploy** using evaluator-deploy skill
5. **Monitor** classification results for 24h

**Expected Impact:**
- Followable wallets: 122 → ~60-80 (exclude wallets with realized losses)
- All wallets with negative FIFO realized PnL excluded
- Dashboard accurately reflects Polymarket data
- Classification based on proven performance, not paper gains
