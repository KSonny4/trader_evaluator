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

/// Last completed run stats from discovery_scheduler_state (written by evaluator).
pub fn last_run_stats(conn: &Connection) -> Result<LastRunStats> {
    timed_db_op("web.last_run_stats", || {
        let row = |key: &str| -> Result<(i64, Option<String>)> {
            let opt = conn
                .query_row(
                    "SELECT value_int, updated_at FROM discovery_scheduler_state WHERE key = ?1",
                    [key],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .optional()
                .map_err(anyhow::Error::from)?;
            Ok(opt.unwrap_or((0, None)))
        };
        let (trades_wallets, trades_run_at) = row("last_run_trades_wallets")?;
        let (trades_inserted, _) = row("last_run_trades_inserted")?;
        let (events_markets, events_run_at) = row("last_run_events_markets")?;
        Ok(LastRunStats {
            trades_wallets,
            trades_inserted,
            events_markets,
            trades_run_at,
            events_run_at,
        })
    })
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

pub fn all_job_statuses(conn: &Connection) -> Result<Vec<JobStatusRow>> {
    timed_db_op("web.all_job_statuses", || {
        let mut stmt = conn.prepare(
            "SELECT job_name, status, last_run_at, metadata, duration_ms, last_error, updated_at
             FROM job_status
             ORDER BY job_name",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(JobStatusRow {
                    job_name: row.get(0)?,
                    status: row.get(1)?,
                    last_run_at: row.get(2)?,
                    next_run_at: None,
                    last_error: row.get(5)?,
                    duration_ms: row.get(4)?,
                    metadata: row.get(3)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
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
        // Evaluated = active, passed Stage 1, classified, and oldest trade >= 45 days ago.
        // Uses shared helper to avoid duplicate CTE scans.
        let personas_evaluated = personas_evaluated_count(conn)?;
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

/// Helper: Count personas evaluated (>= 45 days wallet age).
/// Shared by unified_funnel_counts and suitable_personas_counts to avoid duplicate CTE scans.
fn personas_evaluated_count(conn: &Connection) -> Result<i64> {
    let count: i64 = conn.query_row(
        "
        WITH wallet_age_days AS (
          SELECT proxy_wallet,
                 CAST((julianday('now') - julianday(datetime(MIN(timestamp), 'unixepoch'))) AS INTEGER) AS age_days
          FROM trades_raw
          GROUP BY proxy_wallet
        )
        SELECT COUNT(*)
        FROM wallets w
        LEFT JOIN wallet_age_days wad ON wad.proxy_wallet = w.proxy_wallet
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
          AND COALESCE(wad.age_days, 0) >= 45
        ",
        [],
        |r| r.get(0),
    )?;
    Ok(count)
}

/// Returns (suitable_count, evaluated_count) for the suitable personas section.
/// Evaluated = wallets whose oldest trade is at least 45 days ago (matches stage1_min_wallet_age_days).
pub fn suitable_personas_counts(conn: &Connection) -> Result<(i64, i64)> {
    let suitable: i64 = conn.query_row("SELECT COUNT(*) FROM wallet_personas", [], |r| r.get(0))?;
    let evaluated = personas_evaluated_count(conn)?;
    Ok((suitable, evaluated))
}

/// Per-persona breakdown: count of wallets per persona type (latest classification only).
pub fn persona_breakdown_counts(conn: &Connection) -> Result<Vec<PersonaBreakdownRow>> {
    timed_db_op("web.persona_breakdown_counts", || {
        let mut stmt = conn.prepare(
            "
            SELECT p.persona, COUNT(*) as count
            FROM wallet_personas p
            INNER JOIN (
                SELECT proxy_wallet, MAX(classified_at) AS max_at
                FROM wallet_personas GROUP BY proxy_wallet
            ) latest ON latest.proxy_wallet = p.proxy_wallet AND latest.max_at = p.classified_at
            GROUP BY p.persona
            ORDER BY count DESC
            ",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(PersonaBreakdownRow {
                    persona: row.get(0)?,
                    count: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// Ingestion stats: active wallets and wallets with at least 1 trade.
pub fn ingestion_stats(conn: &Connection) -> Result<IngestionStats> {
    timed_db_op("web.ingestion_stats", || {
        let active_wallets: i64 = conn.query_row(
            "SELECT COUNT(*) FROM wallets WHERE is_active = 1",
            [],
            |r| r.get(0),
        )?;
        // Use EXISTS against the indexed proxy_wallet column instead of
        // COUNT(DISTINCT proxy_wallet) FROM trades_raw, which requires a full index scan
        // and times out on large tables (>5s on production).
        let wallets_with_trades: i64 = conn.query_row(
            "SELECT COUNT(*) FROM wallets w WHERE EXISTS (SELECT 1 FROM trades_raw t WHERE t.proxy_wallet = w.proxy_wallet LIMIT 1)",
            [],
            |r| r.get(0),
        )?;
        Ok(IngestionStats {
            active_wallets,
            wallets_with_trades,
        })
    })
}

pub fn suitable_personas_wallets(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<SuitablePersonaRow>> {
    timed_db_op("web.suitable_personas_wallets", || {
        // One row per wallet: latest classification only (wallet_personas can have multiple rows per wallet on reclassify).
        let mut stmt = conn.prepare(
            "
            SELECT p.proxy_wallet, p.persona, p.classified_at
            FROM wallet_personas p
            INNER JOIN (
                SELECT proxy_wallet, MAX(classified_at) AS max_at
                FROM wallet_personas
                GROUP BY proxy_wallet
            ) latest ON latest.proxy_wallet = p.proxy_wallet AND latest.max_at = p.classified_at
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
                    COALESCE(tc.trade_count, 0),
                    COALESCE(pnl.total_pnl, 0)
            FROM wallet_scores_daily ws
            JOIN wallet_scores_daily ws30
              ON ws30.proxy_wallet = ws.proxy_wallet
             AND ws30.score_date = ws.score_date
             AND ws30.window_days = 30
            LEFT JOIN (
              SELECT proxy_wallet, COUNT(*) as trade_count
              FROM trades_raw
              GROUP BY proxy_wallet
            ) tc ON tc.proxy_wallet = ws.proxy_wallet
            LEFT JOIN (
              SELECT proxy_wallet, SUM(pnl) as total_pnl
              FROM paper_trades
              WHERE status != 'open'
              GROUP BY proxy_wallet
            ) pnl ON pnl.proxy_wallet = ws.proxy_wallet
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

/// Format unix timestamp (seconds) for display.
fn format_unix_timestamp(secs: i64) -> String {
    use chrono::{TimeZone, Utc};
    match Utc.timestamp_opt(secs, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        _ => secs.to_string(),
    }
}

/// Format a number with comma thousands separator.
fn format_with_commas(n: f64) -> String {
    let s = format!("{n:.0}");
    let mut result = String::new();
    let (start, ch) = if s.starts_with('-') {
        (1, "-")
    } else {
        (0, "")
    };
    result.push_str(ch);
    let digits: Vec<char> = s.chars().skip(start).collect();
    let len = digits.len();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(*c);
    }
    result
}

/// Format price (0–1) as cents for display, e.g. 0.06 -> "6c", 0.67 -> "67c".
fn format_price_cents(price: f64) -> String {
    let cents = (price * 100.0).round() as i32;
    format!("{cents}c")
}

/// Build a Polymarket URL from event_slug (preferred) or market slug.
fn polymarket_url(event_slug: Option<&str>, slug: Option<&str>) -> Option<String> {
    if let Some(es) = event_slug {
        if !es.is_empty() {
            return Some(format!("https://polymarket.com/event/{es}"));
        }
    }
    if let Some(s) = slug {
        if !s.is_empty() {
            return Some(format!("https://polymarket.com/market/{s}"));
        }
    }
    None
}

/// Build a Polygonscan URL from a transaction hash.
fn polygonscan_url(tx_hash: Option<&str>) -> Option<String> {
    tx_hash
        .filter(|h| !h.is_empty())
        .map(|h| format!("https://polygonscan.com/tx/{h}"))
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

/// Total number of positions (condition_id + outcome groups) for a wallet.
pub fn wallet_positions_count(conn: &Connection, proxy_wallet: &str) -> Result<usize> {
    let n: i64 = conn.query_row(
        "
        SELECT COUNT(*) FROM (
            SELECT 1 FROM trades_raw WHERE proxy_wallet = ?1 GROUP BY condition_id, outcome
        )
        ",
        [proxy_wallet],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

/// Count of active positions (net_shares > 0.5) for a wallet.
fn wallet_active_positions_count(conn: &Connection, proxy_wallet: &str) -> Result<usize> {
    let n: i64 = conn.query_row(
        "
        SELECT COUNT(*) FROM (
            SELECT
              SUM(CASE WHEN side = 'BUY' THEN size ELSE 0 END)
                - SUM(CASE WHEN side = 'SELL' THEN size ELSE 0 END) AS net_shares
            FROM trades_raw WHERE proxy_wallet = ?1
            GROUP BY condition_id, outcome
            HAVING net_shares > 0.5
        )
        ",
        [proxy_wallet],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

/// Count of closed positions (net_shares <= 0.5) for a wallet.
fn wallet_closed_positions_count(conn: &Connection, proxy_wallet: &str) -> Result<usize> {
    let n: i64 = conn.query_row(
        "
        SELECT COUNT(*) FROM (
            SELECT
              SUM(CASE WHEN side = 'BUY' THEN size ELSE 0 END)
                - SUM(CASE WHEN side = 'SELL' THEN size ELSE 0 END) AS net_shares
            FROM trades_raw WHERE proxy_wallet = ?1
            GROUP BY condition_id, outcome
            HAVING net_shares <= 0.5
        )
        ",
        [proxy_wallet],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

/// Consolidated positions summary: both active and closed positions in a single query.
struct PositionsSummary {
    active_positions: Vec<WalletPositionRow>,
    active_count: usize,
    closed_positions: Vec<WalletPositionRow>,
    closed_count: usize,
}

/// Fetch both active and closed positions in a single query using CTE.
/// Replaces 4 separate queries (2 counts + 2 data queries) with 1.
fn wallet_positions_summary(
    conn: &Connection,
    proxy_wallet: &str,
    limit: u32,
) -> Result<PositionsSummary> {
    let limit = limit.min(100);
    let sql = "
        WITH position_base AS (
          SELECT
            tr.condition_id,
            m.title,
            tr.outcome,
            SUM(CASE WHEN tr.side = 'BUY' THEN tr.size ELSE 0 END)
              - SUM(CASE WHEN tr.side = 'SELL' THEN tr.size ELSE 0 END) AS net_shares,
            CASE WHEN SUM(CASE WHEN tr.side = 'BUY' THEN tr.size ELSE 0 END) > 0
              THEN SUM(CASE WHEN tr.side = 'BUY' THEN tr.size * tr.price ELSE 0 END)
                   / SUM(CASE WHEN tr.side = 'BUY' THEN tr.size ELSE 0 END)
              ELSE 0 END AS avg_entry_price,
            SUM(CASE WHEN tr.side = 'BUY' THEN tr.size * tr.price ELSE 0 END) AS total_bet,
            COUNT(*) AS trade_count,
            m.event_slug,
            m.slug,
            MAX(tr.timestamp) AS last_trade_at
          FROM trades_raw tr
          LEFT JOIN markets m ON m.condition_id = tr.condition_id
          WHERE tr.proxy_wallet = ?1
          GROUP BY tr.condition_id, tr.outcome
        )
        SELECT condition_id, title, outcome, net_shares, avg_entry_price, total_bet, trade_count,
               event_slug, slug,
               CASE WHEN net_shares > 0.5 THEN 1 ELSE 0 END AS is_active
        FROM position_base
        ORDER BY last_trade_at DESC
    ";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([proxy_wallet], |r| {
        Ok((
            r.get::<_, String>(0)?,         // condition_id
            r.get::<_, Option<String>>(1)?, // title
            r.get::<_, Option<String>>(2)?, // outcome
            r.get::<_, f64>(3)?,            // net_shares
            r.get::<_, f64>(4)?,            // avg_entry_price
            r.get::<_, f64>(5)?,            // total_bet
            r.get::<_, i64>(6)?,            // trade_count
            r.get::<_, Option<String>>(7)?, // event_slug
            r.get::<_, Option<String>>(8)?, // slug
            r.get::<_, i64>(9)?,            // is_active
        ))
    })?;

    let mut active_positions = Vec::new();
    let mut closed_positions = Vec::new();
    let mut active_count = 0;
    let mut closed_count = 0;

    for row in rows {
        let (
            condition_id,
            market_title,
            outcome,
            net_shares,
            avg_entry_price,
            total_bet,
            trade_count,
            event_slug,
            slug,
            is_active,
        ) = row?;

        let pm_url = polymarket_url(event_slug.as_deref(), slug.as_deref());
        let position = WalletPositionRow {
            condition_id,
            market_title,
            outcome,
            shares_display: format_with_commas(net_shares),
            avg_price_display: format_price_cents(avg_entry_price),
            total_bet_display: format!("${total_bet:.2}"),
            trade_count: trade_count as u32,
            polymarket_url: pm_url,
        };

        if is_active == 1 {
            active_count += 1;
            if active_positions.len() < limit as usize {
                active_positions.push(position);
            }
        } else {
            closed_count += 1;
            if closed_positions.len() < limit as usize {
                closed_positions.push(position);
            }
        }
    }

    Ok(PositionsSummary {
        active_count,
        active_positions,
        closed_count,
        closed_positions,
    })
}

/// Internal: query positions with a HAVING filter for active/closed split.
fn wallet_positions_filtered(
    conn: &Connection,
    proxy_wallet: &str,
    offset: u32,
    limit: u32,
    having_clause: &str,
) -> Result<Vec<WalletPositionRow>> {
    let limit = limit.min(100);
    let sql = format!(
        "
        SELECT
          tr.condition_id,
          m.title,
          tr.outcome,
          SUM(CASE WHEN tr.side = 'BUY' THEN tr.size ELSE 0 END)
            - SUM(CASE WHEN tr.side = 'SELL' THEN tr.size ELSE 0 END) AS net_shares,
          CASE WHEN SUM(CASE WHEN tr.side = 'BUY' THEN tr.size ELSE 0 END) > 0
            THEN SUM(CASE WHEN tr.side = 'BUY' THEN tr.size * tr.price ELSE 0 END)
                 / SUM(CASE WHEN tr.side = 'BUY' THEN tr.size ELSE 0 END)
            ELSE 0 END AS avg_entry_price,
          SUM(CASE WHEN tr.side = 'BUY' THEN tr.size * tr.price ELSE 0 END) AS total_bet,
          COUNT(*) AS trade_count,
          m.event_slug,
          m.slug
        FROM trades_raw tr
        LEFT JOIN markets m ON m.condition_id = tr.condition_id
        WHERE tr.proxy_wallet = ?1
        GROUP BY tr.condition_id, tr.outcome
        {having_clause}
        ORDER BY MAX(tr.timestamp) DESC
        LIMIT ?2 OFFSET ?3
        "
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![proxy_wallet, i64::from(limit), i64::from(offset)],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, f64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        },
    )?;
    let positions: Vec<WalletPositionRow> = rows
        .map(|row| {
            let (
                condition_id,
                market_title,
                outcome,
                net_shares,
                avg_entry_price,
                total_bet,
                trade_count,
                event_slug,
                slug,
            ) = row?;
            let pm_url = polymarket_url(event_slug.as_deref(), slug.as_deref());
            Ok(WalletPositionRow {
                condition_id,
                market_title,
                outcome,
                shares_display: format_with_commas(net_shares),
                avg_price_display: format_price_cents(avg_entry_price),
                total_bet_display: format!("${total_bet:.2}"),
                trade_count: trade_count as u32,
                polymarket_url: pm_url,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(positions)
}

/// Paginated active positions (net_shares > 0.5) for a wallet.
pub fn wallet_active_positions_page(
    conn: &Connection,
    proxy_wallet: &str,
    offset: u32,
    limit: u32,
) -> Result<(Vec<WalletPositionRow>, usize)> {
    timed_db_op("web.wallet_active_positions_page", || {
        let total = wallet_active_positions_count(conn, proxy_wallet)?;
        let positions = wallet_positions_filtered(
            conn,
            proxy_wallet,
            offset,
            limit,
            "HAVING net_shares > 0.5",
        )?;
        Ok((positions, total))
    })
}

/// Paginated closed positions (net_shares <= 0.5) for a wallet.
pub fn wallet_closed_positions_page(
    conn: &Connection,
    proxy_wallet: &str,
    offset: u32,
    limit: u32,
) -> Result<(Vec<WalletPositionRow>, usize)> {
    timed_db_op("web.wallet_closed_positions_page", || {
        let total = wallet_closed_positions_count(conn, proxy_wallet)?;
        let positions = wallet_positions_filtered(
            conn,
            proxy_wallet,
            offset,
            limit,
            "HAVING net_shares <= 0.5",
        )?;
        Ok((positions, total))
    })
}

/// Paginated positions for a wallet (grouped by condition_id, outcome). Returns (positions, total_count).
/// Backward-compatible: returns ALL positions (active + closed).
pub fn wallet_positions_page(
    conn: &Connection,
    proxy_wallet: &str,
    offset: u32,
    limit: u32,
) -> Result<(Vec<WalletPositionRow>, usize)> {
    timed_db_op("web.wallet_positions_page", || {
        let total = wallet_positions_count(conn, proxy_wallet)?;
        let positions = wallet_positions_filtered(conn, proxy_wallet, offset, limit, "")?;
        Ok((positions, total))
    })
}

/// Total number of activity rows for a wallet.
pub fn wallet_activity_count(conn: &Connection, proxy_wallet: &str) -> Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM activity_raw WHERE proxy_wallet = ?1",
        [proxy_wallet],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

/// Paginated activity feed for a wallet (from activity_raw). Returns (activities, total_count).
pub fn wallet_activity_page(
    conn: &Connection,
    proxy_wallet: &str,
    offset: u32,
    limit: u32,
) -> Result<(Vec<WalletActivityRow>, usize)> {
    timed_db_op("web.wallet_activity_page", || {
        let total = wallet_activity_count(conn, proxy_wallet)?;
        let limit = i64::from(limit.min(100));
        let offset = i64::from(offset);

        let mut stmt = conn.prepare(
            "
            SELECT
              a.activity_type,
              a.condition_id,
              m.title,
              a.outcome,
              a.size,
              a.usdc_size,
              a.timestamp,
              a.transaction_hash,
              m.event_slug,
              m.slug
            FROM activity_raw a
            LEFT JOIN markets m ON m.condition_id = a.condition_id
            WHERE a.proxy_wallet = ?1
            ORDER BY a.timestamp DESC
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let rows = stmt.query_map(rusqlite::params![proxy_wallet, limit, offset], |r| {
            Ok((
                r.get::<_, String>(0)?,         // activity_type
                r.get::<_, Option<String>>(1)?, // condition_id
                r.get::<_, Option<String>>(2)?, // market_title
                r.get::<_, Option<String>>(3)?, // outcome
                r.get::<_, Option<f64>>(4)?,    // size (shares)
                r.get::<_, Option<f64>>(5)?,    // usdc_size
                r.get::<_, i64>(6)?,            // timestamp
                r.get::<_, Option<String>>(7)?, // transaction_hash
                r.get::<_, Option<String>>(8)?, // event_slug
                r.get::<_, Option<String>>(9)?, // slug
            ))
        })?;
        let activities: Vec<WalletActivityRow> = rows
            .map(|row| {
                let (
                    activity_type,
                    condition_id,
                    market_title,
                    outcome,
                    size,
                    usdc_size,
                    timestamp_sec,
                    transaction_hash,
                    event_slug,
                    slug,
                ) = row?;
                let timestamp_display = format_unix_timestamp(timestamp_sec);
                let ps_url = polygonscan_url(transaction_hash.as_deref());
                let pm_url = polymarket_url(event_slug.as_deref(), slug.as_deref());
                Ok(WalletActivityRow {
                    activity_type,
                    condition_id,
                    market_title,
                    outcome,
                    shares_display: size.map_or_else(|| "-".to_string(), |s| format!("{s:.2}")),
                    usdc_amount_display: usdc_size
                        .map_or_else(|| "-".to_string(), |u| format!("${u:.2}")),
                    timestamp_display,
                    polygonscan_url: ps_url,
                    polymarket_url: pm_url,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((activities, total))
    })
}

/// Latest 30-day features snapshot for a wallet.
fn wallet_features_latest(
    conn: &Connection,
    proxy_wallet: &str,
) -> Result<Option<WalletFeaturesSnapshot>> {
    let row = conn
        .query_row(
            "
            SELECT feature_date, COALESCE(total_pnl, 0), COALESCE(win_count, 0), COALESCE(loss_count, 0),
                   COALESCE(max_drawdown_pct, 0), COALESCE(sharpe_ratio, 0),
                   COALESCE(trades_per_day, 0), COALESCE(trade_count, 0), COALESCE(unique_markets, 0),
                   COALESCE(profitable_markets, 0), COALESCE(concentration_ratio, 0),
                   COALESCE(avg_trade_size_usdc, 0), COALESCE(size_cv, 0),
                   COALESCE(buy_sell_balance, 0), COALESCE(burstiness_top_1h_ratio, 0),
                   COALESCE(top_domain, ''), COALESCE(top_domain_ratio, 0),
                   COALESCE(mid_fill_ratio, 0), COALESCE(extreme_price_ratio, 0),
                   COALESCE(active_positions, 0), COALESCE(avg_position_size, 0),
                   COALESCE(cashflow_pnl, 0), COALESCE(fifo_realized_pnl, 0),
                   COALESCE(unrealized_pnl, 0), COALESCE(open_positions_count, 0)
            FROM wallet_features_daily
            WHERE proxy_wallet = ?1 AND window_days = 30
            ORDER BY feature_date DESC
            LIMIT 1
            ",
            [proxy_wallet],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, f64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, i64>(9)?,
                    r.get::<_, f64>(10)?,
                    r.get::<_, f64>(11)?,
                    r.get::<_, f64>(12)?,
                    r.get::<_, f64>(13)?,
                    r.get::<_, f64>(14)?,
                    r.get::<_, String>(15)?,
                    r.get::<_, f64>(16)?,
                    r.get::<_, f64>(17)?,
                    r.get::<_, f64>(18)?,
                    r.get::<_, i64>(19)?,
                    r.get::<_, f64>(20)?,
                    r.get::<_, f64>(21)?,
                    r.get::<_, f64>(22)?,
                    r.get::<_, f64>(23)?,
                    r.get::<_, i64>(24)?,
                ))
            },
        )
        .optional()?;

    let Some((
        feature_date,
        total_pnl,
        win_count,
        loss_count,
        max_drawdown_pct,
        sharpe_ratio,
        trades_per_day,
        trade_count,
        unique_markets,
        profitable_markets,
        concentration_ratio,
        avg_trade_size_usdc,
        size_cv,
        buy_sell_balance,
        burstiness,
        top_domain,
        top_domain_ratio,
        mid_fill_ratio,
        extreme_price_ratio,
        active_positions,
        avg_position_size,
        cashflow_pnl,
        fifo_realized_pnl,
        unrealized_pnl,
        open_positions_count,
    )) = row
    else {
        return Ok(None);
    };

    let pnl_sign = if total_pnl >= 0.0 { "+" } else { "" };
    let pnl_color = if total_pnl >= 0.0 {
        "text-green-400"
    } else {
        "text-red-400"
    };
    let total_closed = win_count + loss_count;
    let hit_rate = if total_closed > 0 {
        100.0 * win_count as f64 / total_closed as f64
    } else {
        0.0
    };
    let invested = avg_position_size * trade_count as f64;
    let roi_pct = if invested > 0.0 {
        100.0 * total_pnl / invested
    } else {
        0.0
    };
    let roi_sign = if roi_pct >= 0.0 { "+" } else { "" };
    let roi_color = if roi_pct >= 0.0 {
        "text-green-400"
    } else {
        "text-red-400"
    };
    let top_domain_display = if top_domain.is_empty() {
        "N/A".to_string()
    } else {
        format!("{top_domain} ({:.0}%)", top_domain_ratio * 100.0)
    };

    Ok(Some(WalletFeaturesSnapshot {
        feature_date,
        total_pnl,
        pnl_display: format!("{pnl_sign}${total_pnl:.2}"),
        pnl_color: pnl_color.to_string(),
        win_count,
        loss_count,
        hit_rate_display: format!("{hit_rate:.0}% ({win_count}W / {loss_count}L)"),
        max_drawdown_pct,
        drawdown_display: format!("{max_drawdown_pct:.1}%"),
        sharpe_ratio,
        sharpe_display: format!("{sharpe_ratio:.2}"),
        roi_pct,
        roi_display: format!("{roi_sign}{roi_pct:.1}%"),
        roi_color: roi_color.to_string(),
        trades_per_day,
        trades_per_day_display: format!("{trades_per_day:.1}"),
        trade_count,
        unique_markets,
        profitable_markets,
        market_skill_display: format!("{profitable_markets} / {unique_markets} profitable"),
        concentration_display: format!("{:.0}%", concentration_ratio * 100.0),
        avg_size_display: format!("${avg_trade_size_usdc:.2}"),
        size_cv_display: format!("{size_cv:.2}"),
        buy_sell_balance_display: format!("{buy_sell_balance:.2}"),
        burstiness_display: format!("{:.0}%", burstiness * 100.0),
        top_domain_display,
        mid_fill_display: format!("{:.0}%", mid_fill_ratio * 100.0),
        extreme_price_display: format!("{:.0}%", extreme_price_ratio * 100.0),
        active_positions,
        cashflow_pnl,
        fifo_realized_pnl,
        unrealized_pnl,
        open_positions_count,
    }))
}

/// Latest 30-day WScore snapshot for a wallet.
fn wallet_score_latest(
    conn: &Connection,
    proxy_wallet: &str,
) -> Result<Option<WalletScoreSnapshot>> {
    let row = conn
        .query_row(
            "
            SELECT score_date, wscore,
                   COALESCE(edge_score, 0), COALESCE(consistency_score, 0),
                   COALESCE(market_skill_score, 0), COALESCE(timing_skill_score, 0),
                   COALESCE(behavior_quality_score, 0)
            FROM wallet_scores_daily
            WHERE proxy_wallet = ?1 AND window_days = 30
            ORDER BY score_date DESC
            LIMIT 1
            ",
            [proxy_wallet],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, f64>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, f64>(6)?,
                ))
            },
        )
        .optional()?;

    let Some((score_date, wscore, edge, consistency, market_skill, timing, behavior)) = row else {
        return Ok(None);
    };

    Ok(Some(WalletScoreSnapshot {
        score_date,
        wscore,
        wscore_display: format!("{wscore:.2}"),
        wscore_pct: format!("{:.0}", wscore * 100.0),
        edge_display: format!("{edge:.2}"),
        edge_pct: format!("{:.0}", edge * 100.0),
        consistency_display: format!("{consistency:.2}"),
        consistency_pct: format!("{:.0}", consistency * 100.0),
        market_skill_display: format!("{market_skill:.2}"),
        market_skill_pct: format!("{:.0}", market_skill * 100.0),
        timing_skill_display: format!("{timing:.2}"),
        timing_skill_pct: format!("{:.0}", timing * 100.0),
        behavior_quality_display: format!("{behavior:.2}"),
        behavior_quality_pct: format!("{:.0}", behavior * 100.0),
    }))
}

/// Last 30 score history rows (30-day window) for a wallet, newest first.
fn wallet_score_history(conn: &Connection, proxy_wallet: &str) -> Result<Vec<ScoreHistoryRow>> {
    let mut stmt = conn.prepare(
        "
        SELECT score_date, wscore, COALESCE(edge_score, 0), COALESCE(consistency_score, 0),
               COALESCE(paper_roi_pct, 0)
        FROM wallet_scores_daily
        WHERE proxy_wallet = ?1 AND window_days = 30
        ORDER BY score_date DESC
        LIMIT 30
        ",
    )?;
    let rows = stmt
        .query_map([proxy_wallet], |r| {
            let score_date: String = r.get(0)?;
            let wscore: f64 = r.get(1)?;
            let edge: f64 = r.get(2)?;
            let consistency: f64 = r.get(3)?;
            let roi: f64 = r.get(4)?;
            let roi_sign = if roi >= 0.0 { "+" } else { "" };
            let roi_color = if roi >= 0.0 {
                "text-green-400"
            } else {
                "text-red-400"
            };
            Ok(ScoreHistoryRow {
                score_date,
                wscore_display: format!("{wscore:.2}"),
                edge_display: format!("{edge:.2}"),
                consistency_display: format!("{consistency:.2}"),
                roi_display: format!("{roi_sign}{roi:.1}%"),
                roi_color: roi_color.to_string(),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Persona traits for a wallet, with badge colors.
fn wallet_traits(conn: &Connection, proxy_wallet: &str) -> Result<Vec<WalletTrait>> {
    let mut stmt = conn.prepare(
        "SELECT trait_key, trait_value FROM wallet_persona_traits WHERE proxy_wallet = ?1",
    )?;
    let rows = stmt
        .query_map([proxy_wallet], |r| {
            let key: String = r.get(0)?;
            let value: String = r.get(1)?;
            Ok((key, value))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows
        .into_iter()
        .map(|(key, value)| {
            let badge_color = match key.as_str() {
                "BONDER" => "bg-purple-900 text-purple-200",
                "WHALE" => "bg-blue-900 text-blue-200",
                "TOPIC_LANE" => "bg-green-900 text-green-200",
                _ => "bg-gray-700 text-gray-300",
            };
            let display = if key == "TOPIC_LANE" {
                format!("TOPIC: {value}")
            } else {
                key
            };
            WalletTrait {
                display,
                badge_color: badge_color.to_string(),
            }
        })
        .collect())
}

/// Rules engine events for a wallet, converted to JourneyEvent timeline entries.
fn wallet_rules_events_timeline(
    conn: &Connection,
    proxy_wallet: &str,
) -> Result<Vec<JourneyEvent>> {
    let mut stmt = conn.prepare(
        "
        SELECT phase, allow, reason, created_at
        FROM wallet_rules_events
        WHERE proxy_wallet = ?1
        ORDER BY created_at ASC
        ",
    )?;
    let rows = stmt
        .query_map([proxy_wallet], |r| {
            let phase: String = r.get(0)?;
            let allow: bool = r.get::<_, i64>(1)? != 0;
            let reason: String = r.get(2)?;
            let created_at: String = r.get(3)?;
            Ok((phase, allow, reason, created_at))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows
        .into_iter()
        .map(|(phase, allow, reason, created_at)| {
            let label = format!(
                "{} {}",
                phase.to_uppercase(),
                if allow { "PASSED" } else { "BLOCKED" }
            );
            JourneyEvent {
                at: created_at,
                label,
                detail: reason,
            }
        })
        .collect())
}

#[allow(clippy::too_many_lines)]
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

        let last_trades_ingestion_at: Option<String> = conn.query_row(
            "SELECT MAX(ingested_at) FROM trades_raw WHERE proxy_wallet = ?1",
            [proxy_wallet],
            |r| r.get(0),
        )?;

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

        let first_classified_at: Option<String> = conn.query_row(
            "SELECT MIN(classified_at) FROM wallet_personas WHERE proxy_wallet = ?1",
            [proxy_wallet],
            |r| r.get::<_, Option<String>>(0),
        )?;

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

        let pipeline_state: String = conn
            .query_row(
                "SELECT state FROM wallet_rules_state WHERE proxy_wallet = ?1",
                [proxy_wallet],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "CANDIDATE".to_string());

        // On-chain data: features, scores, traits, history
        let mut features = wallet_features_latest(conn, proxy_wallet)?;
        let score = wallet_score_latest(conn, proxy_wallet)?;

        // Use upstream ROI from wallet_scores_daily when available (more accurate than
        // the approximation in features, which double-counts capital for round-trips).
        if let (Some(ref mut f), Some(ref s)) = (&mut features, &score) {
            let upstream_roi: Option<f64> = conn
                .query_row(
                    "SELECT paper_roi_pct FROM wallet_scores_daily WHERE proxy_wallet = ?1 AND window_days = 30 AND score_date = ?2",
                    rusqlite::params![proxy_wallet, s.score_date],
                    |r| r.get::<_, Option<f64>>(0),
                )
                .optional()?
                .flatten();
            if let Some(roi) = upstream_roi {
                f.roi_pct = roi;
                let roi_sign = if roi >= 0.0 { "+" } else { "" };
                f.roi_display = format!("{roi_sign}{roi:.1}%");
                f.roi_color = if roi >= 0.0 {
                    "text-green-400".to_string()
                } else {
                    "text-red-400".to_string()
                };
            }
        }
        let score_history = wallet_score_history(conn, proxy_wallet)?;
        let traits = wallet_traits(conn, proxy_wallet)?;

        // Use on-chain PnL from features as fallback for paper_pnl_display
        let paper_pnl_display = features
            .as_ref()
            .map_or_else(|| "N/A".to_string(), |f| f.pnl_display.clone());
        let exposure_display = "N/A".to_string();

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

        let first_entered_paper_at: Option<String> = conn.query_row(
            "SELECT MIN(created_at) FROM wallet_rules_events WHERE proxy_wallet = ?1 AND phase = 'discovery' AND allow = 1",
            [proxy_wallet],
            |r| r.get::<_, Option<String>>(0),
        )?;

        let mut events: Vec<JourneyEvent> = vec![JourneyEvent {
            at: discovered_at.clone(),
            label: "Discovered".to_string(),
            detail: "Wallet discovered".to_string(),
        }];

        if let Some((persona, confidence, latest_classified_at)) = &persona_row {
            let at = first_classified_at
                .clone()
                .unwrap_or_else(|| latest_classified_at.clone());
            events.push(JourneyEvent {
                at,
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

        let had_first_trade = first_paper_trade_at.is_some();
        let paper_trading_started_at =
            first_paper_trade_at.or_else(|| first_entered_paper_at.clone());
        if let Some(at) = paper_trading_started_at {
            let detail = if had_first_trade {
                "First paper trade created".to_string()
            } else {
                "Eligible for paper trading (no mirrored trade yet)".to_string()
            };
            events.push(JourneyEvent {
                at,
                label: "Paper trading started".to_string(),
                detail,
            });
        }

        // Add rules engine events to the timeline
        let rules_events = wallet_rules_events_timeline(conn, proxy_wallet)?;
        events.extend(rules_events);

        events.sort_by(|a, b| a.at.cmp(&b.at));

        // Consolidate position queries: fetch both active and closed in single query
        let positions_summary = wallet_positions_summary(conn, proxy_wallet, 20)?;
        let active_positions = positions_summary.active_positions;
        let total_active_positions_count = positions_summary.active_count;
        let closed_positions = positions_summary.closed_positions;
        let total_closed_positions_count = positions_summary.closed_count;

        let (activities, total_activities_count) = wallet_activity_page(conn, proxy_wallet, 0, 20)?;

        let total_trades_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM trades_raw WHERE proxy_wallet = ?1",
            [proxy_wallet],
            |r| r.get::<_, i64>(0).map(|n| n as usize),
        )?;

        let trades: Vec<WalletTradeRow> = {
            let mut stmt = conn.prepare(
                "
                SELECT tr.id, tr.condition_id, m.title, tr.side, tr.size, tr.price, tr.timestamp, tr.outcome, tr.transaction_hash
                FROM trades_raw tr
                LEFT JOIN markets m ON m.condition_id = tr.condition_id
                WHERE tr.proxy_wallet = ?1
                ORDER BY tr.timestamp DESC
                LIMIT 20
                ",
            )?;
            let rows = stmt.query_map([proxy_wallet], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                ))
            })?;
            rows.map(|row| {
                let (
                    id,
                    condition_id,
                    market_title,
                    side,
                    size,
                    price,
                    timestamp_sec,
                    outcome,
                    tx_hash,
                ) = row?;
                let timestamp_display = format_unix_timestamp(timestamp_sec);
                let ps_url = polygonscan_url(tx_hash.as_deref());
                Ok(WalletTradeRow {
                    id,
                    condition_id,
                    market_title,
                    side,
                    size_display: format!("{size:.2}"),
                    price_display: format!("{price:.2}"),
                    timestamp_display,
                    outcome,
                    polygonscan_url: ps_url,
                })
            })
            .collect::<Result<Vec<_>>>()?
        };

        let (persona, confidence_display) =
            persona_row.map_or((None, None), |(p, c, _)| (Some(p), Some(format!("{c:.2}"))));
        let exclusion_reason = exclusion_row.map(|(r, _, _, _)| r);

        let wallet_short = shorten_wallet(proxy_wallet);
        Ok(Some(WalletJourney {
            proxy_wallet: proxy_wallet.to_string(),
            wallet_short: wallet_short.clone(),
            wallet_display_label: wallet_short,
            discovered_at,
            last_trades_ingestion_at,
            persona,
            confidence_display,
            exclusion_reason,
            pipeline_state,
            paper_pnl_display,
            exposure_display,
            copy_fidelity_display,
            follower_slippage_display,
            score,
            features,
            traits,
            score_history,
            events,
            active_positions,
            total_active_positions_count,
            closed_positions,
            total_closed_positions_count,
            activities,
            total_activities_count,
            trades,
            total_trades_count,
        }))
    })
}

/// Paginated trades for a wallet (for load-more on scorecard). Returns (trades, total_count).
pub fn wallet_trades_page(
    conn: &Connection,
    proxy_wallet: &str,
    offset: u32,
    limit: u32,
) -> Result<(Vec<WalletTradeRow>, u64)> {
    timed_db_op("web.wallet_trades_page", || {
        let total: u64 = conn.query_row(
            "SELECT COUNT(*) FROM trades_raw WHERE proxy_wallet = ?1",
            [proxy_wallet],
            |r| r.get(0),
        )?;

        let limit = i64::from(limit.min(100));
        let offset = i64::from(offset);

        let mut stmt = conn.prepare(
            "
            SELECT tr.id, tr.condition_id, m.title, tr.side, tr.size, tr.price, tr.timestamp, tr.outcome, tr.transaction_hash
            FROM trades_raw tr
            LEFT JOIN markets m ON m.condition_id = tr.condition_id
            WHERE tr.proxy_wallet = ?1
            ORDER BY tr.timestamp DESC
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let rows = stmt.query_map(rusqlite::params![proxy_wallet, limit, offset], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, f64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?;
        let trades: Vec<WalletTradeRow> = rows
            .map(|row| {
                let (
                    id,
                    condition_id,
                    market_title,
                    side,
                    size,
                    price,
                    timestamp_sec,
                    outcome,
                    tx_hash,
                ) = row?;
                let timestamp_display = format_unix_timestamp(timestamp_sec);
                let ps_url = polygonscan_url(tx_hash.as_deref());
                Ok(WalletTradeRow {
                    id,
                    condition_id,
                    market_title,
                    side,
                    size_display: format!("{size:.2}"),
                    price_display: format!("{price:.2}"),
                    timestamp_display,
                    outcome,
                    polygonscan_url: ps_url,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((trades, total))
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
        // 0xold: oldest trade 50 days ago -> counts as evaluated (threshold 45 days).
        let ts_50d = (Utc::now() - Duration::days(50)).timestamp();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash)
             VALUES ('0xold', '0xm', 'BUY', 10.0, 0.5, ?1, '0xtx_old')",
            rusqlite::params![ts_50d],
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
            "only wallet with oldest trade >= 45 days ago counts as evaluated"
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
    fn test_wallet_active_positions_only_positive_shares() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, outcome, side, size, price, timestamp, transaction_hash)
             VALUES ('0xw', '0xm', 'Yes', 'BUY', 10.0, 0.5, 100, '0xtx1')",
            [],
        )
        .unwrap();
        // Net shares = 10.0 (active)

        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, outcome, side, size, price, timestamp, transaction_hash)
             VALUES ('0xw', '0xm2', 'No', 'BUY', 10.0, 0.5, 100, '0xtx2')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, outcome, side, size, price, timestamp, transaction_hash)
             VALUES ('0xw', '0xm2', 'No', 'SELL', 10.0, 0.6, 101, '0xtx3')",
            [],
        )
        .unwrap();
        // Net shares = 0.0 (closed)

        let (active, count) = wallet_active_positions_page(&conn, "0xw", 0, 10).unwrap();
        assert_eq!(count, 1); // Only 0xm is active
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].condition_id, "0xm");

        let (closed, count_closed) = wallet_closed_positions_page(&conn, "0xw", 0, 10).unwrap();
        assert_eq!(count_closed, 1); // Only 0xm2 is closed
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].condition_id, "0xm2");
    }

    #[test]
    fn test_wallet_activity_from_activity_raw() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO activity_raw (proxy_wallet, activity_type, condition_id, size, usdc_size, timestamp, transaction_hash)
             VALUES ('0xw', 'Buy', '0xm', 10.0, 5.0, 100, '0xtx')",
            [],
        )
        .unwrap();

        let (activity, count) = wallet_activity_page(&conn, "0xw", 0, 10).unwrap();
        assert_eq!(count, 1);
        let a = &activity[0];
        assert_eq!(a.activity_type, "Buy");
        assert_eq!(a.shares_display, "10.00");
        assert_eq!(a.usdc_amount_display, "$5.00");
        assert_eq!(
            a.polygonscan_url.as_deref(),
            Some("https://polygonscan.com/tx/0xtx")
        );
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

    /// Helper: insert a wallet with features, scores, traits, and rules events for scorecard tests.
    fn insert_scored_wallet(conn: &Connection) {
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xscored', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash)
             VALUES ('0xscored', '0xm1', 'BUY', 50.0, 0.50, 1700000000, '0xtx_scored')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_features_daily (proxy_wallet, feature_date, window_days, trade_count, win_count, loss_count, total_pnl, avg_position_size, unique_markets, max_drawdown_pct, sharpe_ratio, trades_per_day, profitable_markets, concentration_ratio, avg_trade_size_usdc, size_cv, buy_sell_balance, mid_fill_ratio, extreme_price_ratio, burstiness_top_1h_ratio, active_positions)
             VALUES ('0xscored', '2026-02-13', 30, 100, 60, 40, 500.0, 50.0, 8, 15.0, 1.50, 3.3, 5, 0.45, 50.0, 0.60, 0.52, 0.65, 0.12, 0.15, 3)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, edge_score, consistency_score, market_skill_score, timing_skill_score, behavior_quality_score, paper_roi_pct)
             VALUES ('0xscored', '2026-02-13', 30, 0.72, 0.65, 0.80, 0.63, 0.55, 0.88, 12.3)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, edge_score, consistency_score, market_skill_score, timing_skill_score, behavior_quality_score, paper_roi_pct)
             VALUES ('0xscored', '2026-02-12', 30, 0.70, 0.60, 0.78, 0.61, 0.50, 0.85, 10.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_persona_traits (proxy_wallet, trait_key, trait_value) VALUES ('0xscored', 'BONDER', 'true')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_persona_traits (proxy_wallet, trait_key, trait_value) VALUES ('0xscored', 'TOPIC_LANE', 'crypto')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_rules_events (proxy_wallet, phase, allow, reason) VALUES ('0xscored', 'discovery', 1, 'All gates passed')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_personas (proxy_wallet, persona, confidence) VALUES ('0xscored', 'INFORMED_SPECIALIST', 0.87)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn test_wallet_journey_includes_features() {
        let conn = test_db();
        insert_scored_wallet(&conn);
        let journey = wallet_journey(&conn, "0xscored").unwrap().unwrap();
        let f = journey.features.expect("features should be populated");
        assert_eq!(f.trade_count, 100);
        assert_eq!(f.win_count, 60);
        assert_eq!(f.loss_count, 40);
        assert!(f.total_pnl > 0.0);
        assert_eq!(f.pnl_display, "+$500.00");
        assert_eq!(f.pnl_color, "text-green-400");
        assert_eq!(f.hit_rate_display, "60% (60W / 40L)");
        assert_eq!(f.drawdown_display, "15.0%");
        assert_eq!(f.sharpe_display, "1.50");
        assert_eq!(f.unique_markets, 8);
        assert_eq!(f.profitable_markets, 5);
        assert_eq!(f.active_positions, 3);
    }

    #[test]
    fn test_wallet_journey_includes_scores() {
        let conn = test_db();
        insert_scored_wallet(&conn);
        let journey = wallet_journey(&conn, "0xscored").unwrap().unwrap();
        let s = journey.score.expect("score should be populated");
        assert_eq!(s.wscore_display, "0.72");
        assert_eq!(s.wscore_pct, "72");
        assert_eq!(s.edge_display, "0.65");
        assert_eq!(s.consistency_display, "0.80");
        assert_eq!(s.market_skill_display, "0.63");
        assert_eq!(s.timing_skill_display, "0.55");
        assert_eq!(s.behavior_quality_display, "0.88");
    }

    #[test]
    fn test_wallet_journey_includes_traits() {
        let conn = test_db();
        insert_scored_wallet(&conn);
        let journey = wallet_journey(&conn, "0xscored").unwrap().unwrap();
        assert_eq!(journey.traits.len(), 2);
        let bonder = journey.traits.iter().find(|t| t.display == "BONDER");
        assert!(bonder.is_some(), "should have BONDER trait");
        assert!(
            bonder.unwrap().badge_color.contains("purple"),
            "BONDER should be purple"
        );
        let topic = journey
            .traits
            .iter()
            .find(|t| t.display.starts_with("TOPIC:"));
        assert!(topic.is_some(), "should have TOPIC_LANE trait");
        assert!(
            topic.unwrap().badge_color.contains("green"),
            "TOPIC_LANE should be green"
        );
    }

    #[test]
    fn test_wallet_journey_rules_events_in_timeline() {
        let conn = test_db();
        insert_scored_wallet(&conn);
        let journey = wallet_journey(&conn, "0xscored").unwrap().unwrap();
        let rules_event = journey
            .events
            .iter()
            .find(|e| e.label.contains("DISCOVERY PASSED"));
        assert!(
            rules_event.is_some(),
            "timeline should include rules engine events"
        );
    }

    #[test]
    fn test_wallet_journey_graceful_without_data() {
        let conn = test_db();
        // Insert wallet with no features, scores, traits
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xbare', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        let journey = wallet_journey(&conn, "0xbare").unwrap().unwrap();
        assert!(journey.features.is_none());
        assert!(journey.score.is_none());
        assert!(journey.traits.is_empty());
        assert!(journey.score_history.is_empty());
        assert_eq!(journey.paper_pnl_display, "N/A");
    }

    #[test]
    fn test_wallet_journey_score_history() {
        let conn = test_db();
        insert_scored_wallet(&conn);
        let journey = wallet_journey(&conn, "0xscored").unwrap().unwrap();
        assert_eq!(journey.score_history.len(), 2);
        // Newest first
        assert_eq!(journey.score_history[0].score_date, "2026-02-13");
        assert_eq!(journey.score_history[1].score_date, "2026-02-12");
        assert_eq!(journey.score_history[0].wscore_display, "0.72");
    }

    /// Characterization test for position queries before consolidation optimization.
    /// Tests active/closed position split and counts.
    #[test]
    fn test_wallet_positions_active_and_closed_split() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xpos', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO markets (condition_id, title, event_slug, slug) VALUES ('0xm1', 'Market 1', 'event1', 'market1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO markets (condition_id, title, event_slug, slug) VALUES ('0xm2', 'Market 2', 'event2', 'market2')",
            [],
        )
        .unwrap();

        // Active position: BUY 100, SELL 40 = net 60 shares (> 0.5)
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
             VALUES ('0xpos', '0xm1', 'BUY', 100.0, 0.50, 1000000000, 'Yes')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
             VALUES ('0xpos', '0xm1', 'SELL', 40.0, 0.60, 1000000100, 'Yes')",
            [],
        )
        .unwrap();

        // Closed position: BUY 50, SELL 50 = net 0 shares (<= 0.5)
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
             VALUES ('0xpos', '0xm2', 'BUY', 50.0, 0.45, 1000000200, 'No')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
             VALUES ('0xpos', '0xm2', 'SELL', 50.0, 0.55, 1000000300, 'No')",
            [],
        )
        .unwrap();

        let (active, active_count) = wallet_active_positions_page(&conn, "0xpos", 0, 20).unwrap();
        let (closed, closed_count) = wallet_closed_positions_page(&conn, "0xpos", 0, 20).unwrap();

        assert_eq!(active_count, 1, "should have 1 active position");
        assert_eq!(closed_count, 1, "should have 1 closed position");
        assert_eq!(active.len(), 1);
        assert_eq!(closed.len(), 1);
        assert_eq!(active[0].condition_id, "0xm1");
        assert_eq!(closed[0].condition_id, "0xm2");
    }

    /// Characterization test for follow_worthy_rankings before N+1 fix.
    /// Tests that trade counts and PnL are correctly retrieved.
    #[test]
    fn test_follow_worthy_rankings_with_trade_counts_and_pnl() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xrank1', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xrank2', 'HOLDER', 1)",
            [],
        )
        .unwrap();

        // rank1: 3 trades, 10.0 PnL
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
             VALUES ('0xrank1', '0xm1', 'BUY', 10.0, 0.50, 1000000000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
             VALUES ('0xrank1', '0xm1', 'SELL', 10.0, 0.60, 1000000100)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
             VALUES ('0xrank1', '0xm2', 'BUY', 5.0, 0.40, 1000000200)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl)
             VALUES ('0xrank1', 'mirror', '0xm1', 'BUY', 25.0, 0.60, 'settled_win', 8.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl)
             VALUES ('0xrank1', 'mirror', '0xm2', 'BUY', 25.0, 0.55, 'settled_win', 2.0)",
            [],
        )
        .unwrap();

        // rank2: 1 trade, -5.0 PnL
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
             VALUES ('0xrank2', '0xm3', 'BUY', 20.0, 0.50, 1000000000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl)
             VALUES ('0xrank2', 'mirror', '0xm3', 'BUY', 25.0, 0.60, 'settled_loss', -5.0)",
            [],
        )
        .unwrap();

        // Add wallet scores to make them eligible for rankings
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, paper_roi_pct)
             VALUES ('0xrank1', date('now'), 7, 0.80, 6.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, paper_roi_pct)
             VALUES ('0xrank1', date('now'), 30, 0.85, 11.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, paper_roi_pct)
             VALUES ('0xrank2', date('now'), 7, 0.70, 5.5)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, paper_roi_pct)
             VALUES ('0xrank2', date('now'), 30, 0.75, 10.5)",
            [],
        )
        .unwrap();

        let rankings = follow_worthy_rankings(&conn, Some(10)).unwrap();
        assert_eq!(rankings.len(), 2);

        // rank1 has higher wscore, should be first
        assert_eq!(rankings[0].proxy_wallet, "0xrank1");
        assert_eq!(rankings[0].trade_count, 3);
        assert_eq!(rankings[0].pnl_display, "+$10.00");

        assert_eq!(rankings[1].proxy_wallet, "0xrank2");
        assert_eq!(rankings[1].trade_count, 1);
        assert_eq!(rankings[1].pnl_display, "$-5.00");
    }

    /// Direct test for wallet_positions_summary consolidated query.
    /// Verifies new function matches behavior of old separate queries.
    #[test]
    fn test_wallet_positions_summary_direct() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xtest', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO markets (condition_id, title, event_slug, slug) VALUES ('0xm1', 'Market 1', 'event1', 'market1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO markets (condition_id, title, event_slug, slug) VALUES ('0xm2', 'Market 2', 'event2', 'market2')",
            [],
        )
        .unwrap();

        // Active: 60 net shares
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
             VALUES ('0xtest', '0xm1', 'BUY', 100.0, 0.50, 1000000000, 'Yes')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
             VALUES ('0xtest', '0xm1', 'SELL', 40.0, 0.60, 1000000100, 'Yes')",
            [],
        )
        .unwrap();

        // Closed: 0 net shares
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
             VALUES ('0xtest', '0xm2', 'BUY', 50.0, 0.45, 1000000200, 'No')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
             VALUES ('0xtest', '0xm2', 'SELL', 50.0, 0.55, 1000000300, 'No')",
            [],
        )
        .unwrap();

        // Test new consolidated function
        let summary = wallet_positions_summary(&conn, "0xtest", 20).unwrap();

        assert_eq!(summary.active_count, 1);
        assert_eq!(summary.closed_count, 1);
        assert_eq!(summary.active_positions.len(), 1);
        assert_eq!(summary.closed_positions.len(), 1);
        assert_eq!(summary.active_positions[0].condition_id, "0xm1");
        assert_eq!(summary.closed_positions[0].condition_id, "0xm2");
    }

    /// Test wallet_positions_summary respects limit with many positions.
    #[test]
    fn test_wallet_positions_summary_respects_limit() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xlimit', 'HOLDER', 1)",
            [],
        )
        .unwrap();

        // Create 25 active positions and 15 closed positions
        for i in 0..25 {
            let cond_id = format!("0xactive{i}");
            conn.execute(
                &format!(
                    "INSERT INTO markets (condition_id, title) VALUES ('{cond_id}', 'Market {i}')"
                ),
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
                 VALUES ('0xlimit', ?1, 'BUY', 10.0, 0.50, 1000000000, 'Yes')",
                [&cond_id],
            )
            .unwrap();
        }

        for i in 0..15 {
            let cond_id = format!("0xclosed{i}");
            conn.execute(
                &format!(
                    "INSERT INTO markets (condition_id, title) VALUES ('{cond_id}', 'Market {i}')"
                ),
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
                 VALUES ('0xlimit', ?1, 'BUY', 10.0, 0.50, 1000000000, 'Yes')",
                [&cond_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, outcome)
                 VALUES ('0xlimit', ?1, 'SELL', 10.0, 0.60, 1000000100, 'Yes')",
                [&cond_id],
            )
            .unwrap();
        }

        let summary = wallet_positions_summary(&conn, "0xlimit", 20).unwrap();

        // Counts should include ALL positions
        assert_eq!(
            summary.active_count, 25,
            "count should include all 25 active positions"
        );
        assert_eq!(
            summary.closed_count, 15,
            "count should include all 15 closed positions"
        );

        // But data arrays should respect limit
        assert_eq!(
            summary.active_positions.len(),
            20,
            "should limit active data to 20"
        );
        assert_eq!(
            summary.closed_positions.len(),
            15,
            "closed has <20 so all included"
        );
    }

    /// Characterization test for unified_funnel_counts before CTE optimization.
    /// Tests that wallet age filtering works correctly.
    #[test]
    fn test_unified_funnel_counts_wallet_age_filtering() {
        use chrono::{Duration, Utc};
        let conn = test_db();

        // Old wallet (60 days old) - should be evaluated
        let old_ts = (Utc::now() - Duration::days(60)).timestamp();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xold', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
             VALUES ('0xold', '0xm1', 'BUY', 10.0, 0.50, ?1)",
            [old_ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_personas (proxy_wallet, persona, confidence)
             VALUES ('0xold', 'Informed Specialist', 0.87)",
            [],
        )
        .unwrap();

        // Young wallet (10 days old) - should not be evaluated
        let young_ts = (Utc::now() - Duration::days(10)).timestamp();
        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xyoung', 'HOLDER', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
             VALUES ('0xyoung', '0xm2', 'BUY', 10.0, 0.50, ?1)",
            [young_ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_personas (proxy_wallet, persona, confidence)
             VALUES ('0xyoung', 'Informed Specialist', 0.85)",
            [],
        )
        .unwrap();

        let counts = unified_funnel_counts(&conn).unwrap();
        assert_eq!(counts.all_wallets, 2);
        assert_eq!(counts.suitable_personas, 2);
        assert_eq!(
            counts.personas_evaluated, 1,
            "only old wallet should be evaluated (>= 45 days)"
        );
    }
}
