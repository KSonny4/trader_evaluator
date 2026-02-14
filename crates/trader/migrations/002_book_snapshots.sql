-- Order book snapshot recording for fillability analysis
-- Records CLOB order book state for 120s after detecting a trade

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
    fillable INTEGER NOT NULL DEFAULT 0,
    available_depth_usd REAL,
    vwap REAL,
    slippage_cents REAL,
    levels_json TEXT,
    snapshot_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_book_snap_condition ON book_snapshots(condition_id, snapshot_at);
CREATE INDEX IF NOT EXISTS idx_book_snap_token ON book_snapshots(token_id, snapshot_at);

CREATE TABLE IF NOT EXISTS fillability_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    condition_id TEXT NOT NULL,
    token_id TEXT NOT NULL,
    trigger_trade_hashes TEXT NOT NULL,
    snapshot_count INTEGER NOT NULL,
    fillable_count INTEGER NOT NULL,
    fill_probability REAL NOT NULL,
    opportunity_window_secs REAL,
    avg_available_depth_usd REAL,
    avg_vwap REAL,
    avg_slippage_cents REAL,
    window_start TEXT NOT NULL,
    window_end TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_fill_result_condition ON fillability_results(condition_id);
CREATE INDEX IF NOT EXISTS idx_fill_result_probability ON fillability_results(fill_probability);
