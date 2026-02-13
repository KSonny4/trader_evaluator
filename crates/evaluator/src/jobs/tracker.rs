use anyhow::Result;
use common::db::AsyncDb;
use std::time::Instant;

pub struct JobTracker {
    db: AsyncDb,
    job_name: String,
    start_time: Instant,
}

impl JobTracker {
    pub async fn start(db: &AsyncDb, job_name: &str) -> Result<Self> {
        let name = job_name.to_string();
        db.call_named("job_tracker.start", move |conn| {
            conn.execute(
                "INSERT INTO job_status (job_name, status, last_run_at, updated_at)
                 VALUES (?1, 'running', datetime('now'), datetime('now'))
                 ON CONFLICT(job_name) DO UPDATE SET
                    status = 'running',
                    last_run_at = datetime('now'),
                    updated_at = datetime('now'),
                    last_error = NULL, 
                    duration_ms = NULL",
                rusqlite::params![name],
            )?;
            Ok(())
        })
        .await?;

        Ok(Self {
            db: db.clone(),
            job_name: job_name.to_string(),
            start_time: Instant::now(),
        })
    }

    pub async fn success(self, metadata: Option<serde_json::Value>) -> Result<()> {
        let duration_ms = self.start_time.elapsed().as_millis() as i64;
        let name = self.job_name.clone();
        let meta_str = metadata.map(|v| v.to_string());

        self.db
            .call_named("job_tracker.success", move |conn| {
                conn.execute(
                    "UPDATE job_status SET
                    status = 'idle',
                    duration_ms = ?2,
                    metadata = ?3,
                    updated_at = datetime('now')
                 WHERE job_name = ?1",
                    rusqlite::params![name, duration_ms, meta_str],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn fail(self, error: &anyhow::Error) -> Result<()> {
        let duration_ms = self.start_time.elapsed().as_millis() as i64;
        let name = self.job_name;
        let error_msg = error.to_string();

        self.db
            .call_named("job_tracker.fail", move |conn| {
                conn.execute(
                    "UPDATE job_status SET
                    status = 'failed',
                    duration_ms = ?2,
                    last_error = ?3,
                    updated_at = datetime('now')
                 WHERE job_name = ?1",
                    rusqlite::params![name, duration_ms, error_msg],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Update job progress without completing the job
    pub async fn update_progress(&self, metadata: serde_json::Value) -> Result<()> {
        let name = self.job_name.clone();
        let meta_str = metadata.to_string();

        self.db
            .call_named("job_tracker.update_progress", move |conn| {
                conn.execute(
                    "UPDATE job_status SET
                        metadata = ?2,
                        updated_at = datetime('now')
                     WHERE job_name = ?1",
                    rusqlite::params![name, meta_str],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_update_progress_updates_metadata_without_completing() {
        let db = AsyncDb::open(":memory:").await.unwrap();

        // Start a job
        let tracker = JobTracker::start(&db, "test_job").await.unwrap();

        // Update progress
        let progress_json = serde_json::json!({
            "progress": 10,
            "total": 100,
            "phase": "processing"
        });
        tracker
            .update_progress(progress_json.clone())
            .await
            .unwrap();

        // Verify: status should still be "running" and metadata should be updated
        let status: String = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT status FROM job_status WHERE job_name = 'test_job'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();

        let metadata: Option<String> = db
            .call(|conn| {
                Ok(conn.query_row(
                    "SELECT metadata FROM job_status WHERE job_name = 'test_job'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();

        assert_eq!(
            status, "running",
            "Job should still be running after update_progress"
        );
        assert_eq!(
            metadata,
            Some(progress_json.to_string()),
            "Metadata should be updated"
        );
    }
}
