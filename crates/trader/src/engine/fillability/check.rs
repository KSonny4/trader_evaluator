use crate::types::Side;
use serde::{Deserialize, Serialize};

/// A single price level from the order book.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BookLevel {
    #[serde(deserialize_with = "de_string_f64")]
    pub price: f64,
    #[serde(deserialize_with = "de_string_f64")]
    pub size: f64,
}

/// Polymarket CLOB WebSocket book event.
#[derive(Debug, Clone, Deserialize)]
pub struct WsBookEvent {
    #[allow(dead_code)] // Present in CLOB WebSocket messages, parsed for completeness
    pub asset_id: Option<String>,
    pub bids: Option<Vec<BookLevel>>,
    pub asks: Option<Vec<BookLevel>>,
}

/// Result of checking whether our trade could fill at a single book snapshot.
#[derive(Debug, Clone)]
pub struct FillCheck {
    pub fillable: bool,
    pub available_depth_usd: f64,
    pub vwap: f64,
    pub slippage_cents: f64,
}

/// Aggregate fillability result for a recording window.
#[derive(Debug, Clone)]
pub struct FillabilityResult {
    pub snapshot_count: u32,
    pub fillable_count: u32,
    pub fill_probability: f64,
    pub opportunity_window_secs: f64,
    pub avg_available_depth_usd: f64,
    pub avg_vwap: f64,
    pub avg_slippage_cents: f64,
    pub window_start: String,
}

/// Could we fill `our_size_usd` at `our_target_price` or better?
///
/// For BUY: walk the asks (buy from sellers) where ask.price <= target_price.
/// For SELL: walk the bids (sell to buyers) where bid.price >= target_price.
pub fn check_fillable(
    bids: &[BookLevel],
    asks: &[BookLevel],
    side: Side,
    size_usd: f64,
    target_price: f64,
) -> FillCheck {
    let levels: Vec<&BookLevel> = match side {
        Side::Buy => {
            let mut sorted: Vec<&BookLevel> = asks.iter().collect();
            sorted.sort_by(|a, b| {
                a.price
                    .partial_cmp(&b.price)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            sorted
                .into_iter()
                .filter(|l| l.price <= target_price)
                .collect()
        }
        Side::Sell => {
            let mut sorted: Vec<&BookLevel> = bids.iter().collect();
            sorted.sort_by(|a, b| {
                b.price
                    .partial_cmp(&a.price)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            sorted
                .into_iter()
                .filter(|l| l.price >= target_price)
                .collect()
        }
    };

    // Track total available depth (all qualifying levels) and
    // VWAP (only the levels we would consume up to size_usd).
    let mut total_depth_usd = 0.0;
    let mut fill_accumulated = 0.0;
    let mut fill_weighted_sum = 0.0;

    for level in &levels {
        let level_usd = level.price * level.size;
        total_depth_usd += level_usd;

        // For VWAP: only accumulate up to our fill size
        if fill_accumulated < size_usd {
            let remaining = size_usd - fill_accumulated;
            let take = level_usd.min(remaining);
            fill_accumulated += take;
            fill_weighted_sum += level.price * take;
        }
    }

    let vwap = if fill_accumulated > 0.0 {
        fill_weighted_sum / fill_accumulated
    } else {
        target_price
    };

    let slippage_cents = (vwap - target_price).abs() * 100.0;
    let fillable = total_depth_usd >= size_usd;

    FillCheck {
        fillable,
        available_depth_usd: total_depth_usd,
        vwap,
        slippage_cents,
    }
}

/// Compute aggregate fillability metrics from a set of snapshots.
pub fn compute_fill_probability(
    snapshots: &[(bool, f64, f64, f64)], // (fillable, depth, vwap, slippage)
    window_start: &str,
    snapshot_interval_estimate_secs: f64,
) -> FillabilityResult {
    let total = snapshots.len() as u32;
    let fillable_count = snapshots.iter().filter(|(f, _, _, _)| *f).count() as u32;
    let fill_probability = if total > 0 {
        f64::from(fillable_count) / f64::from(total)
    } else {
        0.0
    };

    // Opportunity window: contiguous fillable seconds from start
    let contiguous_fillable = snapshots.iter().take_while(|(f, _, _, _)| *f).count() as f64;
    let opportunity_window_secs = contiguous_fillable * snapshot_interval_estimate_secs;

    let avg_depth = if total > 0 {
        snapshots.iter().map(|(_, d, _, _)| d).sum::<f64>() / f64::from(total)
    } else {
        0.0
    };

    let fillable_snapshots: Vec<&(bool, f64, f64, f64)> =
        snapshots.iter().filter(|(f, _, _, _)| *f).collect();
    let avg_vwap = if !fillable_snapshots.is_empty() {
        fillable_snapshots.iter().map(|(_, _, v, _)| v).sum::<f64>()
            / fillable_snapshots.len() as f64
    } else {
        0.0
    };
    let avg_slippage = if !fillable_snapshots.is_empty() {
        fillable_snapshots.iter().map(|(_, _, _, s)| s).sum::<f64>()
            / fillable_snapshots.len() as f64
    } else {
        0.0
    };

    FillabilityResult {
        snapshot_count: total,
        fillable_count,
        fill_probability,
        opportunity_window_secs,
        avg_available_depth_usd: avg_depth,
        avg_vwap,
        avg_slippage_cents: avg_slippage,
        window_start: window_start.to_string(),
    }
}

fn de_string_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StringOrNumber;

    impl<'de> de::Visitor<'de> for StringOrNumber {
        type Value = f64;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "a string or number")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            v.parse().map_err(de::Error::custom)
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(v)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(v as f64)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(v as f64)
        }
    }

    deserializer.deserialize_any(StringOrNumber)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_fillable_buy_sufficient_depth() {
        let bids = vec![];
        let asks = vec![
            BookLevel {
                price: 0.60,
                size: 100.0,
            },
            BookLevel {
                price: 0.61,
                size: 200.0,
            },
            BookLevel {
                price: 0.62,
                size: 150.0,
            },
        ];
        let result = check_fillable(&bids, &asks, Side::Buy, 50.0, 0.62);
        assert!(result.fillable);
        assert!(result.available_depth_usd > 50.0);
    }

    #[test]
    fn test_check_fillable_buy_insufficient_depth() {
        let bids = vec![];
        let asks = vec![BookLevel {
            price: 0.60,
            size: 10.0,
        }];
        let result = check_fillable(&bids, &asks, Side::Buy, 50.0, 0.62);
        assert!(!result.fillable);
        assert!(result.available_depth_usd < 50.0);
    }

    #[test]
    fn test_check_fillable_sell_sufficient_depth() {
        let bids = vec![
            BookLevel {
                price: 0.65,
                size: 200.0,
            },
            BookLevel {
                price: 0.64,
                size: 300.0,
            },
        ];
        let asks = vec![];
        let result = check_fillable(&bids, &asks, Side::Sell, 50.0, 0.64);
        assert!(result.fillable);
    }

    #[test]
    fn test_check_fillable_vwap_and_slippage() {
        let bids = vec![];
        let asks = vec![
            BookLevel {
                price: 0.50,
                size: 100.0,
            },
            BookLevel {
                price: 0.51,
                size: 100.0,
            },
        ];
        let result = check_fillable(&bids, &asks, Side::Buy, 10.0, 0.52);
        assert!(result.fillable);
        assert!(result.vwap >= 0.50);
        assert!(result.vwap <= 0.52);
        assert!(result.slippage_cents > 0.0);
    }

    #[test]
    fn test_check_fillable_empty_book() {
        let result = check_fillable(&[], &[], Side::Buy, 50.0, 0.50);
        assert!(!result.fillable);
        assert!((result.available_depth_usd).abs() < f64::EPSILON);
    }

    #[test]
    fn test_check_fillable_asks_above_target() {
        let asks = vec![
            BookLevel {
                price: 0.70,
                size: 1000.0,
            },
            BookLevel {
                price: 0.75,
                size: 1000.0,
            },
        ];
        let result = check_fillable(&[], &asks, Side::Buy, 10.0, 0.65);
        assert!(!result.fillable);
        assert!((result.available_depth_usd).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_fill_probability_all_fillable() {
        let snapshots = vec![
            (true, 100.0, 0.50, 0.5),
            (true, 110.0, 0.51, 0.4),
            (true, 105.0, 0.50, 0.6),
        ];
        let result = compute_fill_probability(&snapshots, "2026-01-01T00:00:00Z", 1.0);
        assert!((result.fill_probability - 1.0).abs() < f64::EPSILON);
        assert_eq!(result.snapshot_count, 3);
        assert_eq!(result.fillable_count, 3);
        assert!((result.opportunity_window_secs - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_fill_probability_none_fillable() {
        let snapshots = vec![(false, 5.0, 0.0, 0.0), (false, 3.0, 0.0, 0.0)];
        let result = compute_fill_probability(&snapshots, "2026-01-01T00:00:00Z", 1.0);
        assert!((result.fill_probability).abs() < f64::EPSILON);
        assert_eq!(result.fillable_count, 0);
        assert!((result.opportunity_window_secs).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_fill_probability_partial() {
        let snapshots = vec![
            (true, 100.0, 0.50, 0.5),
            (false, 5.0, 0.0, 0.0),
            (true, 100.0, 0.51, 0.4),
        ];
        let result = compute_fill_probability(&snapshots, "2026-01-01T00:00:00Z", 2.0);
        let expected_prob = 2.0 / 3.0;
        assert!((result.fill_probability - expected_prob).abs() < 0.01);
        assert!((result.opportunity_window_secs - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_fill_probability_empty() {
        let snapshots: Vec<(bool, f64, f64, f64)> = vec![];
        let result = compute_fill_probability(&snapshots, "2026-01-01T00:00:00Z", 1.0);
        assert!((result.fill_probability).abs() < f64::EPSILON);
        assert_eq!(result.snapshot_count, 0);
    }

    #[test]
    fn test_book_level_deserialize_string_values() {
        let json = r#"{"price": "0.523", "size": "100.5"}"#;
        let level: BookLevel = serde_json::from_str(json).unwrap();
        assert!((level.price - 0.523).abs() < f64::EPSILON);
        assert!((level.size - 100.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_book_level_deserialize_numeric_values() {
        let json = r#"{"price": 0.65, "size": 200}"#;
        let level: BookLevel = serde_json::from_str(json).unwrap();
        assert!((level.price - 0.65).abs() < f64::EPSILON);
        assert!((level.size - 200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_ws_book_event_deserialize() {
        let json = r#"{
            "asset_id": "token-123",
            "bids": [{"price": "0.50", "size": "100"}],
            "asks": [{"price": "0.55", "size": "200"}]
        }"#;
        let event: WsBookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.asset_id.as_deref(), Some("token-123"));
        assert_eq!(event.bids.as_ref().unwrap().len(), 1);
        assert_eq!(event.asks.as_ref().unwrap().len(), 1);
    }
}
