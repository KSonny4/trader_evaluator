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

/// Collect SQLite file and page statistics and record them as Prometheus gauges.
///
/// Runs `PRAGMA page_count`, `PRAGMA page_size`, `PRAGMA freelist_count` on the
/// background thread, and reads file sizes for the `.db` and `-wal` files from
/// the filesystem.
pub async fn run_sqlite_stats_once(db: &AsyncDb, db_path: &str) -> Result<()> {
    // File sizes from the filesystem (cheap, no DB lock needed).
    let db_file_size = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let wal_path = format!("{db_path}-wal");
    let wal_file_size = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);

    metrics::gauge!("evaluator_db_file_size_bytes").set(db_file_size as f64);
    metrics::gauge!("evaluator_db_wal_size_bytes").set(wal_file_size as f64);

    // Page stats from SQLite PRAGMAs.
    let (page_count, page_size, freelist_count) = db
        .call_named("sqlite_stats.pragmas", |conn| {
            let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
            let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            let freelist_count: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            Ok((page_count, page_size, freelist_count))
        })
        .await?;

    metrics::gauge!("evaluator_db_page_count").set(page_count as f64);
    metrics::gauge!("evaluator_db_page_size_bytes").set(page_size as f64);
    metrics::gauge!("evaluator_db_freelist_count").set(freelist_count as f64);

    tracing::debug!(
        db_file_size,
        wal_file_size,
        page_count,
        page_size,
        freelist_count,
        "sqlite stats collected"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;

    #[test]
    fn test_sqlite_stats_records_gauges() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        let rt = tokio::runtime::Runtime::new().unwrap();
        metrics::with_local_recorder(&recorder, || {
            rt.block_on(async {
                let tmp = tempfile::NamedTempFile::new().unwrap();
                let path = tmp.path().to_str().unwrap();
                let db = AsyncDb::open(path).await.unwrap();

                run_sqlite_stats_once(&db, path).await.unwrap();
            });
        });

        let rendered = handle.render();
        assert!(
            rendered.contains("evaluator_db_file_size_bytes"),
            "expected evaluator_db_file_size_bytes, got:\n{rendered}"
        );
        assert!(
            rendered.contains("evaluator_db_page_count"),
            "expected evaluator_db_page_count, got:\n{rendered}"
        );
        assert!(
            rendered.contains("evaluator_db_page_size_bytes"),
            "expected evaluator_db_page_size_bytes, got:\n{rendered}"
        );
        assert!(
            rendered.contains("evaluator_db_freelist_count"),
            "expected evaluator_db_freelist_count, got:\n{rendered}"
        );
    }
}
