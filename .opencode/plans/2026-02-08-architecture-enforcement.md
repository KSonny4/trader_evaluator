# Architecture Enforcement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Split all overlength files to under 500 lines, add comprehensive clippy lints, fix all violations, and enforce architectural quality going forward.

**Architecture:** Convert single-file modules into directory modules (`mod.rs` + submodules) for `jobs` and `queries`. Split `web/main.rs` into separate files for auth, templates, and handlers. Add ~20 zero-violation clippy lints as regression guards, fix ~40 auto-fixable violations, and tighten the Makefile enforcement.

**Tech Stack:** Rust, Clippy, Cargo workspace lints, Makefile

**Branch:** `feature/architecture-enforcement`

---

## Phase 1: Setup & Lints (Tasks 1-3)

### Task 1: Create worktree and commit current lint config

**Files:**
- Already modified: `Cargo.toml`, `clippy.toml`, `Makefile`, `crates/*/Cargo.toml`

**Step 1: Create worktree**

```bash
make worktree NAME=architecture-enforcement
```

**Step 2: Copy over the uncommitted changes to the worktree**

The current uncommitted changes on `main` (clippy.toml, Cargo.toml lint config, Makefile check-file-length) need to be applied in the worktree. Stash on main, check out in worktree, apply.

```bash
git stash
cd .worktrees/architecture-enforcement
git stash pop
```

**Step 3: Verify everything still passes**

```bash
cargo test --all
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make check-file-length
```

**Step 4: Commit**

```bash
git add clippy.toml Cargo.toml crates/common/Cargo.toml crates/evaluator/Cargo.toml crates/web/Cargo.toml Makefile
git commit -m "feat: add clippy.toml, workspace lints, and 500-line file limit check"
```

---

### Task 2: Add zero-violation clippy lints (regression guards)

These lints have zero current violations — enabling them prevents future regressions at no cost.

**Files:**
- Modify: `Cargo.toml` (`[workspace.lints.clippy]` section)

**Step 1: Add the following lints to `[workspace.lints.clippy]`:**

```toml
# Zero-violation regression guards
dbg_macro = "deny"
todo = "deny"
unimplemented = "deny"
map_err_ignore = "warn"
rest_pat_in_fully_bound_structs = "warn"
fn_to_numeric_cast_any = "deny"
rc_buffer = "warn"
rc_mutex = "deny"
undocumented_unsafe_blocks = "deny"
float_cmp_const = "deny"
lossy_float_literal = "warn"
empty_drop = "warn"
empty_structs_with_brackets = "warn"
redundant_type_annotations = "warn"
large_stack_arrays = "warn"
large_futures = "warn"
significant_drop_tightening = "warn"
significant_drop_in_scrutinee = "warn"
derive_partial_eq_without_eq = "warn"
mutex_atomic = "warn"
```

**Step 2: Run clippy to verify zero violations**

```bash
cargo clippy --all-targets -- -D warnings
```

Expected: clean pass. If any lint fires (e.g. `derive_partial_eq_without_eq` had 1 borderline violation), fix it immediately or remove that lint.

**Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "feat: add 20 zero-violation clippy lints as regression guards"
```

---

### Task 3: Fix auto-fixable clippy violations and enable those lints

**Files:**
- Modify: `Cargo.toml` (add lints)
- Auto-fixed: various `.rs` files

**Step 1: Run clippy autofix for each lint category**

```bash
cargo clippy --fix --all-targets --allow-dirty -- \
  -W clippy::uninlined_format_args \
  -W clippy::needless_raw_strings \
  -W clippy::semicolon_if_nothing_returned \
  -W clippy::cast_lossless \
  -W clippy::redundant_closure_for_method_calls \
  -W clippy::map_unwrap_or \
  -W clippy::manual_let_else
```

**Step 2: Manually review the changes**

```bash
git diff
```

Verify the auto-fixes are sensible. Revert any that change semantics.

**Step 3: Add these lints to `[workspace.lints.clippy]` in Cargo.toml**

```toml
# Auto-fixed, now enforced
uninlined_format_args = "warn"
needless_raw_strings = "warn"
semicolon_if_nothing_returned = "warn"
cast_lossless = "warn"
redundant_closure_for_method_calls = "warn"
map_unwrap_or = "warn"
manual_let_else = "warn"
```

**Step 4: Verify**

```bash
cargo clippy --all-targets -- -D warnings
cargo test --all
```

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: fix auto-fixable clippy violations and enable 7 pedantic lints"
```

---

## Phase 2: Split `jobs.rs` (Tasks 4-8)

The largest file (1500 lines). Split into a `jobs/` directory module with 5 submodules.

### Target structure:

```
evaluator/src/jobs/
  mod.rs              (~20 lines)  — re-exports
  fetcher_traits.rs   (~53 lines)  — 5 trait definitions
  fetcher_impls.rs    (~182 lines) — 6 impl blocks for PolymarketClient
  ingestion_jobs.rs   (~303 lines) — 4 ingestion job runners
  pipeline_jobs.rs    (~440 lines) — 4 pipeline job runners + helper
  maintenance.rs      (~30 lines)  — WAL checkpoint
```

Tests move with the functions they test. Shared test helpers go in `mod.rs` or a `test_helpers` submodule.

### Task 4: Create `jobs/mod.rs` and `jobs/fetcher_traits.rs`

**Files:**
- Delete: `crates/evaluator/src/jobs.rs` (replace with directory)
- Create: `crates/evaluator/src/jobs/mod.rs`
- Create: `crates/evaluator/src/jobs/fetcher_traits.rs`

**Step 1: Create directory and move `jobs.rs`**

```bash
mkdir -p crates/evaluator/src/jobs
mv crates/evaluator/src/jobs.rs crates/evaluator/src/jobs/_original.rs
```

**Step 2: Create `fetcher_traits.rs`**

Extract lines 711-763 from the original file — the 5 trait definitions:
- `GammaMarketsPager`
- `HoldersFetcher`
- `MarketTradesFetcher`
- `ActivityPager`
- `PositionsPager`

Also include `TradesPager` from `ingestion.rs` if it makes sense to co-locate all API pager traits.

Add necessary imports at the top.

**Step 3: Create `mod.rs` with re-exports**

```rust
mod fetcher_traits;
mod fetcher_impls;
mod ingestion_jobs;
mod pipeline_jobs;
mod maintenance;

pub use fetcher_traits::*;
pub use ingestion_jobs::*;
pub use pipeline_jobs::*;
pub use maintenance::*;
```

**Step 4: Verify it compiles** (it won't yet — other modules don't exist)

Just ensure `fetcher_traits.rs` has correct syntax:

```bash
cargo check -p evaluator 2>&1 | head -20
```

Expected: errors about missing submodules (not syntax errors in `fetcher_traits.rs`).

---

### Task 5: Create `jobs/fetcher_impls.rs`

**Files:**
- Create: `crates/evaluator/src/jobs/fetcher_impls.rs`

**Step 1: Extract lines 14-195** from the original — all 6 `impl ... for PolymarketClient` blocks.

Add necessary imports:

```rust
use common::polymarket::{GammaFilter, PolymarketClient};
use common::types::{ApiActivity, ApiHolderResponse, ApiPosition, ApiTrade, GammaMarket};
use std::time::Instant;
use super::fetcher_traits::*;
use crate::ingestion::TradesPager;  // if TradesPager stays in ingestion.rs
```

**Step 2: Verify compilation**

```bash
cargo check -p evaluator 2>&1 | head -20
```

---

### Task 6: Create `jobs/ingestion_jobs.rs`

**Files:**
- Create: `crates/evaluator/src/jobs/ingestion_jobs.rs`

**Step 1: Extract these functions from the original:**
- `run_trades_ingestion_once` (lines 197-239)
- `run_activity_ingestion_once` (lines 241-328)
- `run_positions_snapshot_once` (lines 330-419)
- `run_holders_snapshot_once` (lines 421-502)

**Step 2: Add imports**

```rust
use anyhow::Result;
use common::config::Config;
use common::db::AsyncDb;
use common::types::{ApiActivity, ApiPosition, ApiTrade};
use super::fetcher_traits::*;
use crate::ingestion::TradesPager;
// ... other necessary imports
```

**Step 3: Move related tests** from the test module:
- `test_run_trades_ingestion_inserts_rows` and its fake/helper structs

**Step 4: Verify compilation**

```bash
cargo check -p evaluator
```

---

### Task 7: Create `jobs/pipeline_jobs.rs`

**Files:**
- Create: `crates/evaluator/src/jobs/pipeline_jobs.rs`

**Step 1: Extract these functions:**
- `run_market_scoring_once` (lines 798-992)
- `run_wallet_discovery_once` (lines 994-1101)
- `run_paper_tick_once` (lines 504-590)
- `run_wallet_scoring_once` (lines 592-709)
- `compute_days_to_expiry` helper (lines 1103-1115)

**Step 2: Add imports**

```rust
use anyhow::Result;
use common::config::Config;
use common::db::AsyncDb;
use crate::market_scoring::{rank_markets, MarketCandidate};
use crate::paper_trading::{mirror_trade_to_paper, Side};
use crate::wallet_discovery::{discover_wallets_for_market, HolderWallet, TradeWallet};
use crate::wallet_scoring::{compute_wscore, WScoreWeights, WalletScoreInput};
use super::fetcher_traits::*;
// ... other necessary imports
```

**Step 3: Move related tests:**
- `test_run_market_scoring_persists_ranked_rows` + `FakeGammaPager`
- `test_run_wallet_discovery_inserts_wallets` + `FakeHoldersFetcher` + `FakeMarketTradesFetcher`
- `test_run_paper_tick_creates_paper_trades`
- `test_run_wallet_scoring_inserts_wallet_scores`

**Step 4: Verify compilation and tests**

```bash
cargo test -p evaluator
```

---

### Task 8: Create `jobs/maintenance.rs` and delete original

**Files:**
- Create: `crates/evaluator/src/jobs/maintenance.rs`
- Delete: `crates/evaluator/src/jobs/_original.rs`

**Step 1: Extract `run_wal_checkpoint_once`** (lines 770-796)

**Step 2: Delete `_original.rs`**

**Step 3: Verify everything passes**

```bash
cargo test --all
cargo clippy --all-targets -- -D warnings
make check-file-length
```

Expected: `jobs.rs` no longer in the allowlist warnings. All submodules under 500 lines.

**Step 4: Remove `crates/evaluator/src/jobs.rs` from `OVERLENGTH_ALLOWLIST` in Makefile**

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor: split jobs.rs (1500 lines) into 5 submodules under jobs/"
```

---

## Phase 3: Split `web/main.rs` (Tasks 9-12)

Split the 741-line file into focused modules: auth, templates, handlers.

### Target structure:

```
web/src/
  main.rs        (~80 lines)  — AppState, router wiring, entry point
  auth.rs        (~120 lines) — middleware + auth tests
  templates.rs   (~50 lines)  — Askama template structs
  handlers.rs    (~500 lines) — handler fns + handler tests
```

### Task 9: Extract `templates.rs`

**Files:**
- Create: `crates/web/src/templates.rs`
- Modify: `crates/web/src/main.rs`

**Step 1: Move all `#[derive(Template)]` structs** (lines 84-131 of original) to `templates.rs`

Make them `pub` and add necessary imports (`askama::Template`, model types).

**Step 2: In `main.rs`, add `mod templates;` and `use templates::*;`**

**Step 3: Verify**

```bash
cargo check -p web
```

**Step 4: Commit**

```bash
git add -A
git commit -m "refactor: extract web template structs to templates.rs"
```

---

### Task 10: Extract `auth.rs`

**Files:**
- Create: `crates/web/src/auth.rs`
- Modify: `crates/web/src/main.rs`

**Step 1: Move `basic_auth_middleware`** (lines 41-80) to `auth.rs`

Add necessary imports for axum middleware types, base64.

**Step 2: Move auth-related tests** to `auth.rs` inline test module:
- `test_auth_returns_401_without_credentials`
- `test_auth_returns_401_with_wrong_password`
- `test_auth_returns_200_with_correct_password`
- `test_auth_disabled_when_no_password`
- `test_auth_partials_also_protected`
- `test_auth_www_authenticate_header_present`
- `create_test_app_with_auth` helper
- `basic_auth_header` helper

**Step 3: In `main.rs`, add `pub mod auth;` and update imports**

**Step 4: Verify**

```bash
cargo test -p web
```

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor: extract auth middleware to auth.rs"
```

---

### Task 11: Extract `handlers.rs`

**Files:**
- Create: `crates/web/src/handlers.rs`
- Modify: `crates/web/src/main.rs`

**Step 1: Move all handler functions** (index, status_partial, funnel_partial, markets_partial, wallets_partial, tracking_partial, paper_partial, rankings_partial) to `handlers.rs`

**Step 2: Move handler tests** (everything except auth tests and the `create_test_app` shared helper)

**Step 3: The shared `create_test_app` helper** — if both `auth.rs` and `handlers.rs` tests need it, put it in a `#[cfg(test)]` block in `main.rs` and make it `pub(crate)`, or duplicate it.

**Step 4: Verify**

```bash
cargo test -p web
```

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor: extract web handlers to handlers.rs"
```

---

### Task 12: Verify web/main.rs is under 500 lines

**Step 1: Check**

```bash
wc -l crates/web/src/main.rs
```

Expected: ~80 lines (AppState, open_readonly, create_router, create_router_with_state, main).

**Step 2: Remove `crates/web/src/main.rs` from `OVERLENGTH_ALLOWLIST` in Makefile**

**Step 3: Full verification**

```bash
cargo test --all
cargo clippy --all-targets -- -D warnings
make check-file-length
```

**Step 4: Commit**

```bash
git add Makefile
git commit -m "chore: remove web/main.rs from overlength allowlist"
```

---

## Phase 4: Split `web/queries.rs` (Tasks 13-14)

Split the 725-line file into a `queries/` directory module.

### Target structure:

```
web/src/queries/
  mod.rs         (~15 lines)  — re-exports
  status.rs      (~220 lines) — system_status, funnel_counts, age helper + tests
  markets.rs     (~50 lines)  — top_markets_today + tests
  wallets.rs     (~100 lines) — wallet_overview, recent_wallets + tests
  tracking.rs    (~80 lines)  — tracking_health, stale_wallets + tests
  paper.rs       (~120 lines) — paper_summary, recent_paper_trades + tests
  rankings.rs    (~95 lines)  — top_rankings + tests
```

### Task 13: Create `queries/` directory and split

**Files:**
- Delete: `crates/web/src/queries.rs` (replace with directory)
- Create: `crates/web/src/queries/mod.rs`
- Create: `crates/web/src/queries/status.rs`
- Create: `crates/web/src/queries/markets.rs`
- Create: `crates/web/src/queries/wallets.rs`
- Create: `crates/web/src/queries/tracking.rs`
- Create: `crates/web/src/queries/paper.rs`
- Create: `crates/web/src/queries/rankings.rs`

**Step 1: Create directory**

```bash
mkdir -p crates/web/src/queries
mv crates/web/src/queries.rs crates/web/src/queries/_original.rs
```

**Step 2: Create each submodule** with the functions listed in the target structure above. Each function moves with its tests. The shared `test_db()` helper goes in `mod.rs` as a `#[cfg(test)]` item.

**Step 3: Create `mod.rs` with re-exports**

```rust
mod status;
mod markets;
mod wallets;
mod tracking;
mod paper;
mod rankings;

pub use status::*;
pub use markets::*;
pub use wallets::*;
pub use tracking::*;
pub use paper::*;
pub use rankings::*;

#[cfg(test)]
pub(crate) fn test_db() -> rusqlite::Connection {
    // shared test helper
}
```

**Step 4: Delete `_original.rs`**

**Step 5: Remove `crates/web/src/queries.rs` from `OVERLENGTH_ALLOWLIST` in Makefile**

**Step 6: Verify**

```bash
cargo test -p web
make check-file-length
```

**Step 7: Commit**

```bash
git add -A
git commit -m "refactor: split queries.rs (725 lines) into 6 submodules under queries/"
```

---

## Phase 5: Clean up remaining files (Tasks 14-16)

### Task 14: Simplify `ingestion.rs` with `Default` derive

**Files:**
- Modify: `crates/common/src/types.rs` — derive `Default` for `ApiTrade`
- Modify: `crates/evaluator/src/ingestion.rs` — simplify test `ApiTrade` construction

**Step 1: Add `#[derive(Default)]` to `ApiTrade`** in `types.rs`

**Step 2: Simplify test `ApiTrade` construction** using `..Default::default()` syntax

**Step 3: Check if file is now under 500 lines**

```bash
wc -l crates/evaluator/src/ingestion.rs
```

**Step 4: Commit**

```bash
git add -A
git commit -m "refactor: derive Default for ApiTrade, simplify test construction"
```

---

### Task 15: Update allowlist for legitimate exceptions

**Files:**
- Modify: `Makefile` (update `OVERLENGTH_ALLOWLIST`)

**Step 1: Update allowlist** — only keep files that legitimately need exceptions:

```makefile
OVERLENGTH_ALLOWLIST := \
	crates/evaluator/src/persona_classification.rs \
	crates/common/src/db.rs
```

(Only add `ingestion.rs` if it's still over 500 after Task 14.)

**Step 2: Verify**

```bash
make check-file-length
```

**Step 3: Commit**

```bash
git add Makefile
git commit -m "chore: update overlength allowlist — only legitimate exceptions remain"
```

---

### Task 16: Fix remaining manual clippy violations

**Step 1: Fix `cognitive_complexity` (1 violation)**

The one function exceeding cognitive complexity 25 (31/25) needs refactoring. Extract sub-functions.

**Step 2: Fix `clone_on_ref_ptr` (12 violations)**

Replace `.clone()` on `Arc` with `Arc::clone(&x)`.

**Step 3: Fix `items_after_statements` (4 violations)**

Move struct/impl definitions before `let` statements.

**Step 4: Enable these lints**

Add to `[workspace.lints.clippy]`:

```toml
cognitive_complexity = "warn"
clone_on_ref_ptr = "warn"
items_after_statements = "warn"
```

**Step 5: Verify**

```bash
cargo clippy --all-targets -- -D warnings
cargo test --all
```

**Step 6: Commit**

```bash
git add -A
git commit -m "fix: resolve cognitive_complexity, clone_on_ref_ptr, items_after_statements violations"
```

---

## Phase 6: Final verification and PR (Task 17)

### Task 17: Final verification and PR

**Step 1: Full test suite**

```bash
cargo test --all
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make check-file-length
```

**Step 2: Check all files are under 500 lines (or allowlisted)**

```bash
find crates/ -name '*.rs' -not -path '*/target/*' | xargs wc -l | sort -rn | head -20
```

**Step 3: Push and create PR**

```bash
git push -u origin feature/architecture-enforcement
gh pr create --title "refactor: architecture enforcement — split overlength files + add clippy lints" --body "$(cat <<'EOF'
## Summary

- Split `jobs.rs` (1500 lines) into 5 submodules under `jobs/`
- Split `web/main.rs` (741 lines) into `auth.rs`, `templates.rs`, `handlers.rs`
- Split `web/queries.rs` (725 lines) into 6 submodules under `queries/`
- Added 20+ zero-violation clippy lints as regression guards
- Fixed ~50 auto-fixable clippy violations (pedantic lints)
- Added 500-line file limit enforcement to `make test`
- Overlength allowlist reduced to 2 legitimate exceptions

## Architecture enforcement added

1. **clippy.toml** — function length limit (250, ratcheting to 100)
2. **Cargo.toml workspace lints** — ~30 clippy lints enforced
3. **Makefile check-file-length** — 500 line limit, runs in CI

## Code review evidence

- [ ] `cargo test --all` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] `make check-file-length` passes
- [ ] All new files under 500 lines
EOF
)"
```

---

## Summary table

| Task | What | Key files |
|------|------|-----------|
| 1 | Create worktree, commit existing lint config | `clippy.toml`, `Cargo.toml`, `Makefile` |
| 2 | Add 20 zero-violation clippy lints | `Cargo.toml` |
| 3 | Fix auto-fixable violations, enable 7 lints | Various `.rs` files |
| 4-8 | Split `jobs.rs` into `jobs/` directory | `evaluator/src/jobs/*.rs` |
| 9-12 | Split `web/main.rs` into 3 files | `web/src/{auth,templates,handlers}.rs` |
| 13 | Split `web/queries.rs` into `queries/` directory | `web/src/queries/*.rs` |
| 14 | Simplify `ingestion.rs` with `Default` derive | `common/src/types.rs`, `evaluator/src/ingestion.rs` |
| 15 | Update allowlist for legitimate exceptions | `Makefile` |
| 16 | Fix remaining manual clippy violations | Various `.rs` files |
| 17 | Final verification and PR | — |

## Files NOT being split (with justification)

| File | Lines | Why keep as-is |
|------|-------|---------------|
| `persona_classification.rs` | 974 | Only 340 prod lines; 635 lines are excellent tests. Highly cohesive single responsibility. |
| `db.rs` | 506 | Only 47 lines of Rust logic; 255 lines are SQL DDL schema. |
