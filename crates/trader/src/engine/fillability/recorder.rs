use super::check::{check_fillable, WsBookEvent};
use super::storage::{insert_book_snapshot, settle_recording};
use crate::config::FillabilityConfig;
use crate::db::TraderDb;
use crate::types::Side;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Records order book snapshots from the CLOB WebSocket after trade detection.
pub struct FillabilityRecorder {
    db: Arc<TraderDb>,
    config: FillabilityConfig,
    active_recordings: Arc<Mutex<HashMap<String, RecordingHandle>>>,
}

struct RecordingHandle {
    timeout_cancel: CancellationToken,
    trade_hashes: Vec<String>,
    condition_id: String,
    recording_started_at: String,
    #[allow(dead_code)] // Stored for settlement enrichment (future: per-trade fill analysis)
    our_side: Side,
    #[allow(dead_code)] // Stored for settlement enrichment
    our_size_usd: f64,
    #[allow(dead_code)] // Stored for settlement enrichment
    our_target_price: f64,
}

impl FillabilityRecorder {
    pub fn new(db: Arc<TraderDb>, config: FillabilityConfig) -> Self {
        Self {
            db,
            config,
            active_recordings: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start recording order book snapshots for this token.
    /// If already recording, restart the 120s timeout (don't duplicate connections).
    pub async fn record_fillability(
        &self,
        token_id: &str,
        condition_id: &str,
        trigger_trade_hash: &str,
        our_side: Side,
        our_size_usd: f64,
        our_target_price: f64,
    ) {
        if !self.config.enabled {
            return;
        }

        let mut recordings = self.active_recordings.lock().await;

        if let Some(handle) = recordings.get_mut(token_id) {
            handle.timeout_cancel.cancel();
            handle.timeout_cancel = CancellationToken::new();
            handle.trade_hashes.push(trigger_trade_hash.to_string());
            debug!(
                token_id = token_id,
                hash = trigger_trade_hash,
                "reset fillability timeout (already recording)"
            );

            let token = token_id.to_string();
            let cancel = handle.timeout_cancel.clone();
            let active = Arc::clone(&self.active_recordings);
            let db = Arc::clone(&self.db);
            let window = self.config.window_secs;
            let cid = handle.condition_id.clone();
            let started_at = handle.recording_started_at.clone();

            tokio::spawn(async move {
                timeout_and_settle(cancel, window, &token, &cid, &started_at, &db, &active).await;
            });
            return;
        }

        if recordings.len() >= self.config.max_concurrent_recordings {
            warn!(
                token_id = token_id,
                limit = self.config.max_concurrent_recordings,
                "max concurrent fillability recordings reached, skipping"
            );
            return;
        }

        let timeout_cancel = CancellationToken::new();
        let started_at = chrono::Utc::now().to_rfc3339();
        recordings.insert(
            token_id.to_string(),
            RecordingHandle {
                timeout_cancel: timeout_cancel.clone(),
                trade_hashes: vec![trigger_trade_hash.to_string()],
                condition_id: condition_id.to_string(),
                recording_started_at: started_at.clone(),
                our_side,
                our_size_usd,
                our_target_price,
            },
        );
        drop(recordings);

        let token = token_id.to_string();
        let cid = condition_id.to_string();
        let db = Arc::clone(&self.db);
        let active = Arc::clone(&self.active_recordings);
        let ws_url = self.config.clob_ws_url.clone();
        let window = self.config.window_secs;

        info!(
            token_id = token_id,
            condition_id = condition_id,
            "starting fillability recording ({window}s)"
        );

        tokio::spawn(async move {
            run_recording(
                &ws_url,
                &token,
                &cid,
                our_side,
                our_size_usd,
                our_target_price,
                window,
                timeout_cancel,
                started_at,
                &db,
                &active,
            )
            .await;
        });
    }

    #[cfg(test)]
    pub async fn active_count(&self) -> usize {
        self.active_recordings.lock().await.len()
    }
}

async fn timeout_and_settle(
    cancel: CancellationToken,
    window_secs: u64,
    token_id: &str,
    condition_id: &str,
    recording_started_at: &str,
    db: &TraderDb,
    active: &Mutex<HashMap<String, RecordingHandle>>,
) {
    tokio::select! {
        _ = cancel.cancelled() => {
            debug!(token_id = token_id, "fillability timeout reset");
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(window_secs)) => {
            // Atomically remove — whoever removes first does the settlement
            let handle = active.lock().await.remove(token_id);
            if let Some(h) = handle {
                info!(token_id = token_id, "fillability window closed, settling");
                settle_recording(db, condition_id, token_id, &h.trade_hashes, recording_started_at).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_recording(
    ws_url: &str,
    token_id: &str,
    condition_id: &str,
    our_side: Side,
    our_size_usd: f64,
    our_target_price: f64,
    window_secs: u64,
    _timeout_cancel: CancellationToken, // Kept alive to prevent premature cancel; dedup resets it
    recording_started_at: String,
    db: &TraderDb,
    active: &Mutex<HashMap<String, RecordingHandle>>,
) {
    let ws_result = tokio_tungstenite::connect_async(ws_url).await;
    let (mut ws_stream, _response) = match ws_result {
        Ok(conn) => conn,
        Err(e) => {
            error!(token_id = token_id, error = %e, "failed to connect to CLOB WebSocket");
            active.lock().await.remove(token_id);
            return;
        }
    };

    let subscribe_msg = serde_json::json!({
        "type": "MARKET",
        "assets_ids": [token_id],
        "custom_feature_enabled": true,
    });
    if let Err(e) = ws_stream
        .send(Message::Text(subscribe_msg.to_string()))
        .await
    {
        error!(token_id = token_id, error = %e, "failed to send subscribe message");
        active.lock().await.remove(token_id);
        return;
    }

    debug!(token_id = token_id, "subscribed to CLOB book updates");
    // Use our own deadline — don't listen for timeout_cancel here because the dedup
    // path resets it to extend the window, and we don't want to stop the WebSocket.
    // The timeout_and_settle task handles settlement when the extended window closes.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(window_secs);

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                info!(token_id = token_id, "fillability recording window expired");
                break;
            }
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        process_book_message(
                            &text, db, condition_id, token_id,
                            our_side, our_size_usd, our_target_price,
                        ).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        warn!(token_id = token_id, error = %e, "CLOB WebSocket error");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    let _ = ws_stream.close(None).await;

    // Atomically remove — whoever removes first does the settlement
    let handle = active.lock().await.remove(token_id);
    if let Some(h) = handle {
        settle_recording(
            db,
            condition_id,
            token_id,
            &h.trade_hashes,
            &recording_started_at,
        )
        .await;
    }
}

async fn process_book_message(
    text: &str,
    db: &TraderDb,
    condition_id: &str,
    token_id: &str,
    our_side: Side,
    our_size_usd: f64,
    our_target_price: f64,
) {
    // The CLOB WebSocket sends initial snapshots as JSON arrays: [{bids, asks, ...}]
    // and subsequent updates as dicts with price_changes (no bids/asks).
    // Try array-wrapped snapshot first, then direct dict.
    let book_event = if let Ok(arr) = serde_json::from_str::<Vec<WsBookEvent>>(text) {
        arr.into_iter().next()
    } else {
        serde_json::from_str::<WsBookEvent>(text).ok()
    };
    let Some(book_event) = book_event else {
        return;
    };

    let bids = book_event.bids.as_deref().unwrap_or(&[]);
    let asks = book_event.asks.as_deref().unwrap_or(&[]);

    if bids.is_empty() && asks.is_empty() {
        return;
    }

    let fill = check_fillable(bids, asks, our_side, our_size_usd, our_target_price);

    let best_bid = bids
        .iter()
        .map(|l| l.price)
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let best_ask = asks
        .iter()
        .map(|l| l.price)
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let bid_depth: f64 = bids.iter().map(|l| l.price * l.size).sum();
    let ask_depth: f64 = asks.iter().map(|l| l.price * l.size).sum();
    let spread = match (best_bid, best_ask) {
        (Some(b), Some(a)) => Some((a - b) * 100.0),
        _ => None,
    };
    let mid = match (best_bid, best_ask) {
        (Some(b), Some(a)) => Some((a + b) / 2.0),
        _ => None,
    };

    let levels_json = serde_json::json!({ "bids": bids, "asks": asks }).to_string();

    insert_book_snapshot(
        db,
        condition_id,
        token_id,
        best_bid,
        best_ask,
        bid_depth,
        ask_depth,
        spread,
        mid,
        fill.fillable,
        fill.available_depth_usd,
        fill.vwap,
        fill.slippage_cents,
        &levels_json,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fillability_recorder_disabled() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let config = FillabilityConfig {
            enabled: false,
            window_secs: 5,
            clob_ws_url: "wss://invalid.example.com/ws".to_string(),
            max_concurrent_recordings: 20,
        };
        let recorder = FillabilityRecorder::new(Arc::clone(&db), config);
        recorder
            .record_fillability("token-1", "cond-1", "hash-1", Side::Buy, 10.0, 0.50)
            .await;
        assert_eq!(recorder.active_count().await, 0);
    }

    #[tokio::test]
    async fn test_fillability_recorder_max_concurrent() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let config = FillabilityConfig {
            enabled: true,
            window_secs: 5,
            clob_ws_url: "wss://invalid.example.com/ws".to_string(),
            max_concurrent_recordings: 2,
        };
        let recorder = FillabilityRecorder::new(Arc::clone(&db), config);

        {
            let mut recordings = recorder.active_recordings.lock().await;
            for (token, cond, hash) in [("token-a", "c-a", "h-a"), ("token-b", "c-b", "h-b")] {
                recordings.insert(
                    token.to_string(),
                    RecordingHandle {
                        timeout_cancel: CancellationToken::new(),
                        trade_hashes: vec![hash.to_string()],
                        condition_id: cond.to_string(),
                        recording_started_at: "2026-01-01T00:00:00Z".to_string(),
                        our_side: Side::Buy,
                        our_size_usd: 10.0,
                        our_target_price: 0.50,
                    },
                );
            }
        }

        recorder
            .record_fillability("token-c", "cond-c", "hash-c", Side::Buy, 10.0, 0.50)
            .await;
        assert_eq!(recorder.active_count().await, 2);
    }
}
