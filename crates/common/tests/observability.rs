use metrics_exporter_prometheus::PrometheusBuilder;

// NOTE: This is an integration test so it exercises the public API surface
// (`common::observability`) instead of reaching into private internals.

#[test]
fn tracing_error_events_counter_increments_on_error_event() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();

    metrics::with_local_recorder(&recorder, || {
        // Build a subscriber that includes the error-counter layer.
        let (dispatch, _otel_guard) = common::observability::build_dispatch("test-service", "info");

        tracing::dispatcher::with_default(&dispatch, || {
            tracing::error!(foo = 123, "boom");
        });
    });

    let rendered = handle.render();
    assert!(
        rendered.contains("tracing_error_events"),
        "expected tracing_error_events in rendered metrics, got:\n{rendered}"
    );
}
