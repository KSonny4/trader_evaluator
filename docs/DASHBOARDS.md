# Dashboards

Grafana dashboard JSON lives in:

- `/Users/petr.kubelka/git_projects/trader_evaluator/dashboards`

Dashboards assume Grafana Cloud datasources with UIDs:

- Prometheus: `grafanacloud-prom`
- Loki: `grafanacloud-logs`

If your stack uses different datasource UIDs, update the JSON once and re-push.

## Pushing Dashboards To Grafana Cloud

1. Provide credentials (either option works):
   - Option A (recommended): `cp .env.agent.example .env.agent`
   - Option B: set `GRAFANA_URL` + `GRAFANA_SA_TOKEN` in `.env`
2. Set:
   - `GRAFANA_URL` (e.g. `https://your-slug.grafana.net`)
   - `GRAFANA_SA_TOKEN` (Grafana Service Account token with Editor/Admin)
3. Push (either works):
   - `source .env.agent && ./deploy/push-dashboards.sh`
   - `./deploy/push-dashboards.sh` (auto-loads `.env.agent` then `.env`)

`deploy/push-dashboards.sh` uploads dashboards into a Grafana folder named `trader-evaluator`.
