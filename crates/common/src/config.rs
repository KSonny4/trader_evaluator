use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub general: General,
    pub database: Database,
    pub risk: Risk,
    pub market_scoring: MarketScoring,
    pub wallet_discovery: WalletDiscovery,
    pub ingestion: Ingestion,
    pub paper_trading: PaperTrading,
    pub wallet_scoring: WalletScoring,
    pub observability: Observability,
    pub polymarket: Polymarket,
}

#[derive(Debug, Deserialize)]
pub struct General {
    pub mode: String,
    pub log_level: String,
}

#[derive(Debug, Deserialize)]
pub struct Database {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct Risk {
    pub max_exposure_per_market_pct: f64,
    pub max_exposure_per_wallet_pct: f64,
    pub max_daily_trades: u32,
    pub slippage_pct: f64,
    pub no_chase_adverse_move_pct: f64,
    pub portfolio_stop_drawdown_pct: f64,
    pub paper_bankroll_usdc: f64,
}

#[derive(Debug, Deserialize)]
pub struct MarketScoring {
    pub top_n_markets: usize,
    pub min_liquidity_usdc: f64,
    pub min_daily_volume_usdc: f64,
    pub min_daily_trades: u32,
    pub min_unique_traders: u32,
    pub max_days_to_expiry: u32,
    pub min_days_to_expiry: u32,
    pub refresh_interval_secs: u64,
    pub weights_liquidity: f64,
    pub weights_volume: f64,
    pub weights_density: f64,
    pub weights_whale_concentration: f64,
    pub weights_time_to_expiry: f64,
}

#[derive(Debug, Deserialize)]
pub struct WalletDiscovery {
    pub min_total_trades: u32,
    pub max_wallets_per_market: usize,
    pub holders_per_market: usize,
    pub refresh_interval_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct Ingestion {
    pub trades_poll_interval_secs: u64,
    pub activity_poll_interval_secs: u64,
    pub positions_poll_interval_secs: u64,
    pub holders_poll_interval_secs: u64,
    pub rate_limit_delay_ms: u64,
    pub max_retries: u32,
    pub backoff_base_ms: u64,
}

#[derive(Debug, Deserialize)]
pub struct PaperTrading {
    pub strategies: Vec<String>,
    pub mirror_delay_secs: u64,
    pub position_size_usdc: f64,
}

#[derive(Debug, Deserialize)]
pub struct WalletScoring {
    pub windows_days: Vec<u32>,
    pub min_trades_for_score: u32,
    pub edge_weight: f64,
    pub consistency_weight: f64,
    pub market_skill_weight: f64,
    pub timing_skill_weight: f64,
    pub behavior_quality_weight: f64,
}

#[derive(Debug, Deserialize)]
pub struct Observability {
    pub prometheus_port: u16,
}

#[derive(Debug, Deserialize)]
pub struct Polymarket {
    pub data_api_url: String,
    pub gamma_api_url: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        let content = std::fs::read_to_string("config/default.toml")?;
        Self::from_str(&content)
    }

    pub fn from_str(s: &str) -> Result<Self> {
        Ok(toml::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_default_config() {
        let config = Config::from_str(include_str!("../../../config/default.toml")).unwrap();
        assert_eq!(config.general.mode, "paper");
        assert!(config.risk.max_exposure_per_market_pct > 0.0);
        assert!(config.ingestion.trades_poll_interval_secs > 0);
    }
}

