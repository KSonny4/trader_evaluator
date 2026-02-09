# Code Review Request: Task 12 — Persona Classification Orchestrator + Stage 2 Job

**Before requesting review:** Commit the Task 12 changes, then use the SHAs below.

```bash
# After committing Task 12:
BASE_SHA=$(git rev-parse HEAD~1)
HEAD_SHA=$(git rev-parse HEAD)
# Or vs main:
# BASE_SHA=$(git rev-parse origin/main)
# HEAD_SHA=$(git rev-parse HEAD)
```

---

## Code Review Agent (template)

You are reviewing code changes for production readiness.

**Your task:**
1. Review Task 12: Persona Classification Orchestrator + Stage 2 Job
2. Compare against MASTER_STRATEGY_IMPLEMENTATION_PLAN.md Task 12 and Strategy Bible
3. Check code quality, architecture, testing
4. Categorize issues by severity
5. Assess production readiness

### What Was Implemented

- **ClassificationResult** enum (Followable / Excluded / Unclassified) and **PersonaConfig** (test defaults + `from_personas()` for production).
- **classify_wallet(conn, features, wallet_age_days, config)** — exclusion-first pipeline (Sniper → Noise → Tail Risk), then followable personas (Informed Specialist → Consistent Generalist → Patient Accumulator); persists to `wallet_exclusions` or `wallet_personas`.
- **record_persona(conn, proxy_wallet, persona, confidence)** — INSERT OR REPLACE into wallet_personas.
- Three tests: `test_classify_wallet_informed_specialist`, `test_classify_wallet_excluded_noise_trader`, `test_classify_wallet_unclassified`.
- **run_persona_classification_once(db, cfg)** — loads active wallets, applies Stage 1 filter, computes features, runs classify_wallet; returns count of classified (followable or excluded).
- Scheduler: new job `persona_classification` (24h interval, run_immediately: false), spawn calling `run_persona_classification_once`.

### Requirements/Plan

- **Plan:** `docs/plans/MASTER_STRATEGY_IMPLEMENTATION_PLAN.md` — Task 12 (§ "Task 12: Persona Classification Orchestrator + Stage 2 Job").
- **Governing doc:** `docs/STRATEGY_BIBLE.md` (persona taxonomy, thresholds).
- Requirements: Single function classifies a wallet; exclusions first then followable; persist to DB; Stage 2 job runs classification for watchlist.

### Git Range to Review

**Base:** `{BASE_SHA}` (commit before Task 12)
**Head:** `{HEAD_SHA}` (commit with Task 12)

```bash
git diff --stat {BASE_SHA}..{HEAD_SHA}
git diff {BASE_SHA}..{HEAD_SHA}
```

### Review Checklist

(Use the checklist in `requesting-code-review/code-reviewer.md`: Code Quality, Architecture, Testing, Requirements, Production Readiness.)

### Output Format

(Use format from `requesting-code-review/code-reviewer.md`: Strengths, Issues by severity, Recommendations, Assessment.)
