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

// Helper to truncate wallet addresses
pub fn shorten_wallet(addr: &str) -> String {
    if addr.len() > 10 {
        format!("{}..{}", &addr[..6], &addr[addr.len() - 4..])
    } else {
        addr.to_string()
    }
}

/// Code-derived tooltip text for each funnel stage (see queries.rs, pipeline_jobs, market_scoring, wallet_discovery, paper_trading, wallet_scoring).
const FUNNEL_STAGE_INFO: [&str; 6] = [
    "Markets fetched from Gamma API: open only, min liquidity/volume, end date ≥ tomorrow. All passing markets are stored.",
    "Markets scored today with MScore (liquidity, volume, density, time-to-expiry). Only top N by score (config: top_n_markets) are written to market_scores_daily.",
    "Wallets discovered from today's scored markets: top holders from Data API + traders with ≥ min_total_trades in that market; capped per market (config).",
    "Wallets with is_active=1. Trades, activity, positions and holders are ingested only for these watchlist wallets.",
    "Total paper trades. Each row is one mirrored trade: paper tick copies trades_raw for tracked wallets when risk rules pass (position size, exposure caps, portfolio stop).",
    "Wallets with a WScore row today. Score is computed from paper PnL over configured windows (edge + consistency; hit-rate penalty).",
];

impl FunnelCounts {
    pub fn to_stages(&self) -> Vec<FunnelStage> {
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
            FUNNEL_STAGE_INFO.len(),
            "funnel stages and FUNNEL_STAGE_INFO must stay in sync; add an info string for each stage"
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
                    info: FUNNEL_STAGE_INFO[i].to_string(),
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
        let stages = counts.to_stages();
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
        let stages = counts.to_stages();
        assert_eq!(stages.len(), FUNNEL_STAGE_INFO.len());
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
        let stages = counts.to_stages();
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
        let stages = counts.to_stages();
        assert!(stages.last().unwrap().drop_pct.is_none());
    }
}
