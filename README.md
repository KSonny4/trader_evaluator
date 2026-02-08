  To build code according to the evaluation strategy, use /evaluator-guidance first — it reads EVALUATION_STRATEGY.md + your
  SQLite data and tells you which phase you're in and what to build next.

  Then to actually implement, the flow is:
  1. /evaluator-guidance → tells you "you're in Phase X, build Y next"
  2. The implementation plan (docs/plans/2026-02-08-wallet-evaluator-mvp.md) has the exact tasks mapped to each phase
  3. Use superpowers:executing-plans to execute the plan task-by-task, or superpowers:subagent-driven-development to dispatch
  tasks from this session

  The workflow after every meaningful change: make deploy && make check-phase-N to verify the DB is filling up as expected. make
   status anytime for a quick pipeline health overview.