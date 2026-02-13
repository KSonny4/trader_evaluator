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
    pub wallet_rules: WalletRules,
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
    #[serde(alias = "top_n_markets")]
    pub top_n_events: usize,
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
    pub holders_per_market: usize,
    /// Max markets to process per discovery run (batched to avoid long bootstrap).
    #[serde(default = "default_markets_per_discovery_run")]
    pub markets_per_discovery_run: usize,
    pub refresh_interval_secs: u64,
    /// Number of pages of 200 trades to fetch per market (offset 0, 200, 400, ...). Cap at 15 (API offset ~3000).
    #[serde(default = "default_trades_pages_per_market")]
    pub trades_pages_per_market: u32,
    /// "continuous" = run discovery in loop (rate limit only); "scheduled" = use refresh_interval_secs.
    #[serde(default = "default_wallet_discovery_mode")]
    pub wallet_discovery_mode: String,
    #[serde(default)]
    pub leaderboard: WalletDiscoveryLeaderboard,
}

fn default_trades_pages_per_market() -> u32 {
    15
}

fn default_markets_per_discovery_run() -> usize {
    20
}

fn default_wallet_discovery_mode() -> String {
    "scheduled".to_string()
}

#[derive(Debug, Deserialize, Default)]
pub struct WalletDiscoveryLeaderboard {
    #[serde(default)]
    pub enabled: bool,
    /// Categories: OVERALL, POLITICS, SPORTS, CRYPTO, CULTURE, ECONOMICS, TECH, FINANCE, etc.
    #[serde(default = "default_leaderboard_categories")]
    pub categories: Vec<String>,
    /// Time periods: DAY, WEEK, MONTH, ALL
    #[serde(default = "default_leaderboard_time_periods")]
    pub time_periods: Vec<String>,
    #[serde(default = "default_leaderboard_pages_per_category")]
    pub pages_per_category: u32,
}

fn default_leaderboard_categories() -> Vec<String> {
    vec![
        "OVERALL".to_string(),
        "POLITICS".to_string(),
        "CRYPTO".to_string(),
    ]
}

fn default_leaderboard_time_periods() -> Vec<String> {
    vec!["WEEK".to_string(), "MONTH".to_string()]
}

fn default_leaderboard_pages_per_category() -> u32 {
    20
}

fn default_wallets_per_ingestion_run() -> u32 {
    20
}

fn default_parallel_enabled() -> bool {
    true
}

fn default_parallel_tasks() -> usize {
    8
}

#[derive(Debug, Deserialize)]
pub struct Ingestion {
    #[serde(default = "default_wallets_per_ingestion_run")]
    pub wallets_per_ingestion_run: u32,
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
    pub mirror_use_proportional_sizing: bool,
    pub mirror_default_their_bankroll_usd: f64,
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
    /// Enable parallel classification (default: true)
    #[serde(default = "default_parallel_enabled")]
    pub parallel_enabled: bool,
    /// Number of parallel tasks per chunk (default: 8)
    #[serde(default = "default_parallel_tasks")]
    pub parallel_tasks: usize,
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
    pub accumulator_min_roi: f64,
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
    // A/B/F/G exclusions + D/C/E trait thresholds
    pub news_sniper_max_burstiness_top_1h_ratio: f64,
    pub liquidity_provider_min_buy_sell_balance: f64,
    pub liquidity_provider_min_mid_fill_ratio: f64,
    pub bot_swarm_min_trades_per_day: f64,
    pub bot_swarm_max_avg_trade_size_usdc: f64,
    pub jackpot_min_pnl_top1_share: f64,
    pub jackpot_max_win_rate: f64,
    #[serde(alias = "topic_lane_min_top_category_ratio")]
    pub topic_lane_min_top_domain_ratio: f64,
    pub bonder_min_extreme_price_ratio: f64,
    pub whale_min_avg_trade_size_usdc: f64,
    /// Stage 2 ROI gate: minimum ROI (win rate + PnL combo) for followable personas. 0 = disabled.
    #[serde(default)]
    pub stage2_min_roi: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WalletRules {
    // Discovery
    pub min_trades_for_discovery: usize,
    pub max_trades_per_day: f64,
    pub max_distinct_markets_30d: usize,
    pub min_median_hold_minutes: f64,
    pub max_flip_rate: f64,
    pub max_size_gini: f64,
    pub min_liquidity_score: f64,
    pub max_median_seconds_between_trades: f64,
    pub max_fraction_trades_at_spread_edge: f64,
    // Paper
    pub paper_window_days: u32,
    pub required_paper_trades: usize,
    pub min_paper_profit_per_trade: f64,
    pub max_paper_drawdown: f64,
    pub max_paper_slippage_bps: f64,
    // Live
    pub live_breakers_enabled: bool,
    pub live_max_drawdown: f64,
    pub live_slippage_bps_spike: f64,
    pub live_style_drift_score: f64,
    pub live_inactivity_days: u32,
    pub live_max_theme_concentration: f64,
    pub live_max_correlation_cluster_exposure: f64,
    // Risk caps
    pub per_trade_risk_cap: f64,
    pub per_market_risk_cap: f64,
    pub per_wallet_risk_cap: f64,
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
        assert_eq!(config.personas.stage1_min_wallet_age_days, 45);
        assert_eq!(config.personas.stage1_max_inactive_days, 45);
        assert_eq!(config.personas.stage1_min_total_trades, 10);
        assert!(config.personas.specialist_min_win_rate > 0.0);
        assert!(config.personas.generalist_min_sharpe > 0.0);
        assert!(config.personas.execution_master_pnl_ratio > 0.0);
        assert!(config.personas.trust_30_90_multiplier > 0.0);
        assert!(config.personas.obscurity_bonus_multiplier > 1.0);
        assert!(config.personas.news_sniper_max_burstiness_top_1h_ratio > 0.0);
        assert!(config.personas.liquidity_provider_min_mid_fill_ratio > 0.0);
        assert!(config.personas.bot_swarm_min_trades_per_day > 0.0);
        assert!(config.personas.jackpot_min_pnl_top1_share > 0.0);
        assert!(config.personas.topic_lane_min_top_domain_ratio > 0.0);
        assert!(config.personas.bonder_min_extreme_price_ratio > 0.0);
        assert!(config.personas.whale_min_avg_trade_size_usdc > 0.0);
        assert!(config.personas.accumulator_min_roi > 0.0);
    }

    #[test]
    fn test_wallet_rules_config_loads() {
        let config = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        assert!(config.wallet_rules.min_trades_for_discovery > 0);
        assert!(config.wallet_rules.max_trades_per_day > 0.0);
        assert!(config.wallet_rules.paper_window_days > 0);
        assert!(config.wallet_rules.required_paper_trades > 0);
        assert!(config.wallet_rules.live_max_drawdown > 0.0);
        assert!(config.wallet_rules.live_inactivity_days > 0);
        assert!(!config.wallet_rules.live_breakers_enabled);
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
    fn test_personas_parallelization_config_loads() {
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
top_n_events = 50
min_liquidity_usdc = 1000.0
min_daily_volume_usdc = 5000.0
min_daily_trades = 20
min_unique_traders = 10
max_days_to_expiry = 90
min_days_to_expiry = 1
refresh_interval_secs = 3600
weights_liquidity = 0.25
weights_volume = 0.25
weights_density = 0.20
weights_whale_concentration = 0.15
weights_time_to_expiry = 0.15

[wallet_discovery]
min_total_trades = 5
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
mirror_use_proportional_sizing = true
mirror_default_their_bankroll_usd = 5000

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
stage1_min_total_trades = 10
stage1_min_wallet_age_days = 30
stage1_max_inactive_days = 180
known_bots = []
parallel_enabled = true
parallel_tasks = 8
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
accumulator_min_roi = 0.05
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
news_sniper_max_burstiness_top_1h_ratio = 0.70
liquidity_provider_min_buy_sell_balance = 0.45
liquidity_provider_min_mid_fill_ratio = 0.60
bot_swarm_min_trades_per_day = 200.0
bot_swarm_max_avg_trade_size_usdc = 5.0
jackpot_min_pnl_top1_share = 0.60
jackpot_max_win_rate = 0.45
topic_lane_min_top_domain_ratio = 0.65
bonder_min_extreme_price_ratio = 0.60
whale_min_avg_trade_size_usdc = 100.0
stage2_min_roi = 0.03

[wallet_rules]
min_trades_for_discovery = 50
max_trades_per_day = 120.0
max_distinct_markets_30d = 60
min_median_hold_minutes = 180.0
max_flip_rate = 0.20
max_size_gini = 0.75
min_liquidity_score = 0.35
max_median_seconds_between_trades = 45.0
max_fraction_trades_at_spread_edge = 0.70
paper_window_days = 14
required_paper_trades = 30
min_paper_profit_per_trade = 0.0
max_paper_drawdown = 0.08
max_paper_slippage_bps = 35.0
live_breakers_enabled = false
live_max_drawdown = 0.12
live_slippage_bps_spike = 80.0
live_style_drift_score = 0.65
live_inactivity_days = 10
live_max_theme_concentration = 0.55
live_max_correlation_cluster_exposure = 0.65
per_trade_risk_cap = 0.01
per_market_risk_cap = 0.03
per_wallet_risk_cap = 0.06

[anomaly]
win_rate_drop_pct = 15.0
max_weekly_drawdown_pct = 20.0
frequency_change_multiplier = 3.0
size_change_multiplier = 10.0
"#;

        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.personas.parallel_enabled);
        assert_eq!(cfg.personas.parallel_tasks, 8);
    }

    #[test]
    fn test_personas_parallelization_defaults() {
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
top_n_events = 50
min_liquidity_usdc = 1000.0
min_daily_volume_usdc = 5000.0
min_daily_trades = 20
min_unique_traders = 10
max_days_to_expiry = 90
min_days_to_expiry = 1
refresh_interval_secs = 3600
weights_liquidity = 0.25
weights_volume = 0.25
weights_density = 0.20
weights_whale_concentration = 0.15
weights_time_to_expiry = 0.15

[wallet_discovery]
min_total_trades = 5
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
mirror_use_proportional_sizing = true
mirror_default_their_bankroll_usd = 5000

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
stage1_min_total_trades = 10
stage1_min_wallet_age_days = 30
stage1_max_inactive_days = 180
known_bots = []
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
accumulator_min_roi = 0.05
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
news_sniper_max_burstiness_top_1h_ratio = 0.70
liquidity_provider_min_buy_sell_balance = 0.45
liquidity_provider_min_mid_fill_ratio = 0.60
bot_swarm_min_trades_per_day = 200.0
bot_swarm_max_avg_trade_size_usdc = 5.0
jackpot_min_pnl_top1_share = 0.60
jackpot_max_win_rate = 0.45
topic_lane_min_top_domain_ratio = 0.65
bonder_min_extreme_price_ratio = 0.60
whale_min_avg_trade_size_usdc = 100.0
stage2_min_roi = 0.03

[wallet_rules]
min_trades_for_discovery = 50
max_trades_per_day = 120.0
max_distinct_markets_30d = 60
min_median_hold_minutes = 180.0
max_flip_rate = 0.20
max_size_gini = 0.75
min_liquidity_score = 0.35
max_median_seconds_between_trades = 45.0
max_fraction_trades_at_spread_edge = 0.70
paper_window_days = 14
required_paper_trades = 30
min_paper_profit_per_trade = 0.0
max_paper_drawdown = 0.08
max_paper_slippage_bps = 35.0
live_breakers_enabled = false
live_max_drawdown = 0.12
live_slippage_bps_spike = 80.0
live_style_drift_score = 0.65
live_inactivity_days = 10
live_max_theme_concentration = 0.55
live_max_correlation_cluster_exposure = 0.65
per_trade_risk_cap = 0.01
per_market_risk_cap = 0.03
per_wallet_risk_cap = 0.06

[anomaly]
win_rate_drop_pct = 15.0
max_weekly_drawdown_pct = 20.0
frequency_change_multiplier = 3.0
size_change_multiplier = 10.0
"#;

        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.personas.parallel_enabled, "should default to true");
        assert_eq!(cfg.personas.parallel_tasks, 8, "should default to 8");
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
top_n_events = 50
min_liquidity_usdc = 1000.0
min_daily_volume_usdc = 5000.0
min_daily_trades = 20
min_unique_traders = 10
max_days_to_expiry = 90
min_days_to_expiry = 1
refresh_interval_secs = 3600
weights_liquidity = 0.25
weights_volume = 0.25
weights_density = 0.20
weights_whale_concentration = 0.15
weights_time_to_expiry = 0.15

[wallet_discovery]
min_total_trades = 5
max_wallets_per_market = 100
holders_per_market = 20
markets_per_discovery_run = 20
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
mirror_use_proportional_sizing = true
mirror_default_their_bankroll_usd = 5000

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
accumulator_min_roi = 0.05
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
news_sniper_max_burstiness_top_1h_ratio = 0.70
liquidity_provider_min_buy_sell_balance = 0.45
liquidity_provider_min_mid_fill_ratio = 0.60
bot_swarm_min_trades_per_day = 200.0
bot_swarm_max_avg_trade_size_usdc = 5.0
jackpot_min_pnl_top1_share = 0.60
jackpot_max_win_rate = 0.45
topic_lane_min_top_domain_ratio = 0.65
bonder_min_extreme_price_ratio = 0.60
whale_min_avg_trade_size_usdc = 100.0
known_bots = []
stage2_min_roi = 0.03

[wallet_rules]
min_trades_for_discovery = 50
max_trades_per_day = 120.0
max_distinct_markets_30d = 60
min_median_hold_minutes = 180.0
max_flip_rate = 0.20
max_size_gini = 0.75
min_liquidity_score = 0.35
max_median_seconds_between_trades = 45.0
max_fraction_trades_at_spread_edge = 0.70
paper_window_days = 14
required_paper_trades = 30
min_paper_profit_per_trade = 0.0
max_paper_drawdown = 0.08
max_paper_slippage_bps = 35.0
live_breakers_enabled = false
live_max_drawdown = 0.12
live_slippage_bps_spike = 80.0
live_style_drift_score = 0.65
live_inactivity_days = 10
live_max_theme_concentration = 0.55
live_max_correlation_cluster_exposure = 0.65
per_trade_risk_cap = 0.01
per_market_risk_cap = 0.03
per_wallet_risk_cap = 0.06

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
