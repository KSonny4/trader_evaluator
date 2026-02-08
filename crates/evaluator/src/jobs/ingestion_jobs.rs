use anyhow::Result;
use common::db::AsyncDb;

use super::fetcher_traits::*;

pub async fn run_trades_ingestion_once<P: crate::ingestion::TradesPager + Sync>(
    db: &AsyncDb,
    pager: &P,
    limit: u32,
) -> Result<(u64, u64)> {
    let wallets: Vec<String> = db
        .call(|conn| {
            let mut stmt = conn.prepare(
                "
                SELECT proxy_wallet
                FROM wallets
                WHERE is_active = 1
                ORDER BY discovered_at DESC
                LIMIT 500
                ",
            )?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;

    let mut pages = 0_u64;
    let mut inserted = 0_u64;
    for w in wallets {
        match crate::ingestion::ingest_trades_for_wallet(db, pager, &w, limit).await {
            Ok((p, ins)) => {
                pages += p;
                inserted += ins;
            }
            Err(e) => {
                tracing::warn!(
                    wallet = %w,
                    error = %e,
                    "trades ingestion failed for wallet; continuing to next"
                );
            }
        }
    }
    metrics::counter!("evaluator_trades_ingested_total").increment(inserted);
    Ok((pages, inserted))
}

pub async fn run_activity_ingestion_once<P: ActivityPager + Sync>(
    db: &AsyncDb,
    pager: &P,
    limit: u32,
) -> Result<u64> {
    let wallets: Vec<String> = db
        .call(|conn| {
            let mut stmt = conn.prepare(
                "
                SELECT proxy_wallet
                FROM wallets
                WHERE is_active = 1
                ORDER BY discovered_at DESC
                LIMIT 500
                ",
            )?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;

    let mut inserted = 0_u64;
    for w in wallets {
        let fetch_result = pager.fetch_activity_page(&w, limit, 0).await;
        let (events, _raw) = match fetch_result {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    wallet = %w,
                    error = %e,
                    "activity ingestion failed for wallet; continuing to next"
                );
                continue;
            }
        };

        let page_inserted = db
            .call(move |conn| {
                let tx = conn.transaction()?;

                let mut ins = 0_u64;
                for e in events {
                    let proxy_wallet = match e.proxy_wallet.as_deref() {
                        Some(v) if !v.is_empty() => v.to_string(),
                        _ => continue,
                    };
                    let activity_type = match e.activity_type.as_deref() {
                        Some(v) if !v.is_empty() => v.to_string(),
                        _ => continue,
                    };
                    let timestamp = e.timestamp.unwrap_or(0);
                    let raw_json = serde_json::to_string(&e).unwrap_or_default();
                    let changed = tx.execute(
                        "
                        INSERT OR IGNORE INTO activity_raw
                            (proxy_wallet, condition_id, activity_type, size, usdc_size, price, side, outcome, outcome_index, timestamp, transaction_hash, raw_json)
                        VALUES
                            (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                        ",
                        rusqlite::params![
                            proxy_wallet,
                            e.condition_id,
                            activity_type,
                            e.size.and_then(|s| s.parse::<f64>().ok()),
                            e.usdc_size.and_then(|s| s.parse::<f64>().ok()),
                            e.price.and_then(|s| s.parse::<f64>().ok()),
                            e.side,
                            e.outcome,
                            e.outcome_index,
                            timestamp,
                            e.transaction_hash,
                            raw_json,
                        ],
                    )?;
                    ins += changed as u64;
                }
                tx.commit()?;
                Ok(ins)
            })
            .await?;

        inserted += page_inserted;
    }

    Ok(inserted)
}

pub async fn run_positions_snapshot_once<P: PositionsPager + Sync>(
    db: &AsyncDb,
    pager: &P,
    limit: u32,
) -> Result<u64> {
    let wallets: Vec<String> = db
        .call(|conn| {
            let mut stmt = conn.prepare(
                "
                SELECT proxy_wallet
                FROM wallets
                WHERE is_active = 1
                ORDER BY discovered_at DESC
                LIMIT 500
                ",
            )?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;

    let mut inserted = 0_u64;
    for w in wallets {
        let fetch_result = pager.fetch_positions_page(&w, limit, 0).await;
        let (positions, _raw) = match fetch_result {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    wallet = %w,
                    error = %e,
                    "positions snapshot failed for wallet; continuing to next"
                );
                continue;
            }
        };

        let page_inserted = db
            .call(move |conn| {
                let tx = conn.transaction()?;

                let mut ins = 0_u64;
                for p in positions {
                    let proxy_wallet = match p.proxy_wallet.as_deref() {
                        Some(v) if !v.is_empty() => v.to_string(),
                        _ => continue,
                    };
                    let condition_id = match p.condition_id.as_deref() {
                        Some(v) if !v.is_empty() => v.to_string(),
                        _ => continue,
                    };
                    let Some(size) = p.size.as_deref().and_then(|s| s.parse::<f64>().ok()) else {
                        continue;
                    };
                    let raw_json = serde_json::to_string(&p).unwrap_or_default();
                    let changed = tx.execute(
                        "
                        INSERT INTO positions_snapshots
                            (proxy_wallet, condition_id, asset, size, avg_price, current_value, cash_pnl, percent_pnl, outcome, outcome_index, raw_json)
                        VALUES
                            (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                        ",
                        rusqlite::params![
                            proxy_wallet,
                            condition_id,
                            p.asset,
                            size,
                            p.avg_price.and_then(|s| s.parse::<f64>().ok()),
                            p.current_value.and_then(|s| s.parse::<f64>().ok()),
                            p.cash_pnl.and_then(|s| s.parse::<f64>().ok()),
                            p.percent_pnl.and_then(|s| s.parse::<f64>().ok()),
                            p.outcome,
                            p.outcome_index,
                            raw_json,
                        ],
                    )?;
                    ins += changed as u64;
                }
                tx.commit()?;
                Ok(ins)
            })
            .await?;

        inserted += page_inserted;
    }

    Ok(inserted)
}

pub async fn run_holders_snapshot_once<H: HoldersFetcher + Sync>(
    db: &AsyncDb,
    holders: &H,
    per_market: u32,
) -> Result<u64> {
    let markets: Vec<String> = db
        .call(|conn| {
            let mut stmt = conn.prepare(
                "
                SELECT condition_id
                FROM market_scores_daily
                WHERE score_date = date('now')
                ORDER BY rank ASC
                LIMIT 20
                ",
            )?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;

    let mut inserted = 0_u64;
    for condition_id in markets {
        let fetch_result = holders.fetch_holders(&condition_id, per_market).await;
        let (holder_resp, _raw_h) = match fetch_result {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    condition_id = %condition_id,
                    error = %e,
                    "holders snapshot failed for market; continuing to next"
                );
                continue;
            }
        };
        let cid = condition_id.clone();

        let page_inserted = db
            .call(move |conn| {
                let tx = conn.transaction()?;

                let mut ins = 0_u64;
                for r in holder_resp {
                    let token = r.token.clone();
                    for h in r.holders {
                        let Some(proxy_wallet) = h.proxy_wallet else {
                            continue;
                        };
                        let Some(amount) = h.amount else {
                            continue;
                        };
                        let changed = tx.execute(
                            "
                            INSERT INTO holders_snapshots
                                (condition_id, token, proxy_wallet, amount, outcome_index, pseudonym)
                            VALUES
                                (?1, ?2, ?3, ?4, ?5, ?6)
                            ",
                            rusqlite::params![
                                cid,
                                token,
                                proxy_wallet,
                                amount,
                                h.outcome_index,
                                h.pseudonym
                            ],
                        )?;
                        ins += changed as u64;
                    }
                }
                tx.commit()?;
                Ok(ins)
            })
            .await?;

        inserted += page_inserted;
    }

    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::types::ApiTrade;

    struct OnePagePager;
    impl crate::ingestion::TradesPager for OnePagePager {
        fn trades_url(&self, user: &str, limit: u32, offset: u32) -> String {
            format!(
                "https://data-api.polymarket.com/trades?user={user}&limit={limit}&offset={offset}"
            )
        }
        async fn fetch_trades_page(
            &self,
            _user: &str,
            _limit: u32,
            offset: u32,
        ) -> Result<(Vec<ApiTrade>, Vec<u8>)> {
            if offset > 0 {
                return Ok((vec![], b"[]".to_vec()));
            }
            Ok((
                vec![ApiTrade {
                    proxy_wallet: Some("0xw".to_string()),
                    condition_id: Some("0xcond".to_string()),
                    transaction_hash: Some("0xtx1".to_string()),
                    size: Some("1".to_string()),
                    price: Some("0.5".to_string()),
                    timestamp: Some(1),
                    asset: None,
                    title: None,
                    slug: None,
                    outcome: Some("YES".to_string()),
                    outcome_index: Some(0),
                    side: Some("BUY".to_string()),
                    pseudonym: None,
                    name: None,
                }],
                br#"[{"page":1}]"#.to_vec(),
            ))
        }
    }

    #[tokio::test]
    async fn test_run_trades_ingestion_inserts_rows() {
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                rusqlite::params!["0xw"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let pager = OnePagePager;
        let (_pages, inserted) = run_trades_ingestion_once(&db, &pager, 100).await.unwrap();
        assert_eq!(inserted, 1);
    }
}
