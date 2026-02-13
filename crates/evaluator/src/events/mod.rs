//! Event type definitions for event-driven pipeline coordination.
//!
//! This module defines three categories of events:
//! - Pipeline events: Job completion signals (pub/sub via broadcast)
//! - Fast-path events: Latency-critical triggers (coalescing via watch)
//! - Operational events: Monitoring and observability

pub mod subscribers;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Pipeline events signal job completion and trigger downstream work.
/// Distributed via broadcast channels (multi-subscriber pub/sub).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineEvent {
    /// Event scoring completed, top 50 events available for discovery
    MarketsScored {
        /// Number of markets scored
        markets_scored: u64,
        /// Number of events ranked
        events_ranked: u64,
        /// Timestamp when scoring completed
        completed_at: DateTime<Utc>,
    },

    /// New wallets discovered and added to watchlist
    WalletsDiscovered {
        /// Market that triggered discovery
        market_id: String,
        /// Number of new wallets added
        wallets_added: u64,
        /// Timestamp of discovery
        discovered_at: DateTime<Utc>,
    },

    /// Trades fetched for tracked wallets
    TradesIngested {
        /// Wallet address
        wallet_address: String,
        /// Number of new trades ingested
        trades_count: u64,
        /// Timestamp when ingestion completed
        ingested_at: DateTime<Utc>,
    },

    /// Personas computed for wallets
    WalletsClassified {
        /// Number of wallets classified
        wallets_classified: u64,
        /// Timestamp when classification completed
        classified_at: DateTime<Utc>,
    },

    /// Wallet state transitions evaluated (Candidate → PaperTrading, etc.)
    WalletRulesEvaluated {
        /// Number of wallets evaluated
        wallets_evaluated: u64,
        /// Number of state transitions
        transitions: u64,
        /// Timestamp when evaluation completed
        evaluated_at: DateTime<Utc>,
    },
}

/// Fast-path events use coalescing (watch channel) for latency-critical work.
/// Only the latest generation is tracked; intermediate triggers are collapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FastPathTrigger {
    /// Generation counter (increments on each trigger)
    pub generation: u64,
}

/// Operational events for monitoring and observability.
/// Distributed via broadcast channels (multi-subscriber pub/sub).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OperationalEvent {
    /// Job started
    JobStarted {
        job_name: String,
        started_at: DateTime<Utc>,
    },

    /// Job completed successfully
    JobCompleted {
        job_name: String,
        duration_ms: u64,
        completed_at: DateTime<Utc>,
    },

    /// Job failed with error
    JobFailed {
        job_name: String,
        error: String,
        failed_at: DateTime<Utc>,
    },

    /// Queue approaching capacity
    BackpressureWarning {
        queue_name: String,
        current_size: usize,
        capacity: usize,
        warned_at: DateTime<Utc>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_pipeline_event_markets_scored_serialization() {
        let event = PipelineEvent::MarketsScored {
            markets_scored: 100,
            events_ranked: 50,
            completed_at: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: PipelineEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_pipeline_event_wallets_discovered_serialization() {
        let event = PipelineEvent::WalletsDiscovered {
            market_id: "test-market".to_string(),
            wallets_added: 10,
            discovered_at: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: PipelineEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_pipeline_event_trades_ingested_serialization() {
        let event = PipelineEvent::TradesIngested {
            wallet_address: "0xabc123".to_string(),
            trades_count: 5,
            ingested_at: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: PipelineEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_pipeline_event_wallets_classified_serialization() {
        let event = PipelineEvent::WalletsClassified {
            wallets_classified: 25,
            classified_at: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: PipelineEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_pipeline_event_wallet_rules_evaluated_serialization() {
        let event = PipelineEvent::WalletRulesEvaluated {
            wallets_evaluated: 30,
            transitions: 5,
            evaluated_at: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: PipelineEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_fast_path_trigger_default() {
        let trigger = FastPathTrigger::default();
        assert_eq!(trigger.generation, 0);
    }

    #[test]
    fn test_fast_path_trigger_increments() {
        let mut trigger = FastPathTrigger::default();
        trigger.generation += 1;
        assert_eq!(trigger.generation, 1);

        trigger.generation += 1;
        assert_eq!(trigger.generation, 2);
    }

    #[test]
    fn test_operational_event_job_started_serialization() {
        let event = OperationalEvent::JobStarted {
            job_name: "test_job".to_string(),
            started_at: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: OperationalEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_operational_event_backpressure_serialization() {
        let event = OperationalEvent::BackpressureWarning {
            queue_name: "wallet_queue".to_string(),
            current_size: 900,
            capacity: 1000,
            warned_at: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: OperationalEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_pipeline_event_json_format() {
        let event = PipelineEvent::MarketsScored {
            markets_scored: 100,
            events_ranked: 50,
            completed_at: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"markets_scored"#));
        assert!(json.contains(r#""markets_scored":100"#));
    }
}
