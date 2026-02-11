use crate::wallet_features::WalletFeatures;
use anyhow::Result;
use common::config::WalletRules;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletRuleState {
    Candidate,
    PaperTrading,
    Approved,
    Stopped,
}

impl WalletRuleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "CANDIDATE",
            Self::PaperTrading => "PAPER_TRADING",
            Self::Approved => "APPROVED",
            Self::Stopped => "STOPPED",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WalletRuleDecision {
    pub allow: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleSnapshot {
    pub trades_per_day: f64,
    pub unique_markets: u32,
    pub burstiness_top_1h_ratio: f64,
    pub buy_sell_balance: f64,
    pub top_category_ratio: f64,
}

pub fn evaluate_discovery(features: &WalletFeatures, cfg: &WalletRules) -> WalletRuleDecision {
    if features.trade_count < cfg.min_trades_for_discovery as u32 {
        return WalletRuleDecision {
            allow: false,
            reason: "not_enough_trades_for_discovery".to_string(),
        };
    }
    if features.trades_per_day > cfg.max_trades_per_day {
        return WalletRuleDecision {
            allow: false,
            reason: "too_active_for_discovery".to_string(),
        };
    }
    if features.unique_markets > cfg.max_distinct_markets_30d as u32 {
        return WalletRuleDecision {
            allow: false,
            reason: "too_many_markets_for_discovery".to_string(),
        };
    }
    // Hold-time is still a proxy feature in this codebase; when unavailable (0.0),
    // skip this gate to avoid hard-blocking all discovery decisions.
    if features.avg_hold_time_hours > 0.0
        && features.avg_hold_time_hours * 60.0 < cfg.min_median_hold_minutes
    {
        return WalletRuleDecision {
            allow: false,
            reason: "holding_too_short_for_discovery".to_string(),
        };
    }
    if features.size_cv > cfg.max_size_gini {
        return WalletRuleDecision {
            allow: false,
            reason: "size_profile_too_uneven".to_string(),
        };
    }
    if features.burstiness_top_1h_ratio > cfg.max_fraction_trades_at_spread_edge {
        return WalletRuleDecision {
            allow: false,
            reason: "bursty_or_speed_like".to_string(),
        };
    }
    WalletRuleDecision {
        allow: true,
        reason: "passes_discovery".to_string(),
    }
}

pub fn evaluate_paper(
    conn: &Connection,
    proxy_wallet: &str,
    cfg: &WalletRules,
) -> Result<WalletRuleDecision> {
    let window = format!("-{} days", cfg.paper_window_days);
    let pnl_values: Vec<f64> = conn
        .prepare(
            "SELECT COALESCE(pnl, 0.0) FROM paper_trades
             WHERE proxy_wallet = ?1
               AND status != 'open'
               AND created_at >= datetime('now', ?2)
             ORDER BY created_at ASC",
        )?
        .query_map(rusqlite::params![proxy_wallet, window], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    if pnl_values.len() < cfg.required_paper_trades {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "not_enough_paper_trades".to_string(),
        });
    }

    let avg_profit = mean(&pnl_values);
    if avg_profit < cfg.min_paper_profit_per_trade {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "paper_profit_too_low".to_string(),
        });
    }

    let max_dd = max_drawdown_from_pnl(&pnl_values);
    if max_dd > cfg.max_paper_drawdown {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "paper_drawdown_too_high".to_string(),
        });
    }

    let slippage_bps: Vec<f64> = conn
        .prepare(
            "SELECT their_entry_price, slippage_cents
             FROM follower_slippage
             WHERE proxy_wallet = ?1
               AND created_at >= datetime('now', ?2)",
        )?
        .query_map(rusqlite::params![proxy_wallet, window], |row| {
            let price: f64 = row.get(0)?;
            let slippage_cents: f64 = row.get(1)?;
            Ok(slippage_cents_to_bps(price, slippage_cents))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    if !slippage_bps.is_empty() && mean(&slippage_bps) > cfg.max_paper_slippage_bps {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "paper_slippage_too_high".to_string(),
        });
    }

    Ok(WalletRuleDecision {
        allow: true,
        reason: "paper_validation_passed".to_string(),
    })
}

pub fn evaluate_live(
    conn: &Connection,
    proxy_wallet: &str,
    now_epoch: i64,
    cfg: &WalletRules,
) -> Result<WalletRuleDecision> {
    if !cfg.live_breakers_enabled {
        return Ok(WalletRuleDecision {
            allow: true,
            reason: "live_breakers_disabled".to_string(),
        });
    }

    let last_seen: Option<i64> = conn
        .query_row(
            "SELECT MAX(timestamp) FROM trades_raw WHERE proxy_wallet = ?1",
            rusqlite::params![proxy_wallet],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let Some(last_seen) = last_seen else {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "live_inactive".to_string(),
        });
    };
    let inactivity_secs = i64::from(cfg.live_inactivity_days) * 86_400;
    if now_epoch - last_seen > inactivity_secs {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "live_inactive".to_string(),
        });
    }

    let pnl_values: Vec<f64> = conn
        .prepare(
            "SELECT COALESCE(pnl, 0.0) FROM paper_trades
             WHERE proxy_wallet = ?1 AND status != 'open'
             ORDER BY created_at ASC",
        )?
        .query_map(rusqlite::params![proxy_wallet], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if !pnl_values.is_empty() && max_drawdown_from_pnl(&pnl_values) > cfg.live_max_drawdown {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "live_drawdown_breach".to_string(),
        });
    }

    let recent_slippage_bps: Vec<f64> = conn
        .prepare(
            "SELECT their_entry_price, slippage_cents
             FROM follower_slippage
             WHERE proxy_wallet = ?1
             ORDER BY created_at DESC
             LIMIT 30",
        )?
        .query_map(rusqlite::params![proxy_wallet], |row| {
            let price: f64 = row.get(0)?;
            let slippage_cents: f64 = row.get(1)?;
            Ok(slippage_cents_to_bps(price, slippage_cents))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if !recent_slippage_bps.is_empty() && mean(&recent_slippage_bps) > cfg.live_slippage_bps_spike {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "live_slippage_spike".to_string(),
        });
    }

    let baseline_json: Option<String> = conn
        .query_row(
            "SELECT baseline_style_json FROM wallet_rules_state WHERE proxy_wallet = ?1",
            rusqlite::params![proxy_wallet],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(baseline_json) = baseline_json {
        if let Ok(baseline) = serde_json::from_str::<StyleSnapshot>(&baseline_json) {
            if let Some(current) = read_latest_style(conn, proxy_wallet)? {
                let drift = style_drift_score(&baseline, &current);
                if drift > cfg.live_style_drift_score {
                    return Ok(WalletRuleDecision {
                        allow: false,
                        reason: "live_style_drift".to_string(),
                    });
                }
                if current.top_category_ratio > cfg.live_max_theme_concentration {
                    return Ok(WalletRuleDecision {
                        allow: false,
                        reason: "live_theme_concentration".to_string(),
                    });
                }
            }
        }
    }

    Ok(WalletRuleDecision {
        allow: true,
        reason: "live_continue".to_string(),
    })
}

pub fn parse_state(s: &str) -> WalletRuleState {
    match s {
        "PAPER_TRADING" => WalletRuleState::PaperTrading,
        "APPROVED" => WalletRuleState::Approved,
        "STOPPED" => WalletRuleState::Stopped,
        _ => WalletRuleState::Candidate,
    }
}

pub fn read_state(conn: &Connection, proxy_wallet: &str) -> Result<WalletRuleState> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT state FROM wallet_rules_state WHERE proxy_wallet = ?1",
            rusqlite::params![proxy_wallet],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw
        .as_deref()
        .map_or(WalletRuleState::Candidate, parse_state))
}

pub fn write_state(
    conn: &Connection,
    proxy_wallet: &str,
    state: WalletRuleState,
    baseline_style_json: Option<&str>,
    last_seen_ts: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO wallet_rules_state (proxy_wallet, state, baseline_style_json, last_seen_ts, updated_at)
         VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%d %H:%M:%f', 'now'))
         ON CONFLICT(proxy_wallet) DO UPDATE SET
           state = excluded.state,
           baseline_style_json = COALESCE(excluded.baseline_style_json, wallet_rules_state.baseline_style_json),
           last_seen_ts = COALESCE(excluded.last_seen_ts, wallet_rules_state.last_seen_ts),
           updated_at = strftime('%Y-%m-%d %H:%M:%f', 'now')",
        rusqlite::params![proxy_wallet, state.as_str(), baseline_style_json, last_seen_ts],
    )?;
    Ok(())
}

pub fn record_event(
    conn: &Connection,
    proxy_wallet: &str,
    phase: &str,
    decision: &WalletRuleDecision,
    metrics_json: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO wallet_rules_events (proxy_wallet, phase, allow, reason, metrics_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%d %H:%M:%f', 'now'))",
        rusqlite::params![
            proxy_wallet,
            phase,
            if decision.allow { 1 } else { 0 },
            decision.reason,
            metrics_json
        ],
    )?;
    Ok(())
}

pub fn style_snapshot_from_features(features: &WalletFeatures) -> StyleSnapshot {
    StyleSnapshot {
        trades_per_day: features.trades_per_day,
        unique_markets: features.unique_markets,
        burstiness_top_1h_ratio: features.burstiness_top_1h_ratio,
        buy_sell_balance: features.buy_sell_balance,
        top_category_ratio: features.top_category_ratio,
    }
}

pub fn read_latest_style(conn: &Connection, proxy_wallet: &str) -> Result<Option<StyleSnapshot>> {
    let row: Option<(f64, u32, f64, f64, f64)> = conn
        .query_row(
            "SELECT
                trades_per_day,
                unique_markets,
                burstiness_top_1h_ratio,
                buy_sell_balance,
                top_category_ratio
             FROM wallet_features_daily
             WHERE proxy_wallet = ?1
             ORDER BY feature_date DESC
             LIMIT 1",
            rusqlite::params![proxy_wallet],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    Ok(row.map(|(tpd, mkts, burst, bal, top)| StyleSnapshot {
        trades_per_day: tpd,
        unique_markets: mkts,
        burstiness_top_1h_ratio: burst,
        buy_sell_balance: bal,
        top_category_ratio: top,
    }))
}

pub fn style_drift_score(base: &StyleSnapshot, cur: &StyleSnapshot) -> f64 {
    let tpd = ((base.trades_per_day - cur.trades_per_day).abs() / 80.0).min(1.0);
    let mkts =
        ((f64::from(base.unique_markets) - f64::from(cur.unique_markets)).abs() / 50.0).min(1.0);
    let burst =
        ((base.burstiness_top_1h_ratio - cur.burstiness_top_1h_ratio).abs() / 0.50).min(1.0);
    let bal = ((base.buy_sell_balance - cur.buy_sell_balance).abs() / 1.0).min(1.0);
    let theme = ((base.top_category_ratio - cur.top_category_ratio).abs() / 1.0).min(1.0);
    0.30 * tpd + 0.20 * mkts + 0.25 * burst + 0.15 * bal + 0.10 * theme
}

fn max_drawdown_from_pnl(pnl_values: &[f64]) -> f64 {
    if pnl_values.is_empty() {
        return 0.0;
    }
    let mut equity = 1.0;
    let mut peak = 1.0;
    let mut max_dd = 0.0;
    for pnl in pnl_values {
        equity = (equity + pnl).max(0.0);
        if equity > peak {
            peak = equity;
        }
        if peak > 0.0 {
            let dd = (peak - equity) / peak;
            if dd > max_dd {
                max_dd = dd;
            }
        }
    }
    max_dd
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

fn slippage_cents_to_bps(price: f64, slippage_cents: f64) -> f64 {
    let denom = price.abs().max(1e-6);
    let slippage_prob = slippage_cents.abs() / 100.0;
    (slippage_prob / denom) * 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::db::Database;

    fn default_rules() -> WalletRules {
        common::config::Config::from_toml_str(include_str!("../../../config/default.toml"))
            .unwrap()
            .wallet_rules
    }

    fn base_features() -> WalletFeatures {
        WalletFeatures {
            proxy_wallet: "0xw".to_string(),
            window_days: 30,
            trade_count: 100,
            win_count: 55,
            loss_count: 45,
            total_pnl: 100.0,
            avg_position_size: 25.0,
            unique_markets: 20,
            avg_hold_time_hours: 24.0,
            max_drawdown_pct: 5.0,
            trades_per_week: 21.0,
            trades_per_day: 3.0,
            sharpe_ratio: 1.1,
            active_positions: 3,
            concentration_ratio: 0.5,
            avg_trade_size_usdc: 25.0,
            size_cv: 0.2,
            buy_sell_balance: 0.6,
            mid_fill_ratio: 0.2,
            extreme_price_ratio: 0.1,
            burstiness_top_1h_ratio: 0.1,
            top_category: Some("sports".to_string()),
            top_category_ratio: 0.7,
        }
    }

    #[test]
    fn test_candidate_to_paper_trading_on_discovery_pass() {
        let mut f = base_features();
        f.avg_hold_time_hours = 4.0;
        let cfg = default_rules();
        let decision = evaluate_discovery(&f, &cfg);
        assert!(decision.allow);
    }
    #[test]
    fn test_candidate_stays_on_discovery_fail() {
        let mut f = base_features();
        f.trade_count = 5;
        let cfg = default_rules();
        let decision = evaluate_discovery(&f, &cfg);
        assert!(!decision.allow);
    }
    #[test]
    fn test_discovery_skips_hold_gate_when_hold_proxy_missing() {
        let mut f = base_features();
        f.avg_hold_time_hours = 0.0;
        let cfg = default_rules();
        let decision = evaluate_discovery(&f, &cfg);
        assert!(decision.allow);
    }
    #[test]
    fn test_paper_trading_to_approved_on_paper_pass() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        for i in 0..40 {
            db.conn
                .execute(
                    "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl, created_at)
                     VALUES ('0xw', 'mirror', 'm1', 'BUY', 10.0, 0.5, 'settled_win', 0.01, datetime('now', ?1))",
                    rusqlite::params![format!("-{} hour", i)],
                )
                .unwrap();
        }
        let cfg = default_rules();
        let decision = evaluate_paper(&db.conn, "0xw", &cfg).unwrap();
        assert!(decision.allow);
    }
    #[test]
    fn test_approved_to_stopped_on_live_fail() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        let mut cfg = default_rules();
        cfg.live_breakers_enabled = true;
        cfg.live_inactivity_days = 1;
        let now = 1_700_000_000i64;
        let decision = evaluate_live(&db.conn, "0xw", now, &cfg).unwrap();
        assert!(!decision.allow);
    }
    #[test]
    fn test_slippage_cents_to_bps_conversion() {
        let bps = slippage_cents_to_bps(0.5, 1.0);
        assert!((bps - 200.0).abs() < 1e-9);
    }
}
