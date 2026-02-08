use serde::{Deserialize, Serialize};

fn de_opt_string_any<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct OptStringAnyVisitor;

    impl<'de> serde::de::Visitor<'de> for OptStringAnyVisitor {
        type Value = Option<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("null, string, number, or bool")
        }

        fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            de_opt_string_any(deserializer)
        }

        fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(v.to_string()))
        }

        fn visit_string<E>(self, v: String) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(v))
        }

        fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(v.to_string()))
        }

        fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(v.to_string()))
        }

        fn visit_f64<E>(self, v: f64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(v.to_string()))
        }

        fn visit_bool<E>(self, v: bool) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(v.to_string()))
        }
    }

    deserializer.deserialize_any(OptStringAnyVisitor)
}

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
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GammaMarket {
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub title: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string_any")]
    pub liquidity: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string_any")]
    pub volume: Option<String>,
    pub category: Option<String>,
    #[serde(rename = "eventSlug")]
    pub event_slug: Option<String>,
}

/// Trade from Data API /trades.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiTrade {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub asset: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string_any")]
    pub size: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string_any")]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiHolderResponse {
    pub token: Option<String>,
    pub holders: Vec<ApiHolder>,
}

/// Activity from Data API /activity.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiActivity {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    #[serde(rename = "type")]
    pub activity_type: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string_any")]
    pub size: Option<String>,
    #[serde(rename = "usdcSize")]
    #[serde(default, deserialize_with = "de_opt_string_any")]
    pub usdc_size: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string_any")]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiPosition {
    #[serde(rename = "proxyWallet")]
    pub proxy_wallet: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: Option<String>,
    pub asset: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string_any")]
    pub size: Option<String>,
    #[serde(rename = "avgPrice")]
    #[serde(default, deserialize_with = "de_opt_string_any")]
    pub avg_price: Option<String>,
    #[serde(rename = "currentValue")]
    #[serde(default, deserialize_with = "de_opt_string_any")]
    pub current_value: Option<String>,
    #[serde(rename = "cashPnl")]
    #[serde(default, deserialize_with = "de_opt_string_any")]
    pub cash_pnl: Option<String>,
    #[serde(rename = "percentPnl")]
    #[serde(default, deserialize_with = "de_opt_string_any")]
    pub percent_pnl: Option<String>,
    pub outcome: Option<String>,
    #[serde(rename = "outcomeIndex")]
    pub outcome_index: Option<i32>,
}

/// Leaderboard entry from Data API /v1/leaderboard.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiLeaderboardEntry {
    #[serde(default, deserialize_with = "de_opt_string_any")]
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
