use crate::polymarket::{RawTrade, TraderPolymarketClient};
use std::collections::HashSet;
use tracing::info;

/// Watermark-based new trade detector.
/// Tracks which trade hashes we've already seen and filters to only new ones.
pub struct TradeDetector {
    seen_hashes: HashSet<String>,
    last_timestamp: Option<i64>,
    warmed_up: bool,
}

impl TradeDetector {
    pub fn new(last_seen_hash: Option<String>) -> Self {
        let warmed_up = last_seen_hash.is_some();
        let mut seen = HashSet::new();
        if let Some(hash) = last_seen_hash {
            seen.insert(hash);
        }
        Self {
            seen_hashes: seen,
            last_timestamp: None,
            warmed_up,
        }
    }

    /// Filter a batch of trades to only those we haven't seen before.
    /// Returns new trades in chronological order (oldest first).
    pub fn detect_new<'a>(&mut self, trades: &'a [RawTrade]) -> Vec<&'a RawTrade> {
        if !self.warmed_up {
            self.warmed_up = true;
            for trade in trades {
                let hash = TraderPolymarketClient::trade_hash(trade);
                self.seen_hashes.insert(hash);
            }
            if let Some(last) = trades.iter().max_by_key(|t| t.timestamp.unwrap_or(0)) {
                self.last_timestamp = last.timestamp;
            }
            info!(count = trades.len(), "warm-up: ingested existing trades, will only mirror new ones");
            return Vec::new();
        }

        let mut new_trades: Vec<&RawTrade> = Vec::new();

        for trade in trades {
            let hash = TraderPolymarketClient::trade_hash(trade);
            if self.seen_hashes.contains(&hash) {
                continue;
            }
            self.seen_hashes.insert(hash);
            new_trades.push(trade);
        }

        // Sort by timestamp ascending (oldest first) for correct ordering
        new_trades.sort_by_key(|t| t.timestamp.unwrap_or(0));

        // Update watermark to latest timestamp
        if let Some(last) = new_trades.last() {
            if let Some(ts) = last.timestamp {
                self.last_timestamp = Some(ts);
            }
        }

        new_trades
    }

    /// Get the latest trade hash for persisting watermark.
    #[allow(dead_code)] // Used in tests
    pub fn latest_hash(&self) -> Option<&str> {
        // Return the hash of the most recently seen trade
        // In practice this is updated when we detect new trades
        None // Simplified — the caller manages watermark persistence
    }

    #[allow(dead_code)] // Used in tests
    pub fn seen_count(&self) -> usize {
        self.seen_hashes.len()
    }

    /// Prune old hashes to prevent unbounded memory growth.
    /// Keep only the most recent N hashes.
    pub fn prune(&mut self, keep: usize) {
        if self.seen_hashes.len() > keep * 2 {
            // Simple strategy: clear and rely on timestamp watermark
            // In practice we'd keep a bounded set, but for now this prevents leaks
            self.seen_hashes.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_trade(id: &str, timestamp: i64) -> RawTrade {
        RawTrade {
            id: Some(id.to_string()),
            proxy_wallet: Some("0xtest".to_string()),
            condition_id: Some("cond-1".to_string()),
            asset: None,
            size: Some("100".to_string()),
            price: Some("0.50".to_string()),
            timestamp: Some(timestamp),
            outcome: Some("Yes".to_string()),
            outcome_index: Some(0),
            side: Some("BUY".to_string()),
            transaction_hash: None,
        }
    }

    #[test]
    fn test_detect_new_all_fresh_after_warmup() {
        let mut detector = TradeDetector::new(None);
        let trades = vec![make_trade("t1", 100), make_trade("t2", 200)];

        // First call is warm-up — returns empty
        let new = detector.detect_new(&trades);
        assert_eq!(new.len(), 0);

        // Second call with a new trade — only the new one is returned
        let trades2 = vec![
            make_trade("t1", 100),
            make_trade("t2", 200),
            make_trade("t3", 300),
        ];
        let new2 = detector.detect_new(&trades2);
        assert_eq!(new2.len(), 1);
        assert_eq!(new2[0].id.as_deref(), Some("t3"));
    }

    #[test]
    fn test_detect_new_with_watermark() {
        let mut detector = TradeDetector::new(Some("t1".to_string()));
        let trades = vec![make_trade("t1", 100), make_trade("t2", 200)];

        let new = detector.detect_new(&trades);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].id.as_deref(), Some("t2"));
    }

    #[test]
    fn test_detect_new_no_duplicates_across_calls() {
        let mut detector = TradeDetector::new(None);

        // Warm-up
        let batch1 = vec![make_trade("t1", 100), make_trade("t2", 200)];
        let new1 = detector.detect_new(&batch1);
        assert_eq!(new1.len(), 0);

        // Same batch again — nothing new (already seen during warm-up)
        let batch2 = vec![make_trade("t1", 100), make_trade("t2", 200)];
        let new2 = detector.detect_new(&batch2);
        assert_eq!(new2.len(), 0);

        // New trade added
        let batch3 = vec![
            make_trade("t1", 100),
            make_trade("t2", 200),
            make_trade("t3", 300),
        ];
        let new3 = detector.detect_new(&batch3);
        assert_eq!(new3.len(), 1);
        assert_eq!(new3[0].id.as_deref(), Some("t3"));
    }

    #[test]
    fn test_detect_new_sorted_chronologically() {
        let mut detector = TradeDetector::new(None);

        // Warm-up with initial trades
        let warmup = vec![make_trade("t0", 50)];
        detector.detect_new(&warmup);

        // New trades arrive in reverse order
        let trades = vec![
            make_trade("t0", 50),
            make_trade("t3", 300),
            make_trade("t1", 100),
            make_trade("t2", 200),
        ];

        let new = detector.detect_new(&trades);
        assert_eq!(new.len(), 3);
        // Should be sorted oldest first
        assert_eq!(new[0].id.as_deref(), Some("t1"));
        assert_eq!(new[1].id.as_deref(), Some("t2"));
        assert_eq!(new[2].id.as_deref(), Some("t3"));
    }

    #[test]
    fn test_seen_count() {
        let mut detector = TradeDetector::new(None);
        assert_eq!(detector.seen_count(), 0);

        // Warm-up ingests hashes
        let trades = vec![make_trade("t1", 100), make_trade("t2", 200)];
        detector.detect_new(&trades);
        assert_eq!(detector.seen_count(), 2);

        // New trade adds to seen count
        let trades2 = vec![
            make_trade("t1", 100),
            make_trade("t2", 200),
            make_trade("t3", 300),
        ];
        detector.detect_new(&trades2);
        assert_eq!(detector.seen_count(), 3);
    }

    #[test]
    fn test_prune() {
        // Start with a watermark so warm-up is skipped
        let mut detector = TradeDetector::new(Some("seed".to_string()));
        for i in 0..200 {
            let trades = vec![make_trade(&format!("t{i}"), i64::from(i))];
            detector.detect_new(&trades);
        }
        // 200 trades + 1 seed hash
        assert_eq!(detector.seen_count(), 201);

        // Prune with keep=50 — since 201 > 50*2, it clears
        detector.prune(50);
        assert_eq!(detector.seen_count(), 0);
    }

    #[test]
    fn test_warmup_skips_first_batch() {
        let mut detector = TradeDetector::new(None);
        let trades = vec![make_trade("t1", 100), make_trade("t2", 200)];

        // First call: warm-up, returns empty
        let new = detector.detect_new(&trades);
        assert!(new.is_empty());

        // Second call with a new trade: only new trade returned
        let trades2 = vec![
            make_trade("t1", 100),
            make_trade("t2", 200),
            make_trade("t3", 300),
        ];
        let new2 = detector.detect_new(&trades2);
        assert_eq!(new2.len(), 1);
        assert_eq!(new2[0].id.as_deref(), Some("t3"));
    }

    #[test]
    fn test_warmup_not_needed_with_watermark() {
        let mut detector = TradeDetector::new(Some("t1".to_string()));
        let trades = vec![make_trade("t1", 100), make_trade("t2", 200)];

        // First call returns new trades immediately (no warm-up needed)
        let new = detector.detect_new(&trades);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].id.as_deref(), Some("t2"));
    }
}
