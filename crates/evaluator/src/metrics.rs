use anyhow::Result;
use metrics::{describe_counter, describe_gauge, describe_histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::net::SocketAddr;

pub fn describe() {
    describe_counter!(
        "evaluator_markets_scored_total",
        "Number of markets scored by MScore."
    );
    describe_counter!(
        "evaluator_wallets_discovered_total",
        "Number of wallets discovered."
    );
    describe_gauge!(
        "evaluator_wallets_on_watchlist",
        "Current wallets on watchlist."
    );
    describe_counter!(
        "evaluator_trades_ingested_total",
        "Number of trades ingested into trades_raw."
    );
    describe_counter!(
        "evaluator_paper_trades_total",
        "Number of paper trades created."
    );
    describe_gauge!("evaluator_paper_pnl", "Paper PnL (USDC).");
    describe_counter!(
        "evaluator_api_requests_total",
        "Number of API requests made."
    );
    describe_histogram!(
        "evaluator_api_latency_ms",
        "API request latency in milliseconds."
    );
    describe_gauge!(
        "evaluator_ingestion_lag_secs",
        "Ingestion lag (seconds) from newest observed trade."
    );
    describe_counter!(
        "evaluator_risk_violations_total",
        "Number of risk rule violations."
    );
}

pub fn install_prometheus(port: u16) -> Result<PrometheusHandle> {
    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    Ok(PrometheusBuilder::new()
        .with_http_listener(addr)
        .install_recorder()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prometheus_handle_renders_metric_names() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        describe();

        metrics::with_local_recorder(&recorder, || {
            let c = metrics::counter!("evaluator_markets_scored_total");
            c.increment(1);
        });

        let rendered = handle.render();
        assert!(rendered.contains("evaluator_markets_scored_total"));
    }
}
