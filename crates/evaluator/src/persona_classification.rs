use anyhow::Result;
use rusqlite::Connection;

use crate::wallet_features::WalletFeatures;

/// Result of running the full classification pipeline on a wallet.
#[derive(Debug, Clone, PartialEq)]
pub enum ClassificationResult {
    Followable(Persona),
    Excluded(ExclusionReason),
    Unclassified,
}

/// Configuration for persona detection thresholds. Built from config or test defaults.
#[derive(Debug, Clone)]
pub struct PersonaConfig {
    pub specialist_max_active_positions: u32,
    pub specialist_min_concentration: f64,
    pub specialist_min_win_rate: f64,
    pub generalist_min_markets: u32,
    pub generalist_min_win_rate: f64,
    pub generalist_max_win_rate: f64,
    pub generalist_max_drawdown: f64,
    pub generalist_min_sharpe: f64,
    pub accumulator_min_hold_hours: f64,
    pub accumulator_max_trades_per_week: f64,
    #[allow(dead_code)] // Used when Execution Master detection is wired (PnL decomposition)
    pub execution_master_pnl_ratio: f64,
    pub tail_risk_min_win_rate: f64,
    pub tail_risk_loss_multiplier: f64,
    pub noise_max_trades_per_week: f64,
    pub noise_max_abs_roi: f64,
    pub sniper_max_age_days: u32,
    pub sniper_min_win_rate: f64,
    pub sniper_max_trades: u32,
    pub news_sniper_max_burstiness_top_1h_ratio: f64,
    pub liquidity_provider_min_buy_sell_balance: f64,
    pub liquidity_provider_min_mid_fill_ratio: f64,
    pub bot_swarm_min_trades_per_day: f64,
    pub bot_swarm_max_avg_trade_size_usdc: f64,
    pub jackpot_min_pnl_top1_share: f64,
    pub jackpot_max_win_rate: f64,
    pub topic_lane_min_top_domain_ratio: f64,
    pub bonder_min_extreme_price_ratio: f64,
    pub whale_min_avg_trade_size_usdc: f64,
    pub stage2_min_roi: f64,
}

impl PersonaConfig {
    #[cfg(test)]
    pub fn default_for_test() -> Self {
        Self {
            specialist_max_active_positions: 10,
            specialist_min_concentration: 0.60,
            specialist_min_win_rate: 0.60,
            generalist_min_markets: 20,
            generalist_min_win_rate: 0.52,
            generalist_max_win_rate: 0.60,
            generalist_max_drawdown: 15.0,
            generalist_min_sharpe: 1.0,
            accumulator_min_hold_hours: 48.0,
            accumulator_max_trades_per_week: 5.0,
            execution_master_pnl_ratio: 0.70,
            tail_risk_min_win_rate: 0.80,
            tail_risk_loss_multiplier: 5.0,
            noise_max_trades_per_week: 50.0,
            noise_max_abs_roi: 0.02,
            sniper_max_age_days: 30,
            sniper_min_win_rate: 0.85,
            sniper_max_trades: 20,
            news_sniper_max_burstiness_top_1h_ratio: 0.70,
            liquidity_provider_min_buy_sell_balance: 0.45,
            liquidity_provider_min_mid_fill_ratio: 0.60,
            bot_swarm_min_trades_per_day: 200.0,
            bot_swarm_max_avg_trade_size_usdc: 5.0,
            jackpot_min_pnl_top1_share: 0.60,
            jackpot_max_win_rate: 0.45,
            topic_lane_min_top_domain_ratio: 0.65,
            bonder_min_extreme_price_ratio: 0.60,
            whale_min_avg_trade_size_usdc: 100.0,
            stage2_min_roi: 0.0,
        }
    }

    /// Build from common config Personas (for production).
    pub fn from_personas(p: &common::config::Personas) -> Self {
        Self {
            specialist_max_active_positions: p.specialist_max_active_positions,
            specialist_min_concentration: p.specialist_min_concentration,
            specialist_min_win_rate: p.specialist_min_win_rate,
            generalist_min_markets: p.generalist_min_markets,
            generalist_min_win_rate: p.generalist_min_win_rate,
            generalist_max_win_rate: p.generalist_max_win_rate,
            generalist_max_drawdown: p.generalist_max_drawdown,
            generalist_min_sharpe: p.generalist_min_sharpe,
            accumulator_min_hold_hours: p.accumulator_min_hold_hours,
            accumulator_max_trades_per_week: p.accumulator_max_trades_per_week,
            execution_master_pnl_ratio: p.execution_master_pnl_ratio,
            tail_risk_min_win_rate: p.tail_risk_min_win_rate,
            tail_risk_loss_multiplier: p.tail_risk_loss_multiplier,
            noise_max_trades_per_week: p.noise_max_trades_per_week,
            noise_max_abs_roi: p.noise_max_abs_roi,
            sniper_max_age_days: p.sniper_max_age_days,
            sniper_min_win_rate: p.sniper_min_win_rate,
            sniper_max_trades: p.sniper_max_trades,
            news_sniper_max_burstiness_top_1h_ratio: p.news_sniper_max_burstiness_top_1h_ratio,
            liquidity_provider_min_buy_sell_balance: p.liquidity_provider_min_buy_sell_balance,
            liquidity_provider_min_mid_fill_ratio: p.liquidity_provider_min_mid_fill_ratio,
            bot_swarm_min_trades_per_day: p.bot_swarm_min_trades_per_day,
            bot_swarm_max_avg_trade_size_usdc: p.bot_swarm_max_avg_trade_size_usdc,
            jackpot_min_pnl_top1_share: p.jackpot_min_pnl_top1_share,
            jackpot_max_win_rate: p.jackpot_max_win_rate,
            topic_lane_min_top_domain_ratio: p.topic_lane_min_top_domain_ratio,
            bonder_min_extreme_price_ratio: p.bonder_min_extreme_price_ratio,
            whale_min_avg_trade_size_usdc: p.whale_min_avg_trade_size_usdc,
            stage2_min_roi: p.stage2_min_roi,
        }
    }
}

#[allow(dead_code)] // Wired into scheduler in Task 21
#[derive(Debug, Clone, PartialEq)]
pub enum ExclusionReason {
    TooYoung {
        age_days: u32,
        min_required: u32,
    },
    TooFewTrades {
        total: u32,
        min_required: u32,
    },
    Inactive {
        days_since_last: u32,
        max_allowed: u32,
    },
    ExecutionMaster {
        execution_pnl_ratio: f64,
        threshold: f64,
    },
    TailRiskSeller {
        win_rate: f64,
        max_loss_ratio: f64,
        min_win_rate_threshold: f64,
        loss_multiplier_threshold: f64,
    },
    NoiseTrader {
        trades_per_week: f64,
        abs_roi: f64,
        max_trades_threshold: f64,
        max_roi_threshold: f64,
    },
    SniperInsider {
        age_days: u32,
        win_rate: f64,
        trade_count: u32,
        max_age_threshold: u32,
        min_win_rate_threshold: f64,
        max_trades_threshold: u32,
    },
    NewsSniper {
        burstiness: f64,
        max_burstiness: f64,
    },
    LiquidityProvider {
        side_balance: f64,
        mid_fill_ratio: f64,
    },
    JackpotGambler {
        pnl_top1_share: f64,
        win_rate: f64,
    },
    BotSwarmMicro {
        trades_per_day: f64,
        avg_size_usdc: f64,
    },
    /// Wallet is in the configured known_bots list (Strategy Bible §4 Stage 1).
    KnownBot,
    /// Would pass persona checks but ROI < stage2_min_roi (win rate + PnL combo).
    InsufficientPnl {
        roi: f64,
        min_roi: f64,
    },
}

#[allow(dead_code)] // Wired into scheduler in Task 21
impl ExclusionReason {
    pub fn reason_str(&self) -> &'static str {
        match self {
            Self::TooYoung { .. } => "STAGE1_TOO_YOUNG",
            Self::TooFewTrades { .. } => "STAGE1_TOO_FEW_TRADES",
            Self::Inactive { .. } => "STAGE1_INACTIVE",
            Self::ExecutionMaster { .. } => "EXECUTION_MASTER",
            Self::TailRiskSeller { .. } => "TAIL_RISK_SELLER",
            Self::NoiseTrader { .. } => "NOISE_TRADER",
            Self::SniperInsider { .. } => "SNIPER_INSIDER",
            Self::NewsSniper { .. } => "NEWS_SNIPER",
            Self::LiquidityProvider { .. } => "LIQUIDITY_PROVIDER",
            Self::JackpotGambler { .. } => "JACKPOT_GAMBLER",
            Self::BotSwarmMicro { .. } => "BOT_SWARM_MICRO",
            Self::KnownBot => "KNOWN_BOT",
            Self::InsufficientPnl { .. } => "INSUFFICIENT_PNL",
        }
    }

    pub fn metric_value(&self) -> f64 {
        match self {
            Self::TooYoung { age_days, .. } => f64::from(*age_days),
            Self::TooFewTrades { total, .. } => f64::from(*total),
            Self::Inactive {
                days_since_last, ..
            } => f64::from(*days_since_last),
            Self::ExecutionMaster {
                execution_pnl_ratio,
                ..
            } => *execution_pnl_ratio,
            Self::TailRiskSeller { win_rate, .. } => *win_rate,
            Self::NoiseTrader {
                trades_per_week, ..
            } => *trades_per_week,
            Self::SniperInsider { win_rate, .. } => *win_rate,
            Self::NewsSniper { burstiness, .. } => *burstiness,
            Self::LiquidityProvider { mid_fill_ratio, .. } => *mid_fill_ratio,
            Self::JackpotGambler { pnl_top1_share, .. } => *pnl_top1_share,
            Self::BotSwarmMicro { trades_per_day, .. } => *trades_per_day,
            Self::KnownBot => 1.0,
            Self::InsufficientPnl { roi, .. } => *roi,
        }
    }

    pub fn threshold(&self) -> f64 {
        match self {
            Self::TooYoung { min_required, .. } => f64::from(*min_required),
            Self::TooFewTrades { min_required, .. } => f64::from(*min_required),
            Self::Inactive { max_allowed, .. } => f64::from(*max_allowed),
            Self::ExecutionMaster { threshold, .. } => *threshold,
            Self::TailRiskSeller {
                loss_multiplier_threshold,
                ..
            } => *loss_multiplier_threshold,
            Self::NoiseTrader {
                max_trades_threshold,
                ..
            } => *max_trades_threshold,
            Self::SniperInsider {
                min_win_rate_threshold,
                ..
            } => *min_win_rate_threshold,
            Self::NewsSniper { max_burstiness, .. } => *max_burstiness,
            Self::LiquidityProvider { .. } => 0.0,
            Self::JackpotGambler { .. } => 0.0,
            Self::BotSwarmMicro { .. } => 0.0,
            Self::KnownBot => 0.0,
            Self::InsufficientPnl { min_roi, .. } => *min_roi,
        }
    }
}

#[allow(dead_code)] // Wired into scheduler in Task 21
#[derive(Debug, Clone)]
pub struct Stage1Config {
    pub min_wallet_age_days: u32,
    pub min_total_trades: u32,
    pub max_inactive_days: u32,
    /// Proxy wallet addresses to exclude as known bots (Strategy Bible §4 Stage 1).
    pub known_bots: Vec<String>,
}

/// Returns Some(KnownBot) if proxy_wallet is in the known_bots list, None otherwise.
#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn stage1_known_bot_check(
    proxy_wallet: &str,
    known_bots: &[String],
) -> Option<ExclusionReason> {
    if known_bots.iter().any(|b| b.as_str() == proxy_wallet) {
        Some(ExclusionReason::KnownBot)
    } else {
        None
    }
}

/// Returns Some(reason) if the wallet should be excluded, None if it passes.
#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn stage1_filter(
    wallet_age_days: u32,
    total_trades: u32,
    days_since_last_trade: u32,
    config: &Stage1Config,
) -> Option<ExclusionReason> {
    if wallet_age_days < config.min_wallet_age_days {
        return Some(ExclusionReason::TooYoung {
            age_days: wallet_age_days,
            min_required: config.min_wallet_age_days,
        });
    }
    if total_trades < config.min_total_trades {
        return Some(ExclusionReason::TooFewTrades {
            total: total_trades,
            min_required: config.min_total_trades,
        });
    }
    if days_since_last_trade > config.max_inactive_days {
        return Some(ExclusionReason::Inactive {
            days_since_last: days_since_last_trade,
            max_allowed: config.max_inactive_days,
        });
    }
    None
}

/// Record an exclusion in the wallet_exclusions table.
#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn record_exclusion(
    conn: &Connection,
    proxy_wallet: &str,
    reason: &ExclusionReason,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold, excluded_at)
         VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%d %H:%M:%f', 'now'))",
        rusqlite::params![
            proxy_wallet,
            reason.reason_str(),
            reason.metric_value(),
            reason.threshold(),
        ],
    )?;
    Ok(())
}

/// If roi < min_roi, records exclusion and returns Some(reason); otherwise returns None.
fn record_exclusion_if_roi_fails(
    conn: &Connection,
    proxy_wallet: &str,
    roi: f64,
    min_roi: f64,
) -> Result<Option<ExclusionReason>> {
    if roi < min_roi {
        let reason = ExclusionReason::InsufficientPnl { roi, min_roi };
        record_exclusion(conn, proxy_wallet, &reason)?;
        Ok(Some(reason))
    } else {
        Ok(None)
    }
}

fn upsert_trait(conn: &Connection, proxy_wallet: &str, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO wallet_persona_traits (proxy_wallet, trait_key, trait_value, computed_at)
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%d %H:%M:%f', 'now'))
         ON CONFLICT(proxy_wallet, trait_key) DO UPDATE SET
           trait_value = excluded.trait_value,
           computed_at = strftime('%Y-%m-%d %H:%M:%f', 'now')",
        rusqlite::params![proxy_wallet, key, value],
    )?;
    Ok(())
}

fn delete_trait(conn: &Connection, proxy_wallet: &str, key: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM wallet_persona_traits WHERE proxy_wallet = ?1 AND trait_key = ?2",
        rusqlite::params![proxy_wallet, key],
    )?;
    Ok(())
}

fn record_persona_traits(
    conn: &Connection,
    features: &WalletFeatures,
    config: &PersonaConfig,
) -> Result<()> {
    if features.top_domain_ratio >= config.topic_lane_min_top_domain_ratio {
        if let Some(domain) = features.top_domain.as_deref() {
            upsert_trait(conn, &features.proxy_wallet, "TOPIC_LANE", domain)?;
        }
    } else {
        delete_trait(conn, &features.proxy_wallet, "TOPIC_LANE")?;
    }

    if features.extreme_price_ratio >= config.bonder_min_extreme_price_ratio {
        upsert_trait(conn, &features.proxy_wallet, "BONDER", "1")?;
    } else {
        delete_trait(conn, &features.proxy_wallet, "BONDER")?;
    }

    if features.avg_trade_size_usdc >= config.whale_min_avg_trade_size_usdc {
        upsert_trait(conn, &features.proxy_wallet, "WHALE", "1")?;
    } else {
        delete_trait(conn, &features.proxy_wallet, "WHALE")?;
    }

    Ok(())
}

#[allow(dead_code)] // Wired into scheduler in Task 21
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Persona {
    InformedSpecialist,
    ConsistentGeneralist,
    PatientAccumulator,
}

#[allow(dead_code)] // Wired into scheduler in Task 21
impl Persona {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InformedSpecialist => "INFORMED_SPECIALIST",
            Self::ConsistentGeneralist => "CONSISTENT_GENERALIST",
            Self::PatientAccumulator => "PATIENT_ACCUMULATOR",
        }
    }

    pub fn follow_mode(&self) -> &'static str {
        match self {
            Self::InformedSpecialist => "mirror_with_delay",
            Self::ConsistentGeneralist => "mirror",
            Self::PatientAccumulator => "mirror_slow",
        }
    }
}

/// Personas that are considered followable at Stage 2 (for flow metrics and ranking).
/// Single source of truth: when adding a followable Persona variant, add it here.
pub const FOLLOWABLE_PERSONAS: &[Persona] = &[
    Persona::InformedSpecialist,
    Persona::ConsistentGeneralist,
    Persona::PatientAccumulator,
];

/// Detect the Informed Specialist persona: concentrated positions, high win rate.
/// Combines active_positions count AND concentration_ratio to identify true specialists.
/// Returns Some(InformedSpecialist) if criteria are met, None otherwise.
#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn detect_informed_specialist(
    features: &WalletFeatures,
    max_active_positions: u32,
    min_concentration_ratio: f64,
    min_win_rate: f64,
) -> Option<Persona> {
    // Check active positions limit (catches "dabbler" with scattered trades)
    if features.active_positions > max_active_positions {
        return None;
    }

    // Check concentration ratio (catches "generalist" trading many markets equally)
    if features.concentration_ratio < min_concentration_ratio {
        return None;
    }

    let total_resolved = features.win_count + features.loss_count;
    if total_resolved == 0 {
        return None;
    }
    let win_rate = f64::from(features.win_count) / f64::from(total_resolved);
    if win_rate < min_win_rate {
        return None;
    }
    Some(Persona::InformedSpecialist)
}

/// Detects wallets whose profit comes primarily from execution edge (unreplicable).
/// execution_pnl_ratio = execution_pnl / total_pnl (from PnL decomposition).
/// If ratio > threshold, this wallet's edge is in execution, not direction.
#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn detect_execution_master(
    execution_pnl_ratio: f64,
    threshold: f64,
) -> Option<ExclusionReason> {
    if execution_pnl_ratio > threshold {
        Some(ExclusionReason::ExecutionMaster {
            execution_pnl_ratio,
            threshold,
        })
    } else {
        None
    }
}

/// Detects wallets with very high win rate but occasional catastrophic losses.
/// These look great on paper but will eventually blow up.
#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn detect_tail_risk_seller(
    win_rate: f64,
    max_loss_vs_avg_win: f64,
    min_win_rate_threshold: f64,
    loss_multiplier_threshold: f64,
) -> Option<ExclusionReason> {
    if win_rate > min_win_rate_threshold && max_loss_vs_avg_win > loss_multiplier_threshold {
        Some(ExclusionReason::TailRiskSeller {
            win_rate,
            max_loss_ratio: max_loss_vs_avg_win,
            min_win_rate_threshold,
            loss_multiplier_threshold,
        })
    } else {
        None
    }
}

/// Detects high-churn wallets with no statistical edge.
/// High frequency + near-zero ROI = noise.
#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn detect_noise_trader(
    trades_per_week: f64,
    abs_roi: f64,
    max_trades_per_week: f64,
    max_abs_roi: f64,
) -> Option<ExclusionReason> {
    if trades_per_week > max_trades_per_week && abs_roi < max_abs_roi {
        Some(ExclusionReason::NoiseTrader {
            trades_per_week,
            abs_roi,
            max_trades_threshold: max_trades_per_week,
            max_roi_threshold: max_abs_roi,
        })
    } else {
        None
    }
}

/// Detects suspiciously new wallets with anomalous win rates.
/// Young + high win rate + few trades = likely insider or lucky sniper.
#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn detect_sniper_insider(
    wallet_age_days: u32,
    win_rate: f64,
    trade_count: u32,
    max_age_days: u32,
    min_win_rate: f64,
    max_trades: u32,
) -> Option<ExclusionReason> {
    if wallet_age_days < max_age_days && win_rate > min_win_rate && trade_count < max_trades {
        Some(ExclusionReason::SniperInsider {
            age_days: wallet_age_days,
            win_rate,
            trade_count,
            max_age_threshold: max_age_days,
            min_win_rate_threshold: min_win_rate,
            max_trades_threshold: max_trades,
        })
    } else {
        None
    }
}

#[allow(dead_code)] // Wired into scheduler in Task 32
pub fn detect_news_sniper(
    burstiness_top_1h_ratio: f64,
    max_burstiness: f64,
) -> Option<ExclusionReason> {
    if burstiness_top_1h_ratio > max_burstiness {
        Some(ExclusionReason::NewsSniper {
            burstiness: burstiness_top_1h_ratio,
            max_burstiness,
        })
    } else {
        None
    }
}

#[allow(dead_code)] // Wired into scheduler in Task 32
pub fn detect_liquidity_provider(
    buy_sell_balance: f64,
    mid_fill_ratio: f64,
    min_buy_sell_balance: f64,
    min_mid_fill_ratio: f64,
) -> Option<ExclusionReason> {
    if buy_sell_balance >= min_buy_sell_balance && mid_fill_ratio >= min_mid_fill_ratio {
        Some(ExclusionReason::LiquidityProvider {
            side_balance: buy_sell_balance,
            mid_fill_ratio,
        })
    } else {
        None
    }
}

#[allow(dead_code)] // Wired into scheduler in Task 32
pub fn detect_jackpot_gambler(
    pnl_top1_share: f64,
    win_rate: f64,
    min_pnl_top1_share: f64,
    max_win_rate: f64,
) -> Option<ExclusionReason> {
    if pnl_top1_share >= min_pnl_top1_share && win_rate <= max_win_rate {
        Some(ExclusionReason::JackpotGambler {
            pnl_top1_share,
            win_rate,
        })
    } else {
        None
    }
}

#[allow(dead_code)] // Wired into scheduler in Task 32
pub fn detect_bot_swarm_micro(
    trades_per_day: f64,
    avg_size_usdc: f64,
    min_trades_per_day: f64,
    max_avg_size_usdc: f64,
) -> Option<ExclusionReason> {
    if trades_per_day >= min_trades_per_day && avg_size_usdc <= max_avg_size_usdc {
        Some(ExclusionReason::BotSwarmMicro {
            trades_per_day,
            avg_size_usdc,
        })
    } else {
        None
    }
}

/// Detect the Patient Accumulator persona: long holds, low trading frequency.
/// Returns Some(PatientAccumulator) if criteria are met, None otherwise.
#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn detect_patient_accumulator(
    features: &WalletFeatures,
    min_hold_hours: f64,
    max_trades_per_week: f64,
) -> Option<Persona> {
    if features.avg_hold_time_hours < min_hold_hours {
        return None;
    }
    if features.trades_per_week > max_trades_per_week {
        return None;
    }
    Some(Persona::PatientAccumulator)
}

/// Detect the Consistent Generalist persona: many markets, steady returns, low drawdown.
/// Returns Some(ConsistentGeneralist) if criteria are met, None otherwise.
#[allow(dead_code)] // Wired into scheduler in Task 21
pub fn detect_consistent_generalist(
    features: &WalletFeatures,
    min_markets: u32,
    min_win_rate: f64,
    max_win_rate: f64,
    max_drawdown: f64,
    min_sharpe: f64,
) -> Option<Persona> {
    if features.unique_markets < min_markets {
        return None;
    }
    let total_resolved = features.win_count + features.loss_count;
    if total_resolved == 0 {
        return None;
    }
    let win_rate = f64::from(features.win_count) / f64::from(total_resolved);
    if win_rate < min_win_rate || win_rate > max_win_rate {
        return None;
    }
    if features.max_drawdown_pct > max_drawdown {
        return None;
    }
    if features.sharpe_ratio < min_sharpe {
        return None;
    }
    Some(Persona::ConsistentGeneralist)
}

/// Full classification pipeline for a wallet.
/// Checks exclusions first (order matters — cheapest checks first), then followable personas.
pub fn classify_wallet(
    conn: &Connection,
    features: &WalletFeatures,
    wallet_age_days: u32,
    config: &PersonaConfig,
) -> Result<ClassificationResult> {
    let total_resolved = features.win_count + features.loss_count;
    let win_rate = if total_resolved > 0 {
        f64::from(features.win_count) / f64::from(total_resolved)
    } else {
        0.0
    };

    let roi = if features.trade_count > 0 && features.avg_position_size > 0.0 {
        features.total_pnl / (f64::from(features.trade_count) * features.avg_position_size)
    } else {
        0.0
    };

    // Traits are computed for all wallets (followable and excluded).
    record_persona_traits(conn, features, config)?;

    // --- Exclusion checks (Stage 2) ---

    if let Some(reason) = detect_sniper_insider(
        wallet_age_days,
        win_rate,
        features.trade_count,
        config.sniper_max_age_days,
        config.sniper_min_win_rate,
        config.sniper_max_trades,
    ) {
        record_exclusion(conn, &features.proxy_wallet, &reason)?;
        return Ok(ClassificationResult::Excluded(reason));
    }

    if let Some(reason) = detect_noise_trader(
        features.trades_per_week,
        roi.abs(),
        config.noise_max_trades_per_week,
        config.noise_max_abs_roi,
    ) {
        record_exclusion(conn, &features.proxy_wallet, &reason)?;
        return Ok(ClassificationResult::Excluded(reason));
    }

    if let Some(reason) = detect_news_sniper(
        features.burstiness_top_1h_ratio,
        config.news_sniper_max_burstiness_top_1h_ratio,
    ) {
        record_exclusion(conn, &features.proxy_wallet, &reason)?;
        return Ok(ClassificationResult::Excluded(reason));
    }

    if let Some(reason) = detect_liquidity_provider(
        features.buy_sell_balance,
        features.mid_fill_ratio,
        config.liquidity_provider_min_buy_sell_balance,
        config.liquidity_provider_min_mid_fill_ratio,
    ) {
        record_exclusion(conn, &features.proxy_wallet, &reason)?;
        return Ok(ClassificationResult::Excluded(reason));
    }

    if let Some(reason) = detect_bot_swarm_micro(
        features.trades_per_day,
        features.avg_trade_size_usdc,
        config.bot_swarm_min_trades_per_day,
        config.bot_swarm_max_avg_trade_size_usdc,
    ) {
        record_exclusion(conn, &features.proxy_wallet, &reason)?;
        return Ok(ClassificationResult::Excluded(reason));
    }

    let avg_win_pnl = if features.win_count > 0 {
        features.total_pnl.max(1.0) / f64::from(features.win_count)
    } else {
        1.0
    };
    let max_loss_proxy = features.max_drawdown_pct * features.avg_position_size / 100.0;
    let loss_ratio = if avg_win_pnl > 0.0 {
        max_loss_proxy / avg_win_pnl
    } else {
        0.0
    };

    if let Some(reason) = detect_tail_risk_seller(
        win_rate,
        loss_ratio,
        config.tail_risk_min_win_rate,
        config.tail_risk_loss_multiplier,
    ) {
        record_exclusion(conn, &features.proxy_wallet, &reason)?;
        return Ok(ClassificationResult::Excluded(reason));
    }

    if let Some(reason) = detect_jackpot_gambler(
        features.concentration_ratio,
        win_rate,
        config.jackpot_min_pnl_top1_share,
        config.jackpot_max_win_rate,
    ) {
        record_exclusion(conn, &features.proxy_wallet, &reason)?;
        return Ok(ClassificationResult::Excluded(reason));
    }

    // --- Followable persona detection (priority order) ---
    // Each followable persona also requires roi >= stage2_min_roi (win rate + PnL combo).

    if let Some(persona) = detect_informed_specialist(
        features,
        config.specialist_max_active_positions,
        config.specialist_min_concentration,
        config.specialist_min_win_rate,
    ) {
        if let Some(reason) =
            record_exclusion_if_roi_fails(conn, &features.proxy_wallet, roi, config.stage2_min_roi)?
        {
            return Ok(ClassificationResult::Excluded(reason));
        }
        record_persona(conn, &features.proxy_wallet, &persona, win_rate)?;
        return Ok(ClassificationResult::Followable(persona));
    }

    if let Some(persona) = detect_consistent_generalist(
        features,
        config.generalist_min_markets,
        config.generalist_min_win_rate,
        config.generalist_max_win_rate,
        config.generalist_max_drawdown,
        config.generalist_min_sharpe,
    ) {
        if let Some(reason) =
            record_exclusion_if_roi_fails(conn, &features.proxy_wallet, roi, config.stage2_min_roi)?
        {
            return Ok(ClassificationResult::Excluded(reason));
        }
        record_persona(conn, &features.proxy_wallet, &persona, win_rate)?;
        return Ok(ClassificationResult::Followable(persona));
    }

    if let Some(persona) = detect_patient_accumulator(
        features,
        config.accumulator_min_hold_hours,
        config.accumulator_max_trades_per_week,
    ) {
        if let Some(reason) =
            record_exclusion_if_roi_fails(conn, &features.proxy_wallet, roi, config.stage2_min_roi)?
        {
            return Ok(ClassificationResult::Excluded(reason));
        }
        record_persona(conn, &features.proxy_wallet, &persona, win_rate)?;
        return Ok(ClassificationResult::Followable(persona));
    }

    Ok(ClassificationResult::Unclassified)
}

/// Record a followable persona classification.
/// Schema has UNIQUE(proxy_wallet, classified_at), so each run adds a row; use latest by classified_at for "current" persona.
pub fn record_persona(
    conn: &Connection,
    proxy_wallet: &str,
    persona: &Persona,
    confidence: f64,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%d %H:%M:%f', 'now'))",
        rusqlite::params![proxy_wallet, persona.as_str(), confidence],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::db::Database;

    #[test]
    fn test_known_bot_excluded_when_in_list() {
        let known_bots = vec!["0xbot".to_string(), "0xother".to_string()];
        let result = stage1_known_bot_check("0xbot", &known_bots);
        assert_eq!(result, Some(ExclusionReason::KnownBot));
    }

    #[test]
    fn test_known_bot_not_excluded_when_not_in_list() {
        let known_bots = vec!["0xbot".to_string()];
        let result = stage1_known_bot_check("0xhuman", &known_bots);
        assert_eq!(result, None);
    }

    #[test]
    fn test_known_bot_record_exclusion_persists() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let reason = ExclusionReason::KnownBot;
        record_exclusion(&db.conn, "0xbot", &reason).unwrap();

        let (stored_reason, metric, threshold): (String, f64, f64) = db.conn
            .query_row(
                "SELECT reason, metric_value, threshold FROM wallet_exclusions WHERE proxy_wallet = '0xbot'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(stored_reason, "KNOWN_BOT");
        assert!((metric - 1.0).abs() < f64::EPSILON);
        assert!((threshold - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_stage1_too_young() {
        let result = stage1_filter(
            5,  // wallet_age_days
            50, // total_trades
            1,  // days_since_last_trade
            &Stage1Config {
                min_wallet_age_days: 30,
                min_total_trades: 10,
                max_inactive_days: 30,
                known_bots: vec![],
            },
        );
        assert_eq!(
            result,
            Some(ExclusionReason::TooYoung {
                age_days: 5,
                min_required: 30
            })
        );
    }

    #[test]
    fn test_stage1_too_few_trades() {
        let result = stage1_filter(
            60, // old enough
            3,  // too few trades
            1,
            &Stage1Config {
                min_wallet_age_days: 30,
                min_total_trades: 10,
                max_inactive_days: 30,
                known_bots: vec![],
            },
        );
        assert_eq!(
            result,
            Some(ExclusionReason::TooFewTrades {
                total: 3,
                min_required: 10
            })
        );
    }

    #[test]
    fn test_stage1_inactive() {
        let result = stage1_filter(
            180,
            50,
            45,
            &Stage1Config {
                min_wallet_age_days: 30,
                min_total_trades: 10,
                max_inactive_days: 30,
                known_bots: vec![],
            },
        );
        assert_eq!(
            result,
            Some(ExclusionReason::Inactive {
                days_since_last: 45,
                max_allowed: 30
            })
        );
    }

    #[test]
    fn test_stage1_passes() {
        let result = stage1_filter(
            60,
            50,
            1,
            &Stage1Config {
                min_wallet_age_days: 30,
                min_total_trades: 10,
                max_inactive_days: 30,
                known_bots: vec![],
            },
        );
        assert_eq!(result, None);
    }

    #[test]
    fn test_stage1_boundary_exact_min_age() {
        // Exactly at the boundary — 30 days should pass (not < 30)
        let result = stage1_filter(
            30,
            50,
            1,
            &Stage1Config {
                min_wallet_age_days: 30,
                min_total_trades: 10,
                max_inactive_days: 30,
                known_bots: vec![],
            },
        );
        assert_eq!(result, None);
    }

    #[test]
    fn test_stage1_boundary_exact_min_trades() {
        // Exactly at the boundary — 10 trades should pass (not < 10)
        let result = stage1_filter(
            60,
            10,
            1,
            &Stage1Config {
                min_wallet_age_days: 30,
                min_total_trades: 10,
                max_inactive_days: 30,
                known_bots: vec![],
            },
        );
        assert_eq!(result, None);
    }

    #[test]
    fn test_stage1_boundary_exact_max_inactive() {
        // Exactly at the boundary — 30 days inactive should pass (not > 30)
        let result = stage1_filter(
            60,
            50,
            30,
            &Stage1Config {
                min_wallet_age_days: 30,
                min_total_trades: 10,
                max_inactive_days: 30,
                known_bots: vec![],
            },
        );
        assert_eq!(result, None);
    }

    #[test]
    fn test_record_exclusion_persists() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let reason = ExclusionReason::TooYoung {
            age_days: 5,
            min_required: 30,
        };
        record_exclusion(&db.conn, "0xabc", &reason).unwrap();

        let (stored_reason, metric, threshold): (String, f64, f64) = db.conn.query_row(
            "SELECT reason, metric_value, threshold FROM wallet_exclusions WHERE proxy_wallet = '0xabc'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(stored_reason, "STAGE1_TOO_YOUNG");
        assert!((metric - 5.0).abs() < f64::EPSILON);
        assert!((threshold - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_record_exclusion_replaces_not_duplicates() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        // Insert first exclusion
        let reason1 = ExclusionReason::TooYoung {
            age_days: 5,
            min_required: 30,
        };
        record_exclusion(&db.conn, "0xabc", &reason1).unwrap();

        // Insert same wallet+reason again with different metric
        let reason2 = ExclusionReason::TooYoung {
            age_days: 10,
            min_required: 30,
        };
        record_exclusion(&db.conn, "0xabc", &reason2).unwrap();

        // Should be 1 row, not 2 (INSERT OR REPLACE with UNIQUE constraint)
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM wallet_exclusions WHERE proxy_wallet = '0xabc' AND reason = 'STAGE1_TOO_YOUNG'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // The metric should be updated to the latest value
        let metric: f64 = db
            .conn
            .query_row(
                "SELECT metric_value FROM wallet_exclusions WHERE proxy_wallet = '0xabc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!((metric - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_exclusion_reason_str_all_variants() {
        assert_eq!(
            ExclusionReason::TooYoung {
                age_days: 1,
                min_required: 30
            }
            .reason_str(),
            "STAGE1_TOO_YOUNG"
        );
        assert_eq!(
            ExclusionReason::TooFewTrades {
                total: 1,
                min_required: 10
            }
            .reason_str(),
            "STAGE1_TOO_FEW_TRADES"
        );
        assert_eq!(
            ExclusionReason::Inactive {
                days_since_last: 45,
                max_allowed: 30
            }
            .reason_str(),
            "STAGE1_INACTIVE"
        );
        assert_eq!(
            ExclusionReason::ExecutionMaster {
                execution_pnl_ratio: 0.8,
                threshold: 0.7
            }
            .reason_str(),
            "EXECUTION_MASTER"
        );
        assert_eq!(
            ExclusionReason::TailRiskSeller {
                win_rate: 0.85,
                max_loss_ratio: 8.0,
                min_win_rate_threshold: 0.80,
                loss_multiplier_threshold: 5.0,
            }
            .reason_str(),
            "TAIL_RISK_SELLER"
        );
        assert_eq!(
            ExclusionReason::NoiseTrader {
                trades_per_week: 60.0,
                abs_roi: 0.005,
                max_trades_threshold: 50.0,
                max_roi_threshold: 0.02,
            }
            .reason_str(),
            "NOISE_TRADER"
        );
        assert_eq!(
            ExclusionReason::SniperInsider {
                age_days: 15,
                win_rate: 0.90,
                trade_count: 12,
                max_age_threshold: 30,
                min_win_rate_threshold: 0.85,
                max_trades_threshold: 20,
            }
            .reason_str(),
            "SNIPER_INSIDER"
        );
        assert_eq!(ExclusionReason::KnownBot.reason_str(), "KNOWN_BOT");
    }

    #[test]
    fn test_exclusion_reason_strings_include_ag_personas() {
        assert_eq!(
            ExclusionReason::NewsSniper {
                burstiness: 1.0,
                max_burstiness: 0.9
            }
            .reason_str(),
            "NEWS_SNIPER"
        );
        assert_eq!(
            ExclusionReason::LiquidityProvider {
                side_balance: 0.5,
                mid_fill_ratio: 0.8
            }
            .reason_str(),
            "LIQUIDITY_PROVIDER"
        );
        assert_eq!(
            ExclusionReason::JackpotGambler {
                pnl_top1_share: 0.9,
                win_rate: 0.3
            }
            .reason_str(),
            "JACKPOT_GAMBLER"
        );
        assert_eq!(
            ExclusionReason::BotSwarmMicro {
                trades_per_day: 500.0,
                avg_size_usdc: 1.0
            }
            .reason_str(),
            "BOT_SWARM_MICRO"
        );
    }

    fn make_features(unique_markets: u32, win_count: u32, loss_count: u32) -> WalletFeatures {
        WalletFeatures {
            proxy_wallet: "0xabc".to_string(),
            window_days: 30,
            trade_count: win_count + loss_count,
            win_count,
            loss_count,
            total_pnl: 500.0,
            avg_position_size: 200.0,
            unique_markets,
            avg_hold_time_hours: 24.0,
            max_drawdown_pct: 8.0,
            trades_per_week: 10.0,
            trades_per_day: 10.0 / 7.0,
            sharpe_ratio: 1.5,
            active_positions: 3,
            concentration_ratio: 0.75,
            avg_trade_size_usdc: 200.0,
            size_cv: 0.0,
            buy_sell_balance: 0.0,
            mid_fill_ratio: 0.0,
            extreme_price_ratio: 0.0,
            burstiness_top_1h_ratio: 0.0,
            top_domain: None,
            top_domain_ratio: 0.0,
        }
    }

    #[test]
    fn test_detect_informed_specialist() {
        let features = make_features(5, 28, 12); // 5 markets, 70% win rate
        let persona = detect_informed_specialist(&features, 5, 0.60, 0.60);
        assert_eq!(persona, Some(Persona::InformedSpecialist));
    }

    #[test]
    fn test_not_specialist_too_many_active_positions() {
        let mut features = make_features(5, 28, 12); // 5 markets, 70% win rate
        features.active_positions = 10; // Too many active positions
        let persona = detect_informed_specialist(&features, 5, 0.60, 0.60);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_not_specialist_low_concentration() {
        let mut features = make_features(5, 28, 12); // 5 markets, 70% win rate
        features.concentration_ratio = 0.30; // Too low concentration
        let persona = detect_informed_specialist(&features, 5, 0.60, 0.60);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_not_specialist_low_win_rate() {
        let features = make_features(5, 10, 30); // 25% win rate < 60%
        let persona = detect_informed_specialist(&features, 5, 0.60, 0.60);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_not_specialist_zero_resolved_trades() {
        let features = make_features(5, 0, 0); // no wins or losses
        let persona = detect_informed_specialist(&features, 5, 0.60, 0.60);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_specialist_boundary_exact_max_positions() {
        let mut features = make_features(5, 28, 12); // 5 markets, 70% win rate
        features.active_positions = 5; // exactly 5 positions = max
        let persona = detect_informed_specialist(&features, 5, 0.60, 0.60);
        assert_eq!(persona, Some(Persona::InformedSpecialist));
    }

    #[test]
    fn test_specialist_boundary_exact_min_concentration() {
        let mut features = make_features(5, 28, 12);
        features.concentration_ratio = 0.60; // exactly 60% concentration
        let persona = detect_informed_specialist(&features, 5, 0.60, 0.60);
        assert_eq!(persona, Some(Persona::InformedSpecialist));
    }

    #[test]
    fn test_persona_as_str() {
        assert_eq!(Persona::InformedSpecialist.as_str(), "INFORMED_SPECIALIST");
        assert_eq!(
            Persona::ConsistentGeneralist.as_str(),
            "CONSISTENT_GENERALIST"
        );
        assert_eq!(Persona::PatientAccumulator.as_str(), "PATIENT_ACCUMULATOR");
    }

    #[test]
    fn test_persona_follow_mode() {
        assert_eq!(
            Persona::InformedSpecialist.follow_mode(),
            "mirror_with_delay"
        );
        assert_eq!(Persona::ConsistentGeneralist.follow_mode(), "mirror");
        assert_eq!(Persona::PatientAccumulator.follow_mode(), "mirror_slow");
    }

    fn make_generalist_features(
        unique_markets: u32,
        win_count: u32,
        loss_count: u32,
        max_drawdown_pct: f64,
        sharpe_ratio: f64,
    ) -> WalletFeatures {
        WalletFeatures {
            proxy_wallet: "0xgen".to_string(),
            window_days: 30,
            trade_count: win_count + loss_count,
            win_count,
            loss_count,
            total_pnl: 200.0,
            avg_position_size: 25.0,
            unique_markets,
            avg_hold_time_hours: 12.0,
            max_drawdown_pct,
            trades_per_week: 25.0,
            trades_per_day: 25.0 / 7.0,
            sharpe_ratio,
            active_positions: 8,
            concentration_ratio: 0.50,
            avg_trade_size_usdc: 25.0,
            size_cv: 0.0,
            buy_sell_balance: 0.0,
            mid_fill_ratio: 0.0,
            extreme_price_ratio: 0.0,
            burstiness_top_1h_ratio: 0.0,
            top_domain: None,
            top_domain_ratio: 0.0,
        }
    }

    #[test]
    fn test_detect_consistent_generalist() {
        // 25 markets, 55% win rate, 10% drawdown, 1.2 sharpe
        let features = make_generalist_features(25, 55, 45, 10.0, 1.2);
        let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
        assert_eq!(persona, Some(Persona::ConsistentGeneralist));
    }

    #[test]
    fn test_not_generalist_low_sharpe() {
        let features = make_generalist_features(25, 55, 45, 10.0, 0.5); // sharpe < 1.0
        let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_not_generalist_too_few_markets() {
        let features = make_generalist_features(15, 55, 45, 10.0, 1.2); // 15 < 20
        let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_not_generalist_win_rate_too_high() {
        // 75% win rate > max 60% — too good, might be tail risk seller
        let features = make_generalist_features(25, 75, 25, 10.0, 1.2);
        let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_not_generalist_win_rate_too_low() {
        // 40% win rate < min 52%
        let features = make_generalist_features(25, 40, 60, 10.0, 1.2);
        let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_not_generalist_high_drawdown() {
        let features = make_generalist_features(25, 55, 45, 20.0, 1.2); // 20% > 15%
        let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_not_generalist_zero_resolved() {
        let features = make_generalist_features(25, 0, 0, 10.0, 1.2);
        let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_generalist_boundary_exact_min_markets() {
        // Exactly 20 markets = minimum threshold
        let features = make_generalist_features(20, 55, 45, 10.0, 1.2);
        let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
        assert_eq!(persona, Some(Persona::ConsistentGeneralist));
    }

    #[test]
    fn test_generalist_boundary_exact_max_drawdown() {
        // Exactly 15% drawdown = at threshold, should pass (not >)
        let features = make_generalist_features(25, 55, 45, 15.0, 1.2);
        let persona = detect_consistent_generalist(&features, 20, 0.52, 0.60, 15.0, 1.0);
        assert_eq!(persona, Some(Persona::ConsistentGeneralist));
    }

    // --- Task 7: Patient Accumulator ---

    fn make_accumulator_features(avg_hold_time_hours: f64, trades_per_week: f64) -> WalletFeatures {
        WalletFeatures {
            proxy_wallet: "0xacc".to_string(),
            window_days: 30,
            trade_count: 12,
            win_count: 8,
            loss_count: 4,
            total_pnl: 800.0,
            avg_position_size: 2000.0,
            unique_markets: 3,
            avg_hold_time_hours,
            max_drawdown_pct: 5.0,
            trades_per_week,
            trades_per_day: trades_per_week / 7.0,
            sharpe_ratio: 0.8,
            active_positions: 2,
            concentration_ratio: 0.80,
            avg_trade_size_usdc: 2000.0,
            size_cv: 0.0,
            buy_sell_balance: 0.0,
            mid_fill_ratio: 0.0,
            extreme_price_ratio: 0.0,
            burstiness_top_1h_ratio: 0.0,
            top_domain: None,
            top_domain_ratio: 0.0,
        }
    }

    #[test]
    fn test_detect_patient_accumulator() {
        let features = make_accumulator_features(72.0, 3.0); // holds >48h, <5 trades/week
        let persona = detect_patient_accumulator(&features, 48.0, 5.0);
        assert_eq!(persona, Some(Persona::PatientAccumulator));
    }

    #[test]
    fn test_not_accumulator_too_frequent() {
        let features = make_accumulator_features(72.0, 15.0); // >5 trades/week
        let persona = detect_patient_accumulator(&features, 48.0, 5.0);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_not_accumulator_short_holds() {
        let features = make_accumulator_features(12.0, 3.0); // holds <48h
        let persona = detect_patient_accumulator(&features, 48.0, 5.0);
        assert_eq!(persona, None);
    }

    #[test]
    fn test_accumulator_boundary_exact_min_hold() {
        // Exactly 48h = at threshold, should pass (not <)
        let features = make_accumulator_features(48.0, 3.0);
        let persona = detect_patient_accumulator(&features, 48.0, 5.0);
        assert_eq!(persona, Some(Persona::PatientAccumulator));
    }

    #[test]
    fn test_accumulator_boundary_exact_max_frequency() {
        // Exactly 5 trades/week = at threshold, should pass (not >)
        let features = make_accumulator_features(72.0, 5.0);
        let persona = detect_patient_accumulator(&features, 48.0, 5.0);
        assert_eq!(persona, Some(Persona::PatientAccumulator));
    }

    // --- Task 8: Execution Master ---

    #[test]
    fn test_detect_execution_master() {
        // Wallet where 80% of PnL comes from execution edge (buying below mid)
        let result = detect_execution_master(0.80, 0.70);
        assert_eq!(
            result,
            Some(ExclusionReason::ExecutionMaster {
                execution_pnl_ratio: 0.80,
                threshold: 0.70,
            })
        );
    }

    #[test]
    fn test_not_execution_master() {
        let result = detect_execution_master(0.30, 0.70);
        assert_eq!(result, None);
    }

    #[test]
    fn test_execution_master_boundary_at_threshold() {
        // Exactly at 0.70 — should NOT trigger (> not >=)
        let result = detect_execution_master(0.70, 0.70);
        assert_eq!(result, None);
    }

    // --- Task 9: Tail Risk Seller ---

    #[test]
    fn test_detect_tail_risk_seller() {
        // 85% win rate but max single loss is 8x average win
        let result = detect_tail_risk_seller(0.85, 8.0, 0.80, 5.0);
        assert_eq!(
            result,
            Some(ExclusionReason::TailRiskSeller {
                win_rate: 0.85,
                max_loss_ratio: 8.0,
                min_win_rate_threshold: 0.80,
                loss_multiplier_threshold: 5.0,
            })
        );
    }

    #[test]
    fn test_not_tail_risk_seller_low_win_rate() {
        let result = detect_tail_risk_seller(0.55, 8.0, 0.80, 5.0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_not_tail_risk_seller_small_losses() {
        let result = detect_tail_risk_seller(0.85, 2.0, 0.80, 5.0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_tail_risk_seller_boundary_at_thresholds() {
        // Exactly at both thresholds — should NOT trigger (> not >=)
        let result = detect_tail_risk_seller(0.80, 5.0, 0.80, 5.0);
        assert_eq!(result, None);
    }

    // --- Task 10: Noise Trader ---

    #[test]
    fn test_detect_noise_trader() {
        // 60 trades/week with near-zero ROI = pure noise
        let result = detect_noise_trader(60.0, 0.005, 50.0, 0.02);
        assert_eq!(
            result,
            Some(ExclusionReason::NoiseTrader {
                trades_per_week: 60.0,
                abs_roi: 0.005,
                max_trades_threshold: 50.0,
                max_roi_threshold: 0.02,
            })
        );
    }

    #[test]
    fn test_not_noise_low_frequency() {
        let result = detect_noise_trader(10.0, 0.005, 50.0, 0.02);
        assert_eq!(result, None);
    }

    #[test]
    fn test_not_noise_significant_roi() {
        let result = detect_noise_trader(60.0, 0.10, 50.0, 0.02);
        assert_eq!(result, None);
    }

    #[test]
    fn test_noise_trader_boundary_at_thresholds() {
        // Exactly at both thresholds — should NOT trigger (> and < are strict)
        let result = detect_noise_trader(50.0, 0.02, 50.0, 0.02);
        assert_eq!(result, None);
    }

    // --- Task 11: Sniper/Insider ---

    #[test]
    fn test_detect_sniper() {
        // New wallet (15 days), 90% win rate on 12 trades = suspicious
        let result = detect_sniper_insider(15, 0.90, 12, 30, 0.85, 20);
        assert_eq!(
            result,
            Some(ExclusionReason::SniperInsider {
                age_days: 15,
                win_rate: 0.90,
                trade_count: 12,
                max_age_threshold: 30,
                min_win_rate_threshold: 0.85,
                max_trades_threshold: 20,
            })
        );
    }

    #[test]
    fn test_not_sniper_old_wallet() {
        let result = detect_sniper_insider(180, 0.90, 12, 30, 0.85, 20);
        assert_eq!(result, None);
    }

    #[test]
    fn test_not_sniper_normal_win_rate() {
        let result = detect_sniper_insider(15, 0.55, 12, 30, 0.85, 20);
        assert_eq!(result, None);
    }

    #[test]
    fn test_not_sniper_too_many_trades() {
        // 25 trades > max 20 — enough history to not be suspicious
        let result = detect_sniper_insider(15, 0.90, 25, 30, 0.85, 20);
        assert_eq!(result, None);
    }

    #[test]
    fn test_sniper_boundary_at_thresholds() {
        // Exactly at all thresholds — should NOT trigger (all use strict < and >)
        let result = detect_sniper_insider(30, 0.85, 20, 30, 0.85, 20);
        assert_eq!(result, None);
    }

    #[test]
    fn test_detect_news_sniper_bursty_short_horizon_proxy_excludes() {
        let reason = detect_news_sniper(0.95, 0.90);
        assert!(matches!(reason, Some(ExclusionReason::NewsSniper { .. })));
    }

    #[test]
    fn test_detect_liquidity_provider_two_sided_mid_fills_excludes() {
        let reason = detect_liquidity_provider(0.50, 0.80, 0.45, 0.60);
        assert!(matches!(
            reason,
            Some(ExclusionReason::LiquidityProvider { .. })
        ));
    }

    #[test]
    fn test_detect_jackpot_gambler_pnl_concentration_excludes() {
        let reason = detect_jackpot_gambler(0.80, 0.30, 0.60, 0.45);
        assert!(matches!(
            reason,
            Some(ExclusionReason::JackpotGambler { .. })
        ));
    }

    #[test]
    fn test_detect_bot_swarm_micro_extreme_frequency_excludes() {
        let reason = detect_bot_swarm_micro(300.0, 2.0, 200.0, 5.0);
        assert!(matches!(
            reason,
            Some(ExclusionReason::BotSwarmMicro { .. })
        ));
    }

    // --- Task 12: Classification orchestrator ---

    #[test]
    fn test_classify_wallet_informed_specialist() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let features = WalletFeatures {
            proxy_wallet: "0xabc".to_string(),
            window_days: 30,
            trade_count: 40,
            win_count: 28,
            loss_count: 12,
            total_pnl: 500.0,
            avg_position_size: 200.0,
            unique_markets: 5,
            avg_hold_time_hours: 24.0,
            max_drawdown_pct: 8.0,
            trades_per_week: 10.0,
            trades_per_day: 10.0 / 7.0,
            sharpe_ratio: 1.5,
            active_positions: 3,
            concentration_ratio: 0.75,
            avg_trade_size_usdc: 200.0,
            size_cv: 0.0,
            buy_sell_balance: 0.0,
            mid_fill_ratio: 0.0,
            extreme_price_ratio: 0.0,
            burstiness_top_1h_ratio: 0.0,
            top_domain: None,
            top_domain_ratio: 0.0,
        };

        let config = PersonaConfig::default_for_test();
        let result = classify_wallet(&db.conn, &features, 90, &config).unwrap();

        assert_eq!(
            result,
            ClassificationResult::Followable(Persona::InformedSpecialist)
        );

        let persona: String = db
            .conn
            .query_row(
                "SELECT persona FROM wallet_personas WHERE proxy_wallet = '0xabc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(persona, "INFORMED_SPECIALIST");
    }

    #[test]
    fn test_classify_wallet_excluded_noise_trader() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let features = WalletFeatures {
            proxy_wallet: "0xnoise".to_string(),
            window_days: 30,
            trade_count: 300,
            win_count: 150,
            loss_count: 150,
            total_pnl: 5.0,
            avg_position_size: 10.0,
            unique_markets: 30,
            avg_hold_time_hours: 0.5,
            max_drawdown_pct: 3.0,
            trades_per_week: 75.0,
            trades_per_day: 75.0 / 7.0,
            sharpe_ratio: 0.1,
            active_positions: 15,
            concentration_ratio: 0.3,
            avg_trade_size_usdc: 10.0,
            size_cv: 0.0,
            buy_sell_balance: 0.0,
            mid_fill_ratio: 0.0,
            extreme_price_ratio: 0.0,
            burstiness_top_1h_ratio: 0.0,
            top_domain: None,
            top_domain_ratio: 0.0,
        };

        let config = PersonaConfig::default_for_test();
        let result = classify_wallet(&db.conn, &features, 180, &config).unwrap();

        match &result {
            ClassificationResult::Excluded(reason) => {
                assert_eq!(reason.reason_str(), "NOISE_TRADER");
            }
            _ => panic!("Expected exclusion, got {result:?}"),
        }

        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM wallet_exclusions WHERE proxy_wallet = '0xnoise'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_classify_wallet_excluded_insufficient_pnl() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        // Would be Informed Specialist (high win rate, concentration) but negative PnL
        let features = WalletFeatures {
            proxy_wallet: "0xneg".to_string(),
            window_days: 30,
            trade_count: 40,
            win_count: 28,
            loss_count: 12,
            total_pnl: -100.0, // negative
            avg_position_size: 200.0,
            unique_markets: 5,
            avg_hold_time_hours: 24.0,
            max_drawdown_pct: 8.0,
            trades_per_week: 10.0,
            trades_per_day: 10.0 / 7.0,
            sharpe_ratio: 1.5,
            active_positions: 3,
            concentration_ratio: 0.75,
            avg_trade_size_usdc: 200.0,
            size_cv: 0.0,
            buy_sell_balance: 0.0,
            mid_fill_ratio: 0.0,
            extreme_price_ratio: 0.0,
            burstiness_top_1h_ratio: 0.0,
            top_domain: None,
            top_domain_ratio: 0.0,
        };

        let mut config = PersonaConfig::default_for_test();
        config.stage2_min_roi = 0.0; // require non-negative
        let result = classify_wallet(&db.conn, &features, 90, &config).unwrap();

        match &result {
            ClassificationResult::Excluded(reason) => {
                assert_eq!(reason.reason_str(), "INSUFFICIENT_PNL");
            }
            _ => panic!("Expected InsufficientPnl exclusion, got {result:?}"),
        }

        let reason: String = db
            .conn
            .query_row(
                "SELECT reason FROM wallet_exclusions WHERE proxy_wallet = '0xneg'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reason, "INSUFFICIENT_PNL");
    }

    #[test]
    fn test_classify_wallet_unclassified() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let features = WalletFeatures {
            proxy_wallet: "0xmid".to_string(),
            window_days: 30,
            trade_count: 50,
            win_count: 25,
            loss_count: 25,
            total_pnl: 20.0,
            avg_position_size: 100.0,
            unique_markets: 15,
            avg_hold_time_hours: 12.0,
            max_drawdown_pct: 8.0,
            trades_per_week: 12.0,
            trades_per_day: 12.0 / 7.0,
            sharpe_ratio: 0.7,
            active_positions: 8,
            concentration_ratio: 0.50,
            avg_trade_size_usdc: 100.0,
            size_cv: 0.0,
            buy_sell_balance: 0.0,
            mid_fill_ratio: 0.0,
            extreme_price_ratio: 0.0,
            burstiness_top_1h_ratio: 0.0,
            top_domain: None,
            top_domain_ratio: 0.0,
        };

        let config = PersonaConfig::default_for_test();
        let result = classify_wallet(&db.conn, &features, 180, &config).unwrap();

        assert_eq!(result, ClassificationResult::Unclassified);
    }

    #[test]
    fn test_classify_wallet_records_traits() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let features = WalletFeatures {
            proxy_wallet: "0xtrait".to_string(),
            window_days: 30,
            trade_count: 60,
            win_count: 42,
            loss_count: 18,
            total_pnl: 900.0,
            avg_position_size: 300.0,
            unique_markets: 6,
            avg_hold_time_hours: 72.0,
            max_drawdown_pct: 5.0,
            trades_per_week: 3.0,
            trades_per_day: 0.4,
            sharpe_ratio: 1.2,
            active_positions: 3,
            concentration_ratio: 0.7,
            avg_trade_size_usdc: 300.0,
            size_cv: 0.2,
            buy_sell_balance: 0.5,
            mid_fill_ratio: 0.3,
            extreme_price_ratio: 0.8,
            burstiness_top_1h_ratio: 0.1,
            top_domain: Some("sports".to_string()),
            top_domain_ratio: 0.9,
        };

        let config = PersonaConfig::default_for_test();
        let _ = classify_wallet(&db.conn, &features, 180, &config).unwrap();

        let traits: Vec<(String, String)> = db
            .conn
            .prepare(
                "SELECT trait_key, trait_value FROM wallet_persona_traits WHERE proxy_wallet = '0xtrait'",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();

        assert!(traits.contains(&("TOPIC_LANE".to_string(), "sports".to_string())));
        assert!(traits.contains(&("BONDER".to_string(), "1".to_string())));
        assert!(traits.contains(&("WHALE".to_string(), "1".to_string())));
    }
}
