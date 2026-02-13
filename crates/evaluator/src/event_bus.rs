//! In-process EventBus for coordinating pipeline jobs via pub/sub and coalescing triggers.
//!
//! # Architecture Decision: broadcast + watch hybrid
//!
//! We use two Tokio channel types rather than a single pub/sub mechanism:
//!
//! - **`broadcast`** for pipeline and operational events: Every subscriber gets every
//!   event. Needed for logging (sees all events), discovery triggers (reacts to
//!   MarketsScored), and classification triggers (accumulates TradesIngested).
//!
//! - **`watch`** for fast-path triggers: Only the latest generation matters. Multiple
//!   TradesIngested events arriving in rapid succession coalesce into a single downstream
//!   reaction. This prevents thundering-herd problems in paper trading.
//!
//! We chose in-process channels over an external broker (Redis, SQS) because the evaluator
//! is a single-process system. The same event types can later back a network bus if we split
//! into multiple processes.
//!
//! # Communication patterns
//!
//! - **Pipeline events:** Multi-subscriber broadcast for job completion signals
//! - **Fast-path triggers:** Coalescing watch channel for latency-critical work
//! - **Operational events:** Multi-subscriber broadcast for monitoring
//!
//! See `docs/EVENT_ARCHITECTURE.md` for the full architecture reference.

use crate::events::{FastPathTrigger, OperationalEvent, PipelineEvent};
use chrono::Utc;
use tokio::sync::{broadcast, watch};

/// Policy applied when a broadcast channel is at capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(dead_code)] // Phase 4: Variants used via with_backpressure_policy(), production wiring in Phase 5+
pub enum BackpressurePolicy {
    /// Drop the oldest event in the buffer to make room (default broadcast behavior).
    #[default]
    DropOldest,
    /// Drop the newest event (the one being published) when the buffer is full.
    DropNewest,
}

/// EventBus coordinates job execution via typed events.
#[derive(Clone)]
#[allow(dead_code)] // Phase 1: Infrastructure only, will be used in Phase 2+
pub struct EventBus {
    /// Multi-subscriber pub/sub for pipeline events (job completion signals)
    pipeline_tx: broadcast::Sender<PipelineEvent>,

    /// Coalescing wakeup for fast-path triggers (latest generation wins)
    fast_path_tx: watch::Sender<FastPathTrigger>,

    /// Multi-subscriber pub/sub for operational events (monitoring)
    operational_tx: broadcast::Sender<OperationalEvent>,

    /// Channel capacity (stored for backpressure threshold calculations)
    capacity: usize,

    /// Backpressure policy for pipeline events
    pipeline_backpressure: BackpressurePolicy,

    /// Threshold percentage [0, 100] at which to emit BackpressureWarning
    warn_threshold_pct: u8,
}

#[allow(dead_code)] // Phase 1: Infrastructure only, will be used in Phase 2+
impl EventBus {
    /// Creates a new EventBus with specified channel capacity.
    ///
    /// # Arguments
    /// * `capacity` - Buffer size for broadcast channels (pipeline and operational events)
    pub fn new(capacity: usize) -> Self {
        let (pipeline_tx, _) = broadcast::channel(capacity);
        let (fast_path_tx, _) = watch::channel(FastPathTrigger::default());
        let (operational_tx, _) = broadcast::channel(capacity);

        Self {
            pipeline_tx,
            fast_path_tx,
            operational_tx,
            capacity,
            pipeline_backpressure: BackpressurePolicy::default(),
            warn_threshold_pct: 90,
        }
    }

    /// Sets the backpressure policy for pipeline events.
    pub fn with_backpressure_policy(mut self, policy: BackpressurePolicy) -> Self {
        self.pipeline_backpressure = policy;
        self
    }

    /// Sets the warning threshold percentage (0-100) for backpressure warnings.
    pub fn with_warn_threshold_pct(mut self, pct: u8) -> Self {
        self.warn_threshold_pct = pct;
        self
    }

    /// Returns the configured backpressure policy.
    pub fn backpressure_policy(&self) -> BackpressurePolicy {
        self.pipeline_backpressure
    }

    /// Returns the current number of queued pipeline events.
    pub fn pipeline_len(&self) -> usize {
        self.pipeline_tx.len()
    }

    /// Returns the channel capacity.
    pub fn pipeline_capacity(&self) -> usize {
        self.capacity
    }

    /// Publishes a pipeline event, applying the configured backpressure policy.
    ///
    /// - **DropOldest**: Default broadcast behavior. Oldest events are overwritten when full.
    /// - **DropNewest**: If the channel is at capacity, the new event is silently dropped.
    /// - **Block**: Uses default broadcast send (which overwrites oldest in tokio broadcast).
    ///
    /// Emits `OperationalEvent::BackpressureWarning` when queue fill exceeds the threshold.
    pub fn publish_pipeline(
        &self,
        event: PipelineEvent,
    ) -> Result<usize, broadcast::error::SendError<PipelineEvent>> {
        // Check fill level before sending
        let current_len = self.pipeline_tx.len();
        let threshold = (self.capacity as u64 * u64::from(self.warn_threshold_pct) / 100) as usize;

        if current_len >= threshold && threshold > 0 {
            // Emit backpressure warning on the operational channel
            let _ = self
                .operational_tx
                .send(OperationalEvent::BackpressureWarning {
                    queue_name: "pipeline".to_string(),
                    current_size: current_len,
                    capacity: self.capacity,
                    warned_at: Utc::now(),
                });
        }

        match self.pipeline_backpressure {
            BackpressurePolicy::DropOldest => {
                // Default tokio broadcast behavior: oldest messages are overwritten
                self.pipeline_tx.send(event)
            }
            BackpressurePolicy::DropNewest => {
                if current_len >= self.capacity {
                    // Channel is full: drop the new event (return Ok(0) to indicate no receivers got it)
                    tracing::warn!(
                        current_len,
                        capacity = self.capacity,
                        "Backpressure: dropping newest pipeline event (channel full)"
                    );
                    Ok(0)
                } else {
                    self.pipeline_tx.send(event)
                }
            }
        }
    }

    /// Subscribes to pipeline events.
    ///
    /// Returns a receiver that will receive all pipeline events published after subscription.
    pub fn subscribe_pipeline(&self) -> broadcast::Receiver<PipelineEvent> {
        self.pipeline_tx.subscribe()
    }

    /// Triggers fast-path processing by incrementing the generation counter.
    ///
    /// Multiple triggers are coalesced - only the latest generation is tracked.
    pub fn trigger_fast_path(&self) {
        self.fast_path_tx.send_modify(|trigger| {
            trigger.generation += 1;
        });
    }

    /// Subscribes to fast-path triggers.
    ///
    /// Returns a watch receiver that coalesces triggers (only latest generation matters).
    pub fn subscribe_fast_path(&self) -> watch::Receiver<FastPathTrigger> {
        self.fast_path_tx.subscribe()
    }

    /// Publishes an operational event to all subscribers.
    ///
    /// Returns an error if there are no active subscribers.
    pub fn publish_operational(
        &self,
        event: OperationalEvent,
    ) -> Result<usize, broadcast::error::SendError<OperationalEvent>> {
        self.operational_tx.send(event)
    }

    /// Subscribes to operational events.
    ///
    /// Returns a receiver that will receive all operational events published after subscription.
    pub fn subscribe_operational(&self) -> broadcast::Receiver<OperationalEvent> {
        self.operational_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{OperationalEvent, PipelineEvent};
    use chrono::Utc;

    #[tokio::test]
    async fn test_event_bus_publishes_pipeline_events_to_subscribers() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe_pipeline();

        let event = PipelineEvent::MarketsScored {
            markets_scored: 100,
            events_ranked: 50,
            completed_at: Utc::now(),
        };

        bus.publish_pipeline(event.clone()).unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn test_event_bus_supports_multiple_pipeline_subscribers() {
        let bus = EventBus::new(16);
        let mut rx1 = bus.subscribe_pipeline();
        let mut rx2 = bus.subscribe_pipeline();

        let event = PipelineEvent::WalletsDiscovered {
            market_id: "test-market".to_string(),
            wallets_added: 10,
            discovered_at: Utc::now(),
        };

        bus.publish_pipeline(event.clone()).unwrap();

        // Both subscribers receive the same event
        let received1 = rx1.recv().await.unwrap();
        let received2 = rx2.recv().await.unwrap();

        assert_eq!(received1, event);
        assert_eq!(received2, event);
    }

    #[tokio::test]
    async fn test_event_bus_fast_path_coalesces_triggers() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe_fast_path();

        // Trigger multiple times
        bus.trigger_fast_path();
        bus.trigger_fast_path();
        bus.trigger_fast_path();

        // Wait for notification (coalesced)
        rx.changed().await.unwrap();

        // Generation counter should be 3 (all triggers counted)
        assert_eq!(rx.borrow().generation, 3);
    }

    #[tokio::test]
    async fn test_event_bus_fast_path_starts_at_generation_zero() {
        let bus = EventBus::new(16);
        let rx = bus.subscribe_fast_path();

        assert_eq!(rx.borrow().generation, 0);
    }

    #[tokio::test]
    async fn test_event_bus_publishes_operational_events() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe_operational();

        let event = OperationalEvent::JobStarted {
            job_name: "test_job".to_string(),
            started_at: Utc::now(),
        };

        bus.publish_operational(event.clone()).unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn test_event_bus_returns_error_when_no_subscribers() {
        let bus = EventBus::new(16);

        // No subscribers, so publish should fail with SendError
        let event = PipelineEvent::MarketsScored {
            markets_scored: 100,
            events_ranked: 50,
            completed_at: Utc::now(),
        };

        let result = bus.publish_pipeline(event);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_event_bus_pipeline_lagged_subscriber_receives_missed_events() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe_pipeline();

        // Publish multiple events quickly
        for i in 0..5 {
            let event = PipelineEvent::TradesIngested {
                wallet_address: format!("0xwallet{i}"),
                trades_count: i,
                ingested_at: Utc::now(),
            };
            bus.publish_pipeline(event).unwrap();
        }

        // Subscriber should receive all events (up to buffer size 16)
        for i in 0..5 {
            let received = rx.recv().await.unwrap();
            match received {
                PipelineEvent::TradesIngested { trades_count, .. } => {
                    assert_eq!(trades_count, i);
                }
                _ => panic!("Expected TradesIngested event"),
            }
        }
    }

    // ── Backpressure policy tests ──

    #[test]
    fn test_backpressure_policy_default_is_drop_oldest() {
        let bus = EventBus::new(16);
        assert_eq!(bus.backpressure_policy(), BackpressurePolicy::DropOldest);
    }

    #[test]
    fn test_with_backpressure_policy_sets_policy() {
        let bus = EventBus::new(16).with_backpressure_policy(BackpressurePolicy::DropNewest);
        assert_eq!(bus.backpressure_policy(), BackpressurePolicy::DropNewest);

        let bus = EventBus::new(16).with_backpressure_policy(BackpressurePolicy::DropOldest);
        assert_eq!(bus.backpressure_policy(), BackpressurePolicy::DropOldest);
    }

    #[tokio::test]
    async fn test_drop_oldest_policy_overwrites_old_events_when_full() {
        // Capacity of 4: fill it, then send one more. The oldest should be gone.
        let bus = EventBus::new(4).with_backpressure_policy(BackpressurePolicy::DropOldest);
        let mut rx = bus.subscribe_pipeline();

        // Fill the buffer with 4 events (capacity)
        for i in 0..4u64 {
            bus.publish_pipeline(PipelineEvent::TradesIngested {
                wallet_address: format!("0xwallet{i}"),
                trades_count: i,
                ingested_at: Utc::now(),
            })
            .unwrap();
        }

        // Send a 5th event, which should cause the oldest (i=0) to be dropped
        bus.publish_pipeline(PipelineEvent::TradesIngested {
            wallet_address: "0xwallet4".to_string(),
            trades_count: 4,
            ingested_at: Utc::now(),
        })
        .unwrap();

        // The receiver should get a Lagged error since event 0 was overwritten
        let result = rx.recv().await;
        match result {
            Err(broadcast::error::RecvError::Lagged(n)) => {
                assert!(n >= 1, "should have lagged by at least 1 event");
            }
            Ok(event) => {
                // If we get an event, it should NOT be the first one (i=0)
                match &event {
                    PipelineEvent::TradesIngested { trades_count, .. } => {
                        assert!(
                            *trades_count > 0,
                            "oldest event (0) should have been dropped"
                        );
                    }
                    _ => panic!("Expected TradesIngested"),
                }
            }
            Err(e) => panic!("Unexpected error: {e:?}"),
        }
    }

    #[tokio::test]
    async fn test_drop_newest_policy_drops_new_event_when_full() {
        // Capacity of 4: fill buffer, then try to add one more.
        // With DropNewest, the new event should be silently dropped.
        let bus = EventBus::new(4).with_backpressure_policy(BackpressurePolicy::DropNewest);
        let mut rx = bus.subscribe_pipeline();

        // Fill the buffer
        for i in 0..4u64 {
            let result = bus.publish_pipeline(PipelineEvent::TradesIngested {
                wallet_address: format!("0xwallet{i}"),
                trades_count: i,
                ingested_at: Utc::now(),
            });
            assert!(result.is_ok());
        }

        // This 5th event should be dropped (returns Ok(0))
        let result = bus.publish_pipeline(PipelineEvent::TradesIngested {
            wallet_address: "0xwallet_dropped".to_string(),
            trades_count: 999,
            ingested_at: Utc::now(),
        });
        assert_eq!(
            result.unwrap(),
            0,
            "DropNewest should return 0 receivers when event is dropped"
        );

        // We should be able to read all 4 original events without error
        for i in 0..4u64 {
            let received = rx.recv().await.unwrap();
            match received {
                PipelineEvent::TradesIngested { trades_count, .. } => {
                    assert_eq!(trades_count, i);
                }
                _ => panic!("Expected TradesIngested"),
            }
        }
    }

    #[tokio::test]
    async fn test_backpressure_warning_emitted_at_threshold() {
        // Capacity 10, threshold 90% = warn when len >= 9 at time of next publish.
        // The check happens BEFORE the send, so we need 9 items already in the
        // buffer when we call publish_pipeline for the 10th event.
        let bus = EventBus::new(10)
            .with_backpressure_policy(BackpressurePolicy::DropOldest)
            .with_warn_threshold_pct(90);

        let _pipeline_rx = bus.subscribe_pipeline();
        let mut operational_rx = bus.subscribe_operational();

        // Fill to 9 events (90% fill). During these sends, the check sees
        // len 0..8, all below threshold of 9, so no warning yet.
        for i in 0..9u64 {
            bus.publish_pipeline(PipelineEvent::TradesIngested {
                wallet_address: format!("0xwallet{i}"),
                trades_count: i,
                ingested_at: Utc::now(),
            })
            .unwrap();
        }

        // No warning should have been emitted yet (max len seen was 8 < 9)
        let warn_result = operational_rx.try_recv();
        assert!(
            warn_result.is_err(),
            "Should not emit warning while filling to threshold"
        );

        // Send 10th event: len is now 9, which equals threshold -> warning emitted
        bus.publish_pipeline(PipelineEvent::TradesIngested {
            wallet_address: "0xwallet9".to_string(),
            trades_count: 9,
            ingested_at: Utc::now(),
        })
        .unwrap();

        let warning = operational_rx.try_recv();
        assert!(
            warning.is_ok(),
            "Should emit BackpressureWarning when queue fill >= 90%"
        );
        match warning.unwrap() {
            OperationalEvent::BackpressureWarning {
                queue_name,
                current_size,
                capacity,
                ..
            } => {
                assert_eq!(queue_name, "pipeline");
                assert_eq!(current_size, 9);
                assert_eq!(capacity, 10);
            }
            other => panic!("Expected BackpressureWarning, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_backpressure_warning_not_emitted_below_threshold() {
        // Capacity 10, threshold 90% -- send only 5 events (50%)
        let bus = EventBus::new(10)
            .with_backpressure_policy(BackpressurePolicy::DropOldest)
            .with_warn_threshold_pct(90);

        let _pipeline_rx = bus.subscribe_pipeline();
        let mut operational_rx = bus.subscribe_operational();

        for i in 0..5u64 {
            bus.publish_pipeline(PipelineEvent::TradesIngested {
                wallet_address: format!("0xwallet{i}"),
                trades_count: i,
                ingested_at: Utc::now(),
            })
            .unwrap();
        }

        let result = operational_rx.try_recv();
        assert!(
            result.is_err(),
            "Should not emit warning when fill is only 50%"
        );
    }

    #[test]
    fn test_pipeline_len_tracks_queued_events() {
        let bus = EventBus::new(16);
        let _rx = bus.subscribe_pipeline();

        assert_eq!(bus.pipeline_len(), 0);

        bus.publish_pipeline(PipelineEvent::MarketsScored {
            markets_scored: 1,
            events_ranked: 1,
            completed_at: Utc::now(),
        })
        .unwrap();

        assert_eq!(bus.pipeline_len(), 1);
    }

    #[test]
    fn test_pipeline_capacity_returns_configured_capacity() {
        let bus = EventBus::new(32);
        assert_eq!(bus.pipeline_capacity(), 32);
    }

    #[test]
    fn test_with_warn_threshold_pct_sets_threshold() {
        let bus = EventBus::new(16).with_warn_threshold_pct(75);
        assert_eq!(bus.warn_threshold_pct, 75);
    }
}
