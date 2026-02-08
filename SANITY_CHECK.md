# Strategy Sanity Check: Why This Should Succeed

## The Complete Closed-Loop System

Most competitors only do one piece (wallet tracking, copy bots, or dashboards). This system is the **only complete closed loop**:

- **Market selection → Wallet discovery → Long-term tracking → Risk-managed paper copy → Evidence-based ranking**
- Each stage validates the previous one. We don't just find wallets, we prove they work.

## Edge Detection, Not Speed Trading

**Core principle:** We find people who *know things*, not people who click fast

- **5-120 second delay** proves directional edge, not execution edge
- **Follower slippage tracking** ensures we don't lose even with realistic delays
- **PnL decomposition** separates execution edge (unreplicable) from directional edge (copyable)

## Risk Management That Actually Works

- **Two-level risk:** Per-wallet caps prevent one bad wallet from destroying portfolio
- **Real-time circuit breakers:** Error breaker, slippage breaker, correlation breaker
- **Portfolio stop-losses:** Daily/weekly caps, max drawdown limits
- **Copy fidelity tracking:** If we can't copy >80% of trades, paper PnL is unreliable

## Data Quality That Enables Replay

- **Per-row raw JSON stored** — `raw_json` column on trades_raw, activity_raw etc. enables re-parsing
- **Append-only storage** — never delete data, always replay
- **Deterministic replay** — every paper trade decision logged with reason
- **Follower slippage metrics** — the critical metric that determines if copy-trading works

## Persona-Based Selectivity

- **Only 3 followable personas:** Informed Specialist, Consistent Generalist, Patient Accumulator
- **Stage 1 fast filters:** Exclude wallets that can't possibly work (age, trades, activity)
- **Stage 2 deep analysis:** Detect execution masters, tail risk sellers, noise traders, snipers
- **Weekly re-evaluation:** Personas can change — we stop following if behavior shifts

## Competitive Differentiation

| What We Do | What Others Do |
|------------|----------------|
| **Complete closed loop** | One piece only |
| **Paper trading proof** | No validation |
| **Risk management first** | Risk afterthought |
| **Follower slippage tracking** | Assume perfect execution |
| **Persona-based selection** | No persona classification |
| **Per-row data replay** | Can't re-evaluate |

## Technical Excellence

- **Rust + Tokio:** Performance, type safety for money, compiled binaries
- **SQLite with WAL mode:** Zero-dependency, concurrent reads during writes
- **Prometheus metrics:** Real-time observability in Grafana
- **AWS t3.micro:** Same infrastructure as proven trading bots
- **TDD throughout:** Every feature has tests before implementation

## Scalability and Sustainability

- **Market selection for discovery only:** Copying is wallet-centric, not market-dependent
- **High funnel selectivity:** Discover 500+ wallets, follow 5-10 proven ones
- **Configurable thresholds:** No hardcoded decisions, everything flows from config
- **Continuous re-evaluation:** Never stop classifying, never stop proving

## The Critical Metric: Follower Slippage

```
follower_slippage = (our_avg_entry - their_avg_entry) + our_fees
```

If this consistently exceeds their edge, we lose even copying perfectly. We track this per wallet and kill wallets where slippage eats the edge.

## Why It Will Work

- **Proof before execution:** Paper trading proves edge survives delays and costs
- **Risk management prevents ruin:** Two-level caps, circuit breakers, stop-losses
- **Data quality enables learning:** Raw responses, deterministic replay, slippage tracking
- **Selectivity ensures quality:** Only 3 followable personas out of 8 total
- **Continuous validation:** Weekly re-evaluation, anomaly detection, persona changes

This isn't gambling on wallets — it's a scientific approach to finding people who consistently make profitable directional bets, proving those bets survive realistic execution constraints, and then following them with strict risk controls.

The system succeeds because it's not about finding lucky wallets — it's about finding reproducible edge and proving it works before risking real money.