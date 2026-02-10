# Deployment

Deployment is based on:
- cross-compiling static Linux binaries on macOS
- systemd services on the server
- Grafana Alloy on the server for metrics/logs/traces shipping

Key scripts:
- `deploy/setup-server.sh`: install prerequisites (Alloy, node_exporter)
- `deploy/deploy.sh`: build + upload + install systemd units + configure Alloy + (optionally) push dashboards

## First-Time Server Setup

1. Provision or pick a server and ensure SSH access.
2. Run on the server:
   - `./deploy/setup-server.sh`
3. Ensure `/opt/evaluator` exists (created by deploy).

## Deploy

From your local machine:
- `./deploy/deploy.sh <SERVER_IP>`

What `deploy/deploy.sh` does (high level):
1. Cross-compiles `evaluator` + `web` for `x86_64-unknown-linux-musl`.
2. Uploads binaries to `/opt/evaluator`.
3. Uploads `config/default.toml` (and `.env` if present).
4. Installs systemd units.
5. If `.env` contains `GRAFANA_CLOUD*`, installs `/etc/alloy/config.alloy` and injects env vars into `/etc/default/alloy`, then restarts Alloy.
6. If `.env.agent` provides `GRAFANA_URL` + `GRAFANA_SA_TOKEN`, pushes dashboards.
7. Restarts `evaluator` + `web`.

## Common Paths (Server)

- App dir: `/opt/evaluator`
- Config: `/opt/evaluator/config/default.toml`
- Data: `/opt/evaluator/data/evaluator.db`
- Alloy config: `/etc/alloy/config.alloy`
- Alloy env: `/etc/default/alloy`

