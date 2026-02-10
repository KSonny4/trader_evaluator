use anyhow::Result;
use metrics::{describe_counter, describe_gauge, describe_histogram};
use metrics_exporter_prometheus::Matcher;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::SocketAddr;

const HISTOGRAM_BUCKETS_MS: &[f64] = &[
    1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
];

pub fn describe() {
    describe_counter!(
        "tracing_error_events",
        "Cumulative count of all ERROR-level tracing events."
    );
    describe_histogram!(
        "evaluator_db_query_latency_ms",
        "SQLite DB operation latency in milliseconds."
    );
    describe_counter!(
        "evaluator_db_query_errors_total",
        "SQLite DB operation errors."
    );
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
    describe_counter!(
        "evaluator_api_errors_total",
        "Number of API request failures classified by kind."
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
    describe_counter!(
        "evaluator_recovery_paper_trades_total",
        "Paper trades processed during startup recovery (after process was killed)."
    );
}

pub fn install_prometheus(port: u16) -> Result<()> {
    // Bind to localhost by default. This keeps the metrics endpoint private on the host
    // (Grafana/Alloy can scrape via localhost) and avoids accidentally exposing it publicly.
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();

    // IMPORTANT: `install_recorder` only installs the recorder (no HTTP listener).
    // Use `install` to spawn the exporter task so /metrics is actually served.
    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Prefix("evaluator_".to_string()),
            HISTOGRAM_BUCKETS_MS,
        )
        .map_err(anyhow::Error::from)?
        .with_http_listener(addr)
        .install()
        .map_err(anyhow::Error::msg)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    #[test]
    fn test_prometheus_handle_renders_metric_names() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        describe();

        metrics::with_local_recorder(&recorder, || {
            let c = metrics::counter!("evaluator_markets_scored_total");
            c.increment(1);
            metrics::counter!("tracing_error_events").increment(1);
        });

        let rendered = handle.render();
        assert!(rendered.contains("evaluator_markets_scored_total"));
        assert!(rendered.contains("tracing_error_events"));
    }

    fn free_local_port() -> u16 {
        // Bind to an ephemeral port to reserve a likely-free port number.
        // There is a small race between releasing it and our server binding,
        // but this is acceptable for test purposes.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    #[tokio::test]
    #[ignore] // Requires opening local TCP sockets; not available in some sandboxed environments.
    async fn test_install_prometheus_starts_http_listener() {
        let port = free_local_port();

        // This should start an HTTP listener serving /metrics.
        install_prometheus(port).unwrap();

        // Wait briefly for the listener to come up.
        let addr = format!("127.0.0.1:{port}");
        let mut last_err: Option<String> = None;
        for _ in 0..50 {
            match TcpStream::connect(&addr).await {
                Ok(mut stream) => {
                    stream
                        .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
                        .await
                        .unwrap();
                    let mut buf = vec![0u8; 1024];
                    let n = stream.read(&mut buf).await.unwrap();
                    let s = String::from_utf8_lossy(&buf[..n]);
                    assert!(s.contains("200") || s.contains("# TYPE"), "response: {s}");
                    return;
                }
                Err(e) => {
                    last_err = Some(e.to_string());
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
        }

        panic!("metrics listener did not start on {addr}; last_err={last_err:?}");
    }
}
