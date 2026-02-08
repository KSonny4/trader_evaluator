use anyhow::Result;

mod ingestion;
mod market_scoring;
mod paper_trading;
mod wallet_discovery;
mod wallet_scoring;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .json()
        .init();

    tracing::info!("trader_evaluator starting");

    // Task 2+3 will wire up config loading and DB migrations.

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    Ok(())
}
