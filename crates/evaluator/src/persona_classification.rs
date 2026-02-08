use anyhow::Result;
use rusqlite::Connection;

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
    },
    NoiseTrader {
        trades_per_week: f64,
        abs_roi: f64,
    },
    SniperInsider {
        age_days: u32,
        win_rate: f64,
        trade_count: u32,
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
        }
    }

    pub fn metric_value(&self) -> f64 {
        match self {
            Self::TooYoung { age_days, .. } => *age_days as f64,
            Self::TooFewTrades { total, .. } => *total as f64,
            Self::Inactive {
                days_since_last, ..
            } => *days_since_last as f64,
            Self::ExecutionMaster {
                execution_pnl_ratio,
                ..
            } => *execution_pnl_ratio,
            Self::TailRiskSeller { win_rate, .. } => *win_rate,
            Self::NoiseTrader {
                trades_per_week, ..
            } => *trades_per_week,
            Self::SniperInsider { win_rate, .. } => *win_rate,
        }
    }

    pub fn threshold(&self) -> f64 {
        match self {
            Self::TooYoung { min_required, .. } => *min_required as f64,
            Self::TooFewTrades { min_required, .. } => *min_required as f64,
            Self::Inactive { max_allowed, .. } => *max_allowed as f64,
            Self::ExecutionMaster { threshold, .. } => *threshold,
            Self::TailRiskSeller { .. } => 0.0,
            Self::NoiseTrader { .. } => 0.0,
            Self::SniperInsider { .. } => 0.0,
        }
    }
}

#[allow(dead_code)] // Wired into scheduler in Task 21
#[derive(Debug, Clone)]
pub struct Stage1Config {
    pub min_wallet_age_days: u32,
    pub min_total_trades: u32,
    pub max_inactive_days: u32,
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
         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        rusqlite::params![
            proxy_wallet,
            reason.reason_str(),
            reason.metric_value(),
            reason.threshold(),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::db::Database;

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
                max_loss_ratio: 8.0
            }
            .reason_str(),
            "TAIL_RISK_SELLER"
        );
        assert_eq!(
            ExclusionReason::NoiseTrader {
                trades_per_week: 60.0,
                abs_roi: 0.005
            }
            .reason_str(),
            "NOISE_TRADER"
        );
        assert_eq!(
            ExclusionReason::SniperInsider {
                age_days: 15,
                win_rate: 0.90,
                trade_count: 12
            }
            .reason_str(),
            "SNIPER_INSIDER"
        );
    }
}
