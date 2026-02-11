# Evaluation Strategy: Polymarket Wallet Discovery & Paper Copy Trading

> **Purpose:** Operational guide for phase progression and evaluation criteria. Governed by the Strategy Bible (docs/STRATEGY_BIBLE.md) for all strategic decisions — persona definitions, risk thresholds, scoring formulas. This document defines phase gates and decision rules. It is consumed by both humans and the `trading-evaluator-guidance` skill.

**Last updated:** 2026-02-08

---

## 1) System Maturity Phases

The system progresses through discrete phases. Each phase has entry criteria (what must exist) and exit criteria (what must be proven before advancing). **Never skip a phase.**

### Phase 0: Foundation (BUILD)
**Entry:** Nothing exists yet.
**What to build:** Database schema, Polymarket API client, config system, basic CLI.
**Exit criteria:**
- [x] SQLite database created with all core tables
- [x] Polymarket Data API client can fetch trades, holders, activity, positions
- [x] Gamma API client can list and filter markets
- [x] Config system loads from TOML
- [x] `cargo test --all` passes
- [x] At least one real API response saved as test fixture

### Phase 1: Market Discovery (COLLECT)
**Entry:** Phase 0 complete.
**What to build:** Market scoring (MScore), daily market selection job.
**Exit criteria:**
- [ ] System fetches all active markets from Gamma API
- [ ] MScore computed for each market using: liquidity, volume, trade density, whale concentration
- [ ] Daily top-20 markets selected and persisted to `market_scores_daily`
- [ ] Output is explainable (each factor visible in the score breakdown)
- [ ] Running for 3+ days with stable output

### Phase 2: Wallet Discovery & Classification (COLLECT)
**Entry:** Phase 1 complete (we know which markets to watch).
**What to build:** Wallet extraction from markets, persona classification, watchlist persistence.

**Wallet persona taxonomy (classify every wallet before paper-trading):**

Every discovered wallet gets classified into a persona. This determines whether to follow, how to follow, and what to watch for. Classification uses features computed from `wallet_features_daily` — start with rule-based SQL queries, upgrade to ML classifier once we have labeled data from paper-trading results.

| Persona | Description | Key features | Follow? | Follow mode |
|---------|-------------|-------------|---------|-------------|
| **Informed Specialist** | Trades few markets, high win rate, enters before big moves | unique_markets < 10, win_rate > 60%, pre-move entry timing | YES — primary target | Mirror with delay |
| **Consistent Generalist** | Trades many markets, moderate win rate, steady returns | unique_markets > 20, win_rate 52-60%, low drawdown, Sharpe > 1 | YES | Mirror |
| **Patient Accumulator** | Large positions, long hold times, few trades | avg_hold_time > 48h, avg_position_size > 90th percentile, trade_count < 5/week | YES — but slow | Delay (24h+) |
| **Execution Master** | Makes money from fills, not direction. Maker-like execution, tight spreads | execution_pnl / total_pnl > 0.7, mostly MAKER_LIKE fills, high trade frequency | NO — edge is unreplicable | Do not follow |
| **Tail Risk Seller** | High win rate but occasional massive losses | win_rate > 80%, max_single_loss > 5x avg_win, drawdown spikes | NO — will blow you up | Do not follow |
| **Noise Trader** | Many tiny trades, no statistical edge | trade_count > 50/week, win_rate 45-55%, ROI near zero, high churn | NO — no signal | Do not follow |
| **Sniper/Insider** | Coordinated entries, suspicious timing, new wallet with big wins | wallet_age < 30d AND win_rate > 0.85 on < 20 trades | AVOID — adversarial risk | Do not follow |
| **Sybil Cluster** | Multiple wallets with correlated trades, same funding source | DBSCAN cluster membership, shared funding chain, identical trade timing | AVOID — fake signal | Do not follow |

**Wallet age as trust factor:**
- < 30 days: Wallets < 30 days are excluded at Stage 1 (hard filter). See Strategy Bible §4.
- 30-90 days: reduced confidence — classify but apply 0.8x trust multiplier to WScore. Need more data before committing.
- 90-365 days: normal confidence — enough history to classify reliably.
- > 365 days: high confidence — long track record, stable behavior patterns. If edge persists across multiple regime changes, strong signal.

Wallet age < 30 days is a **hard filter** at Stage 1 — wallets are excluded immediately. Older wallets are still stored and tracked with trust multipliers applied to WScore.

**Obscurity bonus:** Wallets NOT on the public leaderboard and NOT tracked by known analytics tools (predictfolio, polyterm) get a scoring bonus. Rationale: less-known wallets = fewer copiers = less front-running = better fills for us. Detection: check if wallet appears in leaderboard API top-500. If not, apply 1.2x multiplier to WScore.

**The funnel (discover many → trade few):**
```
Discover: hundreds of wallets from all markets
    ↓ classify into personas
Track: all of them (data is cheap, store everything)
    ↓ filter to followable personas only
Paper-trade: ~5-10 best wallets (Informed Specialist, Consistent Generalist, Patient Accumulator)
    ↓ rank by paper performance + follower slippage
Follow with real money: top 3-5 wallets that survive 30+ days of paper-trading
```
The goal is a wide funnel at the top and extreme selectivity at the bottom. We want to watch 500 wallets but only trade alongside 5.

**Continuous re-evaluation (never stop classifying):**
1. Re-classify personas **weekly** — behavior changes, so must the label
2. Re-rank paper-traded wallets **daily** — drop wallets that degrade, promote new ones that qualify
3. A wallet that was "Informed Specialist" can become "Noise Trader" if behavior shifts — auto-reclassify and halt paper-trading
4. A wallet that was "too young" 60 days ago may now qualify — auto-promote when age + data threshold is met
5. Kill wallets fast (see Decision Rules §3.2), promote slowly (§3.3 requires 2 rolling windows of consistency)

**Classification pipeline:**
1. Compute features from `wallet_features_daily` (trade count, win rate, unique markets, hold time, drawdown, PnL decomposition)
2. Apply rule-based persona assignment (thresholds above)
3. Store persona + confidence + feature values in `wallet_personas` table
4. Re-classify weekly as more data arrives
5. Future: train classifier on paper-trading outcomes (label = "profitable to follow" vs "not")

**Exit criteria:**
- [ ] For each selected market: ALL participants extracted and stored (not just top holders — everyone who traded)
- [ ] Wallets tagged with discovery source (HOLDER, TRADER_RECENT, LEADERBOARD)
- [ ] Every wallet classified into a persona with confidence score
- [ ] Persona + features stored in `wallet_personas` table with: wallet, persona, confidence, feature_values, classified_at
- [ ] Exclusion decisions stored in `wallet_exclusions` table with: wallet, reason, metric_value, threshold, excluded_at — never silently drop a wallet
- [ ] No arbitrary candidate count cap — store every wallet we find, filter later by persona + profile fit
- [ ] Watchlist persisted to `wallets` table with `discovered_from` and `discovered_at`
- [ ] Only wallets with persona in {Informed Specialist, Consistent Generalist, Patient Accumulator} advance to paper-trading

### Phase 3: Long-Term Tracking (COLLECT)
**Entry:** Phase 2 complete (we have wallets to watch).
**What to build:** Continuous ingestion jobs for watched wallets.
**Exit criteria:**
- [ ] Trades ingestion running: poll by wallet, deduplicated, append-only
- [ ] Activity ingestion running: poll per wallet at configurable frequency
- [ ] Position snapshots: daily per watched wallet
- [ ] Holder snapshots: daily per selected market
- [ ] Raw JSON stored alongside parsed data
- [ ] 7 days of continuous collection without data gaps
- [ ] Ingestion lag metric tracked and <1 hour

### Phase 4: Paper Trading (EVALUATE)
**Entry:** Phase 3 running for 7+ days (enough wallet data to copy). Account identified as worth following.
**What to build:** Paper trading engine that mirrors wallet trades with full risk management from `docs/on_risk2.txt`.

**Risk management (non-negotiable, from Strategy Bible §7 (primary), docs/on_risk2.txt (supplementary circuit breaker details)):**

All circuit breakers and risk gates must be implemented BEFORE the first paper trade runs. These are not optional enhancements — they define what "paper copy" means.

**Level 1: Per-Wallet Risk (one bad wallet cannot destroy the portfolio):**
| Control | Default Threshold | Action on Breach |
|---------|------------------|-----------------|
| Max exposure | 5% of bankroll | Skip new trades until positions close |
| Daily loss | 2% of bankroll | Pause this wallet for the day |
| Weekly loss | 5% of bankroll | Pause for the week, trigger re-evaluation |
| Max drawdown | 15% from peak PnL | **KILL** — stop paper-trading, re-classify |
| Follower slippage | avg slippage > their avg edge | **KILL** — we lose even copying perfectly |
| Copy fidelity | < 80% over 7 days | **FLAG** — paper PnL unreliable |

**Level 2: Portfolio Risk (all wallets combined):**
| Control | Default Threshold | Action on Breach |
|---------|------------------|-----------------|
| Total exposure | 15% of bankroll | Skip ALL new trades |
| Daily loss | 3% of bankroll | Halt ALL trading for the day |
| Weekly loss | 8% of bankroll | Halt ALL trading for the week |
| Concurrent positions | 20 | Skip new trades |
| Correlation cap | 5% per theme | Skip over-represented themes |

**Gates (skip trade if any gate fails):**

Per-trade gates are supplementary — primary risk management is portfolio-level. See Strategy Bible §7.

| Gate | Threshold |
|------|-----------|
| Spread gate | ≤ 2 cents |
| Entry slippage cap | ≤ 2 cents vs detected price |
| Volatility breaker | price moves ≥ 2 cents in 30s → halt 10 min |
| Resolution clarity | must restate in 1 sentence + 1 source, else skip |
| Copy delay | 5-30s random delay (configurable per persona) |

**Circuit breakers (halt all trading):**
- Error breaker: 3+ order/API errors in 2 minutes → halt all for the day
- Slippage breaker: if average slippage exceeds threshold → halt copying that wallet
- Correlation breaker: cap total open risk per theme (crypto, politics, sports) to ≤ 5% bankroll

**The critical metric (measure this or you're guessing):**
```
follower_slippage = (our_avg_entry - their_avg_entry) + our_fees
```
If follower_slippage is consistently negative by more than the trader's edge, you lose even with perfect risk controls. Track this per wallet and kill wallets where slippage eats the edge.

**Exit criteria:**
- [ ] Mirror-trades strategy implemented (same direction, proportional size)
- [ ] ALL exposure caps implemented and enforced
- [ ] ALL gates implemented (spread, slippage, volatility, resolution clarity, trade count, copy delay)
- [ ] ALL circuit breakers implemented (error, slippage, correlation)
- [ ] Limit-only execution (never market orders — market orders are how copiers get farmed)
- [ ] Follower slippage tracked per wallet
- [ ] Paper bankroll tracked per-wallet portfolio
- [ ] At least 7 days of paper trading results for top-10 wallets
- [ ] Results reproducible from stored events (deterministic replay)
- [ ] Every skip decision logged with reason (which gate failed, by how much)


### Phase 5: Wallet Ranking (EVALUATE)
**Entry:** Phase 4 has 7+ days of paper results.
**What to build:** WScore computation and ranking output.
**Exit criteria:**
- [ ] WScore computed using: edge, consistency, market skill, timing skill, behavior quality
- [ ] Rankings produced for 7d/30d windows
- [ ] Evidence summary per wallet (PnL, drawdown, hit rate, timing metrics)
- [ ] Recommended follow mode per wallet (mirror, delay, consensus)
- [ ] Risk flags generated (concentration, leverage, instability)
- [ ] Top-10 ranked wallets list stable across 3 consecutive days

### Phase 6: Production Validation (VALIDATE)
**Entry:** Phase 5 complete with stable rankings.
**What to build:** Deploy to AWS, run alongside existing trading bots.

> **Note on early deployment:** Deploy to AWS as soon as Phase 3 (Long-Term Tracking) is ready — not Phase 6. Reason: Phase 4 (Paper Trading) needs realistic latency to measure follower_slippage accurately. Running locally adds artificial delay that makes paper results unreliable. The Phase 3 → AWS path is: basic systemd service, SQLite on EBS, Prometheus export. Phase 6 adds the final validation criteria below.

**Exit criteria:**
- [ ] Running on AWS t3.micro (same instance as trading bots)
- [ ] Prometheus metrics exported and visible in Grafana
- [ ] No crashes for 72 hours
- [ ] Ingestion running continuously
- [ ] Paper portfolios updating in real-time
- [ ] WScore updates daily

---

## 2) Evaluation Framework

### 2.1 Market Score (MScore) Evaluation

**How to know if MScore is working:**

| Metric | Good | Marginal | Bad |
|--------|------|----------|-----|
| Markets with MScore >0.5 | >50 per day | 20-50 | <20 |
| Top-20 markets have >$10K volume/day | >80% | 50-80% | <50% |
| Top-20 markets have >50 unique traders/day | >80% | 50-80% | <50% |
| Markets with resolution within 30 days | >70% | 40-70% | <40% |
| MScore ranking stable day-to-day (Kendall tau >0.6) | Yes | 0.3-0.6 | <0.3 |

**If MScore is Bad:** Reweight factors. Check if the Gamma API is returning stale data. Consider adding new factors (social sentiment, historical accuracy of market category).

### 2.2 Wallet Discovery Evaluation

**How to know if we're finding good wallets:**

| Metric | Good | Marginal | Bad |
|--------|------|----------|-----|
| Candidate wallets per market | >50 | 20-50 | <20 |
| Wallets with >10 historical trades | >70% | 40-70% | <40% |
| Wallets active in last 7 days | >50% | 20-50% | <20% |
| Leaderboard overlap (top wallets on leaderboard) | >20% | 5-20% | <5% |
| Sybil-flagged wallets | <10% | 10-30% | >30% |

**If wallet discovery is Bad:** Lower min-trade threshold. Expand to more markets. Add leaderboard seeding. Check if API is rate-limited and we're missing data.

### 2.3 Paper Trading Evaluation

**How to know if copy-trading works:**

| Metric | Good | Marginal | Bad | Kill |
|--------|------|----------|-----|------|
| Portfolio ROI (7d) | >5% | 0-5% | -5% to 0% | <-5% |
| Portfolio ROI (30d) | >10% | 0-10% | -10% to 0% | <-10% |
| Hit rate (% profitable trades) | >55% | 50-55% | 45-50% | <45% |
| Max drawdown (7d) | <10% | 10-20% | 20-30% | >30% |
| Sharpe-like ratio (daily returns) | >1.0 | 0.5-1.0 | 0-0.5 | <0 |
| Number of risk violations/week | 0 | 1-3 | 4-10 | >10 |

**If paper trading is Bad:**
1. Check if it's market-regime dependent (BTC trending vs sideways)
2. Check if specific wallets are dragging performance
3. Reduce position sizes
4. Add delay to mirror strategy (maybe wallets are being front-run)

**If paper trading is Kill:**
1. Stop paper trading that wallet immediately
2. Investigate: was the wallet a sybil/bait?
3. Tighten pruning filters
4. Do NOT re-enable without human approval

### 2.4 WScore Evaluation

**How to know if rankings are meaningful:**

| Metric | Good | Marginal | Bad |
|--------|------|----------|-----|
| Top-10 wallets beat random basket | >2x | 1-2x | <1x |
| Rankings stable across 7d windows (Kendall tau) | >0.5 | 0.2-0.5 | <0.2 |
| Top wallets maintain edge in new markets | >60% | 40-60% | <40% |
| Bottom wallets actually perform worse | Yes | Mixed | No |

---

## 3) Decision Rules

### 3.1 When to advance to next phase
- ALL exit criteria for current phase are checked
- System has been stable for the required time window
- No critical bugs or data quality issues

### 3.2 When to kill a wallet from the watchlist
- Paper PnL < -10% over 7 days
- Hit rate < 40% over 30+ trades
- No activity for 14+ days
- Flagged as sybil with high confidence

### 3.3 When to promote a wallet to "follow-worthy"
- Paper PnL > +5% over 7 days AND >+10% over 30 days
- Hit rate > 55% over 50+ trades
- Active in last 7 days
- Max drawdown < 15%
- Consistent across at least 2 rolling windows

### 3.4 When to start real-money copy trading (Phase 7 - FUTURE)
- At least 3 wallets meet "follow-worthy" criteria for 30+ days
- Combined paper portfolio Sharpe > 1.0 for 30 days
- Human explicitly approves with specific bankroll amount
- Start at $100-200, scale up weekly if profitable

### 3.5 When to pause/halt the entire system
- API rate limits hit consistently (>10% of requests failing)
- Data quality issues (>20% of ingestion jobs failing)
- All paper portfolios in drawdown simultaneously
- Infrastructure cost exceeds budget threshold

---

## 4) Data Quality Checks (Run Daily)

These must pass before any evaluation metrics are trusted:

1. **Ingestion freshness:** Most recent trade for each watched wallet < 2 hours old
2. **Deduplication:** Zero duplicate trade IDs in `trades_raw`
3. **Completeness:** Position snapshots exist for >95% of watched wallets today
4. **Consistency:** Paper trade count matches expected (no gaps in the paper engine)
5. **API health:** <5% error rate on all Polymarket endpoints in last 24h
6. **Schema integrity:** All required fields non-null in raw data

---

## 5) Key Metrics Dashboard (What to Check Daily)

### System Health
- Ingestion job success rate (target: >99%)
- API error rate by endpoint
- Database size and growth rate
- Last successful run timestamp per job

### Pipeline Output
- Markets scored today (should be >100)
- New wallets discovered today
- Wallets currently on watchlist
- Active paper portfolios

### Performance
- Paper PnL by wallet (top 5, bottom 5)
- Paper PnL by strategy (mirror, delay, consensus)
- Hit rate by market category
- Drawdown per portfolio

---

## 6) Technology Decisions

Based on the PRD and reference `trading` project:

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Language | Rust | Reuse Polymarket client from `trading` project. Performance. Type safety for money. |
| Database | SQLite | Same as `trading` project. Zero-dependency deployment. Good enough for our scale. |
| Deployment | AWS t3.micro | Same instance as trading bots. Cost-efficient. Already provisioned. |
| Job scheduling | Tokio timers | No external scheduler needed. Rust async handles periodic jobs. |
| Config | TOML | Same pattern as `trading` project. |
| Observability | Prometheus + Grafana | Same stack as `trading` project. Reuse dashboards. |
| API client | reqwest | Same as `trading` project. |

---

## 7) Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Polymarket API rate limits | High | Medium | Exponential backoff. Cache responses. Batch requests. |
| Wallet discovery finds only noise | Medium | High | Leaderboard seeding as fallback. Manual curation. |
| Paper trading doesn't predict real trading | Medium | High | Conservative slippage model. Delay strategy as baseline. |
| Sybil/bait wallets poison rankings | Medium | High | Multi-window consistency requirement. Per-wallet exposure caps. |
| API changes break ingestion | Low | High | Store raw JSON. Schema versioning. Integration tests. |
| Edge decays over time | High | Medium | Continuous re-evaluation. Rolling windows. Kill underperformers fast. |
| **Execution mismatch (late copy, worse fills)** | **High** | **High** | Limit-only execution. Max slippage gate. Copy delay + price improvement rule. Track follower_slippage. |
| **Becoming exit liquidity** | **High** | **High** | Random copy delay (5-120s). Price improvement rule. Skip if price moved beyond threshold. |
| **Liquidity traps (can't exit at stop)** | **Medium** | **High** | Spread gate. Depth gate. Size tiny relative to book. Halt if depth drops. |
| **Correlation blowup (same bet many markets)** | **Medium** | **High** | Theme-level exposure cap (≤3% bankroll per theme). |
| **Resolution/oracle disputes** | **Medium** | **Medium** | Rule clarity gate. Skip subjective/ambiguous markets. Smaller size when unclear. |
| **Adversarial wallets (bait/pump)** | **Medium** | **High** | Random delay. Price improvement. Skip signal accounts with sudden public attention. |
| **Automation bugs (infinite loop, wrong side)** | **Low** | **Critical** | Error breaker (3 errors in 2 min → halt). Kill switch. Trade count cap. |

---

## 8) Interaction with `trading-evaluator-guidance` Skill

The guidance skill reads this document and all collected data to produce recommendations. It must:

1. **Check current phase** — what phase are we in? Are exit criteria met?
2. **Run data quality checks** — are we safe to evaluate?
3. **Compute evaluation metrics** — for whichever phase we're in
4. **Apply decision rules** — should we advance, pivot, or kill?
5. **Produce prioritized next actions** — what should the human/agent do next?
6. **Explain with numbers** — every recommendation backed by specific metrics

The skill should be run:
- At the start of every session
- After any significant data collection milestone
- After any parameter change
- When the human asks "what should I do next?"
