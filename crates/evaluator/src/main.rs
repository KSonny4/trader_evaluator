use anyhow::Result;
use std::sync::Arc;

mod cli;
mod event_bus;
mod events;
mod flow_metrics;
mod ingestion;
mod jobs;
mod market_scoring;
mod metrics;
mod persona_classification;
mod scheduler;
mod wallet_discovery;
mod wallet_features;
mod wallet_rules_engine;
mod wallet_scoring;

#[allow(clippy::too_many_lines)] // job wiring and worker loops
#[tokio::main]
async fn main() -> Result<()> {
    let config = common::config::Config::load()?;

    let (dispatch, _otel_guard) =
        common::observability::build_dispatch("evaluator", &config.general.log_level);
    tracing::dispatcher::set_global_default(dispatch).map_err(anyhow::Error::msg)?;

    tracing::info!("trader_evaluator starting");

    if let Some(parent) = std::path::Path::new(&config.database.path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    // CLI commands use sync Database — they exit immediately, no need for async.
    let cmd = cli::parse_args(std::env::args()).map_err(anyhow::Error::msg)?;
    if cmd != cli::Command::Run {
        let db = common::db::Database::open(&config.database.path)?;
        db.run_migrations()?;
        cli::run_command(&db, cmd)?;
        return Ok(());
    }

    metrics::install_prometheus(config.observability.prometheus_port)?;
    metrics::describe();

    // AsyncDb for the main evaluator process — dedicated background thread for SQLite.
    let db = common::db::AsyncDb::open(&config.database.path).await?;

    let cfg = Arc::new(config);
    let api = Arc::new(common::polymarket::PolymarketClient::new_with_settings(
        &cfg.polymarket.data_api_url,
        &cfg.polymarket.gamma_api_url,
        std::time::Duration::from_secs(15),
        std::time::Duration::from_millis(cfg.ingestion.rate_limit_delay_ms),
        cfg.ingestion.max_retries,
        std::time::Duration::from_millis(cfg.ingestion.backoff_base_ms),
    ));

    // ── Event Bus: Initialize if enabled (Phase 1 infrastructure, not yet used) ──
    let _event_bus = if cfg.events.enabled {
        tracing::info!("event bus enabled (capacity={})", cfg.events.bus_capacity);
        Some(Arc::new(event_bus::EventBus::new(cfg.events.bus_capacity)))
    } else {
        None
    };

    // ── Bootstrap: seed markets + wallets, then let scheduler handle the rest ──
    // Order: event_scoring first (wallet_discovery reads market_scores). Then run
    // wallet_discovery, leaderboard_discovery, and recovery in parallel (they are independent).
    // Finally wallet_rules (needs wallets to exist).
    tracing::info!("bootstrap: seeding markets and wallets");

    match jobs::run_event_scoring_once(&db, api.as_ref(), cfg.as_ref()).await {
        Ok(n) => tracing::info!(inserted = n, "bootstrap: event_scoring done"),
        Err(e) => tracing::error!(error = %e, "bootstrap: event_scoring failed"),
    }

    let (wallet_res, leaderboard_res) = tokio::join!(
        jobs::run_wallet_discovery_once(&db, api.as_ref(), api.as_ref(), cfg.as_ref()),
        jobs::run_leaderboard_discovery_once(&db, api.as_ref(), cfg.as_ref()),
    );
    match wallet_res {
        Ok(n) => tracing::info!(inserted = n, "bootstrap: wallet_discovery done"),
        Err(e) => tracing::error!(error = %e, "bootstrap: wallet_discovery failed"),
    }
    match leaderboard_res {
        Ok(n) => tracing::info!(inserted = n, "bootstrap: leaderboard_discovery done"),
        Err(e) => tracing::error!(error = %e, "bootstrap: leaderboard_discovery failed"),
    }

    match jobs::run_wallet_rules_once(&db, cfg.as_ref()).await {
        Ok(changed) => tracing::info!(changed, "bootstrap: wallet_rules done"),
        Err(e) => tracing::error!(error = %e, "bootstrap: wallet_rules failed"),
    }

    tracing::info!("bootstrap done — starting scheduler (ingestion runs immediately)");

    // ── Periodic scheduler ──
    let (event_scoring_tx, mut event_scoring_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (wallet_discovery_tx, mut wallet_discovery_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (trades_ingestion_tx, mut trades_ingestion_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (activity_ingestion_tx, mut activity_ingestion_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (positions_snapshot_tx, mut positions_snapshot_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (holders_snapshot_tx, mut holders_snapshot_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (wallet_rules_tx, mut wallet_rules_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (wallet_scoring_tx, mut wallet_scoring_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (persona_classification_tx, mut persona_classification_rx) =
        tokio::sync::mpsc::channel::<()>(8);
    let (wal_checkpoint_tx, mut wal_checkpoint_rx) = tokio::sync::mpsc::channel::<()>(8);
    let (flow_metrics_tx, mut flow_metrics_rx) = tokio::sync::mpsc::channel::<()>(8);

    let discovery_continuous = cfg
        .wallet_discovery
        .wallet_discovery_mode
        .eq_ignore_ascii_case("continuous");

    let mut scheduler_jobs = vec![scheduler::JobSpec {
        name: "event_scoring".to_string(),
        interval: std::time::Duration::from_secs(cfg.market_scoring.refresh_interval_secs),
        tick: event_scoring_tx,
        run_immediately: false,
    }];
    if !discovery_continuous {
        scheduler_jobs.push(scheduler::JobSpec {
            name: "wallet_discovery".to_string(),
            interval: std::time::Duration::from_secs(cfg.wallet_discovery.refresh_interval_secs),
            tick: wallet_discovery_tx,
            run_immediately: false,
        });
    }
    scheduler_jobs.extend([
        scheduler::JobSpec {
            name: "trades_ingestion".to_string(),
            interval: std::time::Duration::from_secs(cfg.ingestion.trades_poll_interval_secs),
            tick: trades_ingestion_tx,
            run_immediately: true,
        },
        scheduler::JobSpec {
            name: "activity_ingestion".to_string(),
            interval: std::time::Duration::from_secs(cfg.ingestion.activity_poll_interval_secs),
            tick: activity_ingestion_tx,
            run_immediately: true,
        },
        scheduler::JobSpec {
            name: "positions_snapshot".to_string(),
            interval: std::time::Duration::from_secs(cfg.ingestion.positions_poll_interval_secs),
            tick: positions_snapshot_tx,
            run_immediately: true,
        },
        scheduler::JobSpec {
            name: "holders_snapshot".to_string(),
            interval: std::time::Duration::from_secs(cfg.ingestion.holders_poll_interval_secs),
            tick: holders_snapshot_tx,
            run_immediately: true,
        },
        scheduler::JobSpec {
            name: "wallet_rules".to_string(),
            interval: std::time::Duration::from_secs(300),
            tick: wallet_rules_tx,
            run_immediately: true,
        },
        scheduler::JobSpec {
            name: "wallet_scoring".to_string(),
            interval: std::time::Duration::from_secs(86400),
            tick: wallet_scoring_tx,
            run_immediately: true,
        },
        scheduler::JobSpec {
            name: "persona_classification".to_string(),
            interval: std::time::Duration::from_secs(3600), // every hour
            tick: persona_classification_tx,
            run_immediately: true,
        },
        scheduler::JobSpec {
            name: "wal_checkpoint".to_string(),
            interval: std::time::Duration::from_secs(300), // every 5 minutes
            tick: wal_checkpoint_tx,
            run_immediately: false, // no need to checkpoint at startup
        },
        scheduler::JobSpec {
            name: "flow_metrics".to_string(),
            interval: std::time::Duration::from_secs(60), // every minute for Grafana flow panels
            tick: flow_metrics_tx,
            run_immediately: true,
        },
    ]);
    let _scheduler_handles = scheduler::start(scheduler_jobs);

    tokio::spawn({
        let api = api.clone();
        let cfg = cfg.clone();
        let db = db.clone();
        async move {
            while event_scoring_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "event_scoring");
                let _g = span.enter();
                match jobs::run_event_scoring_once(&db, api.as_ref(), cfg.as_ref()).await {
                    Ok(n) => tracing::info!(inserted = n, "event_scoring done"),
                    Err(e) => tracing::error!(error = %e, "event_scoring failed"),
                }
            }
        }
    });

    if discovery_continuous {
        // Continuous mode: run discovery in a loop (rate limit only, no scheduler interval).
        tokio::spawn({
            let api = api.clone();
            let cfg = cfg.clone();
            let db = db.clone();
            async move {
                loop {
                    let span = tracing::info_span!("job_run", job = "wallet_discovery");
                    let _g = span.enter();
                    let mut had_error = false;
                    match jobs::run_wallet_discovery_once(
                        &db,
                        api.as_ref(),
                        api.as_ref(),
                        cfg.as_ref(),
                    )
                    .await
                    {
                        Ok(n) => tracing::info!(inserted = n, "wallet_discovery done"),
                        Err(e) => {
                            tracing::error!(error = %e, "wallet_discovery failed");
                            had_error = true;
                        }
                    }
                    match jobs::run_leaderboard_discovery_once(&db, api.as_ref(), cfg.as_ref())
                        .await
                    {
                        Ok(n) => tracing::info!(inserted = n, "leaderboard_discovery done"),
                        Err(e) => {
                            tracing::error!(error = %e, "leaderboard_discovery failed");
                            had_error = true;
                        }
                    }
                    if had_error {
                        tracing::info!("discovery error backoff: sleeping 60s");
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    }
                }
            }
        });
    } else {
        // Scheduled mode: run on scheduler ticks.
        tokio::spawn({
            let api = api.clone();
            let cfg = cfg.clone();
            let db = db.clone();
            async move {
                while wallet_discovery_rx.recv().await.is_some() {
                    let span = tracing::info_span!("job_run", job = "wallet_discovery");
                    let _g = span.enter();
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
                    match jobs::run_leaderboard_discovery_once(&db, api.as_ref(), cfg.as_ref())
                        .await
                    {
                        Ok(n) => tracing::info!(inserted = n, "leaderboard_discovery done"),
                        Err(e) => tracing::error!(error = %e, "leaderboard_discovery failed"),
                    }
                }
            }
        });
    }

    tokio::spawn({
        let api = api.clone();
        let cfg = cfg.clone();
        let db = db.clone();
        async move {
            while trades_ingestion_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "trades_ingestion");
                let _g = span.enter();
                let w = cfg.ingestion.wallets_per_ingestion_run;
                match jobs::run_trades_ingestion_once(&db, api.as_ref(), 200, w).await {
                    Ok((_pages, inserted)) => {
                        tracing::info!(inserted, "trades_ingestion done");
                    }
                    Err(e) => tracing::error!(error = %e, "trades_ingestion failed"),
                }
            }
        }
    });

    tokio::spawn({
        let api = api.clone();
        let cfg = cfg.clone();
        let db = db.clone();
        async move {
            while activity_ingestion_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "activity_ingestion");
                let _g = span.enter();
                let w = cfg.ingestion.wallets_per_ingestion_run;
                match jobs::run_activity_ingestion_once(&db, api.as_ref(), 200, w).await {
                    Ok(inserted) => tracing::info!(inserted, "activity_ingestion done"),
                    Err(e) => tracing::error!(error = %e, "activity_ingestion failed"),
                }
            }
        }
    });

    tokio::spawn({
        let api = api.clone();
        let cfg = cfg.clone();
        let db = db.clone();
        async move {
            while positions_snapshot_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "positions_snapshot");
                let _g = span.enter();
                let w = cfg.ingestion.wallets_per_ingestion_run;
                match jobs::run_positions_snapshot_once(&db, api.as_ref(), 200, w).await {
                    Ok(inserted) => tracing::info!(inserted, "positions_snapshot done"),
                    Err(e) => tracing::error!(error = %e, "positions_snapshot failed"),
                }
            }
        }
    });

    tokio::spawn({
        let api = api.clone();
        let cfg = cfg.clone();
        let db = db.clone();
        async move {
            while holders_snapshot_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "holders_snapshot");
                let _g = span.enter();
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

    tokio::spawn({
        let cfg = cfg.clone();
        let db = db.clone();
        async move {
            while wallet_rules_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "wallet_rules");
                let _g = span.enter();
                match jobs::run_wallet_rules_once(&db, cfg.as_ref()).await {
                    Ok(changed) => tracing::info!(changed, "wallet_rules done"),
                    Err(e) => tracing::error!(error = %e, "wallet_rules failed"),
                }
            }
        }
    });

    tokio::spawn({
        let cfg = cfg.clone();
        let db = db.clone();
        async move {
            while wallet_scoring_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "wallet_scoring");
                let _g = span.enter();
                match jobs::run_wallet_scoring_once(&db, cfg.as_ref()).await {
                    Ok(inserted) => tracing::info!(inserted, "wallet_scoring done"),
                    Err(e) => tracing::error!(error = %e, "wallet_scoring failed"),
                }
            }
        }
    });

    tokio::spawn({
        let cfg = cfg.clone();
        let db = db.clone();
        async move {
            while persona_classification_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "persona_classification");
                let _g = span.enter();
                match jobs::run_persona_classification_once(&db, cfg.as_ref()).await {
                    Ok(classified) => {
                        tracing::info!(classified, "persona_classification done");
                    }
                    Err(e) => tracing::error!(error = %e, "persona_classification failed"),
                }
            }
        }
    });

    tokio::spawn({
        let db = db.clone();
        async move {
            while wal_checkpoint_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "wal_checkpoint");
                let _g = span.enter();
                match jobs::run_wal_checkpoint_once(&db).await {
                    Ok((log, checkpointed)) => {
                        tracing::info!(log, checkpointed, "wal_checkpoint done");
                    }
                    Err(e) => tracing::error!(error = %e, "wal_checkpoint failed"),
                }
            }
        }
    });

    tokio::spawn({
        let db = db.clone();
        async move {
            while flow_metrics_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "flow_metrics");
                let _g = span.enter();
                if let Err(e) = jobs::run_flow_metrics_once(&db).await {
                    tracing::error!(error = %e, "flow_metrics failed");
                }
            }
        }
    });

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");

    Ok(())
}
