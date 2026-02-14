use crate::wallet_features::WalletFeatures;

#[derive(Debug, Clone, Copy)]
pub struct WScoreWeights {
    pub edge_weight: f64,
    pub consistency_weight: f64,
    pub market_skill_weight: f64,
    pub timing_skill_weight: f64,
    pub behavior_quality_weight: f64,
}

impl Default for WScoreWeights {
    fn default() -> Self {
        Self {
            edge_weight: 0.30,
            consistency_weight: 0.25,
            market_skill_weight: 0.20,
            timing_skill_weight: 0.15,
            behavior_quality_weight: 0.10,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WalletScoreInput {
    /// Total ROI over the scoring window, percent (e.g. +12.3).
    pub roi_pct: f64,
    /// Stddev of daily returns over the window, percent.
    pub daily_return_stdev_pct: f64,
    /// Win rate (hit rate) in range [0, 1].
    pub hit_rate: f64,
    /// Number of markets where PnL was positive.
    pub profitable_markets: u32,
    /// Number of markets traded (or evaluated for profitability).
    pub total_markets: u32,
    /// Avg post-entry drift in cents: >0 means price moved in our favor after entry.
    pub avg_post_entry_drift_cents: f64,
    /// Fraction of trades that are "noise" (0..1). Lower is better.
    pub noise_trade_ratio: f64,
    /// Wallet age in days (Strategy Bible trust multiplier input).
    pub wallet_age_days: u32,
    /// If true, wallet is on the public leaderboard top-500 (no obscurity bonus).
    pub is_public_leaderboard_top_500: bool,
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

pub fn edge_score(roi_pct: f64) -> f64 {
    // Normalize ROI into [0, 1], treating <=0 as 0.
    // In early MVP we cap at +20% => 1.0.
    clamp01(roi_pct.max(0.0) / 20.0)
}

pub fn consistency_score(daily_return_stdev_pct: f64) -> f64 {
    // Normalize stdev into [0, 1] where 0% stdev => 1.0 and >=10% => 0.0.
    let max_stdev = 10.0;
    clamp01(1.0 - (daily_return_stdev_pct / max_stdev))
}

/// Market skill: fraction of markets that were profitable.
pub fn market_skill_score(profitable_markets: u32, total_markets: u32) -> f64 {
    if total_markets == 0 {
        return 0.0;
    }
    clamp01(f64::from(profitable_markets) / f64::from(total_markets))
}

/// Timing skill: did price move in our favor after entry?
/// avg_post_entry_drift_cents > 0 = good timing, < 0 = bad timing.
/// Normalized: 0 at -10 cents, 0.5 at 0, 1.0 at +10 cents.
pub fn timing_skill_score(avg_post_entry_drift_cents: f64) -> f64 {
    let normalized = (avg_post_entry_drift_cents + 10.0) / 20.0;
    clamp01(normalized)
}

/// Behavior quality: fewer noise trades = higher quality.
/// noise_trade_ratio = 0 -> score 1.0, noise_trade_ratio = 1 -> score 0.0
pub fn behavior_quality_score(noise_trade_ratio: f64) -> f64 {
    clamp01(1.0 - noise_trade_ratio)
}

pub fn compute_wscore(
    input: &WalletScoreInput,
    w: &WScoreWeights,
    trust_30_90_multiplier: f64,
    obscurity_bonus_multiplier: f64,
) -> f64 {
    let e = edge_score(input.roi_pct);
    let c = consistency_score(input.daily_return_stdev_pct);
    let ms = market_skill_score(input.profitable_markets, input.total_markets);
    let ts = timing_skill_score(input.avg_post_entry_drift_cents);
    let bq = behavior_quality_score(input.noise_trade_ratio);

    let total_w = w.edge_weight
        + w.consistency_weight
        + w.market_skill_weight
        + w.timing_skill_weight
        + w.behavior_quality_weight;
    if total_w <= 0.0 {
        return 0.0;
    }

    let mut score = (w.edge_weight * e
        + w.consistency_weight * c
        + w.market_skill_weight * ms
        + w.timing_skill_weight * ts
        + w.behavior_quality_weight * bq)
        / total_w;

    // Win rate sensitivity analysis (Strategy Bible §3):
    // Penalize low hit rates even if ROI is high (e.g. tail risk sellers or lucky snipers).
    if input.hit_rate < 0.45 {
        score *= 0.5; // Heavy penalty for win rate < 45%
    } else if input.hit_rate < 0.52 {
        score *= 0.8; // Minor penalty for borderline win rate
    }

    // Trust and obscurity (Strategy Bible Appendix A, §4).
    // Note: stage1 filters should already remove wallets under 30 days; we still
    // apply the trust multiplier for any wallet under 90 days as a conservative default.
    let trust_mult = if input.wallet_age_days < 90 {
        trust_30_90_multiplier.max(0.0)
    } else {
        1.0
    };
    let obscurity_mult = if input.is_public_leaderboard_top_500 {
        1.0
    } else {
        obscurity_bonus_multiplier.max(0.0)
    };

    score *= trust_mult * obscurity_mult;

    clamp01(score)
}

/// Build a WalletScoreInput from on-chain WalletFeatures (no paper_trades needed).
pub fn score_input_from_features(
    features: &WalletFeatures,
    wallet_age_days: u32,
    is_leaderboard: bool,
) -> WalletScoreInput {
    let total_trades = features.win_count + features.loss_count;
    let hit_rate = if total_trades > 0 {
        f64::from(features.win_count) / f64::from(total_trades)
    } else {
        0.0
    };

    // ROI% based on total volume (avg_position_size * trade_count) as proxy bankroll.
    let bankroll_proxy = features.avg_position_size * f64::from(features.trade_count).max(1.0);
    let roi_pct = if bankroll_proxy > 0.0 {
        100.0 * features.total_pnl / bankroll_proxy
    } else {
        0.0
    };

    // daily_return_stdev_pct: use max_drawdown as heuristic proxy.
    // Rationale: wallets with high drawdown have high return variance.
    let daily_return_stdev_pct = features.max_drawdown_pct * 0.5;

    // noise_trade_ratio: blend of extreme_price fills and burstiness.
    let noise_trade_ratio =
        features.extreme_price_ratio * 0.5 + features.burstiness_top_1h_ratio * 0.5;

    WalletScoreInput {
        roi_pct,
        daily_return_stdev_pct,
        hit_rate,
        profitable_markets: features.profitable_markets,
        total_markets: features.unique_markets,
        avg_post_entry_drift_cents: 0.0, // TODO(#81): compute from post-entry price movement
        noise_trade_ratio,
        wallet_age_days,
        is_public_leaderboard_top_500: is_leaderboard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_skill_score() {
        // Profitable in 3 out of 5 markets = 0.6
        let score = market_skill_score(3, 5);
        assert!((score - 0.6).abs() < 0.01);
    }

    #[test]
    fn test_timing_skill_score() {
        // Average post-entry drift of +5 cents = good timing
        let score = timing_skill_score(5.0);
        assert!(score > 0.5);

        // Average post-entry drift of -3 cents = bad timing
        let score = timing_skill_score(-3.0);
        assert!(score < 0.5);
    }

    #[test]
    fn test_behavior_quality_score() {
        // 5% noise trades = high quality
        let score = behavior_quality_score(0.05);
        assert!(score > 0.9);

        // 50% noise trades = low quality
        let score = behavior_quality_score(0.50);
        assert!(score < 0.6);
    }

    #[test]
    fn test_full_wscore_all_5_components() {
        let input = WalletScoreInput {
            roi_pct: 10.0,
            daily_return_stdev_pct: 3.0,
            hit_rate: 0.55,
            profitable_markets: 5,
            total_markets: 8,
            avg_post_entry_drift_cents: 3.0,
            noise_trade_ratio: 0.10,
            wallet_age_days: 120,
            is_public_leaderboard_top_500: false,
        };
        let weights = WScoreWeights {
            edge_weight: 0.30,
            consistency_weight: 0.25,
            market_skill_weight: 0.20,
            timing_skill_weight: 0.15,
            behavior_quality_weight: 0.10,
        };
        let score = compute_wscore(&input, &weights, 0.8, 1.2);
        assert!(score > 0.0 && score <= 1.0);
    }

    #[test]
    fn test_compute_wscore_in_range() {
        let w = WScoreWeights::default();
        let s = compute_wscore(
            &WalletScoreInput {
                roi_pct: 12.0,
                daily_return_stdev_pct: 3.0,
                hit_rate: 0.55,
                profitable_markets: 0,
                total_markets: 0,
                avg_post_entry_drift_cents: 0.0,
                noise_trade_ratio: 0.0,
                wallet_age_days: 120,
                is_public_leaderboard_top_500: false,
            },
            &w,
            1.0,
            1.0,
        );
        assert!(s >= 0.0);
        assert!(s <= 1.0);
    }

    #[test]
    fn test_positive_edge_scores_higher() {
        let w = WScoreWeights::default();
        let good = compute_wscore(
            &WalletScoreInput {
                roi_pct: 10.0,
                daily_return_stdev_pct: 2.0,
                hit_rate: 0.60,
                profitable_markets: 0,
                total_markets: 0,
                avg_post_entry_drift_cents: 0.0,
                noise_trade_ratio: 0.0,
                wallet_age_days: 120,
                is_public_leaderboard_top_500: false,
            },
            &w,
            1.0,
            1.0,
        );
        let bad = compute_wscore(
            &WalletScoreInput {
                roi_pct: 0.0,
                daily_return_stdev_pct: 2.0,
                hit_rate: 0.60,
                profitable_markets: 0,
                total_markets: 0,
                avg_post_entry_drift_cents: 0.0,
                noise_trade_ratio: 0.0,
                wallet_age_days: 120,
                is_public_leaderboard_top_500: false,
            },
            &w,
            1.0,
            1.0,
        );
        assert!(good > bad);
    }

    #[test]
    fn test_unstable_wallet_scores_lower_on_consistency() {
        let w = WScoreWeights::default();
        let stable = compute_wscore(
            &WalletScoreInput {
                roi_pct: 10.0,
                daily_return_stdev_pct: 1.0,
                hit_rate: 0.60,
                profitable_markets: 0,
                total_markets: 0,
                avg_post_entry_drift_cents: 0.0,
                noise_trade_ratio: 0.0,
                wallet_age_days: 120,
                is_public_leaderboard_top_500: false,
            },
            &w,
            1.0,
            1.0,
        );
        let unstable = compute_wscore(
            &WalletScoreInput {
                roi_pct: 10.0,
                daily_return_stdev_pct: 12.0,
                hit_rate: 0.60,
                profitable_markets: 0,
                total_markets: 0,
                avg_post_entry_drift_cents: 0.0,
                noise_trade_ratio: 0.0,
                wallet_age_days: 120,
                is_public_leaderboard_top_500: false,
            },
            &w,
            1.0,
            1.0,
        );
        assert!(stable > unstable);
    }

    #[test]
    fn test_low_win_rate_penalty() {
        let w = WScoreWeights::default();
        let high_wr = compute_wscore(
            &WalletScoreInput {
                roi_pct: 10.0,
                daily_return_stdev_pct: 2.0,
                hit_rate: 0.60,
                profitable_markets: 0,
                total_markets: 0,
                avg_post_entry_drift_cents: 0.0,
                noise_trade_ratio: 0.0,
                wallet_age_days: 120,
                is_public_leaderboard_top_500: false,
            },
            &w,
            1.0,
            1.0,
        );
        let low_wr = compute_wscore(
            &WalletScoreInput {
                roi_pct: 10.0,
                daily_return_stdev_pct: 2.0,
                hit_rate: 0.40,
                profitable_markets: 0,
                total_markets: 0,
                avg_post_entry_drift_cents: 0.0,
                noise_trade_ratio: 0.0,
                wallet_age_days: 120,
                is_public_leaderboard_top_500: false,
            },
            &w,
            1.0,
            1.0,
        );
        assert!(high_wr > low_wr);
    }

    #[test]
    fn test_score_input_from_features() {
        let features = WalletFeatures {
            proxy_wallet: "0xabc".to_string(),
            window_days: 30,
            trade_count: 100,
            win_count: 60,
            loss_count: 40,
            total_pnl: 500.0,
            avg_position_size: 50.0,
            unique_markets: 10,
            avg_hold_time_hours: 24.0,
            max_drawdown_pct: 8.0,
            trades_per_week: 25.0,
            trades_per_day: 3.5,
            sharpe_ratio: 1.2,
            active_positions: 3,
            concentration_ratio: 0.5,
            avg_trade_size_usdc: 50.0,
            size_cv: 0.2,
            buy_sell_balance: 0.8,
            mid_fill_ratio: 0.1,
            extreme_price_ratio: 0.05,
            burstiness_top_1h_ratio: 0.1,
            top_domain: Some("sports".to_string()),
            top_domain_ratio: 0.7,
            profitable_markets: 7,
            cashflow_pnl: 100.0,
            fifo_realized_pnl: 0.0,
            unrealized_pnl: 0.0,
            open_positions_count: 0,
        };
        let input = score_input_from_features(&features, 120, false);
        assert!((input.hit_rate - 0.6).abs() < 0.01);
        assert_eq!(input.profitable_markets, 7);
        assert_eq!(input.total_markets, 10);
        assert!(input.roi_pct > 0.0);
        assert_eq!(input.wallet_age_days, 120);
        assert!(!input.is_public_leaderboard_top_500);
        // daily_return_stdev_pct = max_drawdown_pct * 0.5 = 4.0
        assert!((input.daily_return_stdev_pct - 4.0).abs() < 0.01);
        // noise = 0.05*0.5 + 0.1*0.5 = 0.075
        assert!((input.noise_trade_ratio - 0.075).abs() < 0.01);
    }
}
