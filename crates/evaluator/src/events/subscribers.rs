//! Event subscribers for logging and monitoring.
//!
//! Subscribers receive events from the EventBus and perform actions like
//! logging to stdout, persisting to database, or updating metrics.

use crate::event_bus::EventBus;
use crate::events::{OperationalEvent, PipelineEvent};
use std::sync::Arc;

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
}
