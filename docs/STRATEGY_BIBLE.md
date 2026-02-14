# Strategy Bible

> **The single source of truth for what this system does, why, and how.**
> Every strategic decision is codified here. Every test validates against this document.
> If the code contradicts this document, the code is wrong.

## 1. Mission & Core Principle

**Mission:** Discover Polymarket wallets with reproducible directional edge. Prove the edge is real and copyable via paper trading. Then copy them with real money.

**Core principle:** We are **edge detectors**, not speed traders. We find people who know things, prove their knowledge translates to profit after realistic delays and costs, then ride along.

**The edge must survive:**
- A 5-120 second execution delay (it's not execution-dependent)
- Realistic slippage (orderbook can absorb our size)
- Fees and spread overhead

**What we are NOT:**
- Not a latency-optimized HFT system
- Not following every wallet — we follow 5-10 proven ones out of 500+ discovered
- Not trading our own signals — we are pure copy-traders

**Scope:**
- **Discover** wallets via top-50 EScore events (the net we cast)
- **Follow** discovered wallets **everywhere** they trade (the strategy)
- **High copy fidelity:** copy ALL their trades, manage risk at portfolio level only
- **Event scoring is for DISCOVERY only.** Copying is wallet-centric, market-agnostic.

**Event vs market:** Polymarket **events** are the primary unit for discovery (e.g. "Will Bitcoin reach $150,000 in February?"). Each event has multiple outcome **markets** (condition_ids). We are **event-focused** for discovery: we rank and select top events. Markets remain important for copying (trades, positions, settlement) — we read and use them throughout the pipeline — but discovery casts the net at the event level.

### Domain hierarchy (canonical terminology)

**Use these terms consistently in code, docs, and UI.** Polymarket uses different names; we map them as follows:

| Our term | Polymarket field | Example | Definition |
|----------|------------------|---------|------------|
| **Domain** | `category` | Sport, Politics, Crypto | Top-level topic (Gamma API `category`) |
| **Sub-domain** | — | Football, US Elections | Finer topic within domain (when distinguishable) |
| **Event** | `eventSlug` | sparta-slavia | Specific competition/match (e.g. Sparta vs Slavia) |
| **Market** | `conditionId` | 0xabc… | Outcome market within event (e.g. "Who will win?") |

**Hierarchy:** Domain → (Sub-domain) → Event → Market

- **Domain** = broad topic (Sports, Politics, Crypto, Culture, etc.). Stored in `markets.category`.
- **Event** = specific occurrence (match, election, resolution deadline). Identified by `event_slug`. When null, we treat the market as a singleton event.
- **Market** = tradable outcome (Yes/No, or multi-way). Identified by `condition_id`. Trades, positions, and copying operate at market level.

**Wallet traits:** `TOPIC_LANE=<domain>` means the wallet has edge in that domain (e.g. `TOPIC_LANE=Sports`). We use `top_domain` and `top_domain_ratio` in wallet features.

---

## 2. The Funnel

```
All Polymarket wallets (~1M+)
    ↓ [Event Selection: top-50 EScore events]
Wallets active on those events (~500+)
    ↓ [Stage 1: Fast automated filters — runs inline during discovery]
Candidate wallets (~50-100)
    ↓ [Stage 2: Deep analysis — runs async as background job]
Classified wallets with persona (~20-50)
    ↓ [Followable personas only]
Paper-traded wallets (~5-15)
    ↓ [Paper trading proof over 7-30 days]
Follow-worthy wallets (~3-5)
    ↓ [Human approval]
Real-money followed wallets (~1-3)
```

Each stage has measurable drop-off rates visible in the UI and Grafana.

### Funnel stages (implementation)

The dashboard and code use **6 implementation funnel stages** that map to the 5 evaluation phases (§9). These are the canonical names; the UI (section headers "Stage 1" … "Stage 5") and phase strings must use the **phase names** below. "Paper Copy Engine" is the implementation name for the component that performs Paper Trading.

| # | Canonical label (short) | Phase | What it measures (one line) |
|---|-------------------------|-------|-----------------------------|
| 1 | Events | 1 Event Discovery | Distinct events (grouped by event_slug) |
| 2 | Scored | 1 Event Discovery | EScore top-n events (aggregate of market MScore) |
| 3 | Wallets | 2 Wallet Discovery | Discovered wallets (watchlist) |
| 4 | Tracked | 3 Long-Term Tracking | Wallets with is_active=1 |
| 5 | Paper | 4 Paper Trading | Paper trades created (mirror engine) |
| 6 | Ranked | 5 Wallet Ranking | Wallets with WScore today |

**Phase names (for "Stage N" headers and phase string):** Event Discovery, Wallet Discovery, Long-Term Tracking, Paper Trading, Wallet Ranking.

**Event focus:** The dashboard and funnel show **events** (not raw markets). Events are grouped by `event_slug`; markets without an event_slug are treated as singleton events. EScore = best MScore among markets in that event. See §Event Discovery & EScore for full definition.

### Event Discovery & EScore

**EScore (Event Score)** is the primary metric for event discovery. It determines which events we use to discover wallets.

| Concept | Definition |
|---------|------------|
| **Event** | A Polymarket event (e.g. "Will Bitcoin reach $150,000 in February?") identified by `event_slug`. When `event_slug` is null, we treat the market as a singleton event. |
| **EScore** | `max(MScore)` over all markets in that event. The best MScore among the event's outcome markets. |
| **Usage** | Rank events by EScore; select top-50 for wallet discovery. Pipeline stores MScore per market; we aggregate to EScore when grouping by event. |

**How EScore is used:**
1. **Discovery:** Select top-50 events by EScore. Wallets active on those events' markets become candidates.
2. **Dashboard:** Funnel "Events" stage = count of distinct events (grouped by `event_slug` or `condition_id`).
3. **UI list:** Show top-50 events ordered by EScore; display the event question (from market `question` field, e.g. "Will Bitcoin reach $150,000 in February?").

**UI display:** When `event_slug` exists we show true events (one row per event, multiple markets grouped). When `event_slug` is null we show markets as singleton events — the row is a market, but we label it "event" for consistency. The title shown is the market's `question` (Gamma API), which is the event-level question; for multi-outcome events, markets in the same event share that question.

---

## 3. Who We Follow (Followable Personas)

Only three personas advance to paper trading. Everything else is excluded.

| Persona | Key Signals | Topic Lane (if detected) | Follow Mode | Trust Level |
|---------|------------|---------------------------|-------------|-------------|
| **Informed Specialist** | active_positions ≤ 5, concentration ≥ 60%, win_rate > 60% | If `TOPIC_LANE` exists: prefer mirroring **in-lane** first; expand out-of-lane only if proven | Mirror with 5-30s delay | PRIMARY target |
| **Consistent Generalist** | unique_markets > 20, win_rate 52-60%, low drawdown, Sharpe > 1 | If `TOPIC_LANE` exists: treat as “generalist with a strong lane” and evaluate per-topic | Mirror | SECONDARY |
| **Patient Accumulator** | avg_hold_time > 48h, large positions (>90th percentile), < 5 trades/week | If `TOPIC_LANE` exists: copy only in-lane unless out-of-lane performance is comparable | Mirror with 24h+ delay | SLOW but reliable |

**Topic lane definition:** A wallet’s “lane” is the dominant **domain** they trade (e.g. Politics, Sports, Crypto). Lane is a **trait** (`TOPIC_LANE=<domain>`), not a persona. We use it to avoid copying a specialist outside their proven domain.

### Classification thresholds (configurable in `default.toml`)

**Informed Specialist:**
- `active_positions <= specialist_max_active_positions` (default: 5) — limits currently open positions
- `concentration_ratio >= specialist_min_concentration` (default: 0.60) — % of volume in top 3 markets
- `win_rate > specialist_min_win_rate` (default: 0.60)

**Why active_positions + concentration_ratio?**
- `unique_markets` fails for old accounts (5 years = many historical markets)
- `active_positions` catches current focus (prevents "dabbler" with scattered trades)
- `concentration_ratio` catches true specialists (prevents "generalist" trading many markets equally)
- Combined: wallet with 3 active positions and 75% in top 3 markets = specialist

**Consistent Generalist:**
- `unique_markets > generalist_min_markets` (default: 20)
- `win_rate` in `[generalist_min_win_rate, generalist_max_win_rate]` (default: 0.52-0.60)
- `max_drawdown_pct < generalist_max_drawdown` (default: 15%)
- `sharpe_ratio > generalist_min_sharpe` (default: 1.0)

**Patient Accumulator:**
- `avg_hold_time_hours > accumulator_min_hold_hours` (default: 48)
- `roi >= accumulator_min_roi` (default: 0.05 = 5% per-trade ROI)
- `trades_per_week < accumulator_max_trades_per_week` (default: 5)

> The ROI gate replaces the 90th-percentile position size check (impractical to compute inline).

### Followable ROI Gate

All followable personas must pass `stage2_min_roi` after persona detection. This is per-trade ROI: `total_pnl / (trade_count * avg_position_size)`.

**Three ROI Tiers:**

| Tier | Gate | Default | Purpose |
|------|------|---------|---------|
| **Tier 1 (Classification)** | `stage2_min_roi` | 3% | Bare minimum to be followable — applies to all personas |
| **Tier 2 (Accumulator-specific)** | `accumulator_min_roi` | 5% | Higher bar for slow traders who must compensate for low frequency |
| **Tier 3 (Follow-worthy)** | `paper_roi_pct > 5% (7d) AND > 10% (30d)` | — | Portfolio-level proof before real-money promotion |

> **Opportunity cost note:** SAP/S&P 500 returns ~7% annually. Our Tier 3 thresholds (5% in 7 days ~ 260% annualized) far exceed passive alternatives, ensuring we only follow wallets with genuine edge.

---

## 4. Who We Exclude (Non-Followable Personas)

### Stage 1: Fast Filters (inline, during discovery)

Fail any single filter → immediately excluded with recorded reason in `wallet_exclusions`.

| Filter | Config Key | Default | Why |
|--------|-----------|---------|-----|
| Wallet age | `stage1_min_wallet_age_days` | 45 | Age = days since oldest trade in `trades_raw` (not discovery time). New wallets = insufficient data or sniper risk |
| Minimum trades | `stage1_min_total_trades` | 10 | Can't classify with fewer |
| Basic activity | `stage1_max_inactive_days` | 45 | Dead wallets waste resources |
| All-time ROI | `stage1_min_all_time_roi` | -0.10 (-10%) | Wallets with catastrophic lifetime losses are unfollowable even with recent hot streaks. Cashflow PnL = total sell proceeds - total buy costs (ALL trades). A wallet that destroyed $200K lifetime but had a good month is still down $200K. |
| Not a known bot | Check against `known_bots` list | — | Automated accounts are unfollowable |

### Stage 2: Deep Analysis (async, scheduled background job)

| Persona to Detect | Analysis | Config Key | Exclusion Trigger |
|-------------------|----------|-----------|-------------------|
| **Execution Master** (not yet implemented) | PnL decomposition: directional vs execution | `execution_master_pnl_ratio` | execution_pnl / total_pnl > 0.70 |
| **Tail Risk Seller** | Loss distribution analysis | `tail_risk_min_win_rate`, `tail_risk_loss_multiplier` | win_rate > 0.80 AND max_single_loss > 5x avg_win |
| **Noise Trader** | Churn rate + ROI | `noise_max_trades_per_week`, `noise_max_abs_roi` | trades/week > 50 AND abs(ROI) < 0.02 |
| **Sniper/Insider** | Age + anomalous win rate | `sniper_max_age_days`, `sniper_min_win_rate`, `sniper_max_trades` | age < 30d AND win_rate > 0.85 on < 20 trades |
| **News Sniper** | Ultra-short edge + bursty timing | `news_sniper_max_burstiness_top_1h_ratio` | burstiness spikes + edge collapses with 30-120s delay |
| **Liquidity Provider / Market Maker** | Two-sided flow + mid-centric fills | `liquidity_provider_min_buy_sell_balance`, `liquidity_provider_min_mid_fill_ratio` | buy/sell balance high AND trades cluster near mid |
| **Jackpot Gambler** | PnL concentrated in few trades | `jackpot_min_pnl_top1_share`, `jackpot_max_win_rate` | top-1 trade share dominates PnL, low win-rate/huge variance |
| **Bot Swarm / Micro-trader** | Extreme frequency + micro sizing | `bot_swarm_min_trades_per_day`, `bot_swarm_max_avg_trade_size_usdc` | trades/day extreme AND size small/uniform |
| **Sybil Cluster** (not yet implemented) | DBSCAN clustering on trade timing | `sybil_min_cluster_size`, `sybil_min_overlap` | cluster > 3 wallets AND > 80% trade overlap |

### Persona Traits (Not Personas)

We also store **traits** that refine how we follow a wallet without changing whether it is followable:

| Trait | Meaning | How it is used |
|------|---------|----------------|
| **Topic lane** | Wallet has edge only in one domain (Sports/Politics/Crypto/...) | Rank and optionally copy **only within that lane** |
| **Bonder** | High-probability grinder: trades concentrated near price 0.0/1.0 with longer holds | More copyable at 30s-120s delays; prefer them |
| **Whale** | Large sizing and/or slow accumulation | Tighten slippage/impact checks; avoid lying to ourselves about fills |

Traits are stored in `wallet_persona_traits` as:
- `TOPIC_LANE=<domain>`
- `BONDER=1`
- `WHALE=1`

**Important distinction:** **Sniper/Insider** is a *risk persona* (suspicious new wallet + anomalous wins). **News Sniper** is a *style persona* (edge is mainly speed-dependent). Either can be unfollowable.

### Trust Multipliers (applied to WScore)

| Wallet Age | Multiplier | Config Key |
|-----------|-----------|-----------|
| < 30 days | EXCLUDED at Stage 1 | `stage1_min_wallet_age_days` |
| 30-90 days | 0.8x | `trust_30_90_multiplier` |
| 90-365 days | 1.0x (normal) | — |
| 365+ days | 1.0x (high confidence) | — |

**Obscurity bonus:** Wallets NOT on public leaderboard top-500 → 1.2x WScore multiplier.
Config key: `obscurity_bonus_multiplier` (default: 1.2).

### Recording Rule

**Every exclusion is recorded. No wallet is silently dropped.**

`wallet_exclusions` table stores: `proxy_wallet`, `reason`, `metric_value`, `threshold`, `excluded_at`.

The UI shows the full list of excluded wallets and why they were excluded.

---

## 5. Re-evaluation

### Weekly Full Re-classification

All paper-traded wallets are re-classified weekly. Personas can change. If a wallet's persona changes from followable to non-followable → immediately stop paper-trading, record the change.

### Anomaly Triggers (continuous monitoring)

If a paper-traded wallet suddenly exhibits any of these, trigger **immediate re-evaluation** and pause paper trading:

| Anomaly | Threshold | Config Key |
|---------|-----------|-----------|
| Win rate drop | > 15 percentage points from historical avg | `anomaly_win_rate_drop_pct` |
| Drawdown spike | > 20% in a single week | `anomaly_max_weekly_drawdown_pct` |
| Frequency change | Trade frequency changes > 3x from historical | `anomaly_frequency_change_multiplier` |
| Position size anomaly | Single trade > 10x their normal size | `anomaly_size_change_multiplier` |

---

## 6. Paper Trading Engine

### Design Principle

Paper trading is a **proof system**, not a game. Every paper trade must answer: "Would this have made money in reality?" If we can't prove it, the paper trade is worthless.

### Mirror strategy: copy direction, timing, and sizing (proportional)

We copy **direction** (buy/sell), **timing** (when they trade), and **sizing** (relative to bankroll). Their edge often includes *when to go big vs small* — e.g. small when uncertain, large when confident. If we use a **flat** size for every trade, paper PnL and live results will not match their profile: we underweight their big winners and overweight their small losers. So:

- **Proportional sizing (required for success):** For each trade, we use their trade size (from API) and scale to our bankroll:  
  **our_size = their_size × (our_bankroll / estimated_their_bankroll)**  
  then clamp to our risk limits (max % per trade, per wallet, per market). Same formula in **paper** (with paper bankroll) and **live** (with real bankroll) so that paper results are representative of live.

- **Estimating their bankroll:** We do not see their account. Options: (a) rolling sum of their open exposure or recent position sizes as proxy; (b) assume a nominal "effective bankroll" per wallet from their typical position size (e.g. if they often risk ~2%, use their_size / 0.02); (c) configurable per-wallet or global default. Estimate must be conservative so we don't over-size when they're actually small.

- **Caps:** Our size is always capped by: single-trade max (% of our bankroll, e.g. 50%), per-wallet exposure %, portfolio exposure %, and daily loss limits. So we never exceed our risk framework.

- **When can flat sizing ever work?** Only in narrow cases: (1) pure direction/timing test with no intention to replicate their returns (e.g. "did we get the sign right?"); (2) all copied traders happen to use nearly the same fixed size and we match that size; (3) we have no size data (e.g. backtest). For **paper that predicts live** and **live copy that replicates their edge**, we must use proportional sizing. Flat is acceptable only as a **fallback** when we cannot estimate their bankroll (then use `per_trade_size_usd` and document that paper/live will not match their sizing profile).

### Realism Requirements

| Aspect | What We Do | Why |
|--------|-----------|-----|
| **Slippage** | Apply estimated slippage to entry price | We won't get their exact price |
| **Fees** | Conditional: quartic for 15m crypto, zero for everything else | Real cost on Polymarket (most markets = zero) |
| **Fill probability** | Check orderbook at detection time (when available) | Don't paper-trade fills that couldn't happen |
| **Detection delay** | Record time delta: their trade timestamp vs our detection | Measure realistic lag |
| **Copy fidelity** | Track: trades_we_copied / trades_they_made | If < 80%, paper PnL diverges from reality |
| **Settlement** | Settle when market resolves ($1 or $0) | Must close the loop on every trade |

### Paper Trade Lifecycle

```
1. Detect wallet's new trade in trades_raw (we have their price and size_usd).
2. Record: their_price, their_size_usd, our_detection_time, market_state_at_detection
3. Compute our_size_usd = proportional size: their_size_usd * (our_bankroll / estimated_their_bankroll), then clamp to our max % per trade and risk caps; if we cannot estimate their bankroll, fall back to per_trade_size_usd.
4. Calculate: our_entry_price = their_price + estimated_slippage
5. Apply fee to our_entry_price
6. Check portfolio-level risk only (high fidelity — minimize per-trade gates):
   a. Per-wallet exposure + our_size_usd < per_wallet_max_exposure_pct of bankroll
   b. Portfolio total exposure + our_size_usd < max_total_exposure_pct of bankroll
   c. Daily loss limit not exceeded
   → If any limit hit: SKIP, log which limit and values
7. Create paper_trade (status: "open") with size_usdc = our_size_usd
8. When market resolves:
   - Win: PnL = (1.0 - our_entry_price) * our_size_usd
   - Loss: PnL = (0.0 - our_entry_price) * our_size_usd
   - Fees already baked into entry price
9. Update portfolio PnL + per-wallet PnL
```

### Copy Fidelity Tracking

Every trade the followed wallet makes gets exactly one outcome:

| Outcome | What Happened |
|---------|-------------|
| `COPIED` | We paper-traded it |
| `SKIPPED_PORTFOLIO_RISK` | Portfolio exposure cap hit |
| `SKIPPED_WALLET_RISK` | Per-wallet exposure cap hit |
| `SKIPPED_DAILY_LOSS` | Daily loss limit hit |
| `SKIPPED_MARKET_CLOSED` | Market already resolved/expired |
| `SKIPPED_DETECTION_LAG` | Detected too late: fill price moved beyond our max slippage budget (when orderbook available) |
| `SKIPPED_NO_FILL` | Orderbook depth couldn't fully fill our size (when orderbook available) |

**copy_fidelity = COPIED / (COPIED + all SKIPPED_*)**

If fidelity < `min_copy_fidelity_pct` (default: 80%) for a wallet → paper PnL is unreliable → FLAG wallet.

**Detection lag threshold (definition):** When orderbook data is available, we define "opportunity gone" by our **slippage budget**, not by an arbitrary mid-price move. For a BUY, if the book-walked VWAP needed to fill `per_trade_size_usd` implies slippage (VWAP - their_price) greater than our slippage budget (use `slippage_default_cents` as the current budget), we record `SKIPPED_DETECTION_LAG`. For a SELL, the analogous case is (their_price - VWAP) exceeding the budget. If the book cannot fill the full size at all (insufficient depth), record `SKIPPED_NO_FILL`.

**Optional: opportunity window / exploitability.** If we sample orderbook fill-feasibility for a short bounded window after detection (e.g. up to 120s), we can record `opportunity_window_secs` = how long the trade remained fillable within our slippage budget. This can be summarized as an exploitability curve: `P(opportunity_window_secs >= L)` for latency `L` (per wallet and overall).

### Parameters (all in `[paper_trading]` section of `default.toml`)

```toml
[paper_trading]
bankroll_usd = 1000
# Proportional sizing: our_size = their_size * (our_bankroll / estimated_their_bankroll), then clamp.
# When we cannot estimate their bankroll, use per_trade_size_usd (flat fallback).
per_trade_size_usd = 25              # fallback when proportional not available
mirror_use_proportional_sizing = true # if false, use flat per_trade_size_usd (not recommended)
# Optional: default assumed "effective bankroll" per wallet when we have no estimate (e.g. 5000).
# Their size is then interpreted as a fraction of that; we scale to our bankroll.
mirror_default_their_bankroll_usd = 5000
max_total_exposure_pct = 15.0
max_daily_loss_pct = 3.0
max_concurrent_positions = 20
min_copy_fidelity_pct = 80.0
slippage_default_cents = 1.0
strategies = ["mirror"]
mirror_delay_secs = 0
```

---

## 7. Risk Management (Two Levels)

### 7.1. Level 1: Per-Wallet Risk

Each followed wallet has its own risk envelope. One bad wallet cannot destroy the portfolio.

| Control | Default Threshold | Config Key | Action on Breach |
|---------|------------------|-----------|-----------------|
| Max exposure | 5% of bankroll | `max_exposure_per_wallet_pct` | Skip new trades until positions close |
| Daily loss | 2% of bankroll | `per_wallet_daily_loss_pct` | Pause this wallet for the day |
| Weekly loss | 5% of bankroll | `per_wallet_weekly_loss_pct` | Pause for the week, trigger re-evaluation |
| Max drawdown | 15% from peak PnL | `per_wallet_max_drawdown_pct` | **KILL** — stop paper-trading, re-classify |
| Follower slippage | avg slippage > their avg edge | `per_wallet_max_slippage_vs_edge` | **KILL** — we lose even copying perfectly |
| Copy fidelity | < 80% over 7 days | `min_copy_fidelity_pct` | **FLAG** — paper PnL unreliable |

### 7.2. Level 2: Portfolio Risk (all wallets combined)

| Control | Default Threshold | Config Key | Action on Breach |
|---------|------------------|-----------|-----------------|
| Total exposure | 15% of bankroll | `max_total_exposure_pct` | Skip ALL new trades |
| Daily loss | 3% of bankroll | `portfolio_daily_loss_pct` | Halt ALL trading for the day |
| Weekly loss | 8% of bankroll | `portfolio_weekly_loss_pct` | Halt ALL trading for the week |
| Concurrent positions | 20 | `max_concurrent_positions` | Skip new trades |
| Correlation cap | 5% per theme | `max_theme_exposure_pct` | Skip over-represented themes |

### 7.3. Follower Slippage & Position Enforcement (the critical metric)

```
follower_slippage = (our_avg_entry - their_avg_entry) + our_fees
```

If `follower_slippage` consistently exceeds the trader's edge → we lose money even copying perfectly → **KILL** that wallet.

Track per wallet, per market, and in aggregate. This is the metric that determines if copy-trading is viable at all.

**Position Size Enforcement:** Individual trade sizes must not exceed 50% of total bankroll to prevent single-event catastrophic failure (see `mirror_trade_to_paper` implementation).

---

## 8. Timing & Polling

### Polling Intervals by Phase

| Phase | Trade Polling | Config Key | Why |
|-------|--------------|-----------|-----|
| Phase 3 (ingestion) | 60s | `trades_poll_interval_secs` | Timely data collection |
| Phase 4 (paper trading) | 60s | same | Measure realistic detection lag |
| Phase 6 (live trading) | 5-10s | same (reconfigure) | Fast execution for real money |

All intervals are configurable. Start conservative, reduce as we confirm rate limits.

### Full Execution Flow (Phase 6, Live)

```
Their trade happens on-chain
    ↓ (~0-5s)
Trade appears in Data API /trades endpoint
    ↓ (5-10s — our polling interval)
We detect the new trade (total: ~10-15s after their trade)
    ↓ (~200ms)
Risk checks + trade decision
    ↓ (5-30s intentional delay, configurable)
Our order placed (total: ~20-50s after their trade)
```

### Intentional Delay Rationale

- **Anti-detection:** Instant copying is detectable and exploitable
- **Edge proof:** Trades surviving 5-30s delay prove directional edge, not execution edge
- **Configurable:** `mirror_delay_secs` in `default.toml`

---

## 9. Phases & Exit Criteria

### Phase Progression (never skip a phase)

| Phase | Name | Exit Criteria (ALL must pass) | Min Duration |
|-------|------|------------------------------|-------------|
| 0 | Foundation | Compiles, APIs reachable, all tests pass | — |
| 1 | Event Discovery | EScore for events, top-50 selected, 3 consecutive scoring days | 3 days |
| 2 | Wallet Discovery | Stage 1 + Stage 2 running, every wallet has persona OR exclusion, zero unclassified wallets | — |
| 3 | Long-Term Tracking | 7 days continuous ingestion, <1hr lag, no data gaps | 7 days |
| 4 | Paper Trading | 7+ days paper trading, all risk gates active, settlement working, copy fidelity >80% for all wallets | 7 days |
| 5 | Wallet Ranking | WScore stable (<20% rank change day-over-day) for 3 consecutive days | 3 days |
| 6 | Production | AWS deployed, 72hr uptime, Prometheus + Grafana live | 72 hours |

### Kill Wallet Triggers

Any of these → stop paper-trading that wallet immediately:

- Paper PnL < -10% over 7 days
- Hit rate < 40% over 30+ trades
- No activity for 14+ days
- Flagged as sybil with high confidence
- Follower slippage exceeds their edge
- Persona re-classified to non-followable

### Promote Wallet Criteria (for real money, future)

ALL of these must be true:

- Paper PnL > +5% over 7d AND > +10% over 30d
- Hit rate > 55% over 50+ trades
- Active in last 7 days
- Max drawdown < 15%
- Consistent across at least 2 rolling windows (14+ days)
- Human explicitly approves

### Real Money Transition (future)

- At least 3 wallets meet "follow-worthy" criteria for 30+ days
- Combined paper portfolio Sharpe > 1.0 for 30 days
- Start at $100-200, scale up weekly if profitable
- Human approves specific bankroll amount

---

## 10. What the UI Must Show

### Per Followed Wallet (Journey View)

```
Wallet 0xABC...
├── Persona: Informed Specialist (confidence: 0.87)
├── Status: ACTIVE (paper-trading since 2026-01-15)
├── Last re-evaluated: 2026-02-05
├── Copy fidelity: 92% (46/50 trades copied)
├── Paper PnL: +$47.30 (+4.73%)
├── Current exposure: $35.00 (3.5% of bankroll)
├── Follower slippage: -0.8 cents avg
├── Risk gates triggered: 2 this week (both portfolio-level)
├── Journey:
│   ├── 2026-01-10: Discovered (HOLDER on "US Election 2028")
│   ├── 2026-01-10: Stage 1 PASSED (age: 245 days, trades: 87)
│   ├── 2026-01-12: Stage 2 PASSED (persona: Informed Specialist, confidence: 0.87)
│   ├── 2026-01-15: Paper trading started
│   ├── 2026-01-22: 7-day review: +3.2% ROI, 58% hit rate OK
│   ├── 2026-02-05: Weekly re-evaluation: persona confirmed
│   └── 2026-02-08: Current status
└── Anomaly alerts: None
```

### Excluded Wallets List

```
Excluded Wallets (47 total)
├── 0xDEF... — Tail Risk Seller (win_rate: 83%, max_loss: 12x avg_win) — excluded 2026-01-11
├── 0x123... — Noise Trader (52 trades/week, ROI: -0.3%) — excluded 2026-01-12
├── 0x456... — Sniper/Insider (age: 12 days, win_rate: 91% on 8 trades) — excluded 2026-01-13
├── 0x789... — Sybil Cluster #3 (5 wallets, 94% trade overlap) — excluded 2026-01-14
├── 0xABC... — Stage 1: too young (age: 5 days) — excluded 2026-01-15
└── ... [full list with pagination]
```

### Portfolio Overview

```
Paper Portfolio ($1,000 simulated bankroll)
├── Total PnL: +$127.50 (+12.75%)
├── Active positions: 8 ($142.00 exposure, 14.2%)
├── Wallets followed: 4
├── Copy fidelity (avg): 89%
├── Avg follower slippage: -1.2 cents
├── Risk status: ALL CLEAR
│   ├── Daily loss: -$5.20 / $30.00 limit (17%)
│   ├── Portfolio exposure: $142 / $150 limit (95%) ← APPROACHING
│   └── Concurrent positions: 8 / 20
└── Last 7 days: +$32.40 (+3.24%)
```

---

## Appendix A: Key Formulas

### MScore (Market Score) & EScore (Event Score)

MScore is computed per **market** (condition_id). **EScore** = `max(MScore)` over markets in that event. See §Event Discovery & EScore for full definition and usage.

```
MScore = 0.25 * liquidity_factor
       + 0.25 * volume_factor
       + 0.20 * density_factor
       + 0.15 * (1.0 - whale_concentration)
       + 0.15 * time_to_expiry_factor

liquidity_factor = min(1.0, log10(liquidity + 1) / log10(1_000_000))
volume_factor    = min(1.0, log10(volume + 1) / log10(500_000))
density_factor   = min(1.0, trades_24h / 500.0)
time_factor      = bell curve peaking at 7-30 days, drops at extremes
```

### WScore (Wallet Score)

```
WScore = 0.30 * edge_score
       + 0.25 * consistency_score
       + 0.20 * market_skill_score
       + 0.15 * timing_skill_score
       + 0.10 * behavior_quality_score
       * trust_multiplier
       * obscurity_bonus
```

### Taker Fees

**Most Polymarket markets have ZERO trading fees.** The quartic taker fee formula applies ONLY to 15-minute crypto price prediction markets (BTC, ETH). Source: https://docs.polymarket.com/polymarket-learn/trading/fees

```
# Only for 15-minute crypto markets (BTC, ETH price predictions):
fee = price * 0.25 * (price * (1 - price))^2
# Max ~1.56% at p=0.50, approaches zero near p=0 or p=1.

# For all other markets (political, sports, weather, etc.):
fee = 0
```

Detection: check market title/slug for crypto asset names (BTC, ETH, bitcoin, ethereum) AND 15-minute time frame indicators (15m, 15 min). If both present → quartic fee. Otherwise → zero.

### PnL Decomposition

```
actual_pnl    = sum((settle_price - entry_price) * size)
directional   = sum((settle_price - mid_at_entry) * size)
execution     = sum((mid_at_entry - entry_price) * size)
// actual = directional + execution
```

### Follower Slippage

```
follower_slippage = (our_avg_entry - their_avg_entry) + our_fees
```

---

## Appendix B: Config Key Reference

All thresholds referenced in this document are configurable. See `config/default.toml` for current values. No strategic decision is hardcoded — everything flows from config.

## Appendix C: Relationship to Other Docs

| Document | Relationship |
|----------|-------------|
| `docs/EVALUATION_STRATEGY.md` | Phase gates and evaluation framework — this bible supersedes for strategic decisions |
| `docs/on_risk.txt` | Detailed risk management — this bible summarizes; refer to on_risk.txt for circuit breaker specifics |
| `docs/prd.txt` | Product requirements — this bible is the operational translation |
| `CLAUDE.md` | Development guide — references this bible for strategy |
