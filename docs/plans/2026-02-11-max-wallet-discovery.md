# Max Wallet Discovery Plan

## Goal

Collect "preferably all" wallets by:

1. Running discovery continuously (rate limit as the only blocker)
2. Paginating market trades to discover many more traders
3. Adding leaderboard sourcing
4. Increasing market count (no per-market cap: take everything)

---

## Implementation Summary

- **Remove max_wallets_per_market cap** — insert all discovered wallets per market
- **Paginate market trades** — `trades_pages_per_market` (default 15) pages of 200 trades each
- **Leaderboard discovery** — fetch `/v1/leaderboard` for multiple categories/time periods
- **Continuous mode** — `wallet_discovery_mode = "continuous"` runs discovery in a loop (rate limit only)
- **Config**: `top_n_events = 50`, remove `max_wallets_per_market`, add `trades_pages_per_market`

---

## API Call Volume (per full discovery run)

| Source      | Calls                                     |
| ----------- | ----------------------------------------- |
| Holders     | 50 markets × 1 = 50                       |
| Trades      | 50 markets × 15 pages = 750               |
| Leaderboard | 3 categories × 2 periods × 20 pages = 120 |
| **Total**   | **~920**                                  |

At 200ms/call: ~4 min per run. Continuous = new run every ~4 min (rate limit only).
