use crate::config::TraderConfig;
use crate::db::TraderDb;
use crate::engine::detector::TradeDetector;
use crate::engine::mirror;
use crate::engine::settlement;
use crate::engine::FollowedWallet;
use crate::polymarket::TraderPolymarketClient;
use crate::risk::wallet as risk_wallet;
use crate::risk::RiskManager;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Aggregates poll-loop stats and emits a single debug summary periodically.
/// Errors and important events (new trades, mirrors) still log immediately.
struct PollStats {
    polls: u64,
    skipped_halted: u64,
    skipped_paused: u64,
    fetches_ok: u64,
    fetch_errors: u64,
    new_trades: u64,
    mirrors_executed: u64,
    mirrors_skipped: u64,
    last_flush: Instant,
    flush_interval: Duration,
}

impl PollStats {
    fn new(flush_interval: Duration) -> Self {
        Self {
            polls: 0,
            skipped_halted: 0,
            skipped_paused: 0,
            fetches_ok: 0,
            fetch_errors: 0,
            new_trades: 0,
            mirrors_executed: 0,
            mirrors_skipped: 0,
            last_flush: Instant::now(),
            flush_interval,
        }
    }

    fn should_flush(&self) -> bool {
        self.last_flush.elapsed() >= self.flush_interval
    }

    fn flush(&mut self, wallet: &str) {
        if self.polls > 0 {
            debug!(
                wallet = wallet,
                polls = self.polls,
                fetches_ok = self.fetches_ok,
                fetch_errors = self.fetch_errors,
                new_trades = self.new_trades,
                mirrors_executed = self.mirrors_executed,
                mirrors_skipped = self.mirrors_skipped,
                skipped_halted = self.skipped_halted,
                skipped_paused = self.skipped_paused,
                "poll summary"
            );
        }
        self.polls = 0;
        self.skipped_halted = 0;
        self.skipped_paused = 0;
        self.fetches_ok = 0;
        self.fetch_errors = 0;
        self.new_trades = 0;
        self.mirrors_executed = 0;
        self.mirrors_skipped = 0;
        self.last_flush = Instant::now();
    }
}

/// Run the watcher loop for a single wallet.
/// Polls for new trades, detects new ones, and (in later phases) mirrors them.
pub async fn run_watcher(
    db: Arc<TraderDb>,
    client: Arc<TraderPolymarketClient>,
    config: Arc<TraderConfig>,
    risk: Arc<RiskManager>,
    wallet: FollowedWallet,
    halted: Arc<AtomicBool>,
    cancel: CancellationToken,
) {
    let addr = &wallet.proxy_wallet;
    let poll_interval = config
        .trading
        .poll_interval_ms
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_secs(config.trading.poll_interval_secs));

    info!(wallet = %addr, mode = %wallet.trading_mode, "watcher started");

    let mut detector = TradeDetector::new(wallet.last_trade_seen_hash.clone());
    let mut interval = tokio::time::interval(poll_interval);
    let mut stats = PollStats::new(Duration::from_secs(60));
    // Don't fire immediately — let the system stabilize first
    interval.tick().await;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                stats.flush(addr);
                info!(wallet = %addr, "watcher cancelled");
                break;
            }
            _ = interval.tick() => {
                stats.polls += 1;

                // Check if globally halted
                if halted.load(Ordering::SeqCst) {
                    stats.skipped_halted += 1;
                    if stats.should_flush() { stats.flush(addr); }
                    continue;
                }

                // Check wallet status in DB
                let status = load_wallet_status(&db, addr).await;
                match status.as_deref() {
                    Some("paused") => {
                        stats.skipped_paused += 1;
                        if stats.should_flush() { stats.flush(addr); }
                        continue;
                    }
                    Some("killed") | Some("removed") => {
                        stats.flush(addr);
                        info!(wallet = %addr, status = status.as_deref().unwrap_or(""), "wallet no longer active, stopping watcher");
                        break;
                    }
                    _ => {}
                }

                // Poll for trades
                match client.fetch_trades(addr, 200, 0).await {
                    Ok(trades) => {
                        stats.fetches_ok += 1;
                        let new_trades = detector.detect_new(&trades);
                        if !new_trades.is_empty() {
                            stats.new_trades += new_trades.len() as u64;
                            info!(wallet = %addr, new_count = new_trades.len(), "detected new trades");

                            // Update watermark in DB
                            if let Some(last) = new_trades.last() {
                                let hash = TraderPolymarketClient::trade_hash(last);
                                let ts = last.timestamp.map(|t| {
                                    chrono::DateTime::from_timestamp(t, 0)
                                        .map(|dt| dt.to_rfc3339())
                                        .unwrap_or_default()
                                });
                                update_watermark(&db, addr, &hash, ts.as_deref()).await;
                            }

                            // Mirror each detected trade
                            let their_bankroll = wallet
                                .estimated_bankroll_usd
                                .unwrap_or(config.trading.default_their_bankroll_usd);

                            for trade in &new_trades {
                                log_trade_event(&db, addr, trade).await;

                                let detection_delay_ms = trade
                                    .timestamp
                                    .map_or(0, |ts| {
                                        let now_ms = chrono::Utc::now().timestamp_millis();
                                        now_ms - (ts * 1000)
                                    });

                                match mirror::mirror_trade(
                                    &db,
                                    &risk,
                                    &config.trading,
                                    trade,
                                    addr,
                                    wallet.trading_mode,
                                    detection_delay_ms,
                                    their_bankroll,
                                )
                                .await
                                {
                                    Ok(result) => {
                                        if result.executed {
                                            stats.mirrors_executed += 1;
                                            info!(
                                                wallet = %addr,
                                                trade_id = result.trade_id.unwrap_or(0),
                                                "mirror trade executed"
                                            );
                                        } else {
                                            stats.mirrors_skipped += 1;
                                        }
                                    }
                                    Err(e) => {
                                        error!(
                                            wallet = %addr,
                                            error = %e,
                                            "mirror trade failed"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        stats.fetch_errors += 1;
                        warn!(wallet = %addr, error = %e, "failed to fetch trades");
                    }
                }

                // Check for settled markets on this wallet's open positions
                check_settlements(&db, &client, addr).await;

                // Periodically prune detector to prevent memory leak
                detector.prune(10_000);

                // Flush aggregated stats periodically
                if stats.should_flush() { stats.flush(addr); }
            }
        }
    }

    info!(wallet = %addr, "watcher stopped");
}

async fn load_wallet_status(db: &TraderDb, wallet: &str) -> Option<String> {
    let addr = wallet.to_string();
    db.call(move |conn| {
        conn.query_row(
            "SELECT status FROM followed_wallets WHERE proxy_wallet = ?1",
            [addr],
            |row| row.get(0),
        )
    })
    .await
    .ok()
}

async fn update_watermark(db: &TraderDb, wallet: &str, hash: &str, timestamp: Option<&str>) {
    let addr = wallet.to_string();
    let h = hash.to_string();
    let ts = timestamp.map(std::string::ToString::to_string);
    let now = chrono::Utc::now().to_rfc3339();

    if let Err(e) = db
        .call(move |conn| {
            conn.execute(
                "UPDATE followed_wallets SET last_trade_seen_hash = ?1, last_trade_seen_at = ?2, updated_at = ?3 WHERE proxy_wallet = ?4",
                rusqlite::params![h, ts, now, addr],
            )?;
            Ok(())
        })
        .await
    {
        error!(wallet = wallet, error = %e, "failed to update watermark");
    }
}

async fn log_trade_event(db: &TraderDb, wallet: &str, trade: &crate::polymarket::RawTrade) {
    let addr = wallet.to_string();
    let details = serde_json::json!({
        "condition_id": trade.condition_id,
        "side": trade.side,
        "size": trade.size,
        "price": trade.price,
        "timestamp": trade.timestamp,
    })
    .to_string();
    let now = chrono::Utc::now().to_rfc3339();

    if let Err(e) = db
        .call(move |conn| {
            conn.execute(
                "INSERT INTO trade_events (event_type, proxy_wallet, details_json, created_at)
                 VALUES ('trade_detected', ?1, ?2, ?3)",
                rusqlite::params![addr, details, now],
            )?;
            Ok(())
        })
        .await
    {
        error!(wallet = wallet, error = %e, "failed to log trade event");
    }
}

/// Check for markets that have settled and process settlement.
/// Queries open positions for this wallet, checks market resolution via Gamma API.
async fn check_settlements(db: &Arc<TraderDb>, client: &Arc<TraderPolymarketClient>, wallet: &str) {
    // Get distinct condition_ids with open trades for this wallet
    let addr = wallet.to_string();
    let open_conditions: Vec<String> = match db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT condition_id FROM trader_trades
                 WHERE proxy_wallet = ?1 AND status = 'open'",
            )?;
            let rows = stmt
                .query_map([&addr], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(rows)
        })
        .await
    {
        Ok(conds) => conds,
        Err(e) => {
            debug!(wallet = wallet, error = %e, "failed to query open conditions");
            return;
        }
    };

    if open_conditions.is_empty() {
        return;
    }

    // For each condition, check if the market is resolved via the Gamma API
    for condition_id in &open_conditions {
        let url = format!(
            "https://gamma-api.polymarket.com/markets?condition_id={}",
            urlencoding::encode(condition_id)
        );

        let Some(resp) = client.check_market_resolution(&url).await else {
            continue;
        };

        match settlement::settle_market(db, condition_id, resp).await {
            Ok(count) if count > 0 => {
                info!(
                    wallet = wallet,
                    condition_id = condition_id,
                    settled = count,
                    "settled resolved market"
                );
                // Reduce exposure for settled trades
                if let Err(e) = risk_wallet::reduce_exposure(db, wallet, count as f64).await {
                    error!(wallet = wallet, error = %e, "failed to reduce wallet exposure after settlement");
                }
                if let Err(e) = risk_wallet::reduce_portfolio_exposure(db, count as f64, 0.0).await
                {
                    error!(wallet = wallet, error = %e, "failed to reduce portfolio exposure after settlement");
                }
            }
            Ok(_) => {}
            Err(e) => {
                error!(
                    wallet = wallet,
                    condition_id = condition_id,
                    error = %e,
                    "settlement failed"
                );
            }
        }
    }
}
