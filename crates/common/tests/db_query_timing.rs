use metrics_exporter_prometheus::PrometheusBuilder;

#[test]
fn asyncdb_call_named_records_latency_and_errors() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();

    let rt = tokio::runtime::Runtime::new().unwrap();
    metrics::with_local_recorder(&recorder, || {
        rt.block_on(async {
            let tmp = tempfile::NamedTempFile::new().unwrap();
            let db = common::db::AsyncDb::open(tmp.path().to_str().unwrap())
                .await
                .unwrap();

            // Success path should record a histogram sample.
            let v: i64 = db.call_named("test.ok", |_conn| Ok(1)).await.unwrap();
            assert_eq!(v, 1);

            // Error path should increment errors counter and record latency with status=err.
            let err: anyhow::Result<()> = db
                .call_named("test.err", |conn| {
                    let _ = conn.execute("SELECT * FROM definitely_missing_table", [])?;
                    Ok(())
                })
                .await;
            assert!(err.is_err());
        });
    });

    let rendered = handle.render();
    assert!(
        rendered.contains("evaluator_db_query_latency_ms"),
        "expected evaluator_db_query_latency_ms in rendered metrics, got:\n{rendered}"
    );
    assert!(
        rendered.contains("evaluator_db_query_errors_total"),
        "expected evaluator_db_query_errors_total in rendered metrics, got:\n{rendered}"
    );
    // New split timing metrics
    assert!(
        rendered.contains("evaluator_db_queue_wait_ms"),
        "expected evaluator_db_queue_wait_ms in rendered metrics, got:\n{rendered}"
    );
    assert!(
        rendered.contains("evaluator_db_exec_ms"),
        "expected evaluator_db_exec_ms in rendered metrics, got:\n{rendered}"
    );
}

#[test]
fn asyncdb_call_named_tracks_queue_depth() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();

    let rt = tokio::runtime::Runtime::new().unwrap();
    metrics::with_local_recorder(&recorder, || {
        rt.block_on(async {
            let tmp = tempfile::NamedTempFile::new().unwrap();
            let db = common::db::AsyncDb::open(tmp.path().to_str().unwrap())
                .await
                .unwrap();

            // After a call completes, queue depth should be back to 0.
            db.call_named("test.depth", |_conn| Ok(())).await.unwrap();
        });
    });

    let rendered = handle.render();
    assert!(
        rendered.contains("evaluator_db_queue_depth"),
        "expected evaluator_db_queue_depth in rendered metrics, got:\n{rendered}"
    );
}
