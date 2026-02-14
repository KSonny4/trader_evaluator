# Design: Complete Position Tracking with Realized + Unrealized PnL

**Date:** 2026-02-14
**Status:** Approved
**Author:** Claude Sonnet 4.5

## Problem Statement

The current system uses **cashflow PnL** (all sell proceeds - all buy costs) for Stage 1/1.5 gates, which includes unrealized paper gains. This allows wallets with negative **realized PnL** (losing on closed positions) to be classified as followable if they have large unrealized gains.

**Example Bug:**
- Wallet `0xf983feb22d5eabfc7697b426ec1040b8038a651c`:
  - **Realized PnL: -$95.92** (proven losses on closed positions)
  - **Unrealized PnL: ~+$1,097** (paper gains on open positions)
  - **Cashflow PnL: +$1,001.54** (includes unrealized)
  - **Classification: PATIENT_ACCUMULATOR** ❌ (should be EXCLUDED)

**Root Cause:**
- Field naming bug: `WalletFeatures.realized_pnl` actually contains cashflow PnL
- Strategy Bible says "Cashflow PnL = total sell proceeds - total buy costs" (includes unrealized)
- Stage 1 gate checks this cashflow metric, not true realized PnL

**Impact:** Wallets that lose money on every closed position but have unrealized paper gains are being classified as followable.

---

## Design Goals

1. **Accuracy:** Separate realized (proven) from unrealized (paper) PnL
2. **Correct Classification:** Stage 1/1.5 gates use only realized PnL
3. **Transparency:** Dashboard shows realized/unrealized breakdown
4. **API Integration:** Fetch current positions from Polymarket for accurate unrealized valuation
5. **Robustness:** Fallback gracefully if API fails

---

## Solution: FIFO Position Tracking with Polymarket Integration

### 1. Data Model Changes

#### WalletFeatures Struct
```rust
pub struct WalletFeatures {
    // RENAMED (breaking change)
    pub cashflow_pnl: f64,  // Was: realized_pnl. Sell proceeds - buy costs (all trades)

    // NEW FIELDS
    pub fifo_realized_pnl: f64,     // Sum of closed positions (FIFO matched)
    pub unrealized_pnl: f64,         // Open positions valued at current price
    pub total_pnl: f64,              // fifo_realized_pnl + unrealized_pnl
    pub open_positions_count: u32,   // Markets with unmatched buys

    // ... existing fields unchanged
}
```

#### Database Schema
```sql
-- Add new columns to wallet_features_daily
ALTER TABLE wallet_features_daily ADD COLUMN cashflow_pnl REAL NOT NULL DEFAULT 0.0;
ALTER TABLE wallet_features_daily ADD COLUMN fifo_realized_pnl REAL NOT NULL DEFAULT 0.0;
ALTER TABLE wallet_features_daily ADD COLUMN unrealized_pnl REAL NOT NULL DEFAULT 0.0;
ALTER TABLE wallet_features_daily ADD COLUMN total_pnl REAL NOT NULL DEFAULT 0.0;
ALTER TABLE wallet_features_daily ADD COLUMN open_positions_count INTEGER NOT NULL DEFAULT 0;

-- Migrate old realized_pnl → cashflow_pnl for existing records (if any)
UPDATE wallet_features_daily SET cashflow_pnl = realized_pnl WHERE cashflow_pnl = 0.0;
```

---

### 2. Position Tracking & FIFO Logic

#### Enhanced PairedStats
```rust
struct PairedStats {
    wins: u32,
    losses: u32,
    hold_seconds: Vec<f64>,
    closed_pnls: Vec<(i64, f64)>,
    profitable_markets: u32,

    // NEW
    total_fifo_realized_pnl: f64,   // Sum of all closed_pnls
    open_positions: Vec<OpenPosition>,  // Unmatched buys per market
}

struct OpenPosition {
    condition_id: String,
    total_size: f64,           // Sum of unmatched buy sizes
    weighted_cost_basis: f64,  // Weighted average buy price
    oldest_buy_timestamp: i64,
}
```

#### FIFO Pairing Algorithm
Enhance existing `paired_trade_stats` function:

1. **Group trades by condition_id**
2. **For each market:**
   - Sort buys and sells by timestamp
   - Match each sell to oldest unmatched buy (FIFO)
   - Realized PnL = (sell_price - buy_price) * matched_size
   - Add to `total_fifo_realized_pnl`
3. **Track unmatched buys:**
   - Aggregate remaining buy sizes per market
   - Compute weighted average cost basis
   - Store in `open_positions` vector

**Example:**
```
Market A:
  BUY 100 @ $0.40 (cost: $40)
  BUY 50  @ $0.50 (cost: $25)
  SELL 80 @ $0.60 (proceeds: $48)

FIFO Matching:
  - SELL 80 matches first BUY (100):
    * 80 shares @ ($0.60 - $0.40) = +$16.00 realized
  - Remaining: 20 shares @ $0.40 + 50 shares @ $0.50
    * Open position: 70 shares, cost basis $0.457 weighted avg
    * Unrealized: 70 * (current_price - $0.457)
```

---

### 3. Unrealized PnL Calculation (Polymarket API)

#### API Integration
```rust
/// Fetch current positions from Polymarket
async fn fetch_current_positions(
    client: &reqwest::Client,
    proxy_wallet: &str,
) -> Result<Vec<PolymarketPosition>> {
    let url = format!(
        "https://data-api.polymarket.com/positions?user={}",
        proxy_wallet
    );

    // Rate limiting: 200ms delay between requests
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let response = client.get(&url).send().await?;
    let positions: Vec<PolymarketPosition> = response.json().await?;

    Ok(positions)
}

struct PolymarketPosition {
    condition_id: String,
    size: f64,
    market_price: f64,  // Current mark price from Polymarket
}
```

#### Unrealized PnL Computation
```rust
fn compute_unrealized_pnl(
    open_positions: &[OpenPosition],      // From FIFO pairing
    current_positions: &[PolymarketPosition],  // From API
) -> (f64, u32) {
    let mut unrealized_pnl = 0.0;
    let mut open_count = 0;

    for open_pos in open_positions {
        if let Some(current) = find_matching_position(current_positions, &open_pos.condition_id) {
            // Unrealized PnL = (current_price - cost_basis) * size
            let pnl = (current.market_price - open_pos.weighted_cost_basis) * open_pos.total_size;
            unrealized_pnl += pnl;
            open_count += 1;
        } else {
            // Position closed since last trade sync
            warn!("Open position {} not in current positions - may have been closed",
                  &open_pos.condition_id);
        }
    }

    (unrealized_pnl, open_count)
}
```

#### Fallback Strategy
- If Polymarket API fails/times out: set `unrealized_pnl = 0.0`, log warning
- Don't fail entire feature computation
- Stage 1/1.5 gates only use `fifo_realized_pnl` (not affected by API failures)

---

### 4. Stage 1 & 1.5 Gate Updates

#### Stage 1 All-Time ROI Gate
```rust
pub fn compute_all_time_roi(conn: &Connection, proxy_wallet: &str) -> Result<f64> {
    // Compute FIFO-paired realized PnL for ALL time
    let paired_stats = paired_trade_stats(conn, proxy_wallet, 0)?; // cutoff=0 = all time
    let realized_pnl = paired_stats.total_fifo_realized_pnl;

    // Compute ROI denominator
    let (total_buy_cost, _): (f64, f64) = conn.query_row(
        "SELECT
            COALESCE(SUM(CASE WHEN side = 'BUY' THEN size * price ELSE 0.0 END), 0.0),
            COALESCE(SUM(CASE WHEN side = 'SELL' THEN size * price ELSE 0.0 END), 0.0)
         FROM trades_raw
         WHERE proxy_wallet = ?1",
        rusqlite::params![proxy_wallet],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    if total_buy_cost == 0.0 {
        return Ok(0.0);
    }

    Ok(realized_pnl / total_buy_cost)  // ROI based on FIFO realized only
}

// Stage 1 gate check (in pipeline_jobs.rs)
let all_time_roi = compute_all_time_roi(&conn, proxy_wallet)?;
if all_time_roi < config.personas.stage1_min_all_time_roi {
    exclude("STAGE1_REALIZED_LOSS", all_time_roi);  // Updated reason
}
```

#### Stage 1.5 Recent Profitability Gate
```rust
pub fn compute_recent_pnl(
    conn: &Connection,
    proxy_wallet: &str,
    window_days: u32,
    now_epoch: i64,
) -> Result<f64> {
    let cutoff = now_epoch - (i64::from(window_days) * 86400);

    // Use FIFO-paired realized PnL in window (not cashflow!)
    let paired_stats = paired_trade_stats(conn, proxy_wallet, cutoff)?;
    Ok(paired_stats.total_fifo_realized_pnl)
}
```

**Key Changes:**
- Both gates now use **FIFO-paired realized PnL only**
- Unrealized gains don't affect classification decisions
- Wallet with -$95.92 realized will be EXCLUDED (regardless of unrealized)

---

### 5. Dashboard Updates

#### Wallet Detail Page
**New display:**
```
┌─────────────────────────────────┐
│ Performance Summary             │
├─────────────────────────────────┤
│ Realized PnL:    -$95.92  ❌    │
│ Unrealized PnL:  +$1,097.46  📈 │
│ Total PnL:       +$1,001.54     │
│                                 │
│ Open Positions:  3 markets      │
│ Closed Trades:   35 round-trips │
├─────────────────────────────────┤
│ Classification: EXCLUDED        │
│ Reason: STAGE1_REALIZED_LOSS    │
│ (-$95.92 < $0 threshold)        │
└─────────────────────────────────┘
```

#### Query Updates
- Update `wallet_features_latest` to read new columns
- Update `WalletFeaturesSnapshot` struct with new fields
- Update scorecard HTML template for breakdown display

---

### 6. Strategy Bible Updates

**§4 Stage 1 Gate:**
```markdown
| All-time ROI | `stage1_min_all_time_roi` | 0.00 (0%) | Wallets must have positive FIFO-paired realized PnL (closed positions only). Unrealized gains don't count - only proven profits from closed positions demonstrate true edge. |
```

**§4 Stage 1.5 Gate:**
```markdown
| Recent profitability | `stage1_require_recent_profit` | true | Exclude wallets with negative realized PnL in last 30 days. Uses FIFO-paired closed positions, not cashflow or unrealized gains. |
```

**Add Glossary:**
```markdown
### PnL Metrics Glossary

| Metric | Formula | What it measures | Used for |
|--------|---------|------------------|----------|
| **Cashflow PnL** | Total sell proceeds - Total buy costs | All capital flow (includes unrealized positions) | Analytics, capital deployed tracking |
| **FIFO Realized PnL** | Sum of (sell_price - buy_price) * size for FIFO-matched positions | Proven profits/losses from closed positions only | Stage 1/1.5 gates, classification decisions |
| **Unrealized PnL** | (current_price - cost_basis) * size for open positions | Paper gains/losses on positions not yet closed | Dashboard display, risk monitoring |
| **Total PnL** | Realized + Unrealized | Complete current value | Dashboard summary |
```

---

## Implementation Order

1. Database migration (5 min)
2. Enhance paired_trade_stats with TDD (20 min)
3. Add unrealized PnL computation with TDD (15 min)
4. Update WalletFeatures struct (10 min)
5. Fix Stage 1 & 1.5 gates with TDD (10 min)
6. Update dashboard queries (15 min)
7. Update Strategy Bible (5 min)
8. Full verification (10 min)

**Total:** ~90 minutes

---

## Expected Outcomes

**After implementation:**
1. ✅ Wallet `0xf983feb...` EXCLUDED by Stage 1 (realized PnL: -$95.92 < 0%)
2. ✅ All wallets with negative realized PnL excluded (even with unrealized gains)
3. ✅ Dashboard shows accurate breakdown matching Polymarket
4. ✅ Stage 1/1.5 gates use proven, closed profits only
5. ✅ Classification decisions based on real performance, not paper gains

**Followable wallet count expected to drop further** (122 → ~60-80) as wallets with unrealized-only gains get filtered out.

---

## Risk Mitigation

**API Dependency:**
- Polymarket /positions API required for unrealized PnL
- Fallback: If API fails, set `unrealized_pnl = 0.0` and continue
- Gates don't depend on unrealized, so classification still works

**Database Migration:**
- New columns have DEFAULT 0.0 (backward compatible)
- Old records readable, just missing new metrics
- Recompute features after migration for accuracy

**Breaking Changes:**
- Field renamed: `realized_pnl` → `cashflow_pnl`
- All code referencing `realized_pnl` must update
- Tests using old field name must update

---

## Success Criteria

1. ✅ All 356+ tests pass
2. ✅ Wallet `0xf983feb...` excluded with reason "STAGE1_REALIZED_LOSS"
3. ✅ Dashboard shows realized/unrealized breakdown
4. ✅ No wallets with negative realized PnL classified as followable
5. ✅ Strategy Bible updated with PnL glossary
6. ✅ Polymarket positions API integration working
7. ✅ 80%+ test coverage maintained
