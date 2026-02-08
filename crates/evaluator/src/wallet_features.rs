use anyhow::Result;
use rusqlite::Connection;

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
    pub sharpe_ratio: f64,
}

#[allow(dead_code)] // Wired into scheduler in Task 21
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

    // Total PnL from paper trades (if any)
    let total_pnl: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(pnl), 0.0) FROM paper_trades
             WHERE proxy_wallet = ?1 AND status != 'open'
             AND created_at >= datetime(?2, 'unixepoch')",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    let weeks = (window_days as f64) / 7.0;
    let trades_per_week = if weeks > 0.0 {
        trade_count as f64 / weeks
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

#[allow(dead_code)] // Wired into scheduler in Task 21
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

    #[test]
    fn test_compute_features_basic() {
        let now = 1_700_000_000i64;
        let day = 86_400i64;
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
        assert!(features.win_count >= 1); // at least the first SELL at 0.70 > 0.5
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
            avg_position_size: 100.0,
            unique_markets: 5,
            avg_hold_time_hours: 24.0,
            max_drawdown_pct: 8.0,
            trades_per_week: 2.5,
            sharpe_ratio: 1.2,
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
    }
}
