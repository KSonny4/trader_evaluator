# CLAUDE.md

## Project overview

Polymarket wallet discovery and paper copy-trading evaluation system. Discovers "follow-worthy" markets, extracts wallets trading them, tracks those wallets long-term, runs risk-managed paper-copy portfolios, and ranks "who to follow" with evidence. Written in Rust. Deployed to AWS t3.micro.

## The pipeline

> **Grafana dashboard:** Build a funnel view showing: events scored → events selected → wallets discovered → wallets tracked → wallets paper-copied → wallets ranked. Show counts at each stage and drop-off rates.

```
Markets (Gamma API) → MScore + EScore ranking → Top-50 events
    ↓
Wallet Discovery (Data API: holders + traders) → Watchlist
    ↓
Long-Term Tracking (trades, activity, positions, holders snapshots)
    ↓
Paper Trading (mirror trades with risk caps)
    ↓
WScore Ranking → "Who to Follow" with evidence
```

## Key documents

| Document | Purpose |
|----------|---------|
| `docs/STRATEGY_BIBLE.md` | **Governing doc** — the single source of truth for what the system does, why, and how. Persona taxonomy, risk levels, copy fidelity, WScore/MScore formulas. If code contradicts this, the code is wrong. |
| `docs/plans/MASTER_STRATEGY_IMPLEMENTATION_PLAN.md` | **Current implementation plan** — 24 tasks to bridge Strategy Bible to code. TDD, bite-sized. |
| `docs/EVALUATION_STRATEGY.md` | Phase gates, evaluation metrics, decision rules. Superseded by Strategy Bible for strategic decisions, but still valid for phase progression. |
| `docs/prd.txt` | Full product requirements — goals, data sources, data model, acceptance criteria |
| `docs/on_risk2.txt` | Risk management framework — supplementary to Strategy Bible §7 |
| `docs/plans/2026-02-08-wallet-evaluator-mvp.md` | Original MVP plan — 15 tasks, mostly complete |
| `docs/plans/2026-02-08-evaluator-frontend-dashboard.md` | Dashboard implementation plan |
| `docs/inspiration.txt` | Reference projects and links |
| `docs/REFERENCE.md` | Technical stack, project structure, DB tables, competitive analysis, data replay architecture, persona taxonomy, on-chain contracts |
| `docs/ARCHITECTURE.md` | Runtime and orchestration: current vs target (scheduler, events, queue, Tempo, saga) |

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

## Domain concepts

- **MScore**: Market Score [0, 1] — ranks individual markets by follow-worthiness using liquidity, volume, trade density, whale concentration, time-to-expiry
- **EScore**: Event Score [0, 1] — max(MScore) over all markets in an event; used to rank events (top-50 for discovery)
- **WScore**: Wallet Score [0, 1] — ranks wallets by copy-worthiness using edge, consistency, market skill, timing skill, behavior quality
- **Paper copy**: Simulated portfolio that mirrors a wallet's trades with risk caps and slippage
- **Discovery source**: How we found a wallet — HOLDER (top holders list), TRADER_RECENT (active in market), LEADERBOARD (global ranking)
- **Mirror strategy**: Copy trades same direction, proportional size, with configurable delay
- **Quartic taker fee**: `fee = price * 0.25 * (price * (1 - price))^2` — max ~1.56% at p=0.50. CONDITIONAL: applies only to crypto 15-min markets. All other markets have zero taker fee.

## Evaluation phases

The system progresses through 7 phases. **Never skip a phase.**

| Phase | Name | Key gate |
|-------|------|----------|
| 0 | Foundation | Build compiles, APIs reachable, tests pass |
| 1 | Event Discovery | Events scored for 3+ days. Handle rotating markets (e.g. BTC 15m markets change slug every 15 min). |
| 2 | Wallet Discovery & Classification | All participants stored. Every wallet classified into persona. Only followable personas advance. |
| 3 | Long-Term Tracking | 7 days continuous ingestion, no data gaps |
| 4 | Paper Trading | Paper-trade only the best classified wallets (~5-10). Full risk management from `docs/on_risk2.txt`. |
| 5 | Wallet Ranking | WScore stable across 3 consecutive days |
| 6 | Production | Deployed on AWS, 72h no crashes, metrics in Grafana |

**Full details in `docs/EVALUATION_STRATEGY.md`.**

## Build / test / run

```bash
make test              # cargo test + clippy + fmt + file-length check
make coverage          # cargo-llvm-cov, 70% threshold
make deploy            # Test, cross-compile musl, upload, restart
make status            # SSH: service status, DB size, disk, recent logs
make worktree NAME=foo # Create .worktrees/foo on branch feature/foo
make worktree-clean NAME=foo
cargo run -p evaluator             # Run main process
cargo run -p evaluator -- markets  # CLI: top markets
cargo run -p evaluator -- wallets  # CLI: watchlist
cargo run -p evaluator -- rankings # CLI: WScore rankings
```

**Dashboard (port 8080):** When auth is configured, the dashboard uses cookie-based session auth: SHA-256 token, constant-time password comparison, and CSRF protection (token in form + cookie).

## Development workflow

- **Never commit to `main`.** Feature branches + PRs only.
- Always use git worktrees: `make worktree NAME=<name>`
- TDD always (red-green-refactor). No production code without failing test.
- CI must pass: test + clippy + fmt + coverage.
- Agents NEVER push to `main` or merge PRs.
- After merge: `make worktree-clean NAME=<name>`
- First-time: `make setup-hooks`
- **Always use `superpowers` skills** (brainstorming, TDD, systematic-debugging, verification-before-completion, requesting-code-review, etc.)
- **NO PR WITHOUT CODE REVIEW SKILL** — dispatch code-reviewer subagent, use its output

## Testing rules

- Test against real Polymarket API data. No mocking API shapes.
- Integration tests hit real APIs; `tests/fixtures/` for offline replay.
- Minimum 70% coverage (target 80%), enforced in CI.
- Every module: happy path + error handling + edge cases + boundary conditions + state transitions.
- Test naming: `test_<function>_<scenario>`

## Reference

> See `docs/REFERENCE.md` for: technical stack, project structure, database tables, data replay architecture, wallet persona taxonomy, competitive analysis (8 projects), extracted patterns (WScore formula, PnL decomposition, trade detection), on-chain contract addresses, environment variables.
