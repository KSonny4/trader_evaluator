use crate::db::TraderDb;
use crate::types::FidelityOutcome;
use anyhow::Result;
use std::sync::Arc;

/// Log a copy fidelity decision (Strategy Bible §6).
pub async fn log_fidelity(
    db: &Arc<TraderDb>,
    proxy_wallet: &str,
    condition_id: &str,
    their_trade_hash: &str,
    outcome: FidelityOutcome,
    detail: Option<&str>,
) -> Result<()> {
    let addr = proxy_wallet.to_string();
    let cid = condition_id.to_string();
    let hash = their_trade_hash.to_string();
    let outcome_str = outcome.to_string();
    let detail_str = detail.map(str::to_string);
    let now = chrono::Utc::now().to_rfc3339();

    db.call(move |conn| {
        conn.execute(
            "INSERT INTO copy_fidelity_log (proxy_wallet, condition_id, their_trade_hash, outcome, outcome_detail, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![addr, cid, hash, outcome_str, detail_str, now],
        )?;
        Ok(())
    })
    .await?;

    Ok(())
}

/// Calculate copy fidelity percentage for a wallet.
/// fidelity = COPIED / (COPIED + all SKIPPED) * 100
pub async fn get_copy_fidelity_pct(db: &Arc<TraderDb>, proxy_wallet: &str) -> Result<f64> {
    let addr = proxy_wallet.to_string();

    let (copied, total): (i64, i64) = db
        .call(move |conn| {
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM copy_fidelity_log WHERE proxy_wallet = ?1",
                [&addr],
                |row| row.get(0),
            )?;
            let copied: i64 = conn.query_row(
                "SELECT COUNT(*) FROM copy_fidelity_log WHERE proxy_wallet = ?1 AND outcome = 'COPIED'",
                [&addr],
                |row| row.get(0),
            )?;
            Ok((copied, total))
        })
        .await?;

    if total == 0 {
        return Ok(100.0); // No decisions yet = 100% fidelity
    }

    Ok(copied as f64 / total as f64 * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_log_fidelity_copied() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        log_fidelity(
            &db,
            "0xtest",
            "cond-1",
            "hash-1",
            FidelityOutcome::Copied,
            None,
        )
        .await
        .unwrap();

        let count: i64 = db
            .call(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM copy_fidelity_log WHERE outcome = 'COPIED'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_log_fidelity_skipped() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        log_fidelity(
            &db,
            "0xtest",
            "cond-1",
            "hash-1",
            FidelityOutcome::SkippedPortfolioRisk,
            Some("portfolio exposure at 14%"),
        )
        .await
        .unwrap();

        let outcome: String = db
            .call(|conn| {
                conn.query_row(
                    "SELECT outcome FROM copy_fidelity_log WHERE proxy_wallet = '0xtest'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(outcome, "SKIPPED_PORTFOLIO_RISK");
    }

    #[tokio::test]
    async fn test_fidelity_pct_no_decisions() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let pct = get_copy_fidelity_pct(&db, "0xtest").await.unwrap();
        assert!((pct - 100.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_fidelity_pct_mixed() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        // 8 copied, 2 skipped = 80% fidelity
        for i in 0..8 {
            log_fidelity(
                &db,
                "0xtest",
                "cond-1",
                &format!("hash-{i}"),
                FidelityOutcome::Copied,
                None,
            )
            .await
            .unwrap();
        }
        for i in 8..10 {
            log_fidelity(
                &db,
                "0xtest",
                "cond-1",
                &format!("hash-{i}"),
                FidelityOutcome::SkippedWalletRisk,
                None,
            )
            .await
            .unwrap();
        }

        let pct = get_copy_fidelity_pct(&db, "0xtest").await.unwrap();
        assert!((pct - 80.0).abs() < f64::EPSILON);
    }
}
