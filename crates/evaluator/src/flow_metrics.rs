use anyhow::Result;
use rusqlite::Connection;

use crate::persona_classification;

/// Records current flow counts to Prometheus gauges (for Grafana flow panels).
pub fn record_flow_counts(counts: &FlowCounts) {
    metrics::gauge!("evaluator_flow_funnel_markets_fetched")
        .set(counts.funnel.markets_fetched as f64);
    metrics::gauge!("evaluator_flow_funnel_markets_scored_today")
        .set(counts.funnel.markets_scored_today as f64);
    metrics::gauge!("evaluator_flow_funnel_wallets_discovered")
        .set(counts.funnel.wallets_discovered as f64);
    metrics::gauge!("evaluator_flow_funnel_wallets_tracked")
        .set(counts.funnel.wallets_tracked as f64);
    metrics::gauge!("evaluator_flow_funnel_paper_wallets").set(counts.funnel.paper_wallets as f64);
    metrics::gauge!("evaluator_flow_funnel_wallets_ranked_today")
        .set(counts.funnel.wallets_ranked_today as f64);
    metrics::gauge!("evaluator_flow_classification_wallets_tracked")
        .set(counts.classification.wallets_tracked as f64);
    metrics::gauge!("evaluator_flow_classification_stage1_excluded")
        .set(counts.classification.stage1_excluded as f64);
    metrics::gauge!("evaluator_flow_classification_stage1_passed")
        .set(counts.classification.stage1_passed as f64);
    metrics::gauge!("evaluator_flow_classification_stage2_followable")
        .set(counts.classification.stage2_followable as f64);
    metrics::gauge!("evaluator_flow_classification_stage2_excluded")
        .set(counts.classification.stage2_excluded as f64);
    metrics::gauge!("evaluator_flow_classification_stage2_unclassified")
        .set(counts.classification.stage2_unclassified as f64);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunnelFlowCounts {
    pub markets_fetched: i64,
    pub markets_scored_today: i64,
    pub wallets_discovered: i64,
    pub wallets_tracked: i64,
    /// Distinct wallets with ≥1 paper trade (matches dashboard funnel).
    pub paper_wallets: i64,
    pub wallets_ranked_today: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationFlowCounts {
    pub wallets_tracked: i64,
    pub stage1_excluded: i64,
    pub stage1_passed: i64,
    pub stage2_followable: i64,
    pub stage2_excluded: i64,
    pub stage2_unclassified: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowCounts {
    pub funnel: FunnelFlowCounts,
    pub classification: ClassificationFlowCounts,
}

/// Followable personas for Stage 2 counts come from
/// `persona_classification::FOLLOWABLE_PERSONAS` (single source of truth).
/// When adding a followable Persona, add it there.
pub fn compute_flow_counts(conn: &Connection) -> Result<FlowCounts> {
    let markets_fetched: i64 = conn.query_row("SELECT COUNT(*) FROM markets", [], |r| r.get(0))?;
    let markets_scored_today: i64 = conn.query_row(
        "SELECT COUNT(*) FROM market_scores WHERE score_date = date('now')",
        [],
        |r| r.get(0),
    )?;
    let wallets_discovered: i64 =
        conn.query_row("SELECT COUNT(*) FROM wallets", [], |r| r.get(0))?;
    let wallets_tracked: i64 = conn.query_row(
        "SELECT COUNT(*) FROM wallets WHERE is_active = 1",
        [],
        |r| r.get(0),
    )?;
    let paper_wallets: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT proxy_wallet) FROM paper_trades",
        [],
        |r| r.get(0),
    )?;
    let wallets_ranked_today: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT proxy_wallet) FROM wallet_scores_daily WHERE score_date = date('now')",
        [],
        |r| r.get(0),
    )?;

    // Stage 1: active wallets with any STAGE1_% exclusion
    let stage1_excluded: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT e.proxy_wallet) FROM wallet_exclusions e
         INNER JOIN wallets w ON w.proxy_wallet = e.proxy_wallet AND w.is_active = 1
         WHERE e.reason LIKE 'STAGE1_%'",
        [],
        |r| r.get(0),
    )?;
    let stage1_passed = wallets_tracked.saturating_sub(stage1_excluded);

    // Stage 2 followable: active wallets with latest persona in followable list and no STAGE2_% exclusion
    let followable_strs: Vec<&str> = persona_classification::FOLLOWABLE_PERSONAS
        .iter()
        .map(persona_classification::Persona::as_str)
        .collect();
    let placeholders = followable_strs
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let mut followable_stmt = conn.prepare(&format!(
        "SELECT COUNT(*) FROM (
          SELECT p.proxy_wallet FROM wallet_personas p
          INNER JOIN wallets w ON w.proxy_wallet = p.proxy_wallet AND w.is_active = 1
          WHERE p.classified_at = (SELECT MAX(classified_at) FROM wallet_personas WHERE proxy_wallet = p.proxy_wallet)
          AND p.persona IN ({placeholders})
          AND NOT EXISTS (SELECT 1 FROM wallet_exclusions e WHERE e.proxy_wallet = p.proxy_wallet AND e.reason LIKE 'STAGE2_%')
        )"
    ))?;
    let stage2_followable: i64 = followable_stmt.query_row(
        rusqlite::params_from_iter(followable_strs.into_iter()),
        |r| r.get(0),
    )?;

    let stage2_excluded: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT e.proxy_wallet) FROM wallet_exclusions e
         INNER JOIN wallets w ON w.proxy_wallet = e.proxy_wallet AND w.is_active = 1
         WHERE e.reason LIKE 'STAGE2_%'",
        [],
        |r| r.get(0),
    )?;

    let stage2_unclassified = stage1_passed
        .saturating_sub(stage2_followable)
        .saturating_sub(stage2_excluded);

    Ok(FlowCounts {
        funnel: FunnelFlowCounts {
            markets_fetched,
            markets_scored_today,
            wallets_discovered,
            wallets_tracked,
            paper_wallets,
            wallets_ranked_today,
        },
        classification: ClassificationFlowCounts {
            wallets_tracked,
            stage1_excluded,
            stage1_passed,
            stage2_followable,
            stage2_excluded,
            stage2_unclassified,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_flow_counts_returns_expected_counts() {
        let db = common::db::Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        // Funnel seed data.
        db.conn
            .execute(
                "INSERT INTO markets (condition_id, title) VALUES ('c1','m1'),('c2','m2')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES ('c1', date('now'), 1.0, 1)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('w1','HOLDER',1),('w2','TRADER_RECENT',1),('w3','HOLDER',0)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, slippage_applied, status) VALUES
                 ('w1','mirror','c1','BUY',10,0.5,0,'open'),
                 ('w1','mirror','c1','BUY',10,0.5,0,'settled_win'),
                 ('w2','mirror','c1','BUY',10,0.5,0,'settled_loss'),
                 ('w2','mirror','c2','BUY',10,0.5,0,'open'),
                 ('w2','mirror','c2','BUY',10,0.5,0,'open')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore) VALUES ('w1', date('now'), 30, 0.9)",
                [],
            )
            .unwrap();

        // Classification seed data: 2 active wallets have a latest classification, 1 is pending.
        db.conn
            .execute(
                "INSERT INTO wallet_exclusions (proxy_wallet, reason, excluded_at) VALUES ('w1','STAGE1_TOO_YOUNG', datetime('now'))",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at) VALUES ('w2','INFORMED_SPECIALIST', 0.8, datetime('now'))",
                [],
            )
            .unwrap();

        let got = compute_flow_counts(&db.conn).unwrap();

        assert_eq!(
            got.funnel,
            FunnelFlowCounts {
                markets_fetched: 2,
                markets_scored_today: 1,
                wallets_discovered: 3,
                wallets_tracked: 2,
                paper_wallets: 2,
                wallets_ranked_today: 1,
            }
        );

        assert_eq!(
            got.classification,
            ClassificationFlowCounts {
                wallets_tracked: 2,
                stage1_excluded: 1,
                stage1_passed: 1,
                stage2_followable: 1,
                stage2_excluded: 0,
                stage2_unclassified: 0,
            }
        );
    }

    #[test]
    fn test_followable_personas_single_source_matches_expected() {
        // persona_classification::FOLLOWABLE_PERSONAS is the single source of truth.
        // This test ensures we don't accidentally drop or reorder the list.
        let expected = [
            "INFORMED_SPECIALIST",
            "CONSISTENT_GENERALIST",
            "PATIENT_ACCUMULATOR",
        ];
        let actual: Vec<&str> = persona_classification::FOLLOWABLE_PERSONAS
            .iter()
            .map(persona_classification::Persona::as_str)
            .collect();
        assert_eq!(
            actual.as_slice(),
            expected,
            "persona_classification::FOLLOWABLE_PERSONAS must match expected followable persona strings"
        );
    }
}
