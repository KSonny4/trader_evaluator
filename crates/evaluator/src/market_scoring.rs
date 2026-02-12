#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct MarketCandidate {
    pub condition_id: String, // market (outcome)
    pub title: String,
    pub event_slug: Option<String>, // event (e.g. sparta-slavia)
    pub liquidity: f64,
    pub volume_24h: f64,
    pub trades_24h: u32,
    pub unique_traders_24h: u32,
    pub top_holder_concentration: f64,
    pub days_to_expiry: u32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct ScoringWeights {
    pub liquidity: f64,
    pub volume: f64,
    pub density: f64,
    pub whale_concentration: f64,
    pub time_to_expiry: f64,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            liquidity: 0.25,
            volume: 0.25,
            density: 0.20,
            whale_concentration: 0.15,
            time_to_expiry: 0.15,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ScoredMarket {
    pub market: MarketCandidate,
    pub mscore: f64,
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

#[allow(dead_code)]
pub fn compute_mscore(market: &MarketCandidate, weights: &ScoringWeights) -> f64 {
    let liquidity_score = clamp01((market.liquidity + 1.0).log10() / 1_000_000_f64.log10());
    let volume_score = clamp01((market.volume_24h + 1.0).log10() / 500_000_f64.log10());
    let density_score = clamp01(f64::from(market.trades_24h) / 500.0);
    let whale_concentration_score = clamp01(1.0 - market.top_holder_concentration);
    let time_to_expiry_score = time_to_expiry_score(market.days_to_expiry);

    let total_w = weights.liquidity
        + weights.volume
        + weights.density
        + weights.whale_concentration
        + weights.time_to_expiry;
    if total_w <= 0.0 {
        return 0.0;
    }

    let sum = weights.liquidity * liquidity_score
        + weights.volume * volume_score
        + weights.density * density_score
        + weights.whale_concentration * whale_concentration_score
        + weights.time_to_expiry * time_to_expiry_score;

    // Don't allow a "dead market" (no liquidity/volume/trades) to score highly just because
    // secondary factors (whale dispersion, time-to-expiry) look good.
    let activity_gate = (liquidity_score + volume_score + density_score) / 3.0;

    clamp01((sum / total_w) * activity_gate)
}

fn time_to_expiry_score(days: u32) -> f64 {
    // Simple bell-ish curve:
    // - ramp up from 0 at 0d to 1 at 7d
    // - stay at 1 between 7d..=30d
    // - ramp down to 0 at 90d
    let d = f64::from(days);
    if d <= 0.0 || d >= 90.0 {
        return 0.0;
    }
    if d < 7.0 {
        return clamp01(d / 7.0);
    }
    if d <= 30.0 {
        return 1.0;
    }
    // 30..90 maps to 1..0
    clamp01(1.0 - (d - 30.0) / 60.0)
}

/// Score all markets with MScore. Truncation to top N is done by `rank_events`.
#[allow(dead_code)]
pub fn rank_markets(markets: Vec<MarketCandidate>) -> Vec<ScoredMarket> {
    let weights = ScoringWeights::default();
    markets
        .into_iter()
        .map(|m| {
            let mscore = compute_mscore(&m, &weights);
            ScoredMarket { market: m, mscore }
        })
        .collect()
}

/// Event key: event_slug if present, else condition_id (singleton event).
fn event_key(m: &ScoredMarket) -> String {
    m.market
        .event_slug
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| m.market.condition_id.clone())
}

/// Rank by EScore (best MScore per event), select top N events, return all markets with event_rank.
/// Returns (total_events_evaluated, ranked_markets) for pipeline to write to market_scores
/// and scoring_stats.
pub fn rank_events(
    scored: Vec<ScoredMarket>,
    top_n_events: usize,
) -> (usize, Vec<(i64, ScoredMarket)>) {
    use std::collections::HashMap;
    let mut by_event: HashMap<String, Vec<ScoredMarket>> = HashMap::new();
    for sm in scored {
        let key = event_key(&sm);
        by_event.entry(key).or_default().push(sm);
    }
    let mut events: Vec<_> = by_event
        .into_values()
        .map(|markets| {
            let escore = markets.iter().map(|sm| sm.mscore).fold(0.0_f64, f64::max);
            (escore, markets)
        })
        .collect();
    events.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let total_events_evaluated = events.len();
    events.truncate(top_n_events);
    let ranked = events
        .into_iter()
        .enumerate()
        .flat_map(|(i, (_, markets))| {
            let rank = (i + 1) as i64;
            markets.into_iter().map(move |sm| (rank, sm))
        })
        .collect();
    (total_events_evaluated, ranked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mscore_computation() {
        let market = MarketCandidate {
            condition_id: "0xabc".to_string(),
            title: "Will BTC go up?".to_string(),
            event_slug: None,
            liquidity: 50000.0,
            volume_24h: 20000.0,
            trades_24h: 100,
            unique_traders_24h: 30,
            top_holder_concentration: 0.4,
            days_to_expiry: 14,
        };
        let weights = ScoringWeights {
            liquidity: 0.25,
            volume: 0.25,
            density: 0.20,
            whale_concentration: 0.15,
            time_to_expiry: 0.15,
        };
        let score = compute_mscore(&market, &weights);
        assert!(score > 0.0);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_mscore_zero_liquidity_scores_low() {
        let market = MarketCandidate {
            condition_id: "0xabc".to_string(),
            title: "Dead market".to_string(),
            event_slug: None,
            liquidity: 0.0,
            volume_24h: 0.0,
            trades_24h: 0,
            unique_traders_24h: 0,
            top_holder_concentration: 0.0,
            days_to_expiry: 14,
        };
        let weights = ScoringWeights::default();
        let score = compute_mscore(&market, &weights);
        assert!(score < 0.1);
    }

    #[test]
    fn test_rank_events_selects_top_events() {
        let markets = vec![
            MarketCandidate {
                condition_id: "0x1".to_string(),
                title: "M1".to_string(),
                event_slug: Some("evt-a".to_string()),
                liquidity: 1000.0,
                volume_24h: 500.0,
                trades_24h: 10,
                unique_traders_24h: 5,
                top_holder_concentration: 0.9,
                days_to_expiry: 5,
            },
            MarketCandidate {
                condition_id: "0x2".to_string(),
                title: "M2".to_string(),
                event_slug: Some("evt-a".to_string()),
                liquidity: 50000.0,
                volume_24h: 20000.0,
                trades_24h: 100,
                unique_traders_24h: 30,
                top_holder_concentration: 0.4,
                days_to_expiry: 14,
            },
            MarketCandidate {
                condition_id: "0x3".to_string(),
                title: "M3".to_string(),
                event_slug: Some("evt-b".to_string()),
                liquidity: 200000.0,
                volume_24h: 100000.0,
                trades_24h: 300,
                unique_traders_24h: 80,
                top_holder_concentration: 0.2,
                days_to_expiry: 20,
            },
            MarketCandidate {
                condition_id: "0x4".to_string(),
                title: "M4".to_string(),
                event_slug: None,
                liquidity: 10000.0,
                volume_24h: 2000.0,
                trades_24h: 60,
                unique_traders_24h: 15,
                top_holder_concentration: 0.3,
                days_to_expiry: 45,
            },
            MarketCandidate {
                condition_id: "0x5".to_string(),
                title: "M5".to_string(),
                event_slug: None,
                liquidity: 0.0,
                volume_24h: 0.0,
                trades_24h: 0,
                unique_traders_24h: 0,
                top_holder_concentration: 0.0,
                days_to_expiry: 90,
            },
        ];

        let scored = rank_markets(markets);
        let (total, ranked) = rank_events(scored, 2);
        assert_eq!(total, 4); // evt-a, evt-b, 0x4, 0x5 (0x5 has mscore 0 but still an event)
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].0, 1);
        assert_eq!(ranked[1].0, 2);
        assert_eq!(ranked[2].0, 2);
    }
}
