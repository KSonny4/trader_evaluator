# Persona Taxonomy Enrichment (A–G Styles) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extend our wallet persona taxonomy to cover the A–G “copyability styles” (speed/news, market-maker, bonder, domain specialist, whale, gambler, bot swarm) in a way that is labelable from our data and actionable for copy-trading risk.

**Architecture:** Keep the 3 **followable personas** unchanged (Informed Specialist, Consistent Generalist, Patient Accumulator), but enrich classification with (1) new **explicit exclusion reasons** for unfollowable A–G styles and (2) optional **persona traits** (topic lane, bonder-ness, whale-ness) stored in DB for UI + downstream “copy only their lane” behavior.

**Tech Stack:** Rust, SQLite (rusqlite), TOML config (`config/default.toml`), unit tests via `cargo test --all` (or `make test`).

---

## How This Connects To The Master Plan

This plan is an add-on to `docs/plans/MASTER_STRATEGY_IMPLEMENTATION_PLAN.md`:

- Extends Phase 1 (already “complete” in master plan) with additional persona/exclusion styles derived from your A–G list.
- Benefits from Phase 2 Task 12 (Persona Classification Orchestrator) being wired, so the new labels/traits get computed continuously without manual runs.

Concrete touchpoints:
- Master Task 2 (Schema): we add `wallet_persona_traits` (new table).
- Master Task 3 (Wallet Feature Computation): we add burstiness/two-sidedness/extreme price ratio/category concentration features.
- Master Stage 2 exclusions: we add new exclusion reasons that are “execution-speed/infrastructure dependent” (A/B/G) and “PnL spike” risky (F).
- Strategy Bible “Who We Follow”: we surface `TOPIC_LANE=<category>` as a trait and allow an optional “mirror in-lane only” mode for strong lane-specialists.

## Persona Model (Target)

We will treat your A–G as either:
- **Exclusion personas** (hard “do not follow” by default), or
- **Traits** that can refine execution (e.g. copy only sports lane), without expanding the followable set.

### Mapping A–G to our system

Followable personas (existing, unchanged):
- **Informed Specialist** (often overlaps with D: Domain Specialist)
- **Consistent Generalist**
- **Patient Accumulator** (often overlaps with C: Bonder/High-probability grinder)

Exclusions (existing + new):
- Existing: Execution Master, Tail Risk Seller, Noise Trader, Sniper/Insider, Sybil Cluster (future)
- New exclusions to add:
  - **NEWS_SNIPER** (A: Speed / News Sniper) – not about “young wallet”, but about “edge is ultra-short + bursty”
  - **LIQUIDITY_PROVIDER** (B: Liquidity Provider / Market Maker) – execution-dependent / two-sided / mid-fills proxy
  - **JACKPOT_GAMBLER** (F: Gambler / Jackpot wallet) – PnL concentrated in a few trades + low win-rate + high variance
  - **BOT_SWARM_MICRO** (G: Bot swarm / Micro-trader) – extreme frequency + tiny/uniform sizing

Traits (stored, not hard exclusions):
- **TOPIC_LANE=...** (D: Domain Specialist) – used to rank per topic and optionally copy only that lane
- **BONDER=1** (C: Bonder) – high-prob entries (price near 0/1), longer holds, stable PnL
- **WHALE=1** (E: Whale/Institution) – large sizing and/or slow accumulation; used to tighten slippage/impact checks

Copyability (future-facing, but we’ll store enough signals now):
- `copyability_delay_bucket`: {`3s`, `30s`, `120s`} as a computed suggestion (not a gate yet)

---

# Tasks

### Task 1: Update Strategy Docs With A–G (Source Of Truth)

**Files:**
- Modify: `docs/STRATEGY_BIBLE.md`
- Modify: `docs/REFERENCE.md`

**Step 1: Write the failing “doc test” (lightweight)**

Add a Rust unit test that asserts the new exclusion reason strings exist (this is our “contract test” that docs + code don’t drift too far):

Files:
- Modify: `crates/evaluator/src/persona_classification.rs`

Add:
```rust
#[test]
fn test_exclusion_reason_strings_include_ag_personas() {
    use super::ExclusionReason;
    assert_eq!(ExclusionReason::NewsSniper { burstiness: 1.0, max_burstiness: 0.9 }.reason_str(), "NEWS_SNIPER");
    assert_eq!(ExclusionReason::LiquidityProvider { side_balance: 0.5, mid_fill_ratio: 0.8 }.reason_str(), "LIQUIDITY_PROVIDER");
    assert_eq!(ExclusionReason::JackpotGambler { pnl_top1_share: 0.9, win_rate: 0.3 }.reason_str(), "JACKPOT_GAMBLER");
    assert_eq!(ExclusionReason::BotSwarmMicro { trades_per_day: 500.0, avg_size_usdc: 1.0 }.reason_str(), "BOT_SWARM_MICRO");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p evaluator test_exclusion_reason_strings_include_ag_personas -v`
Expected: FAIL because the enum variants don’t exist yet.

**Step 3: Update docs (no code changes yet besides test)**

In `docs/STRATEGY_BIBLE.md`:
- Add a new subsection under “Who We Exclude” listing A/B/F/G explicitly and clarifying that “Sniper/Insider” (current) is not the full “news sniper” concept.
- Add a “Traits” subsection to define Topic lane, Bonder, Whale and how they affect follow/execution without adding new followable personas.

In `docs/REFERENCE.md`:
- Update the “Wallet persona taxonomy” table to include the new exclusions + traits (as separate concepts).

**Step 4: Re-run test**

Run: `cargo test -p evaluator test_exclusion_reason_strings_include_ag_personas -v`
Expected: still FAIL (docs updates don’t fix the missing enum).

**Step 5: Commit**

```bash
git add docs/STRATEGY_BIBLE.md docs/REFERENCE.md crates/evaluator/src/persona_classification.rs
git commit -m "docs: enrich persona taxonomy with A–G styles"
```

---

### Task 2: Add DB Support For Persona Traits (Lane/Bonder/Whale)

**Files:**
- Modify: `crates/common/src/db.rs`
- Create: `crates/common/src/db_migrations/` (only if the repo already uses migration files; otherwise keep inline schema)

**Step 1: Write the failing test**

Files:
- Modify: `crates/common/src/db.rs`

Add a migration/schema test that the new table exists:
```rust
#[test]
fn test_wallet_persona_traits_table_exists() {
    let db = Database::open(":memory:").unwrap();
    db.run_migrations().unwrap();
    let tables: Vec<String> = db.conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(tables.iter().any(|t| t == "wallet_persona_traits"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p common test_wallet_persona_traits_table_exists -v`
Expected: FAIL because the table isn’t created.

**Step 3: Implement minimal schema change**

Add:
```sql
CREATE TABLE IF NOT EXISTS wallet_persona_traits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    proxy_wallet TEXT NOT NULL,
    trait_key TEXT NOT NULL,          -- e.g. TOPIC_LANE, BONDER, WHALE
    trait_value TEXT NOT NULL,        -- e.g. "sports", "1"
    computed_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(proxy_wallet, trait_key)
);
CREATE INDEX IF NOT EXISTS idx_wallet_persona_traits_wallet ON wallet_persona_traits(proxy_wallet);
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p common test_wallet_persona_traits_table_exists -v`
Expected: PASS.

**Step 5: Commit**

```bash
git add crates/common/src/db.rs
git commit -m "db: add wallet_persona_traits table"
```

---

### Task 3: Add WalletFeatures Signals Needed For A–G

**Files:**
- Modify: `crates/evaluator/src/wallet_features.rs`
- Modify: `crates/common/src/db.rs` (wallet_features_daily columns)

**New features to compute (windowed):**
- `trades_per_day` (frequency proxy for G)
- `avg_trade_size_usdc` (already `avg_position_size`; rename later if needed)
- `size_cv` (coefficient of variation) for “uniform sizing” (G)
- `buy_sell_balance` in `[0,1]` (B proxy)
- `mid_fill_ratio` = pct trades with `abs(price - 0.5) <= mid_band` (B proxy; configurable)
- `extreme_price_ratio` = pct trades with `price >= 0.9` or `price <= 0.1` (C proxy)
- `burstiness_top_1h_ratio` = max(trades in any rolling 1h) / trade_count (A proxy)
- `top_category_ratio` + `top_category` (D trait, using join `trades_raw` -> `markets.category`)

**Step 1: Write failing tests (feature computation)**

Add tests in `crates/evaluator/src/wallet_features.rs` with synthetic `trades_raw` and `markets` rows:
- `test_trades_per_day_computed`
- `test_buy_sell_balance_computed`
- `test_extreme_price_ratio_computed`
- `test_top_category_ratio_computed`

Example test skeleton:
```rust
#[test]
fn test_extreme_price_ratio_computed() {
    let now = 1_700_000_000i64;
    let db = setup_db_with_trades(&[
        ("0xabc", "m1", "BUY", 10.0, 0.99, now - 10),
        ("0xabc", "m1", "SELL", 10.0, 0.98, now - 9),
        ("0xabc", "m2", "BUY", 10.0, 0.50, now - 8),
        ("0xabc", "m2", "SELL", 10.0, 0.52, now - 7),
    ]);
    let f = compute_wallet_features(&db.conn, "0xabc", 30, now).unwrap();
    assert!(f.extreme_price_ratio > 0.4);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p evaluator wallet_features::tests::test_extreme_price_ratio_computed -v`
Expected: FAIL (fields don’t exist yet).

**Step 3: Implement minimal feature computation**

Add the new fields to `WalletFeatures`, and compute them using SQL queries (keep each query simple; correctness over micro-optimizations).

**Step 4: Update wallet_features_daily schema + save_wallet_features**

Add new columns and update the `INSERT` statement accordingly.

**Step 5: Re-run tests**

Run: `cargo test -p evaluator wallet_features::tests::test_extreme_price_ratio_computed -v`
Expected: PASS.

**Step 6: Commit**

```bash
git add crates/evaluator/src/wallet_features.rs crates/common/src/db.rs
git commit -m "feat: compute A–G wallet features (burstiness, two-sidedness, extremes, topic lane)"
```

---

### Task 4: Add New Exclusion Reasons + Detectors (A/B/F/G)

**Files:**
- Modify: `crates/evaluator/src/persona_classification.rs`
- Modify: `crates/common/src/config.rs`
- Modify: `config/default.toml`

**Step 1: Write failing tests for each detector**

Add tests:
- `test_detect_news_sniper_bursty_short_horizon_proxy_excludes`
- `test_detect_liquidity_provider_two_sided_mid_fills_excludes`
- `test_detect_jackpot_gambler_pnl_concentration_excludes`
- `test_detect_bot_swarm_micro_extreme_frequency_excludes`

Example (news sniper proxy):
```rust
#[test]
fn test_detect_news_sniper_bursty_excludes() {
    let reason = detect_news_sniper(0.95, 0.90);
    assert!(matches!(reason, Some(ExclusionReason::NewsSniper{..})));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p evaluator test_detect_news_sniper_bursty_excludes -v`
Expected: FAIL.

**Step 3: Implement minimal exclusions**

Add enum variants + `reason_str()` mappings:
- `NewsSniper { burstiness: f64, max_burstiness: f64 }`
- `LiquidityProvider { side_balance: f64, mid_fill_ratio: f64 }`
- `JackpotGambler { pnl_top1_share: f64, win_rate: f64 }`
- `BotSwarmMicro { trades_per_day: f64, avg_size_usdc: f64 }`

Add simple detector functions:
- `detect_news_sniper(burstiness_top_1h_ratio, threshold)`
- `detect_liquidity_provider(buy_sell_balance, mid_fill_ratio, min_balance, min_mid_ratio)`
- `detect_jackpot_gambler(pnl_top1_share, win_rate, thresholds...)`
- `detect_bot_swarm_micro(trades_per_day, avg_size_usdc, thresholds...)`

**Step 4: Wire into `classify_wallet`**

Insert these checks into the Stage 2 exclusion section (after existing sniper/noise checks, before followable classification).

**Step 5: Add config keys**

In `config/default.toml` `[personas]` add:
```toml
news_sniper_max_burstiness_top_1h_ratio = 0.70
liquidity_provider_min_buy_sell_balance = 0.45
liquidity_provider_min_mid_fill_ratio = 0.60
bot_swarm_min_trades_per_day = 200.0
bot_swarm_max_avg_trade_size_usdc = 5.0
jackpot_min_pnl_top1_share = 0.60
jackpot_max_win_rate = 0.45
```

Update `crates/common/src/config.rs` `Personas` struct and `PersonaConfig` accordingly.

**Step 6: Re-run tests**

Run: `cargo test -p evaluator test_detect_news_sniper_bursty_excludes -v`
Expected: PASS.

**Step 7: Commit**

```bash
git add crates/evaluator/src/persona_classification.rs crates/common/src/config.rs config/default.toml
git commit -m "feat: add A/B/F/G exclusion personas (news sniper, liquidity provider, jackpot, bot swarm)"
```

---

### Task 5: Compute And Store Traits (D/C/E) For UI + “Copy Only Their Lane” Later

**Files:**
- Modify: `crates/evaluator/src/persona_classification.rs` (or new `persona_traits.rs`)

**Step 1: Write failing tests**

Add tests that trait rows are written for:
- Domain specialist: `top_category_ratio >= threshold` => `TOPIC_LANE=<category>`
- Bonder: `extreme_price_ratio >= threshold` => `BONDER=1`
- Whale: `avg_position_size >= threshold` => `WHALE=1`

**Step 2: Run tests to verify they fail**

Run: `cargo test -p evaluator test_record_persona_traits -v`
Expected: FAIL.

**Step 3: Implement minimal trait recording**

Implement:
```rust
fn upsert_trait(conn: &Connection, proxy_wallet: &str, key: &str, value: &str) -> Result<()> { ... }
```
and call it inside `classify_wallet` (for both excluded and followable wallets).

**Step 4: Re-run tests**

Run: `cargo test -p evaluator test_record_persona_traits -v`
Expected: PASS.

**Step 5: Commit**

```bash
git add crates/evaluator/src/persona_classification.rs
git commit -m "feat: record persona traits (topic lane, bonder, whale)"
```

---

### Task 6: Verification (Repo-Wide)

**Files:**
- None

**Step 1: Run full test suite**

Run: `make test`
Expected: PASS (tests, clippy, fmt).

**Step 2: Optional sanity check**

If you have a local `data/evaluator.db`, run:
Run: `make check`
Expected: PASS.

**Step 3: Commit (if any follow-up fixes)**

```bash
git status
```

---

## Notes / Open Decisions To Confirm

1. **News Sniper**: do we want to exclude all ultra-bursty wallets, or only those whose paper-trading PnL collapses when `mirror_delay_secs` is increased?
2. **Liquidity Provider**: until we have maker/taker + orderbook snapshots, our detection is a heuristic. OK to keep it “soft exclusion” (flag) for now?
3. **Topic lane taxonomy**: do we use `markets.category` as-is, or normalize into a smaller set (sports/politics/crypto/weather/other)?
