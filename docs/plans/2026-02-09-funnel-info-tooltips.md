# Funnel Info Tooltips Implementation Plan

**Goal:** Add a small info icon (ⓘ) next to each funnel stage label that shows code-derived criteria and what the stage represents when hovered (or focused).

**Architecture:** Criteria text is derived from the same logic as the funnel counts (queries.rs, pipeline_jobs, market_scoring, wallet_discovery, wallet_scoring, paper_trading). We add an `info: String` to `FunnelStage`, populate it in `FunnelCounts::to_stages()` with static, code-accurate copy, and render a small "i" with `title` (native tooltip) in the funnel bar template.

**Tech Stack:** Rust (crates/web models + Tera template), no new deps.

---

## Code-derived criteria (source references)

| Stage   | Count source (queries.rs) | Criteria / what’s happening (from code) |
|---------|---------------------------|----------------------------------------|
| Markets | `COUNT(*) FROM markets` | Fetched from Gamma API; filter: closed=false, min liquidity, min 24h volume, end_date ≥ tomorrow. All passing markets are upserted (pipeline_jobs.rs fetch loop + markets upsert). |
| Scored  | `market_scores WHERE score_date = max` | MScore per market, EScore = max(MScore) per event. Top N events (config top_n_events, default 50) written to market_scores (pipeline_jobs run_market_scoring_once, market_scoring rank_events). |
| Wallets | `COUNT(*) FROM wallets` | From today’s scored markets: Data API holders (up to holders_per_market) + market trades (200). Included if in top holders or have ≥ min_total_trades in that market; capped at max_wallets_per_market per market (wallet_discovery discover_wallets_for_market, pipeline_jobs run_wallet_discovery_once). |
| Tracked | `COUNT(*) FROM wallets WHERE is_active = 1` | Wallets on the watchlist; ingestion (trades, activity, positions, holders) runs only for is_active=1. New discoveries inserted with is_active=1 (pipeline_jobs, ingestion_jobs). |
| Paper   | `COUNT(*) FROM paper_trades` | Each row = one mirrored paper trade. Paper tick mirrors trades_raw for tracked wallets; each real trade may create one paper trade if risk rules pass: position size, portfolio stop, per-market/per-wallet exposure, daily cap (paper_trading mirror_trade_to_paper, pipeline_jobs run_paper_tick_once). |
| Ranked  | `COUNT(DISTINCT proxy_wallet) FROM wallet_scores_daily WHERE score_date = date('now')` | WScore computed for active wallets from paper PnL over configured windows; one row per (wallet, date, window_days). This count = distinct wallets with a score row today (pipeline_jobs run_wallet_scoring_once, wallet_scoring compute_wscore). |

---

## Task 1: Add `info` to FunnelStage and populate in to_stages()

**Files:**
- Modify: `crates/web/src/models.rs`

**Steps:**

1. Add field to struct:
   - In `FunnelStage`, add `pub info: String`.
2. In `FunnelCounts::to_stages()`, for each stage label, set a short `info` string that matches the table above (one or two sentences). Keep text static (no config values in the string to avoid drift); refer to “config” where needed (e.g. “top N markets (config)”).
3. Update existing tests that build `FunnelStage` or assert on stages: ensure they still pass (either set `info` in tests or adjust assertions).

**Example info strings (concise, code-derived):**

- Markets: "Markets fetched from Gamma API (open, min liquidity/volume, end date ≥ tomorrow)."
- Scored: "Events scored today with EScore (max MScore per event); only top N events (config: top_n_events) are stored."
- Wallets: "Wallets discovered from today’s scored markets: top holders + traders with enough trades in that market; capped per market."
- Tracked: "Wallets with is_active=1; trades, activity, positions and holders are ingested only for these."
- Paper: "Total paper trades. Each mirrored from a real trade for a tracked wallet when risk rules allow."
- Ranked: "Wallets with a WScore row today (score from paper PnL over configured windows)."

---

## Task 2: Render info icon and tooltip in funnel template

**Files:**
- Modify: `crates/web/templates/partials/funnel_bar.html`

**Steps:**

1. Next to each stage label, add a small info indicator:
   - Use a single character or minimal markup, e.g. `<span class="cursor-help text-gray-500 hover:text-gray-300 ml-0.5" title="{{ stage.info }}">ⓘ</span>` so the browser shows the tooltip on hover.
2. Keep the indicator inline with the label (same line), small size (text-xs or similar), and ensure it doesn’t break layout on narrow screens (flex-shrink-0 already on the stage block).

---

## Task 3: Tests and verification

**Files:**
- Modify: `crates/web/src/models.rs` (tests that build or assert on stages)

**Steps:**

1. In `models.rs` tests, ensure any construction of `FunnelStage` includes `info` (e.g. non-empty string).
2. Optionally add a short test that `to_stages()` returns six stages and each has non-empty `info`.
3. Run `make test` (or `cargo test -p web`) and fix any failures.
4. Manually: start web server, open dashboard, hover each ⓘ and confirm tooltip shows the right stage explanation.

---

## Task 4: Branch, commit, PR

**Steps:**

1. Create worktree: `make worktree NAME=funnel-info-tooltips` (or `git worktree add .worktrees/funnel-info-tooltips -b feature/funnel-info-tooltips`).
2. Implement Tasks 1–3 in that worktree.
3. Commit with message: "Add funnel stage info (i) tooltips with code-derived criteria".
4. Push branch, open PR (target main). In PR description, link to this plan and note that criteria text is derived from queries.rs and pipeline/scoring code.

---

## Out of scope

- Config-driven tooltip text (e.g. injecting top_n_events value): keep tooltips static to avoid config/code drift; “config” is enough.
- Fancy tooltip UI (e.g. custom popover): native `title` is sufficient for this feature.
