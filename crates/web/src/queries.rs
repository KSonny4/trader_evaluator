/// SQL queries for the dashboard. All read-only.
use anyhow::Result;
use rusqlite::Connection;

use crate::models::*;

pub fn funnel_counts(conn: &Connection) -> Result<FunnelCounts> {
    let markets_fetched: i64 = conn.query_row("SELECT COUNT(*) FROM markets", [], |r| r.get(0))?;
    let markets_scored: i64 = conn.query_row(
        "SELECT COUNT(*) FROM market_scores_daily WHERE score_date = date('now')",
        [],
        |r| r.get(0),
    )?;
    let wallets_discovered: i64 =
        conn.query_row("SELECT COUNT(*) FROM wallets", [], |r| r.get(0))?;
    let wallets_active: i64 = conn.query_row(
        "SELECT COUNT(*) FROM wallets WHERE is_active = 1",
        [],
        |r| r.get(0),
    )?;
    let paper_trades_total: i64 =
        conn.query_row("SELECT COUNT(*) FROM paper_trades", [], |r| r.get(0))?;
    let wallets_ranked: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT proxy_wallet) FROM wallet_scores_daily WHERE score_date = date('now')",
        [],
        |r| r.get(0),
    )?;
    Ok(FunnelCounts {
        markets_fetched,
        markets_scored,
        wallets_discovered,
        wallets_active,
        paper_trades_total,
        wallets_ranked,
    })
}

pub fn system_status(conn: &Connection, db_path: &str) -> Result<SystemStatus> {
    let db_size_mb = std::fs::metadata(db_path)
        .map(|m| format!("{:.1}", m.len() as f64 / 1_048_576.0))
        .unwrap_or_else(|_| "?".to_string());

    // Determine phase from data presence
    let has_scores: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM market_scores_daily", [], |r| {
            r.get(0)
        })
        .unwrap_or(false);
    let has_wallets: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM wallets", [], |r| r.get(0))
        .unwrap_or(false);
    let has_trades: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM trades_raw", [], |r| r.get(0))
        .unwrap_or(false);
    let has_paper: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM paper_trades", [], |r| r.get(0))
        .unwrap_or(false);
    let has_rankings: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM wallet_scores_daily", [], |r| {
            r.get(0)
        })
        .unwrap_or(false);

    let phase = if has_rankings {
        "5: Wallet Ranking"
    } else if has_paper {
        "4: Paper Copy"
    } else if has_trades {
        "3: Long-Term Tracking"
    } else if has_wallets {
        "2: Wallet Discovery"
    } else if has_scores {
        "1: Market Discovery"
    } else {
        "0: Foundation"
    };

    // Job heartbeats: check freshness of each data source
    let job_defs: Vec<(&str, &str, &str, &str, i64)> = vec![
        (
            "Market Scoring",
            "MScore",
            "market_scores_daily",
            "score_date",
            86400,
        ),
        (
            "Wallet Discovery",
            "WDisc",
            "wallets",
            "discovered_at",
            86400,
        ),
        (
            "Trade Ingestion",
            "Trades",
            "trades_raw",
            "ingested_at",
            3600,
        ),
        (
            "Activity Ingestion",
            "Activity",
            "activity_raw",
            "ingested_at",
            21600,
        ),
        (
            "Position Snapshot",
            "Pos",
            "positions_snapshots",
            "snapshot_at",
            86400,
        ),
        (
            "Holder Snapshot",
            "Hold",
            "holders_snapshots",
            "snapshot_at",
            86400,
        ),
        ("Paper Tick", "Paper", "paper_trades", "created_at", 3600),
        (
            "Wallet Scoring",
            "WScore",
            "wallet_scores_daily",
            "score_date",
            86400,
        ),
    ];

    let mut jobs = Vec::new();
    for (name, short, table, ts_col, expected_interval_secs) in job_defs {
        let last_run: Option<String> = conn
            .query_row(&format!("SELECT MAX({}) FROM {}", ts_col, table), [], |r| {
                r.get(0)
            })
            .unwrap_or(None);

        let color = match &last_run {
            None => "bg-gray-600".to_string(),
            Some(ts) => {
                // Parse timestamp and check age
                let age_secs = age_seconds_from_timestamp(ts);
                if age_secs < expected_interval_secs * 2 {
                    "bg-green-500".to_string()
                } else if age_secs < expected_interval_secs * 3 {
                    "bg-yellow-500".to_string()
                } else {
                    "bg-red-500".to_string()
                }
            }
        };

        jobs.push(JobHeartbeat {
            name: name.to_string(),
            short_name: short.to_string(),
            last_run,
            color,
        });
    }

    Ok(SystemStatus {
        db_size_mb,
        phase: phase.to_string(),
        jobs,
    })
}

/// Parse a SQLite datetime string and return age in seconds from now
fn age_seconds_from_timestamp(ts: &str) -> i64 {
    // SQLite returns either "YYYY-MM-DD" or "YYYY-MM-DD HH:MM:SS"
    use chrono::{NaiveDate, NaiveDateTime, Utc};
    let now = Utc::now().naive_utc();

    if let Ok(dt) = NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        (now - dt).num_seconds()
    } else if let Ok(d) = NaiveDate::parse_from_str(ts, "%Y-%m-%d") {
        let dt = d.and_hms_opt(0, 0, 0).unwrap();
        (now - dt).num_seconds()
    } else {
        i64::MAX // unknown format = treat as very old
    }
}

pub fn top_markets_today(conn: &Connection) -> Result<Vec<MarketRow>> {
    let mut stmt = conn.prepare(
        "SELECT ms.rank, m.title, ms.condition_id, ms.mscore,
                COALESCE(m.liquidity, 0), COALESCE(m.volume, 0),
                COALESCE(ms.density_score, 0), m.end_date
         FROM market_scores_daily ms
         JOIN markets m ON m.condition_id = ms.condition_id
         WHERE ms.score_date = date('now')
         ORDER BY ms.rank ASC
         LIMIT 20",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(MarketRow {
                rank: row.get(0)?,
                title: row.get(1)?,
                condition_id: row.get(2)?,
                mscore: row.get(3)?,
                liquidity: row.get(4)?,
                volume: row.get(5)?,
                density_score: row.get(6)?,
                end_date: row.get(7)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn wallet_overview(conn: &Connection) -> Result<WalletOverview> {
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM wallets", [], |r| r.get(0))?;
    let active: i64 = conn.query_row(
        "SELECT COUNT(*) FROM wallets WHERE is_active = 1",
        [],
        |r| r.get(0),
    )?;
    let from_holder: i64 = conn.query_row(
        "SELECT COUNT(*) FROM wallets WHERE discovered_from = 'HOLDER'",
        [],
        |r| r.get(0),
    )?;
    let from_trader: i64 = conn.query_row(
        "SELECT COUNT(*) FROM wallets WHERE discovered_from = 'TRADER_RECENT'",
        [],
        |r| r.get(0),
    )?;
    let from_leaderboard: i64 = conn.query_row(
        "SELECT COUNT(*) FROM wallets WHERE discovered_from = 'LEADERBOARD'",
        [],
        |r| r.get(0),
    )?;
    let discovered_today: i64 = conn.query_row(
        "SELECT COUNT(*) FROM wallets WHERE date(discovered_at) = date('now')",
        [],
        |r| r.get(0),
    )?;
    Ok(WalletOverview {
        total,
        active,
        from_holder,
        from_trader,
        from_leaderboard,
        discovered_today,
    })
}

pub fn recent_wallets(conn: &Connection, limit: usize) -> Result<Vec<WalletRow>> {
    let mut stmt = conn.prepare(
        "SELECT w.proxy_wallet, w.discovered_from,
                m.title, w.discovered_at, w.is_active,
                (SELECT COUNT(*) FROM trades_raw t WHERE t.proxy_wallet = w.proxy_wallet)
         FROM wallets w
         LEFT JOIN markets m ON m.condition_id = w.discovered_market
         ORDER BY w.discovered_at DESC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit as i64], |row| {
            let wallet: String = row.get(0)?;
            let wallet_short = shorten_wallet(&wallet);
            Ok(WalletRow {
                proxy_wallet: wallet,
                wallet_short,
                discovered_from: row.get(1)?,
                discovered_market_title: row.get(2)?,
                discovered_at: row.get(3)?,
                is_active: row.get::<_, i64>(4)? != 0,
                trade_count: row.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn tracking_health(conn: &Connection) -> Result<Vec<TrackingHealth>> {
    let data_types = vec![
        ("Trades", "trades_raw", "ingested_at"),
        ("Activity", "activity_raw", "ingested_at"),
        ("Positions", "positions_snapshots", "snapshot_at"),
        ("Holders", "holders_snapshots", "snapshot_at"),
    ];

    let mut result = Vec::new();
    for (label, table, ts_col) in data_types {
        let count_1h: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE {} > datetime('now', '-1 hour')",
                table, ts_col
            ),
            [],
            |r| r.get(0),
        )?;
        let count_24h: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE {} > datetime('now', '-1 day')",
                table, ts_col
            ),
            [],
            |r| r.get(0),
        )?;
        let last: Option<String> = conn
            .query_row(&format!("SELECT MAX({}) FROM {}", ts_col, table), [], |r| {
                r.get(0)
            })
            .unwrap_or(None);

        let status_color = match &last {
            None => "text-gray-600".to_string(),
            Some(ts) => {
                let age = age_seconds_from_timestamp(ts);
                if age < 7200 {
                    "text-green-400".to_string()
                } else if age < 86400 {
                    "text-yellow-400".to_string()
                } else {
                    "text-red-400".to_string()
                }
            }
        };

        result.push(TrackingHealth {
            data_type: label.to_string(),
            count_last_1h: count_1h,
            count_last_24h: count_24h,
            last_ingested: last,
            status_color,
        });
    }
    Ok(result)
}

pub fn stale_wallets(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT w.proxy_wallet FROM wallets w
         WHERE w.is_active = 1
         AND NOT EXISTS (
             SELECT 1 FROM trades_raw t
             WHERE t.proxy_wallet = w.proxy_wallet
             AND t.ingested_at > datetime('now', '-1 day')
         )
         LIMIT 20",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let w: String = row.get(0)?;
            Ok(shorten_wallet(&w))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn paper_summary(conn: &Connection, bankroll: f64) -> Result<PaperSummary> {
    let total_pnl: f64 = conn.query_row(
        "SELECT COALESCE(SUM(pnl), 0) FROM paper_trades WHERE status != 'open'",
        [],
        |r| r.get(0),
    )?;
    let open_positions: i64 = conn.query_row(
        "SELECT COUNT(*) FROM paper_trades WHERE status = 'open'",
        [],
        |r| r.get(0),
    )?;
    let settled_wins: i64 = conn.query_row(
        "SELECT COUNT(*) FROM paper_trades WHERE status = 'settled_win'",
        [],
        |r| r.get(0),
    )?;
    let settled_losses: i64 = conn.query_row(
        "SELECT COUNT(*) FROM paper_trades WHERE status = 'settled_loss'",
        [],
        |r| r.get(0),
    )?;
    let pnl_color = if total_pnl >= 0.0 {
        "text-green-400"
    } else {
        "text-red-400"
    };
    let sign = if total_pnl >= 0.0 { "+" } else { "" };
    let pnl_display = format!("{}${:.2}", sign, total_pnl);
    let bankroll_display = format!("${:.0}", bankroll);
    Ok(PaperSummary {
        total_pnl,
        pnl_display,
        open_positions,
        settled_wins,
        settled_losses,
        bankroll,
        bankroll_display,
        pnl_color: pnl_color.to_string(),
    })
}

pub fn recent_paper_trades(conn: &Connection, limit: usize) -> Result<Vec<PaperTradeRow>> {
    let mut stmt = conn.prepare(
        "SELECT pt.proxy_wallet, COALESCE(m.title, pt.condition_id),
                pt.side, pt.size_usdc, pt.entry_price, pt.status,
                pt.pnl, pt.created_at
         FROM paper_trades pt
         LEFT JOIN markets m ON m.condition_id = pt.condition_id
         ORDER BY pt.created_at DESC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit as i64], |row| {
            let wallet: String = row.get(0)?;
            let side: String = row.get(2)?;
            let size_usdc: f64 = row.get(3)?;
            let entry_price: f64 = row.get(4)?;
            let status: String = row.get(5)?;
            let pnl: Option<f64> = row.get(6)?;

            let side_color = if side == "BUY" {
                "bg-green-900/50 text-green-300"
            } else {
                "bg-red-900/50 text-red-300"
            }
            .to_string();

            let status_color = match status.as_str() {
                "open" => "bg-blue-900/50 text-blue-300",
                "settled_win" => "bg-green-900/50 text-green-300",
                "settled_loss" => "bg-red-900/50 text-red-300",
                _ => "bg-gray-800 text-gray-400",
            }
            .to_string();

            let pnl_display = match pnl {
                Some(p) => {
                    let sign = if p >= 0.0 { "+" } else { "" };
                    format!("{}${:.2}", sign, p)
                }
                None => "-".to_string(),
            };

            let pnl_color = match pnl {
                Some(p) if p >= 0.0 => "text-green-400".to_string(),
                Some(_) => "text-red-400".to_string(),
                None => "text-gray-600".to_string(),
            };

            Ok(PaperTradeRow {
                wallet_short: shorten_wallet(&wallet),
                market_title: row.get(1)?,
                side,
                side_color,
                size_display: format!("${:.2}", size_usdc),
                price_display: format!("{:.3}", entry_price),
                status,
                status_color,
                pnl,
                pnl_display,
                pnl_color,
                created_at: row.get(7)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn top_rankings(conn: &Connection, window_days: i64, limit: usize) -> Result<Vec<RankingRow>> {
    let mut stmt = conn.prepare(
        "SELECT ws.proxy_wallet, ws.wscore,
                COALESCE(ws.edge_score, 0), COALESCE(ws.consistency_score, 0),
                COALESCE(ws.recommended_follow_mode, 'mirror'),
                (SELECT COUNT(*) FROM trades_raw t WHERE t.proxy_wallet = ws.proxy_wallet),
                COALESCE((SELECT SUM(pnl) FROM paper_trades pt
                          WHERE pt.proxy_wallet = ws.proxy_wallet AND pt.status != 'open'), 0)
         FROM wallet_scores_daily ws
         WHERE ws.score_date = date('now') AND ws.window_days = ?1
         ORDER BY ws.wscore DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map([window_days, limit as i64], |row| {
            let wallet: String = row.get(0)?;
            let wscore: f64 = row.get(1)?;
            let edge_score: f64 = row.get(2)?;
            let consistency_score: f64 = row.get(3)?;
            let paper_pnl: f64 = row.get(6)?;

            let pnl_color = if paper_pnl >= 0.0 {
                "text-green-400"
            } else {
                "text-red-400"
            }
            .to_string();
            let sign = if paper_pnl >= 0.0 { "+" } else { "" };

            Ok(RankingRow {
                rank: 0, // filled below
                rank_display: String::new(),
                row_class: String::new(),
                proxy_wallet: wallet.clone(),
                wallet_short: shorten_wallet(&wallet),
                wscore,
                wscore_display: format!("{:.2}", wscore),
                wscore_pct: format!("{:.0}", wscore * 100.0),
                edge_score,
                edge_display: format!("{:.2}", edge_score),
                consistency_score,
                consistency_display: format!("{:.2}", consistency_score),
                follow_mode: row.get(4)?,
                trade_count: row.get(5)?,
                paper_pnl,
                pnl_display: format!("{}${:.2}", sign, paper_pnl),
                pnl_color,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    // Fill ranks and rank display
    let rows: Vec<RankingRow> = rows
        .into_iter()
        .enumerate()
        .map(|(i, mut r)| {
            let rank = (i + 1) as i64;
            r.rank = rank;
            r.rank_display = match rank {
                1 => "\u{1F3C6}".to_string(), // trophy
                2 => "\u{1F948}".to_string(), // silver medal
                3 => "\u{1F949}".to_string(), // bronze medal
                n => n.to_string(),
            };
            r.row_class = if rank <= 3 {
                "bg-gray-800/20".to_string()
            } else {
                String::new()
            };
            r
        })
        .collect();

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::db::Database;

    fn test_db() -> Connection {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        db.conn
    }

    #[test]
    fn test_funnel_counts_empty_db() {
        let conn = test_db();
        let counts = funnel_counts(&conn).unwrap();
        assert_eq!(counts.markets_fetched, 0);
        assert_eq!(counts.wallets_discovered, 0);
        assert_eq!(counts.paper_trades_total, 0);
    }

    #[test]
    fn test_funnel_counts_with_data() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO markets (condition_id, title) VALUES ('0xabc', 'Test Market')",
            [],
        )
        .unwrap();
        let counts = funnel_counts(&conn).unwrap();
        assert_eq!(counts.markets_fetched, 1);
    }

    #[test]
    fn test_system_status_empty_db() {
        let conn = test_db();
        let status = system_status(&conn, ":memory:").unwrap();
        assert_eq!(status.phase, "0: Foundation");
        assert_eq!(status.jobs.len(), 8);
    }

    #[test]
    fn test_system_status_phase_detection() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank)
             VALUES ('0xabc', date('now'), 0.8, 1)",
            [],
        )
        .unwrap();
        let status = system_status(&conn, ":memory:").unwrap();
        assert_eq!(status.phase, "1: Market Discovery");
    }

    #[test]
    fn test_top_markets_empty() {
        let conn = test_db();
        let markets = top_markets_today(&conn).unwrap();
        assert!(markets.is_empty());
    }

    #[test]
    fn test_top_markets_with_data() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO markets (condition_id, title) VALUES ('0xabc', 'BTC > 100k')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank)
             VALUES ('0xabc', date('now'), 0.85, 1)",
            [],
        )
        .unwrap();
        let markets = top_markets_today(&conn).unwrap();
        assert_eq!(markets.len(), 1);
        assert_eq!(markets[0].title, "BTC > 100k");
        assert_eq!(markets[0].rank, 1);
    }

    #[test]
    fn test_wallet_overview_counts_sources() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from) VALUES ('0x1', 'HOLDER')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from) VALUES ('0x2', 'TRADER_RECENT')",
            [],
        )
        .unwrap();
        let overview = wallet_overview(&conn).unwrap();
        assert_eq!(overview.total, 2);
        assert_eq!(overview.from_holder, 1);
        assert_eq!(overview.from_trader, 1);
        assert_eq!(overview.from_leaderboard, 0);
    }

    #[test]
    fn test_recent_wallets_with_trade_count() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from) VALUES ('0xabcdef12345678', 'HOLDER')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, size, price, timestamp, transaction_hash)
             VALUES ('0xabcdef12345678', '0xm1', 10.0, 0.5, 1700000000, '0xtx1')",
            [],
        )
        .unwrap();
        let wallets = recent_wallets(&conn, 10).unwrap();
        assert_eq!(wallets.len(), 1);
        assert_eq!(wallets[0].trade_count, 1);
        assert_eq!(wallets[0].wallet_short, "0xabcd..5678");
    }

    #[test]
    fn test_tracking_health_empty() {
        let conn = test_db();
        let health = tracking_health(&conn).unwrap();
        assert_eq!(health.len(), 4);
        assert_eq!(health[0].data_type, "Trades");
        assert_eq!(health[0].count_last_24h, 0);
    }

    #[test]
    fn test_paper_summary_calculates_pnl() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl)
             VALUES ('0x1', 'mirror', '0xm1', 'BUY', 100.0, 0.60, 'settled_win', 25.0)",
            [],
        )
        .unwrap();
        let summary = paper_summary(&conn, 10000.0).unwrap();
        assert_eq!(summary.total_pnl, 25.0);
        assert_eq!(summary.settled_wins, 1);
        assert_eq!(summary.settled_losses, 0);
        assert_eq!(summary.pnl_color, "text-green-400");
    }

    #[test]
    fn test_rankings_ordered_by_wscore() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, edge_score, consistency_score)
             VALUES ('0x1', date('now'), 30, 0.80, 0.9, 0.7)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, edge_score, consistency_score)
             VALUES ('0x2', date('now'), 30, 0.60, 0.5, 0.7)",
            [],
        )
        .unwrap();
        let rankings = top_rankings(&conn, 30, 10).unwrap();
        assert_eq!(rankings.len(), 2);
        assert_eq!(rankings[0].rank, 1);
        assert!(rankings[0].wscore > rankings[1].wscore);
    }

    #[test]
    fn test_age_seconds_datetime_format() {
        // A date far in the past should have large age
        let age = age_seconds_from_timestamp("2020-01-01 00:00:00");
        assert!(age > 86400);
    }

    #[test]
    fn test_age_seconds_date_only_format() {
        let age = age_seconds_from_timestamp("2020-01-01");
        assert!(age > 86400);
    }

    #[test]
    fn test_age_seconds_unknown_format() {
        let age = age_seconds_from_timestamp("garbage");
        assert_eq!(age, i64::MAX);
    }
}
