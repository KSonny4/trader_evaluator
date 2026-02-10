# Paper fill verification: mini orderbook and opportunity window

Design note for paper trading when following wallets that trade many new markets. Complements Strategy Bible §6 (Realism Requirements) and PR #21 (execution interface).

## Insight

We are **edge detectors**, not speed traders. When we paper-copy someone who constantly tests new markets:

- We **do not need to race** the orderbook in real time.
- We only need to know: (1) **was it possible to place** (for some realistic window after detection), and (2) **the outcome** (market resolution).
- So we **do not need to stream or record** the full orderbook during paper trading — we can verify **after** we detect the trade.

That allows verifying many more trades (including on rotating / short-lived markets) without real-time ingestion.

## What we need to store

### 1. Mini orderbook snapshot (required for proof)

- **When:** After we detect the copied trade, within a short window (e.g. 10–120 seconds).
- **What:** A minimal orderbook snapshot sufficient to prove that our size could have been filled at our entry price (or within slippage).
- **Why:** So we don’t paper-trade fills that couldn’t have happened. Aligns with Strategy Bible “Fill probability | Check orderbook at detection time (when available)” — here “detection time” is “within a short window after detection.”
- **Scope:** One (or a few) snapshots per paper trade, not a continuous stream.

### 2. Opportunity window duration (optional, for evaluation)

- **Idea:** For evaluation it’s useful to know **how long** the opportunity was available (e.g. “could have filled for 45 seconds” vs “only 5 seconds”).
- **Requires:** Recording orderbook (or at least fill feasibility) over a short window, then closing the window when:
  - a fixed max (e.g. 2 minutes), or
  - price moves beyond a threshold (opportunity gone).
- **Use:** Window size can inform “exploitability” or probability — longer window = more realistic to copy; very short window = more execution-sensitive.

## Design choices (to be decided)

| Topic | Options |
|-------|--------|
| Window length | 10–120 s (configurable); min to prove fill, max to bound storage. |
| Snapshot format | Minimal: best bid/ask + depth for our size; or full L2 for the window (heavier). |
| When to fetch | On detection: async fetch orderbook (or recent CLOB data) for that market; store if fill was possible. |
| Opportunity window recording | Optional feature: record “fill possible” over time; close after 2 min or Δprice threshold. |

## Relation to existing work

- **Strategy Bible §6:** Fill probability at detection time → this doc refines it to “within a short window after detection” and “mini snapshot” instead of real-time stream.
- **PR #21 (execution interface):** PaperExecutor can remain “immediate fill + paper settlement”; the **fill decision** can use this mini-orderbook verification before creating the paper trade. LiveExecutor would use real CLOB; PaperExecutor uses stored snapshot + outcome.
- **SKIPPED_NO_FILL:** Unchanged: if the mini orderbook shows we couldn’t absorb our size, we skip and record `SKIPPED_NO_FILL`.

## Next steps

1. Decide snapshot format and window (10–120 s default).
2. Add optional “opportunity window” duration to schema/metrics if we want it for evaluation.
3. In implementation plan: task(s) for “on paper-trade detection, fetch and store mini orderbook; verify fill possible before creating paper_trade.”
