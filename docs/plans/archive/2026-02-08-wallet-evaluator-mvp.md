# Wallet Evaluator MVP Implementation Plan

> **🗄️ ARCHIVED — COMPLETED**
> 
> All 15 tasks implemented. This plan established the foundation: project skeleton, config system, database schema, API clients, market scoring, wallet discovery, ingestion, paper trading, wallet scoring, scheduler, metrics, CLI, and integration tests.
> 
> **Current work:** See `../MASTER_STRATEGY_IMPLEMENTATION_PLAN.md` for active development (persona classification, paper settlement, risk management).

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Polymarket wallet discovery and paper copy-trading evaluation system that selects "follow-worthy" markets, discovers wallets trading them, tracks those wallets long-term, and runs risk-managed paper-copy portfolios to rank "who to follow" with evidence.

**Architecture:** Rust workspace with shared crates. SQLite for storage, TOML for config, Prometheus for metrics, deployed to AWS t3.micro. The system runs as a single long-lived async process with Tokio-scheduled periodic jobs (market scoring, wallet discovery, trade ingestion, position snapshots, paper trading, wallet ranking).

**Deploy-early philosophy:** We deploy to AWS as soon as the binary compiles and can create the DB. Every subsequent task adds a feature, deploys, and verifies the SQLite DB has the expected data. `make deploy` is our workflow — it builds, ships, restarts, and runs `make check-phase-N` to confirm the DB is filling up correctly. Data collection starts immediately; we build the analysis on top of a growing dataset.

**Raw data rule (for future re-evaluation):** Every external API call must save the **raw, unmodified HTTP response body** (bytes) into an append-only SQLite table (`raw_api_responses`) *before* any parsing/feature logic runs. Parsed tables (`trades_raw`, `activity_raw`, etc.) are derived artifacts that we can recompute later if our parsing/scoring logic changes.

**Tech Stack:** Rust, Tokio, SQLite (rusqlite), reqwest, serde/serde_json, rust_decimal, tracing, metrics/metrics-exporter-prometheus, TOML config (toml + serde).

**Reference project:** Proven Polymarket API patterns and deployment architecture.

**Governing document:** `docs/EVALUATION_STRATEGY.md` — all evaluation metrics, phase gates, and decision rules live there.

---

## Progress (as of 2026-02-08)

- [x] Task 1: Project Skeleton & Workspace Setup
- [x] Task 2: Config System
- [x] Task 3: Database Schema & Migrations
- [x] Task 4: Core Types
- [x] Task 5: Makefile + Deployment Setup (DEPLOY EARLY)
- [x] Task 6: Polymarket API Client
- [x] Task 7: Market Scoring (MScore) (pure logic + tests)
- [x] Task 8: Wallet Discovery (pure logic + tests)
- [x] Task 9: Ingestion Jobs (pure logic + tests; raw responses stored before parse inside ingestion functions)
- [x] Task 10: Paper Trading Engine (pure logic + tests)
- [x] Task 11: Wallet Scoring (WScore) (pure logic + tests)
- [x] Task 12: Main Event Loop & Job Scheduler (ticks execute job logic; runs locally via LocalSet; jobs run immediately on startup)
- [x] Task 13: Observability (Prometheus Metrics) (jobs + API calls increment counters/histograms/gauges)
- [x] Task 14: CLI Commands for Manual Inspection (added query helpers + seeded-DB tests)
- [x] Task 15: Integration Test with Real APIs (ignored-by-default tests added; can be run via `cargo test -p common --test integration_real_apis -- --ignored`)

## Task 1: Project Skeleton & Workspace Setup

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `CLAUDE.md`
- Create: `config/default.toml`
- Create: `.gitignore`
- Create: `.env.example`
- Create: `crates/common/Cargo.toml`
- Create: `crates/common/src/lib.rs`
- Create: `crates/evaluator/Cargo.toml`
- Create: `crates/evaluator/src/main.rs`

**Step 1: Initialize git repo**

```bash
cd /Users/petr.kubelka/git_projects/trader_evaluator
git init
```

**Step 2: Create workspace Cargo.toml**

```toml
[workspace]
resolver = "2"
members = [
    "crates/common",
    "crates/evaluator",
]

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json"] }
rusqlite = { version = "0.31", features = ["bundled"] }
rust_decimal = { version = "1", features = ["serde-with-str"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
toml = "0.8"
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
thiserror = "2"
metrics = "0.24"
metrics-exporter-prometheus = "0.16"
```

**Step 3: Create common crate**

`crates/common/Cargo.toml`:
```toml
[package]
name = "common"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
rusqlite = { workspace = true }
rust_decimal = { workspace = true }
tracing = { workspace = true }
toml = { workspace = true }
chrono = { workspace = true }
anyhow = { workspace = true }
thiserror = { workspace = true }
```

`crates/common/src/lib.rs`:
```rust
pub mod config;
pub mod db;
pub mod types;
```

**Step 4: Create evaluator crate**

`crates/evaluator/Cargo.toml`:
```toml
[package]
name = "evaluator"
version = "0.1.0"
edition = "2021"

[dependencies]
common = { path = "../common" }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow = { workspace = true }
```

`crates/evaluator/src/main.rs`:
```rust
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .json()
        .init();

    tracing::info!("trader_evaluator starting");

    let config = common::config::Config::load()?;
    let db = common::db::Database::open(&config.database.path)?;
    db.run_migrations()?;

    tracing::info!("initialized — entering main loop");

    // TODO: start periodic jobs
    tokio::signal::ctrl_c().await?;

    tracing::info!("shutting down");
    Ok(())
}
```

**Step 5: Create .gitignore**

```
/target
*.db
*.db-shm
*.db-wal
.env
.env.*
!.env.example
data/
```

**Step 6: Create .env.example**

```
# No secrets needed for V1 — all Polymarket APIs are public
# Future: add Polymarket CLOB keys if we do real execution
# POLYMARKET_PRIVATE_KEY=
# POLYMARKET_API_KEY=

# Observability (Grafana Cloud — same as trading project)
# GRAFANA_CLOUD_PROM_URL=
# GRAFANA_CLOUD_PROM_USER=
# GRAFANA_CLOUD_API_KEY=
# GRAFANA_CLOUD_LOKI_URL=
# GRAFANA_CLOUD_LOKI_USER=
```

**Step 7: Create CLAUDE.md**

Minimal project guide referencing `docs/EVALUATION_STRATEGY.md` and `docs/prd.txt` for full context. Include build commands, project structure, and link to reference `trading` project.

**Step 8: Build and verify**

Run: `cargo build`
Expected: Compiles successfully.

**Step 9: Commit**

```bash
git add -A
git commit -m "feat: project skeleton — workspace, common crate, evaluator binary"
```

---

## Task 2: Config System

**Files:**
- Create: `crates/common/src/config.rs`
- Create: `config/default.toml`
- Create: `tests/config_test.rs` (or inline #[cfg(test)])

**Step 1: Write the failing test**

In `crates/common/src/config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_default_config() {
        let config = Config::from_str(include_str!("../../config/default.toml")).unwrap();
        assert_eq!(config.general.mode, "paper");
        assert!(config.risk.max_exposure_per_market_pct > 0.0);
        assert!(config.ingestion.trades_poll_interval_secs > 0);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p common`
Expected: FAIL — `Config` type doesn't exist yet.

**Step 3: Write config types and default.toml**

`config/default.toml`:
```toml
[general]
mode = "paper"
log_level = "info"

[database]
path = "data/evaluator.db"

[risk]
max_exposure_per_market_pct = 10.0     # max % of bankroll per market
max_exposure_per_wallet_pct = 5.0      # max % of bankroll per wallet-copy
max_daily_trades = 100                 # turnover cap
slippage_pct = 1.0                     # conservative slippage assumption
no_chase_adverse_move_pct = 5.0        # don't increase after 5% adverse
portfolio_stop_drawdown_pct = 15.0     # pause if drawdown > 15%
paper_bankroll_usdc = 10000.0

[market_scoring]
top_n_markets = 20
min_liquidity_usdc = 1000.0
min_daily_volume_usdc = 5000.0
min_daily_trades = 20
min_unique_traders = 10
max_days_to_expiry = 90
min_days_to_expiry = 1
refresh_interval_secs = 86400          # daily
weights_liquidity = 0.25
weights_volume = 0.25
weights_density = 0.20
weights_whale_concentration = 0.15
weights_time_to_expiry = 0.15

[wallet_discovery]
min_total_trades = 5                   # prune wallets with < N trades
max_wallets_per_market = 100
holders_per_market = 20                # Polymarket API caps at 20
refresh_interval_secs = 86400          # daily

[ingestion]
trades_poll_interval_secs = 3600       # hourly
activity_poll_interval_secs = 21600    # every 6 hours
positions_poll_interval_secs = 86400   # daily
holders_poll_interval_secs = 86400     # daily
rate_limit_delay_ms = 200              # between API calls
max_retries = 3
backoff_base_ms = 1000

[paper_trading]
strategies = ["mirror"]                # later: "delay", "consensus"
mirror_delay_secs = 0                  # 0 = immediate mirror
position_size_usdc = 100.0             # per-trade size

[wallet_scoring]
windows_days = [7, 30, 90]
min_trades_for_score = 10
edge_weight = 0.30
consistency_weight = 0.25
market_skill_weight = 0.20
timing_skill_weight = 0.15
behavior_quality_weight = 0.10

[observability]
prometheus_port = 9094                 # different from trading bots (9091-9093)

[polymarket]
data_api_url = "https://data-api.polymarket.com"
gamma_api_url = "https://gamma-api.polymarket.com"
```

`crates/common/src/config.rs`:
```rust
use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub general: General,
    pub database: Database,
    pub risk: Risk,
    pub market_scoring: MarketScoring,
    pub wallet_discovery: WalletDiscovery,
    pub ingestion: Ingestion,
    pub paper_trading: PaperTrading,
    pub wallet_scoring: WalletScoring,
    pub observability: Observability,
    pub polymarket: Polymarket,
}

#[derive(Debug, Deserialize)]
pub struct General {
    pub mode: String,
    pub log_level: String,
}

#[derive(Debug, Deserialize)]
pub struct Database {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct Risk {
    pub max_exposure_per_market_pct: f64,
    pub max_exposure_per_wallet_pct: f64,
    pub max_daily_trades: u32,
    pub slippage_pct: f64,
    pub no_chase_adverse_move_pct: f64,
    pub portfolio_stop_drawdown_pct: f64,
    pub paper_bankroll_usdc: f64,
}

#[derive(Debug, Deserialize)]
pub struct MarketScoring {
    pub top_n_markets: usize,
    pub min_liquidity_usdc: f64,
    pub min_daily_volume_usdc: f64,
    pub min_daily_trades: u32,
    pub min_unique_traders: u32,
    pub max_days_to_expiry: u32,
    pub min_days_to_expiry: u32,
    pub refresh_interval_secs: u64,
    pub weights_liquidity: f64,
    pub weights_volume: f64,
    pub weights_density: f64,
    pub weights_whale_concentration: f64,
    pub weights_time_to_expiry: f64,
}

#[derive(Debug, Deserialize)]
pub struct WalletDiscovery {
    pub min_total_trades: u32,
    pub max_wallets_per_market: usize,
    pub holders_per_market: usize,
    pub refresh_interval_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct Ingestion {
    pub trades_poll_interval_secs: u64,
    pub activity_poll_interval_secs: u64,
    pub positions_poll_interval_secs: u64,
    pub holders_poll_interval_secs: u64,
    pub rate_limit_delay_ms: u64,
    pub max_retries: u32,
    pub backoff_base_ms: u64,
}

#[derive(Debug, Deserialize)]
pub struct PaperTrading {
    pub strategies: Vec<String>,
    pub mirror_delay_secs: u64,
    pub position_size_usdc: f64,
}

#[derive(Debug, Deserialize)]
pub struct WalletScoring {
    pub windows_days: Vec<u32>,
    pub min_trades_for_score: u32,
    pub edge_weight: f64,
    pub consistency_weight: f64,
    pub market_skill_weight: f64,
    pub timing_skill_weight: f64,
    pub behavior_quality_weight: f64,
}

#[derive(Debug, Deserialize)]
pub struct Observability {
    pub prometheus_port: u16,
}

#[derive(Debug, Deserialize)]
pub struct Polymarket {
    pub data_api_url: String,
    pub gamma_api_url: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        let content = std::fs::read_to_string("config/default.toml")?;
        Self::from_str(&content)
    }

    pub fn from_str(s: &str) -> Result<Self> {
        Ok(toml::from_str(s)?)
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p common`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/common/src/config.rs config/default.toml
git commit -m "feat: config system — TOML config with all evaluator settings"
```

---

## Task 3: Database Schema & Migrations

**Files:**
- Create: `crates/common/src/db.rs`
- Modify: `crates/common/src/lib.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_create_all_tables() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let tables: Vec<String> = db.conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"markets".to_string()));
        assert!(tables.contains(&"wallets".to_string()));
        assert!(tables.contains(&"raw_api_responses".to_string()));
        assert!(tables.contains(&"trades_raw".to_string()));
        assert!(tables.contains(&"activity_raw".to_string()));
        assert!(tables.contains(&"positions_snapshots".to_string()));
        assert!(tables.contains(&"holders_snapshots".to_string()));
        assert!(tables.contains(&"market_scores".to_string()));
        assert!(tables.contains(&"wallet_features_daily".to_string()));
        assert!(tables.contains(&"paper_trades".to_string()));
        assert!(tables.contains(&"paper_positions".to_string()));
        assert!(tables.contains(&"wallet_scores_daily".to_string()));
    }

    #[test]
    fn test_migrations_idempotent() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        db.run_migrations().unwrap(); // second call must not fail
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p common`
Expected: FAIL — `Database` doesn't exist.

**Step 3: Implement Database with schema**

`crates/common/src/db.rs`:
```rust
use anyhow::Result;
use rusqlite::Connection;

pub struct Database {
    pub conn: Connection,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Ok(Self { conn })
    }

    pub fn run_migrations(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA)?;
        Ok(())
    }
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS raw_api_responses (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    api TEXT NOT NULL,                 -- data_api, gamma_api
    method TEXT NOT NULL,              -- GET, POST
    url TEXT NOT NULL,                 -- full URL as requested
    query_params_json TEXT,            -- JSON object of query params (if any)
    request_body_json TEXT,            -- JSON string (if any)
    status INTEGER,                    -- HTTP status
    response_headers_json TEXT,        -- JSON object of headers (best-effort)
    response_body BLOB NOT NULL,       -- raw, unmodified response bytes
    fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
    git_sha TEXT                       -- for traceability (best-effort)
);

CREATE TABLE IF NOT EXISTS markets (
    condition_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    slug TEXT,
    description TEXT,
    end_date TEXT,
    liquidity REAL,
    volume REAL,
    category TEXT,
    event_slug TEXT,
    outcomes_json TEXT,              -- raw JSON of outcome tokens
    first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS wallets (
    proxy_wallet TEXT PRIMARY KEY,
    pseudonym TEXT,
    name TEXT,
    bio TEXT,
    discovered_from TEXT NOT NULL,    -- HOLDER, TRADER_RECENT, LEADERBOARD
    discovered_at TEXT NOT NULL DEFAULT (datetime('now')),
    discovered_market TEXT,           -- condition_id where discovered
    total_markets_traded INTEGER,
    is_active INTEGER NOT NULL DEFAULT 1,
    on_global_watchlist INTEGER NOT NULL DEFAULT 0,
    last_updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS trades_raw (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    condition_id TEXT NOT NULL,
    asset TEXT,
    side TEXT,                        -- BUY or SELL
    size REAL NOT NULL,
    price REAL NOT NULL,
    outcome TEXT,
    outcome_index INTEGER,
    timestamp INTEGER NOT NULL,       -- unix epoch
    transaction_hash TEXT,
    raw_json TEXT,                    -- original API response
    ingested_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(transaction_hash, proxy_wallet, condition_id)
);

CREATE TABLE IF NOT EXISTS activity_raw (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    condition_id TEXT,
    activity_type TEXT NOT NULL,      -- TRADE, SPLIT, MERGE, REDEEM, etc.
    size REAL,
    usdc_size REAL,
    price REAL,
    side TEXT,
    outcome TEXT,
    outcome_index INTEGER,
    timestamp INTEGER NOT NULL,
    transaction_hash TEXT,
    raw_json TEXT,
    ingested_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(transaction_hash, proxy_wallet, activity_type)
);

CREATE TABLE IF NOT EXISTS positions_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    condition_id TEXT NOT NULL,
    asset TEXT,
    size REAL NOT NULL,
    avg_price REAL,
    current_value REAL,
    cash_pnl REAL,
    percent_pnl REAL,
    outcome TEXT,
    outcome_index INTEGER,
    snapshot_at TEXT NOT NULL DEFAULT (datetime('now')),
    raw_json TEXT
);

CREATE TABLE IF NOT EXISTS holders_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    condition_id TEXT NOT NULL,
    token TEXT,
    proxy_wallet TEXT NOT NULL,
    amount REAL NOT NULL,
    outcome_index INTEGER,
    pseudonym TEXT,
    snapshot_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS market_scores (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    condition_id TEXT NOT NULL,
    score_date TEXT NOT NULL,
    mscore REAL NOT NULL,
    liquidity_score REAL,
    volume_score REAL,
    density_score REAL,
    whale_concentration_score REAL,
    time_to_expiry_score REAL,
    rank INTEGER,
    notes TEXT,
    UNIQUE(condition_id, score_date)
);

CREATE TABLE IF NOT EXISTS wallet_features_daily (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    feature_date TEXT NOT NULL,
    window_days INTEGER NOT NULL,     -- 7, 30, or 180
    trade_count INTEGER,
    win_count INTEGER,
    loss_count INTEGER,
    total_pnl REAL,
    avg_position_size REAL,
    unique_markets INTEGER,
    avg_hold_time_hours REAL,
    max_drawdown_pct REAL,
    UNIQUE(proxy_wallet, feature_date, window_days)
);

CREATE TABLE IF NOT EXISTS paper_trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,        -- wallet being copied
    strategy TEXT NOT NULL,            -- mirror, delay, consensus
    condition_id TEXT NOT NULL,
    side TEXT NOT NULL,                -- BUY or SELL
    outcome TEXT,
    outcome_index INTEGER,
    size_usdc REAL NOT NULL,
    entry_price REAL NOT NULL,
    slippage_applied REAL,
    triggered_by_trade_id INTEGER,    -- FK to trades_raw.id
    status TEXT NOT NULL DEFAULT 'open', -- open, settled_win, settled_loss
    exit_price REAL,
    pnl REAL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    settled_at TEXT
);

CREATE TABLE IF NOT EXISTS paper_positions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    strategy TEXT NOT NULL,
    condition_id TEXT NOT NULL,
    side TEXT NOT NULL,
    total_size_usdc REAL NOT NULL,
    avg_entry_price REAL NOT NULL,
    current_value REAL,
    unrealized_pnl REAL,
    last_updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(proxy_wallet, strategy, condition_id, side)
);

CREATE TABLE IF NOT EXISTS wallet_scores_daily (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    score_date TEXT NOT NULL,
    window_days INTEGER NOT NULL,     -- 7, 30, 90
    wscore REAL NOT NULL,
    edge_score REAL,
    consistency_score REAL,
    market_skill_score REAL,
    timing_skill_score REAL,
    behavior_quality_score REAL,
    paper_roi_pct REAL,
    paper_hit_rate REAL,
    paper_max_drawdown_pct REAL,
    recommended_follow_mode TEXT,     -- mirror, delay, consensus
    risk_flags TEXT,                  -- JSON array of flags
    UNIQUE(proxy_wallet, score_date, window_days)
);

CREATE INDEX IF NOT EXISTS idx_trades_raw_wallet ON trades_raw(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_trades_raw_market ON trades_raw(condition_id);
CREATE INDEX IF NOT EXISTS idx_trades_raw_timestamp ON trades_raw(timestamp);
CREATE INDEX IF NOT EXISTS idx_activity_raw_wallet ON activity_raw(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_positions_wallet ON positions_snapshots(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_holders_market ON holders_snapshots(condition_id);
CREATE INDEX IF NOT EXISTS idx_raw_api_responses_fetched_at ON raw_api_responses(fetched_at);
CREATE INDEX IF NOT EXISTS idx_paper_trades_wallet ON paper_trades(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_paper_trades_status ON paper_trades(status);
CREATE INDEX IF NOT EXISTS idx_wallet_scores_date ON wallet_scores_daily(score_date);
"#;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p common`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/common/src/db.rs
git commit -m "feat: database schema — 12 core tables (incl raw_api_responses) with indexes and migrations"
```

---

## Task 4: Core Types

**Files:**
- Create: `crates/common/src/types.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_source_display() {
        assert_eq!(DiscoverySource::Holder.as_str(), "HOLDER");
        assert_eq!(DiscoverySource::TraderRecent.as_str(), "TRADER_RECENT");
        assert_eq!(DiscoverySource::Leaderboard.as_str(), "LEADERBOARD");
    }

    #[test]
    fn test_paper_trade_status() {
        assert_eq!(PaperTradeStatus::Open.as_str(), "open");
        assert_eq!(PaperTradeStatus::SettledWin.as_str(), "settled_win");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p common`
Expected: FAIL

**Step 3: Implement types**

```rust
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverySource {
    Holder,
    TraderRecent,
    Leaderboard,
}

impl DiscoverySource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Holder => "HOLDER",
            Self::TraderRecent => "TRADER_RECENT",
            Self::Leaderboard => "LEADERBOARD",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperTradeStatus {
    Open,
    SettledWin,
    SettledLoss,
}

impl PaperTradeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::SettledWin => "settled_win",
            Self::SettledLoss => "settled_loss",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyStrategy {
    Mirror,
    Delay,
    Consensus,
}

impl CopyStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mirror => "mirror",
            Self::Delay => "delay",
            Self::Consensus => "consensus",
        }
    }
}

/// Market from Gamma API
#[derive(Debug, Clone, Deserialize)]
pub struct GammaMarket {
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub title: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,
    pub liquidity: Option<String>,
    pub volume: Option<String>,
    pub category: Option<String>,
    #[serde(rename = "eventSlug")]
    pub event_slug: Option<String>,
}

/// Trade from Data API /trades
#[derive(Debug, Clone, Deserialize)]
pub struct ApiTrade {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub asset: Option<String>,
    pub size: Option<String>,
    pub price: Option<String>,
    pub timestamp: Option<i64>,
    pub title: Option<String>,
    pub slug: Option<String>,
    pub outcome: Option<String>,
    #[serde(rename = "outcomeIndex")]
    pub outcome_index: Option<i32>,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
    pub side: Option<String>,
    pub pseudonym: Option<String>,
    pub name: Option<String>,
}

/// Holder from Data API /holders
#[derive(Debug, Clone, Deserialize)]
pub struct ApiHolder {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    pub amount: Option<f64>,
    pub asset: Option<String>,
    pub pseudonym: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "outcomeIndex")]
    pub outcome_index: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiHolderResponse {
    pub token: Option<String>,
    pub holders: Vec<ApiHolder>,
}

/// Activity from Data API /activity
#[derive(Debug, Clone, Deserialize)]
pub struct ApiActivity {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    #[serde(rename = "type")]
    pub activity_type: Option<String>,
    pub size: Option<String>,
    #[serde(rename = "usdcSize")]
    pub usdc_size: Option<String>,
    pub price: Option<String>,
    pub side: Option<String>,
    pub outcome: Option<String>,
    #[serde(rename = "outcomeIndex")]
    pub outcome_index: Option<i32>,
    pub timestamp: Option<i64>,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
}

/// Position from Data API /positions
#[derive(Debug, Clone, Deserialize)]
pub struct ApiPosition {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub asset: Option<String>,
    pub size: Option<String>,
    #[serde(rename = "avgPrice")]
    pub avg_price: Option<String>,
    #[serde(rename = "currentValue")]
    pub current_value: Option<String>,
    #[serde(rename = "cashPnl")]
    pub cash_pnl: Option<String>,
    #[serde(rename = "percentPnl")]
    pub percent_pnl: Option<String>,
    pub outcome: Option<String>,
    #[serde(rename = "outcomeIndex")]
    pub outcome_index: Option<i32>,
}

/// Leaderboard entry from Data API /v1/leaderboard
#[derive(Debug, Clone, Deserialize)]
pub struct ApiLeaderboardEntry {
    pub rank: Option<String>,
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "userName")]
    pub user_name: Option<String>,
    pub vol: Option<f64>,
    pub pnl: Option<f64>,
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p common`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/common/src/types.rs
git commit -m "feat: core types — API response types, enums for discovery/strategy/status"
```

---

## Task 5: Makefile + Deployment Setup (DEPLOY EARLY)

**Files:**
- Create: `Makefile`
- Create: `deploy/deploy.sh`
- Create: `deploy/systemd/evaluator.service`
- Create: `deploy/setup-evaluator.sh`

This task must happen BEFORE any feature work. The Makefile is our single workflow — build, test, deploy, verify. Every subsequent task ends with `make deploy` and a phase check.

**Step 1: Create Makefile**

```makefile
.PHONY: build test build-linux deploy check status

# === Build ===
build:
	cargo build --release

test:
	cargo test --all
	cargo clippy --all-targets -- -D warnings
	cargo fmt --check

build-linux:
	cargo build --release --target x86_64-unknown-linux-musl

# === Deploy ===
SERVER ?= ubuntu@YOUR_SERVER_IP
REMOTE_DIR ?= /opt/evaluator
DB = $(REMOTE_DIR)/data/evaluator.db
DB_CMD = ssh $(SERVER) 'sqlite3 $(DB)'

deploy: test build-linux
	scp target/x86_64-unknown-linux-musl/release/evaluator $(SERVER):$(REMOTE_DIR)/evaluator.new
	scp config/default.toml $(SERVER):$(REMOTE_DIR)/config/default.toml
	ssh $(SERVER) 'mv $(REMOTE_DIR)/evaluator.new $(REMOTE_DIR)/evaluator && sudo systemctl restart evaluator'
	@echo "Deployed. Waiting 10s for startup..."
	@sleep 10
	$(MAKE) check

# === Sanity Checks (the single source of truth) ===
#
# After every deploy, run the check for the current phase.
# If check fails, the deploy is bad — rollback or fix.

check: check-tables
	@echo "=== Basic sanity check passed ==="

check-phase-0: check-tables
	@echo "=== Phase 0: Foundation ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM sqlite_master WHERE type='table'" | \
		awk '{if ($$1 < 12) {print "FAIL: only " $$1 " tables (need 12+)"; exit 1} else print "OK: " $$1 " tables"}'
	@echo "Phase 0: PASSED"

check-phase-1: check-phase-0
	@echo "=== Phase 1: Market Discovery ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM markets" | \
		awk '{if ($$1 == 0) {print "FAIL: no markets"; exit 1} else print "OK: " $$1 " markets"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM market_scores WHERE score_date = date('now')" | \
		awk '{if ($$1 == 0) {print "FAIL: no market scores today"; exit 1} else print "OK: " $$1 " market scores today"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM market_scores WHERE score_date = date('now') AND rank <= 20" | \
		awk '{print "OK: " $$1 " top-20 markets selected"}'
	@echo "Phase 1: PASSED"

check-phase-2: check-phase-1
	@echo "=== Phase 2: Wallet Discovery ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM wallets" | \
		awk '{if ($$1 == 0) {print "FAIL: no wallets"; exit 1} else print "OK: " $$1 " wallets discovered"}'
	@$(DB_CMD) "SELECT COUNT(DISTINCT discovered_from) FROM wallets" | \
		awk '{if ($$1 < 2) {print "WARN: only " $$1 " discovery sources"} else print "OK: " $$1 " discovery sources"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM wallet_exclusions" | \
		awk '{print "OK: " $$1 " exclusion decisions recorded"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM wallet_personas" | \
		awk '{if ($$1 == 0) {print "FAIL: no personas classified"; exit 1} else print "OK: " $$1 " wallets classified"}'
	@echo "Phase 2: PASSED"

check-phase-3: check-phase-2
	@echo "=== Phase 3: Long-Term Tracking ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM trades_raw" | \
		awk '{if ($$1 == 0) {print "FAIL: no trades ingested"; exit 1} else print "OK: " $$1 " trades"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM activity_raw" | \
		awk '{print "OK: " $$1 " activity events"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM positions_snapshots" | \
		awk '{if ($$1 == 0) {print "FAIL: no position snapshots"; exit 1} else print "OK: " $$1 " position snapshots"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM holders_snapshots" | \
		awk '{print "OK: " $$1 " holder snapshots"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM raw_api_responses" | \
		awk '{print "OK: " $$1 " raw API responses saved"}'
	@$(DB_CMD) "SELECT CAST((julianday('now') - julianday(MAX(ingested_at))) * 24 AS INTEGER) FROM trades_raw" | \
		awk '{if ($$1 > 2) {print "WARN: trades " $$1 "h stale (target <2h)"} else print "OK: trades " $$1 "h fresh"}'
	@echo "Phase 3: PASSED"

check-phase-4: check-phase-3
	@echo "=== Phase 4: Paper Trading ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM paper_trades" | \
		awk '{if ($$1 == 0) {print "FAIL: no paper trades"; exit 1} else print "OK: " $$1 " paper trades"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM paper_events" | \
		awk '{print "OK: " $$1 " paper events (gate checks, skips, breakers)"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM follower_slippage" | \
		awk '{print "OK: " $$1 " slippage measurements"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM book_snapshots" | \
		awk '{print "OK: " $$1 " book snapshots"}'
	@echo "Phase 4: PASSED"

check-phase-5: check-phase-4
	@echo "=== Phase 5: Wallet Ranking ==="
	@$(DB_CMD) "SELECT COUNT(*) FROM wallet_scores_daily WHERE score_date = date('now')" | \
		awk '{if ($$1 == 0) {print "FAIL: no wallet scores today"; exit 1} else print "OK: " $$1 " wallet scores today"}'
	@$(DB_CMD) "SELECT COUNT(*) FROM wallet_scores_daily WHERE score_date = date('now') AND recommended_follow_mode IS NOT NULL" | \
		awk '{print "OK: " $$1 " wallets with follow-mode recommendation"}'
	@echo "Phase 5: PASSED"

check-tables:
	@$(DB_CMD) ".tables" | grep -q markets || (echo "FAIL: markets table missing" && exit 1)
	@$(DB_CMD) ".tables" | grep -q wallets || (echo "FAIL: wallets table missing" && exit 1)
	@$(DB_CMD) ".tables" | grep -q trades_raw || (echo "FAIL: trades_raw table missing" && exit 1)
	@$(DB_CMD) ".tables" | grep -q raw_api_responses || (echo "FAIL: raw_api_responses table missing" && exit 1)
	@echo "OK: core tables exist"

# === Status (human-friendly pipeline overview) ===
status:
	@echo "=== Pipeline Status ==="
	@$(DB_CMD) "SELECT 'markets:            ' || COUNT(*) FROM markets"
	@$(DB_CMD) "SELECT 'market scores today: ' || COUNT(*) FROM market_scores WHERE score_date = date('now')"
	@$(DB_CMD) "SELECT 'wallets:            ' || COUNT(*) FROM wallets"
	@$(DB_CMD) "SELECT 'wallet personas:    ' || COUNT(*) FROM wallet_personas"
	@$(DB_CMD) "SELECT 'wallet exclusions:  ' || COUNT(*) FROM wallet_exclusions"
	@$(DB_CMD) "SELECT 'trades:             ' || COUNT(*) FROM trades_raw"
	@$(DB_CMD) "SELECT 'activities:         ' || COUNT(*) FROM activity_raw"
	@$(DB_CMD) "SELECT 'position snapshots: ' || COUNT(*) FROM positions_snapshots"
	@$(DB_CMD) "SELECT 'holder snapshots:   ' || COUNT(*) FROM holders_snapshots"
	@$(DB_CMD) "SELECT 'raw API responses:  ' || COUNT(*) FROM raw_api_responses"
	@$(DB_CMD) "SELECT 'paper trades:       ' || COUNT(*) FROM paper_trades"
	@$(DB_CMD) "SELECT 'paper events:       ' || COUNT(*) FROM paper_events"
	@$(DB_CMD) "SELECT 'book snapshots:     ' || COUNT(*) FROM book_snapshots"
	@$(DB_CMD) "SELECT 'follower slippage:  ' || COUNT(*) FROM follower_slippage"
	@$(DB_CMD) "SELECT 'wallet scores today:' || COUNT(*) FROM wallet_scores_daily WHERE score_date = date('now')"
	@$(DB_CMD) "SELECT 'last trade ingested: ' || COALESCE(MAX(ingested_at), 'never') FROM trades_raw"
	@$(DB_CMD) "SELECT 'DB size:            ' || (page_count * page_size / 1024 / 1024) || ' MB' FROM pragma_page_count(), pragma_page_size()"
```

**Step 2: Create deploy/deploy.sh** (thin wrapper for CI or manual use)

```bash
#!/usr/bin/env bash
set -euo pipefail
make deploy SERVER="${1:-ubuntu@YOUR_SERVER_IP}"
```

**Step 3: Create deploy/systemd/evaluator.service**

Based on `trading` project's service files:
```ini
[Unit]
Description=Polymarket Trader Evaluator
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=evaluator
Group=evaluator
WorkingDirectory=/opt/evaluator
ExecStart=/opt/evaluator/evaluator
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

# Hardening
ProtectSystem=strict
ReadWritePaths=/opt/evaluator/data
PrivateTmp=true
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
```

**Step 4: Create deploy/setup-evaluator.sh**

One-time server setup: create user, directories, install service, install sqlite3 CLI (needed for `make check`).

**Step 5: Test the Makefile locally**

Run: `make build && make test`
Expected: Compiles and tests pass.

**Step 6: First deploy (empty binary — just proves the pipeline works)**

Run: `make deploy`
Expected: Binary uploaded, service starts, `make check-tables` passes (DB created with schema), `make check-phase-0` passes.

**Step 7: Commit**

```bash
git add Makefile deploy/
git commit -m "feat: Makefile + deployment — deploy-early pipeline with per-phase SQLite sanity checks"
```

> **From this point forward, every task ends with `make deploy && make check-phase-N`.**

---

## Task 6: Polymarket API Client

> **Deploy checkpoint:** After this task, `make deploy && make check-phase-0` — binary runs on AWS, schema created, APIs reachable.

**Files:**
- Create: `crates/common/src/polymarket.rs`
- Create: `tests/fixtures/` (directory)
- Modify: `crates/common/src/lib.rs`
- Modify: `crates/common/Cargo.toml` (add reqwest)

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_constructs_trades_url() {
        let client = PolymarketClient::new("https://data-api.polymarket.com", "https://gamma-api.polymarket.com");
        let url = client.trades_url("0xabc123", None, 100, 0);
        assert!(url.contains("/trades"));
        assert!(url.contains("user=0xabc123"));
        assert!(url.contains("limit=100"));
    }

    #[test]
    fn test_parse_trades_response() {
        let json = r#"[{"proxyWallet":"0xabc","conditionId":"0xdef","size":"10","price":"0.50","timestamp":1700000000}]"#;
        let trades: Vec<ApiTrade> = serde_json::from_str(json).unwrap();
        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].proxy_wallet.as_deref(), Some("0xabc"));
    }

    #[test]
    fn test_parse_holders_response() {
        let json = r#"[{"token":"0xtok","holders":[{"proxyWallet":"0xabc","amount":100.0,"outcomeIndex":0}]}]"#;
        let holders: Vec<ApiHolderResponse> = serde_json::from_str(json).unwrap();
        assert_eq!(holders[0].holders.len(), 1);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p common`
Expected: FAIL

**Step 3: Implement PolymarketClient**

Implement a client struct with methods:
- `fetch_trades(user, market, limit, offset) -> Vec<ApiTrade>`
- `fetch_holders(condition_ids, limit) -> Vec<ApiHolderResponse>`
- `fetch_activity(user, limit, offset) -> Vec<ApiActivity>`
- `fetch_positions(user, limit, offset) -> Vec<ApiPosition>`
- `fetch_leaderboard(category, time_period, limit, offset) -> Vec<ApiLeaderboardEntry>`
- `fetch_gamma_markets(limit, offset) -> Vec<GammaMarket>`

Each method: builds URL, makes GET request, parses JSON, returns typed result. Includes rate limiting (configurable delay between calls) and retries with exponential backoff.

**Step 4: Run test to verify it passes**

Run: `cargo test -p common`
Expected: PASS

**Step 5: Save real API responses as test fixtures**

Run manually (one-time):
```bash
curl -s "https://data-api.polymarket.com/trades?limit=5" > tests/fixtures/trades_sample.json
curl -s "https://gamma-api.polymarket.com/markets?limit=5" > tests/fixtures/gamma_markets_sample.json
```

**Step 6: Add fixture-based parsing tests**

Test that the client can parse real API responses without panicking.

**Step 7: Commit**

```bash
git add crates/common/src/polymarket.rs tests/fixtures/
git commit -m "feat: Polymarket API client — trades, holders, activity, positions, leaderboard, gamma markets"
```

---

## Task 7: Market Scoring (MScore)

**Files:**
- Create: `crates/evaluator/src/market_scoring.rs`
- Modify: `crates/evaluator/src/main.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mscore_computation() {
        let market = MarketCandidate {
            condition_id: "0xabc".to_string(),
            title: "Will BTC go up?".to_string(),
            liquidity: 50000.0,
            volume_24h: 20000.0,
            trades_24h: 100,
            unique_traders_24h: 30,
            top_holder_concentration: 0.4,
            days_to_expiry: 14,
        };
        let weights = ScoringWeights {
            liquidity: 0.25,
            volume: 0.25,
            density: 0.20,
            whale_concentration: 0.15,
            time_to_expiry: 0.15,
        };
        let score = compute_mscore(&market, &weights);
        assert!(score > 0.0);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_mscore_zero_liquidity_scores_low() {
        let market = MarketCandidate {
            condition_id: "0xabc".to_string(),
            title: "Dead market".to_string(),
            liquidity: 0.0,
            volume_24h: 0.0,
            trades_24h: 0,
            unique_traders_24h: 0,
            top_holder_concentration: 0.0,
            days_to_expiry: 14,
        };
        let weights = ScoringWeights::default();
        let score = compute_mscore(&market, &weights);
        assert!(score < 0.1);
    }

    #[test]
    fn test_rank_markets_returns_top_n() {
        let markets = vec![/* 5 markets with varying scores */];
        let ranked = rank_markets(markets, 3);
        assert_eq!(ranked.len(), 3);
        assert!(ranked[0].mscore >= ranked[1].mscore);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator`
Expected: FAIL

**Step 3: Implement MScore computation**

- Each factor normalized to [0, 1] using log-scaling or min-max
- Liquidity: `min(1.0, log10(liquidity + 1) / log10(1_000_000))`
- Volume: `min(1.0, log10(volume + 1) / log10(500_000))`
- Density: `min(1.0, trades_24h as f64 / 500.0)`
- Whale concentration: inverted — high concentration = lower score: `1.0 - concentration`
- Time to expiry: bell curve — peaks at 7-30 days, drops at extremes
- Weighted sum produces MScore in [0, 1]

**Step 4: Run test to verify it passes**

Run: `cargo test -p evaluator`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/evaluator/src/market_scoring.rs
git commit -m "feat: MScore computation — weighted market scoring with 5 factors"
```

---

## Task 8: Wallet Discovery

> **Deploy checkpoint:** After Tasks 7-8, `make deploy && make check-phase-1 && make check-phase-2` — markets scored, wallets discovered and classified. Data collection starts NOW.

**Files:**
- Create: `crates/evaluator/src/wallet_discovery.rs`

**Step 1: Write the failing test**

Test that `discover_wallets_for_market` returns wallets tagged with correct source, deduplicates across holder and trader sources, and applies minimum trade filter.

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator`
Expected: FAIL

**Step 3: Implement wallet discovery**

- For each selected market:
  1. Fetch top holders via `/holders` -> tag as HOLDER
  2. Fetch recent trades via `/trades?market=X` -> extract unique proxy_wallets -> tag as TRADER_RECENT
  3. Deduplicate (same wallet from multiple sources keeps earliest tag)
  4. Apply pruning filters (min trades from config)
  5. Persist to `wallets` table
- Leaderboard seeding (separate job): fetch `/v1/leaderboard` -> tag as LEADERBOARD

**Step 4: Run test to verify it passes**

Run: `cargo test -p evaluator`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/evaluator/src/wallet_discovery.rs
git commit -m "feat: wallet discovery — holders, traders, leaderboard seeding with dedup and pruning"
```

---

## Task 9: Ingestion Jobs

> **Deploy checkpoint:** After this task, `make deploy && make check-phase-3` — trades, activity, positions, holders flowing into DB. `make status` shows growing row counts.

**Files:**
- Create: `crates/evaluator/src/ingestion.rs`

**Step 1: Write the failing test**

Test that `ingest_trades_for_wallet` deduplicates by transaction_hash, saves the raw unmodified HTTP response body to `raw_api_responses`, and handles pagination correctly.

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator`
Expected: FAIL

**Step 3: Implement ingestion jobs**

Four periodic jobs:
1. **Trades ingestion:** For each watched wallet, fetch recent trades, deduplicate by tx_hash, insert into `trades_raw`
2. **Activity ingestion:** For each watched wallet, fetch activity timeline, deduplicate, insert into `activity_raw`
3. **Positions snapshot:** For each watched wallet, fetch current positions, insert into `positions_snapshots` with timestamp
4. **Holders snapshot:** For each selected market, fetch top holders, insert into `holders_snapshots` with timestamp

Each job: paginate through results, apply rate limiting, retry on failure, and:
- Save the **raw, unmodified HTTP response body bytes** into `raw_api_responses` (append-only) before parsing.
- Then parse and insert the derived rows into `trades_raw` / `activity_raw` / `positions_snapshots` / `holders_snapshots` (those are recomputable).

**Step 4: Run test to verify it passes**

Run: `cargo test -p evaluator`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/evaluator/src/ingestion.rs
git commit -m "feat: ingestion jobs — trades, activity, positions, holders with dedup and pagination"
```

---

## Task 10: Paper Trading Engine

> **Deploy checkpoint:** After this task, `make deploy && make check-phase-4` — paper trades executing, gate checks logged, follower slippage tracked.

**Files:**
- Create: `crates/evaluator/src/paper_trading.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mirror_trade_creates_paper_trade() {
        // Given a wallet trade (BUY YES at 0.60)
        // When mirror strategy processes it
        // Then a paper trade is created with slippage applied
    }

    #[test]
    fn test_risk_cap_blocks_oversized_trade() {
        // Given a bankroll of $10,000 and max_per_market = 10%
        // When existing exposure in market is $900
        // Then a new $200 trade is blocked (would exceed $1,000 cap)
    }

    #[test]
    fn test_portfolio_stop_halts_trading() {
        // Given portfolio_stop_drawdown = 15%
        // When cumulative PnL is -$1,600 on $10,000 bankroll (16%)
        // Then new trades are rejected
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator`
Expected: FAIL

**Step 3: Implement paper trading engine**

- **Mirror strategy:** When a new trade appears in `trades_raw` for a watched wallet, create a corresponding `paper_trade` with:
  - Same direction and outcome
  - Fixed position size from config
  - Slippage applied (entry_price +/- slippage_pct)
  - Status = "open"
- **Risk checks** (all must pass before creating paper trade):
  - Market exposure < max_per_market (bankroll * pct)
  - Wallet exposure < max_per_wallet (bankroll * pct)
  - Daily trade count < max_daily_trades
  - No chasing: if position already exists and price moved adverse by > threshold, skip
  - Portfolio stop: if cumulative drawdown > threshold, halt all paper trading
- **Settlement:** When a market resolves (position goes to $1 or $0), mark paper trades as settled_win or settled_loss, compute PnL.

**Step 4: Run test to verify it passes**

Run: `cargo test -p evaluator`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/evaluator/src/paper_trading.rs
git commit -m "feat: paper trading engine — mirror strategy with risk caps, slippage, and portfolio stop"
```

---

## Task 11: Wallet Scoring (WScore)

> **Deploy checkpoint:** After this task, `make deploy && make check-phase-5` — wallet scores computed daily, follow-mode recommendations present.

**Files:**
- Create: `crates/evaluator/src/wallet_scoring.rs`

**Step 1: Write the failing test**

Test that `compute_wscore` produces a score in [0, 1] based on paper trading results. Test that wallets with positive edge score higher. Test that unstable wallets (high variance) score lower on consistency.

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator`
Expected: FAIL

**Step 3: Implement WScore**

For each window (7d, 30d, 90d):
- **Edge score:** `paper_roi / max_possible_roi` — normalized [0, 1], negative ROI = 0
- **Consistency score:** `1.0 - (daily_return_stdev / max_stdev)` — low variance = high score
- **Market skill score:** `unique_profitable_markets / total_markets_traded` — edge across markets
- **Timing skill score:** average post-entry drift in favorable direction (did price move our way after entry?)
- **Behavior quality score:** `1.0 - (noise_trades / total_trades)` — fewer tiny scattered trades = better

WScore = weighted sum of all factors.

Output: recommended follow mode based on behavior pattern:
- High timing skill -> "mirror" (follow immediately)
- High edge but slow -> "delay" (wait for confirmation)
- Multiple wallets agree -> "consensus"

**Step 4: Run test to verify it passes**

Run: `cargo test -p evaluator`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/evaluator/src/wallet_scoring.rs
git commit -m "feat: WScore computation — edge, consistency, market skill, timing, behavior quality"
```

---

## Task 12: Main Event Loop & Job Scheduler

**Files:**
- Modify: `crates/evaluator/src/main.rs`
- Create: `crates/evaluator/src/scheduler.rs`

**Step 1: Write the failing test**

Test that the scheduler fires jobs at configured intervals.

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator`
Expected: FAIL

**Step 3: Implement scheduler**

Use `tokio::time::interval` for each job:
- Market scoring: every `market_scoring.refresh_interval_secs`
- Wallet discovery: every `wallet_discovery.refresh_interval_secs`
- Trade ingestion: every `ingestion.trades_poll_interval_secs`
- Activity ingestion: every `ingestion.activity_poll_interval_secs`
- Position snapshots: every `ingestion.positions_poll_interval_secs`
- Holder snapshots: every `ingestion.holders_poll_interval_secs`
- Paper trade processing: every 60 seconds (check for new trades to mirror)
- WScore computation: daily at midnight

Wire everything together in `main.rs` using `tokio::select!` to run all jobs concurrently.

**Step 4: Run test to verify it passes**

Run: `cargo test -p evaluator`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/evaluator/src/scheduler.rs crates/evaluator/src/main.rs
git commit -m "feat: job scheduler — Tokio-based periodic jobs for all pipeline stages"
```

---

## Task 13: Observability (Prometheus Metrics)

**Files:**
- Create: `crates/evaluator/src/metrics.rs`
- Modify: `crates/evaluator/Cargo.toml` (add metrics crates)
- Modify: `crates/evaluator/src/main.rs`

**Step 1: Define metrics**

Key metrics to export:
- `evaluator_markets_scored_total` — counter
- `evaluator_wallets_discovered_total` — counter
- `evaluator_wallets_on_watchlist` — gauge
- `evaluator_trades_ingested_total` — counter
- `evaluator_paper_trades_total` — counter (labels: strategy, status)
- `evaluator_paper_pnl` — gauge (labels: strategy)
- `evaluator_api_requests_total` — counter (labels: endpoint, status)
- `evaluator_api_latency_ms` — histogram (labels: endpoint)
- `evaluator_ingestion_lag_secs` — gauge
- `evaluator_risk_violations_total` — counter (labels: rule)

**Step 2: Implement metrics registration and Prometheus exporter**

Start `metrics-exporter-prometheus` on configured port.

**Step 3: Instrument all existing code with metric increments**

Add counters/gauges at appropriate points in market_scoring, wallet_discovery, ingestion, paper_trading, wallet_scoring.

**Step 4: Test locally**

Run: `curl http://localhost:9094/metrics | grep evaluator`
Expected: Metric names appear.

**Step 5: Commit**

```bash
git add crates/evaluator/src/metrics.rs
git commit -m "feat: Prometheus metrics — 10 key metrics covering pipeline, PnL, API, and risk"
```

---

## Task 14: CLI Commands for Manual Inspection

**Files:**
- Create: `crates/evaluator/src/cli.rs`
- Modify: `crates/evaluator/src/main.rs`

Add subcommands:
- `evaluator run` — start the main loop (default)
- `evaluator markets` — show today's top-20 markets with MScore breakdown
- `evaluator wallets` — show watchlist with status
- `evaluator wallet <address>` — show wallet detail (trades, positions, paper results)
- `evaluator paper-pnl` — show paper portfolio performance
- `evaluator rankings` — show WScore rankings

These commands read from SQLite and print formatted tables. No API calls needed.

**Step 1: Implement using clap or simple arg parsing**

**Step 2: Test each command manually against a seeded database**

**Step 3: Commit**

```bash
git add crates/evaluator/src/cli.rs
git commit -m "feat: CLI commands — markets, wallets, paper-pnl, rankings for manual inspection"
```

---

## Task 15: Integration Test with Real APIs

**Files:**
- Create: `tests/integration/` or `crates/evaluator/tests/`

**Step 1: Write integration tests (marked #[ignore])**

```rust
#[tokio::test]
#[ignore] // requires network
async fn test_fetch_real_markets() {
    let client = PolymarketClient::new(...);
    let markets = client.fetch_gamma_markets(5, 0).await.unwrap();
    assert!(markets.len() > 0);
    // Save fixture
    std::fs::write("tests/fixtures/gamma_markets_live.json",
        serde_json::to_string_pretty(&markets).unwrap()).unwrap();
}

#[tokio::test]
#[ignore]
async fn test_fetch_real_trades() {
    let client = PolymarketClient::new(...);
    let trades = client.fetch_trades(None, Some("CONDITION_ID"), 10, 0).await.unwrap();
    assert!(trades.len() > 0);
}
```

**Step 2: Run integration tests**

Run: `cargo test -- --ignored`
Expected: Tests pass and fixtures are saved.

**Step 3: Commit**

```bash
git add tests/
git commit -m "test: integration tests with real Polymarket APIs (ignored by default)"
```

---

## Summary of Task Dependencies

```
Task 1 (skeleton) ─── everything depends on this
  ├── Task 2 (config)
  ├── Task 3 (database)
  └── Task 4 (types)
        └── Task 5 (Makefile + deploy) ─── deploy pipeline ready
              └── Task 6 (API client) ─── depends on types + deploy
                    ├── Task 7 (market scoring)
                    │     └── Task 8 (wallet discovery)
                    │           └─ DEPLOY: make check-phase-1 && make check-phase-2
                    │           └── Task 9 (ingestion)
                    │                 └─ DEPLOY: make check-phase-3 (data flowing!)
                    │                 └── Task 10 (paper trading)
                    │                       └─ DEPLOY: make check-phase-4
                    │                       └── Task 11 (wallet scoring)
                    │                             └─ DEPLOY: make check-phase-5
                    └── Task 15 (integration tests) ─── parallel with Task 7

Task 12 (scheduler) ─── depends on Tasks 7-11
Task 13 (metrics) ─── depends on Tasks 7-11
Task 14 (CLI) ─── depends on Tasks 3, 7, 10, 11
```

**Key difference from v1 plan:** Deployment is Task 5, not Task 13. By the time we have an API client (Task 6), we deploy to AWS and start collecting data. Every feature ships immediately. `make deploy && make check-phase-N` is the rhythm.

**Parallel execution opportunities:**
- Tasks 2, 3, 4 can run in parallel (after Task 1)
- Tasks 7 and 15 can run in parallel (after Task 6)
- Tasks 12, 13, 14 can run in parallel (after Task 11)

---

## Phase Gates (deploy checkpoints)

After Tasks 1-6: **`make deploy && make check-phase-0`** — binary runs on AWS, schema created, APIs reachable.

After Tasks 7-8: **`make deploy && make check-phase-2`** — markets scored, wallets discovered, personas classified. **Data collection starts.**

After Task 9: **`make deploy && make check-phase-3`** — trades, activity, positions, holders flowing. `make status` shows growing row counts.

After Task 10: **`make deploy && make check-phase-4`** — paper trades executing, gate checks logged, follower slippage tracked.

After Task 11: **`make deploy && make check-phase-5`** — wallet scores computed, follow-mode recommendations present.

**At each gate: run `make status` to see pipeline health, and run `/evaluator-guidance` skill to assess progress against EVALUATION_STRATEGY.md.**
