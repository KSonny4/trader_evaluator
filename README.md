# trader_evaluator

Polymarket wallet discovery and paper copy-trading evaluation system. Discovers follow-worthy markets, extracts wallets trading them, tracks those wallets long-term, runs risk-managed paper-copy portfolios, and ranks "who to follow" with evidence.

## The Pipeline

```
Markets (Gamma API) --> MScore ranking --> Top-20 markets
    |
Wallet Discovery (Data API) --> Watchlist
    |
Long-Term Tracking (trades, activity, positions, holders)
    |
Paper Copy Engine (mirror trades with risk caps)
    |
WScore Ranking --> "Who to Follow" with evidence
```

## Quick Start

```bash
make setup-hooks                   # Install pre-push hook (blocks direct pushes to main)
cargo build --release              # Build all crates
make test                          # Run tests + clippy + fmt check
cargo run -p evaluator             # Start the main process (all scheduled jobs)
cargo run -p evaluator -- markets  # Show today's top scored markets
cargo run -p evaluator -- wallets  # Show discovered wallets
cargo run -p evaluator -- paper-pnl  # Show paper portfolio
cargo run -p evaluator -- rankings   # Show wallet rankings
```

## Developer Loop

### Per-Task Loop (Red-Green-Refactor)

For each task in the implementation plan:

```
1. READ the task
   Open docs/plans/MASTER_STRATEGY_IMPLEMENTATION_PLAN.md
   Find the next unchecked task, understand what it asks for

2. WRITE A FAILING TEST
   Add the test in the relevant crate's test module
   Run: cargo test --all
   Confirm it FAILS (red)

3. IMPLEMENT the minimal code to pass
   Edit the source files referenced in the task
   Run: cargo test --all
   Confirm it PASSES (green)

4. REFACTOR if needed
   Clean up, remove duplication
   Run: cargo test --all
   Confirm it still passes

5. VERIFY fully
   Run: make test
   This runs cargo test + clippy + fmt check — all three must pass

6. COMMIT to feature branch
   git add + git commit with a descriptive message
   Never directly to main — always a feature branch

7. REPEAT for next task
```

### Session Loop (Across Tasks)

```
START SESSION
  |
  +-- Check you're on a feature branch (not main)
  +-- Run `make test` to confirm clean baseline
  +-- Open the plan, find the next unchecked task
  |
  +-- DO the per-task loop (steps 1-6) for 1-3 tasks
  |
  +-- Push branch: git push -u origin feature/<name>
  +-- Create PR: gh pr create
  +-- Human reviews & merges
```

### Guardrails

- **Test first, always.** No production code without a failing test. If your test passes immediately, the test is wrong.
- **`make test` is the gate.** Nothing gets committed unless cargo test + clippy + fmt all pass.
- **Tasks are sequential (mostly).** Task 1 (config) must land before Task 5 (persona detectors) because later tasks depend on earlier ones. Some tasks are independent — e.g., tasks 5-11 (individual persona detectors) could be parallelized.
- **One task = one commit** (roughly). Keep commits small and reviewable.
- **Check the Strategy Bible** (`docs/STRATEGY_BIBLE.md`) when in doubt about thresholds or formulas. The plan references it, but the Bible is the source of truth.
- **Never skip phases.** The system progresses Phase 0 -> 1 -> 2 -> 3 -> 4 -> 5 -> 6. Use `make check-phase-N` to verify phase completion on the server.

### AI-Assisted Workflow

When using an AI coding agent, use `/evaluator-guidance` first — it reads `EVALUATION_STRATEGY.md` + your SQLite data and tells you which phase you're in and what to build next. Then execute tasks from the plan using the per-task TDD loop above.

After every meaningful deploy: `make deploy && make check-phase-N` to verify the DB is filling up as expected. `make status` anytime for a quick pipeline health overview.

## Key Documents

| Priority | Document | Purpose |
|----------|----------|---------|
| 1 | `docs/STRATEGY_BIBLE.md` | **The law.** All formulas, personas, risk rules, thresholds. If code contradicts it, fix the code. |
| 2 | `docs/plans/MASTER_STRATEGY_IMPLEMENTATION_PLAN.md` | **Current task list.** 24 TDD tasks bridging Strategy Bible to code. |
| 3 | `docs/EVALUATION_STRATEGY.md` | Phase gates, evaluation metrics, decision rules. |
| 4 | `CLAUDE.md` | Full project context: APIs, DB schema, domain concepts, competitive analysis. |
| 5 | `config/default.toml` | All configurable parameters: risk limits, scoring weights, ingestion intervals. |

## Project Structure

```
crates/
  common/       # Shared: config, DB schema, API client, types
  evaluator/    # Main binary: scheduler, scoring, discovery, ingestion, paper trading
  web/          # Dashboard: Axum + htmx, basic auth, port 8080
config/
  default.toml  # All configuration
deploy/         # Cross-compile, systemd units, server setup scripts
docs/           # Strategy Bible, evaluation strategy, implementation plans
```

## Deployment

```bash
make deploy    # Test + cross-compile musl + upload to AWS + restart systemd
make status    # SSH: service status, DB size, disk, recent logs
make check     # Verify DB schema on remote server
```

Runs on AWS t3.micro as a systemd service. Zero-dependency static musl binary.
