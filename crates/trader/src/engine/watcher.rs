use crate::config::TraderConfig;
use crate::db::TraderDb;
use crate::engine::detector::TradeDetector;
use crate::engine::fillability::FillabilityRecorder;
use crate::engine::mirror;
use crate::engine::settlement;
use crate::engine::FollowedWallet;
use crate::polymarket::TraderPolymarketClient;
use crate::risk::wallet as risk_wallet;
use crate::risk::RiskManager;
use crate::types::Side;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Run the watcher loop for a single wallet.
/// Polls for new trades, detects new ones, mirrors them, and records fillability.
#[allow(clippy::too_many_arguments)]
pub async fn run_watcher(
    db: Arc<TraderDb>,
    client: Arc<TraderPolymarketClient>,
    config: Arc<TraderConfig>,
    risk: Arc<RiskManager>,
    fillability: Arc<FillabilityRecorder>,
    wallet: FollowedWallet,
    halted: Arc<AtomicBool>,
    cancel: CancellationToken,
) {
    let addr = &wallet.proxy_wallet;
    let poll_interval = Duration::from_millis(config.trading.poll_interval_ms);

    info!(wallet = %addr, mode = %wallet.trading_mode, "watcher started");

    let mut detector = TradeDetector::new(wallet.last_trade_seen_hash.clone());
    let mut interval = tokio::time::interval(poll_interval);
    // Don't fire immediately — let the system stabilize first
    interval.tick().await;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!(wallet = %addr, "watcher cancelled");
                break;
            }
            _ = interval.tick() => {
                // Check if globally halted
                if halted.load(Ordering::SeqCst) {
                    debug!(wallet = %addr, "skipping poll — trading halted");
                    continue;
                }

                // Check wallet status in DB
                let status = load_wallet_status(&db, addr).await;
                match status.as_deref() {
                    Some("paused") => {
                        debug!(wallet = %addr, "skipping poll — wallet paused");
                        continue;
                    }
                    Some("killed") | Some("removed") => {
                        info!(wallet = %addr, status = status.as_deref().unwrap_or(""), "wallet no longer active, stopping watcher");
                        break;
                    }
                    _ => {}
                }

                // Poll for trades
                match client.fetch_trades(addr, 200, 0).await {
                    Ok(trades) => {
                        let new_trades = detector.detect_new(&trades);
                        if !new_trades.is_empty() {
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

                                // Start fillability recording (non-blocking background task)
                                if let Some(ref token_id) = trade.asset {
                                    let condition_id = trade.condition_id.as_deref().unwrap_or("");
                                    let trade_hash = TraderPolymarketClient::trade_hash(trade);
                                    let side = trade.side.as_deref()
                                        .and_then(Side::from_str_loose)
                                        .unwrap_or(Side::Buy);
                                    let their_price: f64 = trade.price.as_deref()
                                        .and_then(|s| s.parse().ok())
                                        .unwrap_or(0.5);
                                    let their_size: f64 = trade.size.as_deref()
                                        .and_then(|s| s.parse().ok())
                                        .unwrap_or(0.0);
                                    fillability.record_fillability(
                                        token_id,
                                        condition_id,
                                        &trade_hash,
                                        side,
                                        their_size,
                                        their_price,
                                    ).await;
                                }

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
                                            info!(
                                                wallet = %addr,
                                                trade_id = result.trade_id.unwrap_or(0),
                                                "mirror trade executed"
                                            );
                                        } else {
                                            debug!(
                                                wallet = %addr,
                                                reason = result.reason.as_deref().unwrap_or("unknown"),
                                                "mirror trade skipped"
                                            );
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
                        warn!(wallet = %addr, error = %e, "failed to fetch trades");
                    }
                }

                // Check for settled markets on this wallet's open positions
                check_settlements(&db, &client, addr).await;

                // Periodically prune detector to prevent memory leak
                detector.prune(10_000);
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
