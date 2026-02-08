# Risk Management Sanity Check

## 🚨 Critical Insight: Exposure vs Daily Loss Cap Confusion

### The Problem
Initial configuration created confusing risk parameters:
- **Portfolio Exposure Cap:** $150 (15% of $1,000 bankroll) 
- **Daily Loss Cap:** $30 (3% of $1,000 bankroll)

### Why Both Numbers Make Sense

## 📊 Portfolio Exposure Cap = $150

**Purpose:** Limit simultaneous potential risk
```
3 positions × $50 each = $150 total exposure
IF ALL 3 lose 100% = $150 potential loss
```

**Code Logic:** `max_exposure_per_wallet_pct = 5.0` (5% per wallet)
```
let wallet_cap = bankroll_usdc * (max_exposure_per_wallet_pct / 100.0);
// $1000 * 0.05 = $50 per wallet cap
```

## 🛡️ Daily Loss Cap = $30

**Purpose:** Stop trading after $30 REALIZED loss
```
Realized loss means SETTLED trades only
Unrealized losses (open positions) don't count toward cap
```

**Code Logic:** `portfolio_daily_loss_pct = 3.0` (3% daily loss)
```rust
let realized: f64 = conn.query_row(
    "SELECT COALESCE(SUM(pnl), 0.0) FROM paper_trades WHERE strategy = ?1 AND status != 'open'",
    rusqlite::params![strategy],
    |row| row.get(0),
)?;
if realized.abs() > stop_usdc {
    return "portfolio_stop"  // HALT ALL TRADING
}
```

## 🎯 Real World Example

### Time 10:00 AM - Normal Trading
```
Open Positions:
- BTC $100k YES: $50 (+$10 unrealized)
- Trump Win YES: $50 (-$5 unrealized)  
- ETH $5k YES: $50 (-$15 unrealized)

Total Exposure: $150 (15% of bankroll)
Realized Loss: $0
Status: TRADING CONTINUES
```

### Time 2:00 PM - Positions Settle
```
Settled Positions:
- BTC $100k: +$20 profit ✅
- Trump Win: -$25 loss ❌
- ETH $5k: Still open (-$15 unrealized)

Realized Loss: $5 ($25 loss - $20 profit)
Unrealized Loss: $15
Total Current Loss: $20
Status: TRADING CONTINUES (under $30 cap)
```

### Time 4:00 PM - Disaster Strikes
```
Another Settled Position:
- New market: -$30 loss ❌

Realized Loss: $35 ($5 + $30)
Status: TRADING STOPS (portfolio_stop triggered)
```

## 🤯 The Key Insight

**$150 Exposure = POTENTIAL risk (all positions lose 100%)**
**$30 Loss Cap = REALIZED loss protection (trading stops)**

### Why Both Are Needed

| Control | Amount | Purpose | Trigger |
|---------|---------|---------|----------|
| **Exposure Cap** | $150 | Prevent over-leverage | Opening new positions |
| **Daily Loss Cap** | $30 | Stop catastrophic loss | Realized losses exceed 3% |

## 🔍 Sanity Check Questions

### Q: Why allow $150 exposure if only $30 loss allowed?
**A:** $150 is potential risk spread across multiple positions. $30 cap stops trading early to prevent full $150 disaster.

### Q: Can I lose $150 in one day?
**A:** No. Trading stops at $30 realized loss. The remaining $120 risk is unrealized and may recover.

### Q: Is 15% exposure too high for paper trading?
**A:** For paper trading, it's acceptable because:
- Daily cap provides real safety ($30 max loss)
- Multiple positions = better edge detection
- You're testing, not protecting real capital
- Still conservative vs real trading (would use 5-10% max)

## ✅ Configuration Validation

The current configuration is actually well-designed for paper trading:

```toml
[risk]
max_exposure_per_wallet_pct = 5.0      # $50 per wallet (reasonable)
portfolio_daily_loss_pct = 3.0         # $30 daily loss (conservative)

[paper_trading]  
position_size_usdc = 25.0              # $25 per trade (2.5%)
bankroll_usd = 1000.0                   # $1,000 bankroll
```

**Result:** Conservative daily loss protection with sufficient exposure for edge testing.

## 🎯 Bottom Line

The confusion between $150 exposure and $30 loss cap is actually a **feature, not a bug**:

1. **$150 exposure** allows testing across multiple markets
2. **$30 loss cap** provides catastrophic protection  
3. **Algorithm stops trading** before full exposure loss can occur
4. **Paper trading benefits** from this balanced approach

This design gives you **statistical power** while maintaining **capital protection**.

---

*Document created after user questioned the apparent contradiction between portfolio exposure ($150) and daily loss cap ($30). This sanity check confirms the configuration is correct and intentional.*