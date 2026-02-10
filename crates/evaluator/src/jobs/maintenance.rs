use anyhow::Result;
use common::db::AsyncDb;

use crate::flow_metrics;

/// Compute flow counts from DB and record to Prometheus gauges (for Grafana flow panels).
pub async fn run_flow_metrics_once(db: &AsyncDb) -> Result<()> {
    let counts = db
        .call_named("flow_metrics.compute", |conn| {
            flow_metrics::compute_flow_counts(conn)
        })
        .await?;
    flow_metrics::record_flow_counts(&counts);
    Ok(())
}

/// Run a WAL checkpoint to fold the WAL file back into the main database.
///
/// Without periodic checkpointing, the WAL file grows unbounded (we observed
/// 6.5 GB after 28 hours). TRUNCATE mode resets the WAL to zero bytes after
/// checkpointing all pages.
pub async fn run_wal_checkpoint_once(db: &AsyncDb) -> Result<(i64, i64)> {
    db.call_named("wal_checkpoint.run", |conn| {
        let mut stmt = conn.prepare("PRAGMA wal_checkpoint(TRUNCATE)")?;
        let (busy, log, checkpointed) = stmt.query_row([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        if busy != 0 {
            tracing::warn!(
                busy,
                log,
                checkpointed,
                "WAL checkpoint: database was busy, partial checkpoint"
            );
            metrics::counter!("evaluator_wal_checkpoint_total", "status" => "busy").increment(1);
        } else {
            tracing::info!(log, checkpointed, "WAL checkpoint complete");
            metrics::counter!("evaluator_wal_checkpoint_total", "status" => "ok").increment(1);
        }
        metrics::gauge!("evaluator_wal_checkpoint_pages").set(checkpointed as f64);
        Ok((log, checkpointed))
    })
    .await
}
