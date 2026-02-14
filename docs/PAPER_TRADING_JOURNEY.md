# Paper Trading Journey

How the trader microservice (`crates/trader/`) discovers, mirrors, and settles paper trades end-to-end.

## Architecture overview

```
                         +-----------+
                         |  REST API |  (Axum, port 8080)
                         |  /wallets |
                         |  /trades  |
                         |  /risk    |
                         +-----+-----+
                               |
                         +-----v-----+
                         |  Wallet   |
                         |  Engine   |  (orchestrator)
                         +-----+-----+
                               |
              +----------------+----------------+
              |                |                |
        +-----v-----+   +-----v-----+   +------v------+
        |  Watcher   |  |  Watcher   |  |  Watcher    |
        |  wallet A  |  |  wallet B  |  |  wallet C   |
        +-----+------+  +-----+------+  +------+------+
              |                |                |
              +-------+--------+--------+-------+
                      |                 |
                +-----v-----+    +------v------+
                | Data API  |    | Gamma API   |
                | /trades   |    | /markets    |
                | (polling) |    | (settlement)|
                +-----+-----+    +------+------+
                      |                 |
                +-----v-----------------v------+
                |          SQLite DB           |
                |  trades, positions, risk,    |
                |  fidelity, slippage logs     |
                +------------------------------+
```

## 1. Service startup

```
main()
  -> Load TraderConfig from TOML file
  -> Open SQLite DB (WAL mode, migrations)
  -> Create TraderPolymarketClient (Data API wrapper)
  -> Create RiskManager with risk config
  -> Create WalletEngine (owns DB, client, config, risk)
  -> restore_watchers(): load active wallets from DB, spawn a watcher task per wallet
  -> Start Axum HTTP server with bearer-token auth middleware
```

On restart, the service picks up exactly where it left off. Each wallet's `last_trade_seen_hash` is persisted in the DB, so the detector knows which trades have already been processed.

## 2. Following a wallet

**Endpoint:** `POST /api/wallets`

```json
{
  "proxy_wallet": "d67aeff736bfa5e32b269803f0809e84c07b61060e6eb520be9bc8aae30ed129",
  "label": "whale-btc-scalper",
  "estimated_bankroll_usd": 50000,
  "trading_mode": "paper"
}
```

**What happens:**

1. **Validate** the wallet address (accepts `0x` + 40 hex chars OR 64 hex chars for Polymarket proxy wallets)
2. **Insert** into `followed_wallets` table with `status = 'active'`
3. **Spawn** an async `WalletWatcher` task for this wallet
4. Return `201 Created`

The wallet is now being watched. The watcher runs independently in the background.

## 3. The watcher loop (per wallet)

Each followed wallet gets its own async task that runs a poll loop:

```
loop (every poll_interval_secs, default 30s):
  |
  |-- Check global halt flag -> if halted, skip
  |-- Check wallet status in DB -> if paused/killed/removed, skip or exit
  |
  |-- STEP 1: Fetch trades from Data API
  |     GET /trades?user={wallet}&limit=200
  |
  |-- STEP 2: Detect new trades (watermark-based)
  |     Filter out already-seen trade hashes
  |     Sort chronologically
  |
  |-- STEP 3: For each new trade -> mirror_trade() (sequential — one at a time)
  |
  |-- STEP 4: Check settlements for open positions
  |     Query Gamma API for market resolution
  |     If resolved -> settle and calculate PnL
  |
  |-- Update watermark in DB (last_trade_seen_hash)
```

The halt flag is an `Arc<AtomicBool>` created in `WalletEngine::new()` and cloned to every watcher. `POST /api/halt` calls `halt_all()` which sets it to `true`; `POST /api/resume` clears it. Each watcher checks the flag with `SeqCst` ordering at the top of every poll cycle.

**Minimum effective interval:** No validation prevents setting `poll_interval_secs = 1`, but each poll makes at least one Data API call with a 200ms rate-limit delay. With HTTP 429 backoff (2s), polling faster than ~1s per wallet yields diminishing returns. Default: 30s for paper trading (we're edge detectors, not speed traders).

Trades within a single wallet are mirrored **sequentially**: each `mirror_trade().await` must complete before the next starts. This is intentional — risk checks read current exposure before executing, so concurrent mirroring within one wallet would create race conditions (two trades could both pass risk checks reading stale exposure, exceeding limits). Different wallets already run in parallel via separate watcher tasks.

The watcher is cancelled when the wallet is unfollowed or the service shuts down.

## 4. Trade detection

The `TradeDetector` maintains a set of seen trade hashes to avoid re-processing:

**Hash priority:**
1. `trade.id` (if present)
2. `trade.transaction_hash` (if present)
3. Composite: `"{wallet}-{condition_id}-{timestamp}-{side}"`

**Watermark persistence:** The most recent trade hash is stored in `followed_wallets.last_trade_seen_hash`. On restart, the detector is initialized with this hash, so it skips all trades up to and including the last one processed.

**Memory management:** The seen-set is pruned periodically (`prune(keep=1000)`). If it grows beyond `2 * keep`, it's cleared entirely and the detector relies on the timestamp watermark.

## 5. Trade mirroring

When a new trade is detected, `mirror_trade()` runs the full pipeline:

### 5.1 Parse the trade

```
side       = BUY or SELL
their_price = 0.0 to 1.0 (probability)
their_size  = USD value of their trade
```

### 5.2 Size our position

**Proportional sizing** (default, `use_proportional_sizing = true`):
```
ratio        = our_bankroll / their_estimated_bankroll
our_size_usd = their_size_usd * ratio
our_size_usd = min(our_size_usd, per_trade_size_usd)  // cap
```

**Fixed sizing** (fallback):
```
our_size_usd = per_trade_size_usd
```

### 5.3 Risk check (11 gates)

The risk manager runs every gate in order. First rejection stops the trade:

| # | Gate | Scope | What it checks |
|---|------|-------|----------------|
| 1 | Global halt | System | Emergency stop flag |
| 2 | Portfolio exposure | Portfolio | `(current + trade) <= bankroll * max_total_exposure_pct%` |
| 3 | Daily loss limit | Portfolio | `daily_pnl >= -(bankroll * max_daily_loss_pct%)` |
| 4 | Weekly loss limit | Portfolio | `weekly_pnl >= -(bankroll * max_weekly_loss_pct%)` |
| 5 | Max positions | Portfolio | `open_positions < max_concurrent_positions` |
| 6 | Wallet exposure | Per-wallet | `(wallet_exposure + trade) <= bankroll * per_wallet_max_pct%` |
| 7 | Wallet daily loss | Per-wallet | Same as #3 but per wallet |
| 8 | Wallet weekly loss | Per-wallet | Same as #4 but per wallet |
| 9 | Wallet drawdown | Per-wallet | `(peak_pnl - current_pnl) / peak_pnl < max_drawdown_pct%` |
| 10 | Slippage kill | Per-wallet | Average slippage over last 20 trades < 5 cents |
| 11 | Copy fidelity | Per-wallet | `copied / total_decisions * 100 >= min_copy_fidelity_pct` |

If rejected, the fidelity outcome is logged as `SKIPPED_*` and the trade is not executed.

### 5.4 Apply costs

**Slippage** (simulated market impact):
```
slippage_pct = slippage_default_cents / 100.0  (default: 1 cent)

BUY:  our_entry_price = min(their_price + slippage_pct, 0.99)
SELL: our_entry_price = max(their_price - slippage_pct, 0.01)
```

**Taker fee** (quartic formula, conditional):
```
Only for crypto 15-minute markets:
  fee = price * 0.25 * (price * (1 - price))^2
  Max ~1.56% at p=0.50

All other markets: fee = 0

BUY:  our_entry_price += fee
SELL: our_entry_price -= fee
```

### 5.5 Execute the paper trade

Five writes happen atomically:

1. **INSERT** into `trader_trades` with `status = 'open'`
2. **UPSERT** into `trader_positions` (accumulates size, recalculates weighted-average entry price)
3. **UPDATE** `risk_state` for the wallet (increase exposure, increment positions)
4. **UPDATE** `risk_state` for the portfolio (same)
5. **INSERT** into `copy_fidelity_log` with `outcome = 'COPIED'`
6. **INSERT** into `follower_slippage_log` with slippage metrics

### 5.6 Position accumulation

If we already hold a position in the same market (same wallet + condition_id + side), the position is accumulated:

```
new_shares     = our_size_usd / our_entry_price
total_shares   = old_shares + new_shares
avg_entry      = (old_avg * old_shares + new_price * new_shares) / total_shares
total_size_usd = old_total + our_size_usd
```

## 6. Settlement

Each poll cycle, the watcher checks settlement for every open position:

### 6.1 Check resolution

```
For each open position (condition_id):
  GET gamma-api.polymarket.com/markets?condition_id={id}
  If market.closed == true:
    Extract settlement price from outcomePrices
    settle_price >= 0.99 -> outcome won (1.0)
    settle_price <= 0.01 -> outcome lost (0.0)
```

### 6.2 Calculate PnL

For each open trade in the settled market:

```
BUY:  pnl = (settle_price - our_entry_price) * our_size_usd
SELL: pnl = (our_entry_price - settle_price) * our_size_usd

status = pnl >= 0 ? "settled_win" : "settled_loss"
```

### 6.3 Close out

1. **UPDATE** `trader_trades`: set `status`, `exit_price`, `pnl`, `settled_at`
2. **DELETE** from `trader_positions` (position closed)
3. **UPDATE** `risk_state`: reduce wallet exposure, adjust PnL
4. **UPDATE** `risk_state`: reduce portfolio exposure, adjust PnL

## 7. Risk state tracking

Risk state is tracked at two levels via the `risk_state` table:

| Key | Tracks |
|-----|--------|
| `"portfolio"` | Aggregate across all wallets |
| `"{wallet_address}"` | Per-wallet metrics |

Each row contains:
- `total_exposure_usd` — sum of open position sizes
- `daily_pnl`, `weekly_pnl` — rolling P&L (reset on period boundaries)
- `peak_pnl`, `current_pnl` — for drawdown calculation
- `open_positions` — count of open positions
- `is_halted`, `halt_reason` — per-wallet or global halt

Risk config can be updated at runtime via `PUT /api/risk` without restarting the service.

## 8. Fidelity and slippage tracking

### Copy fidelity

Every trade decision (copy or skip) is logged in `copy_fidelity_log`:

| Outcome | Meaning |
|---------|---------|
| `COPIED` | Trade was mirrored |
| `SKIPPED_PORTFOLIO_RISK` | Rejected by portfolio-level risk gate |
| `SKIPPED_WALLET_RISK` | Rejected by wallet-level risk gate |
| `SKIPPED_DAILY_LOSS` | Daily loss limit hit |
| `SKIPPED_WEEKLY_LOSS` | Weekly loss limit hit |
| `SKIPPED_MARKET_CLOSED` | Market already resolved |
| `SKIPPED_DETECTION_LAG` | Trade too old to mirror |
| `SKIPPED_NO_FILL` | Could not fill at acceptable price |

Fidelity percentage = `COPIED / total_decisions * 100`. If fidelity drops below `min_copy_fidelity_pct`, the wallet is auto-killed (gate #11).

### Slippage

Every executed trade logs to `follower_slippage_log`:
- `their_entry_price` vs `our_entry_price`
- `slippage_cents = abs(our - their) * 100`
- `detection_delay_ms` — time from their trade to our detection

If average slippage over the last 20 trades exceeds 5 cents, the wallet is auto-killed (gate #10).

## 9. Wallet lifecycle

```
                 POST /wallets
                      |
                      v
                  [active] -----> watcher spawned, mirroring trades
                   |    ^
    POST /pause    |    |  POST /resume
                   v    |
                 [paused] -----> watcher skips trades, still polls
                   |
    DELETE /wallet |
                   v
                [removed] -----> watcher exits, no more polling
```

Additional states:
- **killed** — auto-set when slippage or fidelity gates trigger; watcher exits
- **halted** (global) — `POST /api/halt` sets a system-wide flag; all watchers skip trades until `POST /api/resume`

## 10. REST API reference

| Method | Endpoint | Purpose |
|--------|----------|---------|
| GET | `/api/health` | Health check (no auth) |
| GET | `/api/status` | Service status |
| POST | `/api/wallets` | Follow a new wallet |
| GET | `/api/wallets` | List all followed wallets |
| DELETE | `/api/wallets/{addr}` | Unfollow a wallet |
| POST | `/api/wallets/{addr}/pause` | Pause mirroring |
| POST | `/api/wallets/{addr}/resume` | Resume mirroring |
| GET | `/api/trades?wallet=&status=&limit=` | Query paper trades |
| GET | `/api/positions` | Current open positions |
| GET | `/api/pnl` | Portfolio P&L summary |
| POST | `/api/halt` | Emergency stop all trading |
| POST | `/api/resume` | Resume after halt |
| GET | `/api/risk` | Risk state snapshot |
| PUT | `/api/risk` | Update risk config at runtime |

All endpoints except `/api/health` require `Authorization: Bearer <api_key>` when `api_key` is configured.

## 11. Database schema

| Table | Purpose | Key columns |
|-------|---------|-------------|
| `followed_wallets` | Wallet registry | `proxy_wallet` (PK), status, trading_mode, last_trade_seen_hash |
| `trader_trades` | All paper trades | id, proxy_wallet, condition_id, side, their_price, our_entry_price, pnl, status |
| `trader_positions` | Open positions | proxy_wallet + condition_id + side (unique), total_size_usd, avg_entry_price, share_count |
| `risk_state` | Risk tracking | key (PK: "portfolio" or wallet addr), exposure, pnl, positions |
| `copy_fidelity_log` | Copy/skip decisions | proxy_wallet, outcome (COPIED/SKIPPED_*) |
| `follower_slippage_log` | Slippage metrics | proxy_wallet, slippage_cents, detection_delay_ms |
| `trade_events` | Audit trail | event_type, proxy_wallet, details_json |

## 12. Configuration reference

```toml
[server]
port = 8080
host = "0.0.0.0"
api_key = "your-secret-token"

[database]
path = "trader.db"

[polymarket]
data_api_url = "https://data-api.polymarket.com"
gamma_api_url = "https://gamma-api.polymarket.com"
rate_limit_delay_ms = 200

[trading]
bankroll_usd = 1000.0
per_trade_size_usd = 50.0
use_proportional_sizing = true
default_their_bankroll_usd = 10000.0
mirror_delay_secs = 0
slippage_default_cents = 1.0
poll_interval_secs = 30

[risk.portfolio]
max_total_exposure_pct = 80.0
max_daily_loss_pct = 5.0
max_weekly_loss_pct = 10.0
max_concurrent_positions = 20

[risk.per_wallet]
max_exposure_pct = 25.0
daily_loss_pct = 3.0
weekly_loss_pct = 7.0
max_drawdown_pct = 15.0
min_copy_fidelity_pct = 50.0
```
## 13. Detection delay and fillability

### What exists today

`detection_delay_ms = now - their_timestamp` is recorded in both `trader_trades` and `follower_slippage_log` for every paper trade. This tells us how stale the trade was when we detected it, but not whether we could have actually filled.

### Fillability window recording

After detecting a trade, the watcher spawns a background task that opens a WebSocket to the Polymarket CLOB (`wss://ws-subscriptions-clob.polymarket.com/ws/market`) for 120 seconds and records order book snapshots to the `book_snapshots` table. Each snapshot captures best bid/ask, spread, depth at our price, and a `fillable` boolean indicating whether our trade size could have been executed at our target price or better.

If the same market is already being recorded (another trade detected within the 120s window), the timeout is restarted rather than opening a duplicate connection. All triggering trade hashes are accumulated.

When the 120s window closes, a `fillability_results` row is written with:
- `fill_probability` (0.0 to 1.0) — fraction of snapshots where fill was feasible
- `opportunity_window_secs` — contiguous fillable time from start
- `avg_slippage_cents` — average VWAP slippage across fillable snapshots

**Meaning:** `fill_probability = 1.0` means every snapshot showed the order book had sufficient depth — high confidence we could have copied this trade live. `fill_probability = 0.0` means the book was never deep enough — this paper trade is unrealistic.

### Enum support

`FidelityOutcome::SkippedDetectionLag` and `SkippedNoFill` exist in `types.rs` but aren't wired up yet. Once fillability data is available, these will be used to reject trades where fill probability is too low.

## 14. End-to-end example

```
1. You call POST /api/wallets with a whale's proxy wallet address

2. Watcher spawns, polls Data API every 30s

3. Whale buys BTC-YES at $0.62 for $500
   -> Watcher detects the trade (new hash)
   -> Proportional sizing: our_bankroll ($1000) / their_bankroll ($50000) = 0.02
      our_size = $500 * 0.02 = $10
   -> Risk check: all 11 gates pass
   -> Slippage: our_entry = $0.62 + $0.01 = $0.63
   -> Fee: $0 (not a crypto 15m market)
   -> Paper trade recorded: BUY BTC-YES, $10 at $0.63

4. Whale buys more BTC-YES at $0.65 for $300
   -> our_size = $300 * 0.02 = $6
   -> Position accumulated:
      old: 15.87 shares at $0.63 avg
      new: 9.09 shares at $0.66
      total: 24.96 shares at $0.641 avg, $16 total

5. BTC market resolves YES (settle_price = 1.0)
   -> PnL = (1.0 - 0.641) * $16 = $5.74 (settled_win)
   -> Position deleted, risk state updated
   -> You can see results at GET /api/pnl
```
