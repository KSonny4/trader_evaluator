use anyhow::Result;
use common::config::Config;
use common::db::AsyncDb;
use common::polymarket::GammaFilter;
#[cfg(test)]
use common::types::{ApiHolderResponse, ApiTrade, GammaMarket};

use crate::market_scoring::{rank_markets, MarketCandidate};
use crate::paper_trading::{is_crypto_15m_market, mirror_trade_to_paper, Side};
use crate::persona_classification::{classify_wallet, stage1_filter, PersonaConfig, Stage1Config};
use crate::wallet_discovery::{discover_wallets_for_market, HolderWallet, TradeWallet};
use crate::wallet_features::compute_wallet_features;
use crate::wallet_rules_engine::{
    evaluate_discovery, evaluate_live, evaluate_paper, read_state, record_event,
    style_snapshot_from_features, write_state, WalletRuleState,
};
use crate::wallet_scoring::{compute_wscore, WScoreWeights, WalletScoreInput};

use super::fetcher_traits::*;

pub async fn run_paper_tick_once(db: &AsyncDb, cfg: &Config) -> Result<u64> {
    // Read unprocessed trades from DB.
    type TradeRow = (
        i64,
        String,
        String,
        Option<String>,
        f64,
        Option<String>,
        Option<i32>,
    );
    let rows: Vec<TradeRow> = db
        .call_named("paper_tick.select_unprocessed_trades", |conn| {
            let mut stmt = conn.prepare(
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

    let today = chrono::Utc::now().date_naive().to_string();

    let w = WScoreWeights {
        edge_weight: cfg.wallet_scoring.edge_weight,
        consistency_weight: cfg.wallet_scoring.consistency_weight,
    };

    let windows_days = cfg.wallet_scoring.windows_days.clone();
    let bankroll = cfg.risk.paper_bankroll_usdc;

    // Batch read: fetch all (wallet, window) PnL values in one db.call().
    let windows_c = windows_days.clone();
    let pnl_data: Vec<(String, i64, f64)> = db
        .call_named("wallet_scoring.read_pnl_batch", move |conn| {
            let mut stmt = conn.prepare(
                "
                SELECT proxy_wallet
                FROM wallets
                WHERE is_active = 1
                ORDER BY discovered_at DESC
                LIMIT 500
                ",
            )?;
            let wallets: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            let mut results = Vec::new();
            let mut pnl_stmt = conn.prepare(
                "SELECT SUM(pnl) FROM paper_trades WHERE proxy_wallet = ?1 AND status != 'open' AND created_at >= datetime('now', ?2)",
            )?;

            for wallet in &wallets {
                for &wd in &windows_c {
                    let window = format!("-{wd} days");
                    let pnl: Option<f64> = pnl_stmt.query_row(
                        rusqlite::params![wallet, window],
                        |row| row.get(0),
                    )?;
                    results.push((wallet.clone(), i64::from(wd), pnl.unwrap_or(0.0)));
                }
            }
            Ok(results)
        })
        .await?;

    // Compute scores in Rust (no DB needed).
    let mut score_rows = Vec::with_capacity(pnl_data.len());
    for (wallet, window_days, pnl) in &pnl_data {
        let roi_pct = if bankroll > 0.0 {
            100.0 * pnl / bankroll
        } else {
            0.0
        };
        let input = WalletScoreInput {
            paper_roi_pct: roi_pct,
            daily_return_stdev_pct: 0.0,
            hit_rate: 0.50, // TODO: calculate real hit rate from DB
        };
        let wscore = compute_wscore(&input, &w);
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

    Ok(inserted)
}

pub async fn run_market_scoring_once<P: GammaMarketsPager + Sync>(
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

            page_db_rows.push(MarketDbRow {
                condition_id: condition_id.clone(),
                title: title.clone(),
                slug: m.slug.clone(),
                description: m.description.clone(),
                end_date: m.end_date.clone(),
                liquidity,
                volume: volume_24h,
                category: m.category.clone(),
                event_slug: m.event_slug.clone(),
            });

            page_candidates.push(MarketCandidate {
                condition_id,
                title,
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

        all.extend(page_candidates);

        offset = offset.saturating_add(limit);
        if page_len < limit as usize {
            break;
        }
    }

    let ranked = rank_markets(all, cfg.market_scoring.top_n_markets);

    let today = chrono::Utc::now().date_naive().to_string();
    let ranked_data: Vec<(String, f64, i64)> = ranked
        .iter()
        .enumerate()
        .map(|(i, sm)| (sm.market.condition_id.clone(), sm.mscore, (i + 1) as i64))
        .collect();

    let inserted: u64 = db
        .call_named("market_scoring.upsert_ranked_scores", move |conn| {
            let tx = conn.transaction()?;
            let mut ins = 0_u64;
            for (condition_id, mscore, rank) in ranked_data {
                let changed = tx.execute(
                    "
                    INSERT INTO market_scores_daily
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
            tx.commit()?;
            Ok(ins)
        })
        .await?;

    metrics::counter!("evaluator_markets_scored_total").increment(inserted);
    Ok(inserted)
}

pub async fn run_wallet_discovery_once<H: HoldersFetcher + Sync, T: MarketTradesFetcher + Sync>(
    db: &AsyncDb,
    holders: &H,
    trades: &T,
    cfg: &Config,
) -> Result<u64> {
    let top_n_markets = cfg.market_scoring.top_n_markets as i64;
    let markets: Vec<String> = db
        .call_named("wallet_discovery.select_top_markets", move |conn| {
            let mut stmt = conn.prepare(
                "
                SELECT condition_id
                FROM market_scores_daily
                WHERE score_date = date('now')
                ORDER BY rank ASC
                LIMIT ?1
                ",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![top_n_markets], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;

    let mut inserted = 0_u64;
    for condition_id in markets {
        let (holder_resp, _raw_h) = holders
            .fetch_holders(
                &condition_id,
                cfg.wallet_discovery.holders_per_market as u32,
            )
            .await?;

        let (market_trades, _raw_t) = trades
            .fetch_market_trades_page(&condition_id, 200, 0)
            .await?;

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
            .take(cfg.wallet_discovery.max_wallets_per_market)
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
    Ok(inserted)
}

/// Run Stage 2 persona classification for all watchlist wallets that pass Stage 1.
/// Returns the number of wallets that received a classification (followable or excluded).
pub async fn run_persona_classification_once(db: &AsyncDb, cfg: &Config) -> Result<u64> {
    let now_epoch = chrono::Utc::now().timestamp();
    let window_days = 30_u32;
    let persona_config = PersonaConfig::from_personas(&cfg.personas);
    let stage1_config = Stage1Config {
        min_wallet_age_days: cfg.personas.stage1_min_wallet_age_days,
        min_total_trades: cfg.personas.stage1_min_total_trades,
        max_inactive_days: cfg.personas.stage1_max_inactive_days,
    };

    let classified: u64 = db
        .call_named("persona_classification.classify_batch", move |conn| {
            let wallets: Vec<(String, u32, u32, u32)> = conn
                .prepare(
                    "
                    SELECT w.proxy_wallet,
                        CAST((julianday('now') - julianday(w.discovered_at)) AS INTEGER) AS age_days,
                        (SELECT COUNT(*) FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) AS total_trades,
                        (SELECT CAST((julianday('now') - julianday(datetime(MAX(tr.timestamp), 'unixepoch'))) AS INTEGER)
                         FROM trades_raw tr WHERE tr.proxy_wallet = w.proxy_wallet) AS days_since_last
                    FROM wallets w
                    WHERE w.is_active = 1
                    ",
                )?
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1).unwrap_or(0).max(0) as u32,
                        row.get::<_, i64>(2).unwrap_or(0).max(0) as u32,
                        row.get::<_, Option<i64>>(3)?
                            .unwrap_or(i64::MAX)
                            .min(i64::from(i32::MAX))
                            .max(0) as u32,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            let mut count = 0_u64;
            for (proxy_wallet, wallet_age_days, total_trades, days_since_last) in wallets {
                if let Some(reason) = stage1_filter(
                    wallet_age_days,
                    total_trades,
                    days_since_last,
                    &stage1_config,
                ) {
                    crate::persona_classification::record_exclusion(conn, &proxy_wallet, &reason)?;
                    count += 1;
                    continue;
                }

                let Ok(features) = compute_wallet_features(
                    conn,
                    &proxy_wallet,
                    window_days,
                    now_epoch,
                ) else {
                    tracing::warn!(
                        proxy_wallet = %proxy_wallet,
                        "persona classification skipped: compute_wallet_features failed"
                    );
                    continue;
                };

                match classify_wallet(conn, &features, wallet_age_days, &persona_config) {
                    Ok(result) => {
                        if !matches!(result, crate::persona_classification::ClassificationResult::Unclassified) {
                            count += 1;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            proxy_wallet = %proxy_wallet,
                            error = %e,
                            "persona classification skipped: classify_wallet failed"
                        );
                    }
                }
            }
            Ok(count)
        })
        .await?;

    metrics::gauge!("evaluator_persona_classifications_run").set(classified as f64);
    Ok(classified)
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

#[cfg(test)]
mod tests {
    use super::*;

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
                neg_risk: None,
            },
        ];

        let pager = FakeGammaPager::new(vec![(markets, br#"[{"page":1}]"#.to_vec())]);
        let inserted = run_market_scoring_once(&db, &pager, &cfg).await.unwrap();
        assert!(inserted > 0);

        let (cnt_scores, cnt_markets): (i64, i64) = db
            .call(|conn| {
                let cs = conn.query_row("SELECT COUNT(*) FROM market_scores_daily", [], |row| {
                    row.get(0)
                })?;
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

    #[tokio::test]
    async fn test_run_wallet_discovery_inserts_wallets() {
        let cfg = Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.9, 1)",
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
    async fn test_run_wallet_discovery_respects_top_n_markets() {
        let mut cfg =
            Config::from_toml_str(include_str!("../../../../config/default.toml")).unwrap();
        cfg.market_scoring.top_n_markets = 1;
        cfg.wallet_discovery.min_total_trades = 1;

        let db = AsyncDb::open(":memory:").await.unwrap();

        db.call(|conn| {
            // Three markets scored today, with ranks 1..3.
            conn.execute(
                "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.9, 1)",
                rusqlite::params!["0xcond1"],
            )?;
            conn.execute(
                "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.8, 2)",
                rusqlite::params!["0xcond2"],
            )?;
            conn.execute(
                "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.7, 3)",
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

        // top_n_markets=1 => only rank=1 market should be processed (holder + trader).
        assert_eq!(inserted, 2);

        let cnt_wallets: i64 = db
            .call(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM wallets", [], |row| row.get(0))?))
            .await
            .unwrap();
        assert_eq!(cnt_wallets, 2);

        let cnt_wallets_other_markets: i64 = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM wallets WHERE discovered_market IN ('0xcond2', '0xcond3')",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(cnt_wallets_other_markets, 0);
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
}
