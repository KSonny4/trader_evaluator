//! Event subscribers for logging, monitoring, and event-driven triggers.
//!
//! Subscribers receive events from the EventBus and perform actions like
//! logging to stdout, persisting to database, triggering downstream jobs,
//! or updating metrics.

#![allow(dead_code)] // Phase 3 infrastructure - functions used when event triggers enabled

use crate::event_bus::EventBus;
use crate::events::{OperationalEvent, PipelineEvent};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Spawns a logging subscriber that logs all events to stdout.
///
/// This task runs indefinitely, logging pipeline and operational events
/// as they arrive. Useful for production validation and debugging.
pub async fn spawn_logging_subscriber(event_bus: Arc<EventBus>) {
    // Subscribe to both pipeline and operational events
    let mut pipeline_rx = event_bus.subscribe_pipeline();
    let mut operational_rx = event_bus.subscribe_operational();

    loop {
        tokio::select! {
            // Pipeline events (job completion signals)
            Ok(event) = pipeline_rx.recv() => {
                match event {
                    PipelineEvent::MarketsScored { markets_scored, events_ranked, completed_at } => {
                        tracing::info!(
                            event_type = "markets_scored",
                            markets_scored,
                            events_ranked,
                            %completed_at,
                            "Pipeline event: MarketsScored"
                        );
                    }
                    PipelineEvent::WalletsDiscovered { market_id, wallets_added, discovered_at } => {
                        tracing::info!(
                            event_type = "wallets_discovered",
                            %market_id,
                            wallets_added,
                            %discovered_at,
                            "Pipeline event: WalletsDiscovered"
                        );
                    }
                    PipelineEvent::TradesIngested { wallet_address, trades_count, ingested_at } => {
                        tracing::info!(
                            event_type = "trades_ingested",
                            %wallet_address,
                            trades_count,
                            %ingested_at,
                            "Pipeline event: TradesIngested"
                        );
                    }
                    PipelineEvent::WalletsClassified { wallets_classified, classified_at } => {
                        tracing::info!(
                            event_type = "wallets_classified",
                            wallets_classified,
                            %classified_at,
                            "Pipeline event: WalletsClassified"
                        );
                    }
                    PipelineEvent::WalletRulesEvaluated { wallets_evaluated, transitions, evaluated_at } => {
                        tracing::info!(
                            event_type = "wallet_rules_evaluated",
                            wallets_evaluated,
                            transitions,
                            %evaluated_at,
                            "Pipeline event: WalletRulesEvaluated"
                        );
                    }
                }
            }

            // Operational events (monitoring and observability)
            Ok(event) = operational_rx.recv() => {
                match event {
                    OperationalEvent::JobStarted { job_name, started_at } => {
                        tracing::info!(
                            event_type = "job_started",
                            %job_name,
                            %started_at,
                            "Operational event: JobStarted"
                        );
                    }
                    OperationalEvent::JobCompleted { job_name, duration_ms, completed_at } => {
                        tracing::info!(
                            event_type = "job_completed",
                            %job_name,
                            duration_ms,
                            %completed_at,
                            "Operational event: JobCompleted"
                        );
                    }
                    OperationalEvent::JobFailed { job_name, error, failed_at } => {
                        tracing::warn!(
                            event_type = "job_failed",
                            %job_name,
                            %error,
                            %failed_at,
                            "Operational event: JobFailed"
                        );
                    }
                    OperationalEvent::BackpressureWarning { queue_name, current_size, capacity, warned_at } => {
                        tracing::warn!(
                            event_type = "backpressure_warning",
                            %queue_name,
                            current_size,
                            capacity,
                            %warned_at,
                            "Operational event: BackpressureWarning"
                        );
                    }
                }
            }

            else => {
                // Both channels closed, exit loop
                tracing::info!("Logging subscriber shutting down (event bus closed)");
                break;
            }
        }
    }
}

/// Spawns a discovery trigger subscriber that listens for `MarketsScored`
/// pipeline events and triggers wallet discovery by sending on `discovery_tx`.
///
/// This replaces the timer-based discovery scheduling when the
/// `enable_discovery_event_trigger` config flag is enabled.
///
/// The subscriber runs indefinitely until the event bus is dropped.
#[allow(dead_code)] // Phase 3: Will be wired in orchestration subscriber (Task #5)
pub async fn spawn_discovery_trigger_subscriber(
    event_bus: Arc<EventBus>,
    discovery_tx: mpsc::Sender<()>,
) {
    let mut pipeline_rx = event_bus.subscribe_pipeline();

    loop {
        match pipeline_rx.recv().await {
            Ok(PipelineEvent::MarketsScored {
                markets_scored,
                events_ranked,
                completed_at,
            }) => {
                tracing::info!(
                    markets_scored,
                    events_ranked,
                    %completed_at,
                    "MarketsScored received — triggering wallet discovery"
                );
                if let Err(e) = discovery_tx.send(()).await {
                    tracing::error!(error = %e, "failed to send discovery trigger");
                    break;
                }
            }
            Ok(_) => {
                // Ignore non-MarketsScored pipeline events
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(
                    skipped = n,
                    "discovery trigger subscriber lagged, continuing"
                );
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::info!("discovery trigger subscriber shutting down (event bus closed)");
                break;
            }
        }
    }
}

/// Accumulates wallet addresses from `TradesIngested` events and triggers
/// classification in batches. Deduplicates wallets within each batch window.
#[allow(dead_code)] // Phase 3: Will be wired in orchestration subscriber (Task #5)
pub struct TradesIngestedAccumulator {
    wallets: HashSet<String>,
}

#[allow(dead_code)] // Phase 3: Will be wired in orchestration subscriber (Task #5)
impl TradesIngestedAccumulator {
    /// Creates a new empty accumulator.
    pub fn new() -> Self {
        Self {
            wallets: HashSet::new(),
        }
    }

    /// Adds a wallet address to the current batch.
    pub fn add_wallet(&mut self, address: String) {
        self.wallets.insert(address);
    }

    /// Returns the number of unique wallets accumulated.
    pub fn len(&self) -> usize {
        self.wallets.len()
    }

    /// Returns true if no wallets have been accumulated.
    pub fn is_empty(&self) -> bool {
        self.wallets.is_empty()
    }

    /// Drains the accumulated wallets and returns them.
    pub fn drain(&mut self) -> HashSet<String> {
        std::mem::take(&mut self.wallets)
    }
}

/// Spawns a subscriber that batches `TradesIngested` events and triggers
/// persona classification at a configurable interval.
///
/// When `TradesIngested` events arrive, the subscriber accumulates the wallet
/// addresses. Every `batch_window` duration, if any wallets have accumulated,
/// it sends a trigger on the `classification_tx` channel.
///
/// This subscriber only runs when `enable_classification_event_trigger=true`.
#[allow(dead_code)] // Phase 3: Will be wired in orchestration subscriber (Task #5)
pub async fn spawn_classification_trigger_subscriber(
    event_bus: Arc<EventBus>,
    classification_tx: mpsc::Sender<()>,
    batch_window: Duration,
) {
    let mut pipeline_rx = event_bus.subscribe_pipeline();
    let mut accumulator = TradesIngestedAccumulator::new();
    let mut timer = tokio::time::interval(batch_window);
    // The first tick completes immediately; consume it so we don't trigger
    // classification with an empty accumulator at startup.
    timer.tick().await;

    loop {
        tokio::select! {
            Ok(event) = pipeline_rx.recv() => {
                if let PipelineEvent::TradesIngested { wallet_address, .. } = event {
                    accumulator.add_wallet(wallet_address);
                }
            }
            _ = timer.tick() => {
                if !accumulator.is_empty() {
                    let batch = accumulator.drain();
                    tracing::info!(
                        wallets = batch.len(),
                        "classification trigger: batched wallets, triggering classification"
                    );
                    if classification_tx.send(()).await.is_err() {
                        tracing::warn!("classification trigger: channel closed, shutting down");
                        break;
                    }
                }
            }
            else => {
                tracing::info!("classification trigger subscriber shutting down");
                break;
            }
        }
    }
}

/// Spawns a fast-path subscriber that bridges pipeline TradesIngested events
/// to the coalescing fast-path watch channel.
///
/// When trades are ingested, this subscriber triggers the fast-path channel
/// so downstream consumers (e.g. paper trading) react with minimal latency.
/// Multiple TradesIngested events are coalesced - only the latest generation matters.
///
/// Only spawn when `enable_fast_path_trigger=true`.
#[allow(dead_code)] // Phase 3: Will be wired in orchestration subscriber (Task #5)
pub async fn spawn_fast_path_subscriber(event_bus: Arc<EventBus>) {
    let mut pipeline_rx = event_bus.subscribe_pipeline();

    loop {
        match pipeline_rx.recv().await {
            Ok(PipelineEvent::TradesIngested {
                wallet_address,
                trades_count,
                ..
            }) => {
                tracing::info!(
                    %wallet_address,
                    trades_count,
                    "fast-path: TradesIngested -> triggering fast-path"
                );
                event_bus.trigger_fast_path();
            }
            Ok(_) => {
                // Ignore non-TradesIngested pipeline events
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "fast-path subscriber lagged, skipping events");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::info!("fast-path subscriber shutting down (pipeline channel closed)");
                break;
            }
        }
    }
}

/// Spawns a fast-path worker that reacts to coalesced fast-path triggers.
///
/// Watches the fast-path watch channel and sends a tick signal whenever
/// a new generation is available. The signal includes the generation number.
///
/// The `tick_tx` channel is the output: downstream consumers (e.g. paper trading
/// scheduler in the orchestration layer) receive generation numbers when
/// new trades have been ingested and need processing.
#[allow(dead_code)] // Phase 3: Will be wired in orchestration subscriber (Task #5)
pub async fn spawn_fast_path_worker(event_bus: Arc<EventBus>, tick_tx: mpsc::Sender<u64>) {
    let mut fast_path_rx = event_bus.subscribe_fast_path();

    loop {
        match fast_path_rx.changed().await {
            Ok(()) => {
                let generation = fast_path_rx.borrow().generation;
                tracing::info!(
                    generation,
                    "fast-path worker: new generation, signaling paper trading tick"
                );
                if tick_tx.send(generation).await.is_err() {
                    tracing::info!("fast-path worker: tick receiver dropped, shutting down");
                    break;
                }
            }
            Err(_) => {
                tracing::info!("fast-path worker shutting down (watch channel closed)");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_bus::EventBus;
    use crate::events::PipelineEvent;
    use chrono::Utc;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_logging_subscriber_receives_pipeline_events() {
        let bus = Arc::new(EventBus::new(16));

        // Spawn logging subscriber
        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_logging_subscriber(subscriber_bus).await;
        });

        // Give subscriber time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Publish event
        let event = PipelineEvent::MarketsScored {
            markets_scored: 100,
            events_ranked: 50,
            completed_at: Utc::now(),
        };
        let _ = bus.publish_pipeline(event);

        // Give subscriber time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Cleanup
        handle.abort();

        // Test passes if no panic (subscriber received and logged event)
    }

    #[tokio::test]
    async fn test_logging_subscriber_receives_operational_events() {
        let bus = Arc::new(EventBus::new(16));

        // Spawn logging subscriber
        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_logging_subscriber(subscriber_bus).await;
        });

        // Give subscriber time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Publish event
        let event = crate::events::OperationalEvent::JobStarted {
            job_name: "test_job".to_string(),
            started_at: Utc::now(),
        };
        let _ = bus.publish_operational(event);

        // Give subscriber time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Cleanup
        handle.abort();

        // Test passes if no panic (subscriber received and logged event)
    }

    #[tokio::test]
    async fn test_discovery_trigger_markets_scored_triggers_discovery() {
        let bus = Arc::new(EventBus::new(16));
        let (discovery_tx, mut discovery_rx) = mpsc::channel::<()>(8);

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_discovery_trigger_subscriber(subscriber_bus, discovery_tx).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let event = PipelineEvent::MarketsScored {
            markets_scored: 100,
            events_ranked: 50,
            completed_at: Utc::now(),
        };
        bus.publish_pipeline(event).unwrap();

        let result =
            tokio::time::timeout(tokio::time::Duration::from_millis(100), discovery_rx.recv())
                .await;

        assert!(result.is_ok(), "should receive discovery trigger");
        assert!(
            result.unwrap().is_some(),
            "discovery channel should not be closed"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn test_discovery_trigger_ignores_non_markets_scored_events() {
        let bus = Arc::new(EventBus::new(16));
        let (discovery_tx, mut discovery_rx) = mpsc::channel::<()>(8);

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_discovery_trigger_subscriber(subscriber_bus, discovery_tx).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let event = PipelineEvent::WalletsDiscovered {
            market_id: "test-market".to_string(),
            wallets_added: 10,
            discovered_at: Utc::now(),
        };
        bus.publish_pipeline(event).unwrap();

        let result =
            tokio::time::timeout(tokio::time::Duration::from_millis(100), discovery_rx.recv())
                .await;

        assert!(
            result.is_err(),
            "should timeout — non-MarketsScored events should not trigger discovery"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn test_discovery_trigger_multiple_markets_scored_events() {
        let bus = Arc::new(EventBus::new(16));
        let (discovery_tx, mut discovery_rx) = mpsc::channel::<()>(8);

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_discovery_trigger_subscriber(subscriber_bus, discovery_tx).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        for i in 0..3 {
            let event = PipelineEvent::MarketsScored {
                markets_scored: 100 + i,
                events_ranked: 50,
                completed_at: Utc::now(),
            };
            bus.publish_pipeline(event).unwrap();
        }

        for _ in 0..3 {
            let result =
                tokio::time::timeout(tokio::time::Duration::from_millis(200), discovery_rx.recv())
                    .await;
            assert!(result.is_ok(), "should receive each discovery trigger");
        }

        handle.abort();
    }

    #[tokio::test]
    async fn test_discovery_trigger_shuts_down_when_receiver_dropped() {
        let bus = Arc::new(EventBus::new(16));
        let (discovery_tx, discovery_rx) = mpsc::channel::<()>(8);

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_discovery_trigger_subscriber(subscriber_bus, discovery_tx).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Drop the receiver so discovery_tx.send() fails
        drop(discovery_rx);

        // Publish an event to trigger the send path
        let event = PipelineEvent::MarketsScored {
            markets_scored: 100,
            events_ranked: 50,
            completed_at: Utc::now(),
        };
        bus.publish_pipeline(event).unwrap();

        // Subscriber should exit gracefully after failed send
        let result = tokio::time::timeout(tokio::time::Duration::from_millis(200), handle).await;

        assert!(
            result.is_ok(),
            "subscriber should shut down when discovery receiver is dropped"
        );
    }

    // ── Fast-path subscriber tests ──

    #[tokio::test]
    async fn test_fast_path_subscriber_triggers_on_trades_ingested() {
        let bus = Arc::new(EventBus::new(16));
        let mut fast_path_rx = bus.subscribe_fast_path();

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_fast_path_subscriber(subscriber_bus).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let _ = bus.publish_pipeline(PipelineEvent::TradesIngested {
            wallet_address: "0xabc".to_string(),
            trades_count: 5,
            ingested_at: Utc::now(),
        });

        tokio::time::timeout(
            tokio::time::Duration::from_millis(200),
            fast_path_rx.changed(),
        )
        .await
        .expect("timed out waiting for fast-path trigger")
        .expect("watch channel error");

        assert_eq!(fast_path_rx.borrow().generation, 1);
        handle.abort();
    }

    #[tokio::test]
    async fn test_fast_path_subscriber_ignores_non_trades_events() {
        let bus = Arc::new(EventBus::new(16));
        let mut fast_path_rx = bus.subscribe_fast_path();

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_fast_path_subscriber(subscriber_bus).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let _ = bus.publish_pipeline(PipelineEvent::MarketsScored {
            markets_scored: 100,
            events_ranked: 50,
            completed_at: Utc::now(),
        });

        let result = tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            fast_path_rx.changed(),
        )
        .await;

        assert!(
            result.is_err(),
            "fast-path should not trigger on non-TradesIngested events"
        );
        assert_eq!(fast_path_rx.borrow().generation, 0);
        handle.abort();
    }

    #[tokio::test]
    async fn test_fast_path_coalescing_multiple_triggers_one_reaction() {
        let bus = Arc::new(EventBus::new(16));
        let (tick_tx, mut tick_rx) = mpsc::channel::<u64>(16);

        let sub_bus = bus.clone();
        let sub_handle = tokio::spawn(async move {
            spawn_fast_path_subscriber(sub_bus).await;
        });
        let worker_bus = bus.clone();
        let worker_handle = tokio::spawn(async move {
            spawn_fast_path_worker(worker_bus, tick_tx).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        for i in 0..5 {
            let _ = bus.publish_pipeline(PipelineEvent::TradesIngested {
                wallet_address: format!("0xwallet{i}"),
                trades_count: 1,
                ingested_at: Utc::now(),
            });
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let mut ticks = Vec::new();
        while let Ok(gen) = tick_rx.try_recv() {
            ticks.push(gen);
        }

        assert!(
            !ticks.is_empty(),
            "worker should have produced at least one tick"
        );
        let max_gen = *ticks.iter().max().unwrap();
        assert_eq!(
            max_gen, 5,
            "final generation should be 5 (one per TradesIngested)"
        );
        assert!(
            ticks.len() <= 5,
            "coalescing should produce <= events count ticks"
        );

        sub_handle.abort();
        worker_handle.abort();
    }

    #[tokio::test]
    async fn test_fast_path_worker_sends_generation_on_tick() {
        let bus = Arc::new(EventBus::new(16));
        let (tick_tx, mut tick_rx) = mpsc::channel::<u64>(16);

        let worker_bus = bus.clone();
        let worker_handle = tokio::spawn(async move {
            spawn_fast_path_worker(worker_bus, tick_tx).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        bus.trigger_fast_path();

        let gen = tokio::time::timeout(tokio::time::Duration::from_millis(200), tick_rx.recv())
            .await
            .expect("timed out waiting for tick")
            .expect("tick channel closed");

        assert_eq!(gen, 1);
        worker_handle.abort();
    }

    #[tokio::test]
    async fn test_fast_path_worker_shuts_down_when_receiver_dropped() {
        let bus = Arc::new(EventBus::new(16));
        let (tick_tx, tick_rx) = mpsc::channel::<u64>(16);

        let worker_bus = bus.clone();
        let worker_handle = tokio::spawn(async move {
            spawn_fast_path_worker(worker_bus, tick_tx).await;
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        drop(tick_rx);

        bus.trigger_fast_path();

        let result =
            tokio::time::timeout(tokio::time::Duration::from_millis(500), worker_handle).await;

        assert!(
            result.is_ok(),
            "worker should shut down when receiver is dropped"
        );
    }

    // ── TradesIngestedAccumulator unit tests ──

    #[test]
    fn test_accumulator_new_is_empty() {
        let acc = TradesIngestedAccumulator::new();
        assert!(acc.is_empty());
        assert_eq!(acc.len(), 0);
    }

    #[test]
    fn test_accumulator_add_wallet_increases_count() {
        let mut acc = TradesIngestedAccumulator::new();
        acc.add_wallet("0xabc".to_string());
        assert_eq!(acc.len(), 1);
        assert!(!acc.is_empty());
    }

    #[test]
    fn test_accumulator_deduplicates_wallets() {
        let mut acc = TradesIngestedAccumulator::new();
        acc.add_wallet("0xabc".to_string());
        acc.add_wallet("0xabc".to_string());
        acc.add_wallet("0xdef".to_string());
        assert_eq!(acc.len(), 2);
    }

    #[test]
    fn test_accumulator_drain_returns_wallets_and_resets() {
        let mut acc = TradesIngestedAccumulator::new();
        acc.add_wallet("0xabc".to_string());
        acc.add_wallet("0xdef".to_string());

        let drained = acc.drain();
        assert_eq!(drained.len(), 2);
        assert!(drained.contains("0xabc"));
        assert!(drained.contains("0xdef"));

        // After drain, accumulator is empty
        assert!(acc.is_empty());
        assert_eq!(acc.len(), 0);
    }

    // ── Classification trigger subscriber integration tests ──

    #[tokio::test]
    async fn test_classification_trigger_accumulates_trades_ingested_events() {
        let bus = Arc::new(EventBus::new(16));
        let (classification_tx, mut classification_rx) = mpsc::channel::<()>(8);

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_classification_trigger_subscriber(
                subscriber_bus,
                classification_tx,
                Duration::from_millis(100),
            )
            .await;
        });

        // Give subscriber time to start and consume the initial tick
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Publish TradesIngested events for different wallets
        let _ = bus.publish_pipeline(PipelineEvent::TradesIngested {
            wallet_address: "0xwallet1".to_string(),
            trades_count: 5,
            ingested_at: Utc::now(),
        });
        let _ = bus.publish_pipeline(PipelineEvent::TradesIngested {
            wallet_address: "0xwallet2".to_string(),
            trades_count: 3,
            ingested_at: Utc::now(),
        });

        // Wait for batch window to fire
        let result =
            tokio::time::timeout(Duration::from_millis(200), classification_rx.recv()).await;

        assert!(
            result.is_ok(),
            "Should have received classification trigger"
        );
        assert!(result.unwrap().is_some(), "Channel should not be closed");

        handle.abort();
    }

    #[tokio::test]
    async fn test_classification_trigger_does_not_fire_when_no_events() {
        let bus = Arc::new(EventBus::new(16));
        let (classification_tx, mut classification_rx) = mpsc::channel::<()>(8);

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_classification_trigger_subscriber(
                subscriber_bus,
                classification_tx,
                Duration::from_millis(50),
            )
            .await;
        });

        // Wait for more than one batch window without publishing any events
        let result =
            tokio::time::timeout(Duration::from_millis(150), classification_rx.recv()).await;

        // Should timeout -- no trigger because no events were accumulated
        assert!(
            result.is_err(),
            "Should not receive trigger when no events accumulated"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn test_classification_trigger_ignores_non_trades_events() {
        let bus = Arc::new(EventBus::new(16));
        let (classification_tx, mut classification_rx) = mpsc::channel::<()>(8);

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_classification_trigger_subscriber(
                subscriber_bus,
                classification_tx,
                Duration::from_millis(100),
            )
            .await;
        });

        // Give subscriber time to start
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Publish non-TradesIngested events only
        let _ = bus.publish_pipeline(PipelineEvent::MarketsScored {
            markets_scored: 100,
            events_ranked: 50,
            completed_at: Utc::now(),
        });
        let _ = bus.publish_pipeline(PipelineEvent::WalletsDiscovered {
            market_id: "market1".to_string(),
            wallets_added: 10,
            discovered_at: Utc::now(),
        });

        // Wait for batch window
        let result =
            tokio::time::timeout(Duration::from_millis(200), classification_rx.recv()).await;

        // Should timeout -- non-TradesIngested events should not trigger classification
        assert!(
            result.is_err(),
            "Should not trigger classification for non-trades events"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn test_classification_trigger_deduplicates_within_batch() {
        let bus = Arc::new(EventBus::new(16));
        let (classification_tx, mut classification_rx) = mpsc::channel::<()>(8);

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_classification_trigger_subscriber(
                subscriber_bus,
                classification_tx,
                Duration::from_millis(100),
            )
            .await;
        });

        // Give subscriber time to start
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Publish duplicate wallet addresses
        for _ in 0..5 {
            let _ = bus.publish_pipeline(PipelineEvent::TradesIngested {
                wallet_address: "0xsame_wallet".to_string(),
                trades_count: 1,
                ingested_at: Utc::now(),
            });
        }

        // Wait for batch trigger
        let result =
            tokio::time::timeout(Duration::from_millis(200), classification_rx.recv()).await;

        // Should still trigger (at least 1 wallet accumulated)
        assert!(result.is_ok(), "Should trigger even with duplicate wallets");

        handle.abort();
    }

    #[tokio::test]
    async fn test_classification_trigger_batches_across_window() {
        let bus = Arc::new(EventBus::new(16));
        let (classification_tx, mut classification_rx) = mpsc::channel::<()>(8);

        let subscriber_bus = bus.clone();
        let handle = tokio::spawn(async move {
            spawn_classification_trigger_subscriber(
                subscriber_bus,
                classification_tx,
                Duration::from_millis(150),
            )
            .await;
        });

        // Give subscriber time to start
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Publish first event
        let _ = bus.publish_pipeline(PipelineEvent::TradesIngested {
            wallet_address: "0xwallet1".to_string(),
            trades_count: 5,
            ingested_at: Utc::now(),
        });

        // Wait a bit (less than batch window) then publish another
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = bus.publish_pipeline(PipelineEvent::TradesIngested {
            wallet_address: "0xwallet2".to_string(),
            trades_count: 3,
            ingested_at: Utc::now(),
        });

        // First trigger should come after the batch window
        let result =
            tokio::time::timeout(Duration::from_millis(200), classification_rx.recv()).await;
        assert!(result.is_ok(), "Should receive first batch trigger");

        // After the trigger, accumulator should be drained.
        // Publish a new event and wait for next window.
        let _ = bus.publish_pipeline(PipelineEvent::TradesIngested {
            wallet_address: "0xwallet3".to_string(),
            trades_count: 2,
            ingested_at: Utc::now(),
        });

        let result2 =
            tokio::time::timeout(Duration::from_millis(250), classification_rx.recv()).await;
        assert!(
            result2.is_ok(),
            "Should receive second batch trigger for new wallet"
        );

        handle.abort();
    }
}
