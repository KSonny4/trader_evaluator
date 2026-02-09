# Code Review: Task 12 — Persona Classification Orchestrator + Stage 2 Job

**Scope:** Uncommitted working-tree changes (persona_classification.rs, pipeline_jobs.rs, main.rs).  
**Plan:** MASTER_STRATEGY_IMPLEMENTATION_PLAN.md Task 12 + Strategy Bible.

---

### Strengths

- **Clear pipeline order:** `classify_wallet` documents and implements exclusion-first (Sniper → Noise → Tail Risk) then followable personas (Informed Specialist → Consistent Generalist → Patient Accumulator), matching the plan.
- **Config alignment:** `PersonaConfig` is built from `common::config::Personas` via `from_personas()`; no hardcoded thresholds in production path.
- **Focused tests:** Three orchestrator tests cover followable (with DB assert), excluded (with DB assert), and unclassified; use real DB and migrations.
- **Job structure:** Stage 2 job reuses existing `stage1_filter`, `compute_wallet_features`, and `classify_wallet`; single `db.call` for sync DB access; metric `evaluator_persona_classifications_run` emitted.
- **Scheduler wiring:** New job and spawn follow the same pattern as wallet_scoring; 24h interval and `run_immediately: false` are reasonable.

---

### Issues

#### Critical (Must Fix)

*None.*

#### Important (Should Fix)

1. **Errors in classification job are swallowed**
   - **Where:** `pipeline_jobs.rs`: `compute_wallet_features` and `classify_wallet` failures result in `continue` with no log.
   - **Why:** Failures (e.g. DB/IO or bug in feature computation) are invisible; debugging and ops are harder.
   - **Fix:** At least `tracing::warn!(proxy_wallet = %proxy_wallet, error = %e, "persona classification skipped")` when `compute_wallet_features` or `classify_wallet` returns `Err`, or increment a failure metric.

2. **`wallet_personas` can accumulate multiple rows per wallet**
   - **Where:** `persona_classification.rs` — `record_persona` uses `INSERT OR REPLACE` with `(proxy_wallet, persona, confidence, classified_at)` and schema has `UNIQUE(proxy_wallet, classified_at)`.
   - **Why:** Each run inserts a new row (new `classified_at`), so we get history but no single “current” classification per wallet unless the rest of the system always takes the latest row.
   - **Fix:** Either (a) document that “latest classification” is chosen elsewhere by `ORDER BY classified_at DESC LIMIT 1`, or (b) replace with a single row per wallet (e.g. `UNIQUE(proxy_wallet)` and `INSERT OR REPLACE` keyed by `proxy_wallet` only) if the product only needs current classification. Plan does not mandate history; this is a design clarification.

#### Minor (Nice to Have)

1. **Stage 1 total_trades is all-time**
   - **Where:** `pipeline_jobs.rs` — `total_trades` is `SELECT COUNT(*) FROM trades_raw WHERE proxy_wallet = w.proxy_wallet`.
   - **Why:** Strategy Bible says “Can't classify with fewer” but doesn’t say “in window”. All-time is a valid choice; if the intent was “trades in last N days”, the query would need a time window.
   - **Fix:** If product intent is rolling window, add a time filter; otherwise add a one-line comment that Stage 1 uses all-time trade count.

2. **Magic number for classification window**
   - **Where:** `pipeline_jobs.rs`: `let window_days = 30_u32`.
   - **Fix:** Consider `cfg.personas.stage1_min_wallet_age_days` or a dedicated `classification_window_days` in config for consistency and tuning.

3. **Tail-risk proxy**
   - **Where:** `persona_classification.rs`: `avg_win_pnl` uses `features.total_pnl.max(1.0) / win_count` and `max_loss_proxy = max_drawdown_pct * avg_position_size / 100`.
   - **Why:** Plan notes “approximate from features”; acceptable for current phase. No change required now; consider a short comment that this is a proxy until per-trade loss is available.

---

### Recommendations

- Add the Important fix (logging or metric when classification/feature computation fails) before merge.
- Clarify or adjust `wallet_personas` semantics (history vs single row per wallet) and document or change schema accordingly.
- After commit, run `make test` and (if available) a quick run of the evaluator to confirm the new job runs without errors.

---

### Assessment

**Ready to merge?** **Yes** (Important items addressed in follow-up edits).

**Reasoning:** The orchestrator and job match the plan, tests are in place, and config is wired correctly. Follow-up changes: (1) added `tracing::warn!` when `compute_wallet_features` or `classify_wallet` fails in the job; (2) documented `wallet_personas` semantics (multiple rows per wallet; use latest by `classified_at` for current).
