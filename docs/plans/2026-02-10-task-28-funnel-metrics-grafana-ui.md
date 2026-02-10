# Task 28: Funnel Metrics in Grafana + UI Views Implementation Plan

> **For Claude/Codex:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Prometheus metrics and dashboard UI views required by Strategy Bible §2 (funnel drop-offs) and §10 (journey view, excluded wallets, portfolio overview).

**Architecture:** Extend `crates/web` to (1) expose `/metrics` in Prometheus exposition format, (2) compute and export funnel stage gauges derived from SQLite, and (3) add the missing UI pages/sections driven by read-only SQL queries. Keep evaluator core logic unchanged.

**Tech Stack:** Rust, axum, askama, htmx, SQLite (rusqlite), `metrics` + `metrics-exporter-prometheus`, Grafana Alloy (remote write).

---

### Task 1: Add Web `/metrics` Endpoint (Prometheus)

**Files:**
- Create: `crates/web/src/metrics.rs`
- Modify: `crates/web/src/main.rs`

**Step 1: Write failing test**
- Add a test that `GET /metrics` returns 200 and contains a known metric name (e.g. `evaluator_web_build_info`).

**Step 2: Run test to verify it fails**
- Run: `cargo test -p web`
- Expected: FAIL (route not found / 404).

**Step 3: Implement minimal `/metrics`**
- Install a global Prometheus recorder once (guard with `OnceLock`).
- Add `GET /metrics` public route that returns `text/plain; version=0.0.4`.
- Spawn a small upkeep task that calls `PrometheusHandle::run_upkeep()` periodically.

**Step 4: Run tests**
- Run: `cargo test -p web`
- Expected: PASS.

---

### Task 2: Persona Funnel Counts (Stage 1/2, Paper, Follow-Worthy)

**Files:**
- Modify: `crates/web/src/models.rs`
- Modify: `crates/web/src/queries.rs`
- Modify: `crates/web/templates/dashboard.html`
- Create: `crates/web/templates/partials/persona_funnel_bar.html`
- Modify: `crates/web/src/main.rs`

**Step 1: Write failing tests**
- Query test: insert wallets + exclusions/personas + paper trades + scores; assert computed counts match expectations.
- Handler test: `GET /partials/persona_funnel` returns 200 and contains stage labels.

**Step 2: Implement**
- Add `PersonaFunnelCounts` and `PersonaFunnelStage`.
- Add SQL queries:
  - `wallets_discovered` (count `wallets`)
  - `stage1_passed` (watchlist wallets without `wallet_exclusions.reason LIKE 'STAGE1_%'`)
  - `stage2_classified` (stage1_passed wallets with `wallet_personas` OR non-stage1 exclusion)
  - `paper_traded_wallets` (distinct `paper_trades.proxy_wallet`)
  - `follow_worthy_wallets` (best-effort: wallets with both 7d and 30d `wallet_scores_daily.paper_roi_pct` thresholds)

**Step 3: Wire into UI**
- Add a persona funnel bar near the top of the dashboard, polling every 30s.

**Step 4: Run tests**
- Run: `cargo test -p web`
- Expected: PASS.

---

### Task 3: Export Funnel Gauges via `/metrics`

**Files:**
- Modify: `crates/web/src/metrics.rs`
- Modify: `crates/web/src/main.rs`

**Step 1: Write failing test**
- After inserting DB data, call `/metrics` and assert it contains series for the persona funnel stages (e.g. `evaluator_persona_funnel_stage_count{stage=\"stage1_passed\"}`).

**Step 2: Implement**
- On each `/metrics` request, query funnel counts from SQLite and set gauges:
  - `evaluator_pipeline_funnel_stage_count{stage=\"markets_scored\"}` etc (existing pipeline funnel)
  - `evaluator_persona_funnel_stage_count{stage=\"stage1_passed\"}` etc (new persona funnel)

**Step 3: Run tests**
- Run: `cargo test -p web`
- Expected: PASS.

---

### Task 4: Excluded Wallets List (Paginated UI)

**Files:**
- Modify: `crates/web/src/models.rs`
- Modify: `crates/web/src/queries.rs`
- Modify: `crates/web/src/main.rs`
- Create: `crates/web/templates/excluded.html`
- Create: `crates/web/templates/partials/excluded_table.html`

**Step 1: Write failing tests**
- `GET /excluded` returns 200 and includes \"Excluded Wallets\".
- Pagination works (`?page=2` shows different rows).

**Step 2: Implement**
- Query latest exclusion per wallet (group by wallet, pick max excluded_at).
- Render table with wallet, reason, metric_value vs threshold, excluded_at.

**Step 3: Run tests**
- Run: `cargo test -p web`
- Expected: PASS.

---

### Task 5: Journey View (Per Wallet)

**Files:**
- Modify: `crates/web/src/models.rs`
- Modify: `crates/web/src/queries.rs`
- Modify: `crates/web/src/main.rs`
- Create: `crates/web/templates/journey.html`

**Step 1: Write failing tests**
- `GET /journey/:wallet` returns 404 for unknown wallet, 200 for known wallet.
- Response includes \"Journey\" and shows discovered_at.

**Step 2: Implement**
- Build a `WalletJourney` view model with best-available fields:
  - persona (latest) + confidence
  - exclusion (latest, if any)
  - paper pnl (sum)
  - exposure (sum from `paper_positions`)
  - copy fidelity (from `copy_fidelity_events`)
  - follower slippage avg (from `follower_slippage`)
  - journey events timeline (discovered, classified/excluded, first paper trade)

**Step 3: Run tests**
- Run: `cargo test -p web`
- Expected: PASS.

---

### Task 6: Portfolio Overview Enhancements

**Files:**
- Modify: `crates/web/src/models.rs`
- Modify: `crates/web/src/queries.rs`
- Modify: `crates/web/templates/partials/paper.html`

**Step 1: Write failing test**
- Extend existing `test_paper_summary_calculates_pnl` to assert exposure/wallet-followed fields are computed.

**Step 2: Implement**
- Add fields: exposure_usdc + exposure_pct, wallets_followed, copy_fidelity_avg_pct (best-effort), follower_slippage_avg_cents (best-effort), risk_status string (computed from exposure vs config limits).

**Step 3: Run tests**
- Run: `cargo test -p web`
- Expected: PASS.

---

### Task 7: Grafana Dashboard Artifact + Push Script

**Files:**
- Create: `deploy/dashboards/evaluator-funnel.json`
- Create: `deploy/push-dashboards.sh`

**Implementation:**
- Add a Grafana dashboard JSON with panels for:
  - Persona funnel stage counts
  - Drop-off rates computed from ratios
- Add a `deploy/push-dashboards.sh` that uses `GRAFANA_URL` + `GRAFANA_SA_TOKEN` to POST dashboards via Grafana API.

---

### Task 8: Verification

Run:
- `cargo test`
- (Optional) `cargo clippy --all-targets --all-features -D warnings`

