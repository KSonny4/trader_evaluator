# Condense CLAUDE.md Implementation Plan

> **🗄️ ARCHIVED — EXECUTED**
>
> Plan completed: CLAUDE.md condensed from 413 → ~150 lines (358 lines removed). Created docs/REFERENCE.md (241 lines) with moved sections: competitive analysis, project structure, DB tables, persona taxonomy, data replay architecture, on-chain contracts.
>
> **Result:** ~65% reduction in per-session context load. CLAUDE.md now contains only essentials: overview, pipeline, APIs, build commands, dev workflow, testing rules.

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce CLAUDE.md from 414 lines / 27KB to ~150 lines / ~9KB by moving reference material to `docs/REFERENCE.md`, cutting context load by ~65%.

**Architecture:** Split CLAUDE.md into two files — a lean project-essential file loaded every session, and a reference document loaded on-demand. No code changes.

**Tech Stack:** Markdown only.

---

### Task 1: Create `docs/REFERENCE.md` with moved sections

**Files:**
- Create: `docs/REFERENCE.md`

**Step 1:** Create `docs/REFERENCE.md` with these sections moved verbatim from CLAUDE.md:

1. **Technical stack** table (14 lines) — as-is
2. **Project structure (target)** tree (53 lines) — update `jobs.rs` entry to `jobs/` directory:
   ```
   jobs/
     mod.rs              # Re-exports submodules
     fetcher_traits.rs   # API fetcher trait definitions
     fetcher_impls.rs    # PolymarketClient trait implementations
     ingestion_jobs.rs   # Trade/activity/position/holder ingestion
     pipeline_jobs.rs    # Market scoring, wallet discovery/scoring, paper tick
     maintenance.rs      # WAL checkpoint
   ```
3. **Database tables** table (19 lines) — as-is
4. **Reference implementations** (9 lines) — as-is
5. **Data saving and replay** (38 lines) — as-is
6. **Wallet persona taxonomy** (27 lines) — as-is, add note: "Authoritative source: `docs/STRATEGY_BIBLE.md`"
7. **Environment variables** (16 lines) — as-is
8. **Competitive landscape** (16 lines) — as-is
9. **Key patterns extracted from competitive analysis** (37 lines) — as-is (includes WScore formula, PnL decomposition, trade detection, on-chain contracts)

Add file header:
```markdown
# Trader Evaluator — Reference

> Moved from CLAUDE.md to reduce per-session context. Load on-demand when needed.
```

**Step 2:** Commit
```bash
git add docs/REFERENCE.md
git commit -m "docs: create REFERENCE.md with moved sections from CLAUDE.md"
```

---

### Task 2: Rewrite CLAUDE.md as lean context

**Files:**
- Modify: `CLAUDE.md` (rewrite, keeping only essentials)

**Step 1:** Rewrite CLAUDE.md with these sections only (~150 lines target):

1. **Project overview** — keep as-is (3 lines)
2. **The pipeline** — keep as-is (diagram + Grafana note, 10 lines)
3. **Key documents** — keep table as-is + ADD row for `docs/REFERENCE.md`: "Technical stack, project structure, DB tables, competitive analysis, data replay architecture, persona taxonomy, on-chain contracts"
4. **Polymarket APIs we use** — keep Data API table (7 lines) + Gamma API table (4 lines). DROP "Key data types" subsection (discoverable from code)
5. **Domain concepts** — keep as-is (7 lines: MScore, WScore, paper copy, discovery source, mirror strategy, quartic fee)
6. **Evaluation phases** — keep phase table as-is (10 lines)
7. **Build / test / run** — CONDENSE to Makefile shortcuts only:
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
8. **Development workflow** — CONDENSE to bullet list (12 lines):
   - Never commit to `main`. Feature branches + PRs only.
   - Always use git worktrees: `make worktree NAME=<name>`
   - TDD always (red-green-refactor). No production code without failing test.
   - CI must pass: test + clippy + fmt + coverage.
   - Agents NEVER push to `main` or merge PRs.
   - After merge: `make worktree-clean NAME=<name>`
   - First-time: `make setup-hooks`
9. **Testing rules** — CONDENSE from 42 to 8 lines:
   - Test against real Polymarket API data. No mocking API shapes.
   - Integration tests hit real APIs; `tests/fixtures/` for offline replay.
   - Minimum 70% coverage (target 80%), enforced in CI.
   - Every module: happy path + error handling + edge cases + boundary conditions + state transitions.
   - Test naming: `test_<function>_<scenario>`
10. **Reference** — pointer section (3 lines):
    > See `docs/REFERENCE.md` for: technical stack, project structure, database tables, data replay architecture, wallet persona taxonomy, competitive analysis (8 projects), extracted patterns (WScore formula, PnL decomposition, trade detection), on-chain contract addresses, environment variables.

**Step 2:** Verify line count
```bash
wc -l CLAUDE.md  # Target: ~150 lines
```

**Step 3:** Commit
```bash
git add CLAUDE.md
git commit -m "docs: condense CLAUDE.md from 414 to ~150 lines

Move reference material (competitive analysis, project structure, DB tables,
persona taxonomy, data replay architecture) to docs/REFERENCE.md.
Keep only per-session essentials: overview, pipeline, APIs, build commands,
dev workflow rules, testing rules."
```

---

### Task 3: Verify nothing was lost

**Step 1:** Spot-check key terms exist in one of the two files:
```bash
grep -l "polybot" CLAUDE.md docs/REFERENCE.md          # competitive analysis
grep -l "WScore" CLAUDE.md docs/REFERENCE.md            # scoring formula
grep -l "raw_api_responses" CLAUDE.md docs/REFERENCE.md # deprecated table note
grep -l "CTF Exchange" CLAUDE.md docs/REFERENCE.md      # on-chain contracts
grep -l "Informed Specialist" CLAUDE.md docs/REFERENCE.md # persona
grep -l "WAL mode" CLAUDE.md docs/REFERENCE.md          # data architecture
grep -l "quartic" CLAUDE.md docs/REFERENCE.md            # taker fee
grep -l "rust_decimal" CLAUDE.md docs/REFERENCE.md       # tech stack
```

Each term should appear in at least one file.

**Step 2:** Verify CLAUDE.md references docs/REFERENCE.md:
```bash
grep "REFERENCE.md" CLAUDE.md  # Must find pointer
```

**Step 3:** Verify line count reduction:
```bash
wc -l CLAUDE.md docs/REFERENCE.md
# Expected: CLAUDE.md ~150 lines, REFERENCE.md ~260 lines
```
