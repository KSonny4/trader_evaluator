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

    // ── Event Bus: Initialized when enabled, passed to all jobs (Phase 2) ──
    let event_bus = if cfg.events.enabled {
        tracing::info!("event bus enabled (capacity={})", cfg.events.bus_capacity);
        Some(Arc::new(event_bus::EventBus::new(cfg.events.bus_capacity)))
    } else {
        None
    };

    // ── Event Logging Subscriber: Logs all events to stdout when enabled ──
    if let Some(ref bus) = event_bus {
        let subscriber_bus = bus.clone();
        tokio::spawn(async move {
            events::subscribers::spawn_logging_subscriber(subscriber_bus).await;
        });
        tracing::info!("event logging subscriber started");
    }

    // ── Periodic scheduler: Create channels and start scheduler BEFORE bootstrap ──
    // This ensures jobs like wallet_scoring run immediately on existing data
    // instead of waiting 10+ minutes for bootstrap to complete.
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
    let (sqlite_stats_tx, mut sqlite_stats_rx) = tokio::sync::mpsc::channel::<()>(8);

    let discovery_continuous = cfg
        .wallet_discovery
        .wallet_discovery_mode
        .eq_ignore_ascii_case("continuous");

    // Event-driven discovery: when enabled AND event bus is available, MarketsScored
    // events trigger discovery immediately instead of using a timer.
    let discovery_event_driven = cfg.events.enable_discovery_event_trigger && event_bus.is_some();

    let mut scheduler_jobs = vec![scheduler::JobSpec {
        name: "event_scoring".to_string(),
        interval: std::time::Duration::from_secs(cfg.market_scoring.refresh_interval_secs),
        tick: event_scoring_tx,
        run_immediately: false,
    }];
    if discovery_event_driven {
        // Event-driven mode: MarketsScored events trigger discovery immediately
        let bus = event_bus.as_ref().unwrap().clone();
        tokio::spawn(async move {
            events::subscribers::spawn_discovery_trigger_subscriber(bus, wallet_discovery_tx).await;
        });
        tracing::info!("event-driven discovery trigger enabled (MarketsScored → discovery)");
    } else if !discovery_continuous {
        // Timer-based fallback: only used when neither continuous nor event-driven mode is active
        scheduler_jobs.push(scheduler::JobSpec {
            name: "wallet_discovery".to_string(),
            interval: std::time::Duration::from_secs(cfg.wallet_discovery.refresh_interval_secs),
            tick: wallet_discovery_tx,
            run_immediately: false,
        });
    }

    // Event-driven classification: when enabled, TradesIngested events are batched
    // and trigger classification every N seconds (configurable window).
    let classification_event_driven =
        cfg.events.enable_classification_event_trigger && event_bus.is_some();

    if classification_event_driven {
        let bus = event_bus.as_ref().unwrap().clone();
        let batch_window =
            std::time::Duration::from_secs(cfg.events.classification_batch_window_secs);
        let classification_tx = persona_classification_tx.clone();
        tokio::spawn(async move {
            events::subscribers::spawn_classification_trigger_subscriber(
                bus,
                classification_tx,
                batch_window,
            )
            .await;
        });
        tracing::info!(
            window_secs = cfg.events.classification_batch_window_secs,
            "event-driven classification trigger enabled (TradesIngested batched → classification)"
        );
    }

    // Event-driven fast-path: when enabled, TradesIngested events trigger
    // fast-path coalescing for immediate paper trading reactions.
    let fast_path_enabled = cfg.events.enable_fast_path_trigger && event_bus.is_some();

    if fast_path_enabled {
        let bus = event_bus.as_ref().unwrap().clone();
        tokio::spawn(async move {
            events::subscribers::spawn_fast_path_subscriber(bus).await;
        });

        // Spawn fast-path worker that converts coalesced triggers to ticks
        let bus_worker = event_bus.as_ref().unwrap().clone();
        let (paper_tick_tx, mut paper_tick_rx) = tokio::sync::mpsc::channel::<u64>(8);
        tokio::spawn(async move {
            events::subscribers::spawn_fast_path_worker(bus_worker, paper_tick_tx).await;
        });

        // Wire paper_tick_rx to downstream consumer (future: paper trading scheduler)
        tokio::spawn(async move {
            while let Some(generation) = paper_tick_rx.recv().await {
                tracing::info!(
                    generation,
                    "fast-path tick received (ready for paper trading integration)"
                );
                // TODO(#81): When trader microservice supports event-driven mode, trigger paper tick here
            }
        });

        tracing::info!(
            "event-driven fast-path trigger enabled (TradesIngested coalescing → paper tick)"
        );
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
        scheduler::JobSpec {
            name: "sqlite_stats".to_string(),
            interval: std::time::Duration::from_secs(60), // every minute for Grafana DB panels
            tick: sqlite_stats_tx,
            run_immediately: true,
        },
    ]);

    // Conditionally add persona_classification to scheduler (timer fallback when not event-driven)
    if !classification_event_driven {
        scheduler_jobs.push(scheduler::JobSpec {
            name: "persona_classification".to_string(),
            interval: std::time::Duration::from_secs(3600), // every hour
            tick: persona_classification_tx,
            run_immediately: true,
        });
    }

    // ── Spawn ALL worker loops BEFORE starting scheduler ──
    // This ensures workers are ready to receive messages when scheduler sends them immediately.
    tracing::info!("spawning worker loops (ready to receive scheduler ticks)");

    tokio::spawn({
        let api = api.clone();
        let cfg = cfg.clone();
        let db = db.clone();
        let event_bus = event_bus.clone();
        async move {
            while event_scoring_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "event_scoring");
                let _g = span.enter();
                match jobs::run_event_scoring_once(
                    &db,
                    api.as_ref(),
                    cfg.as_ref(),
                    event_bus.as_deref(),
                )
                .await
                {
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
                        None,
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
                        None,
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
        let event_bus = event_bus.clone();
        async move {
            while trades_ingestion_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "trades_ingestion");
                let _g = span.enter();
                let w = cfg.ingestion.wallets_per_ingestion_run;
                let pt = cfg.ingestion.parallel_tasks;
                match jobs::run_trades_ingestion_once(
                    &db,
                    api.clone(),
                    200,
                    w,
                    pt,
                    event_bus.clone(),
                )
                .await
                {
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
                let pt = cfg.ingestion.parallel_tasks;
                match jobs::run_activity_ingestion_once(&db, api.clone(), 200, w, pt).await {
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
                let pt = cfg.ingestion.parallel_tasks;
                match jobs::run_positions_snapshot_once(&db, api.clone(), 200, w, pt).await {
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
        let event_bus = event_bus.clone();
        async move {
            while wallet_rules_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "wallet_rules");
                let _g = span.enter();
                match jobs::run_wallet_rules_once(&db, cfg.as_ref(), event_bus.as_deref()).await {
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
        let event_bus = event_bus.clone();
        async move {
            while persona_classification_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "persona_classification");
                let _g = span.enter();
                match jobs::run_persona_classification_once(
                    &db,
                    cfg.as_ref(),
                    event_bus.as_deref(),
                    None,
                )
                .await
                {
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

    tokio::spawn({
        let db = db.clone();
        let db_path = cfg.database.path.clone();
        async move {
            while sqlite_stats_rx.recv().await.is_some() {
                let span = tracing::info_span!("job_run", job = "sqlite_stats");
                let _g = span.enter();
                if let Err(e) = jobs::run_sqlite_stats_once(&db, &db_path).await {
                    tracing::error!(error = %e, "sqlite_stats failed");
                }
            }
        }
    });

    tracing::info!("all worker loops spawned and ready");

    // ── Start scheduler AFTER worker loops are ready ──
    // Workers are now listening, so immediate messages will be received.
    let _scheduler_handles = scheduler::start(scheduler_jobs);
    tracing::info!("scheduler started (runs immediately on existing data)");

    // ── Bootstrap: Run all jobs concurrently for immediate startup ──
    tracing::info!("bootstrap: running all jobs in parallel");

    let (scoring_res, wallet_res, leaderboard_res, classification_res, rules_res) = tokio::join!(
        jobs::run_event_scoring_once(&db, api.as_ref(), cfg.as_ref(), event_bus.as_deref()),
        jobs::run_wallet_discovery_once(
            &db,
            api.as_ref(),
            api.as_ref(),
            cfg.as_ref(),
            event_bus.as_deref()
        ),
        jobs::run_leaderboard_discovery_once(&db, api.as_ref(), cfg.as_ref()),
        jobs::run_persona_classification_once(&db, cfg.as_ref(), event_bus.as_deref(), None),
        jobs::run_wallet_rules_once(&db, cfg.as_ref(), event_bus.as_deref()),
    );

    match scoring_res {
        Ok(n) => tracing::info!(inserted = n, "bootstrap: event_scoring done"),
        Err(e) => tracing::error!(error = %e, "bootstrap: event_scoring failed"),
    }
    match wallet_res {
        Ok(n) => tracing::info!(inserted = n, "bootstrap: wallet_discovery done"),
        Err(e) => tracing::error!(error = %e, "bootstrap: wallet_discovery failed"),
    }
    match leaderboard_res {
        Ok(n) => tracing::info!(inserted = n, "bootstrap: leaderboard_discovery done"),
        Err(e) => tracing::error!(error = %e, "bootstrap: leaderboard_discovery failed"),
    }
    match classification_res {
        Ok(classified) => tracing::info!(classified, "bootstrap: persona_classification done"),
        Err(e) => tracing::error!(error = %e, "bootstrap: persona_classification failed"),
    }
    match rules_res {
        Ok(changed) => tracing::info!(changed, "bootstrap: wallet_rules done"),
        Err(e) => tracing::error!(error = %e, "bootstrap: wallet_rules failed"),
    }

    tracing::info!("bootstrap done — worker loops receiving scheduler ticks");

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down (force exit in 5s)");

    // Give spawned tasks a moment to finish, then force exit.
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        tracing::warn!("force exit after timeout");
        std::process::exit(0);
    });

    Ok(())
}
