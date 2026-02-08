#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct WScoreWeights {
    pub edge_weight: f64,
    pub consistency_weight: f64,
}

#[allow(dead_code)]
impl Default for WScoreWeights {
    fn default() -> Self {
        Self {
            edge_weight: 0.60,
            consistency_weight: 0.40,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct WalletScoreInput {
    /// Total paper ROI over the scoring window, percent (e.g. +12.3).
    pub paper_roi_pct: f64,
    /// Stddev of daily returns over the window, percent.
    pub daily_return_stdev_pct: f64,
    /// Win rate (hit rate) in range [0, 1].
    pub hit_rate: f64,
}

#[allow(dead_code)]
fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

#[allow(dead_code)]
fn edge_score(paper_roi_pct: f64) -> f64 {
    // Normalize ROI into [0, 1], treating <=0 as 0.
    // In early MVP we cap at +20% => 1.0.
    clamp01(paper_roi_pct.max(0.0) / 20.0)
}

#[allow(dead_code)]
fn consistency_score(daily_return_stdev_pct: f64) -> f64 {
    // Normalize stdev into [0, 1] where 0% stdev => 1.0 and >=10% => 0.0.
    let max_stdev = 10.0;
    clamp01(1.0 - (daily_return_stdev_pct / max_stdev))
}

#[allow(dead_code)]
pub fn compute_wscore(input: &WalletScoreInput, w: &WScoreWeights) -> f64 {
    let e = edge_score(input.paper_roi_pct);
    let c = consistency_score(input.daily_return_stdev_pct);

    let total_w = w.edge_weight + w.consistency_weight;
    if total_w <= 0.0 {
        return 0.0;
    }

    let mut score = (w.edge_weight * e + w.consistency_weight * c) / total_w;

    // Win rate sensitivity analysis (Strategy Bible §3):
    // Penalize low hit rates even if ROI is high (e.g. tail risk sellers or lucky snipers).
    if input.hit_rate < 0.45 {
        score *= 0.5; // Heavy penalty for win rate < 45%
    } else if input.hit_rate < 0.52 {
        score *= 0.8; // Minor penalty for borderline win rate
    }

    clamp01(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_wscore_in_range() {
        let w = WScoreWeights::default();
        let s = compute_wscore(
            &WalletScoreInput {
                paper_roi_pct: 12.0,
                daily_return_stdev_pct: 3.0,
                hit_rate: 0.55,
            },
            &w,
        );
        assert!(s >= 0.0);
        assert!(s <= 1.0);
    }

    #[test]
    fn test_positive_edge_scores_higher() {
        let w = WScoreWeights::default();
        let good = compute_wscore(
            &WalletScoreInput {
                paper_roi_pct: 10.0,
                daily_return_stdev_pct: 2.0,
                hit_rate: 0.60,
            },
            &w,
        );
        let bad = compute_wscore(
            &WalletScoreInput {
                paper_roi_pct: 0.0,
                daily_return_stdev_pct: 2.0,
                hit_rate: 0.60,
            },
            &w,
        );
        assert!(good > bad);
    }

    #[test]
    fn test_unstable_wallet_scores_lower_on_consistency() {
        let w = WScoreWeights::default();
        let stable = compute_wscore(
            &WalletScoreInput {
                paper_roi_pct: 10.0,
                daily_return_stdev_pct: 1.0,
                hit_rate: 0.60,
            },
            &w,
        );
        let unstable = compute_wscore(
            &WalletScoreInput {
                paper_roi_pct: 10.0,
                daily_return_stdev_pct: 12.0,
                hit_rate: 0.60,
            },
            &w,
        );
        assert!(stable > unstable);
    }

    #[test]
    fn test_low_win_rate_penalty() {
        let w = WScoreWeights::default();
        let high_wr = compute_wscore(
            &WalletScoreInput {
                paper_roi_pct: 10.0,
                daily_return_stdev_pct: 2.0,
                hit_rate: 0.60,
            },
            &w,
        );
        let low_wr = compute_wscore(
            &WalletScoreInput {
                paper_roi_pct: 10.0,
                daily_return_stdev_pct: 2.0,
                hit_rate: 0.40,
            },
            &w,
        );
        assert!(high_wr > low_wr);
    }
}
