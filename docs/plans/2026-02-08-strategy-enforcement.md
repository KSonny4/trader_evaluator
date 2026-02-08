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
- [ ] Task 14: Quartic Taker Fee
- [ ] Task 15: Copy Fidelity Tracking
- [ ] Task 16: Two-Level Risk Management (Per-Wallet + Portfolio)
- [ ] Task 17: Follower Slippage Tracking
- [ ] Task 18: WScore — Missing 3 Sub-Components
- [ ] Task 19: Weekly Re-evaluation + Anomaly Detection
- [ ] Task 20: MScore — Real Inputs (density, whale concentration)
- [ ] Task 21: Wire New Jobs into Scheduler

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

## Task 14: Quartic Taker Fee

The Strategy Bible specifies: `fee = price * 0.25 * (price * (1 - price))^2`. Currently not applied.

**Files:**
- Modify: `crates/evaluator/src/paper_trading.rs`

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
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_quartic`
Expected: FAIL

**Step 3: Implement**

```rust
/// Quartic taker fee on Polymarket.
/// fee = price * 0.25 * (price * (1 - price))^2
/// Max ~1.44% at p≈0.60, approaches zero near p=0 or p=1.
pub fn quartic_taker_fee(price: f64) -> f64 {
    let p = price.clamp(0.0, 1.0);
    p * 0.25 * (p * (1.0 - p)).powi(2)
}
```

Then update `mirror_trade_to_paper` to apply the fee to the entry price:

```rust
// After slippage calculation, before inserting:
let fee = quartic_taker_fee(adjusted_price);
let entry_price_with_fee = if side == Side::Buy {
    adjusted_price + fee  // buying costs more
} else {
    adjusted_price - fee  // selling gets less
};
```

**Step 4: Run tests**

Run: `cargo test -p evaluator`
Expected: ALL PASS (update existing tests if entry prices shifted slightly due to fee)

**Step 5: Commit**

```bash
git commit -am "feat: quartic taker fee applied to paper trades"
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

## Verification Checklist

After all 21 tasks are complete, verify:

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
```
