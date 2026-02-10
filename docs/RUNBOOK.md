# Runbook

## Service Control

Status:
- `sudo systemctl status evaluator`
- `sudo systemctl status web`
- `sudo systemctl status alloy`

Restart:
- `sudo systemctl restart evaluator`
- `sudo systemctl restart web`
- `sudo systemctl restart alloy`

Logs:
- `sudo journalctl -u evaluator -n 200 --no-pager`
- `sudo journalctl -u web -n 200 --no-pager`
- `sudo journalctl -u alloy -n 200 --no-pager`

## Health Checks

Metrics endpoints (server localhost):
- Evaluator: `curl -s http://127.0.0.1:9094/metrics | head`
- Web: `curl -s http://127.0.0.1:3000/metrics | head`

Traces receiver:
- `sudo ss -ltnp | rg 4318`

## Common Failures

### No Metrics In Grafana

1. Check local endpoints respond (`/metrics` curls above).
2. Check Alloy is running and has env vars:
   - `sudo systemctl show alloy -p Environment | rg GRAFANA_CLOUD`
3. Check Alloy logs for auth/remote_write errors:
   - `sudo journalctl -u alloy -n 200 --no-pager`

### No Logs In Loki

1. Verify journald has entries:
   - `sudo journalctl -u evaluator -n 50 --no-pager`
2. Verify Alloy Loki config env vars present:
   - `sudo systemctl show alloy -p Environment | rg GRAFANA_CLOUD_LOKI`

### No Traces In Tempo

1. Confirm OTLP receiver is listening (4318).
2. Confirm services have OTEL env vars:
   - `sudo systemctl show evaluator -p Environment | rg OTEL_`
   - `sudo systemctl show web -p Environment | rg OTEL_`
3. Check Alloy logs for exporter errors.

