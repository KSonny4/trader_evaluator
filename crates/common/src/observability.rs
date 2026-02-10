use std::borrow::Cow;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_sdk::Resource;
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{EnvFilter, Layer};

/// Guard object that ensures tracer provider shutdown (flush) on drop.
///
/// We intentionally use the global tracer provider shutdown because
/// `tracing-opentelemetry` wiring is global within the process anyway.
pub struct OtelGuard {
    _private: (),
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        // Best-effort flush on shutdown.
        opentelemetry::global::shutdown_tracer_provider();
    }
}

struct ErrorCounterLayer;

impl<S> Layer<S> for ErrorCounterLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() == tracing::Level::ERROR {
            metrics::counter!("tracing_error_events").increment(1);
        }
    }
}

/// Build a `tracing` dispatcher configured for:
/// - JSON logs to stdout
/// - EnvFilter that respects `RUST_LOG` (takes precedence) and falls back to `default_level`
/// - `tracing_error_events` counter for ERROR events
/// - Optional OpenTelemetry OTLP trace export when `OTEL_EXPORTER_OTLP_ENDPOINT` is set
pub fn build_dispatch(
    service_name: impl Into<Cow<'static, str>>,
    default_level: &str,
) -> (tracing::Dispatch, Option<OtelGuard>) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_timer(tracing_subscriber::fmt::time::SystemTime)
        .json();

    let error_counter_layer = ErrorCounterLayer;

    let service_name = service_name.into();

    // Only enable OTLP export if the endpoint env var exists. This avoids noisy
    // local dev behavior and keeps tests deterministic.
    let otel_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();

    if let Some(endpoint) = otel_endpoint {
        use opentelemetry_otlp::WithExportConfig;

        // Configure OTLP exporter (HTTP/protobuf). The systemd unit will set
        // `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf` but we explicitly use HTTP here.
        let Ok(exporter) = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .build()
        else {
            // Best-effort: if exporter build fails, fall back to logs+metrics only.
            let subscriber = tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .with(error_counter_layer);
            return (tracing::Dispatch::new(subscriber), None);
        };

        let resource = Resource::new(vec![KeyValue::new(
            "service.name",
            service_name.to_string(),
        )]);

        // Requires a Tokio runtime; both binaries are `#[tokio::main]`.
        let provider = opentelemetry_sdk::trace::TracerProvider::builder()
            .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
            .with_resource(resource)
            .build();

        let tracer = provider.tracer("trader_evaluator");
        let _ = opentelemetry::global::set_tracer_provider(provider);

        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        let subscriber = tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .with(error_counter_layer)
            .with(otel_layer);

        (
            tracing::Dispatch::new(subscriber),
            Some(OtelGuard { _private: () }),
        )
    } else {
        let subscriber = tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .with(error_counter_layer);

        (tracing::Dispatch::new(subscriber), None)
    }
}
