use anyhow::Result;
use common::db::AsyncDb;
use common::types::ApiTrade;

pub trait TradesPager {
    #[allow(dead_code)]
    fn trades_url(&self, user: &str, limit: u32, offset: u32) -> String;

    fn fetch_trades_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> impl std::future::Future<Output = Result<(Vec<ApiTrade>, Vec<u8>)>> + Send;
}

#[allow(dead_code)]
pub async fn ingest_trades_for_wallet<P: TradesPager + Sync>(
    db: &AsyncDb,
    pager: &P,
    user: &str,
    limit: u32,
) -> Result<(u64, u64)> {
    // Query the latest known trade timestamp for this wallet so we can stop
    // pagination early once we reach trades we already have.
    let user_owned = user.to_string();
    let max_known_ts: Option<i64> = db
        .call(move |conn| {
            let ts = conn
                .query_row(
                    "SELECT MAX(timestamp) FROM trades_raw WHERE proxy_wallet = ?1",
                    [&user_owned],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .unwrap_or(None);
            Ok(ts)
        })
        .await?;

    let mut offset = 0;
    let mut pages = 0_u64;
    let mut inserted = 0_u64;

    loop {
        let fetch_result = pager.fetch_trades_page(user, limit, offset).await;
        let (trades, _raw_body) = match fetch_result {
            Ok(v) => v,
            Err(e) => {
                // Treat errors during pagination (e.g., HTTP 400 at high offsets)
                // as "end of data" — return what we collected so far.
                tracing::warn!(
                    user,
                    offset,
                    error = %e,
                    "trades pagination stopped early due to error; returning collected data"
                );
                break;
            }
        };
        let page_len = trades.len();
        pages += 1;

        // Check if ALL trades on this page are at or before our latest known
        // timestamp. If so, we've already ingested everything newer and can stop.
        // The API returns trades in descending order (newest first), so once a
        // full page is "old", there's nothing new beyond it.
        //
        // Safety: verify descending order before relying on the optimisation.
        // If the API ever changes sort order, we fall back to full pagination.
        let is_descending = if trades.len() >= 2 {
            let first_ts = trades.first().map_or(0, |t| t.timestamp.unwrap_or(0));
            let last_ts = trades.last().map_or(0, |t| t.timestamp.unwrap_or(0));
            first_ts >= last_ts
        } else {
            true // single-element or empty page — trivially sorted
        };

        let all_known = if let Some(max_ts) = max_known_ts {
            if !is_descending {
                tracing::warn!(
                    user,
                    "trades API returned non-descending order; skipping early-stop optimisation"
                );
                false
            } else {
                !trades.is_empty() && trades.iter().all(|t| t.timestamp.unwrap_or(0) <= max_ts)
            }
        } else {
            false
        };

        // Batch all DB work for this page into a single db.call() closure
        // wrapped in a transaction for atomicity.
        let page_inserted = db
            .call(move |conn| {
                let tx = conn.transaction()?;

                let mut page_ins = 0_u64;
                for t in trades {
                    let proxy_wallet = match t.proxy_wallet.as_deref() {
                        Some(v) if !v.is_empty() => v.to_string(),
                        _ => continue, // required key missing
                    };
                    let condition_id = match t.condition_id.as_deref() {
                        Some(v) if !v.is_empty() => v.to_string(),
                        _ => continue, // required key missing
                    };
                    let tx_hash = t.transaction_hash.clone();

                    // Persist derived row; rely on UNIQUE constraint to deduplicate.
                    let raw_json = serde_json::to_string(&t).unwrap_or_default();
                    let changed = tx.execute(
                        "
                        INSERT OR IGNORE INTO trades_raw
                            (proxy_wallet, condition_id, asset, side, size, price, outcome, outcome_index, timestamp, transaction_hash, raw_json)
                        VALUES
                            (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                        ",
                        rusqlite::params![
                            proxy_wallet,
                            condition_id,
                            t.asset,
                            t.side,
                            t.size.and_then(|s| s.parse::<f64>().ok()),
                            t.price.and_then(|s| s.parse::<f64>().ok()),
                            t.outcome,
                            t.outcome_index,
                            t.timestamp.unwrap_or(0),
                            tx_hash,
                            raw_json,
                        ],
                    )?;
                    page_ins += changed as u64;
                }
                tx.commit()?;
                Ok(page_ins)
            })
            .await?;

        inserted += page_inserted;
        offset += limit;

        // If all trades on this page were already known, stop — no need to
        // fetch older pages we've already ingested.
        if all_known {
            tracing::debug!(
                user,
                offset,
                "all trades on page already known; stopping pagination"
            );
            break;
        }

        // Stop if API returns less than a full page.
        if page_len < limit as usize {
            break;
        }
    }

    Ok((pages, inserted))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each page is either Ok((trades, raw_bytes)) or Err.
    struct FakeTradesPager {
        pages: Vec<Result<(Vec<ApiTrade>, Vec<u8>)>>,
    }

    impl FakeTradesPager {
        fn from_ok_pages(pages: Vec<(Vec<ApiTrade>, Vec<u8>)>) -> Self {
            Self {
                pages: pages.into_iter().map(Ok).collect(),
            }
        }

        fn new(pages: Vec<Result<(Vec<ApiTrade>, Vec<u8>)>>) -> Self {
            Self { pages }
        }
    }

    impl TradesPager for FakeTradesPager {
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
            let idx = (offset / 2) as usize;
            match self.pages.get(idx) {
                Some(Ok(page)) => Ok(page.clone()),
                Some(Err(_)) => Err(anyhow::anyhow!("HTTP 400 Bad Request")),
                None => Ok(Default::default()),
            }
        }
    }

    #[tokio::test]
    async fn test_ingest_trades_for_wallet_dedup_raw_and_pagination() {
        let db = AsyncDb::open(":memory:").await.unwrap();

        let page1 = vec![
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xtx1".to_string()),
                size: Some("10".to_string()),
                price: Some("0.5".to_string()),
                timestamp: Some(1),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xtx2".to_string()),
                size: Some("5".to_string()),
                price: Some("0.6".to_string()),
                timestamp: Some(2),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
        ];

        // page2 repeats tx2 (duplicate) and adds tx3.
        let page2 = vec![
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xtx2".to_string()),
                size: Some("5".to_string()),
                price: Some("0.6".to_string()),
                timestamp: Some(2),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xtx3".to_string()),
                size: Some("1".to_string()),
                price: Some("0.7".to_string()),
                timestamp: Some(3),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
        ];

        // page3 contains:
        // - a trade with missing tx hash (should still insert, and NOT collide with other missing tx hash rows)
        // - a trade missing required keys (should be skipped)
        let page3 = vec![
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: None,
                size: Some("2".to_string()),
                price: Some("0.55".to_string()),
                timestamp: Some(4),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
            ApiTrade {
                proxy_wallet: None,
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xtx_skip".to_string()),
                size: Some("2".to_string()),
                price: Some("0.55".to_string()),
                timestamp: Some(4),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
        ];

        let pager = FakeTradesPager::from_ok_pages(vec![
            (page1, br#"[{"page":1}]"#.to_vec()),
            (page2, br#"[{"page":2}]"#.to_vec()),
            (page3, br#"[{"page":3}]"#.to_vec()),
            (vec![], b"[]".to_vec()), // end
        ]);

        let (_pages, inserted) = ingest_trades_for_wallet(&db, &pager, "0xw", 2)
            .await
            .unwrap();
        assert_eq!(inserted, 4); // tx2 inserted once, + tx1 + tx3 + missing-tx row; skipped row not inserted

        let trades_count: i64 = db
            .call(|conn| {
                let tc = conn.query_row("SELECT COUNT(*) FROM trades_raw", [], |row| row.get(0))?;
                Ok(tc)
            })
            .await
            .unwrap();
        assert_eq!(trades_count, 4);
    }

    #[tokio::test]
    async fn test_ingest_trades_gracefully_handles_http_400_at_high_offset() {
        let db = AsyncDb::open(":memory:").await.unwrap();

        let page1 = vec![
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xtx1".to_string()),
                size: Some("10".to_string()),
                price: Some("0.5".to_string()),
                timestamp: Some(1),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xtx2".to_string()),
                size: Some("5".to_string()),
                price: Some("0.6".to_string()),
                timestamp: Some(2),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
        ];

        // Page 2 returns HTTP 400 (simulating Polymarket offset cap).
        let pager = FakeTradesPager::new(vec![
            Ok((page1, br#"[{"page":1}]"#.to_vec())),
            Err(anyhow::anyhow!("HTTP 400 Bad Request")),
        ]);

        // Should NOT return an error — should return what was collected before the 400.
        let result = ingest_trades_for_wallet(&db, &pager, "0xw", 2).await;
        assert!(result.is_ok(), "Expected Ok but got: {result:?}");

        let (_pages, inserted) = result.unwrap();
        assert_eq!(inserted, 2); // tx1 + tx2 from page 1

        let trades_count: i64 = db
            .call(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM trades_raw", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(trades_count, 2);
    }

    #[tokio::test]
    async fn test_ingest_trades_stops_early_when_all_trades_already_known() {
        let db = AsyncDb::open(":memory:").await.unwrap();

        // Pre-populate two trades with timestamps 100 and 200.
        db.call(|conn| {
            conn.execute(
                "INSERT INTO trades_raw
                   (proxy_wallet, condition_id, size, price, timestamp, transaction_hash, raw_json)
                   VALUES ('0xw', '0xm', 10.0, 0.5, 100, '0xold1', '{}')",
                [],
            )?;
            conn.execute(
                "INSERT INTO trades_raw
                   (proxy_wallet, condition_id, size, price, timestamp, transaction_hash, raw_json)
                   VALUES ('0xw', '0xm', 5.0, 0.6, 200, '0xold2', '{}')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // API returns: page1 has one new trade (ts=300) + one old (ts=200),
        // page2 has all old trades (ts=100, ts=50). We should stop after page2.
        // Page3 should NEVER be fetched.
        let page1 = vec![
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xnew1".to_string()),
                size: Some("3".to_string()),
                price: Some("0.7".to_string()),
                timestamp: Some(300),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xold2".to_string()),
                size: Some("5".to_string()),
                price: Some("0.6".to_string()),
                timestamp: Some(200),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
        ];
        let page2 = vec![
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xold1".to_string()),
                size: Some("10".to_string()),
                price: Some("0.5".to_string()),
                timestamp: Some(100),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xancient".to_string()),
                size: Some("1".to_string()),
                price: Some("0.4".to_string()),
                timestamp: Some(50),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
        ];
        // Page3 = trap: if we reach it, pagination didn't stop early.
        let page3 = vec![
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xtrap".to_string()),
                size: Some("99".to_string()),
                price: Some("0.99".to_string()),
                timestamp: Some(1),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
            ApiTrade {
                proxy_wallet: Some("0xw".to_string()),
                condition_id: Some("0xm".to_string()),
                transaction_hash: Some("0xtrap2".to_string()),
                size: Some("99".to_string()),
                price: Some("0.99".to_string()),
                timestamp: Some(2),
                asset: None,
                title: None,
                slug: None,
                outcome: None,
                outcome_index: None,
                side: None,
                pseudonym: None,
                name: None,
            },
        ];

        let pager = FakeTradesPager::from_ok_pages(vec![
            (page1, b"[]".to_vec()),
            (page2, b"[]".to_vec()),
            (page3, b"[]".to_vec()),
        ]);

        let (pages, inserted) = ingest_trades_for_wallet(&db, &pager, "0xw", 2)
            .await
            .unwrap();

        // Page1 had a mix (new + old), so we continue. Page2 was all old, so we stop.
        assert_eq!(pages, 2, "should have fetched exactly 2 pages, not 3");
        // Only 0xnew1 (ts=300) and 0xancient (ts=50) are new inserts.
        // 0xold1 and 0xold2 are deduped by INSERT OR IGNORE.
        assert_eq!(inserted, 2, "should have inserted 2 new trades");

        // Total in DB: 2 pre-existing + 2 new = 4
        let total: i64 = db
            .call(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM trades_raw", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(total, 4);
    }
}
