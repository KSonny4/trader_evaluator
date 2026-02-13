//! Dead-letter queue (DLQ) for events that fail to process.
//!
//! Failed events are recorded in the `failed_events` table with error details
//! and retry count. A CLI command (`retry-failed-events`) can reprocess them.

#![allow(dead_code)] // Phase 4 infrastructure - functions used when DLQ wired into subscribers

use anyhow::Result;
use common::db::AsyncDb;

/// Maximum number of retries before an event is considered permanently failed.
pub const MAX_RETRIES: i64 = 3;

/// Records a failed event into the `failed_events` table.
///
/// Each call increments the retry count if the same event_type + event_data
/// combination already exists, otherwise inserts a new row.
pub async fn record_failed_event(
    db: &AsyncDb,
    event_type: &str,
    event_data: &str,
    error: &str,
) -> Result<()> {
    let event_type = event_type.to_string();
    let event_data = event_data.to_string();
    let error = error.to_string();

    db.call(move |conn| {
        conn.execute(
            "INSERT INTO failed_events (event_type, event_data, error, retry_count, failed_at, status)
             VALUES (?1, ?2, ?3, 0, datetime('now'), 'pending')
             ON CONFLICT(event_type, event_data) DO UPDATE SET
                error = excluded.error,
                retry_count = retry_count + 1,
                failed_at = datetime('now'),
                status = CASE WHEN retry_count + 1 >= ?4 THEN 'exhausted' ELSE 'pending' END",
            rusqlite::params![event_type, event_data, error, MAX_RETRIES],
        )?;
        Ok(())
    })
    .await
}

/// A single failed event row from the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedEvent {
    pub id: i64,
    pub event_type: String,
    pub event_data: String,
    pub error: String,
    pub retry_count: i64,
    pub failed_at: String,
    pub status: String,
}

/// Retrieves pending failed events (those eligible for retry).
pub async fn get_pending_failed_events(db: &AsyncDb, limit: usize) -> Result<Vec<FailedEvent>> {
    let limit = limit as i64;
    db.call(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, event_type, event_data, error, retry_count, failed_at, status
             FROM failed_events
             WHERE status = 'pending' AND retry_count < ?1
             ORDER BY failed_at ASC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![MAX_RETRIES, limit], |row| {
                Ok(FailedEvent {
                    id: row.get(0)?,
                    event_type: row.get(1)?,
                    event_data: row.get(2)?,
                    error: row.get(3)?,
                    retry_count: row.get(4)?,
                    failed_at: row.get(5)?,
                    status: row.get(6)?,
                })
            })?
            .filter_map(std::result::Result::ok)
            .collect();
        Ok(rows)
    })
    .await
}

/// Marks a failed event as successfully retried (removes from pending).
pub async fn mark_event_retried(db: &AsyncDb, event_id: i64) -> Result<()> {
    db.call(move |conn| {
        conn.execute(
            "UPDATE failed_events SET status = 'retried' WHERE id = ?1",
            rusqlite::params![event_id],
        )?;
        Ok(())
    })
    .await
}

/// Returns a count of failed events grouped by status.
pub async fn failed_event_counts(db: &AsyncDb) -> Result<Vec<(String, i64)>> {
    db.call(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT status, COUNT(*) FROM failed_events GROUP BY status ORDER BY status",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(std::result::Result::ok)
            .collect();
        Ok(rows)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::db::AsyncDb;

    async fn test_db() -> AsyncDb {
        AsyncDb::open(":memory:").await.unwrap()
    }

    #[tokio::test]
    async fn test_failed_events_table_exists() {
        let db = test_db().await;

        let tables: Vec<String> = db
            .call(|conn| {
                let mut stmt = conn
                    .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
                let rows = stmt
                    .query_map([], |row| row.get(0))?
                    .filter_map(std::result::Result::ok)
                    .collect();
                Ok(rows)
            })
            .await
            .unwrap();

        assert!(
            tables.contains(&"failed_events".to_string()),
            "failed_events table should exist after migrations; got: {tables:?}"
        );
    }

    #[tokio::test]
    async fn test_record_failed_event_inserts_row() {
        let db = test_db().await;

        record_failed_event(&db, "pipeline", r#"{"type":"markets_scored"}"#, "timeout")
            .await
            .unwrap();

        let events = get_pending_failed_events(&db, 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "pipeline");
        assert_eq!(events[0].event_data, r#"{"type":"markets_scored"}"#);
        assert_eq!(events[0].error, "timeout");
        assert_eq!(events[0].retry_count, 0);
        assert_eq!(events[0].status, "pending");
    }

    #[tokio::test]
    async fn test_record_failed_event_increments_retry_count_on_duplicate() {
        let db = test_db().await;

        // Record same event twice
        record_failed_event(&db, "pipeline", r#"{"type":"markets_scored"}"#, "error1")
            .await
            .unwrap();
        record_failed_event(&db, "pipeline", r#"{"type":"markets_scored"}"#, "error2")
            .await
            .unwrap();

        let events = get_pending_failed_events(&db, 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].retry_count, 1);
        assert_eq!(events[0].error, "error2"); // Latest error
    }

    #[tokio::test]
    async fn test_record_failed_event_marks_exhausted_after_max_retries() {
        let db = test_db().await;

        // Record same event MAX_RETRIES + 1 times
        for i in 0..=MAX_RETRIES {
            record_failed_event(
                &db,
                "pipeline",
                r#"{"type":"markets_scored"}"#,
                &format!("error{i}"),
            )
            .await
            .unwrap();
        }

        // Should no longer appear in pending
        let pending = get_pending_failed_events(&db, 10).await.unwrap();
        assert!(
            pending.is_empty(),
            "exhausted events should not appear in pending"
        );

        // But should show up in counts as exhausted
        let counts = failed_event_counts(&db).await.unwrap();
        let exhausted = counts.iter().find(|(s, _)| s == "exhausted");
        assert!(exhausted.is_some(), "should have exhausted events");
        assert_eq!(exhausted.unwrap().1, 1);
    }

    #[tokio::test]
    async fn test_get_pending_failed_events_respects_limit() {
        let db = test_db().await;

        for i in 0..5 {
            record_failed_event(&db, "pipeline", &format!("event{i}"), "error")
                .await
                .unwrap();
        }

        let events = get_pending_failed_events(&db, 3).await.unwrap();
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn test_mark_event_retried() {
        let db = test_db().await;

        record_failed_event(&db, "pipeline", r#"{"type":"test"}"#, "error")
            .await
            .unwrap();

        let events = get_pending_failed_events(&db, 10).await.unwrap();
        assert_eq!(events.len(), 1);

        mark_event_retried(&db, events[0].id).await.unwrap();

        // Should no longer appear in pending
        let after = get_pending_failed_events(&db, 10).await.unwrap();
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn test_failed_event_counts_grouped_by_status() {
        let db = test_db().await;

        // Insert two pending events
        record_failed_event(&db, "pipeline", "event1", "error")
            .await
            .unwrap();
        record_failed_event(&db, "pipeline", "event2", "error")
            .await
            .unwrap();

        // Retry one
        let events = get_pending_failed_events(&db, 10).await.unwrap();
        mark_event_retried(&db, events[0].id).await.unwrap();

        let counts = failed_event_counts(&db).await.unwrap();
        let pending_count = counts
            .iter()
            .find(|(s, _)| s == "pending")
            .map_or(0, |c| c.1);
        let retried_count = counts
            .iter()
            .find(|(s, _)| s == "retried")
            .map_or(0, |c| c.1);

        assert_eq!(pending_count, 1);
        assert_eq!(retried_count, 1);
    }

    #[tokio::test]
    async fn test_different_event_types_are_separate() {
        let db = test_db().await;

        record_failed_event(&db, "pipeline", "same_data", "error1")
            .await
            .unwrap();
        record_failed_event(&db, "operational", "same_data", "error2")
            .await
            .unwrap();

        let events = get_pending_failed_events(&db, 10).await.unwrap();
        assert_eq!(
            events.len(),
            2,
            "different event_types should be separate rows"
        );
    }
}
