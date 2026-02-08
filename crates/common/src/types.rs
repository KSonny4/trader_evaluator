use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverySource {
    Holder,
    TraderRecent,
    Leaderboard,
}

impl DiscoverySource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Holder => "HOLDER",
            Self::TraderRecent => "TRADER_RECENT",
            Self::Leaderboard => "LEADERBOARD",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperTradeStatus {
    Open,
    SettledWin,
    SettledLoss,
}

impl PaperTradeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::SettledWin => "settled_win",
            Self::SettledLoss => "settled_loss",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyStrategy {
    Mirror,
    Delay,
    Consensus,
}

impl CopyStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mirror => "mirror",
            Self::Delay => "delay",
            Self::Consensus => "consensus",
        }
    }
}

/// Market from Gamma API.
#[derive(Debug, Clone, Deserialize)]
pub struct GammaMarket {
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub title: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,
    pub liquidity: Option<String>,
    pub volume: Option<String>,
    pub category: Option<String>,
    #[serde(rename = "eventSlug")]
    pub event_slug: Option<String>,
}

/// Trade from Data API /trades.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiTrade {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub asset: Option<String>,
    pub size: Option<String>,
    pub price: Option<String>,
    pub timestamp: Option<i64>,
    pub title: Option<String>,
    pub slug: Option<String>,
    pub outcome: Option<String>,
    #[serde(rename = "outcomeIndex")]
    pub outcome_index: Option<i32>,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
    pub side: Option<String>,
    pub pseudonym: Option<String>,
    pub name: Option<String>,
}

/// Holder from Data API /holders.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiHolder {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    pub amount: Option<f64>,
    pub asset: Option<String>,
    pub pseudonym: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "outcomeIndex")]
    pub outcome_index: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiHolderResponse {
    pub token: Option<String>,
    pub holders: Vec<ApiHolder>,
}

/// Activity from Data API /activity.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiActivity {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    #[serde(rename = "type")]
    pub activity_type: Option<String>,
    pub size: Option<String>,
    #[serde(rename = "usdcSize")]
    pub usdc_size: Option<String>,
    pub price: Option<String>,
    pub side: Option<String>,
    pub outcome: Option<String>,
    #[serde(rename = "outcomeIndex")]
    pub outcome_index: Option<i32>,
    pub timestamp: Option<i64>,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
}

/// Position from Data API /positions.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiPosition {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub asset: Option<String>,
    pub size: Option<String>,
    #[serde(rename = "avgPrice")]
    pub avg_price: Option<String>,
    #[serde(rename = "currentValue")]
    pub current_value: Option<String>,
    #[serde(rename = "cashPnl")]
    pub cash_pnl: Option<String>,
    #[serde(rename = "percentPnl")]
    pub percent_pnl: Option<String>,
    pub outcome: Option<String>,
    #[serde(rename = "outcomeIndex")]
    pub outcome_index: Option<i32>,
}

/// Leaderboard entry from Data API /v1/leaderboard.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiLeaderboardEntry {
    pub rank: Option<String>,
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "userName")]
    pub user_name: Option<String>,
    pub vol: Option<f64>,
    pub pnl: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_source_display() {
        assert_eq!(DiscoverySource::Holder.as_str(), "HOLDER");
        assert_eq!(DiscoverySource::TraderRecent.as_str(), "TRADER_RECENT");
        assert_eq!(DiscoverySource::Leaderboard.as_str(), "LEADERBOARD");
    }

    #[test]
    fn test_paper_trade_status() {
        assert_eq!(PaperTradeStatus::Open.as_str(), "open");
        assert_eq!(PaperTradeStatus::SettledWin.as_str(), "settled_win");
    }
}

