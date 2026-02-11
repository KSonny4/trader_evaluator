//! View models for dashboard templates.
//! These are the typed structs that templates render — no DB or business logic here.
// Allow dead_code while handlers are being built incrementally (tasks 5-12).
// Will be removed when all handlers are wired up.
#![allow(dead_code)]

/// Funnel counts for the summary bar
pub struct FunnelCounts {
    pub markets_fetched: i64,
    pub markets_scored: i64,
    pub wallets_discovered: i64,
    pub wallets_active: i64,
    pub paper_trades_total: i64,
    pub wallets_ranked: i64,
}

/// Persona funnel counts for Strategy Bible §2 (drop-offs through Stage 1/2 to paper/follow-worthy).
pub struct PersonaFunnelCounts {
    pub wallets_discovered: i64,
    pub stage1_passed: i64,
    pub stage2_classified: i64,
    pub paper_traded_wallets: i64,
    pub follow_worthy_wallets: i64,
}

/// One stage in the persona funnel bar.
pub struct PersonaFunnelStage {
    pub label: String,
    pub count: i64,
    /// Drop-off percentage to next stage (None for last stage)
    pub drop_pct: Option<String>,
    /// Tailwind bg color class
    pub bg_color: String,
    /// Tailwind text color class for drop-off
    pub drop_color: String,
}

/// One stage in the funnel bar
pub struct FunnelStage {
    pub label: String,
    pub count: i64,
    /// Current stage processed value.
    pub processed: i64,
    /// Previous stage total value used for processed/total.
    pub total: i64,
    /// Processed/total as percentage when total > 0.
    pub pct: Option<f64>,
    /// "markets" or "wallets"
    pub unit_kind: String,
    /// True when previous and current stage units differ.
    pub unit_change_from_prev: bool,
    /// Drop-off percentage to next stage (None for last stage)
    pub drop_pct: Option<String>,
    /// Tailwind bg color class
    pub bg_color: String,
    /// Tailwind text color class for drop-off
    pub drop_color: String,
    /// Tooltip text: code-derived criteria and what this stage represents
    pub info: String,
}

/// Canonical combined funnel counts used for the unified pipeline bar.
pub struct UnifiedFunnelCounts {
    pub markets_fetched: i64,
    pub markets_scored_today: i64,
    pub wallets_discovered: i64,
    pub stage1_passed: i64,
    pub stage2_classified: i64,
    pub paper_active_followable: i64,
    pub follow_worthy_wallets: i64,
    pub human_approval_wallets: i64,
    pub live_wallets: i64,
}

/// A job heartbeat for the status strip
pub struct JobHeartbeat {
    pub name: String,
    pub short_name: String,
    pub last_run: Option<String>,
    /// Tailwind color class for the dot
    pub color: String,
}

/// System status info
pub struct SystemStatus {
    pub db_size_mb: String,
    pub phase: String,
    pub jobs: Vec<JobHeartbeat>,
}

/// Row in the top markets table
pub struct MarketRow {
    pub rank: i64,
    pub title: String,
    pub condition_id: String,
    pub mscore: f64,
    pub liquidity: f64,
    pub volume: f64,
    pub density_score: f64,
    pub end_date: Option<String>,
}

/// Wallet discovery overview counts
pub struct WalletOverview {
    pub total: i64,
    pub active: i64,
    pub from_holder: i64,
    pub from_trader: i64,
    pub from_leaderboard: i64,
    pub discovered_today: i64,
}

/// Row in the wallets table
pub struct WalletRow {
    pub proxy_wallet: String,
    pub wallet_short: String,
    pub discovered_from: String,
    pub discovered_market_title: Option<String>,
    pub discovered_at: String,
    pub is_active: bool,
    pub trade_count: i64,
}

/// Tracking health per data type
pub struct TrackingHealth {
    pub data_type: String,
    pub count_last_1h: i64,
    pub count_last_24h: i64,
    pub last_ingested: Option<String>,
    /// Tailwind color class
    pub status_color: String,
}

/// Paper trade row
pub struct PaperTradeRow {
    pub proxy_wallet: String,
    pub wallet_short: String,
    pub market_title: String,
    pub source_notional_display: String,
    pub side: String,
    pub side_color: String,
    pub size_display: String,
    pub price_display: String,
    pub status: String,
    pub status_color: String,
    pub pnl: Option<f64>,
    pub pnl_display: String,
    pub pnl_color: String,
    pub created_at: String,
}

/// Paper portfolio summary
pub struct PaperSummary {
    pub total_pnl: f64,
    pub pnl_display: String,
    pub open_positions: i64,
    pub settled_wins: i64,
    pub settled_losses: i64,
    pub bankroll: f64,
    pub bankroll_display: String,
    pub pnl_color: String,
    pub wallets_followed: i64,
    pub exposure_usdc: f64,
    pub exposure_display: String,
    pub exposure_pct_display: String,
    pub copy_fidelity_display: String,
    pub follower_slippage_display: String,
    pub sizing_mode_display: String,
    pub sizing_estimator_bankroll_display: String,
    pub risk_status: String,
    pub risk_status_color: String,
}

/// Wallet ranking row
pub struct RankingRow {
    pub rank: i64,
    pub rank_display: String,
    pub row_class: String,
    pub proxy_wallet: String,
    pub wallet_short: String,
    pub wscore: f64,
    pub wscore_display: String,
    pub wscore_pct: String,
    pub edge_score: f64,
    pub edge_display: String,
    pub consistency_score: f64,
    pub consistency_display: String,
    pub trade_count: i64,
    pub paper_pnl: f64,
    pub pnl_display: String,
    pub pnl_color: String,
    pub follow_mode: String,
}

/// Row in the excluded wallets list (latest exclusion per wallet).
pub struct ExcludedWalletRow {
    pub proxy_wallet: String,
    pub wallet_short: String,
    pub reason: String,
    pub metric_value_display: String,
    pub threshold_display: String,
    pub excluded_at: String,
}

pub struct JourneyEvent {
    pub at: String,
    pub label: String,
    pub detail: String,
}

pub struct WalletJourney {
    pub proxy_wallet: String,
    pub wallet_short: String,
    pub discovered_from: String,
    pub discovered_market_title: Option<String>,
    pub discovered_at: String,
    pub persona: Option<String>,
    pub confidence_display: Option<String>,
    pub exclusion_reason: Option<String>,
    pub paper_pnl_display: String,
    pub exposure_display: String,
    pub copy_fidelity_display: String,
    pub follower_slippage_display: String,
    pub events: Vec<JourneyEvent>,
}

// Helper to truncate wallet addresses
pub fn shorten_wallet(addr: &str) -> String {
    if addr.len() > 10 {
        format!("{}..{}", &addr[..6], &addr[addr.len() - 4..])
    } else {
        addr.to_string()
    }
}

impl FunnelCounts {
    pub fn to_stages(&self, infos: &[String]) -> Vec<FunnelStage> {
        let pairs = [
            ("Markets", self.markets_fetched, self.markets_scored),
            ("Scored", self.markets_scored, self.wallets_discovered),
            ("Wallets", self.wallets_discovered, self.wallets_active),
            ("Tracked", self.wallets_active, self.paper_trades_total),
            ("Paper", self.paper_trades_total, self.wallets_ranked),
            ("Ranked", self.wallets_ranked, 0),
        ];
        debug_assert_eq!(
            pairs.len(),
            infos.len(),
            "funnel stages and infos must stay in sync; add an info string for each stage"
        );

        pairs
            .iter()
            .enumerate()
            .map(|(i, (label, count, next))| {
                let is_last = i == pairs.len() - 1;
                let (drop_pct, drop_color) = if is_last || *count == 0 {
                    (None, String::new())
                } else {
                    let pct = (*next as f64 / *count as f64) * 100.0;
                    let color = if pct > 50.0 {
                        "text-green-400"
                    } else if pct > 10.0 {
                        "text-yellow-400"
                    } else {
                        "text-red-400"
                    };
                    (Some(format!("{pct:.1}%")), color.to_string())
                };

                let bg = if *count > 0 {
                    "bg-gray-800"
                } else {
                    "bg-gray-900"
                };

                FunnelStage {
                    label: label.to_string(),
                    count: *count,
                    processed: *count,
                    total: *count,
                    pct: if *count > 0 { Some(100.0) } else { None },
                    unit_kind: "legacy".to_string(),
                    unit_change_from_prev: false,
                    drop_pct,
                    bg_color: bg.to_string(),
                    drop_color,
                    info: infos[i].clone(),
                }
            })
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FunnelUnitKind {
    Markets,
    Wallets,
}

impl FunnelUnitKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Markets => "markets",
            Self::Wallets => "wallets",
        }
    }
}

impl UnifiedFunnelCounts {
    pub fn to_stages(&self) -> Vec<FunnelStage> {
        let stages: [(&str, i64, FunnelUnitKind, &str); 9] = [
            (
                "Markets fetched",
                self.markets_fetched,
                FunnelUnitKind::Markets,
                "All markets currently stored in the DB.",
            ),
            (
                "Markets scored today",
                self.markets_scored_today,
                FunnelUnitKind::Markets,
                "Markets with a score row for today.",
            ),
            (
                "Wallets discovered",
                self.wallets_discovered,
                FunnelUnitKind::Wallets,
                "All discovered wallets in the watchlist table.",
            ),
            (
                "Stage 1 passed",
                self.stage1_passed,
                FunnelUnitKind::Wallets,
                "Active wallets that passed Stage 1 filters.",
            ),
            (
                "Stage 2 classified",
                self.stage2_classified,
                FunnelUnitKind::Wallets,
                "Stage 1-passed wallets with either a persona or a non-Stage-1 exclusion.",
            ),
            (
                "Paper active (followable now)",
                self.paper_active_followable,
                FunnelUnitKind::Wallets,
                "Wallets currently followable now (active + latest persona newer than latest exclusion).",
            ),
            (
                "Follow-worthy",
                self.follow_worthy_wallets,
                FunnelUnitKind::Wallets,
                "Wallets meeting current follow-worthy ROI thresholds.",
            ),
            (
                "Human approval",
                self.human_approval_wallets,
                FunnelUnitKind::Wallets,
                "Placeholder: approvals are not implemented yet.",
            ),
            (
                "Live",
                self.live_wallets,
                FunnelUnitKind::Wallets,
                "Placeholder: live-follow table is not implemented yet.",
            ),
        ];

        stages
            .iter()
            .enumerate()
            .map(|(i, (label, count, unit_kind, info))| {
                let prev = i
                    .checked_sub(1)
                    .map_or((*count, *unit_kind), |p| (stages[p].1, stages[p].2));
                let total = prev.0;
                let pct = if total > 0 {
                    Some(100.0 * *count as f64 / total as f64)
                } else {
                    None
                };
                let unit_change_from_prev = i > 0 && prev.1 != *unit_kind;

                let drop_pct = stages.get(i + 1).and_then(|next| {
                    if *count > 0 {
                        Some(format!("{:.1}%", 100.0 * next.1 as f64 / *count as f64))
                    } else {
                        None
                    }
                });

                let drop_color = match drop_pct
                    .as_deref()
                    .and_then(|s| s.strip_suffix('%'))
                    .and_then(|n| n.parse::<f64>().ok())
                {
                    Some(p) if p > 50.0 => "text-green-400".to_string(),
                    Some(p) if p > 10.0 => "text-yellow-400".to_string(),
                    Some(_) => "text-red-400".to_string(),
                    None => String::new(),
                };

                FunnelStage {
                    label: (*label).to_string(),
                    count: *count,
                    processed: *count,
                    total,
                    pct,
                    unit_kind: unit_kind.as_str().to_string(),
                    unit_change_from_prev,
                    drop_pct,
                    bg_color: if *count > 0 {
                        "bg-gray-800".to_string()
                    } else {
                        "bg-gray-900".to_string()
                    },
                    drop_color,
                    info: (*info).to_string(),
                }
            })
            .collect()
    }
}

impl PersonaFunnelCounts {
    pub fn to_stages(&self) -> Vec<PersonaFunnelStage> {
        let pairs = [
            ("Discovered", self.wallets_discovered, self.stage1_passed),
            ("Stage 1", self.stage1_passed, self.stage2_classified),
            ("Stage 2", self.stage2_classified, self.paper_traded_wallets),
            (
                "Paper",
                self.paper_traded_wallets,
                self.follow_worthy_wallets,
            ),
            ("Follow", self.follow_worthy_wallets, 0),
        ];

        pairs
            .iter()
            .enumerate()
            .map(|(i, (label, count, next))| {
                let is_last = i == pairs.len() - 1;
                let (drop_pct, drop_color) = if is_last || *count == 0 {
                    (None, String::new())
                } else {
                    let pct = (*next as f64 / *count as f64) * 100.0;
                    let color = if pct > 50.0 {
                        "text-green-400"
                    } else if pct > 10.0 {
                        "text-yellow-400"
                    } else {
                        "text-red-400"
                    };
                    (Some(format!("{pct:.1}%")), color.to_string())
                };

                let bg = if *count > 0 {
                    "bg-gray-800"
                } else {
                    "bg-gray-900"
                };

                PersonaFunnelStage {
                    label: (*label).to_string(),
                    count: *count,
                    drop_pct,
                    bg_color: bg.to_string(),
                    drop_color,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shorten_wallet() {
        assert_eq!(shorten_wallet("0xabcdef1234567890"), "0xabcd..7890");
        assert_eq!(shorten_wallet("0x123"), "0x123");
    }

    #[test]
    fn test_funnel_stages_count() {
        let counts = FunnelCounts {
            markets_fetched: 100,
            markets_scored: 20,
            wallets_discovered: 50,
            wallets_active: 40,
            paper_trades_total: 5,
            wallets_ranked: 3,
        };
        let infos = vec!["x".to_string(); 6];
        let stages = counts.to_stages(&infos);
        assert_eq!(stages.len(), 6);
        assert_eq!(stages[0].label, "Markets");
        assert_eq!(stages[0].count, 100);
        assert_eq!(stages[0].processed, 100);
        assert_eq!(stages[0].total, 100);
        assert_eq!(stages[0].unit_kind, "legacy");
        assert!(!stages[0].unit_change_from_prev);
        // 20/100 = 20%
        assert_eq!(stages[0].drop_pct.as_deref(), Some("20.0%"));
        assert_eq!(stages[0].drop_color, "text-yellow-400");
        assert!(!stages[0].info.is_empty());
    }

    #[test]
    fn test_funnel_stages_have_info_tooltips() {
        let counts = FunnelCounts {
            markets_fetched: 1,
            markets_scored: 1,
            wallets_discovered: 1,
            wallets_active: 1,
            paper_trades_total: 1,
            wallets_ranked: 1,
        };
        let infos = vec!["x".to_string(); 6];
        let stages = counts.to_stages(&infos);
        assert_eq!(stages.len(), infos.len());
        for (i, stage) in stages.iter().enumerate() {
            assert!(
                !stage.info.is_empty(),
                "stage {} ({}) must have non-empty info",
                i,
                stage.label
            );
        }
    }

    #[test]
    fn test_funnel_stages_zero_markets() {
        let counts = FunnelCounts {
            markets_fetched: 0,
            markets_scored: 0,
            wallets_discovered: 0,
            wallets_active: 0,
            paper_trades_total: 0,
            wallets_ranked: 0,
        };
        let infos = vec!["x".to_string(); 6];
        let stages = counts.to_stages(&infos);
        // Zero count => no drop-off
        assert!(stages[0].drop_pct.is_none());
    }

    #[test]
    fn test_funnel_last_stage_no_dropoff() {
        let counts = FunnelCounts {
            markets_fetched: 10,
            markets_scored: 5,
            wallets_discovered: 3,
            wallets_active: 2,
            paper_trades_total: 1,
            wallets_ranked: 1,
        };
        let infos = vec!["x".to_string(); 6];
        let stages = counts.to_stages(&infos);
        assert!(stages.last().unwrap().drop_pct.is_none());
    }

    #[test]
    fn test_unified_funnel_stages_have_expected_shape() {
        let counts = UnifiedFunnelCounts {
            markets_fetched: 100,
            markets_scored_today: 20,
            wallets_discovered: 80,
            stage1_passed: 50,
            stage2_classified: 40,
            paper_active_followable: 10,
            follow_worthy_wallets: 5,
            human_approval_wallets: 0,
            live_wallets: 0,
        };
        let stages = counts.to_stages();
        assert_eq!(stages.len(), 9);
        assert_eq!(stages[0].label, "Markets fetched");
        assert_eq!(stages[1].label, "Markets scored today");
        assert_eq!(stages[2].label, "Wallets discovered");
        assert_eq!(stages[2].unit_kind, "wallets");
        assert!(stages[2].unit_change_from_prev);
        assert_eq!(stages[2].processed, 80);
        assert_eq!(stages[2].total, 20);
        assert!(stages[2].pct.unwrap() > 0.0);
    }

    #[test]
    fn test_unified_funnel_placeholder_stages_are_last_two() {
        let counts = UnifiedFunnelCounts {
            markets_fetched: 1,
            markets_scored_today: 1,
            wallets_discovered: 1,
            stage1_passed: 1,
            stage2_classified: 1,
            paper_active_followable: 1,
            follow_worthy_wallets: 1,
            human_approval_wallets: 0,
            live_wallets: 0,
        };
        let stages = counts.to_stages();
        assert_eq!(stages[7].label, "Human approval");
        assert_eq!(stages[7].count, 0);
        assert_eq!(stages[8].label, "Live");
        assert_eq!(stages[8].count, 0);
    }
}
