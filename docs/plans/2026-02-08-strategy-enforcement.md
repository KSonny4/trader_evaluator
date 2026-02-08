# Strategy Enforcement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bridge the gap between the Strategy Bible (`docs/STRATEGY_BIBLE.md`) and the actual codebase. Implement persona classification, wallet feature computation, paper trade settlement, copy fidelity tracking, two-level risk management, real MScore inputs, and anomaly detection — all with TDD and configurable thresholds.

**Architecture:** All new logic lives in the existing `crates/evaluator/src/` and `crates/common/src/` crates. New config sections are added to `config/default.toml` and deserialized into existing `Config` struct. New scheduled jobs are added to `jobs.rs` and wired in `main.rs`. Every threshold comes from config — nothing hardcoded.

**Tech Stack:** Rust, Tokio, SQLite (tokio-rusqlite), rust_decimal, serde, tracing, metrics.

**Current state:** 42 tests pass. MVP pipeline is end-to-end (market scoring → wallet discovery → ingestion → paper trading → wallet scoring). But: persona classification doesn't exist, paper trades never settle, WScore uses 2/5 factors, MScore has 3 hardcoded inputs, no copy fidelity tracking, no anomaly detection.

**Strategy Bible:** `docs/STRATEGY_BIBLE.md` is the governing document. Every threshold and formula in this plan comes from there.

---

## Progress

- [ ] Task 1: Config — Add persona, risk, copy fidelity, and anomaly config sections
- [ ] Task 2: Schema — Add copy_fidelity_events table and missing columns
- [ ] Task 3: Wallet Feature Computation
- [ ] Task 4: Stage 1 Fast Filters (inline exclusion)
- [ ] Task 5: Informed Specialist Detector
- [ ] Task 6: Consistent Generalist Detector
- [ ] Task 7: Patient Accumulator Detector
- [ ] Task 8: Execution Master Detector
- [ ] Task 9: Tail Risk Seller Detector
- [ ] Task 10: Noise Trader Detector
- [ ] Task 11: Sniper/Insider Detector
- [ ] Task 12: Persona Classification Orchestrator + Stage 2 Job
- [ ] Task 13: Paper Trade Settlement
- [ ] Task 14: Conditional Taker Fee (Quartic for Crypto, Zero for Everything Else)
- [ ] Task 15: Copy Fidelity Tracking
- [ ] Task 16: Two-Level Risk Management (Per-Wallet + Portfolio)
- [ ] Task 17: Follower Slippage Tracking
- [ ] Task 18: WScore — Missing 3 Sub-Components
- [ ] Task 19: Weekly Re-evaluation + Anomaly Detection
- [ ] Task 20: MScore — Real Inputs (density, whale concentration)
- [ ] Task 21: Wire New Jobs into Scheduler
- [ ] Task 22: CLOB API Client + `book_snapshots` Table
- [ ] Task 23: WebSocket Book Streaming + Recording
- [ ] Task 24: Depth-Aware Paper Trading (Book-Walking Slippage)

---

## Task 1: Config — Add Persona, Risk, Copy Fidelity, and Anomaly Config Sections

**Files:**
- Modify: `crates/common/src/config.rs`
- Modify: `config/default.toml`

**Step 1: Write the failing test**

In `crates/common/src/config.rs`, add to existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn test_persona_config_loads() {
    let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
    assert_eq!(config.personas.stage1_min_wallet_age_days, 30);
    assert_eq!(config.personas.stage1_min_total_trades, 10);
    assert!(config.personas.specialist_min_win_rate > 0.0);
    assert!(config.personas.generalist_min_sharpe > 0.0);
    assert!(config.personas.execution_master_pnl_ratio > 0.0);
    assert!(config.personas.trust_30_90_multiplier > 0.0);
    assert!(config.personas.obscurity_bonus_multiplier > 1.0);
}

#[test]
fn test_risk_v2_config_loads() {
    let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
    assert!(config.risk.per_wallet_daily_loss_pct > 0.0);
    assert!(config.risk.per_wallet_weekly_loss_pct > 0.0);
    assert!(config.risk.per_wallet_max_drawdown_pct > 0.0);
    assert!(config.risk.portfolio_daily_loss_pct > 0.0);
    assert!(config.risk.portfolio_weekly_loss_pct > 0.0);
    assert!(config.risk.max_concurrent_positions > 0);
}

#[test]
fn test_copy_fidelity_config_loads() {
    let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
    assert!(config.paper_trading.min_copy_fidelity_pct > 0.0);
    assert!(config.paper_trading.bankroll_usd > 0.0);
    assert!(config.paper_trading.max_total_exposure_pct > 0.0);
    assert!(config.paper_trading.max_daily_loss_pct > 0.0);
}

#[test]
fn test_anomaly_config_loads() {
    let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
    assert!(config.anomaly.win_rate_drop_pct > 0.0);
    assert!(config.anomaly.max_weekly_drawdown_pct > 0.0);
    assert!(config.anomaly.frequency_change_multiplier > 1.0);
    assert!(config.anomaly.size_change_multiplier > 1.0);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p common test_persona_config`
Expected: FAIL — `personas` field doesn't exist on Config.

**Step 3: Add config structs and TOML sections**

Add to `crates/common/src/config.rs`:

```rust
#[derive(Debug, Deserialize)]
pub struct Personas {
    // Stage 1 fast filters
    pub stage1_min_wallet_age_days: u32,
    pub stage1_min_total_trades: u32,
    pub stage1_max_inactive_days: u32,
    // Informed Specialist
    pub specialist_max_markets: u32,
    pub specialist_min_win_rate: f64,
    // Consistent Generalist
    pub generalist_min_markets: u32,
    pub generalist_min_win_rate: f64,
    pub generalist_max_win_rate: f64,
    pub generalist_max_drawdown: f64,
    pub generalist_min_sharpe: f64,
    // Patient Accumulator
    pub accumulator_min_hold_hours: f64,
    pub accumulator_max_trades_per_week: f64,
    // Execution Master
    pub execution_master_pnl_ratio: f64,
    // Tail Risk Seller
    pub tail_risk_min_win_rate: f64,
    pub tail_risk_loss_multiplier: f64,
    // Noise Trader
    pub noise_max_trades_per_week: f64,
    pub noise_max_abs_roi: f64,
    // Sniper/Insider
    pub sniper_max_age_days: u32,
    pub sniper_min_win_rate: f64,
    pub sniper_max_trades: u32,
    // Trust multipliers
    pub trust_30_90_multiplier: f64,
    pub obscurity_bonus_multiplier: f64,
}

#[derive(Debug, Deserialize)]
pub struct Anomaly {
    pub win_rate_drop_pct: f64,
    pub max_weekly_drawdown_pct: f64,
    pub frequency_change_multiplier: f64,
    pub size_change_multiplier: f64,
}
```

Add `personas: Personas` and `anomaly: Anomaly` fields to `Config` struct.

Add these new fields to `Risk`:

```rust
pub per_wallet_daily_loss_pct: f64,
pub per_wallet_weekly_loss_pct: f64,
pub per_wallet_max_drawdown_pct: f64,
pub per_wallet_max_slippage_vs_edge: f64,
pub portfolio_daily_loss_pct: f64,
pub portfolio_weekly_loss_pct: f64,
pub max_concurrent_positions: u32,
```

Add these new fields to `PaperTrading`:

```rust
pub bankroll_usd: f64,
pub max_total_exposure_pct: f64,
pub max_daily_loss_pct: f64,
pub min_copy_fidelity_pct: f64,
pub per_trade_size_usd: f64,
pub slippage_default_cents: f64,
```

Add to `config/default.toml`:

```toml
[personas]
# Stage 1 fast filters
stage1_min_wallet_age_days = 30
stage1_min_total_trades = 10
stage1_max_inactive_days = 30
# Informed Specialist
specialist_max_markets = 10
specialist_min_win_rate = 0.60
# Consistent Generalist
generalist_min_markets = 20
generalist_min_win_rate = 0.52
generalist_max_win_rate = 0.60
generalist_max_drawdown = 15.0
generalist_min_sharpe = 1.0
# Patient Accumulator
accumulator_min_hold_hours = 48.0
accumulator_max_trades_per_week = 5.0
# Execution Master (exclusion)
execution_master_pnl_ratio = 0.70
# Tail Risk Seller (exclusion)
tail_risk_min_win_rate = 0.80
tail_risk_loss_multiplier = 5.0
# Noise Trader (exclusion)
noise_max_trades_per_week = 50.0
noise_max_abs_roi = 0.02
# Sniper/Insider (exclusion)
sniper_max_age_days = 30
sniper_min_win_rate = 0.85
sniper_max_trades = 20
# Trust multipliers
trust_30_90_multiplier = 0.8
obscurity_bonus_multiplier = 1.2

[anomaly]
win_rate_drop_pct = 15.0
max_weekly_drawdown_pct = 20.0
frequency_change_multiplier = 3.0
size_change_multiplier = 10.0
```

And add new fields to existing `[risk]` and `[paper_trading]` sections:

```toml
# Add to [risk]
per_wallet_daily_loss_pct = 2.0
per_wallet_weekly_loss_pct = 5.0
per_wallet_max_drawdown_pct = 15.0
per_wallet_max_slippage_vs_edge = 1.0
portfolio_daily_loss_pct = 3.0
portfolio_weekly_loss_pct = 8.0
max_concurrent_positions = 20

# Add to [paper_trading]
bankroll_usd = 1000.0
max_total_exposure_pct = 15.0
max_daily_loss_pct = 3.0
min_copy_fidelity_pct = 80.0
per_trade_size_usd = 25.0
slippage_default_cents = 1.0
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p common`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add crates/common/src/config.rs config/default.toml
git commit -m "feat: add persona, anomaly, copy fidelity, two-level risk config sections"
```

---

## Task 2: Schema — Add copy_fidelity_events Table and Missing Columns

**Files:**
- Modify: `crates/common/src/db.rs`

**Step 1: Write the failing test**

In `crates/common/src/db.rs`, add to existing tests:

```rust
#[test]
fn test_copy_fidelity_events_table_exists() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    let tables: Vec<String> = db.conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert!(tables.contains(&"copy_fidelity_events".to_string()));
    assert!(tables.contains(&"follower_slippage".to_string()));
}

#[test]
fn test_copy_fidelity_events_schema() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    db.conn.execute(
        "INSERT INTO copy_fidelity_events (proxy_wallet, condition_id, their_trade_id, outcome, outcome_detail)
         VALUES ('0xabc', '0xdef', 1, 'COPIED', 'paper_trade_id=5')",
        [],
    ).unwrap();

    db.conn.execute(
        "INSERT INTO copy_fidelity_events (proxy_wallet, condition_id, their_trade_id, outcome, outcome_detail)
         VALUES ('0xabc', '0xdef', 2, 'SKIPPED_PORTFOLIO_RISK', 'exposure=16.2%, limit=15.0%')",
        [],
    ).unwrap();

    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM copy_fidelity_events WHERE proxy_wallet = '0xabc'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 2);
}

#[test]
fn test_follower_slippage_schema() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    db.conn.execute(
        "INSERT INTO follower_slippage (proxy_wallet, condition_id, their_entry_price, our_entry_price, slippage_cents, fee_applied)
         VALUES ('0xabc', '0xdef', 0.55, 0.56, 1.0, 0.008)",
        [],
    ).unwrap();
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p common test_copy_fidelity`
Expected: FAIL — table doesn't exist.

**Step 3: Add tables to SCHEMA**

Add to the `SCHEMA` constant in `db.rs`:

```sql
CREATE TABLE IF NOT EXISTS copy_fidelity_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    condition_id TEXT NOT NULL,
    their_trade_id INTEGER,
    outcome TEXT NOT NULL,
    outcome_detail TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS follower_slippage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    condition_id TEXT NOT NULL,
    their_entry_price REAL NOT NULL,
    our_entry_price REAL NOT NULL,
    slippage_cents REAL NOT NULL,
    fee_applied REAL,
    their_trade_id INTEGER,
    our_paper_trade_id INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_copy_fidelity_wallet ON copy_fidelity_events(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_follower_slippage_wallet ON follower_slippage(proxy_wallet);
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p common`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add crates/common/src/db.rs
git commit -m "feat: add copy_fidelity_events and follower_slippage tables"
```

---

## Task 3: Wallet Feature Computation

The `wallet_features_daily` table exists but nothing writes to it. We need a function that computes features from `trades_raw` and `paper_trades` for each wallet.

**Files:**
- Create: `crates/evaluator/src/wallet_features.rs`
- Modify: `crates/evaluator/src/lib.rs` (or `main.rs` module declaration)

**Step 1: Write the failing test**

```rust
// crates/evaluator/src/wallet_features.rs

#[cfg(test)]
mod tests {
    use super::*;
    use common::db::Database;

    fn setup_db_with_trades(trades: &[(& str, &str, &str, f64, f64, i64)]) -> Database {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        for (wallet, cid, side, size, price, ts) in trades {
            db.conn.execute(
                "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![wallet, cid, side, size, price, ts],
            ).unwrap();
        }
        db
    }

    #[test]
    fn test_compute_features_basic() {
        let now = 1700000000i64;
        let day = 86400i64;
        let trades = vec![
            ("0xabc", "0xm1", "BUY", 100.0, 0.60, now - 5 * day),
            ("0xabc", "0xm1", "SELL", 100.0, 0.70, now - 4 * day),
            ("0xabc", "0xm2", "BUY", 50.0, 0.40, now - 3 * day),
            ("0xabc", "0xm2", "SELL", 50.0, 0.30, now - 2 * day),
        ];
        let db = setup_db_with_trades(&trades);

        let features = compute_wallet_features(&db.conn, "0xabc", 30, now).unwrap();

        assert_eq!(features.trade_count, 4);
        assert_eq!(features.unique_markets, 2);
        assert!(features.win_count >= 1); // at least the first trade pair was profitable
    }

    #[test]
    fn test_compute_features_empty_wallet() {
        let db = setup_db_with_trades(&[]);
        let features = compute_wallet_features(&db.conn, "0xnonexistent", 30, 1700000000).unwrap();
        assert_eq!(features.trade_count, 0);
        assert_eq!(features.unique_markets, 0);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_compute_features`
Expected: FAIL — module/function doesn't exist.

**Step 3: Implement wallet feature computation**

```rust
// crates/evaluator/src/wallet_features.rs

use anyhow::Result;
use rusqlite::Connection;

#[derive(Debug, Clone)]
pub struct WalletFeatures {
    pub proxy_wallet: String,
    pub window_days: u32,
    pub trade_count: u32,
    pub win_count: u32,
    pub loss_count: u32,
    pub total_pnl: f64,
    pub avg_position_size: f64,
    pub unique_markets: u32,
    pub avg_hold_time_hours: f64,
    pub max_drawdown_pct: f64,
    pub trades_per_week: f64,
    pub sharpe_ratio: f64,
}

pub fn compute_wallet_features(
    conn: &Connection,
    proxy_wallet: &str,
    window_days: u32,
    now_epoch: i64,
) -> Result<WalletFeatures> {
    let cutoff = now_epoch - (window_days as i64) * 86400;

    let trade_count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM trades_raw WHERE proxy_wallet = ?1 AND timestamp >= ?2",
        rusqlite::params![proxy_wallet, cutoff],
        |row| row.get(0),
    )?;

    let unique_markets: u32 = conn.query_row(
        "SELECT COUNT(DISTINCT condition_id) FROM trades_raw WHERE proxy_wallet = ?1 AND timestamp >= ?2",
        rusqlite::params![proxy_wallet, cutoff],
        |row| row.get(0),
    )?;

    // Win/loss counting: a "win" is a SELL at price higher than avg BUY price in same market
    // Simplified: count trades where side=SELL and price > 0.5 as wins (directional bet won)
    // This is a rough heuristic — proper PnL requires settlement data
    let win_count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM trades_raw
         WHERE proxy_wallet = ?1 AND timestamp >= ?2
         AND side = 'SELL' AND price > 0.5",
        rusqlite::params![proxy_wallet, cutoff],
        |row| row.get(0),
    ).unwrap_or(0);

    let loss_count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM trades_raw
         WHERE proxy_wallet = ?1 AND timestamp >= ?2
         AND side = 'SELL' AND price <= 0.5",
        rusqlite::params![proxy_wallet, cutoff],
        |row| row.get(0),
    ).unwrap_or(0);

    let avg_position_size: f64 = conn.query_row(
        "SELECT COALESCE(AVG(size * price), 0.0) FROM trades_raw
         WHERE proxy_wallet = ?1 AND timestamp >= ?2",
        rusqlite::params![proxy_wallet, cutoff],
        |row| row.get(0),
    ).unwrap_or(0.0);

    // Total PnL from paper trades (if any)
    let total_pnl: f64 = conn.query_row(
        "SELECT COALESCE(SUM(pnl), 0.0) FROM paper_trades
         WHERE proxy_wallet = ?1 AND status != 'open'
         AND created_at >= datetime(?2, 'unixepoch')",
        rusqlite::params![proxy_wallet, cutoff],
        |row| row.get(0),
    ).unwrap_or(0.0);

    let weeks = (window_days as f64) / 7.0;
    let trades_per_week = if weeks > 0.0 { trade_count as f64 / weeks } else { 0.0 };

    // Avg hold time: approximate from time between BUY and next SELL in same market
    // For now, default to 0 — will be refined when we have proper position tracking
    let avg_hold_time_hours = 0.0;

    // Max drawdown and Sharpe: require daily return series
    // For now, compute from paper trades if available
    let max_drawdown_pct = 0.0;
    let sharpe_ratio = 0.0;

    Ok(WalletFeatures {
        proxy_wallet: proxy_wallet.to_string(),
        window_days,
        trade_count,
        win_count,
        loss_count,
        total_pnl,
        avg_position_size,
        unique_markets,
        avg_hold_time_hours,
        max_drawdown_pct,
        trades_per_week,
        sharpe_ratio,
    })
}

/// Persist computed features to wallet_features_daily table.
pub fn save_wallet_features(
    conn: &Connection,
    features: &WalletFeatures,
    feature_date: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO wallet_features_daily
         (proxy_wallet, feature_date, window_days, trade_count, win_count, loss_count,
          total_pnl, avg_position_size, unique_markets, avg_hold_time_hours, max_drawdown_pct)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            features.proxy_wallet, feature_date, features.window_days,
            features.trade_count, features.win_count, features.loss_count,
            features.total_pnl, features.avg_position_size, features.unique_markets,
            features.avg_hold_time_hours, features.max_drawdown_pct,
        ],
    )?;
    Ok(())
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p evaluator test_compute_features`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs
git commit -m "feat: wallet feature computation from trades_raw + paper_trades"
```

---

## Task 4: Stage 1 Fast Filters (Inline Exclusion)

Stage 1 filters run inline during wallet discovery. They are cheap checks that immediately exclude wallets that can't possibly be followable.

**Files:**
- Create: `crates/evaluator/src/persona_classification.rs`

**Step 1: Write the failing test**

```rust
// crates/evaluator/src/persona_classification.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage1_too_young() {
        let result = stage1_filter(
            5,   // wallet_age_days
            50,  // total_trades
            1,   // days_since_last_trade
            &Stage1Config { min_wallet_age_days: 30, min_total_trades: 10, max_inactive_days: 30 },
        );
        assert_eq!(result, Some(ExclusionReason::TooYoung { age_days: 5, min_required: 30 }));
    }

    #[test]
    fn test_stage1_too_few_trades() {
        let result = stage1_filter(
            60,  // old enough
            3,   // too few trades
            1,
            &Stage1Config { min_wallet_age_days: 30, min_total_trades: 10, max_inactive_days: 30 },
        );
        assert_eq!(result, Some(ExclusionReason::TooFewTrades { total: 3, min_required: 10 }));
    }

    #[test]
    fn test_stage1_inactive() {
        let result = stage1_filter(
            180, 50, 45,
            &Stage1Config { min_wallet_age_days: 30, min_total_trades: 10, max_inactive_days: 30 },
        );
        assert_eq!(result, Some(ExclusionReason::Inactive { days_since_last: 45, max_allowed: 30 }));
    }

    #[test]
    fn test_stage1_passes() {
        let result = stage1_filter(
            60, 50, 1,
            &Stage1Config { min_wallet_age_days: 30, min_total_trades: 10, max_inactive_days: 30 },
        );
        assert_eq!(result, None);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_stage1`
Expected: FAIL — module doesn't exist.

**Step 3: Implement Stage 1 filters**

```rust
// crates/evaluator/src/persona_classification.rs

use anyhow::Result;
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq)]
pub enum ExclusionReason {
    TooYoung { age_days: u32, min_required: u32 },
    TooFewTrades { total: u32, min_required: u32 },
    Inactive { days_since_last: u32, max_allowed: u32 },
    ExecutionMaster { execution_pnl_ratio: f64, threshold: f64 },
    TailRiskSeller { win_rate: f64, max_loss_ratio: f64 },
    NoiseTrader { trades_per_week: f64, abs_roi: f64 },
    SniperInsider { age_days: u32, win_rate: f64, trade_count: u32 },
}

impl ExclusionReason {
    pub fn reason_str(&self) -> &'static str {
        match self {
            Self::TooYoung { .. } => "STAGE1_TOO_YOUNG",
            Self::TooFewTrades { .. } => "STAGE1_TOO_FEW_TRADES",
            Self::Inactive { .. } => "STAGE1_INACTIVE",
            Self::ExecutionMaster { .. } => "EXECUTION_MASTER",
            Self::TailRiskSeller { .. } => "TAIL_RISK_SELLER",
            Self::NoiseTrader { .. } => "NOISE_TRADER",
            Self::SniperInsider { .. } => "SNIPER_INSIDER",
        }
    }

    pub fn metric_value(&self) -> f64 {
        match self {
            Self::TooYoung { age_days, .. } => *age_days as f64,
            Self::TooFewTrades { total, .. } => *total as f64,
            Self::Inactive { days_since_last, .. } => *days_since_last as f64,
            Self::ExecutionMaster { execution_pnl_ratio, .. } => *execution_pnl_ratio,
            Self::TailRiskSeller { win_rate, .. } => *win_rate,
            Self::NoiseTrader { trades_per_week, .. } => *trades_per_week,
            Self::SniperInsider { win_rate, .. } => *win_rate,
        }
    }

    pub fn threshold(&self) -> f64 {
        match self {
            Self::TooYoung { min_required, .. } => *min_required as f64,
            Self::TooFewTrades { min_required, .. } => *min_required as f64,
            Self::Inactive { max_allowed, .. } => *max_allowed as f64,
            Self::ExecutionMaster { threshold, .. } => *threshold,
            Self::TailRiskSeller { .. } => 0.0,
            Self::NoiseTrader { .. } => 0.0,
            Self::SniperInsider { .. } => 0.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Stage1Config {
    pub min_wallet_age_days: u32,
    pub min_total_trades: u32,
    pub max_inactive_days: u32,
}

/// Returns Some(reason) if the wallet should be excluded, None if it passes.
pub fn stage1_filter(
    wallet_age_days: u32,
    total_trades: u32,
    days_since_last_trade: u32,
    config: &Stage1Config,
) -> Option<ExclusionReason> {
    if wallet_age_days < config.min_wallet_age_days {
        return Some(ExclusionReason::TooYoung {
            age_days: wallet_age_days,
            min_required: config.min_wallet_age_days,
        });
    }
    if total_trades < config.min_total_trades {
        return Some(ExclusionReason::TooFewTrades {
            total: total_trades,
            min_required: config.min_total_trades,
        });
    }
    if days_since_last_trade > config.max_inactive_days {
        return Some(ExclusionReason::Inactive {
            days_since_last: days_since_last_trade,
            max_allowed: config.max_inactive_days,
        });
    }
    None
}

/// Record an exclusion in the wallet_exclusions table.
pub fn record_exclusion(
    conn: &Connection,
    proxy_wallet: &str,
    reason: &ExclusionReason,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold, excluded_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        rusqlite::params![
            proxy_wallet,
            reason.reason_str(),
            reason.metric_value(),
            reason.threshold(),
        ],
    )?;
    Ok(())
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p evaluator test_stage1`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/evaluator/src/persona_classification.rs
git commit -m "feat: Stage 1 fast filters — age, trade count, activity checks with exclusion recording"
```

---

## Task 5: Informed Specialist Detector

**Files:**
- Modify: `crates/evaluator/src/persona_classification.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_detect_informed_specialist() {
    let features = WalletFeatures {
        proxy_wallet: "0xabc".to_string(),
        window_days: 30,
        trade_count: 40,
        win_count: 28,  // 70% win rate (28/40)
        loss_count: 12,
        total_pnl: 500.0,
        avg_position_size: 200.0,
        unique_markets: 5,  // < 10 = specialist
        avg_hold_time_hours: 24.0,
        max_drawdown_pct: 8.0,
        trades_per_week: 10.0,
        sharpe_ratio: 1.5,
    };
    let persona = detect_informed_specialist(&features, 10, 0.60);
    assert_eq!(persona, Some(Persona::InformedSpecialist));
}

#[test]
fn test_not_specialist_too_many_markets() {
    let features = WalletFeatures {
        proxy_wallet: "0xabc".to_string(),
        window_days: 30,
        trade_count: 40,
        win_count: 28,
        loss_count: 12,
        total_pnl: 500.0,
        avg_position_size: 200.0,
        unique_markets: 25,  // > 10 = NOT specialist
        avg_hold_time_hours: 24.0,
        max_drawdown_pct: 8.0,
        trades_per_week: 10.0,
        sharpe_ratio: 1.5,
    };
    let persona = detect_informed_specialist(&features, 10, 0.60);
    assert_eq!(persona, None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_detect_informed`
Expected: FAIL

**Step 3: Implement**

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Persona {
    InformedSpecialist,
    ConsistentGeneralist,
    PatientAccumulator,
}

impl Persona {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InformedSpecialist => "INFORMED_SPECIALIST",
            Self::ConsistentGeneralist => "CONSISTENT_GENERALIST",
            Self::PatientAccumulator => "PATIENT_ACCUMULATOR",
        }
    }

    pub fn follow_mode(&self) -> &'static str {
        match self {
            Self::InformedSpecialist => "mirror_with_delay",
            Self::ConsistentGeneralist => "mirror",
            Self::PatientAccumulator => "mirror_slow",
        }
    }
}

pub fn detect_informed_specialist(
    features: &WalletFeatures,
    max_markets: u32,
    min_win_rate: f64,
) -> Option<Persona> {
    if features.unique_markets > max_markets {
        return None;
    }
    let total_resolved = features.win_count + features.loss_count;
    if total_resolved == 0 {
        return None;
    }
    let win_rate = features.win_count as f64 / total_resolved as f64;
    if win_rate < min_win_rate {
        return None;
    }
    Some(Persona::InformedSpecialist)
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator test_detect_informed`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: Informed Specialist persona detector"
```

---

## Task 6: Consistent Generalist Detector

**Files:**
- Modify: `crates/evaluator/src/persona_classification.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_detect_consistent_generalist() {
    let features = WalletFeatures {
        proxy_wallet: "0xabc".to_string(),
        window_days: 30,
        trade_count: 100,
        win_count: 55,  // 55% win rate
        loss_count: 45,
        total_pnl: 200.0,
        avg_position_size: 100.0,
        unique_markets: 25,  // > 20
        avg_hold_time_hours: 12.0,
        max_drawdown_pct: 10.0,  // < 15%
        trades_per_week: 25.0,
        sharpe_ratio: 1.2,  // > 1.0
    };
    let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
    assert_eq!(persona, Some(Persona::ConsistentGeneralist));
}

#[test]
fn test_not_generalist_low_sharpe() {
    let features = WalletFeatures {
        proxy_wallet: "0xabc".to_string(),
        window_days: 30,
        trade_count: 100,
        win_count: 55,
        loss_count: 45,
        total_pnl: 200.0,
        avg_position_size: 100.0,
        unique_markets: 25,
        avg_hold_time_hours: 12.0,
        max_drawdown_pct: 10.0,
        trades_per_week: 25.0,
        sharpe_ratio: 0.5,  // < 1.0
    };
    let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
    assert_eq!(persona, None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_detect_consistent`
Expected: FAIL

**Step 3: Implement**

```rust
pub fn detect_consistent_generalist(
    features: &WalletFeatures,
    min_markets: u32,
    min_win_rate: f64,
    max_win_rate: f64,
    max_drawdown: f64,
    min_sharpe: f64,
) -> Option<Persona> {
    if features.unique_markets < min_markets {
        return None;
    }
    let total_resolved = features.win_count + features.loss_count;
    if total_resolved == 0 {
        return None;
    }
    let win_rate = features.win_count as f64 / total_resolved as f64;
    if win_rate < min_win_rate || win_rate > max_win_rate {
        return None;
    }
    if features.max_drawdown_pct > max_drawdown {
        return None;
    }
    if features.sharpe_ratio < min_sharpe {
        return None;
    }
    Some(Persona::ConsistentGeneralist)
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator test_detect_consistent`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: Consistent Generalist persona detector"
```

---

## Task 7: Patient Accumulator Detector

**Files:**
- Modify: `crates/evaluator/src/persona_classification.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_detect_patient_accumulator() {
    let features = WalletFeatures {
        proxy_wallet: "0xabc".to_string(),
        window_days: 30,
        trade_count: 12,
        win_count: 8,
        loss_count: 4,
        total_pnl: 800.0,
        avg_position_size: 2000.0,  // large positions
        unique_markets: 3,
        avg_hold_time_hours: 72.0,  // > 48h
        max_drawdown_pct: 5.0,
        trades_per_week: 3.0,  // < 5
        sharpe_ratio: 0.8,
    };
    let persona = detect_patient_accumulator(&features, 48.0, 5.0);
    assert_eq!(persona, Some(Persona::PatientAccumulator));
}

#[test]
fn test_not_accumulator_too_frequent() {
    let features = WalletFeatures {
        proxy_wallet: "0xabc".to_string(),
        window_days: 30,
        trade_count: 60,
        win_count: 40,
        loss_count: 20,
        total_pnl: 800.0,
        avg_position_size: 2000.0,
        unique_markets: 3,
        avg_hold_time_hours: 72.0,
        max_drawdown_pct: 5.0,
        trades_per_week: 15.0,  // > 5
        sharpe_ratio: 0.8,
    };
    let persona = detect_patient_accumulator(&features, 48.0, 5.0);
    assert_eq!(persona, None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_detect_patient`
Expected: FAIL

**Step 3: Implement**

```rust
pub fn detect_patient_accumulator(
    features: &WalletFeatures,
    min_hold_hours: f64,
    max_trades_per_week: f64,
) -> Option<Persona> {
    if features.avg_hold_time_hours < min_hold_hours {
        return None;
    }
    if features.trades_per_week > max_trades_per_week {
        return None;
    }
    Some(Persona::PatientAccumulator)
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator test_detect_patient`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: Patient Accumulator persona detector"
```

---

## Task 8: Execution Master Detector (Exclusion)

**Files:**
- Modify: `crates/evaluator/src/persona_classification.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_detect_execution_master() {
    // Wallet where 80% of PnL comes from execution edge (buying below mid)
    let result = detect_execution_master(0.80, 0.70);
    assert_eq!(result, Some(ExclusionReason::ExecutionMaster {
        execution_pnl_ratio: 0.80,
        threshold: 0.70,
    }));
}

#[test]
fn test_not_execution_master() {
    let result = detect_execution_master(0.30, 0.70);
    assert_eq!(result, None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_detect_execution`
Expected: FAIL

**Step 3: Implement**

```rust
/// Detects wallets whose profit comes primarily from execution edge (unreplicable).
/// execution_pnl_ratio = execution_pnl / total_pnl (from PnL decomposition).
/// If ratio > threshold, this wallet's edge is in execution, not direction.
pub fn detect_execution_master(
    execution_pnl_ratio: f64,
    threshold: f64,
) -> Option<ExclusionReason> {
    if execution_pnl_ratio > threshold {
        Some(ExclusionReason::ExecutionMaster {
            execution_pnl_ratio,
            threshold,
        })
    } else {
        None
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator test_detect_execution`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: Execution Master detector (exclusion persona)"
```

---

## Task 9: Tail Risk Seller Detector (Exclusion)

**Files:**
- Modify: `crates/evaluator/src/persona_classification.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_detect_tail_risk_seller() {
    // 85% win rate but max single loss is 8x average win
    let result = detect_tail_risk_seller(0.85, 8.0, 0.80, 5.0);
    assert_eq!(result, Some(ExclusionReason::TailRiskSeller {
        win_rate: 0.85,
        max_loss_ratio: 8.0,
    }));
}

#[test]
fn test_not_tail_risk_seller_low_win_rate() {
    let result = detect_tail_risk_seller(0.55, 8.0, 0.80, 5.0);
    assert_eq!(result, None);
}

#[test]
fn test_not_tail_risk_seller_small_losses() {
    let result = detect_tail_risk_seller(0.85, 2.0, 0.80, 5.0);
    assert_eq!(result, None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_detect_tail`
Expected: FAIL

**Step 3: Implement**

```rust
/// Detects wallets with very high win rate but occasional catastrophic losses.
/// These look great on paper but will eventually blow up.
pub fn detect_tail_risk_seller(
    win_rate: f64,
    max_loss_vs_avg_win: f64,
    min_win_rate_threshold: f64,
    loss_multiplier_threshold: f64,
) -> Option<ExclusionReason> {
    if win_rate > min_win_rate_threshold && max_loss_vs_avg_win > loss_multiplier_threshold {
        Some(ExclusionReason::TailRiskSeller {
            win_rate,
            max_loss_ratio: max_loss_vs_avg_win,
        })
    } else {
        None
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator test_detect_tail`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: Tail Risk Seller detector (exclusion persona)"
```

---

## Task 10: Noise Trader Detector (Exclusion)

**Files:**
- Modify: `crates/evaluator/src/persona_classification.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_detect_noise_trader() {
    // 60 trades/week with near-zero ROI = pure noise
    let result = detect_noise_trader(60.0, 0.005, 50.0, 0.02);
    assert_eq!(result, Some(ExclusionReason::NoiseTrader {
        trades_per_week: 60.0,
        abs_roi: 0.005,
    }));
}

#[test]
fn test_not_noise_low_frequency() {
    let result = detect_noise_trader(10.0, 0.005, 50.0, 0.02);
    assert_eq!(result, None);
}

#[test]
fn test_not_noise_significant_roi() {
    let result = detect_noise_trader(60.0, 0.10, 50.0, 0.02);
    assert_eq!(result, None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_detect_noise`
Expected: FAIL

**Step 3: Implement**

```rust
/// Detects high-churn wallets with no statistical edge.
/// High frequency + near-zero ROI = noise.
pub fn detect_noise_trader(
    trades_per_week: f64,
    abs_roi: f64,
    max_trades_per_week: f64,
    max_abs_roi: f64,
) -> Option<ExclusionReason> {
    if trades_per_week > max_trades_per_week && abs_roi < max_abs_roi {
        Some(ExclusionReason::NoiseTrader {
            trades_per_week,
            abs_roi,
        })
    } else {
        None
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator test_detect_noise`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: Noise Trader detector (exclusion persona)"
```

---

## Task 11: Sniper/Insider Detector (Exclusion)

**Files:**
- Modify: `crates/evaluator/src/persona_classification.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_detect_sniper() {
    // New wallet (15 days), 90% win rate on 12 trades = suspicious
    let result = detect_sniper_insider(15, 0.90, 12, 30, 0.85, 20);
    assert_eq!(result, Some(ExclusionReason::SniperInsider {
        age_days: 15,
        win_rate: 0.90,
        trade_count: 12,
    }));
}

#[test]
fn test_not_sniper_old_wallet() {
    let result = detect_sniper_insider(180, 0.90, 12, 30, 0.85, 20);
    assert_eq!(result, None);
}

#[test]
fn test_not_sniper_normal_win_rate() {
    let result = detect_sniper_insider(15, 0.55, 12, 30, 0.85, 20);
    assert_eq!(result, None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_detect_sniper`
Expected: FAIL

**Step 3: Implement**

```rust
/// Detects suspiciously new wallets with anomalous win rates.
/// Young + high win rate + few trades = likely insider or lucky sniper.
pub fn detect_sniper_insider(
    wallet_age_days: u32,
    win_rate: f64,
    trade_count: u32,
    max_age_days: u32,
    min_win_rate: f64,
    max_trades: u32,
) -> Option<ExclusionReason> {
    if wallet_age_days < max_age_days && win_rate > min_win_rate && trade_count < max_trades {
        Some(ExclusionReason::SniperInsider {
            age_days: wallet_age_days,
            win_rate,
            trade_count,
        })
    } else {
        None
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator test_detect_sniper`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: Sniper/Insider detector (exclusion persona)"
```

---

## Task 12: Persona Classification Orchestrator + Stage 2 Job

This ties all persona detectors together into a single function that classifies a wallet and records the result.

**Files:**
- Modify: `crates/evaluator/src/persona_classification.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_classify_wallet_informed_specialist() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    let features = WalletFeatures {
        proxy_wallet: "0xabc".to_string(),
        window_days: 30,
        trade_count: 40,
        win_count: 28,
        loss_count: 12,
        total_pnl: 500.0,
        avg_position_size: 200.0,
        unique_markets: 5,
        avg_hold_time_hours: 24.0,
        max_drawdown_pct: 8.0,
        trades_per_week: 10.0,
        sharpe_ratio: 1.5,
    };

    let config = PersonaConfig::default_for_test();
    let result = classify_wallet(&db.conn, &features, 90, &config).unwrap();

    assert_eq!(result, ClassificationResult::Followable(Persona::InformedSpecialist));

    // Verify it was persisted to wallet_personas
    let persona: String = db.conn.query_row(
        "SELECT persona FROM wallet_personas WHERE proxy_wallet = '0xabc'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(persona, "INFORMED_SPECIALIST");
}

#[test]
fn test_classify_wallet_excluded_noise_trader() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    let features = WalletFeatures {
        proxy_wallet: "0xnoise".to_string(),
        window_days: 30,
        trade_count: 300,
        win_count: 150,
        loss_count: 150,
        total_pnl: 5.0,
        avg_position_size: 10.0,
        unique_markets: 30,
        avg_hold_time_hours: 0.5,
        max_drawdown_pct: 3.0,
        trades_per_week: 75.0,
        sharpe_ratio: 0.1,
    };

    let config = PersonaConfig::default_for_test();
    let result = classify_wallet(&db.conn, &features, 180, &config).unwrap();

    match result {
        ClassificationResult::Excluded(reason) => {
            assert_eq!(reason.reason_str(), "NOISE_TRADER");
        }
        _ => panic!("Expected exclusion"),
    }

    // Verify exclusion was recorded
    let count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM wallet_exclusions WHERE proxy_wallet = '0xnoise'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_classify_wallet_unclassified() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    // Wallet that doesn't match any followable persona but also isn't excluded
    let features = WalletFeatures {
        proxy_wallet: "0xmid".to_string(),
        window_days: 30,
        trade_count: 50,
        win_count: 25,
        loss_count: 25,
        total_pnl: 20.0,
        avg_position_size: 100.0,
        unique_markets: 15,  // between 10 and 20 — neither specialist nor generalist
        avg_hold_time_hours: 12.0,
        max_drawdown_pct: 8.0,
        trades_per_week: 12.0,
        sharpe_ratio: 0.7,
    };

    let config = PersonaConfig::default_for_test();
    let result = classify_wallet(&db.conn, &features, 180, &config).unwrap();

    assert_eq!(result, ClassificationResult::Unclassified);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_classify_wallet`
Expected: FAIL

**Step 3: Implement the orchestrator**

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ClassificationResult {
    Followable(Persona),
    Excluded(ExclusionReason),
    Unclassified,
}

pub struct PersonaConfig {
    // Specialist
    pub specialist_max_markets: u32,
    pub specialist_min_win_rate: f64,
    // Generalist
    pub generalist_min_markets: u32,
    pub generalist_min_win_rate: f64,
    pub generalist_max_win_rate: f64,
    pub generalist_max_drawdown: f64,
    pub generalist_min_sharpe: f64,
    // Accumulator
    pub accumulator_min_hold_hours: f64,
    pub accumulator_max_trades_per_week: f64,
    // Exclusion: Execution Master
    pub execution_master_pnl_ratio: f64,
    // Exclusion: Tail Risk Seller
    pub tail_risk_min_win_rate: f64,
    pub tail_risk_loss_multiplier: f64,
    // Exclusion: Noise Trader
    pub noise_max_trades_per_week: f64,
    pub noise_max_abs_roi: f64,
    // Exclusion: Sniper
    pub sniper_max_age_days: u32,
    pub sniper_min_win_rate: f64,
    pub sniper_max_trades: u32,
}

impl PersonaConfig {
    #[cfg(test)]
    pub fn default_for_test() -> Self {
        Self {
            specialist_max_markets: 10,
            specialist_min_win_rate: 0.60,
            generalist_min_markets: 20,
            generalist_min_win_rate: 0.52,
            generalist_max_win_rate: 0.60,
            generalist_max_drawdown: 15.0,
            generalist_min_sharpe: 1.0,
            accumulator_min_hold_hours: 48.0,
            accumulator_max_trades_per_week: 5.0,
            execution_master_pnl_ratio: 0.70,
            tail_risk_min_win_rate: 0.80,
            tail_risk_loss_multiplier: 5.0,
            noise_max_trades_per_week: 50.0,
            noise_max_abs_roi: 0.02,
            sniper_max_age_days: 30,
            sniper_min_win_rate: 0.85,
            sniper_max_trades: 20,
        }
    }
}

/// Full classification pipeline for a wallet.
/// Checks exclusions first (order matters — cheapest checks first), then followable personas.
pub fn classify_wallet(
    conn: &Connection,
    features: &WalletFeatures,
    wallet_age_days: u32,
    config: &PersonaConfig,
) -> Result<ClassificationResult> {
    let total_resolved = features.win_count + features.loss_count;
    let win_rate = if total_resolved > 0 {
        features.win_count as f64 / total_resolved as f64
    } else {
        0.0
    };

    let roi = if features.trade_count > 0 && features.avg_position_size > 0.0 {
        features.total_pnl / (features.trade_count as f64 * features.avg_position_size)
    } else {
        0.0
    };

    // --- Exclusion checks (Stage 2) ---

    // Sniper/Insider
    if let Some(reason) = detect_sniper_insider(
        wallet_age_days, win_rate, features.trade_count,
        config.sniper_max_age_days, config.sniper_min_win_rate, config.sniper_max_trades,
    ) {
        record_exclusion(conn, &features.proxy_wallet, &reason)?;
        return Ok(ClassificationResult::Excluded(reason));
    }

    // Noise Trader
    if let Some(reason) = detect_noise_trader(
        features.trades_per_week, roi.abs(),
        config.noise_max_trades_per_week, config.noise_max_abs_roi,
    ) {
        record_exclusion(conn, &features.proxy_wallet, &reason)?;
        return Ok(ClassificationResult::Excluded(reason));
    }

    // Tail Risk Seller
    // Note: max_loss_vs_avg_win requires per-trade loss data; approximate from features
    // For now, use max_drawdown as a proxy for catastrophic loss
    let avg_win_pnl = if features.win_count > 0 {
        features.total_pnl.max(1.0) / features.win_count as f64
    } else {
        1.0
    };
    let max_loss_proxy = features.max_drawdown_pct * features.avg_position_size / 100.0;
    let loss_ratio = if avg_win_pnl > 0.0 { max_loss_proxy / avg_win_pnl } else { 0.0 };

    if let Some(reason) = detect_tail_risk_seller(
        win_rate, loss_ratio,
        config.tail_risk_min_win_rate, config.tail_risk_loss_multiplier,
    ) {
        record_exclusion(conn, &features.proxy_wallet, &reason)?;
        return Ok(ClassificationResult::Excluded(reason));
    }

    // Execution Master — requires PnL decomposition data
    // Placeholder: skip for now (we don't have mid_at_entry data yet)
    // Will be implemented when book_snapshots are available

    // --- Followable persona detection (priority order) ---

    // 1. Informed Specialist (primary target)
    if let Some(persona) = detect_informed_specialist(
        features, config.specialist_max_markets, config.specialist_min_win_rate,
    ) {
        record_persona(conn, &features.proxy_wallet, &persona, win_rate)?;
        return Ok(ClassificationResult::Followable(persona));
    }

    // 2. Consistent Generalist
    if let Some(persona) = detect_consistent_generalist(
        features, config.generalist_min_markets, config.generalist_min_win_rate,
        config.generalist_max_win_rate, config.generalist_max_drawdown, config.generalist_min_sharpe,
    ) {
        record_persona(conn, &features.proxy_wallet, &persona, win_rate)?;
        return Ok(ClassificationResult::Followable(persona));
    }

    // 3. Patient Accumulator
    if let Some(persona) = detect_patient_accumulator(
        features, config.accumulator_min_hold_hours, config.accumulator_max_trades_per_week,
    ) {
        record_persona(conn, &features.proxy_wallet, &persona, win_rate)?;
        return Ok(ClassificationResult::Followable(persona));
    }

    Ok(ClassificationResult::Unclassified)
}

/// Record a followable persona classification.
pub fn record_persona(
    conn: &Connection,
    proxy_wallet: &str,
    persona: &Persona,
    confidence: f64,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
         VALUES (?1, ?2, ?3, datetime('now'))",
        rusqlite::params![proxy_wallet, persona.as_str(), confidence],
    )?;
    Ok(())
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator test_classify_wallet`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: persona classification orchestrator — exclusion-first pipeline with DB persistence"
```

---

## Task 13: Paper Trade Settlement

Paper trades are currently created as `'open'` but never settled. We need to settle them when markets resolve.

**Files:**
- Modify: `crates/evaluator/src/paper_trading.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_settle_paper_trades_win() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    // Insert an open paper trade: BUY YES at 0.60
    db.conn.execute(
        "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, outcome, entry_price, size_usdc, status)
         VALUES ('0xabc', 'mirror', '0xmarket1', 'BUY', 'Yes', 0.60, 25.0, 'open')",
        [],
    ).unwrap();

    // Market resolved: Yes wins (price goes to 1.0)
    let settled = settle_paper_trades_for_market(&db.conn, "0xmarket1", 1.0).unwrap();
    assert_eq!(settled, 1);

    // Verify settlement
    let (status, pnl): (String, f64) = db.conn.query_row(
        "SELECT status, pnl FROM paper_trades WHERE condition_id = '0xmarket1'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(status, "settled_win");
    assert!((pnl - 10.0).abs() < 0.01); // PnL = (1.0 - 0.60) * 25.0 = 10.0
}

#[test]
fn test_settle_paper_trades_loss() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    // BUY YES at 0.60, market resolves to No (price = 0.0)
    db.conn.execute(
        "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, outcome, entry_price, size_usdc, status)
         VALUES ('0xabc', 'mirror', '0xmarket2', 'BUY', 'Yes', 0.60, 25.0, 'open')",
        [],
    ).unwrap();

    let settled = settle_paper_trades_for_market(&db.conn, "0xmarket2", 0.0).unwrap();
    assert_eq!(settled, 1);

    let (status, pnl): (String, f64) = db.conn.query_row(
        "SELECT status, pnl FROM paper_trades WHERE condition_id = '0xmarket2'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(status, "settled_loss");
    assert!((pnl - (-15.0)).abs() < 0.01); // PnL = (0.0 - 0.60) * 25.0 = -15.0
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_settle_paper`
Expected: FAIL — function doesn't exist.

**Step 3: Implement settlement**

Add to `crates/evaluator/src/paper_trading.rs`:

```rust
/// Settle all open paper trades for a market that has resolved.
/// settle_price is 1.0 (outcome won) or 0.0 (outcome lost).
/// Returns number of trades settled.
pub fn settle_paper_trades_for_market(
    conn: &Connection,
    condition_id: &str,
    settle_price: f64,
) -> Result<usize> {
    // Get all open trades for this market
    let mut stmt = conn.prepare(
        "SELECT id, entry_price, size_usdc, side FROM paper_trades
         WHERE condition_id = ?1 AND status = 'open'"
    )?;

    let trades: Vec<(i64, f64, f64, String)> = stmt.query_map(
        rusqlite::params![condition_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?.filter_map(|r| r.ok()).collect();

    let mut settled = 0;

    for (id, entry_price, size_usdc, side) in &trades {
        // PnL calculation:
        // For BUY: pnl = (settle_price - entry_price) * size_usdc
        // For SELL: pnl = (entry_price - settle_price) * size_usdc
        let pnl = if side == "BUY" {
            (settle_price - entry_price) * size_usdc
        } else {
            (entry_price - settle_price) * size_usdc
        };

        let status = if pnl >= 0.0 { "settled_win" } else { "settled_loss" };

        conn.execute(
            "UPDATE paper_trades SET status = ?1, exit_price = ?2, pnl = ?3, settled_at = datetime('now')
             WHERE id = ?4",
            rusqlite::params![status, settle_price, pnl, id],
        )?;

        settled += 1;
    }

    // Clean up paper_positions for this market
    if settled > 0 {
        conn.execute(
            "DELETE FROM paper_positions WHERE condition_id = ?1",
            rusqlite::params![condition_id],
        )?;
    }

    Ok(settled)
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator test_settle_paper`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: paper trade settlement — settle open trades when markets resolve"
```

---

## Task 14: Conditional Taker Fee (Quartic for Crypto, Zero for Everything Else)

**IMPORTANT CONTEXT:** Most Polymarket markets have **zero trading fees**. The quartic taker fee formula (`fee = price * 0.25 * (price * (1 - price))^2`) applies **ONLY to 15-minute crypto markets** (BTC, ETH price prediction markets). Political, sports, weather, and all other event markets have zero fees. See: https://docs.polymarket.com/polymarket-learn/trading/fees

**Files:**
- Modify: `crates/evaluator/src/paper_trading.rs`
- Modify: `crates/common/src/db.rs` (add `is_crypto_15m` column to `markets` table)

**Step 1: Write the failing test**

```rust
#[test]
fn test_quartic_taker_fee() {
    // At p=0.60: fee = 0.60 * 0.25 * (0.60 * 0.40)^2 = 0.60 * 0.25 * 0.0576 = 0.00864
    let fee = quartic_taker_fee(0.60);
    assert!((fee - 0.00864).abs() < 0.0001);
}

#[test]
fn test_quartic_taker_fee_at_extremes() {
    // Near p=0: fee should be ~0
    let fee_low = quartic_taker_fee(0.05);
    assert!(fee_low < 0.001);

    // Near p=1: fee should be ~0
    let fee_high = quartic_taker_fee(0.95);
    assert!(fee_high < 0.001);

    // At p=0.50: fee = 0.50 * 0.25 * (0.50 * 0.50)^2 = 0.50 * 0.25 * 0.0625 = 0.0078125
    let fee_mid = quartic_taker_fee(0.50);
    assert!((fee_mid - 0.0078125).abs() < 0.0001);
}

#[test]
fn test_compute_fee_conditional() {
    // Political market: zero fee
    let fee_political = compute_taker_fee(0.60, false);
    assert!((fee_political - 0.0).abs() < 0.0001);

    // Crypto 15m market: quartic fee
    let fee_crypto = compute_taker_fee(0.60, true);
    assert!((fee_crypto - 0.00864).abs() < 0.0001);
}

#[test]
fn test_detect_crypto_15m_market() {
    // BTC 15-minute markets have slugs like "will-btc-go-above-100000-by-15m"
    // or titles containing "15 min" / "15m" + crypto asset names
    assert!(is_crypto_15m_market("Will BTC go above $100,000 by 15 min?", "btc-15m-above-100k"));
    assert!(is_crypto_15m_market("Will ETH be above $4,000 at 3:15 PM?", "eth-15m-above-4000"));
    assert!(!is_crypto_15m_market("Will Trump win the 2024 election?", "trump-2024-election"));
    assert!(!is_crypto_15m_market("Will Bitcoin reach $200k by 2026?", "bitcoin-200k-2026")); // not 15m
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_quartic`
Expected: FAIL

**Step 3: Implement**

```rust
/// Quartic taker fee on Polymarket.
/// fee = price * 0.25 * (price * (1 - price))^2
/// Max ~1.56% at p=0.50, approaches zero near p=0 or p=1.
/// ONLY applies to 15-minute crypto markets. All other markets have zero fees.
pub fn quartic_taker_fee(price: f64) -> f64 {
    let p = price.clamp(0.0, 1.0);
    p * 0.25 * (p * (1.0 - p)).powi(2)
}

/// Compute the taker fee for a trade. Returns 0.0 for non-crypto markets.
pub fn compute_taker_fee(price: f64, is_crypto_15m: bool) -> f64 {
    if is_crypto_15m {
        quartic_taker_fee(price)
    } else {
        0.0
    }
}

/// Detect if a market is a 15-minute crypto price prediction market.
/// These are the ONLY markets that charge taker fees on Polymarket.
pub fn is_crypto_15m_market(title: &str, slug: &str) -> bool {
    let text = format!("{} {}", title.to_lowercase(), slug.to_lowercase());
    let is_crypto = text.contains("btc") || text.contains("eth") || text.contains("bitcoin") || text.contains("ethereum");
    let is_15m = text.contains("15m") || text.contains("15 min") || text.contains("15-min");
    is_crypto && is_15m
}
```

Then update `mirror_trade_to_paper` to use `compute_taker_fee`:

```rust
// Look up whether this market is a 15m crypto market
let is_crypto_15m: bool = conn.query_row(
    "SELECT COALESCE(is_crypto_15m, 0) FROM markets WHERE condition_id = ?1",
    rusqlite::params![condition_id],
    |row| row.get::<_, bool>(0),
).unwrap_or(false);

// After slippage calculation, before inserting:
let fee = compute_taker_fee(adjusted_price, is_crypto_15m);
let entry_price_with_fee = if side == Side::Buy {
    adjusted_price + fee  // buying costs more
} else {
    adjusted_price - fee  // selling gets less
};
```

Also add `is_crypto_15m` column to `markets` table and populate during market scoring:

```sql
ALTER TABLE markets ADD COLUMN is_crypto_15m BOOLEAN DEFAULT 0;
```

**Step 4: Run tests**

Run: `cargo test -p evaluator`
Expected: ALL PASS (update existing tests if entry prices shifted slightly due to fee)

**Step 5: Commit**

```bash
git commit -am "feat: conditional taker fee — quartic for crypto 15m markets, zero for everything else"
```

---

## Task 15: Copy Fidelity Tracking

Every trade the followed wallet makes gets exactly one outcome: COPIED, SKIPPED_PORTFOLIO_RISK, SKIPPED_WALLET_RISK, etc.

**Files:**
- Modify: `crates/evaluator/src/paper_trading.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_copy_fidelity_event_recorded_on_copy() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();
    // ... insert a wallet, set up trades, call mirror_trade_to_paper ...
    // Verify copy_fidelity_events has a row with outcome = 'COPIED'
    let outcome: String = db.conn.query_row(
        "SELECT outcome FROM copy_fidelity_events WHERE proxy_wallet = '0xabc'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(outcome, "COPIED");
}

#[test]
fn test_copy_fidelity_event_recorded_on_skip() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();
    // ... set up conditions where portfolio stop triggers ...
    // Verify copy_fidelity_events has outcome = 'SKIPPED_PORTFOLIO_RISK'
}

#[test]
fn test_compute_copy_fidelity() {
    // 8 COPIED + 2 SKIPPED = 80% fidelity
    let fidelity = compute_copy_fidelity(8, 2);
    assert!((fidelity - 80.0).abs() < 0.1);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_copy_fidelity`
Expected: FAIL

**Step 3: Implement**

Add copy fidelity event recording to `mirror_trade_to_paper` — at every exit point (copy or skip), insert a row into `copy_fidelity_events`.

```rust
pub fn compute_copy_fidelity(copied: u32, skipped: u32) -> f64 {
    let total = copied + skipped;
    if total == 0 {
        return 100.0;
    }
    (copied as f64 / total as f64) * 100.0
}

fn record_fidelity_event(
    conn: &Connection,
    proxy_wallet: &str,
    condition_id: &str,
    their_trade_id: Option<i64>,
    outcome: &str,
    detail: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO copy_fidelity_events (proxy_wallet, condition_id, their_trade_id, outcome, outcome_detail)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![proxy_wallet, condition_id, their_trade_id, outcome, detail],
    )?;
    Ok(())
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: copy fidelity tracking — record every copy/skip decision"
```

---

## Task 16: Two-Level Risk Management (Per-Wallet + Portfolio)

Upgrade the existing single-level risk checks to the two-level system from the Strategy Bible.

**Files:**
- Modify: `crates/evaluator/src/paper_trading.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_per_wallet_daily_loss_limit() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    // Insert settled losses for wallet today totaling -$19 on $1000 bankroll
    // Per-wallet daily loss limit = 2% = $20
    // A new trade should still be allowed (under limit)
    // ... setup + assert trade is created ...

    // Now add another -$2 loss, totaling -$21 (over 2% of $1000)
    // A new trade should be BLOCKED
    // ... setup + assert trade is blocked with reason ...
}

#[test]
fn test_max_concurrent_positions() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    // Insert 20 open paper positions
    for i in 0..20 {
        db.conn.execute(
            "INSERT INTO paper_positions (proxy_wallet, strategy, condition_id, side, total_size_usdc, avg_entry_price)
             VALUES (?1, 'mirror', ?2, 'BUY', 25.0, 0.50)",
            rusqlite::params![format!("0xwallet{}", i % 3), format!("0xmarket{}", i)],
        ).unwrap();
    }

    // Next trade should be blocked: max_concurrent_positions = 20
    // ... assert blocked ...
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_per_wallet_daily`
Expected: FAIL

**Step 3: Implement enhanced risk checks**

Extend `mirror_trade_to_paper` with additional checks:

```rust
// Per-wallet daily loss check
let wallet_daily_loss: f64 = conn.query_row(
    "SELECT COALESCE(SUM(pnl), 0.0) FROM paper_trades
     WHERE proxy_wallet = ?1 AND status != 'open'
     AND settled_at >= datetime('now', 'start of day')",
    rusqlite::params![proxy_wallet],
    |row| row.get(0),
)?;
if wallet_daily_loss.abs() > bankroll * per_wallet_daily_loss_pct / 100.0 {
    // record fidelity event: SKIPPED_WALLET_RISK
    return Ok(MirrorDecision { inserted: false, reason: Some("per_wallet_daily_loss_exceeded") });
}

// Concurrent positions check
let concurrent: u32 = conn.query_row(
    "SELECT COUNT(*) FROM paper_positions",
    [], |row| row.get(0),
)?;
if concurrent >= max_concurrent_positions {
    return Ok(MirrorDecision { inserted: false, reason: Some("max_concurrent_positions") });
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: two-level risk management — per-wallet daily loss + concurrent positions limits"
```

---

## Task 17: Follower Slippage Tracking

Track the gap between the followed wallet's entry and our paper entry — this is the critical metric for copy-trading viability.

**Files:**
- Modify: `crates/evaluator/src/paper_trading.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_follower_slippage_recorded() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    // Their trade: BUY at 0.55
    // Our paper entry: 0.55 + 1% slippage + fee = ~0.564
    // ... call mirror_trade_to_paper ...

    let slippage: f64 = db.conn.query_row(
        "SELECT slippage_cents FROM follower_slippage WHERE proxy_wallet = '0xabc'",
        [], |row| row.get(0),
    ).unwrap();
    assert!(slippage > 0.0); // we pay more than they did
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_follower_slippage`
Expected: FAIL

**Step 3: Implement**

After a successful paper trade creation in `mirror_trade_to_paper`, insert a slippage record:

```rust
conn.execute(
    "INSERT INTO follower_slippage
     (proxy_wallet, condition_id, their_entry_price, our_entry_price, slippage_cents, fee_applied, their_trade_id, our_paper_trade_id)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    rusqlite::params![
        proxy_wallet, condition_id,
        observed_price, entry_price_with_fee,
        (entry_price_with_fee - observed_price) * 100.0, // in cents
        fee,
        triggered_by_trade_id,
        paper_trade_id,
    ],
)?;
```

**Step 4: Run tests**

Run: `cargo test -p evaluator`
Expected: PASS

**Step 5: Commit**

```bash
git commit -am "feat: follower slippage tracking — record entry price gap per trade"
```

---

## Task 18: WScore — Missing 3 Sub-Components

Currently only edge_score and consistency_score are implemented. Add market_skill_score, timing_skill_score, behavior_quality_score.

**Files:**
- Modify: `crates/evaluator/src/wallet_scoring.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_market_skill_score() {
    // Profitable in 3 out of 5 markets = 0.6
    let score = market_skill_score(3, 5);
    assert!((score - 0.6).abs() < 0.01);
}

#[test]
fn test_timing_skill_score() {
    // Average post-entry drift of +5 cents = good timing
    let score = timing_skill_score(5.0);
    assert!(score > 0.5);

    // Average post-entry drift of -3 cents = bad timing
    let score = timing_skill_score(-3.0);
    assert!(score < 0.5);
}

#[test]
fn test_behavior_quality_score() {
    // 5% noise trades = high quality
    let score = behavior_quality_score(0.05);
    assert!(score > 0.9);

    // 50% noise trades = low quality
    let score = behavior_quality_score(0.50);
    assert!(score < 0.6);
}

#[test]
fn test_full_wscore_all_5_components() {
    let input = WalletScoreInput {
        paper_roi_pct: 10.0,
        daily_return_stdev_pct: 3.0,
        profitable_markets: 5,
        total_markets: 8,
        avg_post_entry_drift_cents: 3.0,
        noise_trade_ratio: 0.10,
    };
    let weights = WScoreWeights {
        edge: 0.30,
        consistency: 0.25,
        market_skill: 0.20,
        timing_skill: 0.15,
        behavior_quality: 0.10,
    };
    let score = compute_wscore(&input, &weights);
    assert!(score > 0.0 && score <= 1.0);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_market_skill`
Expected: FAIL

**Step 3: Implement the 3 missing components**

```rust
/// Market skill: fraction of markets that were profitable.
pub fn market_skill_score(profitable_markets: u32, total_markets: u32) -> f64 {
    if total_markets == 0 { return 0.0; }
    (profitable_markets as f64 / total_markets as f64).clamp(0.0, 1.0)
}

/// Timing skill: did price move in our favor after entry?
/// avg_drift_cents > 0 = good timing, < 0 = bad timing.
/// Normalized: 0 at -10 cents, 0.5 at 0, 1.0 at +10 cents.
pub fn timing_skill_score(avg_post_entry_drift_cents: f64) -> f64 {
    let normalized = (avg_post_entry_drift_cents + 10.0) / 20.0;
    normalized.clamp(0.0, 1.0)
}

/// Behavior quality: fewer noise trades = higher quality.
/// noise_trade_ratio = 0 -> score 1.0, noise_trade_ratio = 1 -> score 0.0
pub fn behavior_quality_score(noise_trade_ratio: f64) -> f64 {
    (1.0 - noise_trade_ratio).clamp(0.0, 1.0)
}
```

Update `WalletScoreInput` and `WScoreWeights` to include all 5 fields. Update `compute_wscore` to use all 5 weighted components.

**Step 4: Run tests**

Run: `cargo test -p evaluator`
Expected: ALL PASS (update existing wscore tests for new struct fields)

**Step 5: Commit**

```bash
git commit -am "feat: WScore complete — market_skill, timing_skill, behavior_quality added"
```

---

## Task 19: Weekly Re-evaluation + Anomaly Detection

**Files:**
- Create: `crates/evaluator/src/anomaly_detection.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_detect_win_rate_drop() {
    let anomalies = detect_anomalies(
        0.60,   // historical_win_rate
        0.40,   // current_win_rate (dropped 20pp)
        5.0,    // weekly_drawdown_pct
        10.0,   // current_trades_per_week
        10.0,   // historical_trades_per_week
        100.0,  // current_max_trade_size
        50.0,   // historical_avg_trade_size
        &AnomalyConfig { win_rate_drop_pct: 15.0, max_weekly_drawdown_pct: 20.0, frequency_change_multiplier: 3.0, size_change_multiplier: 10.0 },
    );
    assert!(anomalies.contains(&AnomalyType::WinRateDrop { drop_pct: 20.0 }));
}

#[test]
fn test_detect_frequency_spike() {
    let anomalies = detect_anomalies(
        0.55, 0.55,
        5.0,
        40.0,   // 4x historical frequency
        10.0,
        100.0, 50.0,
        &AnomalyConfig { win_rate_drop_pct: 15.0, max_weekly_drawdown_pct: 20.0, frequency_change_multiplier: 3.0, size_change_multiplier: 10.0 },
    );
    assert!(anomalies.iter().any(|a| matches!(a, AnomalyType::FrequencyChange { .. })));
}

#[test]
fn test_no_anomalies_when_normal() {
    let anomalies = detect_anomalies(
        0.55, 0.53,  // minor fluctuation
        5.0,
        12.0, 10.0,
        60.0, 50.0,
        &AnomalyConfig { win_rate_drop_pct: 15.0, max_weekly_drawdown_pct: 20.0, frequency_change_multiplier: 3.0, size_change_multiplier: 10.0 },
    );
    assert!(anomalies.is_empty());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_detect_win_rate`
Expected: FAIL

**Step 3: Implement**

```rust
// crates/evaluator/src/anomaly_detection.rs

#[derive(Debug, Clone, PartialEq)]
pub enum AnomalyType {
    WinRateDrop { drop_pct: f64 },
    DrawdownSpike { weekly_drawdown_pct: f64 },
    FrequencyChange { current: f64, historical: f64, multiplier: f64 },
    PositionSizeAnomaly { current_size: f64, historical_avg: f64, multiplier: f64 },
}

pub struct AnomalyConfig {
    pub win_rate_drop_pct: f64,
    pub max_weekly_drawdown_pct: f64,
    pub frequency_change_multiplier: f64,
    pub size_change_multiplier: f64,
}

pub fn detect_anomalies(
    historical_win_rate: f64,
    current_win_rate: f64,
    weekly_drawdown_pct: f64,
    current_trades_per_week: f64,
    historical_trades_per_week: f64,
    current_max_trade_size: f64,
    historical_avg_trade_size: f64,
    config: &AnomalyConfig,
) -> Vec<AnomalyType> {
    let mut anomalies = Vec::new();

    // Win rate drop
    let drop_pct = (historical_win_rate - current_win_rate) * 100.0;
    if drop_pct > config.win_rate_drop_pct {
        anomalies.push(AnomalyType::WinRateDrop { drop_pct });
    }

    // Drawdown spike
    if weekly_drawdown_pct > config.max_weekly_drawdown_pct {
        anomalies.push(AnomalyType::DrawdownSpike { weekly_drawdown_pct });
    }

    // Frequency change
    if historical_trades_per_week > 0.0 {
        let freq_ratio = current_trades_per_week / historical_trades_per_week;
        if freq_ratio > config.frequency_change_multiplier {
            anomalies.push(AnomalyType::FrequencyChange {
                current: current_trades_per_week,
                historical: historical_trades_per_week,
                multiplier: freq_ratio,
            });
        }
    }

    // Position size anomaly
    if historical_avg_trade_size > 0.0 {
        let size_ratio = current_max_trade_size / historical_avg_trade_size;
        if size_ratio > config.size_change_multiplier {
            anomalies.push(AnomalyType::PositionSizeAnomaly {
                current_size: current_max_trade_size,
                historical_avg: historical_avg_trade_size,
                multiplier: size_ratio,
            });
        }
    }

    anomalies
}
```

**Step 4: Run tests**

Run: `cargo test -p evaluator test_detect_`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/evaluator/src/anomaly_detection.rs
git commit -m "feat: anomaly detection — win rate drop, drawdown spike, frequency/size changes"
```

---

## Task 20: MScore — Real Inputs (Density, Whale Concentration)

Currently `trades_24h` is hardcoded to 0 and `top_holder_concentration` to 0.5. Compute these from actual data.

**Files:**
- Modify: `crates/evaluator/src/jobs.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_compute_trades_24h_from_db() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    let now = chrono::Utc::now().timestamp();
    // Insert 5 trades in last 24h for market 0xm1
    for i in 0..5 {
        db.conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
             VALUES ('0xw1', '0xm1', 'BUY', 10.0, 0.50, ?1)",
            rusqlite::params![now - 3600 * i],
        ).unwrap();
    }
    // Insert 3 old trades (>24h ago)
    for i in 0..3 {
        db.conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
             VALUES ('0xw2', '0xm1', 'BUY', 10.0, 0.50, ?1)",
            rusqlite::params![now - 86400 - 3600 * i],
        ).unwrap();
    }

    let trades_24h = count_trades_24h(&db.conn, "0xm1", now).unwrap();
    assert_eq!(trades_24h, 5);

    let unique_traders = count_unique_traders_24h(&db.conn, "0xm1", now).unwrap();
    assert_eq!(unique_traders, 1);  // only 0xw1 traded in last 24h
}

#[test]
fn test_compute_whale_concentration_from_holders() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    // Top holder has 500 out of 1000 total = 50% concentration
    db.conn.execute(
        "INSERT INTO holders_snapshots (condition_id, proxy_wallet, amount, snapshot_at)
         VALUES ('0xm1', '0xwhale', 500.0, datetime('now'))",
        [],
    ).unwrap();
    db.conn.execute(
        "INSERT INTO holders_snapshots (condition_id, proxy_wallet, amount, snapshot_at)
         VALUES ('0xm1', '0xsmall1', 300.0, datetime('now'))",
        [],
    ).unwrap();
    db.conn.execute(
        "INSERT INTO holders_snapshots (condition_id, proxy_wallet, amount, snapshot_at)
         VALUES ('0xm1', '0xsmall2', 200.0, datetime('now'))",
        [],
    ).unwrap();

    let concentration = compute_whale_concentration(&db.conn, "0xm1").unwrap();
    assert!((concentration - 0.5).abs() < 0.01);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_compute_trades_24h`
Expected: FAIL

**Step 3: Implement**

```rust
pub fn count_trades_24h(conn: &Connection, condition_id: &str, now_epoch: i64) -> Result<u32> {
    let cutoff = now_epoch - 86400;
    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM trades_raw WHERE condition_id = ?1 AND timestamp >= ?2",
        rusqlite::params![condition_id, cutoff],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn count_unique_traders_24h(conn: &Connection, condition_id: &str, now_epoch: i64) -> Result<u32> {
    let cutoff = now_epoch - 86400;
    let count: u32 = conn.query_row(
        "SELECT COUNT(DISTINCT proxy_wallet) FROM trades_raw WHERE condition_id = ?1 AND timestamp >= ?2",
        rusqlite::params![condition_id, cutoff],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn compute_whale_concentration(conn: &Connection, condition_id: &str) -> Result<f64> {
    // Get the latest snapshot
    let total: f64 = conn.query_row(
        "SELECT COALESCE(SUM(amount), 0.0) FROM holders_snapshots
         WHERE condition_id = ?1
         AND snapshot_at = (SELECT MAX(snapshot_at) FROM holders_snapshots WHERE condition_id = ?1)",
        rusqlite::params![condition_id],
        |row| row.get(0),
    )?;

    if total <= 0.0 {
        return Ok(0.5); // default when no data
    }

    let top_holder: f64 = conn.query_row(
        "SELECT COALESCE(MAX(amount), 0.0) FROM holders_snapshots
         WHERE condition_id = ?1
         AND snapshot_at = (SELECT MAX(snapshot_at) FROM holders_snapshots WHERE condition_id = ?1)",
        rusqlite::params![condition_id],
        |row| row.get(0),
    )?;

    Ok(top_holder / total)
}
```

Then update `run_market_scoring_once` in `jobs.rs` to call these functions instead of hardcoding.

**Step 4: Run tests**

Run: `cargo test -p evaluator`
Expected: ALL PASS

**Step 5: Commit**

```bash
git commit -am "feat: MScore real inputs — compute trades_24h, unique_traders, whale_concentration from DB"
```

---

## Task 21: Wire New Jobs into Scheduler

Connect persona classification, wallet feature computation, anomaly detection, and paper trade settlement as scheduled jobs.

**Files:**
- Modify: `crates/evaluator/src/jobs.rs`
- Modify: `crates/evaluator/src/main.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_run_persona_classification_job() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    // Insert a wallet with enough trades
    db.conn.execute(
        "INSERT INTO wallets (proxy_wallet, discovered_from, discovered_at)
         VALUES ('0xabc', 'HOLDER', datetime('now', '-60 days'))",
        [],
    ).unwrap();

    let now = chrono::Utc::now().timestamp();
    // Insert 40 trades over the last 30 days
    for i in 0..40 {
        let ts = now - (i * 3600 * 12); // every 12 hours
        db.conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
             VALUES ('0xabc', ?1, 'BUY', 50.0, 0.65, ?2)",
            rusqlite::params![format!("0xm{}", i % 5), ts],
        ).unwrap();
    }

    run_persona_classification_once(&db, &PersonaConfig::default_for_test()).unwrap();

    // Wallet should have a persona or exclusion recorded
    let persona_count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM wallet_personas WHERE proxy_wallet = '0xabc'",
        [], |row| row.get(0),
    ).unwrap();
    let exclusion_count: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM wallet_exclusions WHERE proxy_wallet = '0xabc'",
        [], |row| row.get(0),
    ).unwrap();
    assert!(persona_count + exclusion_count > 0, "Wallet should be classified or excluded");
}

#[test]
fn test_run_settlement_job() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();

    // Insert an open paper trade for a market that has resolved
    db.conn.execute(
        "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, outcome, entry_price, size_usdc, status)
         VALUES ('0xabc', 'mirror', '0xresolved', 'BUY', 'Yes', 0.60, 25.0, 'open')",
        [],
    ).unwrap();

    // Mark market as closed in markets table
    db.conn.execute(
        "INSERT INTO markets (condition_id, title) VALUES ('0xresolved', 'Resolved Market')",
        [],
    ).unwrap();

    // The settlement job checks positions_snapshots or market end_date to detect resolution
    // For this test, we directly call the settlement function
    let settled = settle_paper_trades_for_market(&db.conn, "0xresolved", 1.0).unwrap();
    assert_eq!(settled, 1);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_run_persona`
Expected: FAIL

**Step 3: Implement the job functions and wire them**

Add to `jobs.rs`:

```rust
pub fn run_persona_classification_once(
    db: &Database,
    config: &PersonaConfig,
) -> Result<()> {
    // Get all active wallets that haven't been classified recently
    let wallets: Vec<(String, String)> = db.conn.prepare(
        "SELECT w.proxy_wallet, w.discovered_at FROM wallets w
         WHERE w.is_active = 1
         AND w.proxy_wallet NOT IN (SELECT proxy_wallet FROM wallet_personas WHERE classified_at > datetime('now', '-7 days'))
         AND w.proxy_wallet NOT IN (SELECT proxy_wallet FROM wallet_exclusions WHERE excluded_at > datetime('now', '-7 days'))"
    )?.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.filter_map(|r| r.ok()).collect();

    for (proxy_wallet, discovered_at) in &wallets {
        let now = chrono::Utc::now().timestamp();
        let features = compute_wallet_features(&db.conn, proxy_wallet, 30, now)?;

        // Compute wallet age
        let wallet_age_days = /* compute from discovered_at */ 60; // placeholder

        classify_wallet(&db.conn, &features, wallet_age_days, config)?;
    }
    Ok(())
}
```

Add to `main.rs` — new scheduled jobs:

```rust
// Persona classification: daily
// Wallet features: daily
// Settlement check: every 6 hours
// Anomaly detection: daily
```

**Step 4: Run tests**

Run: `cargo test -p evaluator`
Expected: ALL PASS

**Step 5: Commit**

```bash
git commit -am "feat: wire persona classification, settlement, anomaly detection into scheduler"
```

---

## Task 22: CLOB API Client + `book_snapshots` Table

**Context:** Orderbook/depth data is completely missing from the system. `book_snapshots` is referenced in CLAUDE.md and the Makefile but never implemented. This task adds the REST API client for fetching orderbook snapshots and the database table to store them. Task 23 adds WebSocket streaming on top.

**API Reference:**
- `GET https://clob.polymarket.com/book?token_id={token_id}` — NO AUTH REQUIRED (public)
- `POST https://clob.polymarket.com/books` — batch endpoint, up to 500 tokens per request
- Rate limits: `/book` = 1500 req/10s, `/books` = 500 req/10s
- Response: `{ bids: [{price, size}], asks: [{price, size}], market: condition_id, asset_id: token_id, hash, timestamp }`

**Key mapping:** Book endpoints use `token_id` (outcome token), NOT `condition_id` (market). Each market has 2 tokens (Yes/No). We need to map condition_id → token_ids via the Gamma API `tokens` array, which is already fetched during market discovery.

**Files:**
- Modify: `crates/common/src/polymarket.rs` — add CLOB client methods
- Modify: `crates/common/src/db.rs` — add `book_snapshots` table + token_id columns on `markets`
- Modify: `crates/common/src/types.rs` — add orderbook response types
- Modify: `crates/common/src/config.rs` — add `[clob]` config section
- Modify: `config/default.toml` — add CLOB config

**Step 1: Write the failing test**

```rust
// In crates/common/src/types.rs tests
#[test]
fn test_deserialize_book_response() {
    let json = r#"{
        "market": "0xbd31dc8a20211944f6b70f31557f1001557b59905b7738480ca09bd4532f84af",
        "asset_id": "65818619657568813474341868652308942079804919287380422192892211131408793125422",
        "bids": [
            {"price": "0.48", "size": "30"},
            {"price": "0.49", "size": "20"},
            {"price": "0.50", "size": "15"}
        ],
        "asks": [
            {"price": "0.52", "size": "25"},
            {"price": "0.53", "size": "60"},
            {"price": "0.54", "size": "10"}
        ],
        "hash": "0xabc123",
        "timestamp": "1700000000000"
    }"#;
    let book: BookResponse = serde_json::from_str(json).unwrap();
    assert_eq!(book.bids.len(), 3);
    assert_eq!(book.asks.len(), 3);
    assert_eq!(book.bids[0].price, "0.48");
    assert_eq!(book.bids[0].size, "30");
}

// In crates/common/src/db.rs tests
#[test]
fn test_book_snapshots_schema() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();
    db.conn.execute(
        "INSERT INTO book_snapshots (condition_id, token_id, best_bid, best_ask, bid_depth_usd, ask_depth_usd, spread_cents, mid_price, levels_json, snapshot_at)
         VALUES ('0xm1', 'token1', 0.48, 0.52, 65.0, 95.0, 4.0, 0.50, '{}', datetime('now'))",
        [],
    ).unwrap();
    let count: i64 = db.conn.query_row("SELECT COUNT(*) FROM book_snapshots", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
}

// Integration test: hit real CLOB API
#[tokio::test]
async fn test_fetch_orderbook_live() {
    // Use a known active market token_id
    // First, get a market from Gamma API to find its token_ids
    let client = PolymarketClient::new(&config);
    let markets = client.fetch_markets(1, 0).await.unwrap();
    let market = &markets[0];
    // Extract first token_id from market.tokens
    let token_id = &market.tokens[0].token_id;
    let book = client.fetch_orderbook(token_id).await.unwrap();
    assert!(!book.bids.is_empty() || !book.asks.is_empty());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p common test_deserialize_book`
Expected: FAIL — `BookResponse` doesn't exist

**Step 3: Implement**

Add types:

```rust
// crates/common/src/types.rs
#[derive(Debug, Deserialize, Clone)]
pub struct OrderLevel {
    pub price: String,
    pub size: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BookResponse {
    pub market: Option<String>,   // condition_id
    pub asset_id: Option<String>, // token_id
    pub bids: Vec<OrderLevel>,
    pub asks: Vec<OrderLevel>,
    pub hash: Option<String>,
    pub timestamp: Option<String>,
}

impl BookResponse {
    /// Best bid price, parsed to f64
    pub fn best_bid(&self) -> Option<f64> {
        self.bids.iter().filter_map(|l| l.price.parse::<f64>().ok()).max_by(|a, b| a.partial_cmp(b).unwrap())
    }

    /// Best ask price, parsed to f64
    pub fn best_ask(&self) -> Option<f64> {
        self.asks.iter().filter_map(|l| l.price.parse::<f64>().ok()).min_by(|a, b| a.partial_cmp(b).unwrap())
    }

    /// Mid price = (best_bid + best_ask) / 2
    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
            _ => None,
        }
    }

    /// Spread in cents = (best_ask - best_bid) * 100
    pub fn spread_cents(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((ask - bid) * 100.0),
            _ => None,
        }
    }

    /// Total bid depth in USD (sum of price * size for all bid levels)
    pub fn bid_depth_usd(&self) -> f64 {
        self.bids.iter()
            .filter_map(|l| {
                let price = l.price.parse::<f64>().ok()?;
                let size = l.size.parse::<f64>().ok()?;
                Some(price * size)
            })
            .sum()
    }

    /// Total ask depth in USD (sum of price * size for all ask levels)
    pub fn ask_depth_usd(&self) -> f64 {
        self.asks.iter()
            .filter_map(|l| {
                let price = l.price.parse::<f64>().ok()?;
                let size = l.size.parse::<f64>().ok()?;
                Some(price * size)
            })
            .sum()
    }
}
```

Add CLOB client methods:

```rust
// crates/common/src/polymarket.rs
const CLOB_BASE_URL: &str = "https://clob.polymarket.com";

impl PolymarketClient {
    /// Fetch orderbook for a single token_id (outcome token).
    pub async fn fetch_orderbook(&self, token_id: &str) -> Result<BookResponse> {
        let url = format!("{}/book?token_id={}", CLOB_BASE_URL, token_id);
        let resp = self.client.get(&url).send().await?.json::<BookResponse>().await?;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await; // conservative, rate limit is generous
        Ok(resp)
    }

    /// Fetch orderbooks for multiple token_ids in a single request (max 500).
    pub async fn fetch_orderbooks(&self, token_ids: &[String]) -> Result<Vec<BookResponse>> {
        let url = format!("{}/books", CLOB_BASE_URL);
        // POST body: array of token_ids
        let resp = self.client.post(&url)
            .json(token_ids)
            .send().await?
            .json::<Vec<BookResponse>>().await?;
        Ok(resp)
    }
}
```

Add `book_snapshots` table:

```sql
CREATE TABLE IF NOT EXISTS book_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    condition_id TEXT NOT NULL,
    token_id TEXT NOT NULL,
    best_bid REAL,
    best_ask REAL,
    bid_depth_usd REAL,
    ask_depth_usd REAL,
    spread_cents REAL,
    mid_price REAL,
    levels_json TEXT,   -- full bid/ask levels as JSON for replay
    snapshot_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_book_snapshots_condition ON book_snapshots(condition_id, snapshot_at);
CREATE INDEX IF NOT EXISTS idx_book_snapshots_token ON book_snapshots(token_id, snapshot_at);
```

Add token_id columns to `markets` table:

```sql
ALTER TABLE markets ADD COLUMN yes_token_id TEXT;
ALTER TABLE markets ADD COLUMN no_token_id TEXT;
```

Populate `yes_token_id` and `no_token_id` during market discovery from the Gamma API `tokens` array.

Add config:

```toml
[clob]
base_url = "https://clob.polymarket.com"
book_poll_interval_secs = 60       # REST fallback, WebSocket is primary
batch_size = 50                    # tokens per /books request
```

**Step 4: Run tests**

Run: `cargo test --all`
Expected: ALL PASS

**Step 5: Commit**

```bash
git commit -am "feat: CLOB API client + book_snapshots table — orderbook data infrastructure"
```

---

## Task 23: WebSocket Book Streaming + Recording

**Context:** User chose WebSocket over REST polling from the start. The CLOB WebSocket Market Channel provides real-time book updates. This is a public channel — NO authentication required.

**WebSocket Reference:**
- URL: `wss://ws-subscriptions-clob.polymarket.com/ws/market` (inferred from docs, verify at runtime)
- Subscribe: `{ "type": "MARKET", "assets_ids": ["token_id_1", "token_id_2", ...] }`
- Dynamic subscribe/unsubscribe: `{ "assets_ids": [...], "operation": "subscribe" }` / `{ ..., "operation": "unsubscribe" }`
- No auth needed for market channel

**Messages we receive:**
- `book` — full L2 orderbook snapshot (on subscribe + after each trade)
- `price_change` — incremental updates (new order placed, order cancelled)
- `last_trade_price` — trade events with price/size/side
- `market_resolved` — market resolution events (with `custom_feature_enabled: true`)

**Strategy:** We primarily care about `book` messages (full snapshots). We can also process `price_change` to maintain a local book between full snapshots, but the MVP just records `book` messages.

**Dependencies:** `tokio-tungstenite` for WebSocket, `futures-util` for stream handling.

**Files:**
- Create: `crates/evaluator/src/book_stream.rs` — WebSocket connection + message handling
- Modify: `crates/evaluator/src/main.rs` — spawn book stream task
- Modify: `crates/common/src/config.rs` — add WebSocket config
- Modify: `config/default.toml` — add WebSocket config
- Modify: `Cargo.toml` (evaluator) — add `tokio-tungstenite`, `futures-util` deps

**Step 1: Write the failing test**

```rust
#[test]
fn test_parse_book_ws_message() {
    let msg = r#"{
        "event_type": "book",
        "asset_id": "65818619657568813474341868652308942079804919287380422192892211131408793125422",
        "market": "0xbd31dc8a20211944f6b70f31557f1001557b59905b7738480ca09bd4532f84af",
        "bids": [{"price": ".48", "size": "30"}, {"price": ".49", "size": "20"}],
        "asks": [{"price": ".52", "size": "25"}, {"price": ".53", "size": "60"}],
        "timestamp": "1700000000000",
        "hash": "0xabc"
    }"#;
    let event: WsBookEvent = serde_json::from_str(msg).unwrap();
    assert_eq!(event.event_type, "book");
    assert_eq!(event.bids.len(), 2);
    assert_eq!(event.asks.len(), 2);
    // Note: WS uses ".48" format, not "0.48" — parser must handle both
    assert!(event.best_bid().unwrap() > 0.47);
}

#[test]
fn test_parse_price_change_ws_message() {
    let msg = r#"{
        "event_type": "price_change",
        "market": "0x5f65177b394277fd294cd75650044e32ba009a95022d88a0c1d565897d72f8f1",
        "price_changes": [
            {"asset_id": "token1", "price": "0.5", "size": "200", "side": "BUY", "hash": "abc", "best_bid": "0.5", "best_ask": "1"}
        ],
        "timestamp": "1700000000000"
    }"#;
    let event: WsMarketEvent = serde_json::from_str(msg).unwrap();
    match event {
        WsMarketEvent::PriceChange { price_changes, .. } => {
            assert_eq!(price_changes.len(), 1);
            assert_eq!(price_changes[0].side, "BUY");
        }
        _ => panic!("Expected PriceChange"),
    }
}

#[test]
fn test_parse_market_resolved_ws_message() {
    let msg = r#"{
        "event_type": "market_resolved",
        "market": "0xabc",
        "winning_outcome": "Yes",
        "winning_asset_id": "token1",
        "timestamp": "1700000000000"
    }"#;
    let event: WsMarketEvent = serde_json::from_str(msg).unwrap();
    match event {
        WsMarketEvent::MarketResolved { winning_outcome, .. } => {
            assert_eq!(winning_outcome, "Yes");
        }
        _ => panic!("Expected MarketResolved"),
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_parse_book_ws`
Expected: FAIL — `WsBookEvent` doesn't exist

**Step 3: Implement**

Add WebSocket message types:

```rust
// crates/evaluator/src/book_stream.rs

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct WsBookEvent {
    pub event_type: String,
    pub asset_id: String,
    pub market: String,  // condition_id
    pub bids: Vec<OrderLevel>,
    pub asks: Vec<OrderLevel>,
    pub timestamp: String,
    pub hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "event_type")]
pub enum WsMarketEvent {
    #[serde(rename = "book")]
    Book(WsBookEvent),
    #[serde(rename = "price_change")]
    PriceChange {
        market: String,
        price_changes: Vec<WsPriceChange>,
        timestamp: String,
    },
    #[serde(rename = "last_trade_price")]
    LastTradePrice {
        asset_id: String,
        market: String,
        price: String,
        size: String,
        side: String,
        timestamp: String,
    },
    #[serde(rename = "market_resolved")]
    MarketResolved {
        market: String,
        winning_outcome: String,
        winning_asset_id: String,
        timestamp: String,
    },
}

#[derive(Debug, Deserialize)]
pub struct WsPriceChange {
    pub asset_id: String,
    pub price: String,
    pub size: String,
    pub side: String,
    pub hash: String,
    pub best_bid: String,
    pub best_ask: String,
}
```

Add WebSocket connection manager:

```rust
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{StreamExt, SinkExt};

const WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

pub struct BookStreamManager {
    db: Arc<Database>,
    subscribed_tokens: Arc<RwLock<HashSet<String>>>,
}

impl BookStreamManager {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            subscribed_tokens: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Main loop: connect, subscribe, process messages, reconnect on failure.
    pub async fn run(&self, token_ids: Vec<String>) -> Result<()> {
        loop {
            match self.connect_and_stream(&token_ids).await {
                Ok(()) => {
                    tracing::info!("WebSocket stream ended normally, reconnecting...");
                }
                Err(e) => {
                    tracing::error!("WebSocket error: {:?}, reconnecting in 5s...", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn connect_and_stream(&self, token_ids: &[String]) -> Result<()> {
        let (ws_stream, _) = connect_async(WS_URL).await?;
        let (mut write, mut read) = ws_stream.split();

        // Subscribe to market channel with token_ids
        let subscribe_msg = serde_json::json!({
            "type": "MARKET",
            "assets_ids": token_ids,
            "custom_feature_enabled": true
        });
        write.send(Message::Text(subscribe_msg.to_string())).await?;

        while let Some(msg) = read.next().await {
            match msg? {
                Message::Text(text) => {
                    self.handle_message(&text).await?;
                }
                Message::Ping(data) => {
                    write.send(Message::Pong(data)).await?;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok(())
    }

    async fn handle_message(&self, text: &str) -> Result<()> {
        // Try parsing as WsMarketEvent
        match serde_json::from_str::<WsMarketEvent>(text) {
            Ok(WsMarketEvent::Book(book)) => {
                self.record_book_snapshot(&book).await?;
            }
            Ok(WsMarketEvent::MarketResolved { market, winning_outcome, .. }) => {
                tracing::info!(market = %market, outcome = %winning_outcome, "Market resolved via WebSocket");
                // Trigger settlement
            }
            Ok(_) => {
                // price_change, last_trade_price — log for now, use later
            }
            Err(e) => {
                tracing::debug!("Unrecognized WS message: {}", &text[..text.len().min(200)]);
            }
        }
        Ok(())
    }

    async fn record_book_snapshot(&self, book: &WsBookEvent) -> Result<()> {
        let best_bid = book.best_bid();
        let best_ask = book.best_ask();
        let mid = book.mid_price();
        let spread = book.spread_cents();
        let bid_depth = book.bid_depth_usd();
        let ask_depth = book.ask_depth_usd();
        let levels_json = serde_json::to_string(&serde_json::json!({
            "bids": book.bids,
            "asks": book.asks,
        }))?;

        self.db.conn_async(move |conn| {
            conn.execute(
                "INSERT INTO book_snapshots (condition_id, token_id, best_bid, best_ask, bid_depth_usd, ask_depth_usd, spread_cents, mid_price, levels_json, snapshot_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'))",
                rusqlite::params![
                    book.market, book.asset_id,
                    best_bid, best_ask, bid_depth, ask_depth, spread, mid,
                    levels_json
                ],
            )?;
            Ok(())
        }).await?;

        metrics::counter!("book_snapshots_recorded").increment(1);
        Ok(())
    }

    /// Subscribe to additional token_ids on an existing connection
    pub async fn subscribe_tokens(&self, write: &mut impl SinkExt<Message>, token_ids: &[String]) -> Result<()> {
        let msg = serde_json::json!({
            "assets_ids": token_ids,
            "operation": "subscribe",
            "custom_feature_enabled": true
        });
        write.send(Message::Text(msg.to_string())).await.map_err(|_| anyhow::anyhow!("send failed"))?;
        self.subscribed_tokens.write().await.extend(token_ids.iter().cloned());
        Ok(())
    }
}
```

Add config:

```toml
[clob]
ws_url = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
ws_reconnect_delay_secs = 5
ws_ping_interval_secs = 30
```

Wire into `main.rs`:

```rust
// After market scoring, gather token_ids for top-20 markets
// Spawn: tokio::spawn(book_stream_manager.run(token_ids))
```

**Step 4: Run tests**

Run: `cargo test --all`
Expected: ALL PASS (unit tests pass; integration test connects to real WS)

**Step 5: Commit**

```bash
git add crates/evaluator/src/book_stream.rs
git commit -am "feat: WebSocket book streaming — real-time orderbook data from CLOB"
```

---

## Task 24: Depth-Aware Paper Trading (Book-Walking Slippage)

**Context:** Replace the flat `slippage_default_cents = 1.0` with realistic book-walking slippage when orderbook data is available. Walk through ask levels (for buys) or bid levels (for sells) to compute what our actual fill price would be for a given trade size.

**This feeds:**
- **Execution Master detector (Task 8):** `mid_at_entry` from book for PnL decomposition
- **Follower slippage (Task 17):** realistic entry price instead of flat estimate
- **Copy fidelity (Task 15):** `SKIPPED_NO_FILL` when depth < trade size

**Files:**
- Modify: `crates/evaluator/src/paper_trading.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_walk_book_buy() {
    // Orderbook asks: [0.52 x 25], [0.53 x 60], [0.54 x 10]
    // Buy $25 worth: fills entirely at 0.52 (25 / 0.52 ≈ 48 shares, depth has 25 shares at 0.52)
    // Actually: we want to buy $25 of shares, so we need 25 / price shares
    // At 0.52: can buy 25 shares * $0.52 = $13 worth. Need $25 - $13 = $12 more
    // At 0.53: can buy remaining 12 / 0.53 ≈ 22.6 shares
    // VWAP = (25 * 0.52 + 22.6 * 0.53) / (25 + 22.6) ≈ 0.5247
    let asks = vec![
        OrderLevel { price: "0.52".into(), size: "25".into() },
        OrderLevel { price: "0.53".into(), size: "60".into() },
        OrderLevel { price: "0.54".into(), size: "10".into() },
    ];
    let result = walk_book_for_fill(&asks, 25.0, Side::Buy);
    assert!(result.is_some());
    let (vwap, filled_usd) = result.unwrap();
    assert!(vwap > 0.52 && vwap < 0.53);
    assert!((filled_usd - 25.0).abs() < 0.1);
}

#[test]
fn test_walk_book_sell() {
    // Orderbook bids: [0.50 x 15], [0.49 x 20], [0.48 x 30]
    // Sell $25 worth: fills partially at 0.50 ($7.5), then 0.49 ($9.8), then 0.48 ($7.7)
    let bids = vec![
        OrderLevel { price: "0.50".into(), size: "15".into() },
        OrderLevel { price: "0.49".into(), size: "20".into() },
        OrderLevel { price: "0.48".into(), size: "30".into() },
    ];
    let result = walk_book_for_fill(&bids, 25.0, Side::Sell);
    assert!(result.is_some());
    let (vwap, _) = result.unwrap();
    assert!(vwap < 0.50 && vwap > 0.48);
}

#[test]
fn test_walk_book_insufficient_depth() {
    // Only $10 of depth, trying to fill $25
    let asks = vec![
        OrderLevel { price: "0.52".into(), size: "10".into() },  // 10 * 0.52 = $5.20
        OrderLevel { price: "0.53".into(), size: "10".into() },  // 10 * 0.53 = $5.30
    ];
    let result = walk_book_for_fill(&asks, 25.0, Side::Buy);
    assert!(result.is_none()); // not enough depth
}

#[test]
fn test_compute_slippage_with_book() {
    // Their price: 0.52, our book-walked VWAP: 0.525
    // Slippage = (0.525 - 0.52) * 100 = 0.5 cents
    let slippage = compute_slippage_from_book(0.52, 0.525, Side::Buy);
    assert!((slippage - 0.5).abs() < 0.01);
}

#[test]
fn test_paper_trade_uses_book_slippage_when_available() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();
    // Insert a book snapshot with levels
    db.conn.execute(
        "INSERT INTO book_snapshots (condition_id, token_id, best_bid, best_ask, bid_depth_usd, ask_depth_usd, spread_cents, mid_price, levels_json, snapshot_at)
         VALUES ('0xm1', 'token1', 0.48, 0.52, 65.0, 95.0, 4.0, 0.50,
                 '{\"asks\":[{\"price\":\"0.52\",\"size\":\"25\"},{\"price\":\"0.53\",\"size\":\"60\"}],\"bids\":[{\"price\":\"0.48\",\"size\":\"30\"},{\"price\":\"0.49\",\"size\":\"20\"}]}',
                 datetime('now'))",
        [],
    ).unwrap();
    // ... create paper trade, verify entry price uses book-walked VWAP, not flat slippage
}

#[test]
fn test_paper_trade_falls_back_to_flat_slippage() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();
    // No book_snapshots for this market
    // ... create paper trade, verify it uses flat slippage_default_cents
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_walk_book`
Expected: FAIL — `walk_book_for_fill` doesn't exist

**Step 3: Implement**

```rust
/// Walk through orderbook levels to compute VWAP fill price for a given USD trade size.
/// For buys: walk ask levels (lowest first).
/// For sells: walk bid levels (highest first).
/// Returns None if insufficient depth to fill the order.
/// Returns Some((vwap, filled_usd)) on success.
pub fn walk_book_for_fill(
    levels: &[OrderLevel],
    target_usd: f64,
    side: Side,
) -> Option<(f64, f64)> {
    let mut remaining_usd = target_usd;
    let mut total_shares = 0.0;
    let mut total_cost = 0.0;

    // For buys, levels should be asks sorted ascending by price
    // For sells, levels should be bids sorted descending by price
    // (caller is responsible for correct ordering)
    for level in levels {
        let price = level.price.parse::<f64>().ok()?;
        let available_shares = level.size.parse::<f64>().ok()?;
        if price <= 0.0 || available_shares <= 0.0 {
            continue;
        }

        let available_usd = available_shares * price;
        let fill_usd = remaining_usd.min(available_usd);
        let fill_shares = fill_usd / price;

        total_shares += fill_shares;
        total_cost += fill_usd;
        remaining_usd -= fill_usd;

        if remaining_usd <= 0.01 {  // filled
            let vwap = total_cost / total_shares;
            return Some((vwap, total_cost));
        }
    }

    None // insufficient depth
}

/// Compute slippage in cents between their price and our book-walked VWAP.
pub fn compute_slippage_from_book(their_price: f64, our_vwap: f64, side: Side) -> f64 {
    match side {
        Side::Buy => (our_vwap - their_price) * 100.0,  // we pay more
        Side::Sell => (their_price - our_vwap) * 100.0,  // we receive less
    }
}

/// Get the latest book levels for a market from the database.
/// Returns None if no book snapshot exists.
pub fn get_latest_book_levels(conn: &Connection, condition_id: &str) -> Option<(Vec<OrderLevel>, Vec<OrderLevel>)> {
    let levels_json: Option<String> = conn.query_row(
        "SELECT levels_json FROM book_snapshots
         WHERE condition_id = ?1
         ORDER BY snapshot_at DESC LIMIT 1",
        rusqlite::params![condition_id],
        |row| row.get(0),
    ).ok()?;

    let json: serde_json::Value = serde_json::from_str(&levels_json?).ok()?;
    let bids: Vec<OrderLevel> = serde_json::from_value(json["bids"].clone()).ok()?;
    let asks: Vec<OrderLevel> = serde_json::from_value(json["asks"].clone()).ok()?;
    Some((bids, asks))
}
```

Update `mirror_trade_to_paper` to use book-walking:

```rust
// In mirror_trade_to_paper, replace flat slippage with book-aware logic:
let (entry_price_with_slippage, slippage_source) = match get_latest_book_levels(&conn, condition_id) {
    Some((bids, asks)) => {
        let levels = match side {
            Side::Buy => &asks,
            Side::Sell => &bids,
        };
        match walk_book_for_fill(levels, per_trade_size_usd, side) {
            Some((vwap, _)) => (vwap, "book"),
            None => {
                // Insufficient depth — SKIP trade
                record_fidelity_event(conn, proxy_wallet, condition_id, None, "SKIPPED_NO_FILL", "depth < trade size");
                return Ok(MirrorDecision { inserted: false, reason: Some("SKIPPED_NO_FILL") });
            }
        }
    }
    None => {
        // No book data — fall back to flat slippage
        let flat_slippage = slippage_default_cents / 100.0;
        let price = match side {
            Side::Buy => observed_price + flat_slippage,
            Side::Sell => observed_price - flat_slippage,
        };
        (price, "flat")
    }
};

// Also extract mid_price for PnL decomposition (feeds Execution Master detector)
let mid_at_entry: Option<f64> = get_latest_book_levels(&conn, condition_id)
    .and_then(|_| {
        conn.query_row(
            "SELECT mid_price FROM book_snapshots WHERE condition_id = ?1 ORDER BY snapshot_at DESC LIMIT 1",
            rusqlite::params![condition_id],
            |row| row.get(0),
        ).ok()
    });

// Store mid_at_entry on paper_trade for later PnL decomposition
```

**Step 4: Run tests**

Run: `cargo test --all`
Expected: ALL PASS

**Step 5: Commit**

```bash
git commit -am "feat: depth-aware paper trading — book-walking slippage with flat fallback"
```

---

## Verification Checklist

After all 24 tasks are complete, verify:

```bash
# All tests pass
cargo test --all

# No warnings
cargo clippy --all-targets -- -D warnings

# Formatting
cargo fmt --check

# Key invariants:
# 1. Every config value in default.toml deserializes without error
# 2. wallet_personas table is populated after classification job runs
# 3. wallet_exclusions table is populated with reasons
# 4. Paper trades can be settled (status changes from 'open' to 'settled_win'/'settled_loss')
# 5. copy_fidelity_events has rows after paper_tick runs
# 6. follower_slippage has rows after paper_tick creates trades
# 7. WScore uses all 5 components
# 8. MScore uses real trades_24h and whale_concentration when data is available
# 9. Anomaly detection fires on behavior changes
# 10. book_snapshots table exists and can store orderbook data
# 11. BookResponse deserializes from real CLOB API responses
# 12. WebSocket book messages parse correctly (both ".48" and "0.48" price formats)
# 13. walk_book_for_fill returns None when depth < trade size
# 14. Paper trading uses book-walked VWAP when available, flat slippage as fallback
# 15. Taker fee is zero for non-crypto markets, quartic for 15m crypto markets
# 16. markets table has yes_token_id and no_token_id columns populated
```
