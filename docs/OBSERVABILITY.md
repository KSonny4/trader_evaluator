# Observability

This project uses the same Grafana Cloud stack pattern as `/Users/petr.kubelka/git_projects/trading`:

- **Metrics:** Prometheus exposition on localhost -> Grafana Alloy scrape -> Grafana Cloud Prometheus (remote_write)
- **Logs:** systemd journal -> Grafana Alloy -> Grafana Cloud Loki
- **Traces:** app OTLP (HTTP/protobuf) -> Grafana Alloy OTLP receiver -> Grafana Cloud Tempo

## Data Flow

1. `evaluator` exposes Prometheus metrics on `127.0.0.1:9094`.
2. `web` exposes Prometheus metrics on `127.0.0.1:3000` (health/telemetry counters).
3. Grafana Alloy scrapes both and remote-writes to Grafana Cloud Prometheus.
4. Both services log JSON to stdout; systemd captures it in journald; Alloy ships journald entries to Loki.
5. Both services export OTLP traces to `http://127.0.0.1:4318`; Alloy exports them to Tempo.

## Environment Variables

### Server: `/opt/evaluator/.env` (injected into Alloy via `/etc/default/alloy`)

- `GRAFANA_CLOUD_PROM_URL`
- `GRAFANA_CLOUD_PROM_USER`
- `GRAFANA_CLOUD_API_KEY`
- `GRAFANA_CLOUD_LOKI_URL`
- `GRAFANA_CLOUD_LOKI_USER`
- `GRAFANA_CLOUD_TEMPO_URL`
- `GRAFANA_CLOUD_TEMPO_USER`

### Services: systemd unit environment

Set in:
- `/Users/petr.kubelka/git_projects/trader_evaluator/deploy/systemd/evaluator.service`
- `/Users/petr.kubelka/git_projects/trader_evaluator/deploy/systemd/web.service`

Key vars:
- `RUST_LOG` (takes precedence over config file log_level)
- `OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318`
- `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf`
- `OTEL_SERVICE_NAME=...`

## Verification Checklist (Server)

1. Alloy is listening for OTLP:
   - `sudo ss -ltnp | rg 4318`
2. Metrics endpoints respond locally:
   - `curl -s http://127.0.0.1:9094/metrics | head`
   - `curl -s http://127.0.0.1:3000/metrics | head`
3. Logs exist:
   - `sudo journalctl -u evaluator -n 50 --no-pager`
   - `sudo journalctl -u evaluator-web -n 50 --no-pager`
4. Alloy isn’t erroring:
   - `sudo journalctl -u alloy -n 200 --no-pager`
5. Grafana Cloud:
   - Prometheus queries return data: `evaluator_trades_ingested_total`, `tracing_error_events`
   - Loki shows `unit=evaluator.service` and `unit=evaluator-web.service`
   - Tempo shows services `evaluator` and `evaluator-web`

