use anyhow::Result;
use common::polymarket::{classify_anyhow_api_error, GammaFilter, PolymarketClient};
use common::types::{
    ApiActivity, ApiHolderResponse, ApiLeaderboardEntry, ApiPosition, ApiTrade, GammaMarket,
};
use std::time::Instant;

use super::fetcher_traits::*;

impl GammaMarketsPager for PolymarketClient {
    fn gamma_markets_url(&self, limit: u32, offset: u32) -> String {
        format!(
            "{}/markets?limit={limit}&offset={offset}",
            self.gamma_api_url()
        )
    }

    async fn fetch_gamma_markets_page(
        &self,
        limit: u32,
        offset: u32,
        filter: &GammaFilter,
    ) -> Result<(Vec<GammaMarket>, Vec<u8>)> {
        let start = Instant::now();
        let res = self.fetch_gamma_markets_raw(limit, offset, filter).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "gamma_markets").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "gamma_markets", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "gamma_markets", "status" => "error").increment(1);
                metrics::counter!(
                    "evaluator_api_errors_total",
                    "endpoint" => "gamma_markets",
                    "kind" => classify_anyhow_api_error(&e).as_str()
                )
                .increment(1);
                Err(e)
            }
        }
    }
}

impl HoldersFetcher for PolymarketClient {
    fn holders_url(&self, condition_id: &str, limit: u32) -> String {
        format!(
            "{}/holders?market={condition_id}&limit={limit}",
            self.data_api_url()
        )
    }

    async fn fetch_holders(
        &self,
        condition_id: &str,
        limit: u32,
    ) -> Result<(Vec<ApiHolderResponse>, Vec<u8>)> {
        let start = Instant::now();
        let res = self.fetch_holders_raw(condition_id, limit).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "holders").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "holders", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "holders", "status" => "error").increment(1);
                metrics::counter!(
                    "evaluator_api_errors_total",
                    "endpoint" => "holders",
                    "kind" => classify_anyhow_api_error(&e).as_str()
                )
                .increment(1);
                Err(e)
            }
        }
    }
}

impl MarketTradesFetcher for PolymarketClient {
    fn market_trades_url(&self, condition_id: &str, limit: u32, offset: u32) -> String {
        self.trades_url_any(None, Some(condition_id), limit, offset)
    }

    async fn fetch_market_trades_page(
        &self,
        condition_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiTrade>, Vec<u8>)> {
        let start = Instant::now();
        let res = self
            .fetch_trades_raw_any(None, Some(condition_id), limit, offset)
            .await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "market_trades").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "market_trades", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "market_trades", "status" => "error").increment(1);
                metrics::counter!(
                    "evaluator_api_errors_total",
                    "endpoint" => "market_trades",
                    "kind" => classify_anyhow_api_error(&e).as_str()
                )
                .increment(1);
                Err(e)
            }
        }
    }
}

impl crate::ingestion::TradesPager for PolymarketClient {
    fn trades_url(&self, user: &str, limit: u32, offset: u32) -> String {
        self.trades_url_any(Some(user), None, limit, offset)
    }

    async fn fetch_trades_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiTrade>, Vec<u8>)> {
        let start = Instant::now();
        let res = self
            .fetch_trades_raw_any(Some(user), None, limit, offset)
            .await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "user_trades").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "user_trades", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "user_trades", "status" => "error").increment(1);
                metrics::counter!(
                    "evaluator_api_errors_total",
                    "endpoint" => "user_trades",
                    "kind" => classify_anyhow_api_error(&e).as_str()
                )
                .increment(1);
                Err(e)
            }
        }
    }
}

impl ActivityPager for PolymarketClient {
    fn activity_url(&self, user: &str, limit: u32, offset: u32) -> String {
        format!(
            "{}/activity?user={user}&limit={limit}&offset={offset}",
            self.data_api_url()
        )
    }

    async fn fetch_activity_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiActivity>, Vec<u8>)> {
        let start = Instant::now();
        let res = self.fetch_activity_raw(user, limit, offset).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "activity").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "activity", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "activity", "status" => "error").increment(1);
                metrics::counter!(
                    "evaluator_api_errors_total",
                    "endpoint" => "activity",
                    "kind" => classify_anyhow_api_error(&e).as_str()
                )
                .increment(1);
                Err(e)
            }
        }
    }
}

impl PositionsPager for PolymarketClient {
    fn positions_url(&self, user: &str, limit: u32, offset: u32) -> String {
        format!(
            "{}/positions?user={user}&limit={limit}&offset={offset}",
            self.data_api_url()
        )
    }

    async fn fetch_positions_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiPosition>, Vec<u8>)> {
        let start = Instant::now();
        let res = self.fetch_positions_raw(user, limit, offset).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "positions").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "positions", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "positions", "status" => "error").increment(1);
                metrics::counter!(
                    "evaluator_api_errors_total",
                    "endpoint" => "positions",
                    "kind" => classify_anyhow_api_error(&e).as_str()
                )
                .increment(1);
                Err(e)
            }
        }
    }
}

impl LeaderboardFetcher for PolymarketClient {
    async fn fetch_leaderboard(
        &self,
        category: &str,
        time_period: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ApiLeaderboardEntry>> {
        let start = Instant::now();
        let res =
            PolymarketClient::fetch_leaderboard(self, category, time_period, limit, offset).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "leaderboard").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "leaderboard", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "leaderboard", "status" => "error").increment(1);
                metrics::counter!(
                    "evaluator_api_errors_total",
                    "endpoint" => "leaderboard",
                    "kind" => classify_anyhow_api_error(&e).as_str()
                )
                .increment(1);
                Err(e)
            }
        }
    }
}
