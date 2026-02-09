---
name: evaluator-guidance
description: Use when deciding what to work on next in the trader_evaluator project, when evaluating if the pipeline is working, when checking phase progress, or when the human asks what to do next. Interacts with all collected data and EVALUATION_STRATEGY.md to produce prioritized, evidence-backed recommendations.
---

# Evaluator Guidance

## Overview

**The brain of the wallet evaluator pipeline.** This skill reads the current system state, collected data (SQLite), evaluation strategy document, and implementation plan to answer: "What phase are we in? Is it working? What should we do next?"

Every recommendation is backed by specific numbers from the data. No vague suggestions.

## When to Use

- At the start of every session on the trader_evaluator project
- After deploying new code or completing a plan task
- When the human asks "what should I do next?" or "is it working?"
- After a significant data collection milestone (e.g., 7 days of ingestion)
- When deciding whether to advance to the next phase
- When a component seems broken or underperforming

**When NOT to use:** For building code (use the implementation plan). For the existing `trading` project (use `trading-guidance`).

## The Guidance Process

### Step 1: Determine Current Phase

Read the evaluation strategy and check which phase we're in:

```bash
# Read the governing document
cat docs/EVALUATION_STRATEGY.md
```

Check what code/infrastructure exists:

```bash
# What crates exist?
ls crates/ 2>/dev/null

# Does the database exist?
ls data/ 2>/dev/null

# What's been committed?
git log --oneline -20 2>/dev/null
```

Map the current state to a phase:
- No code → Phase 0 (Foundation) — need to BUILD
- Code but no data → Phase 1/2 (Discovery) — need to COLLECT
- Data flowing but no paper trades → Phase 3/4 (Tracking/Paper) — need to EVALUATE
- Paper results but no rankings → Phase 5 (Ranking) — need to SCORE
- Everything deployed → Phase 6 (Production) — need to VALIDATE

### Step 2: Check Exit Criteria for Current Phase

Read the specific exit criteria from EVALUATION_STRATEGY.md for the current phase. For each criterion, check if it's met:

**Phase 0 checks:**
```bash
# Can we build?
cargo build 2>&1 | tail -5

# Do tests pass?
cargo test --all 2>&1 | tail -10

# Can we talk to Polymarket?
# (check for test fixtures or integration test results)
ls tests/fixtures/ 2>/dev/null
```

**Phase 1-2 checks (market scoring & wallet discovery):**
```bash
# Check SQLite for data
sqlite3 data/evaluator.db "
  SELECT 'markets_scored', COUNT(*) FROM market_scores_daily
  UNION ALL
  SELECT 'wallets_discovered', COUNT(*) FROM wallets
  UNION ALL
  SELECT 'top_market_score', ROUND(MAX(mscore), 3) FROM market_scores_daily
  WHERE score_date = date('now')
" 2>/dev/null
```

**Phase 3-4 checks (ingestion & paper trading):**
```bash
sqlite3 data/evaluator.db "
  SELECT 'trades_ingested', COUNT(*) FROM trades_raw
  UNION ALL
  SELECT 'wallets_tracked', COUNT(DISTINCT proxy_wallet) FROM trades_raw
  UNION ALL
  SELECT 'paper_trades', COUNT(*) FROM paper_trades
  UNION ALL
  SELECT 'paper_pnl', ROUND(SUM(pnl), 2) FROM paper_trades WHERE status != 'open'
  UNION ALL
  SELECT 'oldest_trade', MIN(datetime(timestamp, 'unixepoch')) FROM trades_raw
  UNION ALL
  SELECT 'newest_trade', MAX(datetime(timestamp, 'unixepoch')) FROM trades_raw
" 2>/dev/null
```

**Phase 5 checks (wallet scoring):**
```bash
sqlite3 data/evaluator.db "
  SELECT 'wallets_scored', COUNT(DISTINCT proxy_wallet) FROM wallet_scores_daily
  UNION ALL
  SELECT 'top_wscore_7d', ROUND(MAX(wscore), 3) FROM wallet_scores_daily WHERE window_days = 7
  UNION ALL
  SELECT 'top_wscore_30d', ROUND(MAX(wscore), 3) FROM wallet_scores_daily WHERE window_days = 30
  UNION ALL
  SELECT 'followworthy', COUNT(DISTINCT proxy_wallet) FROM wallet_scores_daily
    WHERE wscore > 0.6 AND window_days = 30
" 2>/dev/null
```

**Phase 6 checks (production):**
```bash
# Is it deployed and running?
ssh -i deploy/trading-bot.pem ubuntu@3.8.206.244 
  'systemctl is-active evaluator 2>/dev/null && echo "RUNNING" || echo "NOT DEPLOYED"'

# Check Prometheus metrics
curl -s localhost:9094/metrics 2>/dev/null | grep evaluator | head -20
```

### Step 3: Run Data Quality Checks

Before trusting any evaluation metrics, verify data quality (from EVALUATION_STRATEGY.md Section 4):

```bash
sqlite3 data/evaluator.db "
  -- 1. Ingestion freshness
  SELECT 'freshest_trade_age_hours',
    ROUND((julianday('now') - julianday(MAX(datetime(timestamp, 'unixepoch')))) * 24, 1)
  FROM trades_raw;

  -- 2. Deduplication check
  SELECT 'duplicate_trades', COUNT(*) FROM (
    SELECT transaction_hash, proxy_wallet, condition_id, COUNT(*) as cnt
    FROM trades_raw GROUP BY transaction_hash, proxy_wallet, condition_id HAVING cnt > 1
  );

  -- 3. Position snapshot completeness
  SELECT 'wallets_with_snapshot_today', COUNT(DISTINCT proxy_wallet)
  FROM positions_snapshots
  WHERE snapshot_at > datetime('now', '-24 hours');

  -- 4. Paper trade integrity
  SELECT 'open_paper_trades', COUNT(*) FROM paper_trades WHERE status = 'open';

  -- 5. API health (check for recent errors in logs if available)
  SELECT 'total_trades_last_24h', COUNT(*) FROM trades_raw
  WHERE ingested_at > datetime('now', '-24 hours');
" 2>/dev/null
```

Report any failures clearly:
- Freshest trade > 2 hours old → ingestion may be stuck
- Duplicate trades > 0 → deduplication bug
- Snapshot coverage < 95% → API rate limiting or failures

### Step 4: Compute Evaluation Metrics

Based on the current phase, compute the relevant metrics from EVALUATION_STRATEGY.md Section 2.

**For MScore evaluation (Phase 1):**
```bash
sqlite3 data/evaluator.db "
  SELECT
    COUNT(*) as markets_scored,
    SUM(CASE WHEN mscore > 0.5 THEN 1 ELSE 0 END) as high_score_markets,
    ROUND(AVG(mscore), 3) as avg_mscore,
    ROUND(MAX(mscore), 3) as max_mscore,
    ROUND(MIN(mscore), 3) as min_mscore
  FROM market_scores_daily
  WHERE score_date = date('now');
"
```

**For wallet discovery evaluation (Phase 2):**
```bash
sqlite3 data/evaluator.db "
  SELECT
    discovered_from,
    COUNT(*) as wallet_count,
    SUM(CASE WHEN total_markets_traded >= 10 THEN 1 ELSE 0 END) as experienced,
    SUM(CASE WHEN is_active = 1 THEN 1 ELSE 0 END) as active
  FROM wallets
  GROUP BY discovered_from;
"
```

**For paper trading evaluation (Phase 4):**
```bash
sqlite3 data/evaluator.db "
  -- Per-wallet paper performance
  SELECT
    pt.proxy_wallet,
    w.pseudonym,
    COUNT(*) as trades,
    SUM(CASE WHEN pt.status = 'settled_win' THEN 1 ELSE 0 END) as wins,
    ROUND(SUM(pt.pnl), 2) as total_pnl,
    ROUND(100.0 * SUM(CASE WHEN pt.status = 'settled_win' THEN 1 ELSE 0 END) / COUNT(*), 1) as hit_rate_pct
  FROM paper_trades pt
  JOIN wallets w ON pt.proxy_wallet = w.proxy_wallet
  WHERE pt.status != 'open'
  GROUP BY pt.proxy_wallet
  HAVING trades >= 5
  ORDER BY total_pnl DESC
  LIMIT 20;
"
```

**For WScore evaluation (Phase 5):**
```bash
sqlite3 data/evaluator.db "
  SELECT
    ws.proxy_wallet,
    w.pseudonym,
    ws.window_days,
    ROUND(ws.wscore, 3) as wscore,
    ROUND(ws.paper_roi_pct, 1) as roi_pct,
    ROUND(ws.paper_hit_rate, 1) as hit_rate,
    ROUND(ws.paper_max_drawdown_pct, 1) as max_dd,
    ws.recommended_follow_mode,
    ws.risk_flags
  FROM wallet_scores_daily ws
  JOIN wallets w ON ws.proxy_wallet = w.proxy_wallet
  WHERE ws.score_date = date('now')
  AND ws.window_days = 30
  ORDER BY ws.wscore DESC
  LIMIT 10;
"
```

### Step 5: Apply Decision Rules

Using the metrics from Step 4 and the decision rules from EVALUATION_STRATEGY.md Section 3:

**Phase advancement:** Check ALL exit criteria. If all met → recommend advancing.

**Wallet decisions:**
- Kill: PnL < -10% over 7d, or hit rate < 40% over 30+ trades, or inactive 14+ days
- Promote: PnL > +5% (7d) AND >+10% (30d), hit rate > 55%, max drawdown < 15%

**System-level decisions:**
- Pause if API error rate > 10%
- Pause if all portfolios in drawdown
- Alert if data quality checks fail

### Step 6: Produce Recommendations

Present exactly 3 prioritized recommendations. Each follows this format:

```markdown
### Recommendation 1: {Action}

**Do this:** {Specific, concrete action}

**Because:** {What the data says — include specific numbers from Step 4}

**The math:** {Expected impact calculation}

**If it doesn't work:** {Fallback plan}

**Effort:** {Hours/days estimate}

**Execution:** {Which plan task to execute, or config change, or investigation}
```

**Prioritization order** (from EVALUATION_STRATEGY.md):
1. Fix broken things (data quality failures, crashed processes)
2. Complete current phase (unmet exit criteria)
3. Advance to next phase (all criteria met)
4. Optimize within phase (tune parameters, improve scores)
5. Research next opportunity

### Step 7: Check Implementation Plan Progress

Read the plan to see what's been completed:

```bash
cat docs/plans/*.md | grep -l "Implementation Plan" | xargs head -n 1
```

Cross-reference with git log to see which tasks are actually done:
```bash
git log --oneline -30
```

Report: "Tasks 1-N complete. Currently on Task M. Next task: Task M+1."

## Output Format

Always produce this structured output:

```markdown
# Evaluator Guidance Report

**Date:** {today}
**Current Phase:** {Phase N: Name} ({BUILD/COLLECT/EVALUATE/VALIDATE})
**Phase Progress:** {X of Y exit criteria met}
**Data Quality:** {PASS / FAIL with details}

## Phase Status

{Table of exit criteria with checkmarks}

## Key Metrics

{Relevant metrics for current phase from Step 4}

## Recommendations

### 1. {Highest priority action}
...

### 2. {Second priority action}
...

### 3. {Third priority action}
...

## Next Session Checklist

- [ ] {First thing to do next session}
- [ ] {Second thing}
- [ ] {Third thing}
```

## Common Mistakes

| Mistake | Why it's wrong | Do this instead |
|---------|---------------|-----------------|
| Skip data quality checks | Metrics are meaningless if data is corrupt | Always run quality checks first |
| Recommend Phase 5 work when Phase 2 isn't done | Phases are sequential for a reason | Complete current phase before advancing |
| Evaluate WScore after 2 days of data | Need 7+ days minimum for meaningful scores | Wait for sufficient data window |
| Ignore failing exit criteria | "Close enough" leads to compounding errors | Every criterion must actually pass |
| Give vague recommendations | "Improve ingestion" is not actionable | "Fix dedup bug in trades_raw — 47 duplicates found in last 24h" |
| Forget the implementation plan | The plan tells you exactly what to build next | Always cross-reference plan progress |
