use anyhow::Result;
use metrics::describe_gauge;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;

static PROM_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

pub fn describe() {
    describe_gauge!(
        "evaluator_web_build_info",
        "Build info for the evaluator web dashboard (value is always 1)."
    );
    describe_gauge!(
        "evaluator_pipeline_funnel_stage_count",
        "Pipeline funnel stage counts (derived from SQLite) for UI/Grafana."
    );
    describe_gauge!(
        "evaluator_persona_funnel_stage_count",
        "Persona funnel stage counts (derived from SQLite) for UI/Grafana."
    );
}

/// Install a global Prometheus recorder exactly once and return a handle for rendering `/metrics`.
///
/// Note: `PrometheusBuilder::install_recorder` requires the caller to run upkeep periodically.
/// We run upkeep opportunistically on each `/metrics` request.
pub fn init_global() -> Result<PrometheusHandle> {
    let handle = PROM_HANDLE.get_or_init(|| {
        // Descriptor registration is idempotent, so it's fine to call each time.
        describe();

        PrometheusBuilder::new()
            .install_recorder()
            .expect("failed to install Prometheus recorder for web")
    });

    // Set a stable "build info" gauge.
    // We intentionally keep labels minimal; Grafana can join on job/instance.
    let git_sha = std::env::var("GIT_SHA").unwrap_or_else(|_| "unknown".to_string());
    ::metrics::gauge!(
        "evaluator_web_build_info",
        "version" => env!("CARGO_PKG_VERSION"),
        "git_sha" => git_sha,
    )
    .set(1.0);

    Ok(handle.clone())
}
