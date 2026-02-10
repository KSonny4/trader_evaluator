use anyhow::Result;
use serde::Deserialize;
use std::str::FromStr;

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
    pub personas: Personas,
    pub anomaly: Anomaly,
    pub web: Option<Web>,
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
    // Two-level risk: per-wallet
    pub per_wallet_daily_loss_pct: f64,
    pub per_wallet_weekly_loss_pct: f64,
    pub per_wallet_max_drawdown_pct: f64,
    pub per_wallet_max_slippage_vs_edge: f64,
    // Two-level risk: portfolio
    pub portfolio_daily_loss_pct: f64,
    pub portfolio_weekly_loss_pct: f64,
    pub max_concurrent_positions: u32,
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
    // Copy fidelity and risk
    pub bankroll_usd: f64,
    pub max_total_exposure_pct: f64,
    pub max_daily_loss_pct: f64,
    pub min_copy_fidelity_pct: f64,
    pub per_trade_size_usd: f64,
    pub slippage_default_cents: f64,
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

#[derive(Debug, Deserialize, Clone)]
pub struct Web {
    pub port: u16,
    pub host: String,
    pub auth_password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Personas {
    // Stage 1 fast filters
    pub stage1_min_wallet_age_days: u32,
    pub stage1_min_total_trades: u32,
    pub stage1_max_inactive_days: u32,
    /// Proxy wallet addresses to exclude as known bots (Strategy Bible §4 Stage 1). E.g. ["0x..."].
    #[serde(default)]
    pub known_bots: Vec<String>,
    // Informed Specialist
    pub specialist_max_active_positions: u32,
    pub specialist_min_concentration: f64,
    pub specialist_min_win_rate: f64,
    // Consistent Generalist
    pub generalist_min_markets: u32,
    pub generalist_min_win_rate: f64,
    pub generalist_max_win_rate: f64,
    pub generalist_max_drawdown: f64,
    pub generalist_min_sharpe: f64,
    // Patient Accumulator
    pub accumulator_min_hold_hours: f64,
    pub accumulator_max_trades_per_week: f64,
    // Execution Master (exclusion)
    pub execution_master_pnl_ratio: f64,
    // Tail Risk Seller (exclusion)
    pub tail_risk_min_win_rate: f64,
    pub tail_risk_loss_multiplier: f64,
    // Noise Trader (exclusion)
    pub noise_max_trades_per_week: f64,
    pub noise_max_abs_roi: f64,
    // Sniper/Insider (exclusion)
    pub sniper_max_age_days: u32,
    pub sniper_min_win_rate: f64,
    pub sniper_max_trades: u32,
    // Trust multipliers
    pub trust_30_90_multiplier: f64,
    pub obscurity_bonus_multiplier: f64,
}

#[derive(Debug, Deserialize)]
pub struct Anomaly {
    pub win_rate_drop_pct: f64,
    pub max_weekly_drawdown_pct: f64,
    pub frequency_change_multiplier: f64,
    pub size_change_multiplier: f64,
}

impl Config {
    pub fn load() -> Result<Self> {
        let content = std::fs::read_to_string("config/default.toml")?;
        Self::from_toml_str(&content)
    }

    pub fn from_toml_str(s: &str) -> Result<Self> {
        Ok(toml::from_str(s)?)
    }
}

impl FromStr for Config {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::from_toml_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_default_config() {
        let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        assert_eq!(config.general.mode, "paper");
        assert!(config.risk.max_exposure_per_market_pct > 0.0);
        assert!(config.ingestion.trades_poll_interval_secs > 0);
    }

    #[test]
    fn test_web_config_section() {
        let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        let web = config.web.expect("web section should be present");
        assert_eq!(web.port, 8080);
        assert_eq!(web.host, "127.0.0.1");
        assert!(web.auth_password.is_some());
    }

    #[test]
    fn test_persona_config_loads() {
        let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        assert_eq!(config.personas.stage1_min_wallet_age_days, 30);
        assert_eq!(config.personas.stage1_min_total_trades, 10);
        assert!(config.personas.specialist_min_win_rate > 0.0);
        assert!(config.personas.generalist_min_sharpe > 0.0);
        assert!(config.personas.execution_master_pnl_ratio > 0.0);
        assert!(config.personas.trust_30_90_multiplier > 0.0);
        assert!(config.personas.obscurity_bonus_multiplier > 1.0);
    }

    #[test]
    fn test_risk_v2_config_loads() {
        let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        assert!(config.risk.per_wallet_daily_loss_pct > 0.0);
        assert!(config.risk.per_wallet_weekly_loss_pct > 0.0);
        assert!(config.risk.per_wallet_max_drawdown_pct > 0.0);
        assert!(config.risk.portfolio_daily_loss_pct > 0.0);
        assert!(config.risk.portfolio_weekly_loss_pct > 0.0);
        assert!(config.risk.max_concurrent_positions > 0);
    }

    #[test]
    fn test_copy_fidelity_config_loads() {
        let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        assert!(config.paper_trading.min_copy_fidelity_pct > 0.0);
        assert!(config.paper_trading.bankroll_usd > 0.0);
        assert!(config.paper_trading.max_total_exposure_pct > 0.0);
        assert!(config.paper_trading.max_daily_loss_pct > 0.0);
    }

    #[test]
    fn test_anomaly_config_loads() {
        let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        assert!(config.anomaly.win_rate_drop_pct > 0.0);
        assert!(config.anomaly.max_weekly_drawdown_pct > 0.0);
        assert!(config.anomaly.frequency_change_multiplier > 1.0);
        assert!(config.anomaly.size_change_multiplier > 1.0);
    }

    #[test]
    fn test_web_config_optional() {
        // Config without [web] section should still parse
        let toml = r#"
[general]
mode = "paper"
log_level = "info"

[database]
path = "data/evaluator.db"

[risk]
max_exposure_per_market_pct = 10.0
max_exposure_per_wallet_pct = 5.0
max_daily_trades = 100
slippage_pct = 1.0
no_chase_adverse_move_pct = 5.0
portfolio_stop_drawdown_pct = 15.0
paper_bankroll_usdc = 1000.0
per_wallet_daily_loss_pct = 2.0
per_wallet_weekly_loss_pct = 5.0
per_wallet_max_drawdown_pct = 15.0
per_wallet_max_slippage_vs_edge = 1.0
portfolio_daily_loss_pct = 3.0
portfolio_weekly_loss_pct = 8.0
max_concurrent_positions = 20

[market_scoring]
top_n_markets = 20
min_liquidity_usdc = 1000.0
min_daily_volume_usdc = 5000.0
min_daily_trades = 20
min_unique_traders = 10
max_days_to_expiry = 90
min_days_to_expiry = 1
refresh_interval_secs = 86400
weights_liquidity = 0.25
weights_volume = 0.25
weights_density = 0.20
weights_whale_concentration = 0.15
weights_time_to_expiry = 0.15

[wallet_discovery]
min_total_trades = 5
max_wallets_per_market = 100
holders_per_market = 20
refresh_interval_secs = 86400

[ingestion]
trades_poll_interval_secs = 3600
activity_poll_interval_secs = 21600
positions_poll_interval_secs = 86400
holders_poll_interval_secs = 86400
rate_limit_delay_ms = 200
max_retries = 3
backoff_base_ms = 1000

[paper_trading]
strategies = ["mirror"]
mirror_delay_secs = 0
position_size_usdc = 25.0
bankroll_usd = 1000.0
max_total_exposure_pct = 15.0
max_daily_loss_pct = 3.0
min_copy_fidelity_pct = 80.0
per_trade_size_usd = 25.0
slippage_default_cents = 1.0

[wallet_scoring]
windows_days = [7, 30, 90]
min_trades_for_score = 10
edge_weight = 0.30
consistency_weight = 0.25
market_skill_weight = 0.20
timing_skill_weight = 0.15
behavior_quality_weight = 0.10

[observability]
prometheus_port = 9094

[polymarket]
data_api_url = "https://data-api.polymarket.com"
gamma_api_url = "https://gamma-api.polymarket.com"

[personas]
stage1_min_wallet_age_days = 30
stage1_min_total_trades = 10
stage1_max_inactive_days = 30
specialist_max_active_positions = 5
specialist_min_concentration = 0.60
specialist_min_win_rate = 0.60
generalist_min_markets = 20
generalist_min_win_rate = 0.52
generalist_max_win_rate = 0.60
generalist_max_drawdown = 15.0
generalist_min_sharpe = 1.0
accumulator_min_hold_hours = 48.0
accumulator_max_trades_per_week = 5.0
execution_master_pnl_ratio = 0.70
tail_risk_min_win_rate = 0.80
tail_risk_loss_multiplier = 5.0
noise_max_trades_per_week = 50.0
noise_max_abs_roi = 0.02
sniper_max_age_days = 30
sniper_min_win_rate = 0.85
sniper_max_trades = 20
trust_30_90_multiplier = 0.8
obscurity_bonus_multiplier = 1.2

[anomaly]
win_rate_drop_pct = 15.0
max_weekly_drawdown_pct = 20.0
frequency_change_multiplier = 3.0
size_change_multiplier = 10.0
"#;
        let config = Config::from_toml_str(toml).unwrap();
        assert!(config.web.is_none());
    }
}
