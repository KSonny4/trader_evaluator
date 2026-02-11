use crate::config::Config;

fn fmt_f64(x: f64) -> String {
    if x.fract().abs() < f64::EPSILON {
        format!("{x:.0}")
    } else {
        let s = format!("{x:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Stage order and short names (Markets, Scored, Wallets, Tracked, Paper, Ranked) must match
/// STRATEGY_BIBLE § Funnel stages (implementation).
pub fn funnel_stage_infos(cfg: &Config) -> [String; 6] {
    let markets = format!(
        "Open Gamma markets: min_liquidity_usdc={} min_daily_volume_usdc={} days_to_expiry=[{},{}] end_date>=tomorrow.",
        fmt_f64(cfg.market_scoring.min_liquidity_usdc),
        fmt_f64(cfg.market_scoring.min_daily_volume_usdc),
        cfg.market_scoring.min_days_to_expiry,
        cfg.market_scoring.max_days_to_expiry
    );

    let scored = format!(
        "Daily MScore ranking: top_n_markets={} written to market_scores_daily.",
        cfg.market_scoring.top_n_markets
    );

    let wallets = format!(
        "From top_n_markets={} scored markets: holders_per_market={} include traders with >=min_total_trades={} cap max_wallets_per_market={} per market.",
        cfg.market_scoring.top_n_markets,
        cfg.wallet_discovery.holders_per_market,
        cfg.wallet_discovery.min_total_trades,
        cfg.wallet_discovery.max_wallets_per_market
    );

    let tracked = "Wallets with is_active=1; ingestion runs only for tracked wallets.".to_string();

    let paper = format!(
        "Mirrored paper trades when checks pass: position_size_usdc={} slippage_pct={} paper_bankroll_usdc={} max_exposure_per_market_pct={} max_exposure_per_wallet_pct={} max_daily_trades={} portfolio_stop_drawdown_pct={}.",
        fmt_f64(cfg.paper_trading.position_size_usdc),
        fmt_f64(cfg.risk.slippage_pct),
        fmt_f64(cfg.risk.paper_bankroll_usdc),
        fmt_f64(cfg.risk.max_exposure_per_market_pct),
        fmt_f64(cfg.risk.max_exposure_per_wallet_pct),
        cfg.risk.max_daily_trades,
        fmt_f64(cfg.risk.portfolio_stop_drawdown_pct),
    );

    let windows = cfg
        .wallet_scoring
        .windows_days
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let ranked = format!(
        "Wallet scores from paper PnL: windows_days=[{}] min_trades_for_score={}; ranked=wallets with a score row today.",
        windows,
        cfg.wallet_scoring.min_trades_for_score
    );

    [markets, scored, wallets, tracked, paper, ranked]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_funnel_stage_infos_include_key_numbers() {
        let cfg = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        let infos = funnel_stage_infos(&cfg);

        assert_eq!(infos.len(), 6);

        assert!(infos[0].contains("min_liquidity_usdc="));
        assert!(infos[0].contains("min_daily_volume_usdc="));
        assert!(infos[0].contains("days_to_expiry=["));

        assert!(infos[1].contains("top_n_markets="));

        assert!(infos[2].contains("holders_per_market="));
        assert!(infos[2].contains("min_total_trades="));
        assert!(infos[2].contains("max_wallets_per_market="));

        assert!(infos[4].contains("position_size_usdc="));
        assert!(infos[4].contains("slippage_pct="));
        assert!(infos[4].contains("paper_bankroll_usdc="));
        assert!(infos[4].contains("max_exposure_per_market_pct="));
        assert!(infos[4].contains("max_exposure_per_wallet_pct="));
        assert!(infos[4].contains("max_daily_trades="));
        assert!(infos[4].contains("portfolio_stop_drawdown_pct="));

        assert!(infos[5].contains("windows_days=["));
        assert!(infos[5].contains("min_trades_for_score="));
    }
}
