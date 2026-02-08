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
        let (trades, raw_body) = pager.fetch_trades_page(user, limit, offset).await?;
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
            let tx = t.transaction_hash.clone().unwrap_or_default();
            let proxy_wallet = t.proxy_wallet.clone().unwrap_or_default();
            let condition_id = t.condition_id.clone().unwrap_or_default();

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

    struct FakeTradesPager {
        pages: Vec<(Vec<ApiTrade>, Vec<u8>)>,
    }

    impl FakeTradesPager {
        fn new(pages: Vec<(Vec<ApiTrade>, Vec<u8>)>) -> Self {
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
            Ok(self.pages.get(idx).cloned().unwrap_or_default())
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

        let pager = FakeTradesPager::new(vec![
            (page1, br#"[{"page":1}]"#.to_vec()),
            (page2, br#"[{"page":2}]"#.to_vec()),
            (vec![], br#"[]"#.to_vec()), // end
        ]);

        let (_pages, inserted) = ingest_trades_for_wallet(&db, &pager, "0xw", 2)
            .await
            .unwrap();
        assert_eq!(inserted, 3); // tx2 inserted once

        let trades_count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM trades_raw", [], |row| row.get(0))
            .unwrap();
        assert_eq!(trades_count, 3);

        let raw_count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM raw_api_responses", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(raw_count >= 2); // at least the two non-empty pages
    }
}
