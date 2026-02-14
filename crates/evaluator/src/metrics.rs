use anyhow::Result;
use metrics::{describe_counter, describe_gauge, describe_histogram};
use metrics_exporter_prometheus::Matcher;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::SocketAddr;

const HISTOGRAM_BUCKETS_MS: &[f64] = &[
    1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
];

const HISTOGRAM_BUCKETS_SECONDS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

pub fn describe() {
    describe_counter!(
        "tracing_error_events",
        "Cumulative count of all ERROR-level tracing events."
    );
    describe_histogram!(
        "evaluator_db_query_latency_ms",
        "SQLite DB operation total latency in milliseconds (queue wait + execution)."
    );
    describe_histogram!(
        "evaluator_db_queue_wait_ms",
        "Time waiting for the SQLite background thread in milliseconds."
    );
    describe_histogram!(
        "evaluator_db_exec_ms",
        "Actual SQLite execution time in milliseconds (excludes queue wait)."
    );
    describe_gauge!(
        "evaluator_db_queue_depth",
        "Number of operations queued for the SQLite background thread."
    );
    describe_counter!(
        "evaluator_db_query_errors_total",
        "SQLite DB operation errors."
    );
    // SQLite file/page stats (collected periodically)
    describe_gauge!(
        "evaluator_db_file_size_bytes",
        "SQLite database file size in bytes."
    );
    describe_gauge!(
        "evaluator_db_wal_size_bytes",
        "SQLite WAL file size in bytes (0 if not present)."
    );
    describe_gauge!(
        "evaluator_db_page_count",
        "Total pages in the SQLite database."
    );
    describe_gauge!("evaluator_db_page_size_bytes", "SQLite page size in bytes.");
    describe_gauge!(
        "evaluator_db_freelist_count",
        "Number of free (wasted) pages in the SQLite database."
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
    // Event bus observability
    describe_counter!(
        "evaluator_events_emitted_total",
        "Total events emitted by the event bus, labeled by event_type."
    );
    describe_counter!(
        "evaluator_events_dropped_total",
        "Total events dropped (no subscribers), labeled by event_type."
    );
    describe_gauge!(
        "evaluator_event_bus_size",
        "Current number of pending events in the event bus."
    );
    // Event-driven orchestration metrics
    describe_counter!(
        "evaluator_event_triggers_fired_total",
        "Total event-driven triggers fired, labeled by trigger_type."
    );
    describe_histogram!(
        "evaluator_event_trigger_latency_seconds",
        "Latency from event emission to trigger execution in seconds, labeled by trigger_type."
    );
    describe_histogram!(
        "evaluator_classification_batch_size",
        "Number of wallets per classification batch."
    );
    // Flow visualization (funnel + classification) — current counts for Grafana Canvas/Node Graph
    describe_gauge!(
        "evaluator_flow_funnel_markets_fetched",
        "Funnel: total markets in DB."
    );
    describe_gauge!(
        "evaluator_flow_funnel_markets_scored_today",
        "Funnel: markets scored today (MScore)."
    );
    describe_gauge!(
        "evaluator_flow_funnel_wallets_discovered",
        "Funnel: total wallets discovered."
    );
    describe_gauge!(
        "evaluator_flow_funnel_wallets_tracked",
        "Funnel: active wallets on watchlist."
    );
    describe_gauge!(
        "evaluator_flow_funnel_wallets_ranked_today",
        "Funnel: wallets with WScore today."
    );
    describe_gauge!(
        "evaluator_flow_classification_wallets_tracked",
        "Classification: active wallets (same as funnel)."
    );
    describe_gauge!(
        "evaluator_flow_classification_stage1_excluded",
        "Classification: excluded at Stage 1 (fast filters)."
    );
    describe_gauge!(
        "evaluator_flow_classification_stage1_passed",
        "Classification: passed Stage 1."
    );
    describe_gauge!(
        "evaluator_flow_classification_stage2_followable",
        "Classification: followable persona at Stage 2."
    );
    describe_gauge!(
        "evaluator_flow_classification_stage2_excluded",
        "Classification: excluded at Stage 2."
    );
    describe_gauge!(
        "evaluator_flow_classification_stage2_unclassified",
        "Classification: passed Stage 1, not yet classified at Stage 2."
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
            Matcher::Full("evaluator_event_trigger_latency_seconds".to_string()),
            HISTOGRAM_BUCKETS_SECONDS,
        )
        .map_err(anyhow::Error::from)?
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

    #[test]
    fn test_event_bus_metrics_described_and_recorded_in_prometheus_output() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        describe();

        metrics::with_local_recorder(&recorder, || {
            // Emit some pipeline events
            metrics::counter!("evaluator_events_emitted_total", "event_type" => "pipeline")
                .increment(3);
            metrics::counter!("evaluator_events_emitted_total", "event_type" => "operational")
                .increment(2);

            // Record some dropped events
            metrics::counter!("evaluator_events_dropped_total", "event_type" => "pipeline")
                .increment(1);

            // Set bus size gauge
            metrics::gauge!("evaluator_event_bus_size").set(5.0);
        });

        let rendered = handle.render();

        // Verify emitted counter appears with both event_type labels
        assert!(
            rendered.contains("evaluator_events_emitted_total"),
            "events emitted counter should appear in Prometheus output"
        );
        assert!(
            rendered.contains(r#"event_type="pipeline""#),
            "pipeline event_type label should appear"
        );
        assert!(
            rendered.contains(r#"event_type="operational""#),
            "operational event_type label should appear"
        );

        // Verify dropped counter appears
        assert!(
            rendered.contains("evaluator_events_dropped_total"),
            "events dropped counter should appear in Prometheus output"
        );

        // Verify bus size gauge appears
        assert!(
            rendered.contains("evaluator_event_bus_size"),
            "event bus size gauge should appear in Prometheus output"
        );
    }

    #[test]
    fn test_flow_gauges_described_and_recorded_in_prometheus_output() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        describe();

        let counts = crate::flow_metrics::FlowCounts {
            funnel: crate::flow_metrics::FunnelFlowCounts {
                markets_fetched: 10,
                markets_scored_today: 5,
                wallets_discovered: 100,
                wallets_tracked: 80,
                wallets_ranked_today: 3,
            },
            classification: crate::flow_metrics::ClassificationFlowCounts {
                wallets_tracked: 80,
                stage1_excluded: 5,
                stage1_passed: 75,
                stage2_followable: 20,
                stage2_excluded: 2,
                stage2_unclassified: 53,
            },
        };

        metrics::with_local_recorder(&recorder, || {
            crate::flow_metrics::record_flow_counts(&counts);
        });

        let rendered = handle.render();
        assert!(
            rendered.contains("evaluator_flow_funnel_markets_fetched"),
            "flow funnel gauges should appear in Prometheus output"
        );
        assert!(
            rendered.contains("evaluator_flow_classification_stage2_followable"),
            "flow classification gauges should appear in Prometheus output"
        );
    }

    fn free_local_port() -> u16 {
        // Bind to an ephemeral port to reserve a likely-free port number.
        // There is a small race between releasing it and our server binding,
        // but this is acceptable for test purposes.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    #[test]
    fn test_event_trigger_metrics_described_and_recorded_in_prometheus_output() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            // Register descriptions with the local recorder
            describe();

            // Record trigger fired counters for each trigger type
            metrics::counter!(
                "evaluator_event_triggers_fired_total",
                "trigger_type" => "discovery"
            )
            .increment(3);
            metrics::counter!(
                "evaluator_event_triggers_fired_total",
                "trigger_type" => "classification"
            )
            .increment(2);
            metrics::counter!(
                "evaluator_event_triggers_fired_total",
                "trigger_type" => "fast_path"
            )
            .increment(1);

            // Record trigger latency histograms
            metrics::histogram!(
                "evaluator_event_trigger_latency_seconds",
                "trigger_type" => "discovery"
            )
            .record(0.15);
            metrics::histogram!(
                "evaluator_event_trigger_latency_seconds",
                "trigger_type" => "classification"
            )
            .record(0.25);
            metrics::histogram!(
                "evaluator_event_trigger_latency_seconds",
                "trigger_type" => "fast_path"
            )
            .record(0.01);

            // Record classification batch size histogram
            metrics::histogram!("evaluator_classification_batch_size").record(42.0);
        });

        let rendered = handle.render();

        // Verify triggers fired counter appears with all trigger_type labels
        assert!(
            rendered.contains("evaluator_event_triggers_fired_total"),
            "event triggers fired counter should appear in Prometheus output"
        );
        assert!(
            rendered.contains(r#"trigger_type="discovery""#),
            "discovery trigger_type label should appear"
        );
        assert!(
            rendered.contains(r#"trigger_type="classification""#),
            "classification trigger_type label should appear"
        );
        assert!(
            rendered.contains(r#"trigger_type="fast_path""#),
            "fast_path trigger_type label should appear"
        );

        // Verify trigger latency histogram appears
        assert!(
            rendered.contains("evaluator_event_trigger_latency_seconds"),
            "event trigger latency histogram should appear in Prometheus output"
        );

        // Verify classification batch size histogram appears
        assert!(
            rendered.contains("evaluator_classification_batch_size"),
            "classification batch size histogram should appear in Prometheus output"
        );

        // Verify HELP lines (descriptions) are present — proves describe() registered them
        assert!(
            rendered.contains("# HELP evaluator_event_triggers_fired_total"),
            "triggers fired counter should have a HELP description"
        );
        assert!(
            rendered.contains("# HELP evaluator_event_trigger_latency_seconds"),
            "trigger latency histogram should have a HELP description"
        );
        assert!(
            rendered.contains("# HELP evaluator_classification_batch_size"),
            "classification batch size histogram should have a HELP description"
        );
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
