# Plan: Add Polymarket Profile Links to All Wallet Tables

**Date:** 2026-02-08
**Goal:** Make every wallet address in the dashboard clickable, linking to `https://polymarket.com/profile/{address}` in a new tab. Add a small external-link icon next to each address.

## Context

The dashboard is read-only / observational. Currently wallet addresses are plain text. The user wants to quickly click through to inspect wallets on Polymarket without copy-pasting addresses.

Both the "mirror button doesn't work" and "link to polymarket account" requests are the same thing: make wallet addresses clickable links to their Polymarket profiles.

## Task 1: Add `proxy_wallet` field to `PaperTradeRow`

**File:** `crates/web/src/models.rs`

`PaperTradeRow` currently only has `wallet_short`. Add `proxy_wallet` so the template can construct the profile URL.

```rust
// Line 89 — add proxy_wallet field
pub struct PaperTradeRow {
    pub proxy_wallet: String,   // <-- ADD THIS
    pub wallet_short: String,
    // ... rest unchanged
}
```

## Task 2: Set `proxy_wallet` in `PaperTradeRow` construction

**File:** `crates/web/src/queries.rs`

In `recent_paper_trades()` around line 445, the full wallet address is already fetched into `wallet: String` (line 409). Just store it before shortening:

```rust
// Line 445-446 — change from:
Ok(PaperTradeRow {
    wallet_short: shorten_wallet(&wallet),

// To:
Ok(PaperTradeRow {
    proxy_wallet: wallet.clone(),
    wallet_short: shorten_wallet(&wallet),
```

## Task 3: Add Polymarket profile link to `wallets.html`

**File:** `crates/web/templates/partials/wallets.html`

Replace line 49:
```html
<!-- FROM: -->
<td class="py-1.5 px-2 font-mono text-gray-300" title="{{ w.proxy_wallet }}">{{ w.wallet_short }}</td>

<!-- TO: -->
<td class="py-1.5 px-2 font-mono" title="{{ w.proxy_wallet }}">
    <a href="https://polymarket.com/profile/{{ w.proxy_wallet }}" target="_blank" rel="noopener"
       class="inline-flex items-center gap-1 text-gray-300 hover:text-blue-400 transition-colors">
        {{ w.wallet_short }}
        <svg class="w-3 h-3 opacity-50" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 6H6a2 2 0 00-2 2v10a2 2 0 002 2h10a2 2 0 002-2v-4M14 4h6m0 0v6m0-6L10 14"/>
        </svg>
    </a>
</td>
```

## Task 4: Add Polymarket profile link to `rankings.html`

**File:** `crates/web/templates/partials/rankings.html`

Replace line 22:
```html
<!-- FROM: -->
<td class="py-1.5 px-2 font-mono text-gray-300" title="{{ r.proxy_wallet }}">{{ r.wallet_short }}</td>

<!-- TO: -->
<td class="py-1.5 px-2 font-mono" title="{{ r.proxy_wallet }}">
    <a href="https://polymarket.com/profile/{{ r.proxy_wallet }}" target="_blank" rel="noopener"
       class="inline-flex items-center gap-1 text-gray-300 hover:text-blue-400 transition-colors">
        {{ r.wallet_short }}
        <svg class="w-3 h-3 opacity-50" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 6H6a2 2 0 00-2 2v10a2 2 0 002 2h10a2 2 0 002-2v-4M14 4h6m0 0v6m0-6L10 14"/>
        </svg>
    </a>
</td>
```

## Task 5: Add Polymarket profile link to `paper.html`

**File:** `crates/web/templates/partials/paper.html`

Replace line 49:
```html
<!-- FROM: -->
<td class="py-1.5 px-2 font-mono text-gray-400">{{ t.wallet_short }}</td>

<!-- TO: -->
<td class="py-1.5 px-2 font-mono">
    <a href="https://polymarket.com/profile/{{ t.proxy_wallet }}" target="_blank" rel="noopener"
       class="inline-flex items-center gap-1 text-gray-400 hover:text-blue-400 transition-colors">
        {{ t.wallet_short }}
        <svg class="w-3 h-3 opacity-50" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 6H6a2 2 0 00-2 2v10a2 2 0 002 2h10a2 2 0 002-2v-4M14 4h6m0 0v6m0-6L10 14"/>
        </svg>
    </a>
</td>
```

## Task 6: Build and verify

```bash
cargo build --release
cargo test --all
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Skipped

- **`tracking.html`** — stale wallets section only has truncated short strings, no full address available. Not worth adding plumbing for warning badges.

## Summary

| File | Change |
|------|--------|
| `crates/web/src/models.rs` | Add `proxy_wallet: String` to `PaperTradeRow` |
| `crates/web/src/queries.rs` | Set `proxy_wallet: wallet.clone()` in `PaperTradeRow` construction |
| `crates/web/templates/partials/wallets.html` | Wrap wallet address in `<a>` link + icon |
| `crates/web/templates/partials/rankings.html` | Wrap wallet address in `<a>` link + icon |
| `crates/web/templates/partials/paper.html` | Wrap wallet address in `<a>` link + icon |

**Total: 2 Rust lines changed, 3 HTML templates updated. Zero new dependencies.**
