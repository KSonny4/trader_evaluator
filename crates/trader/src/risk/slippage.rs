use crate::db::TraderDb;
use anyhow::Result;
use std::sync::Arc;

/// Log follower slippage for a trade (Strategy Bible §7.3).
#[allow(clippy::too_many_arguments)]
pub async fn log_slippage(
    db: &Arc<TraderDb>,
    proxy_wallet: &str,
    condition_id: &str,
    their_entry_price: f64,
    our_entry_price: f64,
    fee_applied: f64,
    their_trade_hash: &str,
    our_trade_id: i64,
    detection_delay_ms: i64,
) -> Result<()> {
    let slippage_cents = (our_entry_price - their_entry_price).abs() * 100.0;
    let addr = proxy_wallet.to_string();
    let cid = condition_id.to_string();
    let hash = their_trade_hash.to_string();
    let now = chrono::Utc::now().to_rfc3339();

    db.call(move |conn| {
        conn.execute(
            "INSERT INTO follower_slippage_log (proxy_wallet, condition_id, their_entry_price, our_entry_price, slippage_cents, fee_applied, their_trade_hash, our_trade_id, detection_delay_ms, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![addr, cid, their_entry_price, our_entry_price, slippage_cents, fee_applied, hash, our_trade_id, detection_delay_ms, now],
        )?;
        Ok(())
    })
    .await?;

    Ok(())
}

/// Get average slippage in cents for a wallet (recent N trades).
pub async fn get_avg_slippage_cents(
    db: &Arc<TraderDb>,
    proxy_wallet: &str,
    recent_n: u32,
) -> Result<f64> {
    let addr = proxy_wallet.to_string();

    let avg = db
        .call(move |conn| {
            conn.query_row(
                "SELECT COALESCE(AVG(slippage_cents), 0.0)
                 FROM (SELECT slippage_cents FROM follower_slippage_log
                       WHERE proxy_wallet = ?1
                       ORDER BY created_at DESC LIMIT ?2)",
                rusqlite::params![addr, recent_n],
                |row| row.get(0),
            )
        })
        .await?;

    Ok(avg)
}

/// Check if slippage KILL trigger should fire (Strategy Bible §7.3).
/// Returns true if avg slippage exceeds threshold.
#[allow(dead_code)] // Used in tests; risk check uses get_avg_slippage_cents directly
pub async fn should_kill_for_slippage(
    db: &Arc<TraderDb>,
    proxy_wallet: &str,
    threshold_cents: f64,
    window: u32,
) -> Result<bool> {
    let avg = get_avg_slippage_cents(db, proxy_wallet, window).await?;
    Ok(avg > threshold_cents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_log_and_query_slippage() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        // Need a followed wallet and trade for FK
        db.call(|conn| {
            conn.execute(
                "INSERT INTO followed_wallets (proxy_wallet, status, trading_mode, added_at, updated_at)
                 VALUES ('0xtest', 'active', 'paper', '2026-01-01', '2026-01-01')",
                [],
            )?;
            conn.execute(
                "INSERT INTO trader_trades (proxy_wallet, condition_id, side, their_price, their_size_usd, their_trade_hash, their_timestamp, our_size_usd, our_entry_price, sizing_method, status, created_at)
                 VALUES ('0xtest', 'cond-1', 'BUY', 0.50, 100.0, 'hash-1', 1700000000, 25.0, 0.51, 'proportional', 'open', '2026-01-01')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        log_slippage(&db, "0xtest", "cond-1", 0.50, 0.51, 0.003, "hash-1", 1, 500)
            .await
            .unwrap();

        let avg = get_avg_slippage_cents(&db, "0xtest", 10).await.unwrap();
        assert!((avg - 1.0).abs() < 0.01); // 0.01 * 100 = 1 cent
    }

    #[tokio::test]
    async fn test_slippage_kill_trigger() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        db.call(|conn| {
            conn.execute(
                "INSERT INTO followed_wallets (proxy_wallet, status, trading_mode, added_at, updated_at)
                 VALUES ('0xtest', 'active', 'paper', '2026-01-01', '2026-01-01')",
                [],
            )?;
            conn.execute(
                "INSERT INTO trader_trades (id, proxy_wallet, condition_id, side, their_price, their_size_usd, their_trade_hash, their_timestamp, our_size_usd, our_entry_price, sizing_method, status, created_at)
                 VALUES (1, '0xtest', 'cond-1', 'BUY', 0.50, 100.0, 'hash-ref', 1700000000, 25.0, 0.55, 'fixed', 'open', '2026-01-01')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Log several high-slippage trades
        for i in 0..5 {
            let db = Arc::clone(&db);
            log_slippage(
                &db,
                "0xtest",
                "cond-1",
                0.50,
                0.55,
                0.0,
                &format!("hash-{i}"),
                1,
                1000,
            )
            .await
            .unwrap();
        }

        // Average slippage = 5 cents, threshold = 3 cents
        let kill = should_kill_for_slippage(&db, "0xtest", 3.0, 10)
            .await
            .unwrap();
        assert!(kill);

        // With higher threshold, no kill
        let no_kill = should_kill_for_slippage(&db, "0xtest", 10.0, 10)
            .await
            .unwrap();
        assert!(!no_kill);
    }

    #[tokio::test]
    async fn test_slippage_empty_wallet() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let avg = get_avg_slippage_cents(&db, "0xnone", 10).await.unwrap();
        assert!(avg.abs() < f64::EPSILON);
    }
}
