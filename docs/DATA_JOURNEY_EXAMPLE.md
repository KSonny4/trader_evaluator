# Full Data Journey: Market → Paper Trading

Example trace for one wallet that completed the full pipeline from event discovery to paper trading.

---

## Wallet: `0x0361d34c84a5d41970aaac11f5de387841adc736`

---

## Stage 1: Event Discovery

**Market where this wallet was discovered:**

| Field | Value |
|-------|-------|
| Condition ID | `0x2e150ddea5674a8cf543f90a9bf1c3552aa9c3f784d82d24e21a522a391534fb` |
| Title | Will Bitcoin reach $90,000 in February? |
| Liquidity | $292,414 |
| MScore (rank) | 0.46 (rank #9 on 2026-02-11) |

**What was applied:**
- Gamma API fetched markets; MScore ranked by liquidity, volume, density, whale concentration, time-to-expiry
- Top 50 events selected daily (EScore = max MScore per event)
- This market was in a top-50 event, so its holders/traders became discovery candidates

---

## Stage 2: Wallet Discovery

**Discovery record:**

| Field | Value |
|-------|-------|
| Source | HOLDER |
| Discovered at | 2026-02-08 11:10:43 |
| Discovered market | (same as above) |
| Is active | yes |

**What was applied:**
- Data API `/holders` returned top holders for the market
- This wallet was in the top holders list
- Inserted into `wallets` with `discovered_from=HOLDER`

---

## Stage 3: Long-Term Tracking

**Data ingested for this wallet:**

| Data type | Count |
|-----------|-------|
| trades_raw | 29 |
| activity_raw | 42 |
| positions_snapshots | 3 |

**What was applied:**
- Periodic jobs poll `/trades`, `/activity`, `/positions` for each tracked wallet
- Data stored for persona classification, paper tick logic, and WScore

---

## Stage 4: Paper Trading

**Paper trades (mirror strategy):**

| Market | Side | Size | Entry | Status | PnL |
|--------|------|------|-------|-------|-----|
| Will Bitcoin reach $90,000 in February? | BUY | $100 | 0.27 | open | - |
| (same) | BUY | $100 | 0.07 | open | - |
| (same) | BUY | $100 | 0.14 | open | - |
| (same) | BUY | $100 | 0.08 | open | - |
| (same) | BUY | $100 | 0.19 | open | - |

**What was applied:**
- Paper tick job detects source trades in `trades_raw`
- Mirror strategy: same side, proportional size (capped by risk)
- Risk caps: per-wallet exposure, portfolio exposure, bankroll limits
- 5 paper trades created (all open; markets not yet settled)

---

## Stage 5: Wallet Ranking (WScore)

**Wallet score:**

| Field | Value |
|-------|-------|
| WScore | 0.33 |
| Recommended follow mode | mirror |
| Score date | 2026-02-11 |

**What was applied:**
- WScore computed from edge, consistency, market skill, timing skill, behavior quality
- `recommended_follow_mode=mirror` → eligible for paper copy

---

## Wallet Rules State

| Field | Value |
|-------|-------|
| State | CANDIDATE |
| Last seen | 1770838337 |

---

## Summary: Pipeline Flow

```
Gamma API (markets)
    → MScore + EScore ranking (top 50 events)
    → Data API /holders (this market)
        → Wallet 0x0361...c736 discovered (HOLDER)
            → Ingestion: trades, activity, positions
                → Paper tick: mirror 5 trades
                    → WScore: 0.33, mirror
```

**View in dashboard:** `https://sniper.pkubelka.cz/journey/0x0361d34c84a5d41970aaac11f5de387841adc736`
