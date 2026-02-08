use crate::types::{
    ApiActivity, ApiHolderResponse, ApiLeaderboardEntry, ApiPosition, ApiTrade, GammaMarket,
};
use anyhow::Result;
use reqwest::Url;

pub struct PolymarketClient {
    data_api_url: String,
    gamma_api_url: String,
}

impl PolymarketClient {
    pub fn new(data_api_url: &str, gamma_api_url: &str) -> Self {
        Self {
            data_api_url: data_api_url.trim_end_matches('/').to_string(),
            gamma_api_url: gamma_api_url.trim_end_matches('/').to_string(),
        }
    }

    pub fn trades_url(&self, user: &str, market: Option<&str>, limit: u32, offset: u32) -> String {
        let mut url = Url::parse(&format!("{}/trades", self.data_api_url))
            .expect("data_api_url must be a valid absolute URL");
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("user", user);
            if let Some(m) = market {
                qp.append_pair("market", m);
            }
            qp.append_pair("limit", &limit.to_string());
            qp.append_pair("offset", &offset.to_string());
        }
        url.to_string()
    }

    #[allow(dead_code)]
    pub async fn fetch_trades(
        &self,
        user: &str,
        market: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ApiTrade>> {
        let url = self.trades_url(user, market, limit, offset);
        let body = reqwest::get(url).await?.text().await?;
        Ok(serde_json::from_str(&body)?)
    }

    #[allow(dead_code)]
    pub async fn fetch_holders(
        &self,
        condition_ids: &str,
        limit: u32,
    ) -> Result<Vec<ApiHolderResponse>> {
        let mut url = Url::parse(&format!("{}/holders", self.data_api_url))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("market", condition_ids);
            qp.append_pair("limit", &limit.to_string());
        }
        let body = reqwest::get(url).await?.text().await?;
        Ok(serde_json::from_str(&body)?)
    }

    #[allow(dead_code)]
    pub async fn fetch_activity(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ApiActivity>> {
        let mut url = Url::parse(&format!("{}/activity", self.data_api_url))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("user", user);
            qp.append_pair("limit", &limit.to_string());
            qp.append_pair("offset", &offset.to_string());
        }
        let body = reqwest::get(url).await?.text().await?;
        Ok(serde_json::from_str(&body)?)
    }

    #[allow(dead_code)]
    pub async fn fetch_positions(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ApiPosition>> {
        let mut url = Url::parse(&format!("{}/positions", self.data_api_url))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("user", user);
            qp.append_pair("limit", &limit.to_string());
            qp.append_pair("offset", &offset.to_string());
        }
        let body = reqwest::get(url).await?.text().await?;
        Ok(serde_json::from_str(&body)?)
    }

    #[allow(dead_code)]
    pub async fn fetch_leaderboard(
        &self,
        category: &str,
        time_period: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ApiLeaderboardEntry>> {
        let mut url = Url::parse(&format!("{}/v1/leaderboard", self.data_api_url))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("category", category);
            qp.append_pair("timePeriod", time_period);
            qp.append_pair("limit", &limit.to_string());
            qp.append_pair("offset", &offset.to_string());
        }
        let body = reqwest::get(url).await?.text().await?;
        Ok(serde_json::from_str(&body)?)
    }

    #[allow(dead_code)]
    pub async fn fetch_gamma_markets(&self, limit: u32, offset: u32) -> Result<Vec<GammaMarket>> {
        let mut url = Url::parse(&format!("{}/markets", self.gamma_api_url))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("limit", &limit.to_string());
            qp.append_pair("offset", &offset.to_string());
        }
        let body = reqwest::get(url).await?.text().await?;
        Ok(serde_json::from_str(&body)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_constructs_trades_url() {
        let client = PolymarketClient::new(
            "https://data-api.polymarket.com",
            "https://gamma-api.polymarket.com",
        );
        let url = client.trades_url("0xabc123", None, 100, 0);
        assert!(url.contains("/trades"));
        assert!(url.contains("user=0xabc123"));
        assert!(url.contains("limit=100"));
    }

    #[test]
    fn test_parse_trades_response() {
        let json = r#"[{"proxyWallet":"0xabc","conditionId":"0xdef","size":"10","price":"0.50","timestamp":1700000000}]"#;
        let trades: Vec<ApiTrade> = serde_json::from_str(json).unwrap();
        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].proxy_wallet.as_deref(), Some("0xabc"));
    }

    #[test]
    fn test_parse_holders_response() {
        let json = r#"[{"token":"0xtok","holders":[{"proxyWallet":"0xabc","amount":100.0,"outcomeIndex":0}]}]"#;
        let holders: Vec<ApiHolderResponse> = serde_json::from_str(json).unwrap();
        assert_eq!(holders[0].holders.len(), 1);
    }

    #[test]
    fn test_parse_fixture_gamma_markets() {
        let json = include_str!("../../../tests/fixtures/gamma_markets_sample.json");
        let markets: Vec<GammaMarket> = serde_json::from_str(json).unwrap();
        assert!(!markets.is_empty());
    }

    #[test]
    fn test_parse_fixture_trades() {
        let json = include_str!("../../../tests/fixtures/trades_sample.json");
        let trades: Vec<ApiTrade> = serde_json::from_str(json).unwrap();
        assert!(!trades.is_empty());
    }
}
