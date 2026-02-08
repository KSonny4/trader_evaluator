use anyhow::Result;
use common::db::Database;
use common::types::ApiTrade;

pub trait TradesPager {
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
    db: &Database,
    pager: &P,
    user: &str,
    limit: u32,
) -> Result<(u64, u64)> {
    let mut offset = 0;
    let mut pages = 0_u64;
    let mut inserted = 0_u64;

    loop {
        let fetch_result = pager.fetch_trades_page(user, limit, offset).await;
        let (trades, raw_body) = match fetch_result {
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

        // Save raw response bytes (unmodified) before parsing/derivation.
        let url = pager.trades_url(user, limit, offset);
        db.conn.execute(
            "INSERT INTO raw_api_responses (api, method, url, response_body) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["data_api", "GET", url, raw_body],
        )?;

        if trades.is_empty() {
            break;
        }

        for t in trades {
            let proxy_wallet = match t.proxy_wallet.as_deref() {
                Some(v) if !v.is_empty() => v.to_string(),
                _ => continue, // required key missing
            };
            let condition_id = match t.condition_id.as_deref() {
                Some(v) if !v.is_empty() => v.to_string(),
                _ => continue, // required key missing
            };
            let tx = t.transaction_hash.clone();

            // Persist derived row; rely on UNIQUE constraint to deduplicate.
            let raw_json = serde_json::to_string(&t).unwrap_or_default();
            let changed = db.conn.execute(
                r#"
                INSERT OR IGNORE INTO trades_raw
                    (proxy_wallet, condition_id, asset, side, size, price, outcome, outcome_index, timestamp, transaction_hash, raw_json)
                VALUES
                    (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                "#,
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
                    tx,
                    raw_json,
                ],
            )?;
            inserted += changed as u64;
        }

        offset += limit;

        // Stop if API returns less than a full page. This is a simple guard against
        // accidental infinite loops during early bring-up.
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
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

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
            (vec![], br#"[]"#.to_vec()), // end
        ]);

        let (_pages, inserted) = ingest_trades_for_wallet(&db, &pager, "0xw", 2)
            .await
            .unwrap();
        assert_eq!(inserted, 4); // tx2 inserted once, + tx1 + tx3 + missing-tx row; skipped row not inserted

        let trades_count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM trades_raw", [], |row| row.get(0))
            .unwrap();
        assert_eq!(trades_count, 4);

        let raw_count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM raw_api_responses", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(raw_count >= 3); // at least the three non-empty pages
    }

    #[tokio::test]
    async fn test_ingest_trades_gracefully_handles_http_400_at_high_offset() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

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
        assert!(result.is_ok(), "Expected Ok but got: {:?}", result);

        let (_pages, inserted) = result.unwrap();
        assert_eq!(inserted, 2); // tx1 + tx2 from page 1

        let trades_count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM trades_raw", [], |row| row.get(0))
            .unwrap();
        assert_eq!(trades_count, 2);
    }
}
