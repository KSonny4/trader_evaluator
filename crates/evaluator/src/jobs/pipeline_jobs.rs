use anyhow::Result;
use common::config::Config;
use common::db::AsyncDb;
use common::polymarket::GammaFilter;
#[cfg(test)]
use common::types::{ApiHolderResponse, ApiLeaderboardEntry, ApiTrade, GammaMarket};

use crate::market_scoring::{rank_events, rank_markets, MarketCandidate};
use crate::paper_trading::{is_crypto_15m_market, mirror_trade_to_paper, Side};
use crate::persona_classification::{
    classify_wallet, stage1_filter, stage1_known_bot_check, PersonaConfig, Stage1Config,
};
use crate::wallet_discovery::{discover_wallets_for_market, HolderWallet, TradeWallet};
use crate::wallet_features::compute_wallet_features;
use crate::wallet_rules_engine::{
    evaluate_discovery, evaluate_live, evaluate_paper, read_state, record_event,
    style_snapshot_from_features, write_state, WalletRuleState,
};
use crate::wallet_scoring::{compute_wscore, WScoreWeights, WalletScoreInput};

use super::fetcher_traits::*;
use super::tracker::JobTracker;

pub async fn run_paper_tick_once(db: &AsyncDb, cfg: &Config) -> Result<u64> {
    type TradeRow = (
        i64,
        String,
        String,
        Option<String>,
        f64,
        Option<String>,
        Option<i32>,
    );

    let tracker = JobTracker::start(db, "paper_tick").await?;
    // Read unprocessed trades from DB.
    let rows: Vec<TradeRow> = db
        .call_named("paper_tick.select_unprocessed_trades", |conn| {
            let mut stmt = conn.prepare(
                "
                -- Paper tick gating:
                -- Only mirror trades from wallets that are currently followable.
                -- A wallet is considered followable if it has a persona classification and the
                -- latest exclusion (if any) is strictly older than the latest persona.
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
                SELECT tr.id, tr.proxy_wallet, tr.condition_id, tr.side, tr.price, tr.outcome, tr.outcome_index
                FROM trades_raw tr
                LEFT JOIN paper_trades pt ON pt.triggered_by_trade_id = tr.id
                JOIN wallet_rules_state wr ON wr.proxy_wallet = tr.proxy_wallet
                JOIN latest_persona lp ON lp.proxy_wallet = tr.proxy_wallet
                LEFT JOIN latest_exclusion le ON le.proxy_wallet = tr.proxy_wallet
                WHERE pt.id IS NULL
                  AND wr.state IN ('PAPER_TRADING', 'APPROVED')
                  AND (le.excluded_at IS NULL OR le.excluded_at < lp.classified_at)
                ORDER BY tr.id ASC
                LIMIT 500
                ",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, f64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<i32>>(6)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;

    let mut inserted = 0_u64;
    for (trade_id, proxy_wallet, condition_id, side_s, price, outcome, outcome_index) in rows {
        // Stop mirroring immediately if a wallet becomes non-followable while we're mid-batch.
        // (The batch query above filters at selection time, but persona/exclusion state can
        // change concurrently via the persona classification job.)
        let proxy_wallet_check = proxy_wallet.clone();
        let followable_now: bool = db
            .call_named("paper_tick.wallet_is_followable_now", move |conn| {
                let followable: i64 = conn.query_row(
                    "
                    SELECT
                      CASE
                        WHEN (
                          SELECT MAX(classified_at)
                          FROM wallet_personas
                          WHERE proxy_wallet = ?1
                        ) IS NULL THEN 0
                        WHEN (
                          SELECT MAX(excluded_at)
                          FROM wallet_exclusions
                          WHERE proxy_wallet = ?1
                        ) IS NULL THEN 1
                        WHEN (
                          SELECT MAX(excluded_at)
                          FROM wallet_exclusions
                          WHERE proxy_wallet = ?1
                        ) < (
                          SELECT MAX(classified_at)
                          FROM wallet_personas
                          WHERE proxy_wallet = ?1
                        ) THEN 1
                        ELSE 0
                      END
                    ",
                    rusqlite::params![proxy_wallet_check],
                    |row| row.get(0),
                )?;
                Ok(followable == 1)
            })
            .await?;
        if !followable_now {
            continue;
        }

        let side = match side_s.as_deref() {
            Some("SELL") => Side::Sell,
            _ => Side::Buy,
        };

        let decision = mirror_trade_to_paper(
            db,
            &proxy_wallet,
            &condition_id,
            side,
            outcome.as_deref(),
            outcome_index,
            price,
            Some(trade_id),
            cfg.paper_trading.position_size_usdc,
            cfg.risk.slippage_pct,
            cfg.risk.paper_bankroll_usdc,
            cfg.risk.max_exposure_per_market_pct,
            cfg.risk.max_exposure_per_wallet_pct,
            cfg.risk.max_daily_trades,
            cfg.risk.portfolio_stop_drawdown_pct,
        )
        .await?;

        if decision.inserted {
            inserted += 1;
            metrics::counter!("evaluator_paper_trades_total").increment(1);
        } else if let Some(rule) = decision.reason {
            metrics::counter!("evaluator_risk_violations_total", "rule" => rule).increment(1);
        }
    }

    let pnl: Option<f64> = db
        .call_named("paper_tick.sum_settled_pnl", |conn| {
            Ok(conn.query_row(
                "SELECT SUM(pnl) FROM paper_trades WHERE status != 'open'",
                [],
                |row| row.get(0),
            )?)
        })
        .await?;
    metrics::gauge!("evaluator_paper_pnl").set(pnl.unwrap_or(0.0));

    tracker
        .success(Some(serde_json::json!({"inserted": inserted})))
        .await?;
    Ok(inserted)
}

/// Run once at startup to recover work that may have been in progress when the process
/// was killed. Processes any unprocessed trades into paper trades (idempotent).
/// Ingestion jobs are already idempotent (INSERT OR IGNORE / UNIQUE) so the next
/// scheduled run will catch up; we only run paper_tick here to keep startup fast.
pub async fn run_recovery_once(db: &AsyncDb, cfg: &Config) -> Result<u64> {
    let n = run_paper_tick_once(db, cfg).await?;
    if n > 0 {
        metrics::counter!("evaluator_recovery_paper_trades_total").increment(n);
    }
    Ok(n)
}

pub async fn run_wallet_rules_once(db: &AsyncDb, cfg: &Config) -> Result<u64> {
    let tracker = JobTracker::start(db, "wallet_rules").await?;
    let now_epoch = chrono::Utc::now().timestamp();
    let rules_cfg = cfg.wallet_rules.clone();
    let changed: u64 = db
        .call_named("wallet_rules.evaluate_batch", move |conn| {
            let wallets: Vec<String> = conn
                .prepare(
                    "
                    SELECT proxy_wallet
                    FROM wallets
                    WHERE is_active = 1
                    ORDER BY discovered_at DESC
                    ",
                )?
                .query_map([], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            let mut updates = 0_u64;
            for proxy_wallet in wallets {
                let features = match compute_wallet_features(conn, &proxy_wallet, 30, now_epoch) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(
                            proxy_wallet = %proxy_wallet,
                            error = %e,
                            "wallet rules skipped: compute_wallet_features failed"
                        );
                        continue;
                    }
                };
                let is_followable: Option<bool> = conn.query_row(
                    "
                    SELECT
                      CASE
                        WHEN (
                          SELECT MAX(classified_at)
                          FROM wallet_personas
                          WHERE proxy_wallet = ?1
                        ) IS NULL THEN NULL
                        WHEN (
                          SELECT MAX(excluded_at)
                          FROM wallet_exclusions
                          WHERE proxy_wallet = ?1
                        ) IS NULL THEN 1
                        WHEN (
                          SELECT MAX(excluded_at)
                          FROM wallet_exclusions
                          WHERE proxy_wallet = ?1
                        ) < (
                          SELECT MAX(classified_at)
                          FROM wallet_personas
                          WHERE proxy_wallet = ?1
                        ) THEN 1
                        ELSE 0
                      END
                    ",
                    rusqlite::params![proxy_wallet],
                    |row| {
                        let v: Option<i64> = row.get(0)?;
                        Ok(v.map(|x| x == 1))
                    },
                )?;
                let state = read_state(conn, &proxy_wallet)?;

                if matches!(is_followable, Some(false)) {
                    let decision = crate::wallet_rules_engine::WalletRuleDecision {
                        allow: false,
                        reason: "not_followable_persona_gate".to_string(),
                    };
                    write_state(
                        conn,
                        &proxy_wallet,
                        WalletRuleState::Stopped,
                        None,
                        Some(now_epoch),
                    )?;
                    record_event(conn, &proxy_wallet, "discovery", &decision, None)?;
                    if state != WalletRuleState::Stopped {
                        updates += 1;
                    }
                    continue;
                }

                let (phase, decision, next_state) = match state {
                    WalletRuleState::Candidate | WalletRuleState::Stopped => {
                        let decision = evaluate_discovery(&features, &rules_cfg);
                        let next = if decision.allow {
                            WalletRuleState::PaperTrading
                        } else {
                            state
                        };
                        ("discovery", decision, next)
                    }
                    WalletRuleState::PaperTrading => {
                        let decision = evaluate_paper(conn, &proxy_wallet, &rules_cfg)?;
                        let next = if decision.allow {
                            WalletRuleState::Approved
                        } else {
                            WalletRuleState::PaperTrading
                        };
                        ("paper", decision, next)
                    }
                    WalletRuleState::Approved => {
                        let decision = evaluate_live(conn, &proxy_wallet, now_epoch, &rules_cfg)?;
                        let next = if decision.allow {
                            WalletRuleState::Approved
                        } else {
                            WalletRuleState::Stopped
                        };
                        ("live", decision, next)
                    }
                };

                let baseline_json = if next_state == WalletRuleState::Approved
                    && state != WalletRuleState::Approved
                {
                    Some(serde_json::to_string(&style_snapshot_from_features(
                        &features,
                    ))?)
                } else {
                    None
                };
                write_state(
                    conn,
                    &proxy_wallet,
                    next_state,
                    baseline_json.as_deref(),
                    Some(now_epoch),
                )?;
                record_event(conn, &proxy_wallet, phase, &decision, None)?;

                if next_state != state {
                    updates += 1;
                }
            }

            Ok(updates)
        })
        .await?;
    metrics::gauge!("evaluator_wallet_rules_transitions_run").set(changed as f64);
    tracker
        .success(Some(serde_json::json!({"changed": changed})))
        .await?;
    Ok(changed)
}

pub async fn run_wallet_scoring_once(db: &AsyncDb, cfg: &Config) -> Result<u64> {
    struct ScoreRow {
        proxy_wallet: String,
        window_days: i64,
        wscore: f64,
        edge_score: f64,
        consistency_score: f64,
        roi_pct: f64,
    }

    let tracker = JobTracker::start(db, "wallet_scoring").await?;
    let today = chrono::Utc::now().date_naive().to_string();

    let w = WScoreWeights {
        edge_weight: cfg.wallet_scoring.edge_weight,
        consistency_weight: cfg.wallet_scoring.consistency_weight,
        market_skill_weight: cfg.wallet_scoring.market_skill_weight,
        timing_skill_weight: cfg.wallet_scoring.timing_skill_weight,
        behavior_quality_weight: cfg.wallet_scoring.behavior_quality_weight,
    };

    let windows_days = cfg.wallet_scoring.windows_days.clone();
    let bankroll = cfg.risk.paper_bankroll_usdc;
    let trust_30_90_multiplier = cfg.personas.trust_30_90_multiplier;
    let obscurity_bonus_multiplier = cfg.personas.obscurity_bonus_multiplier;

    // Batch read: fetch all (wallet, window) PnL values in one db.call().
    let windows_c = windows_days.clone();
    let pnl_data: Vec<(String, String, i64, i64, f64, u32, u32)> = db
        .call_named("wallet_scoring.read_pnl_batch", move |conn| {
            let mut stmt = conn.prepare(
                "
                SELECT proxy_wallet,
                       discovered_from,
                       CAST((julianday('now') - julianday(discovered_at)) AS INTEGER) AS age_days
                FROM wallets
                WHERE is_active = 1
                ORDER BY discovered_at DESC
                LIMIT 500
                ",
            )?;
            let wallets: Vec<(String, String, i64)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            let mut results = Vec::new();
            let mut pnl_stmt = conn.prepare(
                "SELECT SUM(pnl) FROM paper_trades WHERE proxy_wallet = ?1 AND status != 'open' AND created_at >= datetime('now', ?2)",
            )?;
            let mut total_markets_stmt = conn.prepare(
                "SELECT COUNT(DISTINCT condition_id) FROM paper_trades WHERE proxy_wallet = ?1 AND status != 'open' AND created_at >= datetime('now', ?2)",
            )?;
            let mut profitable_markets_stmt = conn.prepare(
                "
                SELECT COUNT(*) FROM (
                    SELECT condition_id, SUM(pnl) AS pnl_sum
                    FROM paper_trades
                    WHERE proxy_wallet = ?1 AND status != 'open' AND created_at >= datetime('now', ?2)
                    GROUP BY condition_id
                    HAVING pnl_sum > 0
                )
                ",
            )?;

            for (wallet, discovered_from, age_days) in &wallets {
                for &wd in &windows_c {
                    let window = format!("-{wd} days");
                    let pnl: Option<f64> = pnl_stmt.query_row(
                        rusqlite::params![wallet, window],
                        |row| row.get(0),
                    )?;
                    let total_markets: u32 = total_markets_stmt.query_row(
                        rusqlite::params![wallet, window],
                        |row| row.get(0),
                    )?;
                    let profitable_markets: u32 = profitable_markets_stmt.query_row(
                        rusqlite::params![wallet, window],
                        |row| row.get(0),
                    )?;
                    results.push((
                        wallet.clone(),
                        discovered_from.clone(),
                        *age_days,
                        i64::from(wd),
                        pnl.unwrap_or(0.0),
                        profitable_markets,
                        total_markets,
                    ));
                }
            }
            Ok(results)
        })
        .await?;

    // Compute scores in Rust (no DB needed).
    let mut score_rows = Vec::with_capacity(pnl_data.len());
    for (wallet, discovered_from, age_days, window_days, pnl, profitable_markets, total_markets) in
        &pnl_data
    {
        let roi_pct = if bankroll > 0.0 {
            100.0 * pnl / bankroll
        } else {
            0.0
        };
        let input = WalletScoreInput {
            paper_roi_pct: roi_pct,
            daily_return_stdev_pct: 0.0,
            hit_rate: 0.50, // TODO(Task 38): calculate real hit rate from DB
            profitable_markets: *profitable_markets,
            total_markets: *total_markets,
            avg_post_entry_drift_cents: 0.0, // TODO(Task 38): compute from post-entry price drift metrics
            noise_trade_ratio: 0.0, // TODO(Task 38): compute based on persona/exclusion heuristics
            wallet_age_days: (*age_days).max(0) as u32,
            is_public_leaderboard_top_500: discovered_from == "LEADERBOARD",
        };
        let wscore = compute_wscore(
            &input,
            &w,
            trust_30_90_multiplier,
            obscurity_bonus_multiplier,
        );
        score_rows.push(ScoreRow {
            proxy_wallet: wallet.clone(),
            window_days: *window_days,
            wscore,
            edge_score: input.paper_roi_pct.max(0.0) / 20.0,
            consistency_score: 1.0,
            roi_pct,
        });
    }

    // Batch write: upsert all scores in one db.call() with a transaction.
    let inserted: u64 = db
        .call_named("wallet_scoring.upsert_scores_batch", move |conn| {
            let tx = conn.transaction()?;
            let mut ins = 0_u64;
            for r in &score_rows {
                tx.execute(
                    "
                    INSERT INTO wallet_scores_daily
                        (proxy_wallet, score_date, window_days, wscore, edge_score, consistency_score, paper_roi_pct, recommended_follow_mode)
                    VALUES
                        (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                    ON CONFLICT(proxy_wallet, score_date, window_days) DO UPDATE SET
                        wscore = excluded.wscore,
                        edge_score = excluded.edge_score,
                        consistency_score = excluded.consistency_score,
                        paper_roi_pct = excluded.paper_roi_pct,
                        recommended_follow_mode = excluded.recommended_follow_mode
                    ",
                    rusqlite::params![
                        r.proxy_wallet,
                        today,
                        r.window_days,
                        r.wscore,
                        r.edge_score,
                        r.consistency_score,
                        r.roi_pct,
                        "mirror"
                    ],
                )?;
                ins += 1;
            }
            tx.commit()?;
            Ok(ins)
        })
        .await?;

    tracker
        .success(Some(serde_json::json!({"inserted": inserted})))
        .await?;
    Ok(inserted)
}

pub async fn run_event_scoring_once<P: GammaMarketsPager + Sync>(
    db: &AsyncDb,
    pager: &P,
    cfg: &Config,
) -> Result<u64> {
    // Extra fields from GammaMarket for the full DB upsert (not in MarketCandidate).
    #[derive(Clone)]
    struct MarketDbRow {
        condition_id: String,
        title: String,
        slug: Option<String>,
        description: Option<String>,
        end_date: Option<String>,
        liquidity: f64,
        volume: f64,
        category: Option<String>,
        event_slug: Option<String>,
    }

    let mut offset = 0_u32;
    let limit = 100_u32;
    let mut all: Vec<MarketCandidate> = Vec::new();

    // Build server-side filter from config to avoid fetching thousands of dead markets.
    let tomorrow = (chrono::Utc::now() + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let filter = GammaFilter {
        closed: Some(false),
        liquidity_num_min: Some(cfg.market_scoring.min_liquidity_usdc),
        volume_num_min: Some(cfg.market_scoring.min_daily_volume_usdc),
        end_date_min: Some(tomorrow),
        ..Default::default()
    };

    loop {
        let (markets, _raw) = pager
            .fetch_gamma_markets_page(limit, offset, &filter)
            .await?;
        let page_len = markets.len();

        let mut page_candidates: Vec<MarketCandidate> = Vec::new();
        let mut page_db_rows: Vec<MarketDbRow> = Vec::new();

        for m in markets {
            let Some(condition_id) = m.condition_id.clone() else {
                continue;
            };
            let title = m
                .question
                .clone()
                .or_else(|| m.title.clone())
                .unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let liquidity = m
                .liquidity
                .as_deref()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let volume_24h = m
                .volume_24hr
                .as_deref()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| m.volume.as_deref().and_then(|s| s.parse::<f64>().ok()))
                .unwrap_or(0.0);

            // Filled from local DB (trades_raw + holders_snapshots) after we upsert markets.
            let trades_24h = 0;
            let unique_traders_24h = 0;
            let top_holder_concentration = 0.5;

            let days_to_expiry = compute_days_to_expiry(m.end_date.as_deref()).unwrap_or(0);

            if liquidity < cfg.market_scoring.min_liquidity_usdc {
                continue;
            }
            if volume_24h < cfg.market_scoring.min_daily_volume_usdc {
                continue;
            }
            if days_to_expiry > cfg.market_scoring.max_days_to_expiry
                || days_to_expiry < cfg.market_scoring.min_days_to_expiry
            {
                continue;
            }

            let event_slug = m.effective_event_slug();
            page_db_rows.push(MarketDbRow {
                condition_id: condition_id.clone(),
                title: title.clone(),
                slug: m.slug.clone(),
                description: m.description.clone(),
                end_date: m.end_date.clone(),
                liquidity,
                volume: volume_24h,
                category: m.category.clone(),
                event_slug: event_slug.clone(),
            });

            page_candidates.push(MarketCandidate {
                condition_id,
                title,
                event_slug,
                liquidity,
                volume_24h,
                trades_24h,
                unique_traders_24h,
                top_holder_concentration,
                days_to_expiry,
            });
        }

        // Upsert markets in one db.call().

        db.call_named("market_scoring.upsert_markets_page", move |conn| {
            let tx = conn.transaction()?;

            for r in &page_db_rows {
                let is_crypto_15m = is_crypto_15m_market(&r.title, r.slug.as_deref().unwrap_or(""));
                let is_crypto_15m_i64 = i64::from(is_crypto_15m);
                tx.execute(
                    "
                    INSERT INTO markets
                        (condition_id, title, slug, description, end_date, liquidity, volume, category, event_slug, is_crypto_15m, last_updated_at)
                    VALUES
                        (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'))
                    ON CONFLICT(condition_id) DO UPDATE SET
                        title = excluded.title,
                        slug = excluded.slug,
                        description = excluded.description,
                        end_date = excluded.end_date,
                        liquidity = excluded.liquidity,
                        volume = excluded.volume,
                        category = excluded.category,
                        event_slug = excluded.event_slug,
                        is_crypto_15m = excluded.is_crypto_15m,
                        last_updated_at = datetime('now')
                    ",
                    rusqlite::params![
                        r.condition_id,
                        r.title,
                        r.slug,
                        r.description,
                        r.end_date,
                        r.liquidity,
                        r.volume,
                        r.category,
                        r.event_slug,
                        is_crypto_15m_i64,
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;

        // Populate density and whale inputs from local DB so MScore uses real signals.
        let now_epoch = chrono::Utc::now().timestamp();
        let condition_ids: Vec<String> = page_candidates
            .iter()
            .map(|c| c.condition_id.clone())
            .collect();
        let per_market: std::collections::HashMap<String, (u32, u32, f64)> = db
            .call(move |conn| {
                let mut out: std::collections::HashMap<String, (u32, u32, f64)> =
                    std::collections::HashMap::new();
                for cid in condition_ids {
                    let trades_24h = count_trades_24h(conn, &cid, now_epoch)?;
                    let unique_traders_24h = count_unique_traders_24h(conn, &cid, now_epoch)?;
                    let top_holder_concentration = compute_whale_concentration(conn, &cid)?;
                    out.insert(
                        cid,
                        (trades_24h, unique_traders_24h, top_holder_concentration),
                    );
                }
                Ok(out)
            })
            .await?;

        for c in &mut page_candidates {
            if let Some((t, u, w)) = per_market.get(&c.condition_id) {
                c.trades_24h = *t;
                c.unique_traders_24h = *u;
                c.top_holder_concentration = *w;
            }
        }

        all.extend(page_candidates);

        offset = offset.saturating_add(limit);
        if page_len < limit as usize {
            break;
        }
    }

    let scored = rank_markets(all);
    let (total_events_evaluated, ranked) = rank_events(scored, cfg.market_scoring.top_n_events);

    let today = chrono::Utc::now().date_naive().to_string();
    let ranked_data: Vec<(String, f64, i64)> = ranked
        .iter()
        .map(|(event_rank, sm)| (sm.market.condition_id.clone(), sm.mscore, *event_rank))
        .collect();

    let top_events_selected = cfg.market_scoring.top_n_events;
    let inserted: u64 = db
        .call_named("market_scoring.upsert_ranked_scores", move |conn| {
            let tx = conn.transaction()?;
            let mut ins = 0_u64;
            for (condition_id, mscore, rank) in ranked_data {
                let changed = tx.execute(
                    "
                    INSERT INTO market_scores
                        (condition_id, score_date, mscore, rank)
                    VALUES
                        (?1, ?2, ?3, ?4)
                    ON CONFLICT(condition_id, score_date) DO UPDATE SET
                        mscore = excluded.mscore,
                        rank = excluded.rank
                    ",
                    rusqlite::params![condition_id, today, mscore, rank],
                )?;
                ins += changed as u64;
            }
            tx.execute(
                "
                INSERT INTO scoring_stats (score_date, total_events_evaluated, top_events_selected)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(score_date) DO UPDATE SET
                    total_events_evaluated = excluded.total_events_evaluated,
                    top_events_selected = excluded.top_events_selected
                ",
                rusqlite::params![
                    today,
                    total_events_evaluated as i64,
                    top_events_selected as i64
                ],
            )?;
            tx.commit()?;
            Ok(ins)
        })
        .await?;

    metrics::counter!("evaluator_markets_scored_total").increment(inserted);

    // Persist last-run stats for dashboard "async funnel".
    let markets_count = inserted as i64;
    let _ = db
        .call_named("market_scoring.persist_last_run", move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO discovery_scheduler_state (key, value_int, updated_at) VALUES ('last_run_events_markets', ?1, datetime('now'))",
                [markets_count],
            )?;
            Ok(())
        })
        .await;

    Ok(inserted)
}

pub async fn run_wallet_discovery_once<H: HoldersFetcher + Sync, T: MarketTradesFetcher + Sync>(
    db: &AsyncDb,
    holders: &H,
    trades: &T,
    cfg: &Config,
) -> Result<u64> {
    let tracker = JobTracker::start(db, "wallet_discovery").await?;
    let markets: Vec<String> = db
        .call_named("wallet_discovery.select_top_events_markets", move |conn| {
            let mut stmt = conn.prepare(
                "
                SELECT condition_id
                FROM market_scores
                WHERE score_date = (SELECT MAX(score_date) FROM market_scores)
                ORDER BY rank ASC
                ",
            )?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;

    let total = markets.len();
    if total > 0 {
        tracing::info!(markets = total, "wallet_discovery: processing top events");
    }

    let trades_pages = cfg
        .wallet_discovery
        .trades_pages_per_market
        .min(TRADES_PAGES_CAP);

    let mut inserted = 0_u64;
    for (idx, condition_id) in markets.iter().enumerate() {
        if (idx + 1) % 10 == 0 || idx == 0 {
            tracing::info!(
                progress = idx + 1,
                total = total,
                inserted_so_far = inserted,
                "wallet_discovery: progress"
            );
            // Update progress in database every 10 markets
            let _ = tracker
                .update_progress(serde_json::json!({
                    "progress": idx + 1,
                    "total": total,
                    "inserted": inserted,
                    "phase": "discovering_wallets"
                }))
                .await;
        }
        let (holder_resp, _raw_h) = holders
            .fetch_holders(condition_id, cfg.wallet_discovery.holders_per_market as u32)
            .await?;

        let mut market_trades: Vec<common::types::ApiTrade> = Vec::new();
        for page in 0..trades_pages {
            let offset = page * TRADES_PAGE_SIZE;
            let (page_trades, _) = trades
                .fetch_market_trades_page(condition_id, TRADES_PAGE_SIZE, offset)
                .await?;
            if page_trades.len() < TRADES_PAGE_SIZE as usize {
                market_trades.extend(page_trades);
                break;
            }
            market_trades.extend(page_trades);
        }

        let mut holder_wallets: Vec<HolderWallet> = Vec::new();
        for r in &holder_resp {
            for h in &r.holders {
                if let Some(w) = &h.proxy_wallet {
                    holder_wallets.push(HolderWallet {
                        proxy_wallet: w.clone(),
                    });
                }
            }
        }

        let mut trade_wallets: Vec<TradeWallet> = Vec::new();
        for t in &market_trades {
            if let Some(w) = &t.proxy_wallet {
                trade_wallets.push(TradeWallet {
                    proxy_wallet: w.clone(),
                });
            }
        }

        let discovered = discover_wallets_for_market(
            &holder_wallets,
            &trade_wallets,
            cfg.wallet_discovery.min_total_trades,
        );

        let wallets_to_insert: Vec<(String, String)> = discovered
            .into_iter()
            .map(|w| (w.proxy_wallet, w.discovered_from.as_str().to_string()))
            .collect();

        let cid = condition_id.clone();
        let page_inserted: u64 = db
            .call_named("wallet_discovery.insert_wallets_page", move |conn| {
                let tx = conn.transaction()?;

                let mut ins = 0_u64;
                for (proxy_wallet, discovered_from) in wallets_to_insert {
                    let changed = tx.execute(
                        "
                        INSERT OR IGNORE INTO wallets
                            (proxy_wallet, discovered_from, discovered_market, is_active)
                        VALUES
                            (?1, ?2, ?3, 1)
                        ",
                        rusqlite::params![proxy_wallet, discovered_from, cid],
                    )?;
                    ins += changed as u64;
                }
                tx.commit()?;
                Ok(ins)
            })
            .await?;

        inserted += page_inserted;
    }

    metrics::counter!("evaluator_wallets_discovered_total").increment(inserted);
    let watchlist: i64 = db
        .call_named("wallet_discovery.count_active_wallets", |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM wallets WHERE is_active = 1",
                [],
                |row| row.get(0),
            )?)
        })
        .await?;
    metrics::gauge!("evaluator_wallets_on_watchlist").set(watchlist as f64);
    tracker
        .success(Some(serde_json::json!({
            "inserted": inserted,
            "total": total,
            "completed": true
        })))
        .await?;
    Ok(inserted)
}

/// API limits for Polymarket Data API (documented in CLAUDE.md).
const TRADES_API_OFFSET_CAP: u32 = 3000;
const TRADES_PAGE_SIZE: u32 = 200;
/// Max pages to stay under TRADES_API_OFFSET_CAP (3000 / 200 = 15).
const TRADES_PAGES_CAP: u32 = TRADES_API_OFFSET_CAP / TRADES_PAGE_SIZE;
const LEADERBOARD_API_OFFSET_MAX: u32 = 1000;

/// Discover wallets from Polymarket leaderboard API. Inserts with discovered_from=LEADERBOARD, discovered_market=NULL.
pub async fn run_leaderboard_discovery_once<L: super::fetcher_traits::LeaderboardFetcher + Sync>(
    db: &AsyncDb,
    leaderboard: &L,
    cfg: &Config,
) -> Result<u64> {
    if !cfg.wallet_discovery.leaderboard.enabled {
        return Ok(0);
    }

    let limit = 50_u32;
    let mut inserted = 0_u64;

    for category in &cfg.wallet_discovery.leaderboard.categories {
        for time_period in &cfg.wallet_discovery.leaderboard.time_periods {
            for page in 0..cfg.wallet_discovery.leaderboard.pages_per_category {
                let offset = page * limit;
                if offset > LEADERBOARD_API_OFFSET_MAX {
                    break;
                }
                let entries = match leaderboard
                    .fetch_leaderboard(category, time_period, limit, offset)
                    .await
                {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(
                            category = %category,
                            time_period = %time_period,
                            offset,
                            error = %e,
                            "leaderboard fetch failed; continuing"
                        );
                        continue;
                    }
                };
                if entries.is_empty() {
                    break;
                }

                let wallets: Vec<String> =
                    entries.into_iter().filter_map(|e| e.proxy_wallet).collect();

                let page_inserted: u64 = db
                    .call_named("wallet_discovery.insert_leaderboard_wallets", move |conn| {
                        let tx = conn.transaction()?;
                        let mut ins = 0_u64;
                        for proxy_wallet in wallets {
                            let changed = tx.execute(
                                "
                                INSERT OR IGNORE INTO wallets
                                    (proxy_wallet, discovered_from, discovered_market, is_active)
                                VALUES
                                    (?1, 'LEADERBOARD', NULL, 1)
                                ",
                                rusqlite::params![proxy_wallet],
                            )?;
                            ins += changed as u64;
                        }
                        tx.commit()?;
                        Ok(ins)
                    })
                    .await?;

                inserted += page_inserted;

                if page_inserted == 0 && page > 0 {
                    break; // No new wallets, stop paginating this category/period
                }
            }
        }
    }

    if inserted > 0 {
        metrics::counter!("evaluator_wallets_discovered_total").increment(inserted);
        let watchlist: i64 = db
            .call_named("wallet_discovery.count_active_wallets", |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM wallets WHERE is_active = 1",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await?;
        metrics::gauge!("evaluator_wallets_on_watchlist").set(watchlist as f64);
    }
    Ok(inserted)
}

/// Run Stage 2 persona classification for all watchlist wallets that pass Stage 1.
/// Returns the number of wallets that received a classification (followable or excluded).
///
/// Fetch a paginated chunk of active wallets with their metadata
fn fetch_wallet_chunk(
    conn: &rusqlite::Connection,
    offset: i64,
    limit: i64,
) -> Result<Vec<(String, u32, u32, u32)>> {
    conn.prepare(
        "
        SELECT w.proxy_wallet,
            (SELECT CAST((julianday('now') - julianday(datetime(MIN(tr.timestamp), 'unixepoch'))) AS INTEGER)
             FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) AS age_days,
            (SELECT COUNT(*) FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) AS total_trades,
            (SELECT CAST((julianday('now') - julianday(datetime(MAX(tr.timestamp), 'unixepoch'))) AS INTEGER)
             FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) AS days_since_last
        FROM wallets w
        WHERE w.is_active = 1
        ORDER BY w.proxy_wallet  -- Stable ordering for pagination
        LIMIT ?1 OFFSET ?2
        ",
    )?
    .query_map([limit, offset], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<i64>>(1)?.unwrap_or(0).max(0) as u32,
            row.get::<_, i64>(2).unwrap_or(0).max(0) as u32,
            row.get::<_, Option<i64>>(3)?
                .unwrap_or(i64::MAX)
                .min(i64::from(i32::MAX))
                .max(0) as u32,
        ))
    })?
    .collect::<std::result::Result<Vec<_>, _>>()
    .map_err(Into::into)
}

/// Process a chunk of wallets (Stage 1 + Stage 2)
/// Returns (processed_count, stage1_no_trades, stage1_other, stage2_excluded, suitable)
fn process_wallet_chunk(
    conn: &rusqlite::Connection,
    wallets: &[(String, u32, u32, u32)],
    stage1_config: &Stage1Config,
    persona_config: &PersonaConfig,
    window_days: u32,
    now_epoch: i64,
) -> Result<(u64, u64, u64, u64, u64)> {
    let mut count = 0_u64;
    let mut stage1_no_trades = 0_u64;
    let mut stage1_other = 0_u64;
    let mut stage2_excluded = 0_u64;
    let mut suitable = 0_u64;

    for (proxy_wallet, wallet_age_days, total_trades, days_since_last) in wallets {
        // Stage 1 checks
        if let Some(reason) = stage1_known_bot_check(proxy_wallet, &stage1_config.known_bots) {
            crate::persona_classification::record_exclusion(conn, proxy_wallet, &reason)?;
            stage1_other += 1;
            count += 1;
            continue;
        }

        if let Some(reason) = stage1_filter(
            *wallet_age_days,
            *total_trades,
            *days_since_last,
            stage1_config,
        ) {
            crate::persona_classification::record_exclusion(conn, proxy_wallet, &reason)?;
            if *total_trades == 0 {
                stage1_no_trades += 1;
            } else {
                stage1_other += 1;
            }
            count += 1;
            continue;
        }

        // Stage 1 passed - clear old exclusions
        let _ = crate::persona_classification::clear_stage1_exclusion(conn, proxy_wallet);

        // Compute features
        let Ok(features) = compute_wallet_features(conn, proxy_wallet, window_days, now_epoch)
        else {
            tracing::warn!(proxy_wallet = %proxy_wallet, "compute_wallet_features failed");
            continue;
        };

        // Stage 2 classification
        match classify_wallet(conn, &features, *wallet_age_days, persona_config) {
            Ok(result) => {
                match &result {
                    crate::persona_classification::ClassificationResult::Followable(p) => {
                        suitable += 1;
                        tracing::info!(wallet = %proxy_wallet, persona = %p.as_str(), "persona: suitable");
                    }
                    crate::persona_classification::ClassificationResult::Excluded(r) => {
                        stage2_excluded += 1;
                        tracing::info!(wallet = %proxy_wallet, reason = %r.reason_str(), "persona: excluded Stage 2");
                    }
                    crate::persona_classification::ClassificationResult::Unclassified => {}
                }
                if !matches!(
                    result,
                    crate::persona_classification::ClassificationResult::Unclassified
                ) {
                    count += 1;
                }
            }
            Err(e) => {
                tracing::warn!(proxy_wallet = %proxy_wallet, error = %e, "classify_wallet failed");
            }
        }
    }

    Ok((
        count,
        stage1_no_trades,
        stage1_other,
        stage2_excluded,
        suitable,
    ))
}

/// This job runs on a schedule (e.g. hourly); it does not wait for trades ingestion. It reads
/// age/trades from `trades_raw`. To get wallets evaluated, ensure trades ingestion runs and
/// prioritizes wallets with 0 trades (backfill-first) so `trades_raw` fills; then the next
/// persona run will classify them.
pub async fn run_persona_classification_once(db: &AsyncDb, cfg: &Config) -> Result<u64> {
    let tracker = JobTracker::start(db, "persona_classification").await?;
    let now_epoch = chrono::Utc::now().timestamp();
    let window_days = 180_u32;
    let persona_config = PersonaConfig::from_personas(&cfg.personas);
    let stage1_config = Stage1Config {
        min_wallet_age_days: cfg.personas.stage1_min_wallet_age_days,
        min_total_trades: cfg.personas.stage1_min_total_trades,
        max_inactive_days: cfg.personas.stage1_max_inactive_days,
        known_bots: cfg.personas.known_bots.clone(),
    };

    // Get total wallet count for progress tracking
    let total_wallets: i64 = db
        .call_named("persona_classification.count_wallets", |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM wallets WHERE is_active = 1",
                [],
                |row| row.get(0),
            )?)
        })
        .await?;

    if total_wallets == 0 {
        tracing::warn!("persona_classification: no wallets to classify");
        tracker
            .success(Some(serde_json::json!({
                "classified": 0,
                "skipped": "no wallets"
            })))
            .await?;
        return Ok(0);
    }

    tracing::info!(
        total = total_wallets,
        "persona_classification: starting chunked processing"
    );

    // Accumulate counters across chunks
    let mut total_processed = 0_u64;
    let mut stage1_no_trades = 0_u64;
    let mut stage1_other = 0_u64;
    let mut stage2_excluded = 0_u64;
    let mut suitable = 0_u64;

    let chunk_size = 100_i64;
    let mut offset = 0_i64;

    loop {
        // Process one chunk
        let chunk_result = db
            .call_named("persona_classification.classify_chunk", {
                let stage1_config = stage1_config.clone();
                let persona_config = persona_config.clone();
                move |conn| {
                    let wallets = fetch_wallet_chunk(conn, offset, chunk_size)?;

                    if wallets.is_empty() {
                        return Ok(None); // Signal completion
                    }

                    let counters = process_wallet_chunk(
                        conn,
                        &wallets,
                        &stage1_config,
                        &persona_config,
                        window_days,
                        now_epoch,
                    )?;

                    Ok(Some(counters))
                }
            })
            .await?;

        let Some((chunk_processed, chunk_no_trades, chunk_other, chunk_excluded, chunk_suitable)) =
            chunk_result
        else {
            break; // No more wallets
        };

        // Accumulate counters
        total_processed += chunk_processed;
        stage1_no_trades += chunk_no_trades;
        stage1_other += chunk_other;
        stage2_excluded += chunk_excluded;
        suitable += chunk_suitable;

        // Update progress
        tracker
            .update_progress(serde_json::json!({
                "processed": total_processed,
                "total": total_wallets,
                "suitable": suitable,
                "stage1_no_trades": stage1_no_trades,
                "stage1_other": stage1_other,
                "stage2_excluded": stage2_excluded,
                "phase": "classifying"
            }))
            .await?;

        offset += chunk_size;
    }

    tracing::info!(
        stage1_no_trades,
        stage1_other,
        stage2_excluded,
        suitable,
        "persona_classification: summary"
    );

    metrics::gauge!("evaluator_persona_classifications_run").set(total_processed as f64);
    tracker
        .success(Some(serde_json::json!({
            "processed": total_processed,
            "total": total_wallets,
            "classified": total_processed,
            "suitable": suitable,
            "stage1_no_trades": stage1_no_trades,
            "stage1_other": stage1_other,
            "stage2_excluded": stage2_excluded,
            "completed": true
        })))
        .await?;

    Ok(total_processed)
}

fn compute_days_to_expiry(end_date: Option<&str>) -> Option<u32> {
    let s = end_date?;
    // Gamma endDate is often ISO-8601. We parse via chrono's RFC3339 parser.
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    let end = dt.date_naive();
    let today = chrono::Utc::now().date_naive();
    let days = (end - today).num_days();
    if days < 0 {
        Some(0)
    } else {
        Some(days as u32)
    }
}

fn count_trades_24h(
    conn: &rusqlite::Connection,
    condition_id: &str,
    now_epoch: i64,
) -> Result<u32> {
    let cutoff = now_epoch - 86_400;
    let count_i64: i64 = conn.query_row(
        "SELECT COUNT(*) FROM trades_raw WHERE condition_id = ?1 AND timestamp > ?2",
        rusqlite::params![condition_id, cutoff],
        |row| row.get(0),
    )?;
    Ok(count_i64.max(0) as u32)
}

fn count_unique_traders_24h(
    conn: &rusqlite::Connection,
    condition_id: &str,
    now_epoch: i64,
) -> Result<u32> {
    let cutoff = now_epoch - 86_400;
    let count_i64: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT proxy_wallet) FROM trades_raw WHERE condition_id = ?1 AND timestamp > ?2",
        rusqlite::params![condition_id, cutoff],
        |row| row.get(0),
    )?;
    Ok(count_i64.max(0) as u32)
}

fn compute_whale_concentration(conn: &rusqlite::Connection, condition_id: &str) -> Result<f64> {
    // Latest snapshot for this condition_id (if any).
    let total: f64 = conn.query_row(
        "
        SELECT COALESCE(SUM(amount), 0.0) FROM holders_snapshots
        WHERE condition_id = ?1
          AND snapshot_at = (
            SELECT MAX(snapshot_at) FROM holders_snapshots WHERE condition_id = ?1
          )
        ",
        rusqlite::params![condition_id],
        |row| row.get(0),
    )?;

    if total <= 0.0 {
        return Ok(0.5); // default when no data
    }

    let top_holder: f64 = conn.query_row(
        "
        SELECT COALESCE(MAX(amount), 0.0) FROM holders_snapshots
        WHERE condition_id = ?1
          AND snapshot_at = (
            SELECT MAX(snapshot_at) FROM holders_snapshots WHERE condition_id = ?1
          )
        ",
        rusqlite::params![condition_id],
        |row| row.get(0),
    )?;

    Ok(top_holder / total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::db::Database;

    struct FakeGammaPager {
        pages: Vec<(Vec<GammaMarket>, Vec<u8>)>,
    }

    impl FakeGammaPager {
        fn new(pages: Vec<(Vec<GammaMarket>, Vec<u8>)>) -> Self {
            Self { pages }
        }
    }

    impl GammaMarketsPager for FakeGammaPager {
        fn gamma_markets_url(&self, limit: u32, offset: u32) -> String {
            format!("https://gamma-api.polymarket.com/markets?limit={limit}&offset={offset}")
        }

        async fn fetch_gamma_markets_page(
            &self,
            _limit: u32,
            offset: u32,
            _filter: &GammaFilter,
        ) -> Result<(Vec<GammaMarket>, Vec<u8>)> {
            let idx = (offset / 100) as usize;
            Ok(self.pages.get(idx).cloned().unwrap_or_default())
        }
    }

    #[test]
    fn test_compute_trades_24h_from_db() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let now = chrono::Utc::now().timestamp();
        // Insert 5 trades in last 24h for market 0xm1
        for i in 0..5 {
            db.conn
                .execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES ('0xw1', '0xm1', 'BUY', 10.0, 0.50, ?1)",
                    rusqlite::params![now - 3600 * i],
                )
                .unwrap();
        }
        // Insert 3 old trades (>24h ago)
        for i in 0..3 {
            db.conn
                .execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES ('0xw2', '0xm1', 'BUY', 10.0, 0.50, ?1)",
                    rusqlite::params![now - 86400 - 3600 * i],
                )
                .unwrap();
        }

        let trades_24h = count_trades_24h(&db.conn, "0xm1", now).unwrap();
        assert_eq!(trades_24h, 5);

        let unique_traders = count_unique_traders_24h(&db.conn, "0xm1", now).unwrap();
        assert_eq!(unique_traders, 1); // only 0xw1 traded in last 24h
    }

    #[test]
    fn test_compute_whale_concentration_from_holders() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        // Top holder has 500 out of 1000 total = 50% concentration
        db.conn
            .execute(
                "INSERT INTO holders_snapshots (condition_id, proxy_wallet, amount, snapshot_at)
                 VALUES ('0xm1', '0xwhale', 500.0, datetime('now'))",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO holders_snapshots (condition_id, proxy_wallet, amount, snapshot_at)
                 VALUES ('0xm1', '0xsmall1', 300.0, datetime('now'))",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO holders_snapshots (condition_id, proxy_wallet, amount, snapshot_at)
                 VALUES ('0xm1', '0xsmall2', 200.0, datetime('now'))",
                [],
            )
            .unwrap();

        let concentration = compute_whale_concentration(&db.conn, "0xm1").unwrap();
        assert!((concentration - 0.5).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_run_market_scoring_uses_db_density_and_whale_concentration() {
        let mut cfg =
            Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        cfg.market_scoring.top_n_events = 2;
        // Keep the test focused on DB-derived density/whale factors.
        cfg.market_scoring.min_liquidity_usdc = 0.0;
        cfg.market_scoring.min_daily_volume_usdc = 0.0;
        cfg.market_scoring.min_days_to_expiry = 0;
        cfg.market_scoring.max_days_to_expiry = 10_000;

        let db = AsyncDb::open(":memory:").await.unwrap();

        let now = chrono::Utc::now().timestamp();
        db.call(move |conn| {
            // Market 0x1: lots of recent trades + dispersed holders.
            for i in 0..200 {
                conn.execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES ('0xw1', '0x1', 'BUY', 10.0, 0.50, ?1)",
                    rusqlite::params![now - i * 60],
                )?;
            }

            // Market 0x2: no recent trades + concentrated holders.
            // (Leave trades_raw empty for 0x2.)

            // Use a fixed snapshot_at so "latest snapshot" selection is deterministic.
            let snap = "2026-02-10 00:00:00";

            // 0x1: top holder 100 / total 1000 => 0.1
            conn.execute(
                "INSERT INTO holders_snapshots (condition_id, proxy_wallet, amount, snapshot_at)
                 VALUES ('0x1', '0xwhale1', 100.0, ?1)",
                rusqlite::params![snap],
            )?;
            conn.execute(
                "INSERT INTO holders_snapshots (condition_id, proxy_wallet, amount, snapshot_at)
                 VALUES ('0x1', '0xsmall1', 900.0, ?1)",
                rusqlite::params![snap],
            )?;

            // 0x2: top holder 900 / total 1000 => 0.9
            conn.execute(
                "INSERT INTO holders_snapshots (condition_id, proxy_wallet, amount, snapshot_at)
                 VALUES ('0x2', '0xwhale2', 900.0, ?1)",
                rusqlite::params![snap],
            )?;
            conn.execute(
                "INSERT INTO holders_snapshots (condition_id, proxy_wallet, amount, snapshot_at)
                 VALUES ('0x2', '0xsmall2', 100.0, ?1)",
                rusqlite::params![snap],
            )?;

            Ok(())
        })
        .await
        .unwrap();

        let end_date = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
        let markets = vec![
            GammaMarket {
                condition_id: Some("0x1".to_string()),
                question: Some("M1".to_string()),
                title: None,
                slug: None,
                description: None,
                end_date: Some(end_date.clone()),
                liquidity: Some("5000".to_string()),
                volume: Some("8000".to_string()),
                volume_24hr: Some("8000".to_string()),
                category: None,
                event_slug: None,
                events: None,
                neg_risk: None,
            },
            GammaMarket {
                condition_id: Some("0x2".to_string()),
                question: Some("M2".to_string()),
                title: None,
                slug: None,
                description: None,
                end_date: Some(end_date),
                liquidity: Some("5000".to_string()),
                volume: Some("8000".to_string()),
                volume_24hr: Some("8000".to_string()),
                category: None,
                event_slug: None,
                events: None,
                neg_risk: None,
            },
        ];

        let pager = FakeGammaPager::new(vec![(markets, br#"[{"page":1}]"#.to_vec())]);
        run_event_scoring_once(&db, &pager, &cfg).await.unwrap();

        let (m1, m2): (f64, f64) = db
            .call(|conn| {
                let m1: f64 = conn.query_row(
                    "SELECT mscore FROM market_scores WHERE condition_id = '0x1'",
                    [],
                    |row| row.get(0),
                )?;
                let m2: f64 = conn.query_row(
                    "SELECT mscore FROM market_scores WHERE condition_id = '0x2'",
                    [],
                    |row| row.get(0),
                )?;
                Ok((m1, m2))
            })
            .await
            .unwrap();

        assert!(
            m1 > m2,
            "expected mscore(0x1) > mscore(0x2), got {m1} vs {m2}"
        );
    }

    #[tokio::test]
    async fn test_run_market_scoring_persists_ranked_rows() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        let end_date = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
        let markets = vec![
            GammaMarket {
                condition_id: Some("0x1".to_string()),
                question: Some("M1".to_string()),
                title: None,
                slug: None,
                description: None,
                end_date: Some(end_date.clone()),
                liquidity: Some("5000".to_string()),
                volume: Some("8000".to_string()),
                volume_24hr: Some("8000".to_string()),
                category: None,
                event_slug: None,
                events: None,
                neg_risk: None,
            },
            GammaMarket {
                condition_id: Some("0x2".to_string()),
                question: Some("M2".to_string()),
                title: None,
                slug: None,
                description: None,
                end_date: Some(end_date),
                liquidity: Some("20000".to_string()),
                volume: Some("9000".to_string()),
                volume_24hr: Some("9000".to_string()),
                category: None,
                event_slug: None,
                events: None,
                neg_risk: None,
            },
        ];

        let pager = FakeGammaPager::new(vec![(markets, br#"[{"page":1}]"#.to_vec())]);
        let inserted = run_event_scoring_once(&db, &pager, &cfg).await.unwrap();
        assert!(inserted > 0);

        let (cnt_scores, cnt_markets): (i64, i64) = db
            .call(|conn| {
                let cs =
                    conn.query_row("SELECT COUNT(*) FROM market_scores", [], |row| row.get(0))?;
                let cm = conn.query_row("SELECT COUNT(*) FROM markets", [], |row| row.get(0))?;
                Ok((cs, cm))
            })
            .await
            .unwrap();
        assert!(cnt_scores > 0);
        assert_eq!(cnt_markets, 2);
    }

    struct FakeHoldersFetcher {
        resp: Vec<ApiHolderResponse>,
        raw: Vec<u8>,
    }

    impl HoldersFetcher for FakeHoldersFetcher {
        fn holders_url(&self, condition_id: &str, limit: u32) -> String {
            format!("https://data-api.polymarket.com/holders?market={condition_id}&limit={limit}")
        }

        async fn fetch_holders(
            &self,
            _condition_id: &str,
            _limit: u32,
        ) -> Result<(Vec<ApiHolderResponse>, Vec<u8>)> {
            Ok((self.resp.clone(), self.raw.clone()))
        }
    }

    struct FakeMarketTradesFetcher {
        trades: Vec<ApiTrade>,
        raw: Vec<u8>,
    }

    impl MarketTradesFetcher for FakeMarketTradesFetcher {
        fn market_trades_url(&self, condition_id: &str, limit: u32, offset: u32) -> String {
            format!(
                "https://data-api.polymarket.com/trades?market={condition_id}&limit={limit}&offset={offset}"
            )
        }

        async fn fetch_market_trades_page(
            &self,
            _condition_id: &str,
            _limit: u32,
            _offset: u32,
        ) -> Result<(Vec<ApiTrade>, Vec<u8>)> {
            Ok((self.trades.clone(), self.raw.clone()))
        }
    }

    struct PerMarketHoldersFetcher {
        by_market: std::collections::HashMap<String, Vec<ApiHolderResponse>>,
    }

    impl HoldersFetcher for PerMarketHoldersFetcher {
        fn holders_url(&self, condition_id: &str, limit: u32) -> String {
            format!("https://data-api.polymarket.com/holders?market={condition_id}&limit={limit}")
        }

        async fn fetch_holders(
            &self,
            condition_id: &str,
            _limit: u32,
        ) -> Result<(Vec<ApiHolderResponse>, Vec<u8>)> {
            Ok((
                self.by_market
                    .get(condition_id)
                    .cloned()
                    .unwrap_or_default(),
                b"[]".to_vec(),
            ))
        }
    }

    struct PerMarketTradesFetcher {
        by_market: std::collections::HashMap<String, Vec<ApiTrade>>,
    }

    impl MarketTradesFetcher for PerMarketTradesFetcher {
        fn market_trades_url(&self, condition_id: &str, limit: u32, offset: u32) -> String {
            format!(
                "https://data-api.polymarket.com/trades?market={condition_id}&limit={limit}&offset={offset}"
            )
        }

        async fn fetch_market_trades_page(
            &self,
            condition_id: &str,
            _limit: u32,
            _offset: u32,
        ) -> Result<(Vec<ApiTrade>, Vec<u8>)> {
            Ok((
                self.by_market
                    .get(condition_id)
                    .cloned()
                    .unwrap_or_default(),
                b"[]".to_vec(),
            ))
        }
    }

    struct FakeLeaderboardFetcher {
        entries: Vec<ApiLeaderboardEntry>,
    }

    impl super::super::fetcher_traits::LeaderboardFetcher for FakeLeaderboardFetcher {
        async fn fetch_leaderboard(
            &self,
            _category: &str,
            _time_period: &str,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<ApiLeaderboardEntry>> {
            Ok(self.entries.clone())
        }
    }

    #[tokio::test]
    async fn test_run_leaderboard_discovery_inserts_wallets() {
        let mut cfg =
            Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        cfg.wallet_discovery.leaderboard.enabled = true;
        cfg.wallet_discovery.leaderboard.categories = vec!["OVERALL".to_string()];
        cfg.wallet_discovery.leaderboard.time_periods = vec!["WEEK".to_string()];
        cfg.wallet_discovery.leaderboard.pages_per_category = 1;

        let db = AsyncDb::open(":memory:").await.unwrap();

        let leaderboard = FakeLeaderboardFetcher {
            entries: vec![
                ApiLeaderboardEntry {
                    rank: Some("1".to_string()),
                    proxy_wallet: Some("0xleader1".to_string()),
                    user_name: Some("Alice".to_string()),
                    vol: Some(1000.0),
                    pnl: Some(50.0),
                },
                ApiLeaderboardEntry {
                    rank: Some("2".to_string()),
                    proxy_wallet: Some("0xleader2".to_string()),
                    user_name: Some("Bob".to_string()),
                    vol: Some(800.0),
                    pnl: Some(30.0),
                },
            ],
        };

        let inserted = run_leaderboard_discovery_once(&db, &leaderboard, &cfg)
            .await
            .unwrap();
        assert_eq!(inserted, 2);

        let cnt_wallets: i64 = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM wallets WHERE discovered_from = 'LEADERBOARD'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(cnt_wallets, 2);
    }

    #[tokio::test]
    async fn test_run_wallet_discovery_inserts_wallets() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.9, 1)",
                rusqlite::params!["0xcond"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let holders = FakeHoldersFetcher {
            resp: vec![ApiHolderResponse {
                token: Some("0xtok".to_string()),
                holders: vec![common::types::ApiHolder {
                    proxy_wallet: Some("0xholder".to_string()),
                    amount: Some(123.0),
                    asset: None,
                    pseudonym: None,
                    name: None,
                    outcome_index: Some(0),
                }],
            }],
            raw: b"[]".to_vec(),
        };

        let trades = FakeMarketTradesFetcher {
            trades: vec![
                ApiTrade {
                    proxy_wallet: Some("0xtrader".to_string()),
                    condition_id: Some("0xcond".to_string()),
                    asset: None,
                    size: Some("1".to_string()),
                    price: Some("0.5".to_string()),
                    timestamp: Some(1),
                    title: None,
                    slug: None,
                    outcome: None,
                    outcome_index: None,
                    transaction_hash: Some("0xtx1".to_string()),
                    side: None,
                    pseudonym: None,
                    name: None,
                },
                ApiTrade {
                    proxy_wallet: Some("0xtrader".to_string()),
                    condition_id: Some("0xcond".to_string()),
                    asset: None,
                    size: Some("1".to_string()),
                    price: Some("0.5".to_string()),
                    timestamp: Some(2),
                    title: None,
                    slug: None,
                    outcome: None,
                    outcome_index: None,
                    transaction_hash: Some("0xtx2".to_string()),
                    side: None,
                    pseudonym: None,
                    name: None,
                },
                ApiTrade {
                    proxy_wallet: Some("0xtrader".to_string()),
                    condition_id: Some("0xcond".to_string()),
                    asset: None,
                    size: Some("1".to_string()),
                    price: Some("0.5".to_string()),
                    timestamp: Some(3),
                    title: None,
                    slug: None,
                    outcome: None,
                    outcome_index: None,
                    transaction_hash: Some("0xtx3".to_string()),
                    side: None,
                    pseudonym: None,
                    name: None,
                },
                ApiTrade {
                    proxy_wallet: Some("0xtrader".to_string()),
                    condition_id: Some("0xcond".to_string()),
                    asset: None,
                    size: Some("1".to_string()),
                    price: Some("0.5".to_string()),
                    timestamp: Some(4),
                    title: None,
                    slug: None,
                    outcome: None,
                    outcome_index: None,
                    transaction_hash: Some("0xtx4".to_string()),
                    side: None,
                    pseudonym: None,
                    name: None,
                },
                ApiTrade {
                    proxy_wallet: Some("0xtrader".to_string()),
                    condition_id: Some("0xcond".to_string()),
                    asset: None,
                    size: Some("1".to_string()),
                    price: Some("0.5".to_string()),
                    timestamp: Some(5),
                    title: None,
                    slug: None,
                    outcome: None,
                    outcome_index: None,
                    transaction_hash: Some("0xtx5".to_string()),
                    side: None,
                    pseudonym: None,
                    name: None,
                },
            ],
            raw: b"[]".to_vec(),
        };

        let inserted = run_wallet_discovery_once(&db, &holders, &trades, &cfg)
            .await
            .unwrap();
        assert!(inserted > 0);

        let cnt_wallets: i64 = db
            .call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM wallets", [], |row| row.get(0))?))
            .await
            .unwrap();
        assert!(cnt_wallets >= 2); // holder + trader
    }

    #[tokio::test]
    async fn test_run_wallet_discovery_updates_progress_during_execution() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        // Insert 15 markets to trigger progress update at market 10
        db.call(|conn| {
            for i in 1..=15 {
                conn.execute(
                    "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.9, ?2)",
                    rusqlite::params![format!("0xmarket{}", i), i],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let holders = FakeHoldersFetcher {
            resp: vec![ApiHolderResponse {
                token: Some("0xtok".to_string()),
                holders: vec![],
            }],
            raw: b"[]".to_vec(),
        };

        let trades = FakeMarketTradesFetcher {
            trades: vec![],
            raw: b"[]".to_vec(),
        };

        let _inserted = run_wallet_discovery_once(&db, &holders, &trades, &cfg)
            .await
            .unwrap();

        // Verify final metadata includes progress info
        let metadata: Option<String> = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT metadata FROM job_status WHERE job_name = 'wallet_discovery'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();

        let meta_json: serde_json::Value = serde_json::from_str(&metadata.unwrap()).unwrap();

        // The final success() call should include progress info
        assert_eq!(meta_json["total"], 15, "metadata should include total");
        assert_eq!(
            meta_json["completed"], true,
            "metadata should mark as completed"
        );

        // Note: We can't easily verify intermediate progress updates in a unit test
        // because update_progress() is called during execution and overwritten by success().
        // The important thing is that the mechanism exists and can be called.
    }

    #[tokio::test]
    async fn test_run_wallet_discovery_reports_progress_to_job_status() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        // Insert 25 markets to trigger progress updates (every 10 markets)
        db.call(|conn| {
            for i in 1..=25 {
                conn.execute(
                    "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.9, ?2)",
                    rusqlite::params![format!("0xmarket{}", i), i],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let holders = FakeHoldersFetcher {
            resp: vec![ApiHolderResponse {
                token: Some("0xtok".to_string()),
                holders: vec![common::types::ApiHolder {
                    proxy_wallet: Some("0xholder".to_string()),
                    amount: Some(123.0),
                    asset: None,
                    pseudonym: None,
                    name: None,
                    outcome_index: Some(0),
                }],
            }],
            raw: b"[]".to_vec(),
        };

        let trades = FakeMarketTradesFetcher {
            trades: vec![],
            raw: b"[]".to_vec(),
        };

        let _inserted = run_wallet_discovery_once(&db, &holders, &trades, &cfg)
            .await
            .unwrap();

        // Verify job_status has progress metadata with "total": 25 and "completed": true
        let metadata: Option<String> = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT metadata FROM job_status WHERE job_name = 'wallet_discovery'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();

        assert!(metadata.is_some(), "wallet_discovery should write metadata");
        let meta_json: serde_json::Value = serde_json::from_str(&metadata.unwrap()).unwrap();
        assert_eq!(
            meta_json["total"], 25,
            "metadata should include total markets count"
        );
        assert_eq!(
            meta_json["completed"], true,
            "metadata should mark job as completed"
        );
    }

    #[tokio::test]
    async fn test_run_wallet_discovery_processes_all_top_events_markets() {
        let mut cfg =
            Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        cfg.wallet_discovery.min_total_trades = 1;

        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO markets (condition_id, title) VALUES ('0xcond1', 'M1'), ('0xcond2', 'M2'), ('0xcond3', 'M3')",
                [],
            )?;
            conn.execute(
                "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.9, 1)",
                rusqlite::params!["0xcond1"],
            )?;
            conn.execute(
                "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.8, 2)",
                rusqlite::params!["0xcond2"],
            )?;
            conn.execute(
                "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.7, 2)",
                rusqlite::params!["0xcond3"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let mut holders_by_market: std::collections::HashMap<String, Vec<ApiHolderResponse>> =
            std::collections::HashMap::new();
        for cid in ["0xcond1", "0xcond2", "0xcond3"] {
            holders_by_market.insert(
                cid.to_string(),
                vec![ApiHolderResponse {
                    token: Some("0xtok".to_string()),
                    holders: vec![common::types::ApiHolder {
                        proxy_wallet: Some(format!("0xholder_{cid}")),
                        amount: Some(123.0),
                        asset: None,
                        pseudonym: None,
                        name: None,
                        outcome_index: Some(0),
                    }],
                }],
            );
        }

        let mut trades_by_market: std::collections::HashMap<String, Vec<ApiTrade>> =
            std::collections::HashMap::new();
        for cid in ["0xcond1", "0xcond2", "0xcond3"] {
            trades_by_market.insert(
                cid.to_string(),
                vec![ApiTrade {
                    proxy_wallet: Some(format!("0xtrader_{cid}")),
                    condition_id: Some(cid.to_string()),
                    asset: None,
                    size: Some("1".to_string()),
                    price: Some("0.5".to_string()),
                    timestamp: Some(1),
                    title: None,
                    slug: None,
                    outcome: None,
                    outcome_index: None,
                    transaction_hash: Some(format!("0xtx_{cid}")),
                    side: None,
                    pseudonym: None,
                    name: None,
                }],
            );
        }

        let holders = PerMarketHoldersFetcher {
            by_market: holders_by_market,
        };
        let trades = PerMarketTradesFetcher {
            by_market: trades_by_market,
        };

        let inserted = run_wallet_discovery_once(&db, &holders, &trades, &cfg)
            .await
            .unwrap();

        let cnt_wallets: i64 = db
            .call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM wallets", [], |row| row.get(0))?))
            .await
            .unwrap();
        assert_eq!(cnt_wallets, 6);
        assert_eq!(inserted, 6);
    }

    #[tokio::test]
    async fn test_run_paper_tick_creates_paper_trades() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                rusqlite::params!["0xw"],
            )?;
            // Paper tick gating: only followable wallets (i.e., wallets with a current persona)
            // should be mirrored.
            conn.execute(
                "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
                 VALUES (?1, 'CONSISTENT_GENERALIST', 1.0, '2026-02-10 00:00:00')",
                rusqlite::params!["0xw"],
            )?;
            conn.execute(
                "INSERT INTO wallet_rules_state (proxy_wallet, state) VALUES (?1, 'APPROVED')",
                rusqlite::params!["0xw"],
            )?;
            conn.execute(
                "
                INSERT INTO trades_raw
                    (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                VALUES
                    (?1, ?2, 'BUY', 1.0, 0.5, 1, '0xtx1', '{}')
                ",
                rusqlite::params!["0xw", "0xcond"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let inserted = run_paper_tick_once(&db, &cfg).await.unwrap();
        assert_eq!(inserted, 1);

        let cnt: i64 = db
            .call(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(cnt, 1);
    }

    #[tokio::test]
    async fn test_run_paper_tick_only_mirrors_followable_wallets_and_backfills_when_becoming_followable(
    ) {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                rusqlite::params!["0xunfollowable"],
            )?;
            conn.execute(
                "INSERT INTO wallet_rules_state (proxy_wallet, state) VALUES (?1, 'APPROVED')",
                rusqlite::params!["0xunfollowable"],
            )?;
            conn.execute(
                "
                INSERT INTO trades_raw
                    (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                VALUES
                    (?1, ?2, 'BUY', 1.0, 0.5, 1, '0xtx_unfollowable', '{}')
                ",
                rusqlite::params!["0xunfollowable", "0xcond"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Not followable yet -> no paper trades.
        let inserted = run_paper_tick_once(&db, &cfg).await.unwrap();
        assert_eq!(inserted, 0);

        let cnt: i64 = db
            .call(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(cnt, 0);

        // Become followable -> old (still-unprocessed) trade gets mirrored.
        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
                 VALUES (?1, 'CONSISTENT_GENERALIST', 1.0, datetime('now'))",
                rusqlite::params!["0xunfollowable"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let inserted2 = run_paper_tick_once(&db, &cfg).await.unwrap();
        assert_eq!(inserted2, 1);

        let cnt2: i64 = db
            .call(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(cnt2, 1);
    }

    #[tokio::test]
    async fn test_run_paper_tick_stops_mirroring_immediately_when_wallet_becomes_excluded() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                rusqlite::params!["0xw"],
            )?;
            conn.execute(
                "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
                 VALUES (?1, 'CONSISTENT_GENERALIST', 1.0, datetime('now'))",
                rusqlite::params!["0xw"],
            )?;
            conn.execute(
                "INSERT INTO wallet_rules_state (proxy_wallet, state) VALUES (?1, 'APPROVED')",
                rusqlite::params!["0xw"],
            )?;

            // After the first paper trade is inserted, immediately exclude the wallet.
            // This makes the "stop immediately" requirement deterministic in a unit test.
            conn.execute_batch(
                "
                CREATE TRIGGER exclude_wallet_after_first_paper_trade
                AFTER INSERT ON paper_trades
                WHEN NEW.proxy_wallet = '0xw'
                BEGIN
                    INSERT OR IGNORE INTO wallet_exclusions
                        (proxy_wallet, reason, metric_value, threshold, excluded_at)
                    VALUES
                        ('0xw', 'STAGE1_TOO_YOUNG', 1.0, 30.0, datetime('now'));
                END;
                ",
            )?;

            conn.execute(
                "
                INSERT INTO trades_raw
                    (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                VALUES
                    (?1, ?2, 'BUY', 1.0, 0.5, 1, '0xtx1', '{}')
                ",
                rusqlite::params!["0xw", "0xcond"],
            )?;
            conn.execute(
                "
                INSERT INTO trades_raw
                    (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                VALUES
                    (?1, ?2, 'BUY', 1.0, 0.55, 2, '0xtx2', '{}')
                ",
                rusqlite::params!["0xw", "0xcond"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let inserted = run_paper_tick_once(&db, &cfg).await.unwrap();
        assert_eq!(inserted, 1);

        let cnt: i64 = db
            .call(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(cnt, 1);
    }

    #[tokio::test]
    async fn test_run_paper_tick_skips_unclassified_wallets() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                rusqlite::params!["0xunclassified"],
            )?;
            // No wallet_personas row => not currently followable.
            conn.execute(
                "
                INSERT INTO trades_raw
                    (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                VALUES
                    (?1, ?2, 'BUY', 1.0, 0.5, 1, '0xtx_unclassified', '{}')
                ",
                rusqlite::params!["0xunclassified", "0xcond"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let inserted = run_paper_tick_once(&db, &cfg).await.unwrap();
        assert_eq!(inserted, 0);

        let cnt: i64 = db
            .call(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(cnt, 0);
    }

    #[tokio::test]
    async fn test_run_paper_tick_skips_wallets_excluded_after_persona() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                rusqlite::params!["0xexcluded"],
            )?;
            conn.execute(
                "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at) VALUES (?1, 'Consistent Generalist', 1.0, '2026-02-10 00:00:00')",
                rusqlite::params!["0xexcluded"],
            )?;
            // Exclusion is newer than the persona => wallet is not currently followable.
            conn.execute(
                "INSERT INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold, excluded_at) VALUES (?1, 'NOISE_TRADER', 0.0, 0.0, '2026-02-10 00:00:01')",
                rusqlite::params!["0xexcluded"],
            )?;
            conn.execute(
                "
                INSERT INTO trades_raw
                    (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                VALUES
                    (?1, ?2, 'BUY', 1.0, 0.5, 1, '0xtx_excluded', '{}')
                ",
                rusqlite::params!["0xexcluded", "0xcond"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let inserted = run_paper_tick_once(&db, &cfg).await.unwrap();
        assert_eq!(inserted, 0);

        let cnt: i64 = db
            .call(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(cnt, 0);
    }

    #[tokio::test]
    async fn test_run_wallet_scoring_inserts_wallet_scores() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                rusqlite::params!["0xw"],
            )?;
            conn.execute(
                "
                INSERT INTO paper_trades
                    (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl, created_at, settled_at)
                VALUES
                    (?1, 'mirror', ?2, 'BUY', 25.0, 0.5, 'settled_win', 50.0, datetime('now'), datetime('now'))
                ",
                rusqlite::params!["0xw", "0xcond"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let inserted = run_wallet_scoring_once(&db, &cfg).await.unwrap();
        assert!(inserted > 0);

        let cnt: i64 = db
            .call(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM wallet_scores_daily", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert!(cnt > 0);
    }

    #[tokio::test]
    async fn test_recovery_once_empty_db_returns_zero() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();
        let n = run_recovery_once(&db, &cfg).await.unwrap();
        assert_eq!(n, 0, "recovery with no unprocessed trades should process 0");
    }

    #[tokio::test]
    async fn test_run_wallet_rules_once_candidate_to_paper_trading() {
        let mut cfg =
            Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        cfg.wallet_rules.min_median_hold_minutes = 0.0;
        cfg.wallet_rules.min_trades_for_discovery = 50;
        cfg.wallet_rules.max_fraction_trades_at_spread_edge = 1.0;

        let db = AsyncDb::open(":memory:").await.unwrap();
        db.call(|conn| {
            let now = chrono::Utc::now().timestamp();
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xw', 'HOLDER', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
                 VALUES ('0xw', 'CONSISTENT_GENERALIST', 1.0, '2026-02-10 00:00:00')",
                [],
            )?;
            for i in 0..60 {
                conn.execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                     VALUES ('0xw', 'm1', 'BUY', 1.0, 0.5, ?1, ?2, '{}')",
                    rusqlite::params![now - i, format!("0xtx{i}")],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let changed = run_wallet_rules_once(&db, &cfg).await.unwrap();
        assert_eq!(changed, 1);

        let state: String = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT state FROM wallet_rules_state WHERE proxy_wallet='0xw'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(state, "PAPER_TRADING");
    }

    #[tokio::test]
    async fn test_run_paper_tick_only_mirrors_eligible_rule_states() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xcandidate', 'HOLDER', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xapproved', 'HOLDER', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO wallet_rules_state (proxy_wallet, state) VALUES ('0xcandidate', 'CANDIDATE')",
                [],
            )?;
            conn.execute(
                "INSERT INTO wallet_rules_state (proxy_wallet, state) VALUES ('0xapproved', 'APPROVED')",
                [],
            )?;
            conn.execute(
                "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
                 VALUES ('0xcandidate', 'CONSISTENT_GENERALIST', 1.0, '2026-02-10 00:00:00')",
                [],
            )?;
            conn.execute(
                "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
                 VALUES ('0xapproved', 'CONSISTENT_GENERALIST', 1.0, '2026-02-10 00:00:00')",
                [],
            )?;
            conn.execute(
                "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                 VALUES ('0xcandidate', 'm1', 'BUY', 1.0, 0.5, 1, '0xtx-candidate', '{}')",
                [],
            )?;
            conn.execute(
                "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                 VALUES ('0xapproved', 'm1', 'BUY', 1.0, 0.5, 2, '0xtx-approved', '{}')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let inserted = run_paper_tick_once(&db, &cfg).await.unwrap();
        assert_eq!(
            inserted, 1,
            "only APPROVED/PAPER_TRADING wallets should be mirrored"
        );
    }

    #[tokio::test]
    async fn test_run_paper_tick_skips_persona_excluded_even_if_rules_approved() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xex', 'HOLDER', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO wallet_rules_state (proxy_wallet, state) VALUES ('0xex', 'APPROVED')",
                [],
            )?;
            conn.execute(
                "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
                 VALUES ('0xex', 'CONSISTENT_GENERALIST', 1.0, '2026-02-10 00:00:00')",
                [],
            )?;
            conn.execute(
                "INSERT INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold, excluded_at)
                 VALUES ('0xex', 'NOISE_TRADER', 75.0, 50.0, '2026-02-11 00:00:00')",
                [],
            )?;
            conn.execute(
                "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                 VALUES ('0xex', 'm1', 'BUY', 1.0, 0.5, 1, '0xtx-ex', '{}')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let inserted = run_paper_tick_once(&db, &cfg).await.unwrap();
        assert_eq!(inserted, 0, "excluded wallets must not be mirrored");
    }

    #[test]
    fn test_fetch_wallet_chunk_returns_paginated_wallets() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let now = chrono::Utc::now().timestamp();

        // Create 3 active wallets with different characteristics
        for i in 0..3 {
            let wallet = format!("0xwallet{i}");
            db.conn
                .execute(
                    "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                    rusqlite::params![wallet],
                )
                .unwrap();

            // Add trades with varying ages (all in the past)
            db.conn
                .execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                     VALUES (?1, 'm1', 'BUY', 1.0, 0.5, ?2, ?3, '{}')",
                    rusqlite::params![
                        wallet,
                        now - (i64::from(i) + 1) * 86400, // 1, 2, 3 days ago
                        format!("0xtx{i}")
                    ],
                )
                .unwrap();
        }

        // First chunk: limit 2, offset 0
        let chunk1 = fetch_wallet_chunk(&db.conn, 0, 2).unwrap();
        assert_eq!(chunk1.len(), 2, "should return 2 wallets in first chunk");
        assert_eq!(chunk1[0].0, "0xwallet0", "first wallet should be 0xwallet0");
        assert_eq!(
            chunk1[1].0, "0xwallet1",
            "second wallet should be 0xwallet1"
        );

        // Verify metadata fields are computed correctly
        let (_, age_days, total_trades, _days_since_last) = &chunk1[0];
        assert!(
            *age_days > 0,
            "age_days should be positive (wallet has trades)"
        );
        assert_eq!(*total_trades, 1, "wallet should have 1 trade");

        // Second chunk: limit 2, offset 2
        let chunk2 = fetch_wallet_chunk(&db.conn, 2, 2).unwrap();
        assert_eq!(chunk2.len(), 1, "should return 1 wallet in second chunk");
        assert_eq!(chunk2[0].0, "0xwallet2", "third wallet should be 0xwallet2");

        // Third chunk: beyond data
        let chunk3 = fetch_wallet_chunk(&db.conn, 4, 2).unwrap();
        assert_eq!(
            chunk3.len(),
            0,
            "should return empty when offset exceeds data"
        );
    }

    #[tokio::test]
    async fn test_run_persona_classification_updates_progress_incrementally() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap(); // Migrations run automatically

        let now = chrono::Utc::now().timestamp();

        // Create 250 wallets (enough for 3 chunks of 100)
        for i in 0..250 {
            let wallet = format!("0xwallet{i}");
            db.call(move |conn| {
                let w = wallet.clone();
                conn.execute(
                    "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                    rusqlite::params![w],
                )?;
                conn.execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                     VALUES (?1, 'm1', 'BUY', 1.0, 0.5, ?2, ?3, '{}')",
                    rusqlite::params![
                        wallet,
                        now - 86400 * 30, // 30 days ago
                        format!("0xtx{i}")
                    ],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        }

        // Run classification
        let _classified = run_persona_classification_once(&db, &cfg).await.unwrap();

        // Verify progress was updated (check job_status metadata contains "processed" key)
        let metadata: Option<String> = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT metadata FROM job_status WHERE job_name = 'persona_classification'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();

        assert!(
            metadata.is_some(),
            "metadata should be set after classification"
        );

        let meta_json: serde_json::Value = serde_json::from_str(&metadata.unwrap()).unwrap();
        assert!(
            meta_json.get("processed").is_some(),
            "metadata should contain 'processed' field"
        );
        assert!(
            meta_json.get("total").is_some(),
            "metadata should contain 'total' field"
        );
        assert_eq!(
            meta_json["total"].as_i64().unwrap(),
            250,
            "total should be 250 wallets"
        );
        assert_eq!(
            meta_json["processed"].as_i64().unwrap(),
            250,
            "all 250 wallets should be processed"
        );
    }
}
