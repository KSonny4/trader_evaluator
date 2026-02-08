use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

#[derive(Debug, Clone)]
pub struct JobSpec {
    pub name: String,
    pub interval: Duration,
    pub tick: mpsc::Sender<()>,
}

pub fn start(jobs: Vec<JobSpec>) -> Vec<JoinHandle<()>> {
    jobs.into_iter()
        .map(|job| {
            tokio::spawn(async move {
                let start_at = Instant::now() + job.interval;
                let mut interval = tokio::time::interval_at(start_at, job.interval);
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

                loop {
                    interval.tick().await;
                    tracing::debug!(job = %job.name, "scheduler tick");
                    if job.tick.send(()).await.is_err() {
                        break;
                    }
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test(start_paused = true)]
    async fn test_scheduler_fires_jobs_at_intervals() {
        let (tx, mut rx) = mpsc::channel(16);
        let _handles = start(vec![JobSpec {
            name: "job1".to_string(),
            interval: Duration::from_secs(10),
            tick: tx,
        }]);

        // Ensure spawned task is polled at least once so it registers its timer.
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_secs(9)).await;
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());

        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_ok());

        tokio::time::advance(Duration::from_secs(10)).await;
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_ok()); // t=20

        tokio::time::advance(Duration::from_secs(10)).await;
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_ok()); // t=30
    }
}
