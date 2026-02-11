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
    })
}

pub fn followable_now_count(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "
        WITH latest_persona AS (
            SELECT proxy_wallet, MAX(classified_at) AS classified_at
            FROM wallet_personas
            GROUP BY proxy_wallet
        ),
        latest_exclusion AS (
            SELECT proxy_wallet, MAX(excluded_at) AS excluded_at
            FROM wallet_exclusions
            GROUP BY proxy_wallet
        )
        SELECT COUNT(*)
        FROM wallets w
        JOIN latest_persona lp ON lp.proxy_wallet = w.proxy_wallet
        LEFT JOIN latest_exclusion le ON le.proxy_wallet = w.proxy_wallet
        WHERE w.is_active = 1
          AND (le.excluded_at IS NULL OR le.excluded_at < lp.classified_at)
        ",
        [],
        |r| r.get(0),
    )
    .map_err(anyhow::Error::from)
}

pub fn unified_funnel_counts(conn: &Connection) -> Result<UnifiedFunnelCounts> {
    timed_db_op("web.unified_funnel_counts", || {
        let markets_fetched: i64 =
            conn.query_row("SELECT COUNT(*) FROM markets", [], |r| r.get(0))?;
        let markets_scored_today: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT condition_id) FROM market_scores_daily",
            [],
            |r| r.get(0),
        )?;

        let persona_counts = persona_funnel_counts(conn)?;
        let paper_active_followable = persona_counts.paper_traded_wallets;

        Ok(UnifiedFunnelCounts {
            markets_fetched,
            markets_scored_today,
            wallets_discovered: persona_counts.wallets_discovered,
            stage1_passed: persona_counts.stage1_passed,
            stage2_classified: persona_counts.stage2_classified,
            paper_active_followable,
            follow_worthy_wallets: persona_counts.follow_worthy_wallets,
            human_approval_wallets: 0,
            live_wallets: 0,
        })
    })
}

pub fn persona_funnel_counts(conn: &Connection) -> Result<PersonaFunnelCounts> {
    let wallets_discovered: i64 =
        conn.query_row("SELECT COUNT(*) FROM wallets", [], |r| r.get(0))?;

    // Ever/to-date semantics:
    // Stage 1 passed includes wallets that either:
    // - have no Stage 1 exclusion, or
    // - show any evidence of progressing beyond Stage 1.
    let stage1_passed: i64 = conn.query_row(
        "
        SELECT COUNT(*)
        FROM wallets w
        WHERE
          NOT EXISTS (
              SELECT 1
              FROM wallet_exclusions e
              WHERE e.proxy_wallet = w.proxy_wallet
                AND e.reason LIKE 'STAGE1_%'
          )
          OR EXISTS (
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
          OR EXISTS (
              SELECT 1
              FROM paper_trades pt
              WHERE pt.proxy_wallet = w.proxy_wallet
          )
          OR EXISTS (
              SELECT 1
              FROM wallet_scores_daily ws
              WHERE ws.proxy_wallet = w.proxy_wallet
          )
        ",
        [],
        |r| r.get(0),
    )?;

    // Stage 2 classified ever/to-date.
    let stage2_classified: i64 = conn.query_row(
        "
        SELECT COUNT(*)
        FROM wallets w
        WHERE
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
          OR EXISTS (
              SELECT 1
              FROM paper_trades pt
              WHERE pt.proxy_wallet = w.proxy_wallet
          )
          OR EXISTS (
              SELECT 1
              FROM wallet_scores_daily ws
              WHERE ws.proxy_wallet = w.proxy_wallet
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

    // Follow-worthy ever/to-date:
    // wallet has met ROI thresholds on any score_date and has paper-trading history.
    let follow_worthy_wallets: i64 = conn.query_row(
        "
        SELECT COUNT(DISTINCT ws7.proxy_wallet)
        FROM wallet_scores_daily ws7
        JOIN wallet_scores_daily ws30
          ON ws30.proxy_wallet = ws7.proxy_wallet
         AND ws30.score_date = ws7.score_date
         AND ws30.window_days = 30
        WHERE ws7.window_days = 7
          AND COALESCE(ws7.paper_roi_pct, 0) > 5.0
          AND COALESCE(ws30.paper_roi_pct, 0) > 10.0
          AND EXISTS (
              SELECT 1
              FROM wallets w
              WHERE w.proxy_wallet = ws7.proxy_wallet
          )
          AND EXISTS (
              SELECT 1
              FROM paper_trades pt
              WHERE pt.proxy_wallet = ws7.proxy_wallet
          )
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

pub fn system_status(conn: &Connection, db_path: &str) -> Result<SystemStatus> {
    timed_db_op("web.system_status", || {
        let db_size_mb = std::fs::metadata(db_path).map_or_else(
            |_| "?".to_string(),
            |m| format!("{:.1}", m.len() as f64 / 1_048_576.0),
        );

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
            "3: Wallet Health Monitor"
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
                    COALESCE(ms.density_score, 0), m.end_date
            FROM market_scores_daily ms
            JOIN markets m ON m.condition_id = ms.condition_id
            WHERE ms.score_date = date('now')
            ORDER BY ms.mscore DESC, ms.rank ASC, m.title ASC, ms.condition_id ASC
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
    })
}

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
        let discovered: Option<(String, String, Option<String>)> = conn
            .query_row(
                "
                SELECT w.discovered_at, w.discovered_from, m.title
                FROM wallets w
                LEFT JOIN markets m ON m.condition_id = w.discovered_market
                WHERE w.proxy_wallet = ?1
                ",
                [proxy_wallet],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        let Some((discovered_at, discovered_from, discovered_market_title)) = discovered else {
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
            discovered_from,
            discovered_market_title,
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

pub fn paper_summary(
    conn: &Connection,
    bankroll: f64,
    max_total_exposure_pct: f64,
    max_daily_loss_pct: f64,
    max_concurrent_positions: i64,
    mirror_use_proportional_sizing: bool,
    mirror_default_their_bankroll_usd: f64,
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

        let wallets_followed = followable_now_count(conn)?;

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
            sizing_mode_display: if mirror_use_proportional_sizing {
                "proportional".to_string()
            } else {
                "flat fallback".to_string()
            },
            sizing_estimator_bankroll_display: format!("${mirror_default_their_bankroll_usd:.0}"),
            risk_status,
            risk_status_color,
        })
    })
}

pub fn recent_paper_trades(conn: &Connection, limit: usize) -> Result<Vec<PaperTradeRow>> {
    timed_db_op("web.recent_paper_trades", || {
        let mut stmt = conn.prepare(
            "SELECT pt.proxy_wallet, COALESCE(m.title, pt.condition_id),
                    pt.side, pt.size_usdc, pt.entry_price, pt.status,
                    pt.pnl, pt.created_at,
                    (tr.size * tr.price) as source_notional_usd
            FROM paper_trades pt
            LEFT JOIN markets m ON m.condition_id = pt.condition_id
            LEFT JOIN trades_raw tr ON tr.id = pt.triggered_by_trade_id
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
                let source_notional_usd: Option<f64> = row.get(8)?;

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
                    source_notional_display: source_notional_usd
                        .map_or_else(|| "-".to_string(), |v| format!("${v:.2}")),
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
            WHERE ws.score_date = date('now') AND ws.window_days = ?1
            ORDER BY ws.wscore DESC, ws.proxy_wallet ASC
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
    fn test_followable_now_count_uses_latest_persona_vs_exclusion() {
        let conn = test_db();

        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES
             ('0xw1', 'HOLDER', 1),
             ('0xw2', 'HOLDER', 1),
             ('0xw3', 'HOLDER', 1)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at) VALUES
             ('0xw1', 'INFORMED_SPECIALIST', 0.8, '2026-02-10 00:00:00.100'),
             ('0xw2', 'CONSISTENT_GENERALIST', 0.8, '2026-02-10 00:00:00.100')",
            [],
        )
        .unwrap();

        // Newer exclusion should make wallet non-followable now.
        conn.execute(
            "INSERT INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold, excluded_at) VALUES
             ('0xw2', 'NOISE_TRADER', 1.0, 0.0, '2026-02-10 00:00:00.200')",
            [],
        )
        .unwrap();

        let followable = followable_now_count(&conn).unwrap();
        assert_eq!(followable, 1);
    }

    #[test]
    fn test_unified_funnel_counts_returns_all_stages() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO markets (condition_id, title) VALUES ('0xm1', 'M1'), ('0xm2', 'M2')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank) VALUES
             ('0xm1', date('now'), 0.9, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank) VALUES
             ('0xm2', date('now', '-1 day'), 0.8, 2)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES
             ('0xw1', 'HOLDER', 1),
             ('0xw2', 'HOLDER', 1),
             ('0xw3', 'HOLDER', 1)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold)
             VALUES ('0xw1', 'STAGE1_TOO_YOUNG', 5.0, 30.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
             VALUES ('0xw2', 'INFORMED_SPECIALIST', 0.87, '2026-02-10 00:00:00.100')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold)
             VALUES ('0xw3', 'NOISE_TRADER', 60.0, 50.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, paper_roi_pct)
             VALUES ('0xw2', date('now'), 7, 0.80, 6.0),
                    ('0xw2', date('now'), 30, 0.85, 11.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status)
             VALUES ('0xw2', 'mirror', '0xm1', 'BUY', 25.0, 0.5, 'open')",
            [],
        )
        .unwrap();

        let counts = unified_funnel_counts(&conn).unwrap();
        assert_eq!(counts.markets_fetched, 2);
        assert_eq!(counts.markets_scored_today, 2);
        assert_eq!(counts.wallets_discovered, 3);
        assert_eq!(counts.stage1_passed, 2);
        assert_eq!(counts.stage2_classified, 2);
        assert_eq!(counts.paper_active_followable, 1);
        assert_eq!(counts.follow_worthy_wallets, 1);
        assert_eq!(counts.human_approval_wallets, 0);
        assert_eq!(counts.live_wallets, 0);
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
    fn test_top_markets_ordered_by_mscore_then_rank() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO markets (condition_id, title) VALUES
             ('0xm1', 'A'),
             ('0xm2', 'B')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank) VALUES
             ('0xm1', date('now'), 0.70, 1),
             ('0xm2', date('now'), 0.95, 2)",
            [],
        )
        .unwrap();
        let markets = top_markets_today(&conn).unwrap();
        assert_eq!(markets[0].condition_id, "0xm2");
        assert_eq!(markets[1].condition_id, "0xm1");
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
        let summary = paper_summary(&conn, 1000.0, 15.0, 3.0, 20, true, 5000.0).unwrap();
        assert_eq!(summary.total_pnl, 25.0);
        assert_eq!(summary.settled_wins, 1);
        assert_eq!(summary.settled_losses, 0);
        assert_eq!(summary.pnl_color, "text-green-400");
        assert_eq!(summary.wallets_followed, 0);
        assert_eq!(summary.exposure_usdc, 42.0);
        assert_eq!(summary.sizing_mode_display, "proportional");
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
    fn test_rankings_tie_breaks_by_wallet() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, edge_score, consistency_score)
             VALUES ('0x2', date('now'), 30, 0.80, 0.9, 0.7)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, edge_score, consistency_score)
             VALUES ('0x1', date('now'), 30, 0.80, 0.5, 0.7)",
            [],
        )
        .unwrap();
        let rankings = top_rankings(&conn, 30, 10).unwrap();
        assert_eq!(rankings[0].proxy_wallet, "0x1");
        assert_eq!(rankings[1].proxy_wallet, "0x2");
    }

    #[test]
    fn test_recent_paper_trades_includes_source_notional_display() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO trades_raw (id, proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
             VALUES (1, '0xw', '0xm1', 'BUY', 200.0, 0.5, 1, '0xtx1', '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, triggered_by_trade_id)
             VALUES ('0xw', 'mirror', '0xm1', 'BUY', 20.0, 0.51, 'open', 1)",
            [],
        )
        .unwrap();

        let rows = recent_paper_trades(&conn, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source_notional_display, "$100.00");
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
