use anyhow::Result;
use rusqlite::Connection;

pub struct Database {
    pub conn: Connection,
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

CREATE TABLE IF NOT EXISTS market_scores_daily (
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

CREATE TABLE IF NOT EXISTS wallet_personas (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    persona TEXT NOT NULL,             -- Informed Specialist, Consistent Generalist, etc.
    confidence REAL NOT NULL,          -- 0.0 to 1.0
    feature_values_json TEXT,          -- JSON: trade_count, win_rate, unique_markets, etc.
    classified_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(proxy_wallet, classified_at)
);

CREATE TABLE IF NOT EXISTS wallet_exclusions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    reason TEXT NOT NULL,              -- e.g. "tail_risk_seller", "noise_trader", "too_young"
    metric_value REAL,                 -- the actual value that triggered exclusion
    threshold REAL,                    -- the threshold it was compared against
    excluded_at TEXT NOT NULL DEFAULT (datetime('now'))
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
CREATE INDEX IF NOT EXISTS idx_wallet_personas_wallet ON wallet_personas(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_wallet_exclusions_wallet ON wallet_exclusions(proxy_wallet);
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
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"markets".to_string()));
        assert!(tables.contains(&"wallets".to_string()));
        assert!(tables.contains(&"raw_api_responses".to_string()));
        assert!(tables.contains(&"trades_raw".to_string()));
        assert!(tables.contains(&"activity_raw".to_string()));
        assert!(tables.contains(&"positions_snapshots".to_string()));
        assert!(tables.contains(&"holders_snapshots".to_string()));
        assert!(tables.contains(&"market_scores_daily".to_string()));
        assert!(tables.contains(&"wallet_features_daily".to_string()));
        assert!(tables.contains(&"paper_trades".to_string()));
        assert!(tables.contains(&"paper_positions".to_string()));
        assert!(tables.contains(&"wallet_scores_daily".to_string()));
        assert!(tables.contains(&"wallet_personas".to_string()));
        assert!(tables.contains(&"wallet_exclusions".to_string()));
    }

    #[test]
    fn test_migrations_idempotent() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        db.run_migrations().unwrap(); // second call must not fail
    }
}
