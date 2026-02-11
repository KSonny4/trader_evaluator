use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

#[allow(dead_code)] // Used by tests now, wired into scheduler in Task 21
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
    pub top_category: Option<String>,
    pub top_category_ratio: f64,
}

/// Prefer paper PnL (our copy) when settled paper trades exist; otherwise fallback to positions_snapshots.
fn total_pnl_from_paper_or_positions(conn: &Connection, proxy_wallet: &str, cutoff: i64) -> f64 {
    let paper_pnl: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(pnl), 0.0) FROM paper_trades
             WHERE proxy_wallet = ?1 AND status != 'open'
             AND created_at >= datetime(?2, 'unixepoch')",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    let has_settled_paper_trades: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM paper_trades
             WHERE proxy_wallet = ?1 AND status != 'open'
             AND created_at >= datetime(?2, 'unixepoch'))",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if has_settled_paper_trades {
        paper_pnl
    } else {
        conn.query_row(
            "SELECT COALESCE(SUM(cash_pnl), 0.0) FROM positions_snapshots
             WHERE proxy_wallet = ?1 AND snapshot_at >= datetime(?2, 'unixepoch')
             AND (proxy_wallet, condition_id, snapshot_at) IN (
               SELECT proxy_wallet, condition_id, MAX(snapshot_at)
               FROM positions_snapshots
               WHERE proxy_wallet = ?1 AND snapshot_at >= datetime(?2, 'unixepoch')
               GROUP BY proxy_wallet, condition_id
             )",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0.0)
    }
}

#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn compute_wallet_features(
    conn: &Connection,
    proxy_wallet: &str,
    window_days: u32,
    now_epoch: i64,
) -> Result<WalletFeatures> {
    let cutoff = now_epoch - i64::from(window_days) * 86400;

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

    // Win/loss counting: a "win" is a SELL at price > 0.5 (directional bet won)
    // This is a rough heuristic — proper PnL requires settlement data
    let win_count: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2
             AND side = 'SELL' AND price > 0.5",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let loss_count: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2
             AND side = 'SELL' AND price <= 0.5",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let avg_position_size: f64 = conn
        .query_row(
            "SELECT COALESCE(AVG(size * price), 0.0) FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    let total_pnl = total_pnl_from_paper_or_positions(conn, proxy_wallet, cutoff);

    let weeks = f64::from(window_days) / 7.0;
    let trades_per_week = if weeks > 0.0 {
        f64::from(trade_count) / weeks
    } else {
        0.0
    };
    let trades_per_day = if window_days > 0 {
        f64::from(trade_count) / f64::from(window_days)
    } else {
        0.0
    };

    // Avg hold time: approximate from time between BUY and next SELL in same market
    // For now, default to 0 — will be refined when we have proper position tracking
    let avg_hold_time_hours = 0.0;

    // Max drawdown and Sharpe: require daily return series
    // For now, placeholders — will be computed from paper trades when available
    let max_drawdown_pct = 0.0;
    let sharpe_ratio = 0.0;

    // Active positions: count of currently open paper_positions
    let active_positions: u32 = conn
        .query_row(
            "SELECT COUNT(DISTINCT condition_id) FROM paper_positions
             WHERE proxy_wallet = ?1",
            [proxy_wallet],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // Concentration ratio: % of volume in top 3 markets
    let concentration_ratio: f64 = conn
        .query_row(
            "WITH market_volumes AS (
                SELECT condition_id, SUM(size) as volume
                FROM trades_raw
                WHERE proxy_wallet = ?1 AND timestamp >= ?2
                GROUP BY condition_id
            ),
            total AS (
                SELECT SUM(volume) as total_volume FROM market_volumes
            ),
            top3 AS (
                SELECT SUM(volume) as top3_volume
                FROM (SELECT volume FROM market_volumes ORDER BY volume DESC LIMIT 3)
            )
            SELECT 
                CASE WHEN t.total_volume > 0 THEN CAST(t3.top3_volume AS REAL) / t.total_volume ELSE 0.0 END
            FROM total t, top3 t3",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    let avg_trade_size_usdc: f64 = avg_position_size;

    let (mean_trade, mean_sq_trade): (f64, f64) = conn
        .query_row(
            "SELECT COALESCE(AVG(size * price), 0.0), COALESCE(AVG((size * price) * (size * price)), 0.0)
             FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2",
            rusqlite::params![proxy_wallet, cutoff],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0.0, 0.0));
    let variance = (mean_sq_trade - mean_trade * mean_trade).max(0.0);
    let size_cv = if mean_trade > 0.0 {
        variance.sqrt() / mean_trade
    } else {
        0.0
    };

    let (buy_count, sell_count): (u32, u32) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN side = 'BUY' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN side = 'SELL' THEN 1 ELSE 0 END), 0)
             FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2",
            rusqlite::params![proxy_wallet, cutoff],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0, 0));
    let total_side = buy_count + sell_count;
    let buy_sell_balance = if total_side > 0 {
        let diff = buy_count.abs_diff(sell_count);
        1.0 - (f64::from(diff) / f64::from(total_side))
    } else {
        0.0
    };

    let mid_fill_ratio: f64 = conn
        .query_row(
            "SELECT
                CASE WHEN COUNT(*) > 0
                    THEN CAST(SUM(CASE WHEN ABS(price - 0.5) <= 0.05 THEN 1 ELSE 0 END) AS REAL) / COUNT(*)
                    ELSE 0.0
                END
             FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    let extreme_price_ratio: f64 = conn
        .query_row(
            "SELECT
                CASE WHEN COUNT(*) > 0
                    THEN CAST(SUM(CASE WHEN price >= 0.9 OR price <= 0.1 THEN 1 ELSE 0 END) AS REAL) / COUNT(*)
                    ELSE 0.0
                END
             FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    let ts_rows: Vec<i64> = conn
        .prepare(
            "SELECT timestamp
             FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2
             ORDER BY timestamp ASC",
        )?
        .query_map(rusqlite::params![proxy_wallet, cutoff], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let burstiness_top_1h_ratio = if ts_rows.is_empty() {
        0.0
    } else {
        let mut best = 1usize;
        let mut left = 0usize;
        for right in 0..ts_rows.len() {
            while ts_rows[right] - ts_rows[left] > 3600 {
                left += 1;
            }
            let window = right - left + 1;
            if window > best {
                best = window;
            }
        }
        best as f64 / ts_rows.len() as f64
    };

    let top_category_row: Option<(String, f64)> = conn
        .query_row(
            "
            WITH category_volumes AS (
                SELECT COALESCE(m.category, 'unknown') AS category, SUM(tr.size) AS volume
                FROM trades_raw tr
                LEFT JOIN markets m ON m.condition_id = tr.condition_id
                WHERE tr.proxy_wallet = ?1 AND tr.timestamp >= ?2
                GROUP BY COALESCE(m.category, 'unknown')
            ),
            total AS (
                SELECT COALESCE(SUM(volume), 0.0) AS total_volume FROM category_volumes
            )
            SELECT
                cv.category,
                CASE WHEN t.total_volume > 0 THEN cv.volume / t.total_volume ELSE 0.0 END AS ratio
            FROM category_volumes cv, total t
            ORDER BY cv.volume DESC
            LIMIT 1
            ",
            rusqlite::params![proxy_wallet, cutoff],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (top_category, top_category_ratio) = match top_category_row {
        Some((category, ratio)) => (Some(category), ratio),
        None => (None, 0.0),
    };

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
        top_category,
        top_category_ratio,
    })
}

#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn save_wallet_features(
    conn: &Connection,
    features: &WalletFeatures,
    feature_date: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO wallet_features_daily
         (proxy_wallet, feature_date, window_days, trade_count, win_count, loss_count,
          total_pnl, avg_position_size, unique_markets, avg_hold_time_hours, max_drawdown_pct,
          trades_per_week, trades_per_day, sharpe_ratio, active_positions, concentration_ratio,
          avg_trade_size_usdc, size_cv, buy_sell_balance, mid_fill_ratio, extreme_price_ratio,
          burstiness_top_1h_ratio, top_category, top_category_ratio)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
        rusqlite::params![
            features.proxy_wallet,
            feature_date,
            features.window_days,
            features.trade_count,
            features.win_count,
            features.loss_count,
            features.total_pnl,
            features.avg_position_size,
            features.unique_markets,
            features.avg_hold_time_hours,
            features.max_drawdown_pct,
            features.trades_per_week,
            features.trades_per_day,
            features.sharpe_ratio,
            features.active_positions,
            features.concentration_ratio,
            features.avg_trade_size_usdc,
            features.size_cv,
            features.buy_sell_balance,
            features.mid_fill_ratio,
            features.extreme_price_ratio,
            features.burstiness_top_1h_ratio,
            features.top_category,
            features.top_category_ratio,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::db::Database;

    fn setup_db_with_trades(trades: &[(&str, &str, &str, f64, f64, i64)]) -> Database {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        for (wallet, cid, side, size, price, ts) in trades {
            db.conn
                .execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![wallet, cid, side, size, price, ts],
                )
                .unwrap();
        }
        db
    }

    fn upsert_market_category(db: &Database, condition_id: &str, category: &str) {
        db.conn
            .execute(
                "INSERT OR REPLACE INTO markets (condition_id, title, category) VALUES (?1, ?2, ?3)",
                rusqlite::params![condition_id, format!("Market {condition_id}"), category],
            )
            .unwrap();
    }

    #[test]
    fn test_compute_features_basic() {
        let now = 1_700_000_000i64;
        let day = 86_400i64;
        let trades = vec![
            ("0xabc", "0xm1", "BUY", 25.0, 0.60, now - 5 * day),
            ("0xabc", "0xm1", "SELL", 25.0, 0.70, now - 4 * day),
            ("0xabc", "0xm2", "BUY", 50.0, 0.40, now - 3 * day),
            ("0xabc", "0xm2", "SELL", 50.0, 0.30, now - 2 * day),
        ];
        let db = setup_db_with_trades(&trades);

        let features = compute_wallet_features(&db.conn, "0xabc", 30, now).unwrap();

        assert_eq!(features.trade_count, 4);
        assert_eq!(features.unique_markets, 2);
        assert!(features.win_count >= 1); // at least the first SELL at 0.70 > 0.5
        assert!(features.trades_per_day > 0.0);
        assert!(features.avg_trade_size_usdc > 0.0);
    }

    #[test]
    fn test_compute_features_empty_wallet() {
        let db = setup_db_with_trades(&[]);
        let features =
            compute_wallet_features(&db.conn, "0xnonexistent", 30, 1_700_000_000).unwrap();
        assert_eq!(features.trade_count, 0);
        assert_eq!(features.unique_markets, 0);
    }

    #[test]
    fn test_save_wallet_features() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let features = WalletFeatures {
            proxy_wallet: "0xabc".to_string(),
            window_days: 30,
            trade_count: 10,
            win_count: 7,
            loss_count: 3,
            total_pnl: 150.0,
            avg_position_size: 25.0,
            unique_markets: 5,
            avg_hold_time_hours: 24.0,
            max_drawdown_pct: 8.0,
            trades_per_week: 2.5,
            sharpe_ratio: 1.2,
            active_positions: 3,
            concentration_ratio: 0.75,
            trades_per_day: 0.33,
            avg_trade_size_usdc: 22.5,
            size_cv: 0.2,
            buy_sell_balance: 0.9,
            mid_fill_ratio: 0.3,
            extreme_price_ratio: 0.4,
            burstiness_top_1h_ratio: 0.5,
            top_category: Some("sports".to_string()),
            top_category_ratio: 0.8,
        };

        save_wallet_features(&db.conn, &features, "2026-02-08").unwrap();

        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM wallet_features_daily WHERE proxy_wallet = '0xabc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Verify trades_per_week and sharpe_ratio are persisted (not silently dropped)
        let (tpw, sr, tpd, top_cat): (f64, f64, f64, Option<String>) = db
            .conn
            .query_row(
                "SELECT trades_per_week, sharpe_ratio, trades_per_day, top_category FROM wallet_features_daily WHERE proxy_wallet = '0xabc'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!((tpw - 2.5).abs() < f64::EPSILON);
        assert!((sr - 1.2).abs() < f64::EPSILON);
        assert!((tpd - 0.33).abs() < f64::EPSILON);
        assert_eq!(top_cat.as_deref(), Some("sports"));
    }

    #[test]
    fn test_extreme_price_ratio_computed() {
        let now = 1_700_000_000i64;
        let db = setup_db_with_trades(&[
            ("0xabc", "m1", "BUY", 10.0, 0.99, now - 10),
            ("0xabc", "m1", "SELL", 10.0, 0.98, now - 9),
            ("0xabc", "m2", "BUY", 10.0, 0.50, now - 8),
            ("0xabc", "m2", "SELL", 10.0, 0.52, now - 7),
        ]);
        let f = compute_wallet_features(&db.conn, "0xabc", 30, now).unwrap();
        assert!(f.extreme_price_ratio > 0.4);
    }

    #[test]
    fn test_buy_sell_balance_computed() {
        let now = 1_700_000_000i64;
        let db = setup_db_with_trades(&[
            ("0xabc", "m1", "BUY", 10.0, 0.49, now - 20),
            ("0xabc", "m1", "SELL", 10.0, 0.51, now - 19),
            ("0xabc", "m2", "BUY", 8.0, 0.50, now - 18),
            ("0xabc", "m2", "SELL", 8.0, 0.50, now - 17),
        ]);
        let f = compute_wallet_features(&db.conn, "0xabc", 30, now).unwrap();
        assert!(f.buy_sell_balance >= 0.95);
        assert!(f.mid_fill_ratio >= 0.75);
    }

    #[test]
    fn test_top_category_ratio_computed() {
        let now = 1_700_000_000i64;
        let db = setup_db_with_trades(&[
            ("0xabc", "m_sports", "BUY", 50.0, 0.55, now - 40),
            ("0xabc", "m_sports", "SELL", 50.0, 0.58, now - 39),
            ("0xabc", "m_sports", "BUY", 20.0, 0.60, now - 38),
            ("0xabc", "m_politics", "BUY", 5.0, 0.45, now - 37),
        ]);
        upsert_market_category(&db, "m_sports", "sports");
        upsert_market_category(&db, "m_politics", "politics");

        let f = compute_wallet_features(&db.conn, "0xabc", 30, now).unwrap();
        assert_eq!(f.top_category.as_deref(), Some("sports"));
        assert!(f.top_category_ratio > 0.8);
    }

    #[test]
    fn test_burstiness_and_trades_per_day_computed() {
        let now = 1_700_000_000i64;
        let db = setup_db_with_trades(&[
            ("0xabc", "m1", "BUY", 1.0, 0.5, now - 3_590),
            ("0xabc", "m1", "BUY", 1.0, 0.5, now - 3_000),
            ("0xabc", "m1", "BUY", 1.0, 0.5, now - 2_000),
            ("0xabc", "m1", "BUY", 1.0, 0.5, now - 10_000),
            ("0xabc", "m1", "BUY", 1.0, 0.5, now - 20_000),
        ]);
        let f = compute_wallet_features(&db.conn, "0xabc", 30, now).unwrap();
        assert!(f.burstiness_top_1h_ratio >= 0.5);
        assert!(f.trades_per_day > 0.1);
    }
}
