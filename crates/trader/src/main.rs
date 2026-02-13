mod api;
mod config;
mod db;
mod engine;
mod polymarket;
mod risk;
mod types;

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load config
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(config::TraderConfig::default_config_path);
    info!(path = %config_path, "loading trader config");
    let config = config::TraderConfig::load(&config_path)?;
    let config = Arc::new(config);

    // Create Polymarket client
    let client = Arc::new(polymarket::TraderPolymarketClient::new(
        &config.polymarket.data_api_url,
        config.polymarket.rate_limit_delay_ms,
    ));

    // Create wallet engine with risk manager
    let engine_db = Arc::new(db::TraderDb::open(&config.database.path).await?);
    let risk_manager = Arc::new(risk::RiskManager::new(
        Arc::clone(&engine_db),
        config.risk.clone(),
    ));
    let mut engine_instance = engine::WalletEngine::new(
        Arc::clone(&engine_db),
        Arc::clone(&client),
        Arc::clone(&config),
        Arc::clone(&risk_manager),
    );

    // Restore watchers for active wallets
    engine_instance.restore_watchers().await?;

    // Build app state — share the same DB as the engine
    let state = Arc::new(api::AppState {
        db: Arc::clone(&engine_db),
        engine: Mutex::new(engine_instance),
        risk: Arc::clone(&risk_manager),
        started_at: chrono::Utc::now(),
        api_key: config.server.api_key.clone(),
    });

    // Build router
    let app = api::router(state);

    // Start HTTP server
    let bind_addr = format!("{}:{}", config.server.host, config.server.port);
    info!(addr = %bind_addr, "starting trader HTTP server");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
