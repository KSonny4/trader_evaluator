use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

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
    /// Dominant domain (wallet's lane) — e.g. Sports, Politics, Crypto.
    pub top_domain: Option<String>,
    pub top_domain_ratio: f64,
    /// Number of markets where total FIFO-paired PnL > 0.
    pub profitable_markets: u32,
}

/// Paired round-trip stats: wins, losses, and hold durations (seconds) for each closed position.
struct PairedStats {
    wins: u32,
    losses: u32,
    hold_seconds: Vec<f64>,
    /// (timestamp of close, pnl) for daily series
    closed_pnls: Vec<(i64, f64)>,
    /// Number of markets where total paired PnL > 0
    profitable_markets: u32,
}

/// Pair BUY and SELL trades within each condition_id (FIFO). Compute win/loss from actual PnL,
/// hold time per position, and closed PnLs for drawdown/Sharpe.
type MarketBuysSells = (Vec<(f64, f64, i64)>, Vec<(f64, f64, i64)>);

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
    for (_cid, (buys, sells)) in by_market {
        let n = buys.len().min(sells.len());
        let mut market_pnl = 0.0f64;
        for i in 0..n {
            let (buy_size, buy_price, buy_ts) = buys[i];
            let (sell_size, sell_price, sell_ts) = sells[i];
            let size = buy_size.min(sell_size);
            if size <= 0.0 {
                continue;
            }
            let pnl = (sell_price - buy_price) * size;
            market_pnl += pnl;
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
    }

    Ok(PairedStats {
        wins,
        losses,
        hold_seconds,
        closed_pnls,
        profitable_markets,
    })
}

/// Build daily PnL from (timestamp, pnl) closed positions, then compute max drawdown % and Sharpe ratio.
fn drawdown_and_sharpe_from_daily_pnl(closed_pnls: &[(i64, f64)]) -> Result<(f64, f64)> {
    if closed_pnls.is_empty() {
        return Ok((0.0, 0.0));
    }
    // Group by day (UTC day from timestamp).
    let mut daily: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
    for (ts, pnl) in closed_pnls {
        let day = ts / 86400;
        *daily.entry(day).or_insert(0.0) += *pnl;
    }
    let mut days: Vec<i64> = daily.keys().copied().collect();
    days.sort_unstable();
    let daily_pnl: Vec<f64> = days.iter().map(|d| daily[d]).collect();

    // Equity curve (cumulative PnL).
    let mut equity = Vec::with_capacity(daily_pnl.len());
    let mut cum = 0.0;
    for p in &daily_pnl {
        cum += *p;
        equity.push(cum);
    }

    // Max drawdown: (peak - trough) / peak when peak > 0, as percentage.
    let mut max_drawdown_pct = 0.0f64;
    let mut peak = 0.0f64;
    for &e in &equity {
        if e > peak {
            peak = e;
        }
        if peak > 0.0 {
            let dd = 100.0 * (peak - e) / peak;
            if dd > max_drawdown_pct {
                max_drawdown_pct = dd;
            }
        }
    }

    // Sharpe: daily returns = daily_pnl[i] / equity[i-1], then mean/std annualized.
    if equity.len() < 2 {
        return Ok((max_drawdown_pct, 0.0));
    }
    let mut returns: Vec<f64> = Vec::with_capacity(equity.len() - 1);
    for i in 1..equity.len() {
        let prev = equity[i - 1];
        if prev.abs() > 1e-12 {
            returns.push(daily_pnl[i] / prev);
        } else {
            returns.push(0.0);
        }
    }
    let n = returns.len() as f64;
    if n < 1.0 {
        return Ok((max_drawdown_pct, 0.0));
    }
    let mean_ret: f64 = returns.iter().sum::<f64>() / n;
    let variance = returns.iter().map(|r| (r - mean_ret).powi(2)).sum::<f64>() / n;
    let std_ret = variance.sqrt();
    let sharpe_ratio = if std_ret > 1e-12 {
        // Annualize: multiply by sqrt(252) for daily data.
        (mean_ret / std_ret) * (252.0_f64).sqrt()
    } else {
        0.0
    };

    Ok((max_drawdown_pct, sharpe_ratio))
}

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

    // Win/loss and hold times from actual per-position PnL (BUY-SELL pairing, FIFO per market).
    let paired = paired_trade_stats(conn, proxy_wallet, cutoff)?;
    let win_count = paired.wins;
    let loss_count = paired.losses;

    let avg_position_size: f64 = conn
        .query_row(
            "SELECT COALESCE(AVG(size * price), 0.0) FROM trades_raw
             WHERE proxy_wallet = ?1 AND timestamp >= ?2",
            rusqlite::params![proxy_wallet, cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    let total_pnl: f64 = paired.closed_pnls.iter().map(|(_, pnl)| pnl).sum();

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

    // Avg hold time from paired BUY-SELL round-trips (hours).
    let avg_hold_time_hours = if paired.hold_seconds.is_empty() {
        0.0
    } else {
        let sum: f64 = paired.hold_seconds.iter().sum();
        (sum / paired.hold_seconds.len() as f64) / 3600.0
    };

    // Max drawdown and Sharpe from daily PnL series (built from closed positions).
    let (max_drawdown_pct, sharpe_ratio) = drawdown_and_sharpe_from_daily_pnl(&paired.closed_pnls)?;

    // Active positions: count of markets with size > 0 in latest positions_snapshots
    let active_positions: u32 = conn
        .query_row(
            "SELECT COUNT(DISTINCT condition_id) FROM positions_snapshots
             WHERE proxy_wallet = ?1
             AND (proxy_wallet, condition_id, snapshot_at) IN (
               SELECT proxy_wallet, condition_id, MAX(snapshot_at)
               FROM positions_snapshots WHERE proxy_wallet = ?1
               GROUP BY proxy_wallet, condition_id
             )
             AND size > 0",
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

    // Domain = Polymarket category (Sports, Politics, Crypto). See STRATEGY_BIBLE §Domain hierarchy.
    let top_domain_row: Option<(String, f64)> = conn
        .query_row(
            "
            WITH domain_volumes AS (
                SELECT COALESCE(m.category, 'unknown') AS domain, SUM(tr.size) AS volume
                FROM trades_raw tr
                LEFT JOIN markets m ON m.condition_id = tr.condition_id
                WHERE tr.proxy_wallet = ?1 AND tr.timestamp >= ?2
                GROUP BY COALESCE(m.category, 'unknown')
            ),
            total AS (
                SELECT COALESCE(SUM(volume), 0.0) AS total_volume FROM domain_volumes
            )
            SELECT
                dv.domain,
                CASE WHEN t.total_volume > 0 THEN dv.volume / t.total_volume ELSE 0.0 END AS ratio
            FROM domain_volumes dv, total t
            ORDER BY dv.volume DESC
            LIMIT 1
            ",
            rusqlite::params![proxy_wallet, cutoff],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (top_domain, top_domain_ratio) = match top_domain_row {
        Some((domain, ratio)) => (Some(domain), ratio),
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
        top_domain,
        top_domain_ratio,
        profitable_markets: paired.profitable_markets,
    })
}

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
          burstiness_top_1h_ratio, top_domain, top_domain_ratio, profitable_markets)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
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
            features.top_domain,
            features.top_domain_ratio,
            features.profitable_markets,
        ],
    )?;
    Ok(())
}

/// Compute and save features for a single wallet and window.
///
/// This is a wrapper around the batch feature computation logic,
/// designed for on-demand computation when wallets are first discovered.
///
/// # Errors
/// Returns error if:
/// - Wallet has <5 settled trades (insufficient data)
/// - Database query/insert fails
#[allow(dead_code)] // Used in Task 3 (spawned from discovery)
pub async fn compute_features_for_wallet(
    db: &common::db::AsyncDb,
    _cfg: &common::config::Config,
    proxy_wallet: &str,
    window_days: i64,
) -> anyhow::Result<()> {
    use chrono::Utc;

    let wallet = proxy_wallet.to_string();
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let min_trades = 5_u32;
    let window_days_u32 = window_days as u32;
    let now_epoch = Utc::now().timestamp();

    db.call_named("on_demand_features.compute", move |conn| {
        // Check settled trade count (same gate as daily batch)
        let cutoff_epoch = now_epoch - (window_days * 86400);

        let settled_count: i64 = conn.query_row(
            "
            SELECT COUNT(DISTINCT t1.transaction_hash)
            FROM trades_raw t1
            WHERE t1.proxy_wallet = ?1
              AND t1.timestamp >= ?2
              AND EXISTS (
                  SELECT 1 FROM trades_raw t2
                  WHERE t2.proxy_wallet = t1.proxy_wallet
                    AND t2.condition_id = t1.condition_id
                    AND t2.side != t1.side
                    AND t2.timestamp >= t1.timestamp
                    AND t2.timestamp >= ?2
              )
            ",
            rusqlite::params![&wallet, cutoff_epoch],
            |row| row.get(0),
        )?;

        if settled_count < i64::from(min_trades) {
            return Err(anyhow::anyhow!(
                "insufficient settled trades: {settled_count} < {min_trades}"
            ));
        }

        // Compute features (reuse existing logic)
        let features = compute_wallet_features(conn, &wallet, window_days_u32, now_epoch)?;

        if features.trade_count < min_trades {
            return Err(anyhow::anyhow!(
                "insufficient total trades: {} < {min_trades}",
                features.trade_count
            ));
        }

        // Persist
        save_wallet_features(conn, &features, &today)?;

        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("on-demand feature computation failed: {e}"))
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
            top_domain: Some("sports".to_string()),
            top_domain_ratio: 0.8,
            profitable_markets: 4,
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
                "SELECT trades_per_week, sharpe_ratio, trades_per_day, top_domain FROM wallet_features_daily WHERE proxy_wallet = '0xabc'",
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
    fn test_top_domain_ratio_computed() {
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
        assert_eq!(f.top_domain.as_deref(), Some("sports"));
        assert!(f.top_domain_ratio > 0.8);
    }

    #[test]
    fn test_win_loss_from_actual_pnl_bonder_loses() {
        // Bonder: buy at 0.99, sell at 0.98 => losing trade. Old heuristic (SELL > 0.5) would count as win.
        let now = 1_700_000_000i64;
        let db = setup_db_with_trades(&[
            ("0xbonder", "m1", "BUY", 10.0, 0.99, now - 100),
            ("0xbonder", "m1", "SELL", 10.0, 0.98, now - 99),
            ("0xbonder", "m1", "BUY", 10.0, 0.98, now - 98),
            ("0xbonder", "m1", "SELL", 10.0, 0.97, now - 97),
        ]);
        let f = compute_wallet_features(&db.conn, "0xbonder", 30, now).unwrap();
        assert_eq!(
            f.win_count, 0,
            "bonder selling below buy price should be 0 wins"
        );
        assert_eq!(f.loss_count, 2, "both round-trips lose money");
    }

    #[test]
    fn test_win_loss_from_actual_pnl_real_wins() {
        // Buy low, sell high => wins
        let now = 1_700_000_000i64;
        let db = setup_db_with_trades(&[
            ("0xwinner", "m1", "BUY", 20.0, 0.50, now - 100),
            ("0xwinner", "m1", "SELL", 20.0, 0.70, now - 99),
            ("0xwinner", "m2", "BUY", 10.0, 0.40, now - 98),
            ("0xwinner", "m2", "SELL", 10.0, 0.35, now - 97),
        ]);
        let f = compute_wallet_features(&db.conn, "0xwinner", 30, now).unwrap();
        assert_eq!(f.win_count, 1);
        assert_eq!(f.loss_count, 1);
    }

    #[test]
    fn test_avg_hold_time_hours_from_paired_trades() {
        // One round-trip: BUY at 0, SELL at 7200 sec (2 hours) => avg 2.0 hours
        let now = 1_700_000_000i64;
        let db = setup_db_with_trades(&[
            ("0xhold", "m1", "BUY", 10.0, 0.50, now - 7200),
            ("0xhold", "m1", "SELL", 10.0, 0.55, now),
        ]);
        let f = compute_wallet_features(&db.conn, "0xhold", 30, now).unwrap();
        assert!((f.avg_hold_time_hours - 2.0).abs() < 0.01);
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

    #[tokio::test]
    async fn test_compute_features_for_wallet_success() {
        let cfg =
            common::config::Config::from_toml_str(include_str!("../../../config/default.toml"))
                .unwrap();
        let db = common::db::AsyncDb::open(":memory:").await.unwrap();

        // Use current time so trades fall within the 30-day window
        let now = chrono::Utc::now().timestamp();
        let day = 86400i64;

        // Insert wallet with 5+ settled trades (BUY-SELL pairs)
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xtest', 'HOLDER', 1)",
                [],
            )?;
            // Create 5 settled round-trips (BUY followed by SELL in each condition)
            for i in 0..5 {
                conn.execute(
                    "INSERT INTO trades_raw (transaction_hash, proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES (?1, '0xtest', ?2, 'BUY', 100.0, 0.5, ?3)",
                    rusqlite::params![
                        format!("0xtxbuy{}", i),
                        format!("0xcond{}", i),
                        now - (i + 1) * day
                    ],
                )?;
                conn.execute(
                    "INSERT INTO trades_raw (transaction_hash, proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES (?1, '0xtest', ?2, 'SELL', 100.0, 0.6, ?3)",
                    rusqlite::params![
                        format!("0xtxsell{}", i),
                        format!("0xcond{}", i),
                        now - i * day
                    ],
                )?;
            }
            Ok(())
        }).await.unwrap();

        // Call on-demand feature computation
        let result = compute_features_for_wallet(&db, &cfg, "0xtest", 30).await;
        assert!(
            result.is_ok(),
            "should compute features successfully: {:?}",
            result.err()
        );

        // Verify features row inserted
        let count: i64 = db.call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM wallet_features_daily WHERE proxy_wallet = '0xtest' AND window_days = 30",
                [],
                |row| row.get(0),
            ).map_err(anyhow::Error::from)
        }).await.unwrap();
        assert_eq!(count, 1, "should have 1 feature row");
    }

    #[tokio::test]
    async fn test_compute_features_for_wallet_insufficient_trades() {
        let cfg =
            common::config::Config::from_toml_str(include_str!("../../../config/default.toml"))
                .unwrap();
        let db = common::db::AsyncDb::open(":memory:").await.unwrap();

        // Use current time so trades fall within the 30-day window
        let now = chrono::Utc::now().timestamp();
        let day = 86400i64;

        // Insert wallet with only 2 settled trades (below threshold of 5)
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xfew', 'HOLDER', 1)",
                [],
            )?;
            // Only 2 settled round-trips
            for i in 0..2 {
                conn.execute(
                    "INSERT INTO trades_raw (transaction_hash, proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES (?1, '0xfew', ?2, 'BUY', 100.0, 0.5, ?3)",
                    rusqlite::params![
                        format!("0xtxbuy{}", i),
                        format!("0xcond{}", i),
                        now - (i + 1) * day
                    ],
                )?;
                conn.execute(
                    "INSERT INTO trades_raw (transaction_hash, proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES (?1, '0xfew', ?2, 'SELL', 100.0, 0.6, ?3)",
                    rusqlite::params![
                        format!("0xtxsell{}", i),
                        format!("0xcond{}", i),
                        now - i * day
                    ],
                )?;
            }
            Ok(())
        }).await.unwrap();

        // Call on-demand feature computation
        let result = compute_features_for_wallet(&db, &cfg, "0xfew", 30).await;
        assert!(result.is_err(), "should fail with insufficient trades");
        assert!(
            result.unwrap_err().to_string().contains("insufficient"),
            "error should mention insufficient trades"
        );

        // Verify no features row inserted
        let count: i64 = db
            .call(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM wallet_features_daily WHERE proxy_wallet = '0xfew'",
                    [],
                    |row| row.get(0),
                )
                .map_err(anyhow::Error::from)
            })
            .await
            .unwrap();
        assert_eq!(count, 0, "should have 0 feature rows");
    }
}
