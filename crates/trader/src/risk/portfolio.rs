use crate::db::TraderDb;
use anyhow::Result;
use serde::Serialize;
use std::sync::Arc;

/// Portfolio summary for API responses.
#[derive(Debug, Serialize)]
pub struct PortfolioSummary {
    pub total_exposure_usd: f64,
    pub daily_pnl: f64,
    pub weekly_pnl: f64,
    pub total_pnl: f64,
    pub open_positions: i64,
    pub is_halted: bool,
}

/// Get portfolio summary from risk state.
pub async fn get_portfolio_summary(db: &Arc<TraderDb>) -> Result<PortfolioSummary> {
    let summary = db
        .call(|conn| {
            match conn.query_row(
                "SELECT total_exposure_usd, daily_pnl, weekly_pnl, current_pnl, open_positions, is_halted
                 FROM risk_state WHERE key = 'portfolio'",
                [],
                |row| {
                    Ok(PortfolioSummary {
                        total_exposure_usd: row.get(0)?,
                        daily_pnl: row.get(1)?,
                        weekly_pnl: row.get(2)?,
                        total_pnl: row.get(3)?,
                        open_positions: row.get(4)?,
                        is_halted: row.get::<_, i64>(5)? != 0,
                    })
                },
            ) {
                Ok(s) => Ok(s),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(PortfolioSummary {
                    total_exposure_usd: 0.0,
                    daily_pnl: 0.0,
                    weekly_pnl: 0.0,
                    total_pnl: 0.0,
                    open_positions: 0,
                    is_halted: false,
                }),
                Err(e) => Err(e),
            }
        })
        .await?;

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_portfolio_summary_empty() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let summary = get_portfolio_summary(&db).await.unwrap();

        assert!((summary.total_exposure_usd).abs() < f64::EPSILON);
        assert_eq!(summary.open_positions, 0);
        assert!(!summary.is_halted);
    }

    #[tokio::test]
    async fn test_portfolio_summary_with_state() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let now = chrono::Utc::now().to_rfc3339();

        db.call(move |conn| {
            conn.execute(
                "INSERT INTO risk_state (key, total_exposure_usd, daily_pnl, weekly_pnl, peak_pnl, current_pnl, open_positions, is_halted, updated_at)
                 VALUES ('portfolio', 100.0, -5.0, 10.0, 20.0, 15.0, 3, 0, ?1)",
                [now],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let summary = get_portfolio_summary(&db).await.unwrap();
        assert!((summary.total_exposure_usd - 100.0).abs() < f64::EPSILON);
        assert!((summary.daily_pnl - -5.0).abs() < f64::EPSILON);
        assert!((summary.total_pnl - 15.0).abs() < f64::EPSILON);
        assert_eq!(summary.open_positions, 3);
    }
}
