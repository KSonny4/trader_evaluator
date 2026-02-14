use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, warn};

/// Trader's own minimal Polymarket API client — focused on fetching trades for followed wallets.
pub struct TraderPolymarketClient {
    data_api_url: String,
    client: reqwest::Client,
    rate_limit_delay: Duration,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawTrade {
    #[serde(rename = "proxyWallet", alias = "proxy_wallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "conditionId", alias = "condition_id")]
    pub condition_id: Option<String>,
    pub asset: Option<String>,
    #[serde(deserialize_with = "de_opt_string_any", default)]
    pub size: Option<String>,
    #[serde(deserialize_with = "de_opt_string_any", default)]
    pub price: Option<String>,
    pub timestamp: Option<i64>,
    pub outcome: Option<String>,
    #[serde(rename = "outcomeIndex", alias = "outcome_index")]
    pub outcome_index: Option<i32>,
    pub side: Option<String>,
    #[serde(rename = "transactionHash", alias = "transaction_hash")]
    pub transaction_hash: Option<String>,
    pub id: Option<String>,
}

impl TraderPolymarketClient {
    pub fn new(data_api_url: &str, rate_limit_delay_ms: u64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        Self {
            data_api_url: data_api_url.trim_end_matches('/').to_string(),
            client,
            rate_limit_delay: Duration::from_millis(rate_limit_delay_ms),
        }
    }

    /// Fetch recent trades for a wallet. Returns up to `limit` trades.
    /// Uses the Data API `/trades?user=<wallet>&limit=<n>` endpoint.
    pub async fn fetch_trades(
        &self,
        wallet: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RawTrade>> {
        let encoded_wallet = urlencoding::encode(wallet);
        let url = format!(
            "{}/trades?user={encoded_wallet}&limit={limit}&offset={offset}",
            self.data_api_url
        );

        debug!(url = %url, "fetching trades");

        // Rate limiting
        tokio::time::sleep(self.rate_limit_delay).await;

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("failed to fetch trades for {wallet}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                warn!(wallet = wallet, "rate limited fetching trades, backing off");
                tokio::time::sleep(Duration::from_secs(2)).await;
                return Ok(vec![]);
            }
            anyhow::bail!("trades API returned {status}: {body}");
        }

        let trades: Vec<RawTrade> = resp
            .json()
            .await
            .context("failed to deserialize trades response")?;

        debug!(wallet = wallet, count = trades.len(), "fetched trades");
        Ok(trades)
    }

    /// Check if a market has resolved. Returns Some(settle_price) if resolved, None otherwise.
    /// Uses the Gamma API markets endpoint.
    pub async fn check_market_resolution(&self, url: &str) -> Option<f64> {
        let resp = self.client.get(url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }

        let markets: Vec<serde_json::Value> = resp.json().await.ok()?;
        let market = markets.first()?;

        // Check if market is resolved
        let closed = market.get("closed")?.as_bool()?;
        if !closed {
            return None;
        }

        // Get the resolution price (1.0 = Yes won, 0.0 = No won)
        let price_str = market.get("outcomePrices")?.as_str()?;
        let prices: Vec<f64> = serde_json::from_str(price_str).ok()?;
        // If the first outcome price is >= 0.99, it resolved Yes (1.0); if <= 0.01, resolved No (0.0)
        let settle_price = prices.first().copied()?;
        if settle_price >= 0.99 {
            Some(1.0)
        } else if settle_price <= 0.01 {
            Some(0.0)
        } else {
            None // Market closed but not fully resolved yet
        }
    }

    /// Compute a stable hash for a trade to use as watermark.
    pub fn trade_hash(trade: &RawTrade) -> String {
        // Use id if available, else combine wallet+condition+timestamp+side
        if let Some(id) = &trade.id {
            return id.clone();
        }
        if let Some(hash) = &trade.transaction_hash {
            return hash.clone();
        }
        format!(
            "{}-{}-{}-{}",
            trade.proxy_wallet.as_deref().unwrap_or(""),
            trade.condition_id.as_deref().unwrap_or(""),
            trade.timestamp.unwrap_or(0),
            trade.side.as_deref().unwrap_or(""),
        )
    }
}

/// Deserialize a field that can be either a string or a number into Option<String>.
fn de_opt_string_any<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StringOrNumber;

    impl<'de> de::Visitor<'de> for StringOrNumber {
        type Value = Option<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "a string or number")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }

    deserializer.deserialize_any(StringOrNumber)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trade_hash_with_id() {
        let trade = RawTrade {
            id: Some("trade-123".to_string()),
            proxy_wallet: Some("0xabc".to_string()),
            condition_id: Some("cond-1".to_string()),
            asset: None,
            size: Some("100".to_string()),
            price: Some("0.50".to_string()),
            timestamp: Some(1700000000),
            outcome: Some("Yes".to_string()),
            outcome_index: Some(0),
            side: Some("BUY".to_string()),
            transaction_hash: Some("0xhash".to_string()),
        };
        assert_eq!(TraderPolymarketClient::trade_hash(&trade), "trade-123");
    }

    #[test]
    fn test_trade_hash_fallback_to_tx_hash() {
        let trade = RawTrade {
            id: None,
            proxy_wallet: Some("0xabc".to_string()),
            condition_id: Some("cond-1".to_string()),
            asset: None,
            size: None,
            price: None,
            timestamp: Some(1700000000),
            outcome: None,
            outcome_index: None,
            side: Some("BUY".to_string()),
            transaction_hash: Some("0xtxhash".to_string()),
        };
        assert_eq!(TraderPolymarketClient::trade_hash(&trade), "0xtxhash");
    }

    #[test]
    fn test_trade_hash_fallback_composite() {
        let trade = RawTrade {
            id: None,
            proxy_wallet: Some("0xabc".to_string()),
            condition_id: Some("cond-1".to_string()),
            asset: None,
            size: None,
            price: None,
            timestamp: Some(1700000000),
            outcome: None,
            outcome_index: None,
            side: Some("BUY".to_string()),
            transaction_hash: None,
        };
        assert_eq!(
            TraderPolymarketClient::trade_hash(&trade),
            "0xabc-cond-1-1700000000-BUY"
        );
    }

    #[test]
    fn test_deserialize_raw_trade() {
        let json = r#"{
            "proxyWallet": "0xabc",
            "conditionId": "cond-1",
            "size": "100.5",
            "price": 0.65,
            "timestamp": 1700000000,
            "outcome": "Yes",
            "outcomeIndex": 0,
            "side": "BUY",
            "id": "t-1"
        }"#;
        let trade: RawTrade = serde_json::from_str(json).unwrap();
        assert_eq!(trade.proxy_wallet.as_deref(), Some("0xabc"));
        assert_eq!(trade.condition_id.as_deref(), Some("cond-1"));
        assert_eq!(trade.size.as_deref(), Some("100.5"));
        assert_eq!(trade.price.as_deref(), Some("0.65"));
        assert_eq!(trade.timestamp, Some(1700000000));
        assert_eq!(trade.side.as_deref(), Some("BUY"));
    }

    #[test]
    fn test_deserialize_raw_trade_numeric_size() {
        let json = r#"{
            "proxyWallet": "0x1",
            "conditionId": "c-1",
            "size": 42.5,
            "price": "0.75",
            "timestamp": 1700000000,
            "side": "SELL"
        }"#;
        let trade: RawTrade = serde_json::from_str(json).unwrap();
        assert_eq!(trade.size.as_deref(), Some("42.5"));
        assert_eq!(trade.price.as_deref(), Some("0.75"));
    }
}
