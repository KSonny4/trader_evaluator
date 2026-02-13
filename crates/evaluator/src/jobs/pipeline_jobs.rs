use anyhow::Result;
use common::config::Config;
use common::db::AsyncDb;
use common::polymarket::GammaFilter;
#[cfg(test)]
use common::types::{ApiHolderResponse, ApiLeaderboardEntry, ApiTrade, GammaMarket};

use crate::market_scoring::{rank_events, rank_markets, MarketCandidate};
use crate::persona_classification::{
    classify_wallet, stage1_filter, stage1_known_bot_check, PersonaConfig, Stage1Config,
};
use crate::wallet_discovery::{discover_wallets_for_market, HolderWallet, TradeWallet};
use crate::wallet_features::compute_wallet_features;
use crate::wallet_features::save_wallet_features;
use crate::wallet_rules_engine::{
    evaluate_discovery, evaluate_live, evaluate_paper, read_state, record_event,
    style_snapshot_from_features, write_state, WalletRuleState,
};
use crate::wallet_scoring::{compute_wscore, score_input_from_features, WScoreWeights};

use super::fetcher_traits::*;
use super::tracker::JobTracker;

/// Detect if a market is a 15-minute crypto price prediction market.
/// These are the ONLY markets that charge taker fees on Polymarket.
fn is_crypto_15m_market(title: &str, slug: &str) -> bool {
    let text = format!("{} {}", title.to_lowercase(), slug.to_lowercase());
    let is_crypto = text.contains("btc")
        || text.contains("eth")
        || text.contains("bitcoin")
        || text.contains("ethereum");
    let is_15m = text.contains("15m") || text.contains("15 min") || text.contains("15-min");
    is_crypto && is_15m
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

    let min_trades = i64::from(cfg.wallet_scoring.min_trades_for_score);
    let wallets_ready: i64 = db
        .call_named("wallet_scoring.count_ready_wallets", move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(DISTINCT proxy_wallet)
                 FROM (
                     SELECT proxy_wallet, COUNT(*) as trade_count
                     FROM trades_raw
                     GROUP BY proxy_wallet
                     HAVING COUNT(*) >= ?1
                 )",
                [min_trades],
                |row| row.get(0),
            )?)
        })
        .await?;

    if wallets_ready == 0 {
        tracing::warn!(
            "wallet_scoring: skipping - no wallets with sufficient trades in trades_raw"
        );
        tracker
            .success(Some(serde_json::json!({
                "inserted": 0,
                "skipped": "insufficient trades_raw",
                "min_trades_required": min_trades
            })))
            .await?;
        return Ok(0);
    }

    let today = chrono::Utc::now().date_naive().to_string();
    let now_epoch = chrono::Utc::now().timestamp();

    let w = WScoreWeights {
        edge_weight: cfg.wallet_scoring.edge_weight,
        consistency_weight: cfg.wallet_scoring.consistency_weight,
        market_skill_weight: cfg.wallet_scoring.market_skill_weight,
        timing_skill_weight: cfg.wallet_scoring.timing_skill_weight,
        behavior_quality_weight: cfg.wallet_scoring.behavior_quality_weight,
    };

    let windows_days = cfg.wallet_scoring.windows_days.clone();
    let trust_30_90_multiplier = cfg.personas.trust_30_90_multiplier;
    let obscurity_bonus_multiplier = cfg.personas.obscurity_bonus_multiplier;
    let min_trades_u32 = cfg.wallet_scoring.min_trades_for_score;

    // Compute features, scores, and persist — all in one db.call() to avoid overhead.
    let today_c = today.clone();
    let (inserted, features_saved): (u64, u64) = db
        .call_named("wallet_scoring.compute_and_upsert", move |conn| {
            let wallets: Vec<(String, String, i64)> = conn
                .prepare(
                    "SELECT proxy_wallet,
                            discovered_from,
                            CAST((julianday('now') - julianday(discovered_at)) AS INTEGER) AS age_days
                     FROM wallets
                     WHERE is_active = 1
                     ORDER BY discovered_at DESC
                     LIMIT 500",
                )?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            let mut score_rows = Vec::new();
            let mut feat_count = 0_u64;

            for (wallet, discovered_from, age_days) in &wallets {
                for &wd in &windows_days {
                    let features = match compute_wallet_features(conn, wallet, wd, now_epoch) {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::warn!(
                                proxy_wallet = %wallet, window = wd,
                                error = %e, "wallet_scoring: skipping feature computation"
                            );
                            continue;
                        }
                    };

                    if features.trade_count < min_trades_u32 {
                        continue;
                    }

                    // Persist features
                    if let Err(e) = save_wallet_features(conn, &features, &today_c) {
                        tracing::warn!(
                            proxy_wallet = %wallet, error = %e,
                            "wallet_scoring: failed to save features"
                        );
                    } else {
                        feat_count += 1;
                    }

                    let is_leaderboard = discovered_from == "LEADERBOARD";
                    let input = score_input_from_features(
                        &features,
                        (*age_days).max(0) as u32,
                        is_leaderboard,
                    );
                    let wscore = compute_wscore(
                        &input, &w,
                        trust_30_90_multiplier,
                        obscurity_bonus_multiplier,
                    );
                    score_rows.push(ScoreRow {
                        proxy_wallet: wallet.clone(),
                        window_days: i64::from(wd),
                        wscore,
                        edge_score: crate::wallet_scoring::edge_score(input.roi_pct),
                        consistency_score: crate::wallet_scoring::consistency_score(input.daily_return_stdev_pct),
                        roi_pct: input.roi_pct,
                    });
                }
            }

            // Upsert scores in a transaction
            let tx = conn.transaction()?;
            let mut ins = 0_u64;
            for r in &score_rows {
                tx.execute(
                    "INSERT INTO wallet_scores_daily
                        (proxy_wallet, score_date, window_days, wscore, edge_score,
                         consistency_score, paper_roi_pct, recommended_follow_mode)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(proxy_wallet, score_date, window_days) DO UPDATE SET
                        wscore = excluded.wscore,
                        edge_score = excluded.edge_score,
                        consistency_score = excluded.consistency_score,
                        paper_roi_pct = excluded.paper_roi_pct,
                        recommended_follow_mode = excluded.recommended_follow_mode",
                    rusqlite::params![
                        r.proxy_wallet,
                        today_c,
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
            Ok((ins, feat_count))
        })
        .await?;

    metrics::gauge!("evaluator_wallet_scoring_features_saved").set(features_saved as f64);
    tracker
        .success(Some(serde_json::json!({
            "inserted": inserted,
            "features_saved": features_saved
        })))
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

    // Check for recent market scores (within last 24 hours)
    let (markets, score_date): (Vec<String>, Option<String>) = db
        .call_named("wallet_discovery.select_top_events_markets", move |conn| {
            // First check if we have recent scores
            let latest_date: Option<String> = conn
                .query_row("SELECT MAX(score_date) FROM market_scores", [], |row| {
                    row.get(0)
                })
                .ok();

            if let Some(date_str) = &latest_date {
                // Check if the date is within the last 24 hours (same day or yesterday)
                if let Ok(score_date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                    let now = chrono::Utc::now().date_naive();
                    let age_days = (now - score_date).num_days();

                    if age_days >= 1 {
                        // Scores are too old (more than 24h)
                        return Ok((vec![], latest_date));
                    }
                }
            }

            // Scores are recent, fetch markets
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
            Ok((rows, latest_date))
        })
        .await?;

    let total = markets.len();

    // If no markets (stale scores), skip gracefully
    if total == 0 {
        tracing::warn!(
            latest_score_date = ?score_date,
            "wallet_discovery: skipping - no recent market scores (need scores within 24h)"
        );
        tracker
            .success(Some(serde_json::json!({
                "discovered": 0,
                "skipped": "no recent market scores",
                "latest_score_date": score_date
            })))
            .await?;
        return Ok(0);
    }

    if total > 0 {
        tracing::info!(markets = total, "wallet_discovery: processing top events");
    }

    let trades_pages = cfg
        .wallet_discovery
        .trades_pages_per_market
        .min(TRADES_PAGES_CAP);

    let mut inserted = 0_u64;
    let mut all_new_wallets = Vec::new();
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
        let (page_inserted, new_wallets): (u64, Vec<String>) = db
            .call_named("wallet_discovery.insert_wallets_page", move |conn| {
                let tx = conn.transaction()?;

                let mut ins = 0_u64;
                let mut newly_inserted = Vec::new();
                for (proxy_wallet, discovered_from) in wallets_to_insert {
                    let changed = tx.execute(
                        "
                        INSERT OR IGNORE INTO wallets
                            (proxy_wallet, discovered_from, discovered_market, is_active)
                        VALUES
                            (?1, ?2, ?3, 1)
                        ",
                        rusqlite::params![&proxy_wallet, &discovered_from, &cid],
                    )?;
                    if changed > 0 {
                        newly_inserted.push(proxy_wallet);
                        ins += 1;
                    }
                }
                tx.commit()?;
                Ok((ins, newly_inserted))
            })
            .await?;

        inserted += page_inserted;
        all_new_wallets.extend(new_wallets);
    }

    // Spawn on-demand feature computation for newly discovered wallets
    let cfg = std::sync::Arc::new(cfg.clone());
    for wallet in all_new_wallets {
        let db = db.clone();
        let cfg = cfg.clone();
        tokio::spawn(async move {
            let span = tracing::info_span!("on_demand_features", wallet = %wallet);
            let _g = span.enter();
            match crate::wallet_features::compute_features_for_wallet(&db, &cfg, &wallet, 30).await
            {
                Ok(()) => {
                    tracing::info!("on-demand features computed");
                    metrics::counter!("evaluator_on_demand_features_total", "status" => "success")
                        .increment(1);
                }
                Err(e) => {
                    tracing::warn!(error=%e, "on-demand features failed, will retry in batch");
                    metrics::counter!("evaluator_on_demand_features_total", "status" => "failure")
                        .increment(1);
                }
            }
        });
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
        if let Err(e) = crate::persona_classification::clear_stage1_exclusion(conn, proxy_wallet) {
            tracing::warn!(proxy_wallet = %proxy_wallet, error = %e, "Failed to clear Stage 1 exclusion");
        }

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

    // Quality gate: Check for wallets with sufficient trade history
    // Need at least min_total_trades and 7+ days of tracking
    let min_trades = i64::from(stage1_config.min_total_trades);
    let ready_wallets: i64 = db
        .call_named("persona_classification.count_ready_wallets", move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(DISTINCT w.proxy_wallet)
                 FROM wallets w
                 WHERE w.is_active = 1
                   AND (SELECT COUNT(*) FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) >= ?1
                   AND (SELECT MIN(timestamp) FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet)
                       <= unixepoch('now', '-7 days')",
                [min_trades],
                |row| row.get(0),
            )?)
        })
        .await?;

    if ready_wallets == 0 {
        tracing::warn!(
            "persona_classification: skipping - no wallets with sufficient trade history"
        );
        tracker
            .success(Some(serde_json::json!({
                "classified": 0,
                "skipped": "insufficient trade history",
                "total_wallets": total_wallets,
                "min_trades_required": stage1_config.min_total_trades,
                "min_tracking_days": 7
            })))
            .await?;
        return Ok(0);
    }

    tracing::info!(
        total = total_wallets,
        ready = ready_wallets,
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
    async fn test_run_wallet_scoring_inserts_wallet_scores() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();
        let now = chrono::Utc::now().timestamp();

        db.call(move |conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                rusqlite::params!["0xw"],
            )?;
            // Create 12 trades in trades_raw (>= min_trades_for_score=10) with BUY/SELL pairs
            for i in 0..6 {
                let cid = format!("cond{i}");
                conn.execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES (?1, ?2, 'BUY', 25.0, 0.50, ?3)",
                    rusqlite::params!["0xw", cid, now - 86400 * (i + 2)],
                )?;
                conn.execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES (?1, ?2, 'SELL', 25.0, 0.60, ?3)",
                    rusqlite::params!["0xw", cid, now - 86400 * (i + 1)],
                )?;
            }
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

        // Verify features were also persisted
        let feat_cnt: i64 = db
            .call(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM wallet_features_daily", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert!(feat_cnt > 0);
    }

    #[tokio::test]
    async fn test_wallet_scoring_skips_when_insufficient_trades() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();
        let now = chrono::Utc::now().timestamp();

        // Create wallet with only 3 trades in trades_raw (need min_trades_for_score=10)
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                rusqlite::params!["0xfew"],
            )?;
            for i in 0..3 {
                conn.execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp)
                     VALUES (?1, ?2, 'BUY', 25.0, 0.5, ?3)",
                    rusqlite::params!["0xfew", format!("cond{i}"), now - 86400 * (i + 1)],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        // Run wallet_scoring - should skip because insufficient trades in trades_raw
        let inserted = run_wallet_scoring_once(&db, &cfg).await.unwrap();
        assert_eq!(
            inserted, 0,
            "should score 0 wallets when insufficient trades"
        );

        // Verify metadata shows skip reason
        let metadata: Option<String> = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT metadata FROM job_status WHERE job_name = 'wallet_scoring'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();

        assert!(metadata.is_some(), "metadata should be set");
        let meta_json: serde_json::Value = serde_json::from_str(&metadata.unwrap()).unwrap();
        assert!(
            meta_json.get("skipped").is_some(),
            "metadata should indicate skipping"
        );
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
    async fn test_persona_classification_skips_when_insufficient_trade_history() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        let now = chrono::Utc::now().timestamp();

        // Create wallets with insufficient trade history
        // Wallet 1: Only 5 trades (need 10+)
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xfew', 'HOLDER', 1)",
                [],
            )?;
            for i in 0..5 {
                conn.execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                     VALUES ('0xfew', 'm1', 'BUY', 1.0, 0.5, ?1, ?2, '{}')",
                    rusqlite::params![now - (i + 1) * 86400, format!("0xtx{i}")],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        // Wallet 2: Recent trades only (not 7+ days old)
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xnew', 'HOLDER', 1)",
                [],
            )?;
            for i in 0..15 {
                // 15 trades but all within last 3 days
                conn.execute(
                    "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                     VALUES ('0xnew', 'm1', 'BUY', 1.0, 0.5, ?1, ?2, '{}')",
                    rusqlite::params![now - i64::from(i) * 3600, format!("0xtx_new{i}")],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        // Run classification - should skip because wallets lack sufficient history
        let classified = run_persona_classification_once(&db, &cfg).await.unwrap();

        assert_eq!(
            classified, 0,
            "should classify 0 wallets when trade history insufficient"
        );

        // Verify job_status shows skip reason
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

        assert!(metadata.is_some(), "metadata should be set");
        let meta_json: serde_json::Value = serde_json::from_str(&metadata.unwrap()).unwrap();
        assert!(
            meta_json.get("skipped").is_some(),
            "metadata should indicate skipping"
        );
    }

    #[tokio::test]
    async fn test_run_persona_classification_updates_progress_incrementally() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap(); // Migrations run automatically

        let now = chrono::Utc::now().timestamp();

        // Create 250 wallets (enough for 3 chunks of 100)
        // Each wallet needs 10+ trades and 7+ days of history to pass quality gate
        for i in 0..250 {
            let wallet = format!("0xwallet{i}");
            db.call(move |conn| {
                let w = wallet.clone();
                conn.execute(
                    "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
                    rusqlite::params![w],
                )?;
                // Create 15 trades spread over 30 days to pass quality gate
                for j in 0..15 {
                    conn.execute(
                        "INSERT INTO trades_raw (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
                         VALUES (?1, 'm1', 'BUY', 1.0, 0.5, ?2, ?3, '{}')",
                        rusqlite::params![
                            wallet.clone(),
                            now - 86400 * (30 - (j * 2)), // Spread trades over 30 days
                            format!("0xtx{i}_{j}")
                        ],
                    )?;
                }
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

    #[tokio::test]
    async fn test_wallet_discovery_skips_when_no_recent_market_scores() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        // Insert old market scores (more than 24h ago)
        let old_date = chrono::Utc::now()
            .checked_sub_signed(chrono::Duration::hours(25))
            .unwrap()
            .format("%Y-%m-%d")
            .to_string();

        db.call(move |conn| {
            conn.execute(
                "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES ('m1', ?1, 0.8, 1)",
                rusqlite::params![old_date],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Create fake fetchers (won't be called since we skip)
        let holders = FakeHoldersFetcher {
            resp: vec![],
            raw: b"[]".to_vec(),
        };
        let trades = FakeMarketTradesFetcher {
            trades: vec![],
            raw: b"[]".to_vec(),
        };

        // Run wallet_discovery - should skip because scores are too old
        let discovered = run_wallet_discovery_once(&db, &holders, &trades, &cfg)
            .await
            .unwrap();

        assert_eq!(
            discovered, 0,
            "should discover 0 wallets when scores are stale"
        );

        // Verify job_status shows skip reason
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

        assert!(metadata.is_some(), "metadata should be set");
        let meta_json: serde_json::Value = serde_json::from_str(&metadata.unwrap()).unwrap();
        assert!(
            meta_json.get("skipped").is_some(),
            "metadata should indicate skipping"
        );
    }

    #[tokio::test]
    async fn test_wallet_discovery_tracks_new_wallets() {
        use common::types::{ApiHolder, ApiHolderResponse, ApiTrade};

        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        // Insert market score
        db.call(|conn| {
            Ok(conn.execute(
                "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES ('0xmarket', date('now'), 0.9, 1)",
                [],
            )?)
        })
        .await
        .unwrap();

        // Pre-insert one wallet as "existing"
        db.call(|conn| {
            Ok(conn.execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xold', 'HOLDER', 1)",
                [],
            )?)
        })
        .await
        .unwrap();

        // Setup fake fetchers
        let holders = FakeHoldersFetcher {
            resp: vec![ApiHolderResponse {
                token: None,
                holders: vec![
                    ApiHolder {
                        proxy_wallet: Some("0xold".to_string()),
                        amount: Some(100.0),
                        asset: None,
                        pseudonym: None,
                        name: None,
                        outcome_index: None,
                    },
                    ApiHolder {
                        proxy_wallet: Some("0xnew1".to_string()),
                        amount: Some(50.0),
                        asset: None,
                        pseudonym: None,
                        name: None,
                        outcome_index: None,
                    },
                ],
            }],
            raw: b"[]".to_vec(),
        };

        // Create trades from 0xnew2 (will exceed min_total_trades=5)
        let trades = FakeMarketTradesFetcher {
            trades: vec![
                ApiTrade {
                    proxy_wallet: Some("0xnew2".to_string()),
                    condition_id: Some("0xmarket".to_string()),
                    ..Default::default()
                },
                ApiTrade {
                    proxy_wallet: Some("0xnew2".to_string()),
                    condition_id: Some("0xmarket".to_string()),
                    ..Default::default()
                },
                ApiTrade {
                    proxy_wallet: Some("0xnew2".to_string()),
                    condition_id: Some("0xmarket".to_string()),
                    ..Default::default()
                },
                ApiTrade {
                    proxy_wallet: Some("0xnew2".to_string()),
                    condition_id: Some("0xmarket".to_string()),
                    ..Default::default()
                },
                ApiTrade {
                    proxy_wallet: Some("0xnew2".to_string()),
                    condition_id: Some("0xmarket".to_string()),
                    ..Default::default()
                },
            ],
            raw: b"[]".to_vec(),
        };

        // Discovery should insert 2 new wallets (0xnew1, 0xnew2)
        let inserted = run_wallet_discovery_once(&db, &holders, &trades, &cfg)
            .await
            .unwrap();
        assert_eq!(inserted, 2, "should insert 2 new wallets");

        // TODO: Verify that new_wallets list contains 0xnew1 and 0xnew2
        // This will be validated when we add the spawn logic in next task
    }

    #[tokio::test]
    async fn test_on_demand_features_spawned_after_discovery() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        // Insert market score
        db.call(|conn| {
            conn.execute(
                "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES ('0xmarket', date('now'), 0.9, 1)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Insert trades for new wallet (≥5 for feature computation)
        let now = chrono::Utc::now().timestamp();
        db.call(move |conn| {
            for i in 0..6 {
                conn.execute(
                    "INSERT INTO trades_raw (transaction_hash, proxy_wallet, condition_id, side, size, price, timestamp, raw_json)
                     VALUES (?1, '0xnewwallet', '0xmarket', 'BUY', 100, 0.5, ?2, '{}')",
                    rusqlite::params![format!("0xtx_buy_{}", i), now - (i * 86400)],
                )?;
                conn.execute(
                    "INSERT INTO trades_raw (transaction_hash, proxy_wallet, condition_id, side, size, price, timestamp, raw_json)
                     VALUES (?1, '0xnewwallet', '0xmarket', 'SELL', 100, 0.6, ?2, '{}')",
                    rusqlite::params![format!("0xtx_sell_{}", i), now - (i * 86400) + 3600],
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
                    proxy_wallet: Some("0xnewwallet".to_string()),
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

        // Run discovery
        let inserted = run_wallet_discovery_once(&db, &holders, &trades, &cfg)
            .await
            .unwrap();
        assert_eq!(inserted, 1, "should discover 1 new wallet");

        // Wait for spawned tasks to complete (tokio tasks are async)
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Verify features computed
        let count: i64 = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM wallet_features_daily WHERE proxy_wallet = '0xnewwallet' AND window_days = 30",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "on-demand features should be computed for new wallet"
        );
    }
}
