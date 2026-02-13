use crate::db::TraderDb;
use anyhow::Result;
use std::sync::Arc;

/// Update per-wallet risk state after a trade execution.
pub async fn update_wallet_risk_state(
    db: &Arc<TraderDb>,
    wallet: &str,
    trade_size_usd: f64,
    pnl_delta: f64,
) -> Result<()> {
    let addr = wallet.to_string();
    let now = chrono::Utc::now().to_rfc3339();

    db.call(move |conn| {
        // Upsert risk state for this wallet
        conn.execute(
            "INSERT INTO risk_state (key, total_exposure_usd, daily_pnl, weekly_pnl, peak_pnl, current_pnl, open_positions, updated_at)
             VALUES (?1, ?2, ?3, ?4, 0.0, ?5, 1, ?6)
             ON CONFLICT(key) DO UPDATE SET
                total_exposure_usd = total_exposure_usd + ?2,
                daily_pnl = daily_pnl + ?3,
                weekly_pnl = weekly_pnl + ?4,
                current_pnl = current_pnl + ?5,
                peak_pnl = MAX(peak_pnl, current_pnl + ?5),
                open_positions = (SELECT COUNT(*) FROM trader_positions WHERE proxy_wallet = ?1),
                updated_at = ?6",
            rusqlite::params![addr, trade_size_usd, pnl_delta, pnl_delta, pnl_delta, now],
        )?;
        Ok(())
    })
    .await?;

    Ok(())
}

/// Update portfolio-level risk state.
pub async fn update_portfolio_risk_state(
    db: &Arc<TraderDb>,
    trade_size_usd: f64,
    pnl_delta: f64,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    db.call(move |conn| {
        conn.execute(
            "INSERT INTO risk_state (key, total_exposure_usd, daily_pnl, weekly_pnl, peak_pnl, current_pnl, open_positions, updated_at)
             VALUES ('portfolio', ?1, ?2, ?3, 0.0, ?4, 1, ?5)
             ON CONFLICT(key) DO UPDATE SET
                total_exposure_usd = total_exposure_usd + ?1,
                daily_pnl = daily_pnl + ?2,
                weekly_pnl = weekly_pnl + ?3,
                current_pnl = current_pnl + ?4,
                peak_pnl = MAX(peak_pnl, current_pnl + ?4),
                open_positions = (SELECT COUNT(*) FROM trader_positions),
                updated_at = ?5",
            rusqlite::params![trade_size_usd, pnl_delta, pnl_delta, pnl_delta, now],
        )?;
        Ok(())
    })
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_update_wallet_risk_state() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        update_wallet_risk_state(&db, "0xtest", 25.0, 0.0)
            .await
            .unwrap();

        let exposure: f64 = db
            .call(|conn| {
                conn.query_row(
                    "SELECT total_exposure_usd FROM risk_state WHERE key = '0xtest'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert!((exposure - 25.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_update_portfolio_risk_state() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        update_portfolio_risk_state(&db, 25.0, -2.0).await.unwrap();
        update_portfolio_risk_state(&db, 30.0, 5.0).await.unwrap();

        let (exposure, pnl): (f64, f64) = db
            .call(|conn| {
                conn.query_row(
                    "SELECT total_exposure_usd, daily_pnl FROM risk_state WHERE key = 'portfolio'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .await
            .unwrap();

        assert!((exposure - 55.0).abs() < f64::EPSILON);
        assert!((pnl - 3.0).abs() < f64::EPSILON);
    }
}
