use anyhow::Result;
use rusqlite::Connection;

pub struct Database {
    pub conn: Connection,
}

/// Async database wrapper around `tokio_rusqlite::Connection`.
///
/// Runs all SQLite operations on a dedicated background thread via
/// `tokio_rusqlite`, keeping the Tokio runtime cooperative. Clone is
/// cheap (shared mpsc sender to the background thread).
#[derive(Clone)]
pub struct AsyncDb {
    conn: tokio_rusqlite::Connection,
}

impl AsyncDb {
    /// Open a database at `path`, set PRAGMAs (WAL, foreign keys, busy_timeout),
    /// and run migrations — all on the background thread.
    pub async fn open(path: &str) -> Result<Self> {
        let conn = tokio_rusqlite::Connection::open(path).await?;

        // Startup migrations require a write lock. On production systems we can race with
        // concurrent readers/writers (web requests, admin sqlite3 sessions, deploy checks).
        // If we hard-fail on `database is locked`, systemd will crash-loop. Instead we retry
        // migrations with backoff until the lock clears.
        //
        // IMPORTANT: Use a short SQLite busy_timeout per attempt so we can handle backoff in Rust.
        let mut backoff = std::time::Duration::from_secs(1);
        let max_backoff = std::time::Duration::from_secs(30);
        let max_total_wait = std::time::Duration::from_secs(10 * 60);
        let start = std::time::Instant::now();

        loop {
            let res = conn
                .call(|conn| -> std::result::Result<(), rusqlite::Error> {
                    conn.busy_timeout(std::time::Duration::from_secs(1))?;
                    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
                    migrate_rename_market_scores_daily_to_market_scores(conn)?;
                    conn.execute_batch(SCHEMA)?;
                    migrate_markets_is_crypto_15m(conn)?;
                    migrate_wallet_features_domain_columns(conn)?;
                    migrate_wallet_features_ag_columns(conn)?;
                    // For normal runtime operations we still want a longer busy_timeout.
                    conn.busy_timeout(std::time::Duration::from_secs(30))?;
                    Ok(())
                })
                .await;

            match res {
                Ok(()) => break,
                Err(tokio_rusqlite::Error::Error(err)) => {
                    let is_locked = matches!(
                        err,
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error {
                                code: rusqlite::ffi::ErrorCode::DatabaseBusy
                                    | rusqlite::ffi::ErrorCode::DatabaseLocked,
                                ..
                            },
                            _,
                        )
                    );
                    if !is_locked {
                        return Err(
                            anyhow::Error::from(err).context("AsyncDb::open: migration failed")
                        );
                    }

                    if start.elapsed() >= max_total_wait {
                        return Err(anyhow::Error::from(err).context(
                            "AsyncDb::open: migration failed (database stayed locked too long)",
                        ));
                    }

                    tracing::warn!(
                        wait_for = ?backoff,
                        "AsyncDb::open: database is locked; retrying migrations"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                }
                Err(other) => return Err(anyhow::anyhow!("AsyncDb::open: {other}")),
            }
        }

        Ok(Self { conn })
    }

    /// Run a closure on the background SQLite thread and return the result.
    ///
    /// The closure receives `&mut rusqlite::Connection` and can perform
    /// arbitrary sync SQLite operations. The result is sent back via oneshot
    /// channel.
    pub async fn call<F, R>(&self, function: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        self.conn.call(move |conn| function(conn)).await.map_err(
            |e: tokio_rusqlite::Error<anyhow::Error>| match e {
                tokio_rusqlite::Error::ConnectionClosed => {
                    anyhow::anyhow!("database connection closed")
                }
                tokio_rusqlite::Error::Close((_, err)) => {
                    anyhow::anyhow!("database close error: {err}")
                }
                tokio_rusqlite::Error::Error(err) => err,
                other => anyhow::anyhow!("database error: {other}"),
            },
        )
    }

    /// Like [`Self::call`], but records Prometheus metrics for DB latency and errors.
    ///
    /// This measures the full wall-clock time of the operation, including queueing
    /// on the dedicated SQLite thread and execution of all SQL in the closure.
    pub async fn call_named<F, R>(&self, op: &'static str, function: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let start = std::time::Instant::now();
        let res = self.call(function).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;

        match &res {
            Ok(_) => {
                metrics::histogram!(
                    "evaluator_db_query_latency_ms",
                    "op" => op,
                    "status" => "ok"
                )
                .record(ms);
            }
            Err(_) => {
                metrics::histogram!(
                    "evaluator_db_query_latency_ms",
                    "op" => op,
                    "status" => "err"
                )
                .record(ms);
                metrics::counter!("evaluator_db_query_errors_total", "op" => op).increment(1);
            }
        }

        res
    }
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        // busy_timeout via the rusqlite API — makes SQLite retry for up to 30s
        // when the database is locked by another connection (concurrent jobs).
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Ok(Self { conn })
    }

    pub fn run_migrations(&self) -> Result<()> {
        migrate_rename_market_scores_daily_to_market_scores(&self.conn)
            .map_err(anyhow::Error::from)?;
        self.conn.execute_batch(SCHEMA)?;
        migrate_markets_is_crypto_15m(&self.conn).map_err(anyhow::Error::from)?;
        migrate_wallet_features_domain_columns(&self.conn).map_err(anyhow::Error::from)?;
        migrate_wallet_features_ag_columns(&self.conn).map_err(anyhow::Error::from)?;
        Ok(())
    }
}

/// Rename table market_scores_daily → market_scores (for existing DBs).
fn migrate_rename_market_scores_daily_to_market_scores(
    conn: &Connection,
) -> std::result::Result<(), rusqlite::Error> {
    let old_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='market_scores_daily'",
        [],
        |row| row.get(0),
    )?;
    let new_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='market_scores'",
        [],
        |row| row.get(0),
    )?;
    if old_exists > 0 && new_exists == 0 {
        conn.execute(
            "ALTER TABLE market_scores_daily RENAME TO market_scores",
            [],
        )?;
    }
    Ok(())
}

/// Add is_crypto_15m column to markets if missing (for existing DBs created before Task 14).
fn migrate_markets_is_crypto_15m(conn: &Connection) -> std::result::Result<(), rusqlite::Error> {
    let has: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('markets') WHERE name='is_crypto_15m'",
        [],
        |row| row.get(0),
    )?;
    if has == 0 {
        conn.execute(
            "ALTER TABLE markets ADD COLUMN is_crypto_15m INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

fn migrate_wallet_features_ag_columns(
    conn: &Connection,
) -> std::result::Result<(), rusqlite::Error> {
    let required: [(&str, &str); 13] = [
        ("trades_per_day", "REAL NOT NULL DEFAULT 0.0"),
        ("avg_trade_size_usdc", "REAL NOT NULL DEFAULT 0.0"),
        ("size_cv", "REAL NOT NULL DEFAULT 0.0"),
        ("buy_sell_balance", "REAL NOT NULL DEFAULT 0.0"),
        ("mid_fill_ratio", "REAL NOT NULL DEFAULT 0.0"),
        ("extreme_price_ratio", "REAL NOT NULL DEFAULT 0.0"),
        ("burstiness_top_1h_ratio", "REAL NOT NULL DEFAULT 0.0"),
        ("top_domain", "TEXT"),
        ("top_domain_ratio", "REAL NOT NULL DEFAULT 0.0"),
        ("profitable_markets", "INTEGER NOT NULL DEFAULT 0"),
        ("sharpe_ratio", "REAL NOT NULL DEFAULT 0.0"),
        ("concentration_ratio", "REAL NOT NULL DEFAULT 0.0"),
        ("active_positions", "INTEGER NOT NULL DEFAULT 0"),
    ];
    for (name, ty) in required {
        let has: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('wallet_features_daily') WHERE name=?1",
            rusqlite::params![name],
            |row| row.get(0),
        )?;
        if has == 0 {
            conn.execute(
                &format!("ALTER TABLE wallet_features_daily ADD COLUMN {name} {ty}"),
                [],
            )?;
        }
    }
    Ok(())
}

/// Rename top_category -> top_domain (domain hierarchy terminology).
/// Skip if top_domain already exists (e.g. from a previous ag_columns add), to avoid duplicate column.
fn migrate_wallet_features_domain_columns(
    conn: &Connection,
) -> std::result::Result<(), rusqlite::Error> {
    let has_old: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('wallet_features_daily') WHERE name='top_category'",
        [],
        |row| row.get(0),
    )?;
    let has_new: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('wallet_features_daily') WHERE name='top_domain'",
        [],
        |row| row.get(0),
    )?;
    if has_old > 0 && has_new == 0 {
        conn.execute(
            "ALTER TABLE wallet_features_daily RENAME COLUMN top_category TO top_domain",
            [],
        )?;
        conn.execute("ALTER TABLE wallet_features_daily RENAME COLUMN top_category_ratio TO top_domain_ratio", [])?;
    }
    Ok(())
}

const SCHEMA: &str = r#"
-- WARNING: INSERTs into raw_api_responses were removed (2026-02-08 storage crisis).
-- The table stored full HTTP response bodies (~300KB each, ~3.7 GB after 28 hours).
-- Per-row raw_json columns in trades_raw, activity_raw, etc. already provide
-- equivalent replay capability. Do NOT re-enable bulk raw response storage
-- without disk budget analysis. See deploy/purge-raw.sh for cleanup.
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
    condition_id TEXT PRIMARY KEY,  -- market (outcome within event)
    title TEXT NOT NULL,
    slug TEXT,
    description TEXT,
    end_date TEXT,
    liquidity REAL,
    volume REAL,
    category TEXT,                 -- domain (Polymarket: Sports, Politics, Crypto)
    event_slug TEXT,               -- event (e.g. sparta-slavia)
    outcomes_json TEXT,              -- raw JSON of outcome tokens
    is_crypto_15m INTEGER NOT NULL DEFAULT 0,  -- 1 = quartic taker fee applies
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
    condition_id TEXT NOT NULL,     -- market; we aggregate to event via event_slug
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

CREATE TABLE IF NOT EXISTS scoring_stats (
    score_date TEXT PRIMARY KEY,
    total_events_evaluated INTEGER NOT NULL,
    top_events_selected INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS discovery_scheduler_state (
    key TEXT PRIMARY KEY,
    value_int INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT
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
    trades_per_week REAL,
    sharpe_ratio REAL,
    active_positions INTEGER,
    concentration_ratio REAL,
    trades_per_day REAL NOT NULL DEFAULT 0.0,
    avg_trade_size_usdc REAL NOT NULL DEFAULT 0.0,
    size_cv REAL NOT NULL DEFAULT 0.0,
    buy_sell_balance REAL NOT NULL DEFAULT 0.0,
    mid_fill_ratio REAL NOT NULL DEFAULT 0.0,
    extreme_price_ratio REAL NOT NULL DEFAULT 0.0,
    burstiness_top_1h_ratio REAL NOT NULL DEFAULT 0.0,
    top_domain TEXT,               -- dominant domain (wallet's lane)
    top_domain_ratio REAL NOT NULL DEFAULT 0.0,
    profitable_markets INTEGER NOT NULL DEFAULT 0,
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

CREATE TABLE IF NOT EXISTS wallet_personas (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    persona TEXT NOT NULL,             -- Informed Specialist, Consistent Generalist, etc.
    confidence REAL NOT NULL,          -- 0.0 to 1.0
    feature_values_json TEXT,          -- JSON: trade_count, win_rate, unique_markets, etc.
    classified_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now')),
    UNIQUE(proxy_wallet, classified_at)
);

CREATE TABLE IF NOT EXISTS wallet_exclusions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    reason TEXT NOT NULL,              -- e.g. "tail_risk_seller", "noise_trader", "too_young"
    metric_value REAL,                 -- the actual value that triggered exclusion
    threshold REAL,                    -- the threshold it was compared against
    excluded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now')),
    UNIQUE(proxy_wallet, reason)
);

CREATE TABLE IF NOT EXISTS wallet_persona_traits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    trait_key TEXT NOT NULL,
    trait_value TEXT NOT NULL,
    computed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now')),
    UNIQUE(proxy_wallet, trait_key)
);

CREATE TABLE IF NOT EXISTS wallet_rules_state (
    proxy_wallet TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    baseline_style_json TEXT,
    last_seen_ts INTEGER,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now'))
);

CREATE TABLE IF NOT EXISTS wallet_rules_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    phase TEXT NOT NULL,              -- discovery|paper|live
    allow INTEGER NOT NULL,           -- 1|0
    reason TEXT NOT NULL,
    metrics_json TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%f', 'now'))
);

CREATE TABLE IF NOT EXISTS job_status (
    job_name TEXT PRIMARY KEY,
    status TEXT NOT NULL,              -- running, idle, failed
    last_run_at TEXT,
    duration_ms INTEGER,
    last_error TEXT,
    metadata TEXT,                     -- JSON with progress info
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS event_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,          -- pipeline, operational
    event_data TEXT NOT NULL,          -- JSON serialized event
    emitted_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_trades_raw_wallet ON trades_raw(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_trades_raw_wallet_timestamp ON trades_raw(proxy_wallet, timestamp);
CREATE INDEX IF NOT EXISTS idx_trades_raw_market ON trades_raw(condition_id);
CREATE INDEX IF NOT EXISTS idx_trades_raw_timestamp ON trades_raw(timestamp);
CREATE INDEX IF NOT EXISTS idx_trades_raw_ingested_at ON trades_raw(ingested_at);
CREATE INDEX IF NOT EXISTS idx_activity_raw_wallet ON activity_raw(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_activity_raw_ingested_at ON activity_raw(ingested_at);
CREATE INDEX IF NOT EXISTS idx_positions_wallet ON positions_snapshots(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_positions_snapshots_snapshot_at ON positions_snapshots(snapshot_at);
CREATE INDEX IF NOT EXISTS idx_holders_market ON holders_snapshots(condition_id);
CREATE INDEX IF NOT EXISTS idx_holders_snapshots_snapshot_at ON holders_snapshots(snapshot_at);
CREATE INDEX IF NOT EXISTS idx_raw_api_responses_fetched_at ON raw_api_responses(fetched_at);
CREATE INDEX IF NOT EXISTS idx_paper_trades_wallet ON paper_trades(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_paper_trades_status ON paper_trades(status);
CREATE INDEX IF NOT EXISTS idx_paper_trades_created_at ON paper_trades(created_at);
CREATE INDEX IF NOT EXISTS idx_paper_trades_triggered_by_trade_id ON paper_trades(triggered_by_trade_id);
CREATE INDEX IF NOT EXISTS idx_wallets_discovered_at ON wallets(discovered_at);
CREATE INDEX IF NOT EXISTS idx_market_scores_date_rank ON market_scores(score_date, rank);
CREATE INDEX IF NOT EXISTS idx_wallet_scores_date ON wallet_scores_daily(score_date);
CREATE INDEX IF NOT EXISTS idx_wallet_scores_date_window_wscore ON wallet_scores_daily(score_date, window_days, wscore DESC);
CREATE INDEX IF NOT EXISTS idx_wallet_personas_wallet ON wallet_personas(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_wallet_exclusions_wallet ON wallet_exclusions(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_wallet_persona_traits_wallet ON wallet_persona_traits(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_event_log_emitted_at ON event_log(emitted_at);
CREATE INDEX IF NOT EXISTS idx_event_log_type ON event_log(event_type);
CREATE INDEX IF NOT EXISTS idx_wallet_rules_events_wallet ON wallet_rules_events(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_wallet_rules_events_phase_created_at ON wallet_rules_events(phase, created_at);

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
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_create_all_tables() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let tables: Vec<String> = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();

        assert!(tables.contains(&"markets".to_string()));
        assert!(tables.contains(&"wallets".to_string()));
        assert!(tables.contains(&"raw_api_responses".to_string()));
        assert!(tables.contains(&"trades_raw".to_string()));
        assert!(tables.contains(&"activity_raw".to_string()));
        assert!(tables.contains(&"positions_snapshots".to_string()));
        assert!(tables.contains(&"holders_snapshots".to_string()));
        assert!(tables.contains(&"market_scores".to_string()));
        assert!(tables.contains(&"scoring_stats".to_string()));
        assert!(tables.contains(&"discovery_scheduler_state".to_string()));
        assert!(tables.contains(&"wallet_features_daily".to_string()));
        assert!(tables.contains(&"paper_trades".to_string()));
        assert!(tables.contains(&"paper_positions".to_string()));
        assert!(tables.contains(&"wallet_scores_daily".to_string()));
        assert!(tables.contains(&"wallet_personas".to_string()));
        assert!(tables.contains(&"wallet_exclusions".to_string()));
        assert!(tables.contains(&"wallet_persona_traits".to_string()));
        assert!(tables.contains(&"wallet_rules_state".to_string()));
        assert!(tables.contains(&"wallet_rules_events".to_string()));
        assert!(tables.contains(&"event_log".to_string()));
    }

    #[test]
    fn test_migrations_idempotent() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        db.run_migrations().unwrap(); // second call must not fail
    }

    #[test]
    fn test_migrations_create_expected_indexes() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let indexes: Vec<String> = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();

        // These are required for the dashboard and pipeline to remain fast as the DB grows.
        let expected = [
            "idx_paper_trades_triggered_by_trade_id",
            "idx_paper_trades_created_at",
            "idx_wallets_discovered_at",
            "idx_wallet_scores_date_window_wscore",
            "idx_market_scores_date_rank",
            "idx_trades_raw_ingested_at",
            "idx_activity_raw_ingested_at",
            "idx_positions_snapshots_snapshot_at",
            "idx_holders_snapshots_snapshot_at",
        ];

        for name in expected {
            assert!(
                indexes.contains(&name.to_string()),
                "missing index {name}; existing indexes: {indexes:?}"
            );
        }
    }

    #[test]
    fn test_wallet_features_daily_has_ag_columns() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let cols: Vec<String> = db
            .conn
            .prepare("SELECT name FROM pragma_table_info('wallet_features_daily') ORDER BY cid")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();

        for col in [
            "trades_per_day",
            "avg_trade_size_usdc",
            "size_cv",
            "buy_sell_balance",
            "mid_fill_ratio",
            "extreme_price_ratio",
            "burstiness_top_1h_ratio",
            "top_domain",
            "top_domain_ratio",
        ] {
            assert!(
                cols.contains(&col.to_string()),
                "missing column {col}; got {cols:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_async_db_open_runs_migrations() {
        let db = AsyncDb::open(":memory:").await.unwrap();
        let tables: Vec<String> = db
            .call(|conn| {
                let mut stmt = conn
                    .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
                let rows = stmt
                    .query_map([], |row| row.get(0))?
                    .filter_map(std::result::Result::ok)
                    .collect();
                Ok(rows)
            })
            .await
            .unwrap();

        assert!(tables.contains(&"markets".to_string()));
        assert!(tables.contains(&"wallets".to_string()));
        assert!(tables.contains(&"trades_raw".to_string()));
        assert!(tables.contains(&"paper_trades".to_string()));
    }

    #[tokio::test]
    async fn test_async_db_is_clone_and_send() {
        let db = AsyncDb::open(":memory:").await.unwrap();
        let db2 = db.clone();

        // Write from one clone
        db.call(|conn| {
            conn.execute(
                "INSERT INTO markets (condition_id, title) VALUES ('0xabc', 'Test Market')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Read from the other clone — same underlying connection
        let title: String = db2
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT title FROM markets WHERE condition_id = '0xabc'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();

        assert_eq!(title, "Test Market");
    }

    #[tokio::test]
    async fn test_async_db_call_returns_error_on_bad_sql() {
        let db = AsyncDb::open(":memory:").await.unwrap();
        let result: Result<()> = db
            .call(|conn| {
                conn.execute("INVALID SQL", [])?;
                Ok(())
            })
            .await;

        assert!(result.is_err());
    }

    #[test]
    fn test_copy_fidelity_events_table_exists() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let tables: Vec<String> = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();

        assert!(tables.contains(&"copy_fidelity_events".to_string()));
        assert!(tables.contains(&"follower_slippage".to_string()));
    }

    #[test]
    fn test_copy_fidelity_events_schema() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn
            .execute(
                "INSERT INTO copy_fidelity_events (proxy_wallet, condition_id, their_trade_id, outcome, outcome_detail)
                 VALUES ('0xabc', '0xdef', 1, 'COPIED', 'paper_trade_id=5')",
                [],
            )
            .unwrap();

        db.conn
            .execute(
                "INSERT INTO copy_fidelity_events (proxy_wallet, condition_id, their_trade_id, outcome, outcome_detail)
                 VALUES ('0xabc', '0xdef', 2, 'SKIPPED_PORTFOLIO_RISK', 'exposure=16.2%, limit=15.0%')",
                [],
            )
            .unwrap();

        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM copy_fidelity_events WHERE proxy_wallet = '0xabc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_follower_slippage_schema() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn
            .execute(
                "INSERT INTO follower_slippage (proxy_wallet, condition_id, their_entry_price, our_entry_price, slippage_cents, fee_applied)
                 VALUES ('0xabc', '0xdef', 0.55, 0.56, 1.0, 0.008)",
                [],
            )
            .unwrap();
    }
}
