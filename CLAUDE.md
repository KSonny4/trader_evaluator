# CLAUDE.md

## Project overview

Polymarket wallet discovery and paper copy-trading evaluation system. Discovers "follow-worthy" markets, extracts wallets trading them, tracks those wallets long-term, runs risk-managed paper-copy portfolios, and ranks "who to follow" with evidence. Written in Rust. Deployed to AWS t3.micro.

## The pipeline

> **Grafana dashboard:** Build a funnel view showing: markets scored → markets selected → wallets discovered → wallets tracked → wallets paper-copied → wallets ranked. Show counts at each stage and drop-off rates.

```
Markets (Gamma API) → MScore ranking → Top-20 markets
    ↓
Wallet Discovery (Data API: holders + traders) → Watchlist
    ↓
Long-Term Tracking (trades, activity, positions, holders snapshots)
    ↓
Paper Copy Engine (mirror trades with risk caps)
    ↓
WScore Ranking → "Who to Follow" with evidence
```

## Key documents

| Document | Purpose |
|----------|---------|
| `docs/STRATEGY_BIBLE.md` | **Governing doc** — the single source of truth for what the system does, why, and how. Persona taxonomy, risk levels, copy fidelity, WScore/MScore formulas. If code contradicts this, the code is wrong. |
| `docs/plans/2026-02-08-strategy-enforcement.md` | **Current implementation plan** — 24 tasks to bridge Strategy Bible to code. TDD, bite-sized. |
| `docs/EVALUATION_STRATEGY.md` | Phase gates, evaluation metrics, decision rules. Superseded by Strategy Bible for strategic decisions, but still valid for phase progression. |
| `docs/prd.txt` | Full product requirements — goals, data sources, data model, acceptance criteria |
| `docs/on_risk2.txt` | Risk management framework — supplementary to Strategy Bible §7 |
| `docs/plans/2026-02-08-wallet-evaluator-mvp.md` | Original MVP plan — 15 tasks, mostly complete |
| `docs/plans/2026-02-08-evaluator-frontend-dashboard.md` | Dashboard implementation plan |
| `docs/inspiration.txt` | Reference projects and links |

## Polymarket APIs we use

### Data API (`https://data-api.polymarket.com`) — PRIMARY

| Endpoint | Purpose | Key params | Rate limit |
|----------|---------|------------|------------|
| `GET /trades` | Trade history by user or market | `user`, `market`, `limit` (max 10000), `offset` | Undocumented — use 200ms delay |
| `GET /holders` | Top holders for markets | `market` (condition IDs), `limit` (max 20) | Undocumented |
| `GET /activity` | User activity timeline | `user` (required), `type`, `limit` (max 500), `offset` | Undocumented |
| `GET /positions` | Current positions for user | `user` (required), `limit` (max 500), `offset` | Undocumented |
| `GET /v1/leaderboard` | Trader rankings | `category`, `timePeriod`, `limit` (max 50), `offset` | Undocumented |

### Gamma API (`https://gamma-api.polymarket.com`) — Market discovery

| Endpoint | Purpose | Key params |
|----------|---------|------------|
| `GET /markets` | List/filter markets | `limit`, `offset`, `liquidity_num_min`, `volume_num_min`, `end_date_min/max`, `closed` |

### Key data types

- **condition_id**: `0x`-prefixed 64-hex string — unique market identifier
- **proxy_wallet**: `0x`-prefixed 40-hex address — user wallet identifier
- **asset**: token identifier for a specific outcome
- All APIs are **public** — no authentication required for read-only access
- No rate limits documented — we use conservative 200ms delays between calls

## Domain concepts

- **MScore**: Market Score [0, 1] — ranks markets by follow-worthiness using liquidity, volume, trade density, whale concentration, time-to-expiry
- **WScore**: Wallet Score [0, 1] — ranks wallets by copy-worthiness using edge, consistency, market skill, timing skill, behavior quality
- **Paper copy**: Simulated portfolio that mirrors a wallet's trades with risk caps and slippage
- **Discovery source**: How we found a wallet — HOLDER (top holders list), TRADER_RECENT (active in market), LEADERBOARD (global ranking)
- **Mirror strategy**: Copy trades same direction, proportional size, with configurable delay
- **Quartic taker fee**: `fee = price * 0.25 * (price * (1 - price))^2` — max ~1.56% at p=0.50. CONDITIONAL: applies only to crypto 15-min markets. All other markets have zero taker fee.

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
        jobs.rs                     # Scheduled job runners (ingestion, scoring, WAL checkpoint)
    web/                            # Dashboard web server (htmx + basic auth)
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
      2026-02-08-strategy-enforcement.md  # Current plan: 24 tasks
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

## Reference implementations

This project applies proven Polymarket patterns from production systems. Key architectural patterns include:
- SQLite append-only storage with WAL mode for concurrent reads/writes
- Tokio-based async jobs with configurable scheduling
- Prometheus metrics for observability
- Cross-compiled musl binaries for zero-dependency deployment
- Systemd service management on AWS t3.micro

## Build / test / run

```bash
cargo build --release              # Build all crates
cargo test --all                   # Run all tests
cargo clippy --all-targets -- -D warnings  # Lint
cargo fmt --check                  # Format check

cargo run -p evaluator             # Run main process (starts all jobs)
cargo run -p evaluator -- markets  # Show today's top markets
cargo run -p evaluator -- wallets  # Show watchlist
cargo run -p evaluator -- paper-pnl  # Show paper portfolio performance
cargo run -p evaluator -- rankings # Show WScore rankings

# Makefile shortcuts (preferred)
make test                          # cargo test + clippy + fmt check
make coverage                      # Run coverage locally (cargo-llvm-cov, 70% threshold)
make worktree NAME=foo             # Create .worktrees/foo on branch feature/foo
make worktree-clean NAME=foo       # Remove worktree and delete branch
make deploy                        # Test, cross-compile musl, upload, restart
make status                        # SSH: service status, DB size, disk, recent logs
make check                         # Verify DB schema matches code expectations
```

## Development workflow

**MANDATORY: Never commit directly to `main`.** All changes go through feature branches and pull requests.

**MANDATORY: Always use `superpowers` skills.**

### Branch & PR workflow (non-negotiable)
1. **Always use git worktrees** for feature work: `make worktree NAME=<feature-name>` — this creates `.worktrees/<feature-name>` on branch `feature/<feature-name>`. Never work directly in the main checkout for feature changes.
2. Implement with TDD (red-green-refactor)
3. Commit to the feature branch (never to `main`)
4. Push the branch: `git push -u origin feature/<name>`
5. Create a PR: `gh pr create`
6. CI must pass (cargo test + clippy + fmt + coverage — runs automatically via GitHub Actions)
7. Human reviews and merges the PR
8. **Claude/agents must NEVER push to `main` or merge PRs** — only the human merges
9. After merge, clean up: `make worktree-clean NAME=<feature-name>`

### Skill workflow
- **Before any work:** Run `evaluator-guidance` skill to check current phase and get recommendations
- **Before writing code:** Use `superpowers:writing-plans` or follow the existing plan
- **For any feature/bugfix:** Use `superpowers:test-driven-development` — TDD always
- **For debugging:** Use `superpowers:systematic-debugging`
- **Before claiming done:** Use `superpowers:verification-before-completion`
- **After implementation:** Use `superpowers:requesting-code-review`

All code changes must pass: `cargo test --all`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`

### First-time setup
```bash
make setup-hooks   # Install pre-push hook that blocks direct pushes to main
```

## Testing philosophy

**Test everything against real Polymarket API data. No exceptions.**

All Polymarket APIs are public and free. There is zero reason to mock or assume response shapes — connect to the real endpoints and validate against actual data. Every test should prove the code works against production reality, not against our imagination of what the API returns.

Rules:
- **Every module gets tests before or alongside implementation** — TDD, not afterthought
- **Integration tests hit real APIs** — use `tests/fixtures/` to cache responses for offline replay, but the first run always fetches live data
- **No hardcoded assumptions about API response fields** — Polymarket returns inconsistent field names across endpoints (e.g., `conditionId` vs `condition_id` vs `marketId`). Tests must verify our parsing handles the actual response, not a hand-crafted mock
- **Test edge cases from real data** — neg-risk markets, settled markets, markets with zero liquidity, wallets with thousands of trades, wallets with zero trades
- **Snapshot tests for scoring** — record real wallet data, compute MScore/WScore, assert stability across code changes
- **Data quality tests** — verify no gaps in ingestion, no duplicate trades, no null fields where we expect values
- **Run `cargo test --all` before every commit** — broken tests block everything

### Code coverage requirements

**Minimum 70% line coverage enforced in CI** (target: 80%). Coverage is measured with `cargo-llvm-cov`.

```bash
make coverage   # Run locally to check coverage before pushing
```

Coverage baseline (2026-02-08): 73.39% overall. Key gaps:
- `evaluator/src/main.rs` — 0% (entry point, hard to unit test)
- `common/src/polymarket.rs` — 27% (network code, needs integration tests)
- `evaluator/src/cli.rs` — 47% (needs CLI output tests)

Ramp-up plan: 70% now → 75% after Strategy Enforcement plan → 80% target.

### Test quality requirements (non-negotiable)

Happy-path-only tests are insufficient. Every module must include:

1. **Happy path** — normal inputs produce expected outputs
2. **Error handling** — invalid inputs, network failures, malformed data, DB errors
3. **Edge cases** — empty collections, zero values, maximum values, Unicode, special chars
4. **Boundary conditions** — exactly at thresholds (e.g., MScore = 0.0, MScore = 1.0, wallet age = 30 days exactly)
5. **State transitions** — what happens when data changes between calls (e.g., wallet goes from active to inactive)

Test naming convention: `test_<function>_<scenario>` (e.g., `test_mscore_zero_liquidity_scores_low`, `test_ingest_trades_gracefully_handles_http_400`)

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
| **Sybil Cluster** | AVOID | Correlated trades, shared funding chain | Fake signal |

**Obscurity bonus:** Wallets NOT on public leaderboards get 1.2x WScore multiplier (fewer copiers = less front-running = better fills).

**Wallet age:** **Wallet age is a Stage 1 gate:** Wallets < 30 days old are **excluded** (hard filter, not tracked for paper trading). 30-90 days get 0.8x trust multiplier. 90-365 days are normal confidence (1.0x). >365 days get 1.1x trust bonus. Age gates are applied before persona classification.

**Continuous re-evaluation:** Personas are re-classified weekly. Wallets can move between personas as behavior changes (e.g., a Consistent Generalist who starts tail-risk-selling gets reclassified and dropped).

**Funnel:** Discover hundreds of wallets → classify all → track all → paper-trade only the best ~5-15 followable personas → rank those → follow the top handful with real money.

Detection: rule-based SQL on `wallet_features_daily` first, ML classifier later once we have labeled outcomes.

Full taxonomy with thresholds in `docs/EVALUATION_STRATEGY.md` Phase 2.

## Evaluation strategy (summary)

The system progresses through 7 phases. **Never skip a phase.**

| Phase | Name | Key gate |
|-------|------|----------|
| 0 | Foundation | Build compiles, APIs reachable, tests pass |
| 1 | Market Discovery | Markets scored daily for 3+ days. Handle rotating markets (e.g. BTC 15m markets change slug every 15 min). |
| 2 | Wallet Discovery & Classification | All participants stored. Every wallet classified into persona. Only followable personas advance. |
| 3 | Long-Term Tracking | 7 days continuous ingestion, no data gaps |
| 4 | Paper Copy | Paper-trade only the best classified wallets (~5-10). Full risk management from `docs/on_risk2.txt`. |
| 5 | Wallet Ranking | WScore stable across 3 consecutive days |
| 6 | Production | Deployed on AWS, 72h no crashes, metrics in Grafana |

**Full details in `docs/EVALUATION_STRATEGY.md`.**

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
```

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
