use crate::config::TraderConfig;
use crate::db::TraderDb;
use crate::engine::detector::TradeDetector;
use crate::engine::FollowedWallet;
use crate::polymarket::TraderPolymarketClient;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Run the watcher loop for a single wallet.
/// Polls for new trades, detects new ones, and (in later phases) mirrors them.
pub async fn run_watcher(
    db: Arc<TraderDb>,
    client: Arc<TraderPolymarketClient>,
    config: Arc<TraderConfig>,
    wallet: FollowedWallet,
    halted: Arc<AtomicBool>,
    cancel: CancellationToken,
) {
    let addr = &wallet.proxy_wallet;
    let poll_interval = Duration::from_secs(config.trading.poll_interval_secs);

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

                            // Phase 3 will add: mirror logic + risk checks here
                            // For now, just log the detected trades
                            for trade in &new_trades {
                                log_trade_event(&db, addr, trade).await;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(wallet = %addr, error = %e, "failed to fetch trades");
                    }
                }

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
