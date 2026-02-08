use anyhow::Result;

mod ingestion;
mod market_scoring;
mod metrics;
mod paper_trading;
mod scheduler;
mod wallet_discovery;
mod wallet_scoring;

#[tokio::main]
async fn main() -> Result<()> {
    let config = common::config::Config::load()?;

    tracing_subscriber::fmt()
        .with_env_filter(&config.general.log_level)
        .json()
        .init();

    tracing::info!("trader_evaluator starting");

    let _prom_handle = metrics::install_prometheus(config.observability.prometheus_port)?;
    metrics::describe();

    if let Some(parent) = std::path::Path::new(&config.database.path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let db = common::db::Database::open(&config.database.path)?;
    db.run_migrations()?;

    // Scheduler wires the periodic ticks. Actual job logic will be implemented in later tasks.
    let (market_scoring_tx, mut market_scoring_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (wallet_discovery_tx, mut wallet_discovery_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (trades_ingestion_tx, mut trades_ingestion_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (activity_ingestion_tx, mut activity_ingestion_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (positions_snapshot_tx, mut positions_snapshot_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (holders_snapshot_tx, mut holders_snapshot_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (paper_tick_tx, mut paper_tick_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (wallet_scoring_tx, mut wallet_scoring_rx) = tokio::sync::mpsc::channel::<()>(8);

    let _scheduler_handles = scheduler::start(vec![
        scheduler::JobSpec {
            name: "market_scoring".to_string(),
            interval: std::time::Duration::from_secs(config.market_scoring.refresh_interval_secs),
            tick: market_scoring_tx,
        },
        scheduler::JobSpec {
            name: "wallet_discovery".to_string(),
            interval: std::time::Duration::from_secs(config.wallet_discovery.refresh_interval_secs),
            tick: wallet_discovery_tx,
        },
        scheduler::JobSpec {
            name: "trades_ingestion".to_string(),
            interval: std::time::Duration::from_secs(config.ingestion.trades_poll_interval_secs),
            tick: trades_ingestion_tx,
        },
        scheduler::JobSpec {
            name: "activity_ingestion".to_string(),
            interval: std::time::Duration::from_secs(config.ingestion.activity_poll_interval_secs),
            tick: activity_ingestion_tx,
        },
        scheduler::JobSpec {
            name: "positions_snapshot".to_string(),
            interval: std::time::Duration::from_secs(config.ingestion.positions_poll_interval_secs),
            tick: positions_snapshot_tx,
        },
        scheduler::JobSpec {
            name: "holders_snapshot".to_string(),
            interval: std::time::Duration::from_secs(config.ingestion.holders_poll_interval_secs),
            tick: holders_snapshot_tx,
        },
        scheduler::JobSpec {
            name: "paper_tick".to_string(),
            interval: std::time::Duration::from_secs(60),
            tick: paper_tick_tx,
        },
        scheduler::JobSpec {
            name: "wallet_scoring".to_string(),
            interval: std::time::Duration::from_secs(86400),
            tick: wallet_scoring_tx,
        },
    ]);

    tokio::spawn(async move {
        while market_scoring_rx.recv().await.is_some() {
            tracing::info!("tick: market_scoring");
        }
    });
    tokio::spawn(async move {
        while wallet_discovery_rx.recv().await.is_some() {
            tracing::info!("tick: wallet_discovery");
        }
    });
    tokio::spawn(async move {
        while trades_ingestion_rx.recv().await.is_some() {
            tracing::info!("tick: trades_ingestion");
        }
    });
    tokio::spawn(async move {
        while activity_ingestion_rx.recv().await.is_some() {
            tracing::info!("tick: activity_ingestion");
        }
    });
    tokio::spawn(async move {
        while positions_snapshot_rx.recv().await.is_some() {
            tracing::info!("tick: positions_snapshot");
        }
    });
    tokio::spawn(async move {
        while holders_snapshot_rx.recv().await.is_some() {
            tracing::info!("tick: holders_snapshot");
        }
    });
    tokio::spawn(async move {
        while paper_tick_rx.recv().await.is_some() {
            tracing::info!("tick: paper_tick");
        }
    });
    tokio::spawn(async move {
        while wallet_scoring_rx.recv().await.is_some() {
            tracing::info!("tick: wallet_scoring");
        }
    });

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    Ok(())
}
