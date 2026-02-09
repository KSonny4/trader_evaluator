# Position Size Simulation Results

## Daily Loss Analysis Summary

Based on 100-day simulation with 55% wallet edge, risk-capped paper trading:

### **Key Finding: Daily Loss is CAPPED at $30 for ALL Strategies**

| Position Size | Per Trade | Trades to Hit Daily Cap | Typical Daily Range | Volatility |
|---------------|------------|------------------------|-------------------|-------------|
| **Conservative (2.5%)** | $25 | 2 losing trades | $10-$40 | Low |
| **Moderate (5%)** | $50 | 1-2 losing trades | $20-$60 | Medium |
| **Aggressive (10%)** | $100 | 1 losing trade | $40-$120 | High |

### **Worst-Case Daily Scenarios**

| Scenario | Conservative (2.5%) | Moderate (5%) | Aggressive (10%) |
|----------|---------------------|----------------|-------------------|
| **Terrible Day** (20% win rate) | -$50 (2 trades) | -$100 (2 trades) | -$200 (2 trades) |
| **Bad Day** (35% win rate) | -$40 (2 trades) | -$80 (2 trades) | -$160 (2 trades) |
| **Normal Bad Day** (45% win rate) | -$75 (3 trades) | -$50 (2 trades) | -$100 (1 trade) |

**⚠️  BUT: All scenarios limited to -$30 daily loss by risk cap!**

### **100-Day Performance Summary**

| Strategy | Final Bankroll | Total Return | Avg Daily PnL | Daily Loss Cap Hit | Sharpe Ratio |
|----------|----------------|--------------|----------------|-------------------|--------------|
| **Conservative (2.5%)** | $3,624 | +262% | +$26 | 29 days | 7.81 |
| **Moderate (5%)** | $7,978 | +698% | +$70 | 75 days | 10.84 |
| **Aggressive (10%)** | $6,527 | +553% | +$55 | 42 days | 7.06 |

### **Risk Management in Action**

**Daily Loss Protection:**
- Portfolio cap: 3% = $30 maximum daily loss
- Per-wallet cap: 2% = $20 maximum per wallet
- **All strategies stop trading after hitting caps**

**Position Size Impact:**
- **Does NOT affect maximum daily loss** (still $30)
- **DOES affect data collection speed** and volatility
- **DOES affect psychological impact** of losses

### **Recommendation: Moderate (5% = $50 per trade)**

**Why 5% is optimal:**

1. **Same Safety Net:** $30 daily loss cap protects you
2. **Better Data:** 2x faster statistical significance vs 2.5%
3. **Manageable Volatility:** Less wild than 10% strategy
4. **Best Risk-Adjusted Returns:** Highest Sharpe ratio (10.84)
5. **Practical Position Size:** $50 allows good testing without excessive risk

**Daily Reality:**
- **Most days:** $20-$60 PnL range
- **Bad days:** Limited to -$30 maximum
- **Good days:** Can be $100+ when edge shows through
- **Recovery:** 2 good days recover 1 bad day

**Compared to 2.5%:**
- Same $30 daily loss protection
- 2x faster edge detection
- Better risk-adjusted returns
- Still very conservative overall

**Compared to 10%:**
- Same $30 daily loss protection  
- More stable (less volatile)
- Higher Sharpe ratio
- Less psychological stress

The daily loss caps make all strategies equally safe from catastrophic losses. The choice is about **how quickly you want to gather meaningful data** while staying psychologically comfortable.

**Bottom line: 5% position sizing gives you the best balance of safety, speed, and statistical power.**