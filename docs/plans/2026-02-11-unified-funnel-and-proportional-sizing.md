# Unified Funnel Semantics + Active Paper Wallets + Proportional Sizing

## Implemented scope

This change set unifies dashboard semantics and paper sizing behavior:
- one combined funnel across market + wallet lifecycle
- one canonical meaning of followable wallets (`followable_now`)
- paper stage reflects currently active followable wallets
- paper sizing defaults to proportional mode with deterministic fallback

## Canonical definitions

### `followable_now`

A wallet is followable now when:
1. `wallets.is_active = 1`
2. wallet has a latest row in `wallet_personas`
3. latest `wallet_exclusions.excluded_at` is missing, or older than latest persona timestamp

### Unified funnel stages

1. Markets fetched
2. Markets scored today
3. Wallets discovered
4. Stage 1 passed
5. Stage 2 classified
6. Paper active (followable now)
7. Follow-worthy
8. Human approval (placeholder 0)
9. Live (placeholder 0)

UI invariants:
- every stage renders `processed/total`
- `unit change` marker is displayed on cross-unit transitions (market -> wallet)

## Paper sizing behavior

### Formula

- `their_size_usd = trades_raw.size * trades_raw.price`
- if proportional mode ON:
  - `our_size_usd = their_size_usd * (paper_trading.bankroll_usd / paper_trading.mirror_default_their_bankroll_usd)`
- else:
  - `our_size_usd = paper_trading.per_trade_size_usd`

### Fallback rules

- if source size/price invalid (`<=0`, non-finite), use `per_trade_size_usd`
- if `per_trade_size_usd <= 0`, use legacy `position_size_usdc`

## Configuration additions

In `[paper_trading]`:
- `mirror_use_proportional_sizing = true`
- `mirror_default_their_bankroll_usd = 5000.0`

## Verification

Validated with:
- `cargo test -p web`
- `cargo test -p evaluator`

Plus targeted tests:
- web funnel partial assertions for unified stages + processed/total + unit change marker
- evaluator sizing tests for proportional, flat mode, and invalid-input fallback
