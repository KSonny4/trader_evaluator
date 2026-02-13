use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Buy,
    Sell,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Buy => write!(f, "BUY"),
            Self::Sell => write!(f, "SELL"),
        }
    }
}

impl Side {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "BUY" => Some(Self::Buy),
            "SELL" => Some(Self::Sell),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Domain type used in tests and for future API responses
pub enum TradeStatus {
    Open,
    SettledWin,
    SettledLoss,
}

impl fmt::Display for TradeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::SettledWin => write!(f, "settled_win"),
            Self::SettledLoss => write!(f, "settled_loss"),
        }
    }
}

impl TradeStatus {
    #[allow(dead_code)] // Used in tests
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "settled_win" => Some(Self::SettledWin),
            "settled_loss" => Some(Self::SettledLoss),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletStatus {
    Active,
    Paused,
    Killed,
    Removed,
}

impl fmt::Display for WalletStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Paused => write!(f, "paused"),
            Self::Killed => write!(f, "killed"),
            Self::Removed => write!(f, "removed"),
        }
    }
}

impl WalletStatus {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "killed" => Some(Self::Killed),
            "removed" => Some(Self::Removed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradingMode {
    Paper,
    Live,
}

impl fmt::Display for TradingMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Paper => write!(f, "paper"),
            Self::Live => write!(f, "live"),
        }
    }
}

impl TradingMode {
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s {
            "paper" => Some(Self::Paper),
            "live" => Some(Self::Live),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FidelityOutcome {
    Copied,
    SkippedPortfolioRisk,
    SkippedWalletRisk,
    SkippedDailyLoss,
    SkippedWeeklyLoss,
    SkippedMarketClosed,
    SkippedDetectionLag,
    SkippedNoFill,
}

impl fmt::Display for FidelityOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Copied => write!(f, "COPIED"),
            Self::SkippedPortfolioRisk => write!(f, "SKIPPED_PORTFOLIO_RISK"),
            Self::SkippedWalletRisk => write!(f, "SKIPPED_WALLET_RISK"),
            Self::SkippedDailyLoss => write!(f, "SKIPPED_DAILY_LOSS"),
            Self::SkippedWeeklyLoss => write!(f, "SKIPPED_WEEKLY_LOSS"),
            Self::SkippedMarketClosed => write!(f, "SKIPPED_MARKET_CLOSED"),
            Self::SkippedDetectionLag => write!(f, "SKIPPED_DETECTION_LAG"),
            Self::SkippedNoFill => write!(f, "SKIPPED_NO_FILL"),
        }
    }
}

/// A trade observed on Polymarket for a followed wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // Domain type for future API/display use
pub struct ObservedTrade {
    pub proxy_wallet: String,
    pub condition_id: String,
    pub side: Side,
    pub outcome: Option<String>,
    pub outcome_index: Option<i32>,
    pub price: f64,
    pub size_usd: f64,
    pub timestamp: i64,
    pub trade_hash: String,
}

/// A paper/live trade we executed in response to an observed trade.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // Domain type for future API/display use
pub struct ExecutedTrade {
    pub id: i64,
    pub proxy_wallet: String,
    pub condition_id: String,
    pub side: Side,
    pub outcome: Option<String>,
    pub outcome_index: Option<i32>,
    pub their_price: f64,
    pub their_size_usd: f64,
    pub their_trade_hash: String,
    pub their_timestamp: i64,
    pub our_size_usd: f64,
    pub our_entry_price: f64,
    pub slippage_applied: f64,
    pub fee_applied: f64,
    pub sizing_method: String,
    pub detection_delay_ms: i64,
    pub trading_mode: TradingMode,
    pub status: TradeStatus,
    pub exit_price: Option<f64>,
    pub pnl: Option<f64>,
    pub settled_at: Option<String>,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_side_display_and_parse() {
        assert_eq!(Side::Buy.to_string(), "BUY");
        assert_eq!(Side::Sell.to_string(), "SELL");
        assert_eq!(Side::from_str_loose("BUY"), Some(Side::Buy));
        assert_eq!(Side::from_str_loose("buy"), Some(Side::Buy));
        assert_eq!(Side::from_str_loose("nope"), None);
    }

    #[test]
    fn test_trade_status_display_and_parse() {
        assert_eq!(TradeStatus::Open.to_string(), "open");
        assert_eq!(TradeStatus::SettledWin.to_string(), "settled_win");
        assert_eq!(
            TradeStatus::from_str_loose("settled_loss"),
            Some(TradeStatus::SettledLoss)
        );
        assert_eq!(TradeStatus::from_str_loose("unknown"), None);
    }

    #[test]
    fn test_wallet_status_roundtrip() {
        for status in [
            WalletStatus::Active,
            WalletStatus::Paused,
            WalletStatus::Killed,
            WalletStatus::Removed,
        ] {
            let s = status.to_string();
            assert_eq!(WalletStatus::from_str_loose(&s), Some(status));
        }
    }

    #[test]
    fn test_trading_mode_roundtrip() {
        assert_eq!(TradingMode::Paper.to_string(), "paper");
        assert_eq!(TradingMode::Live.to_string(), "live");
        assert_eq!(
            TradingMode::from_str_loose("paper"),
            Some(TradingMode::Paper)
        );
        assert_eq!(TradingMode::from_str_loose("live"), Some(TradingMode::Live));
        assert_eq!(TradingMode::from_str_loose("demo"), None);
    }

    #[test]
    fn test_fidelity_outcome_display() {
        assert_eq!(FidelityOutcome::Copied.to_string(), "COPIED");
        assert_eq!(
            FidelityOutcome::SkippedPortfolioRisk.to_string(),
            "SKIPPED_PORTFOLIO_RISK"
        );
        assert_eq!(
            FidelityOutcome::SkippedDailyLoss.to_string(),
            "SKIPPED_DAILY_LOSS"
        );
    }

    #[test]
    fn test_side_serde_roundtrip() {
        let buy_json = serde_json::to_string(&Side::Buy).unwrap();
        assert_eq!(buy_json, "\"BUY\"");
        let parsed: Side = serde_json::from_str(&buy_json).unwrap();
        assert_eq!(parsed, Side::Buy);
    }
}
