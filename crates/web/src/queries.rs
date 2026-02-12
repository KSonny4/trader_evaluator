/// SQL queries for the dashboard. All read-only.
use anyhow::Result;
use rusqlite::Connection;
use rusqlite::OptionalExtension;

use crate::models::*;

fn timed_db_op<T>(op: &'static str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let start = std::time::Instant::now();
    let res = f();
    let ms = start.elapsed().as_secs_f64() * 1000.0;

    match &res {
        Ok(_) => {
            metrics::histogram!(
                "evaluator_db_query_latency_ms",
                "op" => op,
                "status" => "ok"
            )
            .record(ms);
        }
        Err(_) => {
            metrics::histogram!(
                "evaluator_db_query_latency_ms",
                "op" => op,
                "status" => "err"
            )
            .record(ms);
            metrics::counter!("evaluator_db_query_errors_total", "op" => op).increment(1);
        }
    }

    res
}

pub fn funnel_counts(conn: &Connection) -> Result<FunnelCounts> {
    timed_db_op("web.funnel_counts", || {
        let markets_fetched: i64 =
            conn.query_row("SELECT COUNT(*) FROM markets", [], |r| r.get(0))?;
        let markets_scored: i64 = conn.query_row(
            "
            SELECT COUNT(DISTINCT COALESCE(m.event_slug, m.condition_id))
            FROM market_scores ms
            JOIN markets m ON m.condition_id = ms.condition_id
            WHERE ms.score_date = (SELECT MAX(score_date) FROM market_scores)
            ",
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
            "SELECT COUNT(DISTINCT proxy_wallet) FROM wallet_scores_daily WHERE score_date = (SELECT MAX(score_date) FROM wallet_scores_daily)",
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
    })
}

pub fn persona_funnel_counts(conn: &Connection) -> Result<PersonaFunnelCounts> {
    let wallets_discovered: i64 =
        conn.query_row("SELECT COUNT(*) FROM wallets", [], |r| r.get(0))?;

    // Stage 1 is evaluated for watchlist wallets (is_active=1). A wallet "passes" if it has no
    // recorded Stage 1 exclusion reason (STAGE1_*).
    let stage1_passed: i64 = conn.query_row(
        "
        SELECT COUNT(*)
        FROM wallets w
        WHERE w.is_active = 1
          AND NOT EXISTS (
            SELECT 1
            FROM wallet_exclusions e
            WHERE e.proxy_wallet = w.proxy_wallet
              AND e.reason LIKE 'STAGE1_%'
          )
        ",
        [],
        |r| r.get(0),
    )?;

    // Stage 2 produces either a followable persona (wallet_personas) or an exclusion reason
    // (wallet_exclusions with non-stage1 reason).
    let stage2_classified: i64 = conn.query_row(
        "
        SELECT COUNT(*)
        FROM wallets w
        WHERE w.is_active = 1
          AND NOT EXISTS (
            SELECT 1
            FROM wallet_exclusions e
            WHERE e.proxy_wallet = w.proxy_wallet
              AND e.reason LIKE 'STAGE1_%'
          )
          AND (
            EXISTS (
              SELECT 1
              FROM wallet_personas p
              WHERE p.proxy_wallet = w.proxy_wallet
            )
            OR EXISTS (
              SELECT 1
              FROM wallet_exclusions e2
              WHERE e2.proxy_wallet = w.proxy_wallet
                AND e2.reason NOT LIKE 'STAGE1_%'
            )
          )
        ",
        [],
        |r| r.get(0),
    )?;

    let paper_traded_wallets: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT proxy_wallet) FROM paper_trades",
        [],
        |r| r.get(0),
    )?;

    // Follow-worthy is a best-effort approximation based on available data:
    // Promotion rules in docs/EVALUATION_STRATEGY.md §3.3 use ROI + hit rate + drawdown, but
    // hit rate/drawdown aren't fully computed yet. For visibility in UI/Grafana, we use ROI-only
    // thresholds: >+5% (7d) and >+10% (30d), both for score_date=today.
    let follow_worthy_wallets: i64 = conn.query_row(
        "
        SELECT COUNT(DISTINCT ws7.proxy_wallet)
        FROM wallet_scores_daily ws7
        JOIN wallet_scores_daily ws30
          ON ws30.proxy_wallet = ws7.proxy_wallet
         AND ws30.score_date = ws7.score_date
         AND ws30.window_days = 30
        WHERE ws7.score_date = (SELECT MAX(score_date) FROM wallet_scores_daily)
          AND ws7.window_days = 7
          AND COALESCE(ws7.paper_roi_pct, 0) > 5.0
          AND COALESCE(ws30.paper_roi_pct, 0) > 10.0
        ",
        [],
        |r| r.get(0),
    )?;

    Ok(PersonaFunnelCounts {
        wallets_discovered,
        stage1_passed,
        stage2_classified,
        paper_traded_wallets,
        follow_worthy_wallets,
    })
}

/// Returns (events_selected, events_evaluated) for the Events section.
/// events_selected uses scoring_stats.top_events_selected (cap at config top_n_events) when present.
pub fn events_counts(conn: &Connection) -> Result<(i64, i64)> {
    timed_db_op("web.events_counts", || {
        let (events_selected, events_evaluated): (i64, i64) = conn.query_row(
            "
            SELECT COALESCE(ss.top_events_selected,
                (SELECT COUNT(DISTINCT COALESCE(m.event_slug, m.condition_id))
                 FROM market_scores ms
                 JOIN markets m ON m.condition_id = ms.condition_id
                 WHERE ms.score_date = (SELECT MAX(score_date) FROM market_scores))),
                   COALESCE(ss.total_events_evaluated,
                (SELECT COUNT(DISTINCT COALESCE(m.event_slug, m.condition_id))
                 FROM market_scores ms
                 JOIN markets m ON m.condition_id = ms.condition_id
                 WHERE ms.score_date = (SELECT MAX(score_date) FROM market_scores)))
            FROM (SELECT MAX(score_date) AS sd FROM market_scores) maxd
            LEFT JOIN scoring_stats ss ON ss.score_date = maxd.sd
            ",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((events_selected, events_evaluated))
    })
}

pub fn unified_funnel_counts(conn: &Connection) -> Result<UnifiedFunnelCounts> {
    timed_db_op("web.unified_funnel_counts", || {
        let (events_selected, events_evaluated) = events_counts(conn)?;
        let all_wallets: i64 = conn.query_row("SELECT COUNT(*) FROM wallets", [], |r| r.get(0))?;
        let suitable_personas: i64 =
            conn.query_row("SELECT COUNT(*) FROM wallet_personas", [], |r| r.get(0))?;
        // Evaluated = active, passed Stage 1, classified (persona or Stage 2 exclusion), and oldest trade >= 30 days ago.
        let personas_evaluated: i64 = conn.query_row(
            "
            SELECT COUNT(*)
            FROM wallets w
            WHERE w.is_active = 1
              AND NOT EXISTS (
                SELECT 1 FROM wallet_exclusions e
                WHERE e.proxy_wallet = w.proxy_wallet AND e.reason LIKE 'STAGE1_%'
              )
              AND (
                EXISTS (SELECT 1 FROM wallet_personas p WHERE p.proxy_wallet = w.proxy_wallet)
                OR EXISTS (SELECT 1 FROM wallet_exclusions e2
                           WHERE e2.proxy_wallet = w.proxy_wallet AND e2.reason NOT LIKE 'STAGE1_%')
              )
              AND (SELECT CAST((julianday('now') - julianday(datetime(MIN(tr.timestamp), 'unixepoch'))) AS INTEGER)
                   FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) >= 30
            ",
            [],
            |r| r.get(0),
        )?;
        let actively_paper_traded: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT proxy_wallet) FROM paper_trades",
            [],
            |r| r.get(0),
        )?;
        let worth_following: i64 = conn.query_row(
            "
            SELECT COUNT(DISTINCT ws7.proxy_wallet)
            FROM wallet_scores_daily ws7
            JOIN wallet_scores_daily ws30
              ON ws30.proxy_wallet = ws7.proxy_wallet
             AND ws30.score_date = ws7.score_date
             AND ws30.window_days = 30
            WHERE ws7.score_date = (SELECT MAX(score_date) FROM wallet_scores_daily)
              AND ws7.window_days = 7
              AND COALESCE(ws7.paper_roi_pct, 0) > 5.0
              AND COALESCE(ws30.paper_roi_pct, 0) > 10.0
            ",
            [],
            |r| r.get(0),
        )?;
        let personas_excluded: i64 = excluded_wallets_count(conn)?;
        Ok(UnifiedFunnelCounts {
            events_selected,
            events_evaluated,
            all_wallets,
            suitable_personas,
            personas_evaluated,
            personas_excluded,
            actively_paper_traded,
            worth_following,
        })
    })
}

/// Returns (suitable_count, evaluated_count) for the suitable personas section.
/// Evaluated counts only wallets whose oldest trade is at least 30 days ago (trade-based age, not scrape age).
pub fn suitable_personas_counts(conn: &Connection) -> Result<(i64, i64)> {
    let suitable: i64 = conn.query_row("SELECT COUNT(*) FROM wallet_personas", [], |r| r.get(0))?;
    let evaluated: i64 = conn.query_row(
        "
        SELECT COUNT(*)
        FROM wallets w
        WHERE w.is_active = 1
          AND NOT EXISTS (
            SELECT 1 FROM wallet_exclusions e
            WHERE e.proxy_wallet = w.proxy_wallet AND e.reason LIKE 'STAGE1_%'
          )
          AND (
            EXISTS (SELECT 1 FROM wallet_personas p WHERE p.proxy_wallet = w.proxy_wallet)
            OR EXISTS (SELECT 1 FROM wallet_exclusions e2
                       WHERE e2.proxy_wallet = w.proxy_wallet AND e2.reason NOT LIKE 'STAGE1_%')
          )
          AND (SELECT CAST((julianday('now') - julianday(datetime(MIN(tr.timestamp), 'unixepoch'))) AS INTEGER)
               FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) >= 30
        ",
        [],
        |r| r.get(0),
    )?;
    Ok((suitable, evaluated))
}

pub fn suitable_personas_wallets(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<SuitablePersonaRow>> {
    timed_db_op("web.suitable_personas_wallets", || {
        let mut stmt = conn.prepare(
            "
            SELECT p.proxy_wallet, p.persona, p.classified_at
            FROM wallet_personas p
            ORDER BY p.classified_at DESC
            LIMIT ?1
            ",
        )?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                let wallet: String = row.get(0)?;
                Ok(SuitablePersonaRow {
                    proxy_wallet: wallet.clone(),
                    wallet_short: shorten_wallet(&wallet),
                    persona: row.get(1)?,
                    classified_at: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

pub fn paper_traded_wallets_list(conn: &Connection, limit: usize) -> Result<Vec<WalletRow>> {
    timed_db_op("web.paper_traded_wallets_list", || {
        let mut stmt = conn.prepare(
            "
            SELECT w.proxy_wallet, w.discovered_from,
                    m.title, w.discovered_at, w.is_active,
                    (SELECT COUNT(*) FROM trades_raw t WHERE t.proxy_wallet = w.proxy_wallet)
            FROM wallets w
            JOIN (
                SELECT proxy_wallet, MAX(created_at) AS last_trade
                FROM paper_trades
                GROUP BY proxy_wallet
                ORDER BY last_trade DESC
                LIMIT ?1
            ) pt ON pt.proxy_wallet = w.proxy_wallet
            LEFT JOIN markets m ON m.condition_id = w.discovered_market
            ORDER BY pt.last_trade DESC
            ",
        )?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                let wallet: String = row.get(0)?;
                Ok(WalletRow {
                    proxy_wallet: wallet.clone(),
                    wallet_short: shorten_wallet(&wallet),
                    discovered_from: row.get(1)?,
                    discovered_market_title: row.get(2)?,
                    discovered_at: row.get(3)?,
                    is_active: row.get::<_, i64>(4)? != 0,
                    trade_count: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

pub fn follow_worthy_rankings(conn: &Connection, limit: Option<usize>) -> Result<Vec<RankingRow>> {
    let limit = limit.unwrap_or(500);
    timed_db_op("web.follow_worthy_rankings", || {
        let mut stmt = conn.prepare(
            "
            SELECT ws.proxy_wallet, ws.wscore,
                    COALESCE(ws.edge_score, 0), COALESCE(ws.consistency_score, 0),
                    COALESCE(ws.recommended_follow_mode, 'mirror'),
                    (SELECT COUNT(*) FROM trades_raw t WHERE t.proxy_wallet = ws.proxy_wallet),
                    COALESCE((SELECT SUM(pnl) FROM paper_trades pt
                              WHERE pt.proxy_wallet = ws.proxy_wallet AND pt.status != 'open'), 0)
            FROM wallet_scores_daily ws
            JOIN wallet_scores_daily ws30
              ON ws30.proxy_wallet = ws.proxy_wallet
             AND ws30.score_date = ws.score_date
             AND ws30.window_days = 30
            WHERE ws.score_date = (SELECT MAX(score_date) FROM wallet_scores_daily)
              AND ws.window_days = 7
              AND COALESCE(ws.paper_roi_pct, 0) > 5.0
              AND COALESCE(ws30.paper_roi_pct, 0) > 10.0
            ORDER BY ws.wscore DESC
            LIMIT ?1
            ",
        )?;
        let rows = stmt
            .query_map([limit as i64], |row| {
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
                    rank: 0,
                    rank_display: String::new(),
                    row_class: String::new(),
                    proxy_wallet: wallet.clone(),
                    wallet_short: shorten_wallet(&wallet),
                    wscore,
                    wscore_display: format!("{wscore:.2}"),
                    wscore_pct: format!("{:.0}", wscore * 100.0),
                    edge_score,
                    edge_display: format!("{edge_score:.2}"),
                    consistency_score,
                    consistency_display: format!("{consistency_score:.2}"),
                    follow_mode: row.get(4)?,
                    trade_count: row.get(5)?,
                    paper_pnl,
                    pnl_display: format!("{sign}${paper_pnl:.2}"),
                    pnl_color,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let rows: Vec<RankingRow> = rows
            .into_iter()
            .enumerate()
            .map(|(i, mut r)| {
                let rank = (i + 1) as i64;
                r.rank = rank;
                r.rank_display = match rank {
                    1 => "\u{1F3C6}".to_string(),
                    2 => "\u{1F948}".to_string(),
                    3 => "\u{1F949}".to_string(),
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
    })
}

pub fn system_status(conn: &Connection, db_path: &str) -> Result<SystemStatus> {
    timed_db_op("web.system_status", || {
        let db_size_mb = std::fs::metadata(db_path).map_or_else(
            |_| "?".to_string(),
            |m| format!("{:.1}", m.len() as f64 / 1_048_576.0),
        );

        // Determine phase from data presence
        let has_scores: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM market_scores", [], |r| r.get(0))
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
            "4: Paper Trading"
        } else if has_trades {
            "3: Wallet Health Monitor"
        } else if has_wallets {
            "2: Wallet Discovery"
        } else if has_scores {
            "1: Event Discovery"
        } else {
            "0: Foundation"
        };

        // Job heartbeats: check freshness of each data source
        let job_defs: Vec<(&str, &str, &str, &str, i64)> = vec![
            (
                "Event Scoring",
                "EScore",
                "market_scores",
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

        let events_display = events_counts(conn).map_or_else(
            |_| "0".to_string(),
            |(sel, ev)| {
                if ev > 0 && ev != sel {
                    format!("{sel} / {ev}")
                } else {
                    sel.to_string()
                }
            },
        );

        let mut jobs = Vec::new();
        for (name, short, table, ts_col, expected_interval_secs) in job_defs {
            let last_run: Option<String> = conn
                .query_row(&format!("SELECT MAX({ts_col}) FROM {table}"), [], |r| {
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
            events_display,
        })
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
    timed_db_op("web.top_markets_today", || {
        let mut stmt = conn.prepare(
            "SELECT ms.rank, m.title, ms.condition_id, ms.mscore,
                    COALESCE(m.liquidity, 0), COALESCE(m.volume, 0),
                    COALESCE(ms.density_score, 0), m.end_date, m.event_slug, m.slug
            FROM market_scores ms
            JOIN markets m ON m.condition_id = ms.condition_id
            WHERE ms.score_date = (SELECT MAX(score_date) FROM market_scores)
            ORDER BY ms.rank ASC
            LIMIT 20",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let event_slug: Option<String> = row.get(8)?;
                let slug: Option<String> = row.get(9)?;
                let polymarket_url = event_slug
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("https://polymarket.com/event/{s}"))
                    .or_else(|| {
                        slug.filter(|s| !s.is_empty())
                            .map(|s| format!("https://polymarket.com/market/{s}"))
                    });
                Ok(MarketRow {
                    rank: row.get(0)?,
                    title: row.get(1)?,
                    condition_id: row.get(2)?,
                    mscore: row.get(3)?,
                    liquidity: row.get(4)?,
                    volume: row.get(5)?,
                    density_score: row.get(6)?,
                    end_date: row.get(7)?,
                    polymarket_url,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

pub fn top_events(conn: &Connection, limit: usize) -> Result<Vec<EventRow>> {
    timed_db_op("web.top_events", || {
        let mut stmt = conn.prepare(
            "
            WITH scored AS (
                SELECT ms.condition_id, ms.mscore, m.title, m.event_slug, m.slug,
                       COALESCE(m.event_slug, ms.condition_id) AS event_key
                FROM market_scores ms
                JOIN markets m ON m.condition_id = ms.condition_id
                WHERE ms.score_date = (SELECT MAX(score_date) FROM market_scores)
            ),
            best AS (
                SELECT event_key, MAX(mscore) AS best_mscore, COUNT(*) AS market_count
                FROM scored
                GROUP BY event_key
            )
            SELECT b.event_key, b.best_mscore, b.market_count,
                   (SELECT s.title FROM scored s WHERE s.event_key = b.event_key ORDER BY s.mscore DESC LIMIT 1),
                   (SELECT s.event_slug FROM scored s WHERE s.event_key = b.event_key ORDER BY s.mscore DESC LIMIT 1),
                   (SELECT s.slug FROM scored s WHERE s.event_key = b.event_key ORDER BY s.mscore DESC LIMIT 1)
            FROM best b
            WHERE b.market_count >= 1
            ORDER BY b.best_mscore DESC
            LIMIT ?1
            ",
        )?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                let event_slug: Option<String> = row.get(4)?;
                let slug: Option<String> = row.get(5)?;
                let polymarket_url = event_slug
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("https://polymarket.com/event/{s}"))
                    .or_else(|| {
                        slug.filter(|s| !s.is_empty())
                            .map(|s| format!("https://polymarket.com/market/{s}"))
                    });
                Ok(EventRow {
                    rank: 0,
                    title: row.get(3)?,
                    event_key: row.get(0)?,
                    best_mscore: row.get(1)?,
                    market_count: row.get(2)?,
                    polymarket_url,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let rows = rows
            .into_iter()
            .enumerate()
            .map(|(i, mut r)| {
                r.rank = (i + 1) as i64;
                r
            })
            .collect();
        Ok(rows)
    })
}

#[allow(dead_code)] // Used by /excluded, journey; retained for potential future use
pub fn wallet_overview(conn: &Connection) -> Result<WalletOverview> {
    timed_db_op("web.wallet_overview", || {
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
    })
}

pub fn recent_wallets(conn: &Connection, limit: usize) -> Result<Vec<WalletRow>> {
    timed_db_op("web.recent_wallets", || {
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
    })
}

#[allow(dead_code)] // Retained for potential future tracking dashboard
pub fn tracking_health(conn: &Connection) -> Result<Vec<TrackingHealth>> {
    timed_db_op("web.tracking_health", || {
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
                    "SELECT COUNT(*) FROM {table} WHERE {ts_col} > datetime('now', '-1 hour')"
                ),
                [],
                |r| r.get(0),
            )?;
            let count_24h: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {ts_col} > datetime('now', '-1 day')"),
                [],
                |r| r.get(0),
            )?;
            let last: Option<String> = conn
                .query_row(&format!("SELECT MAX({ts_col}) FROM {table}"), [], |r| {
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
    })
}

#[allow(dead_code)] // Retained for potential future tracking dashboard
pub fn stale_wallets(conn: &Connection) -> Result<Vec<String>> {
    timed_db_op("web.stale_wallets", || {
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
    })
}

pub fn excluded_wallets_count(conn: &Connection) -> Result<i64> {
    timed_db_op("web.excluded_wallets_count", || {
        let n: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT proxy_wallet) FROM wallet_exclusions",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    })
}

pub fn excluded_wallets_latest(
    conn: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<ExcludedWalletRow>> {
    timed_db_op("web.excluded_wallets_latest", || {
        // NOTE: If multiple exclusion rows share the same `excluded_at` for a wallet, this query can
        // return multiple rows for that wallet (tie on MAX(excluded_at)). Current semantics: show all
        // "latest-timestamp" reasons. If we want strictly one row per wallet, add a deterministic
        // tiebreak (e.g. MAX(id) among rows at MAX(excluded_at)) and join on that.
        let mut stmt = conn.prepare(
            "
            SELECT e.proxy_wallet, e.reason, e.metric_value, e.threshold, e.excluded_at
            FROM wallet_exclusions e
            JOIN (
              SELECT proxy_wallet, MAX(excluded_at) AS max_excluded_at
              FROM wallet_exclusions
              GROUP BY proxy_wallet
            ) latest
              ON latest.proxy_wallet = e.proxy_wallet
             AND latest.max_excluded_at = e.excluded_at
            ORDER BY e.excluded_at DESC
            LIMIT ?1 OFFSET ?2
            ",
        )?;

        let rows = stmt
            .query_map([limit as i64, offset as i64], |row| {
                let wallet: String = row.get(0)?;
                let reason: String = row.get(1)?;
                let metric_value: Option<f64> = row.get(2)?;
                let threshold: Option<f64> = row.get(3)?;
                let excluded_at: String = row.get(4)?;

                let metric_value_display =
                    metric_value.map_or_else(|| "-".to_string(), |v| format!("{v:.2}"));
                let threshold_display =
                    threshold.map_or_else(|| "-".to_string(), |v| format!("{v:.2}"));

                Ok(ExcludedWalletRow {
                    proxy_wallet: wallet.clone(),
                    wallet_short: shorten_wallet(&wallet),
                    reason,
                    metric_value_display,
                    threshold_display,
                    excluded_at,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    })
}

pub fn wallet_journey(conn: &Connection, proxy_wallet: &str) -> Result<Option<WalletJourney>> {
    timed_db_op("web.wallet_journey", || {
        let discovered_at: Option<String> = conn
            .query_row(
                "SELECT discovered_at FROM wallets WHERE proxy_wallet = ?1",
                [proxy_wallet],
                |r| r.get(0),
            )
            .optional()?;

        let Some(discovered_at) = discovered_at else {
            return Ok(None);
        };

        let persona_row: Option<(String, f64, String)> = conn
            .query_row(
                "
                SELECT persona, confidence, classified_at
                FROM wallet_personas
                WHERE proxy_wallet = ?1
                ORDER BY classified_at DESC
                LIMIT 1
                ",
                [proxy_wallet],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        let exclusion_row: Option<(String, Option<f64>, Option<f64>, String)> = conn
            .query_row(
                "
                SELECT reason, metric_value, threshold, excluded_at
                FROM wallet_exclusions
                WHERE proxy_wallet = ?1
                ORDER BY excluded_at DESC
                LIMIT 1
                ",
                [proxy_wallet],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;

        let paper_pnl: f64 = conn.query_row(
            "SELECT COALESCE(SUM(pnl), 0) FROM paper_trades WHERE proxy_wallet = ?1 AND status != 'open'",
            [proxy_wallet],
            |r| r.get(0),
        )?;
        let paper_pnl_display = {
            let sign = if paper_pnl >= 0.0 { "+" } else { "" };
            format!("{sign}${paper_pnl:.2}")
        };

        let exposure_usdc: f64 = conn.query_row(
            "SELECT COALESCE(SUM(total_size_usdc), 0) FROM paper_positions WHERE proxy_wallet = ?1",
            [proxy_wallet],
            |r| r.get(0),
        )?;
        let exposure_display = format!("${exposure_usdc:.2}");

        let (copied, total): (i64, i64) = conn.query_row(
            "
            SELECT
              COALESCE(SUM(CASE WHEN outcome = 'COPIED' THEN 1 ELSE 0 END), 0),
              COUNT(*)
            FROM copy_fidelity_events
            WHERE proxy_wallet = ?1
            ",
            [proxy_wallet],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let copy_fidelity_display = if total > 0 {
            let pct = 100.0 * copied as f64 / total as f64;
            format!("{pct:.0}% ({copied}/{total})")
        } else {
            "N/A".to_string()
        };

        let avg_slip: Option<f64> = conn.query_row(
            "SELECT AVG(slippage_cents) FROM follower_slippage WHERE proxy_wallet = ?1",
            [proxy_wallet],
            |r| r.get::<_, Option<f64>>(0),
        )?;
        let follower_slippage_display =
            avg_slip.map_or_else(|| "N/A".to_string(), |v| format!("{v:.2} cents"));

        let first_paper_trade_at: Option<String> = conn.query_row(
            "SELECT MIN(created_at) FROM paper_trades WHERE proxy_wallet = ?1",
            [proxy_wallet],
            |r| r.get::<_, Option<String>>(0),
        )?;

        let mut events: Vec<JourneyEvent> = vec![JourneyEvent {
            at: discovered_at.clone(),
            label: "Discovered".to_string(),
            detail: "Wallet discovered".to_string(),
        }];

        if let Some((persona, confidence, classified_at)) = &persona_row {
            events.push(JourneyEvent {
                at: classified_at.clone(),
                label: "Stage 2 PASSED".to_string(),
                detail: format!("{persona} (confidence: {confidence:.2})"),
            });
        }

        if let Some((reason, metric_value, threshold, excluded_at)) = &exclusion_row {
            let detail = match (metric_value, threshold) {
                (Some(v), Some(t)) => format!("{reason} ({v:.2} vs {t:.2})"),
                _ => reason.clone(),
            };
            events.push(JourneyEvent {
                at: excluded_at.clone(),
                label: "Excluded".to_string(),
                detail,
            });
        }

        if let Some(at) = first_paper_trade_at {
            events.push(JourneyEvent {
                at,
                label: "Paper trading started".to_string(),
                detail: "First paper trade created".to_string(),
            });
        }

        events.sort_by(|a, b| a.at.cmp(&b.at));

        let (persona, confidence_display) =
            persona_row.map_or((None, None), |(p, c, _)| (Some(p), Some(format!("{c:.2}"))));
        let exclusion_reason = exclusion_row.map(|(r, _, _, _)| r);

        Ok(Some(WalletJourney {
            proxy_wallet: proxy_wallet.to_string(),
            wallet_short: shorten_wallet(proxy_wallet),
            discovered_at,
            persona,
            confidence_display,
            exclusion_reason,
            paper_pnl_display,
            exposure_display,
            copy_fidelity_display,
            follower_slippage_display,
            events,
        }))
    })
}

#[allow(dead_code)] // Retained for potential future paper dashboard
pub fn paper_summary(
    conn: &Connection,
    bankroll: f64,
    max_total_exposure_pct: f64,
    max_daily_loss_pct: f64,
    max_concurrent_positions: i64,
) -> Result<PaperSummary> {
    timed_db_op("web.paper_summary", || {
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
        let pnl_display = format!("{sign}${total_pnl:.2}");
        let bankroll_display = format!("${bankroll:.0}");

        let wallets_followed: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT proxy_wallet) FROM paper_trades",
            [],
            |r| r.get(0),
        )?;

        let exposure_usdc: f64 = conn.query_row(
            "SELECT COALESCE(SUM(total_size_usdc), 0) FROM paper_positions",
            [],
            |r| r.get(0),
        )?;
        let exposure_display = format!("${exposure_usdc:.2}");
        let exposure_pct = if bankroll > 0.0 {
            100.0 * exposure_usdc / bankroll
        } else {
            0.0
        };
        let exposure_pct_display = format!("{exposure_pct:.1}%");

        let (copied, total): (i64, i64) = conn.query_row(
            "
            SELECT
              COALESCE(SUM(CASE WHEN outcome = 'COPIED' THEN 1 ELSE 0 END), 0),
              COUNT(*)
            FROM copy_fidelity_events
            ",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let copy_fidelity_display = if total > 0 {
            let pct = 100.0 * copied as f64 / total as f64;
            format!("{pct:.0}% ({copied}/{total})")
        } else {
            "N/A".to_string()
        };

        let avg_slip: Option<f64> = conn.query_row(
            "SELECT AVG(slippage_cents) FROM follower_slippage",
            [],
            |r| r.get::<_, Option<f64>>(0),
        )?;
        let follower_slippage_display =
            avg_slip.map_or_else(|| "N/A".to_string(), |v| format!("{v:.2} cents"));

        let positions_live: i64 =
            conn.query_row("SELECT COUNT(*) FROM paper_positions", [], |r| r.get(0))?;
        let pnl_today: f64 = conn.query_row(
            "SELECT COALESCE(SUM(pnl), 0) FROM paper_trades WHERE status != 'open' AND date(created_at) = date('now')",
            [],
            |r| r.get(0),
        )?;
        let daily_loss_pct = if bankroll > 0.0 && pnl_today < 0.0 {
            100.0 * (-pnl_today) / bankroll
        } else {
            0.0
        };

        let (risk_status, risk_status_color) = if exposure_pct > max_total_exposure_pct {
            ("EXCEEDED".to_string(), "text-red-400".to_string())
        } else if exposure_pct > 0.9 * max_total_exposure_pct {
            ("APPROACHING".to_string(), "text-yellow-400".to_string())
        } else if positions_live > max_concurrent_positions {
            ("TOO MANY POSITIONS".to_string(), "text-red-400".to_string())
        } else if daily_loss_pct > max_daily_loss_pct {
            ("DAILY LOSS LIMIT".to_string(), "text-red-400".to_string())
        } else {
            ("ALL CLEAR".to_string(), "text-green-400".to_string())
        };

        Ok(PaperSummary {
            total_pnl,
            pnl_display,
            open_positions,
            settled_wins,
            settled_losses,
            bankroll,
            bankroll_display,
            pnl_color: pnl_color.to_string(),
            wallets_followed,
            exposure_usdc,
            exposure_display,
            exposure_pct_display,
            copy_fidelity_display,
            follower_slippage_display,
            risk_status,
            risk_status_color,
        })
    })
}

#[allow(dead_code)] // Retained for potential future paper dashboard
pub fn recent_paper_trades(conn: &Connection, limit: usize) -> Result<Vec<PaperTradeRow>> {
    timed_db_op("web.recent_paper_trades", || {
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
                        format!("{sign}${p:.2}")
                    }
                    None => "-".to_string(),
                };

                let pnl_color = match pnl {
                    Some(p) if p >= 0.0 => "text-green-400".to_string(),
                    Some(_) => "text-red-400".to_string(),
                    None => "text-gray-600".to_string(),
                };

                Ok(PaperTradeRow {
                    proxy_wallet: wallet.clone(),
                    wallet_short: shorten_wallet(&wallet),
                    market_title: row.get(1)?,
                    side,
                    side_color,
                    size_display: format!("${size_usdc:.2}"),
                    price_display: format!("{entry_price:.3}"),
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
    })
}

#[allow(dead_code)] // Replaced by follow_worthy_rankings for unified funnel
pub fn top_rankings(conn: &Connection, window_days: i64, limit: usize) -> Result<Vec<RankingRow>> {
    timed_db_op("web.top_rankings", || {
        let mut stmt = conn.prepare(
            "SELECT ws.proxy_wallet, ws.wscore,
                    COALESCE(ws.edge_score, 0), COALESCE(ws.consistency_score, 0),
                    COALESCE(ws.recommended_follow_mode, 'mirror'),
                    (SELECT COUNT(*) FROM trades_raw t WHERE t.proxy_wallet = ws.proxy_wallet),
                    COALESCE((SELECT SUM(pnl) FROM paper_trades pt
                              WHERE pt.proxy_wallet = ws.proxy_wallet AND pt.status != 'open'), 0)
            FROM wallet_scores_daily ws
            WHERE ws.score_date = (SELECT MAX(score_date) FROM wallet_scores_daily) AND ws.window_days = ?1
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
                    wscore_display: format!("{wscore:.2}"),
                    wscore_pct: format!("{:.0}", wscore * 100.0),
                    edge_score,
                    edge_display: format!("{edge_score:.2}"),
                    consistency_score,
                    consistency_display: format!("{consistency_score:.2}"),
                    follow_mode: row.get(4)?,
                    trade_count: row.get(5)?,
                    paper_pnl,
                    pnl_display: format!("{sign}${paper_pnl:.2}"),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::db::Database;
    use metrics_exporter_prometheus::PrometheusBuilder;

    fn test_db() -> Connection {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        db.conn
    }

    #[test]
    fn test_db_query_timing_metrics_emitted_for_funnel_counts() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            let conn = test_db();
            let _ = funnel_counts(&conn).unwrap();
        });

        let rendered = handle.render();
        assert!(
            rendered.contains("evaluator_db_query_latency_ms"),
            "expected evaluator_db_query_latency_ms in rendered metrics, got:\n{rendered}"
        );
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
    fn test_persona_funnel_counts_with_data() {
        let conn = test_db();

        // 3 discovered wallets, all on watchlist (is_active=1).
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xw1', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xw2', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xw3', 'HOLDER', 1)",
            [],
        )
        .unwrap();

        // w1 fails Stage 1.
        conn.execute(
            "INSERT INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold)
             VALUES ('0xw1', 'STAGE1_TOO_YOUNG', 5.0, 30.0)",
            [],
        )
        .unwrap();

        // w2 passes Stage 1 and is followable (persona).
        conn.execute(
            "INSERT INTO wallet_personas (proxy_wallet, persona, confidence)
             VALUES ('0xw2', 'Informed Specialist', 0.87)",
            [],
        )
        .unwrap();

        // w3 passes Stage 1 but is excluded in Stage 2.
        conn.execute(
            "INSERT INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold)
             VALUES ('0xw3', 'NOISE_TRADER', 60.0, 50.0)",
            [],
        )
        .unwrap();

        // Both w2 and w3 have paper trades.
        conn.execute(
            "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl)
             VALUES ('0xw2', 'mirror', '0xm1', 'BUY', 25.0, 0.60, 'settled_win', 5.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl)
             VALUES ('0xw3', 'mirror', '0xm2', 'BUY', 25.0, 0.55, 'settled_loss', -2.0)",
            [],
        )
        .unwrap();

        // Follow-worthy: best-effort ROI thresholds.
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, paper_roi_pct)
             VALUES ('0xw2', date('now'), 7, 0.80, 6.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, paper_roi_pct)
             VALUES ('0xw2', date('now'), 30, 0.85, 11.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, paper_roi_pct)
             VALUES ('0xw3', date('now'), 7, 0.10, 1.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, paper_roi_pct)
             VALUES ('0xw3', date('now'), 30, 0.10, 1.0)",
            [],
        )
        .unwrap();

        let counts = persona_funnel_counts(&conn).unwrap();
        assert_eq!(counts.wallets_discovered, 3);
        assert_eq!(counts.stage1_passed, 2);
        assert_eq!(counts.stage2_classified, 2);
        assert_eq!(counts.paper_traded_wallets, 2);
        assert_eq!(counts.follow_worthy_wallets, 1);
    }

    #[test]
    fn test_suitable_personas_counts_evaluated_requires_30d_trade_age() {
        use chrono::{Duration, Utc};
        let conn = test_db();
        // Two wallets, both active, both with persona (suitable = 2).
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xold', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xnew', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_personas (proxy_wallet, persona, confidence) VALUES ('0xold', 'Informed Specialist', 0.9)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_personas (proxy_wallet, persona, confidence) VALUES ('0xnew', 'Informed Specialist', 0.9)",
            [],
        )
        .unwrap();
        // 0xold: oldest trade 40 days ago -> counts as evaluated.
        let ts_40d = (Utc::now() - Duration::days(40)).timestamp();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash)
             VALUES ('0xold', '0xm', 'BUY', 10.0, 0.5, ?1, '0xtx_old')",
            rusqlite::params![ts_40d],
        )
        .unwrap();
        // 0xnew: oldest trade 5 days ago -> does not count as evaluated.
        let ts_5d = (Utc::now() - Duration::days(5)).timestamp();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash)
             VALUES ('0xnew', '0xm', 'BUY', 10.0, 0.5, ?1, '0xtx_new')",
            rusqlite::params![ts_5d],
        )
        .unwrap();
        let (suitable, evaluated) = suitable_personas_counts(&conn).unwrap();
        assert_eq!(suitable, 2, "both wallets have persona");
        assert_eq!(
            evaluated, 1,
            "only wallet with trade >= 30 days ago counts as evaluated"
        );
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
            "INSERT INTO market_scores (condition_id, score_date, mscore, rank)
             VALUES ('0xabc', date('now'), 0.8, 1)",
            [],
        )
        .unwrap();
        let status = system_status(&conn, ":memory:").unwrap();
        assert_eq!(status.phase, "1: Event Discovery");
    }

    #[test]
    fn test_system_status_phase_paper_trading() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO market_scores (condition_id, score_date, mscore, rank)
             VALUES ('0xabc', date('now'), 0.8, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from) VALUES ('0xw', 'HOLDER')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash)
             VALUES ('0xw', '0xm', 'BUY', 10.0, 0.5, 1700000000, '0xtx1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status)
             VALUES ('0xw', 'mirror', '0xm', 'BUY', 10.0, 0.5, 'open')",
            [],
        )
        .unwrap();
        let status = system_status(&conn, ":memory:").unwrap();
        assert_eq!(status.phase, "4: Paper Trading");
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
            "INSERT INTO market_scores (condition_id, score_date, mscore, rank)
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
    fn test_top_events_empty() {
        let conn = test_db();
        let events = top_events(&conn, 50).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_top_events_with_data() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO markets (condition_id, title, event_slug) VALUES ('0x1', 'BTC Yes', 'btc-150k')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO markets (condition_id, title, event_slug) VALUES ('0x2', 'BTC No', 'btc-150k')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO market_scores (condition_id, score_date, mscore, rank)
             VALUES ('0x1', date('now'), 0.9, 1), ('0x2', date('now'), 0.7, 2)",
            [],
        )
        .unwrap();
        let events = top_events(&conn, 50).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "BTC Yes");
        assert_eq!(events[0].best_mscore, 0.9);
        assert_eq!(events[0].market_count, 2);
        assert_eq!(
            events[0].polymarket_url.as_deref(),
            Some("https://polymarket.com/event/btc-150k")
        );
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
        conn.execute(
            "INSERT INTO paper_positions (proxy_wallet, strategy, condition_id, side, total_size_usdc, avg_entry_price)
             VALUES ('0x1', 'mirror', '0xm1', 'BUY', 42.0, 0.60)",
            [],
        )
        .unwrap();
        let summary = paper_summary(&conn, 1000.0, 15.0, 3.0, 20).unwrap();
        assert_eq!(summary.total_pnl, 25.0);
        assert_eq!(summary.settled_wins, 1);
        assert_eq!(summary.settled_losses, 0);
        assert_eq!(summary.pnl_color, "text-green-400");
        assert_eq!(summary.wallets_followed, 1);
        assert_eq!(summary.exposure_usdc, 42.0);
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
