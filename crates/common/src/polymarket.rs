use crate::types::{
    ApiActivity, ApiHolderResponse, ApiLeaderboardEntry, ApiPosition, ApiTrade, GammaMarket,
};
use anyhow::Result;
use reqwest::{Client, StatusCode, Url};
use std::error::Error as StdError;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct HttpStatusError {
    pub status: StatusCode,
    pub url: Url,
}

impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} for {}", self.status, self.url)
    }
}

impl StdError for HttpStatusError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorKind {
    RateLimited,
    Timeout,
    Upstream5xx,
    BadRequest,
    PaginationOffsetCap,
    Decode,
    Connect,
    Other,
}

impl ApiErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::Upstream5xx => "upstream_5xx",
            Self::BadRequest => "bad_request",
            Self::PaginationOffsetCap => "pagination_offset_cap",
            Self::Decode => "decode",
            Self::Connect => "connect",
            Self::Other => "other",
        }
    }
}

/// Classify an API failure into a small set of alertable buckets.
///
/// IMPORTANT: keep the returned `kind` set small to avoid Prometheus cardinality blowups.
pub fn classify_anyhow_api_error(err: &anyhow::Error) -> ApiErrorKind {
    for cause in err.chain() {
        if let Some(h) = cause.downcast_ref::<HttpStatusError>() {
            if h.status == StatusCode::TOO_MANY_REQUESTS {
                return ApiErrorKind::RateLimited;
            }
            if h.status.is_server_error() {
                return ApiErrorKind::Upstream5xx;
            }
            if h.status == StatusCode::REQUEST_TIMEOUT {
                return ApiErrorKind::Timeout;
            }
            if h.status == StatusCode::BAD_REQUEST {
                if is_trades_offset_cap_url(&h.url) {
                    return ApiErrorKind::PaginationOffsetCap;
                }
                return ApiErrorKind::BadRequest;
            }
        }

        if let Some(r) = cause.downcast_ref::<reqwest::Error>() {
            if r.is_timeout() {
                return ApiErrorKind::Timeout;
            }
            if r.is_connect() {
                return ApiErrorKind::Connect;
            }
        }

        if cause.downcast_ref::<serde_json::Error>().is_some() {
            return ApiErrorKind::Decode;
        }
    }

    ApiErrorKind::Other
}

fn is_trades_offset_cap_url(url: &Url) -> bool {
    if !url.path().ends_with("/trades") {
        return false;
    }
    for (k, v) in url.query_pairs() {
        if k == "offset" {
            if let Ok(n) = v.parse::<u32>() {
                // Observed: /trades?...&offset=3200 -> HTTP 400.
                return n >= 3000;
            }
        }
    }
    false
}

/// Server-side filters for the Gamma `/markets` endpoint.
#[derive(Debug, Clone, Default)]
pub struct GammaFilter {
    /// Only markets with liquidity >= this value.
    pub liquidity_num_min: Option<f64>,
    /// Only markets with volume >= this value.
    pub volume_num_min: Option<f64>,
    /// Only markets ending on or after this date (ISO-8601, e.g. "2026-02-09").
    pub end_date_min: Option<String>,
    /// Only markets ending on or before this date.
    pub end_date_max: Option<String>,
    /// false = only open markets.
    pub closed: Option<bool>,
}

pub struct PolymarketClient {
    data_api_url: String,
    gamma_api_url: String,
    client: Client,
    rate_limit_delay: Duration,
    max_retries: u32,
    backoff_base: Duration,
}

impl PolymarketClient {
    pub fn data_api_url(&self) -> &str {
        &self.data_api_url
    }

    pub fn gamma_api_url(&self) -> &str {
        &self.gamma_api_url
    }

    pub fn new(data_api_url: &str, gamma_api_url: &str) -> Self {
        Self::new_with_settings(
            data_api_url,
            gamma_api_url,
            Duration::from_secs(15),
            Duration::from_millis(200),
            3,
            Duration::from_millis(1000),
        )
    }

    pub fn new_with_settings(
        data_api_url: &str,
        gamma_api_url: &str,
        timeout: Duration,
        rate_limit_delay: Duration,
        max_retries: u32,
        backoff_base: Duration,
    ) -> Self {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client build failed");

        Self {
            data_api_url: data_api_url.trim_end_matches('/').to_string(),
            gamma_api_url: gamma_api_url.trim_end_matches('/').to_string(),
            client,
            rate_limit_delay,
            max_retries,
            backoff_base,
        }
    }

    pub fn trades_url_any(
        &self,
        user: Option<&str>,
        market: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> String {
        let mut url = Url::parse(&format!("{}/trades", self.data_api_url))
            .expect("data_api_url must be a valid absolute URL");
        {
            let mut qp = url.query_pairs_mut();
            if let Some(u) = user {
                qp.append_pair("user", u);
            }
            if let Some(m) = market {
                qp.append_pair("market", m);
            }
            qp.append_pair("limit", &limit.to_string());
            qp.append_pair("offset", &offset.to_string());
        }
        url.to_string()
    }

    pub fn trades_url(&self, user: &str, market: Option<&str>, limit: u32, offset: u32) -> String {
        self.trades_url_any(Some(user), market, limit, offset)
    }

    #[allow(dead_code)]
    pub async fn fetch_trades(
        &self,
        user: &str,
        market: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ApiTrade>> {
        let (trades, _raw) = self
            .fetch_trades_raw_any(Some(user), market, limit, offset)
            .await?;
        Ok(trades)
    }

    #[allow(dead_code)]
    pub async fn fetch_trades_raw_any(
        &self,
        user: Option<&str>,
        market: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiTrade>, Vec<u8>)> {
        let url = self.trades_url_any(user, market, limit, offset);
        let body = self.get_bytes_with_retry(url).await?;
        Ok((serde_json::from_slice(&body)?, body))
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
        let (holders, _raw) = self.fetch_holders_raw(condition_ids, limit).await?;
        Ok(holders)
    }

    #[allow(dead_code)]
    pub async fn fetch_holders_raw(
        &self,
        condition_ids: &str,
        limit: u32,
    ) -> Result<(Vec<ApiHolderResponse>, Vec<u8>)> {
        let mut url = Url::parse(&format!("{}/holders", self.data_api_url))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("market", condition_ids);
            qp.append_pair("limit", &limit.to_string());
        }
        let body = self.get_bytes_with_retry(url).await?;
        Ok((serde_json::from_slice(&body)?, body))
    }

    #[allow(dead_code)]
    pub async fn fetch_activity(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ApiActivity>> {
        let (activity, _raw) = self.fetch_activity_raw(user, limit, offset).await?;
        Ok(activity)
    }

    #[allow(dead_code)]
    pub async fn fetch_activity_raw(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiActivity>, Vec<u8>)> {
        let mut url = Url::parse(&format!("{}/activity", self.data_api_url))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("user", user);
            qp.append_pair("limit", &limit.to_string());
            qp.append_pair("offset", &offset.to_string());
        }
        let body = self.get_bytes_with_retry(url).await?;
        Ok((serde_json::from_slice(&body)?, body))
    }

    #[allow(dead_code)]
    pub async fn fetch_positions(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ApiPosition>> {
        let (positions, _raw) = self.fetch_positions_raw(user, limit, offset).await?;
        Ok(positions)
    }

    #[allow(dead_code)]
    pub async fn fetch_positions_raw(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiPosition>, Vec<u8>)> {
        let mut url = Url::parse(&format!("{}/positions", self.data_api_url))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("user", user);
            qp.append_pair("limit", &limit.to_string());
            qp.append_pair("offset", &offset.to_string());
        }
        let body = self.get_bytes_with_retry(url).await?;
        Ok((serde_json::from_slice(&body)?, body))
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
        let body = self.get_text_with_retry(url).await?;
        Ok(serde_json::from_str(&body)?)
    }

    #[allow(dead_code)]
    pub async fn fetch_gamma_markets(&self, limit: u32, offset: u32) -> Result<Vec<GammaMarket>> {
        let (markets, _raw) = self
            .fetch_gamma_markets_raw(limit, offset, &GammaFilter::default())
            .await?;
        Ok(markets)
    }

    pub async fn fetch_gamma_markets_raw(
        &self,
        limit: u32,
        offset: u32,
        filter: &GammaFilter,
    ) -> Result<(Vec<GammaMarket>, Vec<u8>)> {
        let mut url = Url::parse(&format!("{}/markets", self.gamma_api_url))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("limit", &limit.to_string());
            qp.append_pair("offset", &offset.to_string());
            if let Some(v) = filter.liquidity_num_min {
                qp.append_pair("liquidity_num_min", &v.to_string());
            }
            if let Some(v) = filter.volume_num_min {
                qp.append_pair("volume_num_min", &v.to_string());
            }
            if let Some(ref v) = filter.end_date_min {
                qp.append_pair("end_date_min", v);
            }
            if let Some(ref v) = filter.end_date_max {
                qp.append_pair("end_date_max", v);
            }
            if let Some(closed) = filter.closed {
                qp.append_pair("closed", &closed.to_string());
            }
        }
        let body = self.get_bytes_with_retry(url).await?;
        Ok((serde_json::from_slice(&body)?, body))
    }

    async fn get_text_with_retry<U: IntoUrlLike>(&self, url: U) -> Result<String> {
        let url = url.into_url()?;
        let mut attempt: u32 = 0;

        loop {
            attempt += 1;
            if !self.rate_limit_delay.is_zero() {
                tokio::time::sleep(self.rate_limit_delay).await;
            }

            let req = self.client.get(url.clone());
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return Ok(resp.text().await?);
                    }

                    // Retry on transient statuses.
                    if attempt <= self.max_retries
                        && (status == StatusCode::TOO_MANY_REQUESTS
                            || status.is_server_error()
                            || status == StatusCode::REQUEST_TIMEOUT)
                    {
                        let backoff = self.backoff_base.mul_f64(2_f64.powi((attempt - 1) as i32));
                        tokio::time::sleep(backoff).await;
                        continue;
                    }

                    return Err(anyhow::Error::new(HttpStatusError { status, url }));
                }
                Err(e) => {
                    if attempt <= self.max_retries {
                        let backoff = self.backoff_base.mul_f64(2_f64.powi((attempt - 1) as i32));
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(e.into());
                }
            }
        }
    }

    async fn get_bytes_with_retry<U: IntoUrlLike>(&self, url: U) -> Result<Vec<u8>> {
        let url = url.into_url()?;
        let mut attempt: u32 = 0;

        loop {
            attempt += 1;
            if !self.rate_limit_delay.is_zero() {
                tokio::time::sleep(self.rate_limit_delay).await;
            }

            let req = self.client.get(url.clone());
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let b = resp.bytes().await?;
                        return Ok(b.to_vec());
                    }

                    if attempt <= self.max_retries
                        && (status == StatusCode::TOO_MANY_REQUESTS
                            || status.is_server_error()
                            || status == StatusCode::REQUEST_TIMEOUT)
                    {
                        let backoff = self.backoff_base.mul_f64(2_f64.powi((attempt - 1) as i32));
                        tokio::time::sleep(backoff).await;
                        continue;
                    }

                    return Err(anyhow::Error::new(HttpStatusError { status, url }));
                }
                Err(e) => {
                    if attempt <= self.max_retries {
                        let backoff = self.backoff_base.mul_f64(2_f64.powi((attempt - 1) as i32));
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(e.into());
                }
            }
        }
    }
}

trait IntoUrlLike {
    fn into_url(self) -> Result<Url>;
}

impl IntoUrlLike for String {
    fn into_url(self) -> Result<Url> {
        Ok(Url::parse(&self)?)
    }
}

impl IntoUrlLike for Url {
    fn into_url(self) -> Result<Url> {
        Ok(self)
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
    fn test_client_constructs_market_trades_url_without_user() {
        let client = PolymarketClient::new(
            "https://data-api.polymarket.com",
            "https://gamma-api.polymarket.com",
        );
        let url = client.trades_url_any(None, Some("0xcond"), 5, 0);
        assert!(url.contains("/trades"));
        assert!(url.contains("market=0xcond"));
        assert!(!url.contains("user="));
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

    #[test]
    fn test_classify_http_429_as_rate_limited() {
        let url = Url::parse("https://data-api.polymarket.com/trades?user=0xabc&limit=200&offset=0")
            .unwrap();
        let err = anyhow::Error::new(HttpStatusError {
            status: StatusCode::TOO_MANY_REQUESTS,
            url,
        });
        assert_eq!(classify_anyhow_api_error(&err), ApiErrorKind::RateLimited);
    }

    #[test]
    fn test_classify_http_400_deep_offset_as_pagination_offset_cap() {
        let url = Url::parse(
            "https://data-api.polymarket.com/trades?user=0xabc&limit=200&offset=3200",
        )
        .unwrap();
        let err = anyhow::Error::new(HttpStatusError {
            status: StatusCode::BAD_REQUEST,
            url,
        });
        assert_eq!(
            classify_anyhow_api_error(&err),
            ApiErrorKind::PaginationOffsetCap
        );
    }

    #[test]
    fn test_classify_json_decode_as_decode() {
        let bad_json = b"{this is not json}";
        let err = serde_json::from_slice::<Vec<ApiTrade>>(bad_json).unwrap_err();
        let err = anyhow::Error::from(err);
        assert_eq!(classify_anyhow_api_error(&err), ApiErrorKind::Decode);
    }
}
