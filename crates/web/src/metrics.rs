use anyhow::Result;
use metrics::describe_gauge;

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

/// Describe metrics and set a stable build-info gauge.
///
/// The global recorder/exporter is installed in `main.rs` via `PrometheusBuilder::install()`.
pub fn init() -> Result<()> {
    // Descriptor registration is idempotent, so it's fine to call each time.
    describe();

    // Set a stable "build info" gauge.
    // We intentionally keep labels minimal; Grafana can join on job/instance.
    let git_sha = std::env::var("GIT_SHA").unwrap_or_else(|_| "unknown".to_string());
    ::metrics::gauge!(
        "evaluator_web_build_info",
        "version" => env!("CARGO_PKG_VERSION"),
        "git_sha" => git_sha,
    )
    .set(1.0);

    Ok(())
}
