use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct TraderConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub polymarket: PolymarketConfig,
    #[allow(dead_code)]
    pub evaluator: EvaluatorConfig,
    pub trading: TradingConfig,
    pub risk: RiskConfig,
    pub fillability: FillabilityConfig,
    #[allow(dead_code)]
    pub observability: ObservabilityConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolymarketConfig {
    pub data_api_url: String,
    #[allow(dead_code)]
    pub gamma_api_url: String,
    pub rate_limit_delay_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // Deserialized from TOML, not yet used in code
pub struct EvaluatorConfig {
    pub api_url: String,
    pub poll_interval_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TradingConfig {
    pub bankroll_usd: f64,
    pub per_trade_size_usd: f64,
    pub use_proportional_sizing: bool,
    pub default_their_bankroll_usd: f64,
    #[allow(dead_code)]
    pub mirror_delay_secs: u64,
    pub slippage_default_cents: f64,
    // TODO: We might want 100ms delay for ALL Data API calls, or use a proxy to avoid rate limiting
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RiskConfig {
    pub portfolio: PortfolioRiskConfig,
    pub per_wallet: PerWalletRiskConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PortfolioRiskConfig {
    pub max_total_exposure_pct: f64,
    pub max_daily_loss_pct: f64,
    pub max_weekly_loss_pct: f64,
    pub max_concurrent_positions: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PerWalletRiskConfig {
    pub max_exposure_pct: f64,
    pub daily_loss_pct: f64,
    pub weekly_loss_pct: f64,
    pub max_drawdown_pct: f64,
    pub min_copy_fidelity_pct: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FillabilityConfig {
    pub enabled: bool,
    pub window_secs: u64,
    pub clob_ws_url: String,
    pub max_concurrent_recordings: usize,
}

impl Default for FillabilityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_secs: 120,
            clob_ws_url: "wss://ws-subscriptions-clob.polymarket.com/ws/market".to_string(),
            max_concurrent_recordings: 20,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // Deserialized from TOML, not yet used in code
pub struct ObservabilityConfig {
    pub prometheus_port: u16,
}

impl TraderConfig {
    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {path}"))?;
        Self::from_str(&content)
    }

    pub fn from_str(content: &str) -> Result<Self> {
        let config: TraderConfig =
            toml::from_str(content).context("failed to parse trader config")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.server.port > 0, "server.port must be > 0");
        anyhow::ensure!(
            self.trading.bankroll_usd > 0.0,
            "trading.bankroll_usd must be > 0"
        );
        anyhow::ensure!(
            self.trading.per_trade_size_usd > 0.0,
            "trading.per_trade_size_usd must be > 0"
        );
        anyhow::ensure!(
            self.risk.portfolio.max_total_exposure_pct > 0.0
                && self.risk.portfolio.max_total_exposure_pct <= 100.0,
            "risk.portfolio.max_total_exposure_pct must be in (0, 100]"
        );
        anyhow::ensure!(
            self.risk.per_wallet.max_exposure_pct > 0.0
                && self.risk.per_wallet.max_exposure_pct <= 100.0,
            "risk.per_wallet.max_exposure_pct must be in (0, 100]"
        );
        Ok(())
    }

    pub fn default_config_path() -> String {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf));

        // Check next to the binary first
        if let Some(dir) = &exe_dir {
            let candidate = dir.join("trader.toml");
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }

        // Check config/ directory relative to cwd
        let candidate = Path::new("config/trader.toml");
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }

        // Check crates/trader/config/ (development)
        let candidate = Path::new("crates/trader/config/trader.toml");
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }

        // Fallback
        "config/trader.toml".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> &'static str {
        r#"
[server]
port = 8081
host = "0.0.0.0"

[database]
path = "data/trader.db"

[polymarket]
data_api_url = "https://data-api.polymarket.com"
gamma_api_url = "https://gamma-api.polymarket.com"
rate_limit_delay_ms = 200

[evaluator]
api_url = "http://localhost:8080"
poll_interval_secs = 3600

[trading]
bankroll_usd = 1000.0
per_trade_size_usd = 25.0
use_proportional_sizing = true
default_their_bankroll_usd = 5000.0
mirror_delay_secs = 0
slippage_default_cents = 1.0
poll_interval_ms = 100

[risk.portfolio]
max_total_exposure_pct = 15.0
max_daily_loss_pct = 3.0
max_weekly_loss_pct = 8.0
max_concurrent_positions = 20

[risk.per_wallet]
max_exposure_pct = 5.0
daily_loss_pct = 2.0
weekly_loss_pct = 5.0
max_drawdown_pct = 15.0
min_copy_fidelity_pct = 80.0

[fillability]
enabled = true
window_secs = 120
clob_ws_url = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
max_concurrent_recordings = 20

[observability]
prometheus_port = 9095
"#
    }

    #[test]
    fn test_parse_valid_config() {
        let config = TraderConfig::from_str(sample_config()).unwrap();
        assert_eq!(config.server.port, 8081);
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.database.path, "data/trader.db");
        assert_eq!(
            config.polymarket.data_api_url,
            "https://data-api.polymarket.com"
        );
        assert_eq!(config.trading.bankroll_usd, 1000.0);
        assert_eq!(config.trading.per_trade_size_usd, 25.0);
        assert!(config.trading.use_proportional_sizing);
        assert_eq!(config.risk.portfolio.max_total_exposure_pct, 15.0);
        assert_eq!(config.risk.per_wallet.max_exposure_pct, 5.0);
        assert_eq!(config.observability.prometheus_port, 9095);
    }

    #[test]
    fn test_parse_invalid_config_missing_field() {
        let bad = "
[server]
port = 8081
";
        let result = TraderConfig::from_str(bad);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_zero_bankroll() {
        let content = sample_config().replace("bankroll_usd = 1000.0", "bankroll_usd = 0.0");
        let result = TraderConfig::from_str(&content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("bankroll_usd must be > 0"));
    }

    #[test]
    fn test_validate_exposure_out_of_range() {
        let content = sample_config().replace(
            "max_total_exposure_pct = 15.0",
            "max_total_exposure_pct = 150.0",
        );
        let result = TraderConfig::from_str(&content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_total_exposure_pct must be in (0, 100]"));
    }

    #[test]
    fn test_load_from_file() {
        let config = TraderConfig::load("config/trader.toml").unwrap();
        assert_eq!(config.server.port, 8081);
    }
}
