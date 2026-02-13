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

/// Unified funnel: Events → All wallets → Suitable personas → Actively paper traded → Worth following.
pub struct UnifiedFunnelCounts {
    /// Distinct events selected (top N written to market_scores)
    pub events_selected: i64,
    /// Total events evaluated before truncation (from scoring_stats, or events_selected when absent)
    pub events_evaluated: i64,
    pub all_wallets: i64,
    pub suitable_personas: i64,
    /// Wallets that passed Stage 1 and have been classified (persona or non-stage1 exclusion), with oldest trade at least 30 days ago (trade-based age, not scrape age).
    pub personas_evaluated: i64,
    /// Distinct wallets with an exclusion (Stage 1 or Stage 2).
    pub personas_excluded: i64,
    pub actively_paper_traded: i64,
    pub worth_following: i64,
}

/// One stage in the unified funnel bar (counts only, no drop %).
pub struct UnifiedFunnelStage {
    pub label: String,
    /// Display string (e.g. "50" or "50 / 127" for selected/evaluated)
    pub count_display: String,
    pub bg_color: String,
}

/// Wallet with persona for suitable-personas stage.
pub struct SuitablePersonaRow {
    pub proxy_wallet: String,
    pub wallet_short: String,
    pub persona: String,
    pub classified_at: String,
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
    /// Drop-off percentage to next stage (None for last stage)
    pub drop_pct: Option<String>,
    /// Tailwind bg color class
    pub bg_color: String,
    /// Tailwind text color class for drop-off
    pub drop_color: String,
    /// Tooltip text: code-derived criteria and what this stage represents
    pub info: String,
}

/// A job heartbeat for the status strip
pub struct JobHeartbeat {
    pub name: String,
    pub short_name: String,
    pub last_run: Option<String>,
    /// Tailwind color class for the dot
    pub color: String,
}

/// Job status row for the async task visualization
#[derive(serde::Serialize)]
pub struct JobStatusRow {
    pub job_name: String,
    pub status: String,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub last_error: Option<String>,
    pub duration_ms: Option<i64>,
    pub metadata: Option<String>,
    pub updated_at: Option<String>,
}

impl JobStatusRow {
    /// Format metadata for display in the UI
    pub fn progress_display(&self) -> String {
        let Some(meta) = &self.metadata else {
            return String::new();
        };

        let Ok(json) = serde_json::from_str::<serde_json::Value>(meta) else {
            return meta.clone();
        };

        match self.job_name.as_str() {
            "wallet_discovery" => {
                // In-progress: {"progress": 10, "total": 100, "inserted": 5, "phase": "discovering_wallets"}
                if let (Some(progress), Some(total), Some(inserted)) = (
                    json.get("progress").and_then(serde_json::Value::as_i64),
                    json.get("total").and_then(serde_json::Value::as_i64),
                    json.get("inserted").and_then(serde_json::Value::as_i64),
                ) {
                    return format!("{progress}/{total} markets ({inserted} inserted)");
                }
                // Completed: {"inserted": 261, "total": 392, "completed": true}
                if let (Some(inserted), Some(total)) = (
                    json.get("inserted").and_then(serde_json::Value::as_i64),
                    json.get("total").and_then(serde_json::Value::as_i64),
                ) {
                    return format!("{inserted} wallets inserted ({total} markets)");
                }
            }
            "persona_classification" => {
                // In-progress: {"processed": 5000, "total": 21239, "suitable": 1200, "phase": "classifying"}
                if let (Some(processed), Some(total), Some(suitable)) = (
                    json.get("processed").and_then(serde_json::Value::as_i64),
                    json.get("total").and_then(serde_json::Value::as_i64),
                    json.get("suitable").and_then(serde_json::Value::as_i64),
                ) {
                    return format!("{processed}/{total} wallets ({suitable} suitable)");
                }
                // Completed: {"classified": 17489, "suitable": 1234, "stage1_excluded": 15000, "stage2_excluded": 1255, "completed": true}
                if let (Some(classified), Some(suitable)) = (
                    json.get("classified").and_then(serde_json::Value::as_i64),
                    json.get("suitable").and_then(serde_json::Value::as_i64),
                ) {
                    let stage1 = json
                        .get("stage1_excluded")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0);
                    let stage2 = json
                        .get("stage2_excluded")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0);
                    let excluded = stage1 + stage2;
                    return format!(
                        "{classified} classified ({suitable} suitable, {excluded} excluded)"
                    );
                }
            }
            _ => {}
        }

        // Fallback: return raw JSON
        meta.clone()
    }
}

/// System status info
pub struct SystemStatus {
    pub db_size_mb: String,
    pub phase: String,
    pub jobs: Vec<JobHeartbeat>,
    /// Events display: "50" or "50 / 127" (selected / evaluated)
    pub events_display: String,
}

/// Last completed run stats for the "async funnel" (wallets/markets/trades processed).
pub struct LastRunStats {
    pub trades_wallets: i64,
    pub trades_inserted: i64,
    pub events_markets: i64,
    pub trades_run_at: Option<String>,
    pub events_run_at: Option<String>,
}

/// Row in the top events table (events = grouped by event_slug, or single market when no event_slug)
pub struct EventRow {
    pub rank: i64,
    pub title: String,
    pub event_key: String, // event_slug or condition_id
    pub best_mscore: f64,
    pub market_count: i64,
    pub polymarket_url: Option<String>,
}

/// Row in the top markets table (legacy; kept for tests)
pub struct MarketRow {
    pub rank: i64,
    pub title: String,
    pub condition_id: String,
    pub mscore: f64,
    pub liquidity: f64,
    pub volume: f64,
    pub density_score: f64,
    pub end_date: Option<String>,
    /// Polymarket URL (event or market), None if no slug
    pub polymarket_url: Option<String>,
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

/// One aggregated position (grouped by condition_id + outcome) for the wallet scorecard.
#[derive(serde::Serialize)]
pub struct WalletPositionRow {
    pub condition_id: String,
    pub market_title: Option<String>,
    pub outcome: Option<String>,
    pub shares_display: String,
    pub avg_price_display: String,
    pub total_bet_display: String,
    pub trade_count: u32,
    /// Polymarket URL for this market (event or market page)
    pub polymarket_url: Option<String>,
}

/// One row from trades_raw for the wallet scorecard.
#[derive(serde::Serialize)]
pub struct WalletTradeRow {
    pub id: i64,
    pub condition_id: String,
    pub market_title: Option<String>,
    pub side: String,
    pub size_display: String,
    pub price_display: String,
    pub timestamp_display: String,
    pub outcome: Option<String>,
    /// Polygonscan URL for this trade's on-chain transaction
    pub polygonscan_url: Option<String>,
}

/// One activity row from activity_raw for the wallet scorecard Activity tab.
#[derive(serde::Serialize)]
pub struct WalletActivityRow {
    pub activity_type: String,
    pub condition_id: Option<String>,
    pub market_title: Option<String>,
    pub outcome: Option<String>,
    pub shares_display: String,
    pub usdc_amount_display: String,
    pub timestamp_display: String,
    /// Polygonscan URL for this activity's on-chain transaction
    pub polygonscan_url: Option<String>,
    /// Polymarket URL for this market (event or market page)
    pub polymarket_url: Option<String>,
}

pub struct WalletJourney {
    pub proxy_wallet: String,
    pub wallet_short: String,
    /// Display label: Polymarket profile name if set, otherwise wallet_short.
    pub wallet_display_label: String,
    pub discovered_at: String,
    /// When we last ingested trades for this wallet (MAX(ingested_at) in trades_raw). We never have "all" trades; this is when we last fetched.
    pub last_trades_ingestion_at: Option<String>,
    pub persona: Option<String>,
    pub confidence_display: Option<String>,
    pub exclusion_reason: Option<String>,
    /// Wallet rules engine state: CANDIDATE, PAPER_TRADING, APPROVED, STOPPED.
    pub pipeline_state: String,
    pub paper_pnl_display: String,
    pub exposure_display: String,
    pub copy_fidelity_display: String,
    pub follower_slippage_display: String,
    pub events: Vec<JourneyEvent>,
    /// Active positions (net_shares > 0.5) — initial page of 20.
    pub active_positions: Vec<WalletPositionRow>,
    pub total_active_positions_count: usize,
    /// Closed positions (net_shares <= 0.5) — initial page of 20.
    pub closed_positions: Vec<WalletPositionRow>,
    pub total_closed_positions_count: usize,
    /// Activity feed from activity_raw — initial page of 20.
    pub activities: Vec<WalletActivityRow>,
    pub total_activities_count: usize,
    /// Trades from trades_raw for this wallet (newest first; for collapsible "All trades" detail).
    pub trades: Vec<WalletTradeRow>,
    /// Total number of trades in trades_raw for this wallet (for "All trades (N)" and load-more).
    pub total_trades_count: usize,
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
                    drop_pct,
                    bg_color: bg.to_string(),
                    drop_color,
                    info: infos[i].clone(),
                }
            })
            .collect()
    }
}

impl UnifiedFunnelCounts {
    pub fn to_stages(&self) -> Vec<UnifiedFunnelStage> {
        let events_display =
            if self.events_evaluated > 0 && self.events_evaluated != self.events_selected {
                format!("{} / {}", self.events_selected, self.events_evaluated)
            } else {
                self.events_selected.to_string()
            };
        let pairs: [(&str, String, i64); 5] = [
            ("Events", events_display, self.events_selected),
            (
                "All wallets",
                self.all_wallets.to_string(),
                self.all_wallets,
            ),
            (
                "Suitable personas wallets",
                format!(
                    "{} / {} / {}",
                    self.suitable_personas, self.personas_evaluated, self.personas_excluded
                ),
                self.suitable_personas,
            ),
            (
                "Actively paper traded",
                self.actively_paper_traded.to_string(),
                self.actively_paper_traded,
            ),
            (
                "Worth following",
                self.worth_following.to_string(),
                self.worth_following,
            ),
        ];
        pairs
            .iter()
            .map(|(label, count_display, count_num)| {
                let bg = if *count_num > 0 {
                    "bg-gray-800"
                } else {
                    "bg-gray-900"
                };
                UnifiedFunnelStage {
                    label: (*label).to_string(),
                    count_display: count_display.clone(),
                    bg_color: bg.to_string(),
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
    fn test_job_status_row_progress_display_wallet_discovery_in_progress() {
        let job = JobStatusRow {
            job_name: "wallet_discovery".to_string(),
            status: "running".to_string(),
            last_run_at: None,
            next_run_at: None,
            last_error: None,
            duration_ms: None,
            metadata: Some(
                r#"{"progress":10,"total":100,"inserted":5,"phase":"discovering_wallets"}"#
                    .to_string(),
            ),
            updated_at: None,
        };
        assert_eq!(job.progress_display(), "10/100 markets (5 inserted)");
    }

    #[test]
    fn test_job_status_row_progress_display_wallet_discovery_completed() {
        let job = JobStatusRow {
            job_name: "wallet_discovery".to_string(),
            status: "idle".to_string(),
            last_run_at: None,
            next_run_at: None,
            last_error: None,
            duration_ms: None,
            metadata: Some(r#"{"inserted":261,"total":392,"completed":true}"#.to_string()),
            updated_at: None,
        };
        assert_eq!(job.progress_display(), "261 wallets inserted (392 markets)");
    }

    #[test]
    fn test_job_status_row_progress_display_persona_classification_in_progress() {
        let job = JobStatusRow {
            job_name: "persona_classification".to_string(),
            status: "running".to_string(),
            last_run_at: None,
            next_run_at: None,
            last_error: None,
            duration_ms: None,
            metadata: Some(
                r#"{"processed":5000,"total":21239,"suitable":1200,"phase":"classifying"}"#
                    .to_string(),
            ),
            updated_at: None,
        };
        assert_eq!(job.progress_display(), "5000/21239 wallets (1200 suitable)");
    }

    #[test]
    fn test_job_status_row_progress_display_persona_classification_completed() {
        let job = JobStatusRow {
            job_name: "persona_classification".to_string(),
            status: "idle".to_string(),
            last_run_at: None,
            next_run_at: None,
            last_error: None,
            duration_ms: None,
            metadata: Some(r#"{"classified":17489,"suitable":1234,"stage1_excluded":15000,"stage2_excluded":1255,"completed":true}"#.to_string()),
            updated_at: None,
        };
        assert_eq!(
            job.progress_display(),
            "17489 classified (1234 suitable, 16255 excluded)"
        );
    }

    #[test]
    fn test_job_status_row_progress_display_fallback_raw_json() {
        let job = JobStatusRow {
            job_name: "unknown_job".to_string(),
            status: "idle".to_string(),
            last_run_at: None,
            next_run_at: None,
            last_error: None,
            duration_ms: None,
            metadata: Some(r#"{"some":"data"}"#.to_string()),
            updated_at: None,
        };
        assert_eq!(job.progress_display(), r#"{"some":"data"}"#);
    }

    #[test]
    fn test_job_status_row_progress_display_no_metadata() {
        let job = JobStatusRow {
            job_name: "wallet_discovery".to_string(),
            status: "idle".to_string(),
            last_run_at: None,
            next_run_at: None,
            last_error: None,
            duration_ms: None,
            metadata: None,
            updated_at: None,
        };
        assert_eq!(job.progress_display(), "");
    }
}
