-- Trader microservice initial schema
-- Fully independent from evaluator.db

CREATE TABLE IF NOT EXISTS followed_wallets (
    proxy_wallet   TEXT PRIMARY KEY,
    label          TEXT,
    status         TEXT NOT NULL DEFAULT 'active',   -- active/paused/killed/removed
    trading_mode   TEXT NOT NULL DEFAULT 'paper',    -- paper/live (per-wallet)
    max_exposure_pct    REAL,
    estimated_bankroll_usd REAL,
    last_trade_seen_at   TEXT,
    last_trade_seen_hash TEXT,
    added_at       TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS trader_trades (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet      TEXT NOT NULL REFERENCES followed_wallets(proxy_wallet),
    condition_id      TEXT NOT NULL,
    side              TEXT NOT NULL,       -- BUY/SELL
    outcome           TEXT,
    outcome_index     INTEGER,
    their_price       REAL NOT NULL,
    their_size_usd    REAL NOT NULL,
    their_trade_hash  TEXT NOT NULL,
    their_timestamp   INTEGER NOT NULL,
    our_size_usd      REAL NOT NULL,
    our_entry_price   REAL NOT NULL,
    slippage_applied  REAL NOT NULL DEFAULT 0.0,
    fee_applied       REAL NOT NULL DEFAULT 0.0,
    sizing_method     TEXT NOT NULL,       -- proportional/fixed
    detection_delay_ms INTEGER NOT NULL DEFAULT 0,
    trading_mode      TEXT NOT NULL DEFAULT 'paper',  -- paper/live
    status            TEXT NOT NULL DEFAULT 'open',   -- open/settled_win/settled_loss
    exit_price        REAL,
    pnl               REAL,
    settled_at        TEXT,
    created_at        TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_trader_trades_wallet ON trader_trades(proxy_wallet);
CREATE INDEX IF NOT EXISTS idx_trader_trades_status ON trader_trades(status);
CREATE INDEX IF NOT EXISTS idx_trader_trades_condition ON trader_trades(condition_id);

CREATE TABLE IF NOT EXISTS trader_positions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet      TEXT NOT NULL REFERENCES followed_wallets(proxy_wallet),
    condition_id      TEXT NOT NULL,
    side              TEXT NOT NULL,
    total_size_usd    REAL NOT NULL DEFAULT 0.0,
    avg_entry_price   REAL NOT NULL DEFAULT 0.0,
    share_count       REAL NOT NULL DEFAULT 0.0,
    unrealized_pnl    REAL NOT NULL DEFAULT 0.0,
    last_updated_at   TEXT NOT NULL,
    UNIQUE(proxy_wallet, condition_id, side)
);

CREATE TABLE IF NOT EXISTS copy_fidelity_log (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet      TEXT NOT NULL,
    condition_id      TEXT NOT NULL,
    their_trade_hash  TEXT NOT NULL,
    outcome           TEXT NOT NULL,       -- COPIED/SKIPPED_PORTFOLIO_RISK/SKIPPED_WALLET_RISK/...
    outcome_detail    TEXT,
    created_at        TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_fidelity_wallet ON copy_fidelity_log(proxy_wallet);

CREATE TABLE IF NOT EXISTS follower_slippage_log (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet      TEXT NOT NULL,
    condition_id      TEXT NOT NULL,
    their_entry_price REAL NOT NULL,
    our_entry_price   REAL NOT NULL,
    slippage_cents    REAL NOT NULL,
    fee_applied       REAL NOT NULL DEFAULT 0.0,
    their_trade_hash  TEXT NOT NULL,
    our_trade_id      INTEGER REFERENCES trader_trades(id),
    detection_delay_ms INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_slippage_wallet ON follower_slippage_log(proxy_wallet);

CREATE TABLE IF NOT EXISTS risk_state (
    key               TEXT PRIMARY KEY,    -- 'portfolio' or wallet address
    total_exposure_usd REAL NOT NULL DEFAULT 0.0,
    daily_pnl         REAL NOT NULL DEFAULT 0.0,
    weekly_pnl        REAL NOT NULL DEFAULT 0.0,
    peak_pnl          REAL NOT NULL DEFAULT 0.0,
    current_pnl       REAL NOT NULL DEFAULT 0.0,
    open_positions    INTEGER NOT NULL DEFAULT 0,
    is_halted         INTEGER NOT NULL DEFAULT 0,
    halt_reason       TEXT,
    halt_until        TEXT,
    updated_at        TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS trade_events (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type        TEXT NOT NULL,
    proxy_wallet      TEXT,
    details_json      TEXT,
    created_at        TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_trade_events_type ON trade_events(event_type);
CREATE INDEX IF NOT EXISTS idx_trade_events_wallet ON trade_events(proxy_wallet);
