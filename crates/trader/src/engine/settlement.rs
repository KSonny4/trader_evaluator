use crate::db::TraderDb;
use anyhow::Result;
use std::sync::Arc;
use tracing::info;

/// Settle all open trades for a market when it resolves.
/// settle_price: 1.0 = outcome won, 0.0 = outcome lost.
pub async fn settle_market(
    db: &Arc<TraderDb>,
    condition_id: &str,
    settle_price: f64,
) -> Result<usize> {
    let cid = condition_id.to_string();

    let settled_count = db
        .call(move |conn| settle_market_sync(conn, &cid, settle_price))
        .await?;

    info!(
        condition_id = condition_id,
        settle_price = settle_price,
        settled = settled_count,
        "market settled"
    );

    Ok(settled_count)
}

fn settle_market_sync(
    conn: &mut rusqlite::Connection,
    condition_id: &str,
    settle_price: f64,
) -> Result<usize, rusqlite::Error> {
    let now = chrono::Utc::now().to_rfc3339();

    // Get all open trades for this market
    let mut stmt = conn.prepare(
        "SELECT id, side, our_entry_price, our_size_usd FROM trader_trades
         WHERE condition_id = ?1 AND status = 'open'",
    )?;

    let trades: Vec<(i64, String, f64, f64)> = stmt
        .query_map([condition_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut count = 0;
    for (id, side, entry_price, size_usd) in &trades {
        let pnl = match side.as_str() {
            "BUY" => (settle_price - entry_price) * size_usd,
            "SELL" => (entry_price - settle_price) * size_usd,
            _ => 0.0,
        };

        let status = if pnl >= 0.0 {
            "settled_win"
        } else {
            "settled_loss"
        };

        conn.execute(
            "UPDATE trader_trades SET status = ?1, exit_price = ?2, pnl = ?3, settled_at = ?4
             WHERE id = ?5",
            rusqlite::params![status, settle_price, pnl, now, id],
        )?;
        count += 1;
    }

    // Remove settled positions
    conn.execute(
        "DELETE FROM trader_positions WHERE condition_id = ?1",
        [condition_id],
    )?;

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db_with_trades() -> Arc<TraderDb> {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        db.call(|conn| {
            // Insert followed wallet
            conn.execute(
                "INSERT INTO followed_wallets (proxy_wallet, status, trading_mode, added_at, updated_at)
                 VALUES ('0xtest', 'active', 'paper', '2026-01-01', '2026-01-01')",
                [],
            )?;

            // Insert open BUY trade at 0.40 for $25
            conn.execute(
                "INSERT INTO trader_trades (proxy_wallet, condition_id, side, their_price, their_size_usd, their_trade_hash, their_timestamp, our_size_usd, our_entry_price, sizing_method, status, created_at)
                 VALUES ('0xtest', 'cond-1', 'BUY', 0.40, 100.0, 'h1', 1700000000, 25.0, 0.40, 'fixed', 'open', '2026-01-01')",
                [],
            )?;

            // Insert open SELL trade at 0.60 for $20
            conn.execute(
                "INSERT INTO trader_trades (proxy_wallet, condition_id, side, their_price, their_size_usd, their_trade_hash, their_timestamp, our_size_usd, our_entry_price, sizing_method, status, created_at)
                 VALUES ('0xtest', 'cond-1', 'SELL', 0.60, 80.0, 'h2', 1700000000, 20.0, 0.60, 'fixed', 'open', '2026-01-01')",
                [],
            )?;

            // Insert position
            conn.execute(
                "INSERT INTO trader_positions (proxy_wallet, condition_id, side, total_size_usd, avg_entry_price, share_count, last_updated_at)
                 VALUES ('0xtest', 'cond-1', 'BUY', 25.0, 0.40, 62.5, '2026-01-01')",
                [],
            )?;

            Ok(())
        })
        .await
        .unwrap();

        db
    }

    #[tokio::test]
    async fn test_settle_market_outcome_won() {
        let db = setup_db_with_trades().await;

        let count = settle_market(&db, "cond-1", 1.0).await.unwrap();
        assert_eq!(count, 2);

        // BUY at 0.40, settle at 1.0: PnL = (1.0 - 0.40) * 25.0 = 15.0
        let (status, pnl): (String, f64) = db
            .call(|conn| {
                conn.query_row(
                    "SELECT status, pnl FROM trader_trades WHERE their_trade_hash = 'h1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .await
            .unwrap();
        assert_eq!(status, "settled_win");
        assert!((pnl - 15.0).abs() < f64::EPSILON);

        // SELL at 0.60, settle at 1.0: PnL = (0.60 - 1.0) * 20.0 = -8.0
        let (status2, pnl2): (String, f64) = db
            .call(|conn| {
                conn.query_row(
                    "SELECT status, pnl FROM trader_trades WHERE their_trade_hash = 'h2'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .await
            .unwrap();
        assert_eq!(status2, "settled_loss");
        assert!((pnl2 - -8.0).abs() < f64::EPSILON);

        // Positions should be deleted
        let pos_count: i64 = db
            .call(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM trader_positions WHERE condition_id = 'cond-1'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(pos_count, 0);
    }

    #[tokio::test]
    async fn test_settle_market_outcome_lost() {
        let db = setup_db_with_trades().await;

        let count = settle_market(&db, "cond-1", 0.0).await.unwrap();
        assert_eq!(count, 2);

        // BUY at 0.40, settle at 0.0: PnL = (0.0 - 0.40) * 25.0 = -10.0
        let pnl: f64 = db
            .call(|conn| {
                conn.query_row(
                    "SELECT pnl FROM trader_trades WHERE their_trade_hash = 'h1'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert!((pnl - -10.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_settle_no_open_trades() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let count = settle_market(&db, "cond-nonexistent", 1.0).await.unwrap();
        assert_eq!(count, 0);
    }
}
