# Trader Evaluator — Reference

> Moved from CLAUDE.md to reduce per-session context. Load on-demand when needed.

## Technical stack

| Component | Choice | Rationale |
|-----------|--------|-----------|
| Language | Rust | Type safety for money. Performance. Compiled to static binaries. |
| Async runtime | Tokio | Non-blocking I/O, multiplexing via `tokio::select!`. |
| Database | SQLite (rusqlite, bundled) | Zero-dep deployment. Append-only data recording. |
| HTTP client | reqwest | Async HTTP with connection pooling. |
| Precision math | rust_decimal | Never floats for money. |
| Config | TOML (toml + serde) | Simple, strongly-typed configuration. |
| Logging | tracing + tracing-subscriber (JSON) | Structured JSON logs to stdout for Grafana Loki. |
| Metrics | metrics + metrics-exporter-prometheus | Prometheus exposition format on port 9094. |
| Deployment | AWS t3.micro, systemd, cross-compile musl | Zero-dependency Linux binaries. |

## Project structure (target)

```
trader_evaluator/
  CLAUDE.md                         # This file
  Cargo.toml                        # Workspace root
  config/
    default.toml                    # All configuration
  crates/
    common/                         # Shared types, config, DB, API client
      src/
        lib.rs
        config.rs                   # TOML config deserialization
        db.rs                       # SQLite schema, migrations, queries
        types.rs                    # API response types, enums
        polymarket.rs               # Polymarket Data + Gamma API client
    evaluator/                      # Main binary
      src/
        main.rs                     # Entry point, Tokio runtime
        scheduler.rs                # Periodic job scheduling
        market_scoring.rs           # MScore computation
        wallet_discovery.rs         # Wallet extraction + watchlist
        ingestion.rs                # Trade/activity/position/holder polling
        paper_trading.rs            # Mirror strategy + risk engine
        wallet_scoring.rs           # WScore computation
        metrics.rs                  # Prometheus metric definitions
        cli.rs                      # Subcommands for inspection
        jobs/
          mod.rs                    # Re-exports submodules
          fetcher_traits.rs         # API fetcher trait definitions
          fetcher_impls.rs          # PolymarketClient trait implementations
          ingestion_jobs.rs         # Trade/activity/position/holder ingestion
          pipeline_jobs.rs          # Market scoring, wallet discovery/scoring, paper tick
          maintenance.rs            # WAL checkpoint
    web/                            # Dashboard web server (htmx + cookie-based auth). With cloudflared/other proxy: forward Cookie and Host.
      src/
        main.rs                     # Axum HTTP server on port 8080
        queries.rs                  # Dashboard SQL queries
        models.rs                   # View models (funnel, wallet, market)
  deploy/
    deploy.sh                       # Cross-compile + upload
    purge-raw.sh                    # One-time: purge raw_api_responses on server
    setup-evaluator.sh              # Server setup
    systemd/
      evaluator.service             # Systemd unit file
  docs/
    STRATEGY_BIBLE.md               # Governing strategy document
    EVALUATION_STRATEGY.md          # Phase gates + decision rules
    prd.txt                         # Product requirements
    on_risk2.txt                    # Risk framework
    plans/
      MASTER_STRATEGY_IMPLEMENTATION_PLAN.md  # Current plan: 24 tasks
      2026-02-08-wallet-evaluator-mvp.md  # Original MVP plan (mostly done)
      2026-02-08-evaluator-frontend-dashboard.md  # Dashboard plan
  tests/
    fixtures/                       # Real API response samples
  data/                             # SQLite database (gitignored)
```

## Database tables

| Table | Purpose | Key columns |
|-------|---------|-------------|
| `markets` | Market metadata from Gamma API | condition_id (PK), title, liquidity, volume |
| `wallets` | Discovered wallets + watchlist state | proxy_wallet (PK), discovered_from, is_active |
| `trades_raw` | Append-only trade history | proxy_wallet, condition_id, size, price, timestamp |
| `activity_raw` | Append-only activity timeline | proxy_wallet, activity_type, timestamp |
| `positions_snapshots` | Periodic position state | proxy_wallet, condition_id, size, cash_pnl |
| `holders_snapshots` | Periodic holder rankings | condition_id, proxy_wallet, amount |
| `market_scores_daily` | MScore + factor breakdown | condition_id, mscore, rank |
| `wallet_features_daily` | Derived features per window | proxy_wallet, window_days, trade_count, total_pnl |
| `paper_trades` | Simulated copy trades | proxy_wallet, strategy, entry_price, pnl, status |
| `paper_positions` | Current paper portfolio state | proxy_wallet, strategy, total_size_usdc |
| `wallet_personas` | Persona classification per wallet | proxy_wallet, persona, confidence, feature_values_json, classified_at |
| `wallet_exclusions` | Why a wallet was excluded | proxy_wallet, reason, metric_value, threshold, excluded_at |
| `wallet_scores_daily` | WScore + factor breakdown | proxy_wallet, wscore, recommended_follow_mode |
| `raw_api_responses` | **DEPRECATED** — schema exists but no code writes to it. Was removed to fix storage crisis (3.7GB in 28h). Parsed data in per-row `raw_json` columns in trades_raw, activity_raw, etc. is sufficient. |

## Canonical dashboard semantics

### Followable now (canonical rule)

`followable_now` remains the runtime gating rule for active mirroring:
- `wallets.is_active = 1`
- wallet has latest persona row in `wallet_personas`
- latest exclusion is missing, or strictly older than latest persona timestamp

### Unified funnel stages

Dashboard funnel uses one canonical sequence:
1. `Markets fetched`
2. `Markets scored (ever)`
3. `Wallets discovered`
4. `Stage 1 passed (ever)`
5. `Stage 2 classified (ever)`
6. `Paper traded (ever)`
7. `Follow-worthy (ever)`
8. `Human approval` (placeholder `0`)
9. `Live` (placeholder `0`)

Each stage shows `processed/total`. For market→wallet transitions, the UI shows a `unit change` marker to make denominator changes explicit.

### Ever/to-date semantics

Unified funnel stages are rendered as cumulative `ever/to-date` counts (historical context), not only current-day snapshots.

## Paper sizing behavior

Paper mirror sizing defaults to proportional mode:
- `their_size_usd = trades_raw.size * trades_raw.price`
- `our_size_usd = their_size_usd * (paper_trading.bankroll_usd / paper_trading.mirror_default_their_bankroll_usd)`

Fallback behavior:
- if proportional mode is disabled, use flat `paper_trading.per_trade_size_usd`
- if source size/price is missing or invalid, use flat `paper_trading.per_trade_size_usd`
- if `per_trade_size_usd <= 0`, fallback to legacy `position_size_usdc`

## Reference implementations

See `docs/ARCHITECTURE.md` for runtime and orchestration (current and target).

This project applies proven Polymarket patterns from production systems. Key architectural patterns include:
- SQLite append-only storage with WAL mode for concurrent reads/writes
- Tokio-based async jobs with configurable scheduling
- Prometheus metrics for observability
- Cross-compiled musl binaries for zero-dependency deployment
- Systemd service management on AWS t3.micro

## Durability and recovery

The implementation is **durable** (each unit of work is committed in a transaction) and **recoverable** (after a sudden kill, the next run catches up).

- **Paper trading:** Each mirror decision runs in a single SQLite transaction (insert `paper_trades` + upsert `paper_positions`). If the process is killed mid-transaction, that transaction is rolled back; at most one trade is lost until the next run. Processing is idempotent per source trade (keyed by `triggered_by_trade_id`).
- **Ingestion:** `trades_raw` and `activity_raw` use `INSERT OR IGNORE` with UNIQUE constraints, so re-running after a kill does not duplicate rows. Positions and holders snapshots are append-only; re-run may add duplicate snapshot rows for the same time (acceptable).
- **Startup recovery:** Before the scheduler starts, the evaluator runs **recovery** once: it processes any unprocessed `trades_raw` into paper trades (same logic as the periodic paper_tick job). So work that was in progress when the process was killed is completed on the next boot. Metric: `evaluator_recovery_paper_trades_total`.

## Data saving and replay

**Save parsed data per-row. No separate raw response table.** We need the ability to replay following any account on any market after the fact — to tune risk management, test alternative strategies, and prove whether we could have profited.

Architecture:
- **WAL mode** (`PRAGMA journal_mode=WAL`) for concurrent reads during writes
- **Periodic WAL checkpoint** (every 5 min, TRUNCATE mode) to prevent WAL bloat
- **Git SHA on every row** for traceability (which code version produced this data)
- **Raw JSON stored per-row** in `raw_json` columns on individual tables — NOT in a separate `raw_api_responses` table (that approach caused a 3.7 GB storage crisis in 28h and was removed)
- **Early-stop pagination** — ingestion queries `MAX(timestamp)` first and stops fetching when all trades on a page are already known

**What to save (append-only):**

| Table | What | Why |
|-------|------|-----|
| `trades_raw` | Every trade from every tracked wallet (+ `raw_json`) | Replay: reconstruct what any wallet did |
| `activity_raw` | Every activity event per wallet (+ `raw_json`) | Catch neg-risk conversions the trades API misses |
| `positions_snapshots` | Position state per wallet, polled regularly | Replay: reconstruct portfolio at any point in time |
| `holders_snapshots` | Holder rankings per market, daily | Track how wallet rankings shift over time |
| `paper_trades` | Every paper trade decision (including skips with reason) | Replay: see exactly what we would have done and why |

**Future tables (not yet implemented):**

| Table | What | Why |
|-------|------|-----|
| `book_snapshots` | Best bid/ask + depth at time of each copy decision | Replay: compute realistic slippage and fill probability |
| `paper_events` | Every risk gate check, circuit breaker trigger, skip reason | Replay: tune risk parameters retroactively |
| `follower_slippage` | Per-wallet: our entry vs their entry, per trade | The critical metric — does copying actually work? |

**Replay capability:**
- Given a wallet address and a time range, we must be able to reconstruct:
  1. What trades they made (from `trades_raw`)
  2. What the market looked like when they traded (from `book_snapshots` — future)
  3. What our paper engine would have done (from `paper_trades`, `paper_events` — future)
  4. What the actual outcome was (from positions/settlement data)
- This lets us ask: "If we had followed wallet X on market Y with risk config Z, would we have made money?"
- **Every parameter change should be testable against historical data before going live**

## Wallet persona taxonomy

> Authoritative source: `docs/STRATEGY_BIBLE.md`

Every wallet is classified into a persona before paper-trading. This is the gatekeeper — only followable personas advance.

| Persona | Follow? | Key signal | Follow mode |
|---------|---------|-----------|-------------|
| **Informed Specialist** | YES — primary target | Few markets, high win rate, enters before moves | Mirror with delay |
| **Consistent Generalist** | YES | Many markets, steady returns, low drawdown | Mirror |
| **Patient Accumulator** | YES — slow | Large positions, long holds, few trades | Delay (24h+) |
| **Execution Master** | NO | >70% PnL from execution edge, not direction | Unreplicable fills |
| **Tail Risk Seller** | NO | 80%+ win rate, occasional massive blowup | Will destroy you |
| **Noise Trader** | NO | High churn, no statistical edge | No signal |
| **Sniper/Insider** | AVOID | New wallet, suspicious timing, clustered entries | Adversarial |
| **News Sniper** | AVOID | Ultra-short edge, bursty timing | Not copyable with delay |
| **Liquidity Provider / Market Maker** | AVOID | Two-sided flow, execution edge | Fill/latency dependent |
| **Jackpot Gambler** | AVOID | PnL concentrated in few trades | Not stable / not repeatable |
| **Bot Swarm / Micro-trader** | AVOID | Extreme frequency, micro sizing | Infra-dependent |
| **Sybil Cluster** | AVOID | Correlated trades, shared funding chain | Fake signal |

**Persona traits (stored separately):**
- `TOPIC_LANE=<category>`: domain specialist lane (copy only inside that lane).
- `BONDER=1`: high-probability grinder (often copyable at longer delays).
- `WHALE=1`: large sizing / slow accumulation (model impact carefully).

**Obscurity bonus:** Wallets NOT on public leaderboards get 1.2x WScore multiplier (fewer copiers = less front-running = better fills).

**Wallet age:** **Wallet age is a Stage 1 gate:** Wallets < 30 days old are **excluded** (hard filter, not tracked for paper trading). 30-90 days get 0.8x trust multiplier. 90-365 days are normal confidence (1.0x). >365 days get 1.1x trust bonus. Age gates are applied before persona classification.

**Continuous re-evaluation:** Personas are re-classified weekly. Wallets can move between personas as behavior changes (e.g., a Consistent Generalist who starts tail-risk-selling gets reclassified and dropped).

**Funnel:** Discover hundreds of wallets → classify all → track all → paper-trade only the best ~5-15 followable personas → rank those → follow the top handful with real money.

Detection: rule-based SQL on `wallet_features_daily` first, ML classifier later once we have labeled outcomes.

Full taxonomy with thresholds in `docs/EVALUATION_STRATEGY.md` Phase 2.

## Environment variables

```bash
# None required for V1 — all Polymarket APIs are public read-only
# Future (real execution):
# POLYMARKET_PRIVATE_KEY
# POLYMARKET_API_KEY

# Observability (Grafana Cloud):
# GRAFANA_CLOUD_PROM_URL
# GRAFANA_CLOUD_PROM_USER
# GRAFANA_CLOUD_API_KEY
# GRAFANA_CLOUD_LOKI_URL
# GRAFANA_CLOUD_LOKI_USER
# GRAFANA_CLOUD_TEMPO_URL
# GRAFANA_CLOUD_TEMPO_USER
```

See `docs/OBSERVABILITY.md` for the full metrics/logs/traces wiring and verification steps.

## Competitive landscape (reference projects analyzed in depth)

All linked repos were cloned/fetched and analyzed at source-code level. Key findings:

| Project | Stack | What it does | Key lessons for us |
|---------|-------|-------------|-------------------|
| **polymarket-intelligence** | Python/FastAPI, SQLite, httpx | Real-time dashboard: top holders, whale tracking, user analytics, AI debate | Hybrid cursor/offset pagination for positions API. Concurrent wallet enrichment with `asyncio.Semaphore(10)`. Field name fallback chains (Polymarket returns inconsistent names across endpoints). IPv4 monkey-patch for DNS. |
| **polybot** | Java 21/Spring Boot, ClickHouse, Kafka | Reverse-engineer any trader's strategy from 44k+ trades | **PnL decomposition**: `actual = directional + execution` (90% of profit was execution edge, not prediction). Execution classification: MAKER_LIKE/TAKER_LIKE/INSIDE/OUTSIDE. Complete-set detection: `edge = 1 - (avg_up_price + avg_down_price)`. Monte Carlo with block bootstrap (20k iterations, block=50). Profile scraping via Next.js `__NEXT_DATA__`. |
| **polymarket-trade-tracker** | Python/Flask, SQLite, requests | Per-market trade analysis with on-chain source classification | Multi-API fallback: trades → activity → on-chain. On-chain source classification from tx receipts: direct/neg-risk/split/merge/transfer/redeem. Batch RPC (10 tx/call) for maker/taker detection. Contract addresses + event topic registry. |
| **polymarket-copy-trading-bot** | Python asyncio + Node.js v3 | Position-based copy trading with TP/SL | Position-based trade detection: poll `/positions?user=` every 4s, diff snapshots. 2-cent slippage buffer on limit orders. WebSocket auto-reconnect with re-subscription. EIP-712 + HMAC-SHA256 dual auth. |
| **polyterm** | Python/click/rich, SQLite | Terminal wallet tracker with risk scoring | Wash trade detection (5 indicators: volume/liquidity ratio, trader concentration, size uniformity, side balance, volume discrepancy). Market risk scoring A-F (resolution clarity, liquidity, time, volume quality, spread, category). Insider risk score 0-100 (wallet age, position size, win rate anomaly, trading pattern). |
| **polymarket-insider-tracker** | Python/asyncio, Postgres, Redis, scikit-learn | Production insider detection pipeline | DBSCAN clustering for sniper detection (coordinated wallets entering within 5 min). Funding chain tracing (USDC transfers backwards, 3 hops, stop at known CEX/bridge). Known entity registry (Binance, Coinbase, Polygon Bridge, etc). Composite risk scorer with multi-signal bonus (1.2x for 2 signals, 1.3x for 3+). Redis deduplication with 1h TTL. |
| **predictfolio.com** | SaaS (closed source) | Wallet analytics: PnL, volume, win rate, leaderboard | Multi-timeframe PnL (1D/1W/1M/YTD/1Y/MAX). "Current" vs "Average" toggle. 180-day PnL projection. Leaderboard = PnL + Markets Traded + Win/Loss Ratio. Coverage: 1M+ users, 30K+ markets, 5 years. |
| **Copy-Trade Docs** | SaaS documentation | Commercial copy-trading platform ($99-499/mo) | **Weighted composite trader score**: Win Rate 30% + ROI 25% + Consistency 20% + Volume 15% + Risk Score 10%. Tier system: Elite (70%+ WR, 50%+ ROI, 6mo), Professional (60%+, 30%+, 3mo), Rising Star (55%+, 15%+, 1mo). 4-phase pipeline: Detect (<50ms) → Analyze → Validate → Execute. |
| **arb-copy-bot** | TypeScript + Rust stubs | Combined arbitrage + selective copy-trading | **Position-based trade detection as primary** (poll every 1s, diff snapshots). 4-method fallback chain: positions → on-chain → activity → trades. Arb detection: `yesPrice + noPrice < 0.99` (after 1% fee). Per-wallet config: minWinRate, maxPositionSizeUsd, positionSizeMultiplier, requireArbSignal gate. |

## Key patterns extracted from competitive analysis

### Trade detection (ranked by reliability)
1. **Position diffing** (PRIMARY) — poll `/positions?user=` every 1-5s, diff snapshots → new/changed/closed positions
2. **On-chain events** — `OrderFilled`, `PositionSplit`, `PositionsConverted` on CTF contract
3. **Activity API** — `/activity?user=` catches neg-risk conversions the trades API misses
4. **Trades API** — `/trades?user=` for historical trade logs with pagination

### Wallet scoring formula (synthesized from all sources)
```
WScore = ( 0.30 * edge_score        # ROI + win_rate (from paper results)
         + 0.25 * consistency_score  # Sharpe ratio proxy: mean/stddev of per-trade returns
         + 0.20 * market_skill_score # unique_markets / trades across different domains
         + 0.15 * timing_skill_score # avg time between source entry and our copy entry
         + 0.10 * behavior_quality   # low churn, no wash patterns, stable frequency
         ) * trust_multiplier        # 0.5 (<30d), 0.8 (30-90d), 1.0 (90-365d), 1.1 (>365d)
           * obscurity_bonus         # 1.2x if NOT on public leaderboard
```

### PnL decomposition (from polybot)
```
actual_pnl    = Σ (settle_price - entry_price) * size    # total
directional   = Σ (settle_price - mid_at_entry) * size   # would you profit at mid?
execution     = Σ (mid_at_entry - entry_price) * size     # edge from buying below mid
# actual = directional + execution
```

### Polygon on-chain contracts
```
CTF (ConditionalTokens):     0x4D97DCd97eC945f40cF65F87097ACe5EA0476045
CTF Exchange:                0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E
NegRisk Adapter:             0xd91E80cF2E7be2e162c6513ceD06f1dD0dA35296
NegRisk CTF Exchange:        0xC5d563A36AE78145C45a50134d48A1215220f80a
Proxy Wallet Factory:        0x56C79347e95530c01A2FC76E732f9566dA16E113
```

**Our differentiation:** We are the **closed loop** — market selection → wallet discovery → long-term tracking → paper copy → ranking with evidence. Most projects do only one piece.
