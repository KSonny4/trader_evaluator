# How It Works: The Complete Wallet Journey

> From market discovery to "who to follow" — a comprehensive walkthrough of every stage.

## Overview

The system discovers promising Polymarket trader wallets, computes behavioral features from their on-chain trades, classifies them into personas, scores them for copy-worthiness (WScore), and ranks the best ones for mirror-trading. The evaluator works purely from historical on-chain data — no paper trading is required to rank wallets.

```
Markets (Gamma API)
    → MScore/EScore ranking → Top-50 events
        → Wallet Discovery (holders + traders)
            → Long-Term Tracking (trades, positions, activity)
                → Feature Computation (FIFO paired trades)
                    → Persona Classification (3 followable, 8 exclusions)
                        → WScore Ranking (5-component weighted score)
                            → "Who to Follow" with evidence
```

---

## Stage 1: Market Discovery & Scoring

**Goal:** Find the most "follow-worthy" markets on Polymarket.

**Source:** Gamma API (`GET /markets`) — fetches active markets filtered by minimum liquidity ($1,000) and volume ($5,000).

### MScore (Market Score)

Each market gets an **MScore in [0, 1]** from 5 weighted signals:

| Signal | Weight | Normalization | What it measures |
|--------|--------|---------------|------------------|
| Liquidity | 0.25 | log10(liquidity) / log10(1M) | Deep markets attract serious traders |
| Volume | 0.25 | log10(volume) / log10(500K) | Active markets have more signal |
| Trade density | 0.20 | trades_24h / 500 | Frequent trading = analyzable |
| Whale concentration | 0.15 | 1 - top_holder_concentration | Dispersed = less manipulated |
| Time to expiry | 0.15 | Bell curve (peak at 7-30d) | Sweet spot for copy-trading |

An **activity gate** multiplier `(liquidity + volume + density) / 3` prevents dead markets from scoring high on time-to-expiry alone.

### EScore (Event Score)

Markets are grouped by event. **EScore = max(MScore)** across all markets in the event. The top 50 events by EScore advance to wallet discovery.

**Output:** `market_scores` table — condition_id, mscore, rank.

**Code:** `crates/evaluator/src/market_scoring.rs` — `compute_mscore()`, `rank_events()`

---

## Stage 2: Wallet Discovery

**Goal:** Find real traders in the top markets.

Three discovery sources feed the `wallets` table:

### Holders Discovery
For each top-50 market, fetch the top 20 holders via Data API (`GET /holders`). These are wallets with the largest positions — tagged `discovered_from = "HOLDER"`.

### Trader Discovery
For each top-50 market, fetch recent trades via Data API (`GET /trades`). Wallets with >= 5 trades in the market are kept — tagged `discovered_from = "TRADER_RECENT"`.

**Dedup rule:** If a wallet appears as both holder and trader, the HOLDER tag wins (earliest tag preserved).

### Leaderboard Discovery
Separately, fetch the Polymarket global leaderboard (`GET /v1/leaderboard`) across categories (OVERALL, POLITICS, CRYPTO) and time periods (WEEK, MONTH). Tagged `discovered_from = "LEADERBOARD"`.

**Output:** `wallets` table — proxy_wallet, discovered_from, discovered_at, is_active.

**Code:** `crates/evaluator/src/wallet_discovery.rs`, `jobs/pipeline_jobs.rs` — `run_wallet_discovery_once()`, `run_leaderboard_discovery_once()`

---

## Stage 3: Long-Term Tracking (Ingestion)

**Goal:** Build a complete trading history for each discovered wallet.

Four ingestion jobs run on fixed intervals, populating raw data tables:

| Job | Interval | API | Table | What it captures |
|-----|----------|-----|-------|------------------|
| Trades | Hourly | `GET /trades?user=` | `trades_raw` | Every trade: side, size, price, timestamp |
| Activity | 6 hours | `GET /activity?user=` | `activity_raw` | TRADE, SPLIT, MERGE, REDEEM events |
| Positions | Daily | `GET /positions?user=` | `positions_snapshots` | Current holdings per market |
| Holders | Daily | `GET /holders?market=` | `holders_snapshots` | Top holders for whale tracking |

All API calls use a 200ms rate limit delay. Trades ingestion caps at 3,000 trades per wallet per run (15 pages x 200).

**Output:** `trades_raw`, `activity_raw`, `positions_snapshots`, `holders_snapshots` tables.

**Code:** `crates/evaluator/src/jobs/ingestion_jobs.rs`

---

## Stage 4: Feature Computation

**Goal:** Distill raw trades into a 24-field behavioral fingerprint per wallet.

### FIFO Paired Trade Stats

The core algorithm pairs BUY and SELL trades per market using FIFO (first-in-first-out):

```
Market m1:  BUY 10@0.50 → SELL 10@0.60 → PnL = (0.60 - 0.50) × 10 = $1.00 (win)
Market m2:  BUY 5@0.70  → SELL 5@0.55  → PnL = (0.55 - 0.70) × 5 = -$0.75 (loss)
```

From these pairs we derive: win_count, loss_count, total_pnl, hold_times, and per-market profitability.

### WalletFeatures Struct

Computed for configurable windows (7d, 30d, 90d):

| Feature | Description | Source |
|---------|-------------|--------|
| `trade_count` | Total trades in window | COUNT from trades_raw |
| `win_count` / `loss_count` | Round-trips with positive/negative PnL | FIFO pairing |
| `total_pnl` | Sum of all closed PnLs | FIFO pairing |
| `profitable_markets` | Markets where net FIFO PnL > 0 | Per-market PnL grouping |
| `avg_position_size` | Average trade value (size x price) | AVG from trades_raw |
| `unique_markets` | Distinct markets traded | COUNT DISTINCT condition_id |
| `avg_hold_time_hours` | Average round-trip duration | (sell_ts - buy_ts) / 3600 |
| `max_drawdown_pct` | Peak-to-trough drawdown | Daily equity curve |
| `sharpe_ratio` | Annualized risk-adjusted return | daily_returns, x sqrt(252) |
| `trades_per_week` / `trades_per_day` | Trading frequency | trade_count / window |
| `active_positions` | Currently open positions | positions_snapshots |
| `concentration_ratio` | Volume in top-3 markets / total | Sorted market volumes |
| `size_cv` | Size coefficient of variation | std(sizes) / mean(sizes) |
| `buy_sell_balance` | Buy/sell ratio balance | 1 - abs(buys - sells) / total |
| `mid_fill_ratio` | Trades at price [0.45, 0.55] | Ratio of mid-priced fills |
| `extreme_price_ratio` | Trades at price < 0.10 or > 0.90 | Ratio of extreme fills |
| `burstiness_top_1h_ratio` | Max trades in any 1h / total | Sliding window |
| `top_domain` / `top_domain_ratio` | Dominant Polymarket category | From markets table |

Features are persisted to `wallet_features_daily` on every scoring run.

**Code:** `crates/evaluator/src/wallet_features.rs` — `paired_trade_stats()`, `compute_wallet_features()`, `save_wallet_features()`

---

## Stage 5: Persona Classification

**Goal:** Label each wallet with a persona (followable or excluded).

### Phase 1: Fast Filters

Quick disqualification before expensive feature computation:

| Check | Threshold | Exclusion Reason |
|-------|-----------|------------------|
| Wallet age | < 45 days | TOO_YOUNG |
| Total trades | < 10 | TOO_FEW_TRADES |
| Days since last trade | > 45 | INACTIVE |
| Known bot list | configurable | KNOWN_BOT |

Wallets passing Phase 1 advance to feature computation and Phase 2.

### Phase 2: Persona Detection

Features are computed, then each wallet is tested against **3 followable personas** and **8 exclusion detectors**.

#### Followable Personas

| Persona | Key Criteria | Copy Strategy |
|---------|-------------|---------------|
| **INFORMED_SPECIALIST** | active_positions <= 5, concentration >= 60%, win_rate >= 60% | mirror_with_delay |
| **CONSISTENT_GENERALIST** | unique_markets >= 20, win_rate 52-60%, drawdown <= 15%, Sharpe >= 1.0 | mirror |
| **PATIENT_ACCUMULATOR** | avg_hold >= 48h, trades_per_week <= 5, ROI >= 5% | mirror_slow |

#### Exclusion Detectors

| Detector | Why Excluded | Key Signal |
|----------|-------------|------------|
| Execution Master | Edge from speed, not direction | execution_pnl_ratio > 0.70 |
| Tail Risk Seller | Rare catastrophic losses | win_rate > 80% AND max_loss >> avg_win |
| Noise Trader | High churn, no edge | trades/week > 50 AND abs(ROI) < 2% |
| Sniper/Insider | Suspiciously new + perfect | age < 30d AND win_rate > 85% |
| News Sniper | Unreplicable news speed | burstiness > 70% |
| Liquidity Provider | Market-making, not trading | balanced buys/sells AND mid-fills |
| Jackpot Gambler | Lucky single bet, not skill | top-1 PnL >= 60% of total AND win_rate <= 45% |
| Bot Swarm | Micro-trading automation | trades/day > 200 AND avg_size < $5 |

**Priority:** Exclusions are checked first. A wallet matching any exclusion is excluded regardless of persona match.

**Output:** `wallet_personas`, `wallet_exclusions`, `wallet_persona_traits` tables.

**Code:** `crates/evaluator/src/persona_classification.rs`

---

## Stage 6: Rules Engine (State Machine)

**Goal:** Gate wallets through progressive trust levels.

### State Transitions

```
   Candidate
       │
       ▼ evaluate_discovery (pass?)
   PaperTrading  ←── "Eligible for operator to push to trader service"
       │
       ▼ evaluate_paper (on-chain validation pass?)
    Approved
       │
       ▼ evaluate_live (breaker triggered?)
    Stopped  ────► Can re-enter Candidate
```

### Gate Functions

**Discovery Gate** — Candidate to PaperTrading:
- min 50 trades, max 120 trades/day, max 60 markets
- min 180 min avg hold time, max 0.75 size CV
- max 0.70 burstiness ratio

**Paper Gate** — PaperTrading to Approved (14-day window):
- min 30 closed round-trip trades
- avg PnL per trade >= $0 (profitable)
- max drawdown <= 8%
- At transition: captures `baseline_style_json` snapshot

**Live Gate** — Approved stays or Stopped (90-day window):
- Inactivity check: must have traded within 10 days
- Drawdown check: max 12% from peak
- Style drift: weighted comparison of 5 behavioral metrics against baseline
  - trades_per_day (30%), unique_markets (20%), burstiness (25%), buy_sell_balance (15%), top_domain_ratio (10%)
  - If drift score > 0.65 → Stopped
- Theme concentration: top_domain_ratio > 0.55 → Stopped

**Output:** `wallet_rules_state`, `wallet_rules_events` (audit log) tables.

**Code:** `crates/evaluator/src/wallet_rules_engine.rs`

---

## Stage 7: WScore Ranking

> **Why after the Rules Engine?** The rules engine (Stage 6) gates wallets through trust levels (Candidate → PaperTrading → Approved). WScore provides the ranking used to decide *which* wallets the operator promotes at each gate. Both run independently — rules engine every 5 minutes, scoring daily — but scoring logically follows because you need the ranked list to make promotion decisions.

**Goal:** Produce a single [0, 1] copy-worthiness score per wallet.

### Bridge: Features to Score Input

`score_input_from_features()` maps WalletFeatures to WalletScoreInput:

| Score Input | Derived From |
|-------------|-------------|
| roi_pct | 100 x total_pnl / (avg_position_size x trade_count) |
| daily_return_stdev_pct | max_drawdown_pct x 0.5 (heuristic proxy) |
| hit_rate | win_count / (win_count + loss_count) |
| profitable_markets | Direct from features |
| total_markets | unique_markets |
| noise_trade_ratio | extreme_price_ratio x 0.5 + burstiness x 0.5 |

### 5-Component WScore

```
WScore = weighted_average(
    edge            × 0.30,    // ROI normalized to [0,1], cap at 20%
    consistency     × 0.25,    // Low return volatility = high score
    market_skill    × 0.20,    // Fraction of markets profitable
    timing_skill    × 0.15,    // Post-entry price drift (TODO: not yet computed)
    behavior_quality × 0.10    // Low noise trade ratio = high score
)
```

### Adjustments

| Adjustment | Condition | Multiplier |
|------------|-----------|------------|
| Win rate penalty (heavy) | hit_rate < 45% | x 0.50 |
| Win rate penalty (mild) | hit_rate < 52% | x 0.80 |
| Youth penalty | wallet age < 90 days | x 0.80 |
| Obscurity bonus | NOT on public leaderboard | x 1.20 |

Final WScore is clamped to [0, 1].

### Scoring Pipeline

`run_wallet_scoring_once()` is a scheduled job that runs once per day. It iterates all active wallets, computes behavioral features from `trades_raw`, calculates WScore, and persists both to `wallet_features_daily` and `wallet_scores_daily`:

1. Fetch all active wallets with sufficient trades in `trades_raw`
2. For each wallet x window (7d, 30d, 90d):
   - `compute_wallet_features()` from trades_raw
   - `save_wallet_features()` to wallet_features_daily
   - `score_input_from_features()` to build scoring input
   - `compute_wscore()` to get final score
3. Upsert into `wallet_scores_daily`

**Output:** `wallet_scores_daily` — proxy_wallet, wscore, edge_score, consistency_score, roi_pct, per date/window.

**Code:** `crates/evaluator/src/wallet_scoring.rs`, `jobs/pipeline_jobs.rs` — `run_wallet_scoring_once()`

### On-Demand Feature Computation

When `wallet_discovery` inserts a new wallet, it spawns a background tokio task to compute 30d window features immediately. This enables classification within the next hourly persona run (~1h latency vs ~25h).

- Silent failure if wallet has <5 settled trades
- Daily batch scoring remains authoritative (computes all 3 windows)
- Metrics: `evaluator_on_demand_features_total{status="success|failure"}`

---

## Stage 8: Output & CLI

### CLI Commands

```bash
evaluator markets         # Top markets today (by MScore)
evaluator wallets         # Wallet watchlist (last 200 discovered)
evaluator wallet <addr>   # Single wallet deep-dive
evaluator rankings        # Top WScore wallets (30d window)
evaluator classify        # Trigger persona classification
evaluator pick-for-paper  # Wallets eligible for paper trading
```

### Example: `evaluator wallet 0xabc`
```
Wallet: 0xabc
  discovered_from=HOLDER  is_active=1
  trades_raw rows=142
  on_chain_pnl=23.45  wins=38  losses=22  markets=15  drawdown=4.2%
  wscore=0.732 (30d)
  state=PAPER_TRADING
  persona=INFORMED_SPECIALIST
```

> **What is drawdown?** `max_drawdown_pct` is the largest peak-to-trough decline in the wallet's equity curve over the scoring window, expressed as a percentage. For example, if equity peaked at $1,000 and fell to $850, drawdown = 15.0%. Lower is better — it measures the worst loss a wallet experienced before recovering.

### Example: `evaluator pick-for-paper`
```
Top wallets eligible for paper trading:
 0.832  30d  INFORMED_SPECIALIST        0xabc123...
 0.791  30d  CONSISTENT_GENERALIST      0xdef456...
 0.724  30d  PATIENT_ACCUMULATOR        0x789abc...
```

---

## Scheduler: How It All Runs

The main process (`cargo run -p evaluator`) starts a tokio runtime with these periodic jobs:

| Job | Interval | What it does |
|-----|----------|-------------|
| Event scoring | Hourly | Re-score markets from Gamma API |
| Wallet discovery | Continuous | Loops through top markets with rate limiting (200ms between API calls), discovering new wallets as they appear. When a batch of markets is fully processed, it restarts from the beginning |
| Leaderboard discovery | With wallet discovery | Fetch global leaderboard |
| Trades ingestion | Hourly | Fetch new trades for active wallets. Future improvement: trigger on new wallet discovery in batches |
| Activity ingestion | 6 hours | Fetch activity timelines |
| Positions snapshot | Daily | Capture each wallet's current holdings (shares per market). Used for: active_positions count in WalletFeatures, detecting when wallets close positions, monitoring portfolio concentration |
| Holders snapshot | Daily | Capture top 20 holders per market. Used for: whale_concentration signal in MScore, detecting when a wallet appears/disappears from top holders, holder discovery source |
| Persona classification | Hourly | Run Phase 1 + Phase 2 classification |
| Wallet rules | 5 minutes | Run state machine transitions |
| Wallet scoring | Daily | Compute features + WScore |
| Flow metrics | 1 minute | Update Grafana metrics |
| WAL checkpoint | 5 minutes | SQLite maintenance |

At startup, the system bootstraps: score events, discover wallets, and initialize rules state before entering the periodic loop.

**Code:** `crates/evaluator/src/main.rs`

---

## Trader Microservice (Separate)

The **trader** crate (`crates/trader/`) is a separate microservice that handles live paper trading. It:

- Receives wallets promoted by the evaluator (operator decision based on `pick-for-paper` output)
- Runs mirror-trading engines that copy wallet trades with configurable delay and risk caps
- Maintains `paper_trades` and `paper_positions` tables
- Provides a REST API for starting/stopping wallet mirrors

The evaluator and trader are intentionally decoupled: the evaluator scores from on-chain history only, and the trader handles execution simulation independently.

---

## Database Tables

### Core Data
| Table | Purpose |
|-------|---------|
| `markets` | Market metadata (condition_id, title, liquidity, volume, category) |
| `wallets` | Discovered wallets (proxy_wallet, discovered_from, is_active) |
| `trades_raw` | Historical trades (side, size, price, timestamp) |
| `activity_raw` | Activity timeline (activity_type, timestamp) |
| `positions_snapshots` | Current holdings per market |
| `holders_snapshots` | Top holders per market |

### Scoring & Classification
| Table | Purpose |
|-------|---------|
| `market_scores` | MScore per market per day |
| `wallet_features_daily` | 24-field feature vector per wallet/window/day |
| `wallet_scores_daily` | WScore + component scores per wallet/window/day |
| `wallet_personas` | Persona classification (persona, confidence, features JSON) |
| `wallet_exclusions` | Wallets filtered out during persona classification (Stage 5). A wallet gets excluded when any of the 8 exclusion detectors fire (e.g., TAIL_RISK_SELLER when win_rate > 80% with outsized losses, NOISE_TRADER when trades/week > 50 with near-zero ROI). Excluded wallets cannot advance through the rules engine even if they match a followable persona |
| `wallet_persona_traits` | Behavioral traits (TOPIC_LANE, BONDER, WHALE) |

### State & Audit
| Table | Purpose |
|-------|---------|
| `wallet_rules_state` | Current state (Candidate/PaperTrading/Approved/Stopped) |
| `wallet_rules_events` | Audit log of all state transitions |
| `job_status` | Scheduler job status and last-run metadata |

### Trader Service
| Table | Purpose |
|-------|---------|
| `paper_trades` | Simulated trades (kept for trader microservice) |
| `paper_positions` | Simulated holdings (kept for trader microservice) |
