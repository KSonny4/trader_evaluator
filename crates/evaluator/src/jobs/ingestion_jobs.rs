use anyhow::Result;
use common::db::AsyncDb;

use super::fetcher_traits::*;
use crate::event_bus::EventBus;
use crate::events::PipelineEvent;

pub async fn run_trades_ingestion_once<P: crate::ingestion::TradesPager + Sync>(
    db: &AsyncDb,
    pager: &P,
    limit: u32,
    wallets_limit: u32,
    event_bus: Option<&EventBus>,
) -> Result<(u64, u64)> {
    // Backfill first: wallets with 0 trades (so persona can evaluate them), then wallets that
    // already have trades. Within each tier, oldest discovered first so we make progress through
    // the backlog and don't starve older wallets. Persona runs on a schedule and reads trades_raw;
    // it doesn't "wait" for ingestion — fill trades_raw by running this job (e.g. hourly).
    let wallets: Vec<String> = db
        .call_named("run_trades_ingestion.wallets_select", move |conn| {
            let mut stmt = conn.prepare(
                "
                SELECT w.proxy_wallet
                FROM wallets w
                WHERE w.is_active = 1
                ORDER BY
                  CASE WHEN (SELECT COUNT(*) FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) = 0 THEN 0 ELSE 1 END,
                  w.discovered_at ASC
                LIMIT ?1
                ",
            )?;
            let rows = stmt
                .query_map([i64::from(wallets_limit)], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;

    let total = wallets.len();
    if total > 0 {
        tracing::info!(wallets = total, first = %wallets[0], "trades_ingestion: processing wallets");
    }
    let mut pages = 0_u64;
    let mut inserted = 0_u64;
    for (i, w) in wallets.iter().enumerate() {
        match crate::ingestion::ingest_trades_for_wallet(db, pager, w, limit).await {
            Ok((p, ins)) => {
                pages += p;
                inserted += ins;
                if (i + 1) % 100 == 0 || i == 0 {
                    tracing::info!(n = i + 1, total, inserted, "trades_ingestion: progress");
                }
                if let Some(bus) = event_bus {
                    let _ = bus.publish_pipeline(PipelineEvent::TradesIngested {
                        wallet_address: w.clone(),
                        trades_count: ins,
                        ingested_at: chrono::Utc::now(),
                    });
                }
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

    // Persist last-run stats for dashboard "async funnel".
    let wallets_count = total as i64;
    let trades_count = inserted as i64;
    let _ = db
        .call_named("run_trades_ingestion.persist_last_run", move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO discovery_scheduler_state (key, value_int, updated_at) VALUES ('last_run_trades_wallets', ?1, datetime('now'))",
                [wallets_count],
            )?;
            conn.execute(
                "INSERT OR REPLACE INTO discovery_scheduler_state (key, value_int, updated_at) VALUES ('last_run_trades_inserted', ?1, datetime('now'))",
                [trades_count],
            )?;
            Ok(())
        })
        .await;

    Ok((pages, inserted))
}

pub async fn run_activity_ingestion_once<P: ActivityPager + Sync>(
    db: &AsyncDb,
    pager: &P,
    limit: u32,
    wallets_limit: u32,
) -> Result<u64> {
    // Same as trades: wallets with recent trades first; then no trades or too old (re)download.
    let wallets: Vec<String> = db
        .call_named("run_activity_ingestion.wallets_select", move |conn| {
            let mut stmt = conn.prepare(
                "
                SELECT w.proxy_wallet
                FROM wallets w
                WHERE w.is_active = 1
                ORDER BY
                  CASE
                    WHEN (SELECT COUNT(*) FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) > 0
                     AND (SELECT CAST((julianday('now') - julianday(datetime(MAX(tr.timestamp), 'unixepoch'))) AS INTEGER)
                          FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) <= 30
                    THEN 0
                    ELSE 1
                  END,
                  w.discovered_at DESC
                LIMIT ?1
                ",
            )?;
            let rows = stmt
                .query_map([i64::from(wallets_limit)], |row| row.get::<_, String>(0))?
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
            .call_named("run_activity_ingestion.insert_page", move |conn| {
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
    wallets_limit: u32,
) -> Result<u64> {
    let wallets: Vec<String> = db
        .call_named("run_positions_snapshot.wallets_select", move |conn| {
            let mut stmt = conn.prepare(
                "
                SELECT proxy_wallet
                FROM wallets
                WHERE is_active = 1
                ORDER BY discovered_at DESC
                LIMIT ?1
                ",
            )?;
            let rows = stmt
                .query_map([i64::from(wallets_limit)], |row| row.get::<_, String>(0))?
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
            .call_named("run_positions_snapshot.insert_page", move |conn| {
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
        .call_named("run_holders_snapshot.markets_select", |conn| {
            let mut stmt = conn.prepare(
                "
                SELECT condition_id
                FROM market_scores
                WHERE score_date = (SELECT MAX(score_date) FROM market_scores)
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
            .call_named("run_holders_snapshot.insert_page", move |conn| {
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
    use crate::event_bus::EventBus;
    use crate::events::PipelineEvent;
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
        let (_pages, inserted) = run_trades_ingestion_once(&db, &pager, 100, 500, None)
            .await
            .unwrap();
        assert_eq!(inserted, 1);
    }

    /// Pager that returns a unique trade per wallet (so inserts succeed for each wallet).
    struct PerWalletPager;
    impl crate::ingestion::TradesPager for PerWalletPager {
        fn trades_url(&self, user: &str, limit: u32, offset: u32) -> String {
            format!(
                "https://data-api.polymarket.com/trades?user={user}&limit={limit}&offset={offset}"
            )
        }
        async fn fetch_trades_page(
            &self,
            user: &str,
            _limit: u32,
            offset: u32,
        ) -> Result<(Vec<ApiTrade>, Vec<u8>)> {
            if offset > 0 {
                return Ok((vec![], b"[]".to_vec()));
            }
            // Return a unique trade per wallet (different tx hash per user)
            Ok((
                vec![ApiTrade {
                    proxy_wallet: Some(user.to_string()),
                    condition_id: Some("0xcond".to_string()),
                    transaction_hash: Some(format!("0xtx_{user}")),
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
    async fn test_run_trades_ingestion_emits_events_per_wallet() {
        let db = AsyncDb::open(":memory:").await.unwrap();

        // Insert two wallets so we get two events
        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                rusqlite::params!["0xw"],
            )?;
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'TRADER_RECENT', 1)",
                rusqlite::params!["0xw2"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let bus = EventBus::new(16);
        let mut rx = bus.subscribe_pipeline();

        let pager = PerWalletPager;
        let (_pages, inserted) = run_trades_ingestion_once(&db, &pager, 100, 500, Some(&bus))
            .await
            .unwrap();

        // PerWalletPager returns 1 unique trade per wallet
        assert_eq!(inserted, 2);

        // Collect all emitted events
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Should have emitted one TradesIngested event per wallet
        assert_eq!(events.len(), 2, "expected one event per wallet");
        for event in &events {
            match event {
                PipelineEvent::TradesIngested {
                    wallet_address,
                    trades_count,
                    ..
                } => {
                    assert!(!wallet_address.is_empty());
                    assert_eq!(*trades_count, 1);
                }
                other => panic!("expected TradesIngested, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn test_run_trades_ingestion_no_events_when_bus_is_none() {
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
        // Should work fine without event_bus (backward compatible)
        let (_pages, inserted) = run_trades_ingestion_once(&db, &pager, 100, 500, None)
            .await
            .unwrap();
        assert_eq!(inserted, 1);
    }
}
