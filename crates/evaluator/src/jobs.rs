use anyhow::Result;
use common::config::Config;
use common::db::Database;
use common::types::{ApiActivity, ApiHolderResponse, ApiPosition, ApiTrade, GammaMarket};

use crate::market_scoring::{rank_markets, MarketCandidate};
use crate::paper_trading::{mirror_trade_to_paper, Side};
use crate::wallet_discovery::{discover_wallets_for_market, HolderWallet, TradeWallet};
use crate::wallet_scoring::{compute_wscore, WScoreWeights, WalletScoreInput};

use common::polymarket::{GammaFilter, PolymarketClient};
use std::time::Instant;

impl GammaMarketsPager for PolymarketClient {
    fn gamma_markets_url(&self, limit: u32, offset: u32) -> String {
        format!(
            "{}/markets?limit={limit}&offset={offset}",
            self.gamma_api_url()
        )
    }

    async fn fetch_gamma_markets_page(
        &self,
        limit: u32,
        offset: u32,
        filter: &GammaFilter,
    ) -> Result<(Vec<GammaMarket>, Vec<u8>)> {
        let start = Instant::now();
        let res = self.fetch_gamma_markets_raw(limit, offset, filter).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "gamma_markets").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "gamma_markets", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "gamma_markets", "status" => "error").increment(1);
                Err(e)
            }
        }
    }
}

impl HoldersFetcher for PolymarketClient {
    fn holders_url(&self, condition_id: &str, limit: u32) -> String {
        format!(
            "{}/holders?market={condition_id}&limit={limit}",
            self.data_api_url()
        )
    }

    async fn fetch_holders(
        &self,
        condition_id: &str,
        limit: u32,
    ) -> Result<(Vec<ApiHolderResponse>, Vec<u8>)> {
        let start = Instant::now();
        let res = self.fetch_holders_raw(condition_id, limit).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "holders").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "holders", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "holders", "status" => "error").increment(1);
                Err(e)
            }
        }
    }
}

impl MarketTradesFetcher for PolymarketClient {
    fn market_trades_url(&self, condition_id: &str, limit: u32, offset: u32) -> String {
        self.trades_url_any(None, Some(condition_id), limit, offset)
    }

    async fn fetch_market_trades_page(
        &self,
        condition_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiTrade>, Vec<u8>)> {
        let start = Instant::now();
        let res = self
            .fetch_trades_raw_any(None, Some(condition_id), limit, offset)
            .await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "market_trades").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "market_trades", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "market_trades", "status" => "error").increment(1);
                Err(e)
            }
        }
    }
}

impl crate::ingestion::TradesPager for PolymarketClient {
    fn trades_url(&self, user: &str, limit: u32, offset: u32) -> String {
        self.trades_url_any(Some(user), None, limit, offset)
    }

    async fn fetch_trades_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiTrade>, Vec<u8>)> {
        let start = Instant::now();
        let res = self
            .fetch_trades_raw_any(Some(user), None, limit, offset)
            .await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "user_trades").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "user_trades", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "user_trades", "status" => "error").increment(1);
                Err(e)
            }
        }
    }
}

impl ActivityPager for PolymarketClient {
    fn activity_url(&self, user: &str, limit: u32, offset: u32) -> String {
        format!(
            "{}/activity?user={user}&limit={limit}&offset={offset}",
            self.data_api_url()
        )
    }

    async fn fetch_activity_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiActivity>, Vec<u8>)> {
        let start = Instant::now();
        let res = self.fetch_activity_raw(user, limit, offset).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "activity").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "activity", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "activity", "status" => "error").increment(1);
                Err(e)
            }
        }
    }
}

impl PositionsPager for PolymarketClient {
    fn positions_url(&self, user: &str, limit: u32, offset: u32) -> String {
        format!(
            "{}/positions?user={user}&limit={limit}&offset={offset}",
            self.data_api_url()
        )
    }

    async fn fetch_positions_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ApiPosition>, Vec<u8>)> {
        let start = Instant::now();
        let res = self.fetch_positions_raw(user, limit, offset).await;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::histogram!("evaluator_api_latency_ms", "endpoint" => "positions").record(ms);
        match res {
            Ok(v) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "positions", "status" => "ok").increment(1);
                Ok(v)
            }
            Err(e) => {
                metrics::counter!("evaluator_api_requests_total", "endpoint" => "positions", "status" => "error").increment(1);
                Err(e)
            }
        }
    }
}

pub async fn run_trades_ingestion_once<P: crate::ingestion::TradesPager + Sync>(
    db: &Database,
    pager: &P,
    limit: u32,
) -> Result<(u64, u64)> {
    let mut stmt = db.conn.prepare(
        r#"
        SELECT proxy_wallet
        FROM wallets
        WHERE is_active = 1
        ORDER BY discovered_at DESC
        LIMIT 500
        "#,
    )?;
    let wallets = stmt.query_map([], |row| row.get::<_, String>(0))?;

    let mut pages = 0_u64;
    let mut inserted = 0_u64;
    for w in wallets {
        let w = w?;
        match crate::ingestion::ingest_trades_for_wallet(db, pager, &w, limit).await {
            Ok((p, ins)) => {
                pages += p;
                inserted += ins;
            }
            Err(e) => {
                tracing::warn!(
                    wallet = %w,
                    error = %e,
                    "trades ingestion failed for wallet; continuing to next"
                );
            }
        }
    }
    metrics::counter!("evaluator_trades_ingested_total").increment(inserted);
    Ok((pages, inserted))
}

pub async fn run_activity_ingestion_once<P: ActivityPager + Sync>(
    db: &Database,
    pager: &P,
    limit: u32,
) -> Result<u64> {
    let mut stmt = db.conn.prepare(
        r#"
        SELECT proxy_wallet
        FROM wallets
        WHERE is_active = 1
        ORDER BY discovered_at DESC
        LIMIT 500
        "#,
    )?;
    let wallets = stmt.query_map([], |row| row.get::<_, String>(0))?;

    let mut inserted = 0_u64;
    for w in wallets {
        let w = w?;
        let fetch_result = pager.fetch_activity_page(&w, limit, 0).await;
        let (events, raw) = match fetch_result {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    wallet = %w,
                    error = %e,
                    "activity ingestion failed for wallet; continuing to next"
                );
                continue;
            }
        };
        let url = pager.activity_url(&w, limit, 0);
        db.conn.execute(
            "INSERT INTO raw_api_responses (api, method, url, response_body) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["data_api", "GET", url, raw],
        )?;

        for e in events {
            let proxy_wallet = match e.proxy_wallet.as_deref() {
                Some(v) if !v.is_empty() => v.to_string(),
                _ => continue,
            };
            let activity_type = match e.activity_type.as_deref() {
                Some(v) if !v.is_empty() => v.to_string(),
                _ => continue,
            };
            let timestamp = e.timestamp.unwrap_or(0);
            let raw_json = serde_json::to_string(&e).unwrap_or_default();
            let changed = db.conn.execute(
                r#"
                INSERT OR IGNORE INTO activity_raw
                    (proxy_wallet, condition_id, activity_type, size, usdc_size, price, side, outcome, outcome_index, timestamp, transaction_hash, raw_json)
                VALUES
                    (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                rusqlite::params![
                    proxy_wallet,
                    e.condition_id,
                    activity_type,
                    e.size.and_then(|s| s.parse::<f64>().ok()),
                    e.usdc_size.and_then(|s| s.parse::<f64>().ok()),
                    e.price.and_then(|s| s.parse::<f64>().ok()),
                    e.side,
                    e.outcome,
                    e.outcome_index,
                    timestamp,
                    e.transaction_hash,
                    raw_json,
                ],
            )?;
            inserted += changed as u64;
        }
    }

    Ok(inserted)
}

pub async fn run_positions_snapshot_once<P: PositionsPager + Sync>(
    db: &Database,
    pager: &P,
    limit: u32,
) -> Result<u64> {
    let mut stmt = db.conn.prepare(
        r#"
        SELECT proxy_wallet
        FROM wallets
        WHERE is_active = 1
        ORDER BY discovered_at DESC
        LIMIT 500
        "#,
    )?;
    let wallets = stmt.query_map([], |row| row.get::<_, String>(0))?;

    let mut inserted = 0_u64;
    for w in wallets {
        let w = w?;
        let fetch_result = pager.fetch_positions_page(&w, limit, 0).await;
        let (positions, raw) = match fetch_result {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    wallet = %w,
                    error = %e,
                    "positions snapshot failed for wallet; continuing to next"
                );
                continue;
            }
        };
        let url = pager.positions_url(&w, limit, 0);
        db.conn.execute(
            "INSERT INTO raw_api_responses (api, method, url, response_body) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["data_api", "GET", url, raw],
        )?;

        for p in positions {
            let proxy_wallet = match p.proxy_wallet.as_deref() {
                Some(v) if !v.is_empty() => v.to_string(),
                _ => continue,
            };
            let condition_id = match p.condition_id.as_deref() {
                Some(v) if !v.is_empty() => v.to_string(),
                _ => continue,
            };
            let size = match p.size.as_deref().and_then(|s| s.parse::<f64>().ok()) {
                Some(v) => v,
                None => continue,
            };
            let raw_json = serde_json::to_string(&p).unwrap_or_default();
            let changed = db.conn.execute(
                r#"
                INSERT INTO positions_snapshots
                    (proxy_wallet, condition_id, asset, size, avg_price, current_value, cash_pnl, percent_pnl, outcome, outcome_index, raw_json)
                VALUES
                    (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                "#,
                rusqlite::params![
                    proxy_wallet,
                    condition_id,
                    p.asset,
                    size,
                    p.avg_price.and_then(|s| s.parse::<f64>().ok()),
                    p.current_value.and_then(|s| s.parse::<f64>().ok()),
                    p.cash_pnl.and_then(|s| s.parse::<f64>().ok()),
                    p.percent_pnl.and_then(|s| s.parse::<f64>().ok()),
                    p.outcome,
                    p.outcome_index,
                    raw_json,
                ],
            )?;
            inserted += changed as u64;
        }
    }

    Ok(inserted)
}

pub async fn run_holders_snapshot_once<H: HoldersFetcher + Sync>(
    db: &Database,
    holders: &H,
    per_market: u32,
) -> Result<u64> {
    let mut stmt = db.conn.prepare(
        r#"
        SELECT condition_id
        FROM market_scores_daily
        WHERE score_date = date('now')
        ORDER BY rank ASC
        LIMIT 20
        "#,
    )?;
    let markets_iter = stmt.query_map([], |row| row.get::<_, String>(0))?;

    let mut inserted = 0_u64;
    for m in markets_iter {
        let condition_id = m?;
        let fetch_result = holders.fetch_holders(&condition_id, per_market).await;
        let (holder_resp, raw_h) = match fetch_result {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    condition_id = %condition_id,
                    error = %e,
                    "holders snapshot failed for market; continuing to next"
                );
                continue;
            }
        };
        let url = holders.holders_url(&condition_id, per_market);
        db.conn.execute(
            "INSERT INTO raw_api_responses (api, method, url, response_body) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["data_api", "GET", url, raw_h],
        )?;

        for r in holder_resp {
            let token = r.token.clone();
            for h in r.holders {
                let Some(proxy_wallet) = h.proxy_wallet else {
                    continue;
                };
                let Some(amount) = h.amount else {
                    continue;
                };
                let changed = db.conn.execute(
                    r#"
                    INSERT INTO holders_snapshots
                        (condition_id, token, proxy_wallet, amount, outcome_index, pseudonym)
                    VALUES
                        (?1, ?2, ?3, ?4, ?5, ?6)
                    "#,
                    rusqlite::params![
                        condition_id,
                        token,
                        proxy_wallet,
                        amount,
                        h.outcome_index,
                        h.pseudonym
                    ],
                )?;
                inserted += changed as u64;
            }
        }
    }

    Ok(inserted)
}

pub fn run_paper_tick_once(db: &Database, cfg: &Config) -> Result<u64> {
    // Mirror only trades we haven't processed yet.
    let mut stmt = db.conn.prepare(
        r#"
        SELECT tr.id, tr.proxy_wallet, tr.condition_id, tr.side, tr.price, tr.outcome, tr.outcome_index
        FROM trades_raw tr
        LEFT JOIN paper_trades pt ON pt.triggered_by_trade_id = tr.id
        WHERE pt.id IS NULL
        ORDER BY tr.id ASC
        LIMIT 500
        "#,
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<i32>>(6)?,
        ))
    })?;

    let mut inserted = 0_u64;
    for r in rows {
        let (trade_id, proxy_wallet, condition_id, side_s, price, outcome, outcome_index) = r?;
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
        )?;

        if decision.inserted {
            inserted += 1;
            metrics::counter!("evaluator_paper_trades_total").increment(1);
        } else if let Some(rule) = decision.reason {
            metrics::counter!("evaluator_risk_violations_total", "rule" => rule).increment(1);
        }
    }

    let pnl: Option<f64> = db.conn.query_row(
        "SELECT SUM(pnl) FROM paper_trades WHERE status != 'open'",
        [],
        |row| row.get(0),
    )?;
    metrics::gauge!("evaluator_paper_pnl").set(pnl.unwrap_or(0.0));

    Ok(inserted)
}

pub fn run_wallet_scoring_once(db: &Database, cfg: &Config) -> Result<u64> {
    let today = chrono::Utc::now().date_naive().to_string();

    let w = WScoreWeights {
        edge_weight: cfg.wallet_scoring.edge_weight,
        consistency_weight: cfg.wallet_scoring.consistency_weight,
    };

    // Minimal MVP scoring: per-wallet settled ROI and a placeholder stdev (0.0).
    let mut stmt = db.conn.prepare(
        r#"
        SELECT proxy_wallet
        FROM wallets
        WHERE is_active = 1
        ORDER BY discovered_at DESC
        LIMIT 500
        "#,
    )?;
    let wallets = stmt.query_map([], |row| row.get::<_, String>(0))?;

    let mut inserted = 0_u64;
    for waddr in wallets {
        let waddr = waddr?;

        for &window_days in &cfg.wallet_scoring.windows_days {
            let window = format!("-{} days", window_days);
            let pnl: Option<f64> = db.conn.query_row(
                "SELECT SUM(pnl) FROM paper_trades WHERE proxy_wallet = ?1 AND status != 'open' AND created_at >= datetime('now', ?2)",
                rusqlite::params![waddr, window],
                |row| row.get(0),
            )?;
            let pnl = pnl.unwrap_or(0.0);
            let roi_pct = if cfg.risk.paper_bankroll_usdc > 0.0 {
                100.0 * pnl / cfg.risk.paper_bankroll_usdc
            } else {
                0.0
            };

            let input = WalletScoreInput {
                paper_roi_pct: roi_pct,
                daily_return_stdev_pct: 0.0,
            };
            let score = compute_wscore(&input, &w);

            let changed = db.conn.execute(
                r#"
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
                "#,
                rusqlite::params![
                    waddr,
                    today,
                    window_days as i64,
                    score,
                    input.paper_roi_pct.max(0.0) / 20.0,
                    1.0,
                    roi_pct,
                    "mirror"
                ],
            )?;
            inserted += changed as u64;
        }
    }

    Ok(inserted)
}

pub trait GammaMarketsPager {
    fn gamma_markets_url(&self, limit: u32, offset: u32) -> String;
    fn fetch_gamma_markets_page(
        &self,
        limit: u32,
        offset: u32,
        filter: &GammaFilter,
    ) -> impl std::future::Future<Output = Result<(Vec<GammaMarket>, Vec<u8>)>> + Send;
}

pub trait HoldersFetcher {
    fn holders_url(&self, condition_id: &str, limit: u32) -> String;
    fn fetch_holders(
        &self,
        condition_id: &str,
        limit: u32,
    ) -> impl std::future::Future<Output = Result<(Vec<ApiHolderResponse>, Vec<u8>)>> + Send;
}

pub trait MarketTradesFetcher {
    fn market_trades_url(&self, condition_id: &str, limit: u32, offset: u32) -> String;
    fn fetch_market_trades_page(
        &self,
        condition_id: &str,
        limit: u32,
        offset: u32,
    ) -> impl std::future::Future<Output = Result<(Vec<ApiTrade>, Vec<u8>)>> + Send;
}

pub trait ActivityPager {
    fn activity_url(&self, user: &str, limit: u32, offset: u32) -> String;
    fn fetch_activity_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> impl std::future::Future<Output = Result<(Vec<ApiActivity>, Vec<u8>)>> + Send;
}

pub trait PositionsPager {
    fn positions_url(&self, user: &str, limit: u32, offset: u32) -> String;
    fn fetch_positions_page(
        &self,
        user: &str,
        limit: u32,
        offset: u32,
    ) -> impl std::future::Future<Output = Result<(Vec<ApiPosition>, Vec<u8>)>> + Send;
}

pub async fn run_market_scoring_once<P: GammaMarketsPager + Sync>(
    db: &Database,
    pager: &P,
    cfg: &Config,
) -> Result<u64> {
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
        let (markets, raw) = pager
            .fetch_gamma_markets_page(limit, offset, &filter)
            .await?;
        let page_len = markets.len();
        let url = pager.gamma_markets_url(limit, offset);
        db.conn.execute(
            "INSERT INTO raw_api_responses (api, method, url, response_body) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["gamma_api", "GET", url, raw],
        )?;

        if markets.is_empty() {
            break;
        }

        for m in markets {
            let Some(condition_id) = m.condition_id.clone() else {
                continue;
            };
            // Gamma API uses `question` for the market title; fall back to `title`.
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
            // Use 24h volume for daily scoring; fall back to total volume.
            let volume_24h = m
                .volume_24hr
                .as_deref()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| m.volume.as_deref().and_then(|s| s.parse::<f64>().ok()))
                .unwrap_or(0.0);

            // Gamma doesn't reliably provide these for MVP. Keep 0 to let liquidity/volume dominate.
            let trades_24h = 0;
            let unique_traders_24h = 0;
            let top_holder_concentration = 0.5;

            let days_to_expiry = compute_days_to_expiry(m.end_date.as_deref()).unwrap_or(0);

            // Apply coarse filters from config.
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

            // Upsert base market row.
            db.conn.execute(
                r#"
                INSERT INTO markets
                    (condition_id, title, slug, description, end_date, liquidity, volume, category, event_slug, last_updated_at)
                VALUES
                    (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'))
                ON CONFLICT(condition_id) DO UPDATE SET
                    title = excluded.title,
                    slug = excluded.slug,
                    description = excluded.description,
                    end_date = excluded.end_date,
                    liquidity = excluded.liquidity,
                    volume = excluded.volume,
                    category = excluded.category,
                    event_slug = excluded.event_slug,
                    last_updated_at = datetime('now')
                "#,
                rusqlite::params![
                    condition_id,
                    title,
                    m.slug,
                    m.description,
                    m.end_date,
                    liquidity,
                    volume_24h,
                    m.category,
                    m.event_slug
                ],
            )?;

            all.push(MarketCandidate {
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

        offset = offset.saturating_add(limit);
        if page_len < limit as usize {
            break;
        }
    }

    let ranked = rank_markets(all, cfg.market_scoring.top_n_markets);

    let mut inserted = 0_u64;
    let today = chrono::Utc::now().date_naive().to_string();
    for (i, sm) in ranked.into_iter().enumerate() {
        let rank = (i + 1) as i64;
        let changed = db.conn.execute(
            r#"
            INSERT INTO market_scores_daily
                (condition_id, score_date, mscore, rank)
            VALUES
                (?1, ?2, ?3, ?4)
            ON CONFLICT(condition_id, score_date) DO UPDATE SET
                mscore = excluded.mscore,
                rank = excluded.rank
            "#,
            rusqlite::params![sm.market.condition_id, today, sm.mscore, rank],
        )?;
        inserted += changed as u64;
    }

    metrics::counter!("evaluator_markets_scored_total").increment(inserted);
    Ok(inserted)
}

pub async fn run_wallet_discovery_once<H: HoldersFetcher + Sync, T: MarketTradesFetcher + Sync>(
    db: &Database,
    holders: &H,
    trades: &T,
    cfg: &Config,
) -> Result<u64> {
    let mut stmt = db.conn.prepare(
        r#"
        SELECT condition_id
        FROM market_scores_daily
        WHERE score_date = date('now')
        ORDER BY rank ASC
        LIMIT 20
        "#,
    )?;
    let markets_iter = stmt.query_map([], |row| row.get::<_, String>(0))?;

    let mut inserted = 0_u64;
    for m in markets_iter {
        let condition_id = m?;
        let (holder_resp, raw_h) = holders
            .fetch_holders(
                &condition_id,
                cfg.wallet_discovery.holders_per_market as u32,
            )
            .await?;
        let holders_url = holders.holders_url(
            &condition_id,
            cfg.wallet_discovery.holders_per_market as u32,
        );
        db.conn.execute(
            "INSERT INTO raw_api_responses (api, method, url, response_body) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["data_api", "GET", holders_url, raw_h],
        )?;

        let mut holder_wallets: Vec<HolderWallet> = Vec::new();
        for r in holder_resp {
            for h in r.holders {
                if let Some(w) = h.proxy_wallet {
                    holder_wallets.push(HolderWallet { proxy_wallet: w });
                }
            }
        }

        let (market_trades, raw_t) = trades
            .fetch_market_trades_page(&condition_id, 200, 0)
            .await?;
        let trades_url = trades.market_trades_url(&condition_id, 200, 0);
        db.conn.execute(
            "INSERT INTO raw_api_responses (api, method, url, response_body) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["data_api", "GET", trades_url, raw_t],
        )?;

        let mut trade_wallets: Vec<TradeWallet> = Vec::new();
        for t in market_trades {
            if let Some(w) = t.proxy_wallet {
                trade_wallets.push(TradeWallet { proxy_wallet: w });
            }
        }

        let discovered = discover_wallets_for_market(
            &holder_wallets,
            &trade_wallets,
            cfg.wallet_discovery.min_total_trades,
        );

        for w in discovered
            .into_iter()
            .take(cfg.wallet_discovery.max_wallets_per_market)
        {
            let changed = db.conn.execute(
                r#"
                INSERT OR IGNORE INTO wallets
                    (proxy_wallet, discovered_from, discovered_market, is_active)
                VALUES
                    (?1, ?2, ?3, 1)
                "#,
                rusqlite::params![w.proxy_wallet, w.discovered_from.as_str(), condition_id],
            )?;
            inserted += changed as u64;
        }
    }

    metrics::counter!("evaluator_wallets_discovered_total").increment(inserted);
    let watchlist: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM wallets WHERE is_active = 1",
        [],
        |row| row.get(0),
    )?;
    metrics::gauge!("evaluator_wallets_on_watchlist").set(watchlist as f64);
    Ok(inserted)
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
        let cfg = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

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

        let cnt_scores: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM market_scores_daily", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(cnt_scores > 0);

        let cnt_markets: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM markets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cnt_markets, 2);

        let cnt_raw: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM raw_api_responses", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(cnt_raw, 1);
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

    #[tokio::test]
    async fn test_run_wallet_discovery_inserts_wallets() {
        let cfg = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn.execute(
            "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank) VALUES (?1, date('now'), 0.9, 1)",
            rusqlite::params!["0xcond"],
        ).unwrap();

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
            raw: br#"[]"#.to_vec(),
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
            raw: br#"[]"#.to_vec(),
        };

        let inserted = run_wallet_discovery_once(&db, &holders, &trades, &cfg)
            .await
            .unwrap();
        assert!(inserted > 0);

        let cnt_wallets: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM wallets", [], |row| row.get(0))
            .unwrap();
        assert!(cnt_wallets >= 2); // holder + trader
    }

    #[tokio::test]
    async fn test_run_trades_ingestion_inserts_rows() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
            rusqlite::params!["0xw"],
        ).unwrap();

        struct OnePagePager;
        impl crate::ingestion::TradesPager for OnePagePager {
            fn trades_url(&self, user: &str, limit: u32, offset: u32) -> String {
                format!("https://data-api.polymarket.com/trades?user={user}&limit={limit}&offset={offset}")
            }
            async fn fetch_trades_page(
                &self,
                _user: &str,
                _limit: u32,
                offset: u32,
            ) -> Result<(Vec<ApiTrade>, Vec<u8>)> {
                if offset > 0 {
                    return Ok((vec![], br#"[]"#.to_vec()));
                }
                Ok((
                    vec![ApiTrade {
                        proxy_wallet: Some("0xw".to_string()),
                        condition_id: Some("0xcond".to_string()),
                        transaction_hash: Some("0xtx1".to_string()),
                        size: Some("1".to_string()),
                        price: Some("0.5".to_string()),
                        timestamp: Some(1),
                        asset: None,
                        title: None,
                        slug: None,
                        outcome: Some("YES".to_string()),
                        outcome_index: Some(0),
                        side: Some("BUY".to_string()),
                        pseudonym: None,
                        name: None,
                    }],
                    br#"[{"page":1}]"#.to_vec(),
                ))
            }
        }

        let pager = OnePagePager;
        let (_pages, inserted) = run_trades_ingestion_once(&db, &pager, 100).await.unwrap();
        assert_eq!(inserted, 1);
    }

    #[test]
    fn test_run_paper_tick_creates_paper_trades() {
        let cfg = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
            rusqlite::params!["0xw"],
        ).unwrap();

        db.conn.execute(
            r#"
            INSERT INTO trades_raw
                (proxy_wallet, condition_id, side, size, price, timestamp, transaction_hash, raw_json)
            VALUES
                (?1, ?2, 'BUY', 1.0, 0.5, 1, '0xtx1', '{}')
            "#,
            rusqlite::params!["0xw", "0xcond"],
        ).unwrap();

        let inserted = run_paper_tick_once(&db, &cfg).unwrap();
        assert_eq!(inserted, 1);

        let cnt: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cnt, 1);
    }

    #[test]
    fn test_run_wallet_scoring_inserts_wallet_scores() {
        let cfg = Config::from_toml_str(include_str!("../../../config/default.toml")).unwrap();
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES (?1, 'HOLDER', 1)",
            rusqlite::params!["0xw"],
        ).unwrap();

        // One settled trade with positive pnl.
        db.conn.execute(
            r#"
            INSERT INTO paper_trades
                (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl, created_at, settled_at)
            VALUES
                (?1, 'mirror', ?2, 'BUY', 100.0, 0.5, 'settled_win', 50.0, datetime('now'), datetime('now'))
            "#,
            rusqlite::params!["0xw", "0xcond"],
        ).unwrap();

        let inserted = run_wallet_scoring_once(&db, &cfg).unwrap();
        assert!(inserted > 0);

        let cnt: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM wallet_scores_daily", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(cnt > 0);
    }
}
