use crate::wallet_features::{compute_wallet_features, WalletFeatures};
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
    pub top_domain_ratio: f64,
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
    let now = chrono::Utc::now().timestamp();
    let features = compute_wallet_features(conn, proxy_wallet, cfg.paper_window_days, now)?;

    let total_closed = features.win_count + features.loss_count;
    if (total_closed as usize) < cfg.required_paper_trades {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "not_enough_closed_trades".to_string(),
        });
    }

    let avg_pnl = if total_closed > 0 {
        features.total_pnl / f64::from(total_closed)
    } else {
        0.0
    };
    if avg_pnl < cfg.min_paper_profit_per_trade {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "onchain_profit_too_low".to_string(),
        });
    }

    if features.max_drawdown_pct / 100.0 > cfg.max_paper_drawdown {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "onchain_drawdown_too_high".to_string(),
        });
    }

    Ok(WalletRuleDecision {
        allow: true,
        reason: "onchain_validation_passed".to_string(),
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

    // Inactivity check (reads trades_raw — on-chain data)
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

    // Drawdown check from on-chain features (FIFO paired trades)
    let features = compute_wallet_features(conn, proxy_wallet, 90, now_epoch)?;
    if features.max_drawdown_pct / 100.0 > cfg.live_max_drawdown {
        return Ok(WalletRuleDecision {
            allow: false,
            reason: "live_drawdown_breach".to_string(),
        });
    }

    // Style drift check (reads wallet_features_daily)
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
                if current.top_domain_ratio > cfg.live_max_theme_concentration {
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
        top_domain_ratio: features.top_domain_ratio,
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
                top_domain_ratio
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
        top_domain_ratio: top,
    }))
}

pub fn style_drift_score(base: &StyleSnapshot, cur: &StyleSnapshot) -> f64 {
    let tpd = ((base.trades_per_day - cur.trades_per_day).abs() / 80.0).min(1.0);
    let mkts =
        ((f64::from(base.unique_markets) - f64::from(cur.unique_markets)).abs() / 50.0).min(1.0);
    let burst =
        ((base.burstiness_top_1h_ratio - cur.burstiness_top_1h_ratio).abs() / 0.50).min(1.0);
    let bal = ((base.buy_sell_balance - cur.buy_sell_balance).abs() / 1.0).min(1.0);
    let theme = ((base.top_domain_ratio - cur.top_domain_ratio).abs() / 1.0).min(1.0);
    0.30 * tpd + 0.20 * mkts + 0.25 * burst + 0.15 * bal + 0.10 * theme
}

#[cfg(test)]
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
            top_domain: Some("sports".to_string()),
            top_domain_ratio: 0.7,
            profitable_markets: 12,
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
    fn test_paper_trading_to_approved_on_onchain_pass() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        let now = chrono::Utc::now().timestamp();
        // Insert 35 BUY/SELL paired round-trips across markets (>= required_paper_trades=30)
        // All within paper_window_days=14 days
        for i in 0..35 {
            let cid = format!("m{i}");
            let offset = i64::from(i * 1000);
            db.conn
                .execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES ('0xw', ?1, 'BUY', 10.0, 0.50, ?2)",
                    rusqlite::params![cid, now - 86400 * 10 + offset],
                )
                .unwrap();
            db.conn
                .execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES ('0xw', ?1, 'SELL', 10.0, 0.51, ?2)",
                    rusqlite::params![cid, now - 86400 * 5 + offset],
                )
                .unwrap();
        }
        let cfg = default_rules();
        let decision = evaluate_paper(&db.conn, "0xw", &cfg).unwrap();
        assert!(decision.allow, "reason: {}", decision.reason);
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
