//! EventBus for coordinating jobs via pub/sub and coalescing triggers.
//!
//! Provides three communication patterns:
//! - Pipeline events: Multi-subscriber broadcast for job completion signals
//! - Fast-path triggers: Coalescing watch channel for latency-critical work
//! - Operational events: Multi-subscriber broadcast for monitoring

use crate::events::{FastPathTrigger, OperationalEvent, PipelineEvent};
use tokio::sync::{broadcast, watch};

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
        }
    }

    /// Publishes a pipeline event to all subscribers.
    ///
    /// Returns an error if there are no active subscribers.
    pub fn publish_pipeline(
        &self,
        event: PipelineEvent,
    ) -> Result<usize, broadcast::error::SendError<PipelineEvent>> {
        self.pipeline_tx.send(event)
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
}
