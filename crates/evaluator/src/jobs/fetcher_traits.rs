use anyhow::Result;
use common::polymarket::GammaFilter;
use common::types::{
    ApiActivity, ApiHolderResponse, ApiLeaderboardEntry, ApiPosition, ApiTrade, GammaMarket,
};

pub trait GammaMarketsPager {
    #[allow(dead_code)]
    fn gamma_markets_url(&self, limit: u32, offset: u32) -> String;
    fn fetch_gamma_markets_page(
        &self,
        limit: u32,
        offset: u32,
        filter: &GammaFilter,
    ) -> impl std::future::Future<Output = Result<(Vec<GammaMarket>, Vec<u8>)>> + Send;
}

pub trait HoldersFetcher {
    #[allow(dead_code)]
    fn holders_url(&self, condition_id: &str, limit: u32) -> String;
    fn fetch_holders(
        &self,
        condition_id: &str,
        limit: u32,
    ) -> impl std::future::Future<Output = Result<(Vec<ApiHolderResponse>, Vec<u8>)>> + Send;
}

pub trait MarketTradesFetcher {
    #[allow(dead_code)]
    fn market_trades_url(&self, condition_id: &str, limit: u32, offset: u32) -> String;
    fn fetch_market_trades_page(
        &self,
        condition_id: &str,
        limit: u32,
        offset: u32,
    ) -> impl std::future::Future<Output = Result<(Vec<ApiTrade>, Vec<u8>)>> + Send;
}

pub trait ActivityPager {
    #[allow(dead_code)]
    fn activity_url(&self, user: &str, limit: u32, offset: u32) -> String;
    fn fetch_activity_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> impl std::future::Future<Output = Result<(Vec<ApiActivity>, Vec<u8>)>> + Send;
}

pub trait PositionsPager {
    #[allow(dead_code)]
    fn positions_url(&self, user: &str, limit: u32, offset: u32) -> String;
    fn fetch_positions_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> impl std::future::Future<Output = Result<(Vec<ApiPosition>, Vec<u8>)>> + Send;
}

pub trait LeaderboardFetcher {
    fn fetch_leaderboard(
        &self,
        category: &str,
        time_period: &str,
        limit: u32,
        offset: u32,
    ) -> impl std::future::Future<Output = Result<Vec<ApiLeaderboardEntry>>> + Send;
}
