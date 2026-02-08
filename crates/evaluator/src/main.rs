use anyhow::Result;

mod cli;
mod ingestion;
mod jobs;
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

    if let Some(parent) = std::path::Path::new(&config.database.path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let db = common::db::Database::open(&config.database.path)?;
    db.run_migrations()?;

    let cmd = cli::parse_args(std::env::args()).map_err(anyhow::Error::msg)?;
    if cmd != cli::Command::Run {
        cli::run_command(&db, cmd)?;
        return Ok(());
    }

    let _prom_handle = metrics::install_prometheus(config.observability.prometheus_port)?;
    metrics::describe();

    let db_path = config.database.path.clone();
    let cfg = std::rc::Rc::new(config);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let api = std::rc::Rc::new(common::polymarket::PolymarketClient::new_with_settings(
                &cfg.polymarket.data_api_url,
                &cfg.polymarket.gamma_api_url,
                std::time::Duration::from_secs(15),
                std::time::Duration::from_millis(cfg.ingestion.rate_limit_delay_ms),
                cfg.ingestion.max_retries,
                std::time::Duration::from_millis(cfg.ingestion.backoff_base_ms),
            ));

            let (market_scoring_tx, mut market_scoring_rx) = tokio::sync::mpsc::channel::<()>(8);
            let (wallet_discovery_tx, mut wallet_discovery_rx) =
                tokio::sync::mpsc::channel::<()>(8);
            let (trades_ingestion_tx, mut trades_ingestion_rx) =
                tokio::sync::mpsc::channel::<()>(8);
            let (activity_ingestion_tx, mut activity_ingestion_rx) =
                tokio::sync::mpsc::channel::<()>(8);
            let (positions_snapshot_tx, mut positions_snapshot_rx) =
                tokio::sync::mpsc::channel::<()>(8);
            let (holders_snapshot_tx, mut holders_snapshot_rx) =
                tokio::sync::mpsc::channel::<()>(8);
            let (paper_tick_tx, mut paper_tick_rx) = tokio::sync::mpsc::channel::<()>(8);
            let (wallet_scoring_tx, mut wallet_scoring_rx) = tokio::sync::mpsc::channel::<()>(8);

            let _scheduler_handles = scheduler::start_local(vec![
                scheduler::JobSpec {
                    name: "market_scoring".to_string(),
                    interval: std::time::Duration::from_secs(
                        cfg.market_scoring.refresh_interval_secs,
                    ),
                    tick: market_scoring_tx,
                    run_immediately: true,
                },
                scheduler::JobSpec {
                    name: "wallet_discovery".to_string(),
                    interval: std::time::Duration::from_secs(
                        cfg.wallet_discovery.refresh_interval_secs,
                    ),
                    tick: wallet_discovery_tx,
                    run_immediately: true,
                },
                scheduler::JobSpec {
                    name: "trades_ingestion".to_string(),
                    interval: std::time::Duration::from_secs(
                        cfg.ingestion.trades_poll_interval_secs,
                    ),
                    tick: trades_ingestion_tx,
                    run_immediately: true,
                },
                scheduler::JobSpec {
                    name: "activity_ingestion".to_string(),
                    interval: std::time::Duration::from_secs(
                        cfg.ingestion.activity_poll_interval_secs,
                    ),
                    tick: activity_ingestion_tx,
                    run_immediately: true,
                },
                scheduler::JobSpec {
                    name: "positions_snapshot".to_string(),
                    interval: std::time::Duration::from_secs(
                        cfg.ingestion.positions_poll_interval_secs,
                    ),
                    tick: positions_snapshot_tx,
                    run_immediately: true,
                },
                scheduler::JobSpec {
                    name: "holders_snapshot".to_string(),
                    interval: std::time::Duration::from_secs(
                        cfg.ingestion.holders_poll_interval_secs,
                    ),
                    tick: holders_snapshot_tx,
                    run_immediately: true,
                },
                scheduler::JobSpec {
                    name: "paper_tick".to_string(),
                    interval: std::time::Duration::from_secs(60),
                    tick: paper_tick_tx,
                    run_immediately: true,
                },
                scheduler::JobSpec {
                    name: "wallet_scoring".to_string(),
                    interval: std::time::Duration::from_secs(86400),
                    tick: wallet_scoring_tx,
                    run_immediately: true,
                },
            ]);

            tokio::task::spawn_local({
                let api = api.clone();
                let cfg = cfg.clone();
                let db_path = db_path.clone();
                async move {
                    while market_scoring_rx.recv().await.is_some() {
                        let db = common::db::Database::open(&db_path).expect("open db");
                        if let Err(e) = db.run_migrations() {
                            tracing::error!(error = %e, "market_scoring migrations failed");
                            continue;
                        }
                        match jobs::run_market_scoring_once(&db, api.as_ref(), cfg.as_ref()).await {
                            Ok(n) => tracing::info!(inserted = n, "market_scoring done"),
                            Err(e) => tracing::error!(error = %e, "market_scoring failed"),
                        }
                    }
                }
            });

            tokio::task::spawn_local({
                let api = api.clone();
                let cfg = cfg.clone();
                let db_path = db_path.clone();
                async move {
                    while wallet_discovery_rx.recv().await.is_some() {
                        let db = common::db::Database::open(&db_path).expect("open db");
                        if let Err(e) = db.run_migrations() {
                            tracing::error!(error = %e, "wallet_discovery migrations failed");
                            continue;
                        }
                        match jobs::run_wallet_discovery_once(
                            &db,
                            api.as_ref(),
                            api.as_ref(),
                            cfg.as_ref(),
                        )
                        .await
                        {
                            Ok(n) => tracing::info!(inserted = n, "wallet_discovery done"),
                            Err(e) => tracing::error!(error = %e, "wallet_discovery failed"),
                        }
                    }
                }
            });

            tokio::task::spawn_local({
                let api = api.clone();
                let db_path = db_path.clone();
                async move {
                    while trades_ingestion_rx.recv().await.is_some() {
                        let db = common::db::Database::open(&db_path).expect("open db");
                        if let Err(e) = db.run_migrations() {
                            tracing::error!(error = %e, "trades_ingestion migrations failed");
                            continue;
                        }
                        match jobs::run_trades_ingestion_once(&db, api.as_ref(), 200).await {
                            Ok((_pages, inserted)) => {
                                tracing::info!(inserted, "trades_ingestion done")
                            }
                            Err(e) => tracing::error!(error = %e, "trades_ingestion failed"),
                        }
                    }
                }
            });

            tokio::task::spawn_local({
                let api = api.clone();
                let db_path = db_path.clone();
                async move {
                    while activity_ingestion_rx.recv().await.is_some() {
                        let db = common::db::Database::open(&db_path).expect("open db");
                        if let Err(e) = db.run_migrations() {
                            tracing::error!(error = %e, "activity_ingestion migrations failed");
                            continue;
                        }
                        match jobs::run_activity_ingestion_once(&db, api.as_ref(), 200).await {
                            Ok(inserted) => tracing::info!(inserted, "activity_ingestion done"),
                            Err(e) => tracing::error!(error = %e, "activity_ingestion failed"),
                        }
                    }
                }
            });

            tokio::task::spawn_local({
                let api = api.clone();
                let db_path = db_path.clone();
                async move {
                    while positions_snapshot_rx.recv().await.is_some() {
                        let db = common::db::Database::open(&db_path).expect("open db");
                        if let Err(e) = db.run_migrations() {
                            tracing::error!(error = %e, "positions_snapshot migrations failed");
                            continue;
                        }
                        match jobs::run_positions_snapshot_once(&db, api.as_ref(), 200).await {
                            Ok(inserted) => tracing::info!(inserted, "positions_snapshot done"),
                            Err(e) => tracing::error!(error = %e, "positions_snapshot failed"),
                        }
                    }
                }
            });

            tokio::task::spawn_local({
                let api = api.clone();
                let cfg = cfg.clone();
                let db_path = db_path.clone();
                async move {
                    while holders_snapshot_rx.recv().await.is_some() {
                        let db = common::db::Database::open(&db_path).expect("open db");
                        if let Err(e) = db.run_migrations() {
                            tracing::error!(error = %e, "holders_snapshot migrations failed");
                            continue;
                        }
                        match jobs::run_holders_snapshot_once(
                            &db,
                            api.as_ref(),
                            cfg.wallet_discovery.holders_per_market as u32,
                        )
                        .await
                        {
                            Ok(inserted) => tracing::info!(inserted, "holders_snapshot done"),
                            Err(e) => tracing::error!(error = %e, "holders_snapshot failed"),
                        }
                    }
                }
            });

            tokio::task::spawn_local({
                let cfg = cfg.clone();
                let db_path = db_path.clone();
                async move {
                    while paper_tick_rx.recv().await.is_some() {
                        let db = common::db::Database::open(&db_path).expect("open db");
                        if let Err(e) = db.run_migrations() {
                            tracing::error!(error = %e, "paper_tick migrations failed");
                            continue;
                        }
                        match jobs::run_paper_tick_once(&db, cfg.as_ref()) {
                            Ok(inserted) => tracing::info!(inserted, "paper_tick done"),
                            Err(e) => tracing::error!(error = %e, "paper_tick failed"),
                        }
                    }
                }
            });

            tokio::task::spawn_local({
                let cfg = cfg.clone();
                let db_path = db_path.clone();
                async move {
                    while wallet_scoring_rx.recv().await.is_some() {
                        let db = common::db::Database::open(&db_path).expect("open db");
                        if let Err(e) = db.run_migrations() {
                            tracing::error!(error = %e, "wallet_scoring migrations failed");
                            continue;
                        }
                        match jobs::run_wallet_scoring_once(&db, cfg.as_ref()) {
                            Ok(inserted) => tracing::info!(inserted, "wallet_scoring done"),
                            Err(e) => tracing::error!(error = %e, "wallet_scoring failed"),
                        }
                    }
                }
            });

            tokio::signal::ctrl_c().await?;
            tracing::info!("shutting down");
            Ok::<(), anyhow::Error>(())
        })
        .await?;

    Ok(())
}
