# Evaluator Frontend Dashboard — Implementation Plan

> **🗄️ ARCHIVED — COMPLETED**
> 
> All 15 tasks implemented and deployed. Dashboard is live at port 8080 (requires auth). Features: system status strip, funnel bar with drop-off rates, 5 stage sections (markets, wallets, tracking, paper, rankings), htmx polling, Tailwind dark mode.
> 
> **Current work:** See `../MASTER_STRATEGY_IMPLEMENTATION_PLAN.md` for active development.

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a live operations dashboard that visualizes the entire evaluator pipeline as a funnel — from market discovery through wallet ranking — so you can see what's happening at each stage, spot problems, and track system health at a glance.

**Architecture:** New `crates/web` binary crate using axum + askama (compile-time HTML templates) + htmx (CDN) + Tailwind CSS (CDN). Opens the evaluator's SQLite database in read-only mode (WAL allows concurrent reads). Each pipeline stage is a collapsible section on a single page, independently refreshed via htmx polling every 30s. No JavaScript files to maintain. No Node.js. Single binary deployed alongside the evaluator on the same t3.micro.

**Tech Stack:** axum (web framework), askama (Jinja2-like templates, compile-time), htmx (CDN, dynamic HTML fragment swaps), Tailwind CSS (CDN, dark mode), rusqlite (read-only DB access), common crate (shared config/DB types).

**Design decisions:**
- Separate crate (`crates/web`), not embedded in evaluator — independent restart, clean separation
- Single-page funnel layout — system status strip at top, funnel bar with counts/drop-off, then 5 collapsible stage sections
- htmx polling (30s) — simple, no WebSocket complexity, matches the hourly/daily job cadence
- Read-only DB access — dashboard never writes, evaluator owns the schema
- askama over tera — compile-time template checking, zero runtime template errors

---

## Progress

- [ ] Task 1: Workspace Setup — Add `crates/web` Crate
- [ ] Task 2: Axum Server Skeleton — Hello World
- [ ] Task 3: Base Template with Tailwind + htmx
- [ ] Task 4: Database Read-Only Connection + Query Module
- [ ] Task 5: System Status Strip (job heartbeats, DB size, phase)
- [ ] Task 6: Funnel Summary Bar (counts + drop-off rates)
- [ ] Task 7: Stage 1 — Market Discovery Section
- [ ] Task 8: Stage 2 — Wallet Discovery Section
- [ ] Task 9: Stage 3 — Long-Term Tracking Section
- [ ] Task 10: Stage 4 — Paper Copy Engine Section
- [ ] Task 11: Stage 5 — Wallet Rankings Section
- [ ] Task 12: htmx Polling for All Sections
- [ ] Task 13: Config Integration (web port, DB path)
- [ ] Task 14: Deploy Script Update
- [ ] Task 15: Integration Test — Dashboard Renders with Real DB

---

## Task 1: Workspace Setup — Add `crates/web` Crate

**Files:**
- Modify: `Cargo.toml` (workspace root — add `crates/web` to members)
- Create: `crates/web/Cargo.toml`
- Create: `crates/web/src/main.rs`

**Step 1: Add workspace member**

In `Cargo.toml` workspace root, add `"crates/web"` to the members list. Also add `axum` and `askama` to workspace dependencies:

```toml
[workspace]
resolver = "2"
members = [
    "crates/common",
    "crates/evaluator",
    "crates/web",
]

[workspace.dependencies]
# ... existing deps ...
axum = "0.8"
askama = { version = "0.13", features = ["with-axum"] }
askama_axum = "0.5"
tower-http = { version = "0.6", features = ["fs"] }
```

**Step 2: Create crate Cargo.toml**

Create `crates/web/Cargo.toml`:

```toml
[package]
name = "web"
version = "0.1.0"
edition = "2021"

[dependencies]
common = { path = "../common" }
axum = { workspace = true }
askama = { workspace = true }
askama_axum = { workspace = true }
tower-http = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
rusqlite = { workspace = true }
anyhow = { workspace = true }
chrono = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
```

**Step 3: Create minimal main.rs**

Create `crates/web/src/main.rs`:

```rust
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("evaluator dashboard starting");
    Ok(())
}
```

**Step 4: Verify it compiles**

Run: `cargo build -p web`
Expected: compiles successfully, no errors.

**Step 5: Commit**

```bash
git add crates/web/ Cargo.toml
git commit -m "feat(web): add crates/web skeleton for dashboard"
```

---

## Task 2: Axum Server Skeleton — Hello World

**Files:**
- Modify: `crates/web/src/main.rs`

**Step 1: Write a test that the server starts and responds**

Add to `crates/web/src/main.rs`:

```rust
use axum::{Router, routing::get, response::Html};
use std::net::SocketAddr;
use anyhow::Result;

async fn index() -> Html<&'static str> {
    Html("<h1>Evaluator Dashboard</h1>")
}

pub fn create_router() -> Router {
    Router::new()
        .route("/", get(index))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let app = create_router();
    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("dashboard listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // for oneshot

    #[tokio::test]
    async fn test_index_returns_200() {
        let app = create_router();
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
```

**Step 2: Run the test**

Run: `cargo test -p web`
Expected: PASS — `test_index_returns_200` passes.

**Step 3: Verify server starts**

Run: `cargo run -p web` (Ctrl+C after seeing "dashboard listening on 0.0.0.0:8080")

**Step 4: Commit**

```bash
git add crates/web/src/main.rs
git commit -m "feat(web): axum hello world with test"
```

---

## Task 3: Base Template with Tailwind + htmx

**Files:**
- Create: `crates/web/templates/base.html`
- Create: `crates/web/templates/dashboard.html`
- Modify: `crates/web/src/main.rs`

**Step 1: Create the base template**

Create `crates/web/templates/base.html`:

```html
<!DOCTYPE html>
<html lang="en" class="dark">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{% block title %}Evaluator Dashboard{% endblock %}</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <script src="https://unpkg.com/htmx.org@2.0.4"></script>
    <script>
        tailwind.config = {
            darkMode: 'class',
        }
    </script>
    <style>
        body { font-family: ui-monospace, SFMono-Regular, 'SF Mono', Menlo, monospace; }
    </style>
</head>
<body class="bg-gray-950 text-gray-100 min-h-screen">
    <div class="max-w-7xl mx-auto px-4 py-6">
        <header class="mb-6">
            <h1 class="text-2xl font-bold text-gray-100">Trader Evaluator</h1>
            <p class="text-sm text-gray-500">Pipeline Dashboard</p>
        </header>
        <main>
            {% block content %}{% endblock %}
        </main>
    </div>
</body>
</html>
```

**Step 2: Create the dashboard template**

Create `crates/web/templates/dashboard.html`:

```html
{% extends "base.html" %}

{% block content %}
<div class="space-y-6">
    <p class="text-gray-400">Dashboard loading...</p>
</div>
{% endblock %}
```

**Step 3: Update main.rs to serve askama templates**

Replace the `index` handler:

```rust
use askama::Template;

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {}

async fn index() -> DashboardTemplate {
    DashboardTemplate {}
}
```

Update the router to use this handler (askama_axum provides the `IntoResponse` impl automatically).

**Step 4: Run test**

Run: `cargo test -p web`
Expected: PASS — the index route still returns 200, now with the full HTML template.

**Step 5: Commit**

```bash
git add crates/web/templates/ crates/web/src/main.rs
git commit -m "feat(web): base template with Tailwind + htmx CDN"
```

---

## Task 4: Database Read-Only Connection + Query Module

**Files:**
- Create: `crates/web/src/queries.rs`
- Create: `crates/web/src/models.rs`
- Modify: `crates/web/src/main.rs`

This task sets up the DB connection pool (shared via axum state) and the query module. All queries are read-only.

**Step 1: Create view models**

Create `crates/web/src/models.rs` with structs for what each dashboard section needs:

```rust
/// Funnel counts for the summary bar
pub struct FunnelCounts {
    pub markets_fetched: i64,      // total markets in DB
    pub markets_scored: i64,       // markets scored today
    pub wallets_discovered: i64,   // total wallets
    pub wallets_active: i64,       // wallets with is_active=1
    pub paper_trades_total: i64,   // total paper trades
    pub wallets_ranked: i64,       // wallets with scores today
}

/// System status info
pub struct SystemStatus {
    pub db_size_mb: f64,
    pub last_market_scoring: Option<String>,
    pub last_wallet_discovery: Option<String>,
    pub last_trade_ingestion: Option<String>,
    pub last_activity_ingestion: Option<String>,
    pub last_position_snapshot: Option<String>,
    pub last_holder_snapshot: Option<String>,
    pub last_paper_tick: Option<String>,
    pub last_wallet_scoring: Option<String>,
}

/// Row in the top markets table
pub struct MarketRow {
    pub rank: i64,
    pub title: String,
    pub condition_id: String,
    pub mscore: f64,
    pub liquidity: f64,
    pub volume: f64,
    pub density_score: f64,
    pub end_date: Option<String>,
}

/// Row in the wallets table
pub struct WalletRow {
    pub proxy_wallet: String,
    pub discovered_from: String,
    pub discovered_market_title: Option<String>,
    pub discovered_at: String,
    pub is_active: bool,
    pub trade_count: i64,
}

/// Tracking health per data type
pub struct TrackingHealth {
    pub data_type: String,
    pub count_last_1h: i64,
    pub count_last_24h: i64,
    pub last_ingested: Option<String>,
}

/// Paper trade row
pub struct PaperTradeRow {
    pub wallet: String,
    pub market_title: Option<String>,
    pub side: String,
    pub size_usdc: f64,
    pub entry_price: f64,
    pub status: String,
    pub pnl: Option<f64>,
    pub created_at: String,
}

/// Paper portfolio summary
pub struct PaperSummary {
    pub total_pnl: f64,
    pub open_positions: i64,
    pub settled_wins: i64,
    pub settled_losses: i64,
    pub bankroll: f64,
}

/// Wallet ranking row
pub struct RankingRow {
    pub rank: i64,
    pub proxy_wallet: String,
    pub wscore: f64,
    pub edge_score: f64,
    pub consistency_score: f64,
    pub trade_count: i64,
    pub paper_pnl: f64,
    pub follow_mode: Option<String>,
}
```

**Step 2: Create query module with funnel counts**

Create `crates/web/src/queries.rs`:

```rust
use rusqlite::Connection;
use anyhow::Result;
use crate::models::*;

pub fn funnel_counts(conn: &Connection) -> Result<FunnelCounts> {
    let markets_fetched: i64 = conn.query_row(
        "SELECT COUNT(*) FROM markets", [], |r| r.get(0)
    )?;
    let markets_scored: i64 = conn.query_row(
        "SELECT COUNT(*) FROM market_scores_daily WHERE score_date = date('now')",
        [], |r| r.get(0)
    )?;
    let wallets_discovered: i64 = conn.query_row(
        "SELECT COUNT(*) FROM wallets", [], |r| r.get(0)
    )?;
    let wallets_active: i64 = conn.query_row(
        "SELECT COUNT(*) FROM wallets WHERE is_active = 1", [], |r| r.get(0)
    )?;
    let paper_trades_total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM paper_trades", [], |r| r.get(0)
    )?;
    let wallets_ranked: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT proxy_wallet) FROM wallet_scores_daily WHERE score_date = date('now')",
        [], |r| r.get(0)
    )?;
    Ok(FunnelCounts {
        markets_fetched,
        markets_scored,
        wallets_discovered,
        wallets_active,
        paper_trades_total,
        wallets_ranked,
    })
}
```

Add all other query functions: `system_status()`, `top_markets_today()`, `recent_wallets()`, `tracking_health()`, `paper_summary()`, `recent_paper_trades()`, `top_rankings()`.

Each function takes `&Connection`, runs a SQL query, maps rows into model structs.

**Step 3: Wire DB into axum state**

In `main.rs`, add shared state:

```rust
use std::sync::Arc;
use std::path::PathBuf;

struct AppState {
    db_path: PathBuf,
}

// Each request opens a fresh read-only connection (SQLite WAL handles this well)
fn open_readonly(state: &AppState) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        &state.db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    Ok(conn)
}
```

Use `axum::extract::State<Arc<AppState>>` in handlers.

**Step 4: Write test with in-memory DB**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use common::db::Database;

    fn test_db() -> Connection {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();
        db.conn
    }

    #[test]
    fn test_funnel_counts_empty_db() {
        let conn = test_db();
        let counts = queries::funnel_counts(&conn).unwrap();
        assert_eq!(counts.markets_fetched, 0);
        assert_eq!(counts.wallets_discovered, 0);
    }

    #[test]
    fn test_funnel_counts_with_data() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO markets (condition_id, title) VALUES ('0xabc', 'Test Market')",
            [],
        ).unwrap();
        let counts = queries::funnel_counts(&conn).unwrap();
        assert_eq!(counts.markets_fetched, 1);
    }
}
```

**Step 5: Run tests**

Run: `cargo test -p web`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/web/src/
git commit -m "feat(web): DB queries + view models for dashboard"
```

---

## Task 5: System Status Strip (job heartbeats, DB size, phase)

**Files:**
- Create: `crates/web/templates/partials/status_strip.html`
- Modify: `crates/web/src/queries.rs` (add `system_status()` query)
- Modify: `crates/web/src/main.rs` (add `/partials/status` route)

**Step 1: Write query for system status**

Add `system_status()` to `queries.rs`. It queries `raw_api_responses` for the latest fetch time per API endpoint to determine job freshness. Uses `PRAGMA page_count * PRAGMA page_size` for DB size.

```rust
pub fn system_status(conn: &Connection, db_path: &str) -> Result<SystemStatus> {
    // DB file size
    let db_size_mb = std::fs::metadata(db_path)
        .map(|m| m.len() as f64 / 1_048_576.0)
        .unwrap_or(0.0);

    // Last market scoring: latest entry in market_scores_daily
    let last_market_scoring: Option<String> = conn.query_row(
        "SELECT MAX(score_date) FROM market_scores_daily",
        [], |r| r.get(0)
    ).unwrap_or(None);

    // Last trade ingestion: latest ingested_at in trades_raw
    let last_trade_ingestion: Option<String> = conn.query_row(
        "SELECT MAX(ingested_at) FROM trades_raw",
        [], |r| r.get(0)
    ).unwrap_or(None);

    // ... similar for each job type

    Ok(SystemStatus {
        db_size_mb,
        last_market_scoring,
        last_wallet_discovery: None, // derived from wallets.discovered_at
        last_trade_ingestion,
        last_activity_ingestion: None,
        last_position_snapshot: None,
        last_holder_snapshot: None,
        last_paper_tick: None,
        last_wallet_scoring: None,
    })
}
```

**Step 2: Create status strip template**

Create `crates/web/templates/partials/status_strip.html`:

```html
<div class="flex items-center gap-4 p-3 bg-gray-900 rounded-lg text-sm">
    <div class="flex items-center gap-2">
        <span class="text-gray-500">Jobs:</span>
        {% for job in jobs %}
        <div class="flex items-center gap-1" title="{{ job.name }}: {{ job.last_run }}">
            <span class="w-2 h-2 rounded-full {{ job.color }}"></span>
            <span class="text-xs text-gray-400">{{ job.short_name }}</span>
        </div>
        {% endfor %}
    </div>
    <div class="ml-auto flex items-center gap-4 text-gray-500">
        <span>DB: {{ db_size_mb }} MB</span>
        <span>Phase: {{ phase }}</span>
    </div>
</div>
```

Job dots: green (`bg-green-500`) if last run < 2x interval, yellow (`bg-yellow-500`) if < 3x, red (`bg-red-500`) otherwise.

**Step 3: Add route**

```rust
async fn status_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let status = queries::system_status(&conn, state.db_path.to_str().unwrap()).unwrap();
    StatusStripTemplate { status }
}

app.route("/partials/status", get(status_partial))
```

**Step 4: Test**

```rust
#[tokio::test]
async fn test_status_partial_returns_200() {
    // Use test app with in-memory DB
    let app = create_test_app();
    let response = app.oneshot(
        Request::builder().uri("/partials/status").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
```

**Step 5: Commit**

```bash
git add crates/web/
git commit -m "feat(web): system status strip with job heartbeats"
```

---

## Task 6: Funnel Summary Bar (counts + drop-off rates)

**Files:**
- Create: `crates/web/templates/partials/funnel_bar.html`
- Modify: `crates/web/src/main.rs` (add `/partials/funnel` route)

**Step 1: Create funnel bar template**

The funnel bar renders as a horizontal row of connected boxes, each showing count and drop-off percentage to the next stage:

```html
<div class="flex items-center gap-2 p-4 bg-gray-900 rounded-lg overflow-x-auto">
    {% for stage in stages %}
    <div class="flex-shrink-0 text-center px-4 py-2 rounded {{ stage.bg_color }}">
        <div class="text-lg font-bold">{{ stage.count }}</div>
        <div class="text-xs text-gray-400">{{ stage.label }}</div>
    </div>
    {% if !loop.last %}
    <div class="flex-shrink-0 text-center">
        <div class="text-gray-600">→</div>
        <div class="text-xs {{ stage.drop_color }}">{{ stage.drop_pct }}%</div>
    </div>
    {% endif %}
    {% endfor %}
</div>
```

Stages: Markets Fetched → Markets Scored → Wallets Discovered → Wallets Tracked → Paper Trades → Wallets Ranked.

Drop-off colors: green (>50%), yellow (10-50%), red (<10%).

**Step 2: Add route handler**

```rust
async fn funnel_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let counts = queries::funnel_counts(&conn).unwrap();
    FunnelBarTemplate::from(counts)
}
```

**Step 3: Test**

```rust
#[test]
fn test_funnel_drop_off_calculation() {
    // 100 markets, 20 scored = 20% conversion
    let counts = FunnelCounts {
        markets_fetched: 100,
        markets_scored: 20,
        wallets_discovered: 50,
        wallets_active: 40,
        paper_trades_total: 5,
        wallets_ranked: 3,
    };
    let stages = compute_funnel_stages(&counts);
    assert_eq!(stages[0].drop_pct, "20.0");  // 20/100
    assert_eq!(stages[2].drop_pct, "80.0");  // 40/50
}
```

**Step 4: Commit**

```bash
git add crates/web/
git commit -m "feat(web): funnel summary bar with drop-off rates"
```

---

## Task 7: Stage 1 — Market Discovery Section

**Files:**
- Create: `crates/web/templates/partials/markets.html`
- Modify: `crates/web/src/queries.rs` (add `top_markets_today()`)
- Modify: `crates/web/src/main.rs` (add `/partials/markets` route)

**Step 1: Write query**

```rust
pub fn top_markets_today(conn: &Connection) -> Result<Vec<MarketRow>> {
    let mut stmt = conn.prepare(
        "SELECT ms.rank, m.title, ms.condition_id, ms.mscore,
                COALESCE(m.liquidity, 0), COALESCE(m.volume, 0),
                COALESCE(ms.density_score, 0), m.end_date
         FROM market_scores_daily ms
         JOIN markets m ON m.condition_id = ms.condition_id
         WHERE ms.score_date = date('now')
         ORDER BY ms.rank ASC
         LIMIT 20"
    )?;
    // ... map rows to MarketRow
}
```

**Step 2: Create template**

Table with: Rank, Title (truncated to 50 chars), MScore (bar visualization), Liquidity, Volume, Density, Expiry. Collapsible via `<details>` tag with summary showing "Stage 1: Market Discovery (20 scored today)".

**Step 3: Add route, wire into dashboard.html**

**Step 4: Test with seeded DB**

```rust
#[test]
fn test_top_markets_empty_db() {
    let conn = test_db();
    let markets = queries::top_markets_today(&conn).unwrap();
    assert!(markets.is_empty());
}

#[test]
fn test_top_markets_with_data() {
    let conn = test_db();
    // Insert a market + score for today
    conn.execute(
        "INSERT INTO markets (condition_id, title) VALUES ('0xabc', 'BTC > 100k')",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO market_scores_daily (condition_id, score_date, mscore, rank)
         VALUES ('0xabc', date('now'), 0.85, 1)",
        [],
    ).unwrap();
    let markets = queries::top_markets_today(&conn).unwrap();
    assert_eq!(markets.len(), 1);
    assert_eq!(markets[0].title, "BTC > 100k");
}
```

**Step 5: Commit**

```bash
git add crates/web/
git commit -m "feat(web): market discovery section with top-20 table"
```

---

## Task 8: Stage 2 — Wallet Discovery Section

**Files:**
- Create: `crates/web/templates/partials/wallets.html`
- Modify: `crates/web/src/queries.rs` (add `wallet_overview()`, `recent_wallets()`)
- Modify: `crates/web/src/main.rs` (add `/partials/wallets` route)

**Step 1: Write queries**

```rust
/// Wallet counts by discovery source
pub struct WalletOverview {
    pub total: i64,
    pub active: i64,
    pub from_holder: i64,
    pub from_trader: i64,
    pub from_leaderboard: i64,
    pub discovered_today: i64,
}

pub fn wallet_overview(conn: &Connection) -> Result<WalletOverview> { ... }

/// Most recently discovered wallets (last 20)
pub fn recent_wallets(conn: &Connection, limit: usize) -> Result<Vec<WalletRow>> {
    let mut stmt = conn.prepare(
        "SELECT w.proxy_wallet, w.discovered_from,
                m.title, w.discovered_at, w.is_active,
                (SELECT COUNT(*) FROM trades_raw t WHERE t.proxy_wallet = w.proxy_wallet)
         FROM wallets w
         LEFT JOIN markets m ON m.condition_id = w.discovered_market
         ORDER BY w.discovered_at DESC
         LIMIT ?1"
    )?;
    // ... map to WalletRow
}
```

**Step 2: Create template**

Shows: overview cards (total, active, by source, discovered today) + table of recent wallets (address truncated `0xab..cd`, source, market, date, active?, trade count).

**Step 3: Test**

```rust
#[test]
fn test_wallet_overview_counts_sources() {
    let conn = test_db();
    conn.execute(
        "INSERT INTO wallets (proxy_wallet, discovered_from) VALUES ('0x1', 'HOLDER')", [],
    ).unwrap();
    conn.execute(
        "INSERT INTO wallets (proxy_wallet, discovered_from) VALUES ('0x2', 'TRADER_RECENT')", [],
    ).unwrap();
    let overview = queries::wallet_overview(&conn).unwrap();
    assert_eq!(overview.total, 2);
    assert_eq!(overview.from_holder, 1);
    assert_eq!(overview.from_trader, 1);
}
```

**Step 4: Commit**

```bash
git add crates/web/
git commit -m "feat(web): wallet discovery section with source breakdown"
```

---

## Task 9: Stage 3 — Long-Term Tracking Section

**Files:**
- Create: `crates/web/templates/partials/tracking.html`
- Modify: `crates/web/src/queries.rs` (add `tracking_health()`)
- Modify: `crates/web/src/main.rs` (add `/partials/tracking` route)

**Step 1: Write queries**

```rust
pub fn tracking_health(conn: &Connection) -> Result<Vec<TrackingHealth>> {
    let data_types = vec![
        ("Trades", "trades_raw", "ingested_at"),
        ("Activity", "activity_raw", "ingested_at"),
        ("Positions", "positions_snapshots", "snapshot_at"),
        ("Holders", "holders_snapshots", "snapshot_at"),
    ];

    let mut result = Vec::new();
    for (label, table, ts_col) in data_types {
        let count_1h: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE {} > datetime('now', '-1 hour')",
                table, ts_col
            ),
            [], |r| r.get(0)
        )?;
        let count_24h: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE {} > datetime('now', '-1 day')",
                table, ts_col
            ),
            [], |r| r.get(0)
        )?;
        let last: Option<String> = conn.query_row(
            &format!("SELECT MAX({}) FROM {}", ts_col, table),
            [], |r| r.get(0)
        ).unwrap_or(None);
        result.push(TrackingHealth {
            data_type: label.to_string(),
            count_last_1h: count_1h,
            count_last_24h: count_24h,
            last_ingested: last,
        });
    }
    Ok(result)
}

/// Wallets with no data in >24h (gap detection)
pub fn stale_wallets(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT w.proxy_wallet FROM wallets w
         WHERE w.is_active = 1
         AND NOT EXISTS (
             SELECT 1 FROM trades_raw t
             WHERE t.proxy_wallet = w.proxy_wallet
             AND t.ingested_at > datetime('now', '-1 day')
         )
         LIMIT 20"
    )?;
    // ... collect wallet addresses
}
```

**Step 2: Create template**

Grid of 4 cards (trades, activity, positions, holders) each showing: count in last 1h, count in last 24h, last ingested timestamp. Color: green if recent, red if stale. Below: list of stale wallets (if any) as a warning.

**Step 3: Test**

```rust
#[test]
fn test_tracking_health_empty() {
    let conn = test_db();
    let health = queries::tracking_health(&conn).unwrap();
    assert_eq!(health.len(), 4);
    assert_eq!(health[0].count_last_24h, 0);
}
```

**Step 4: Commit**

```bash
git add crates/web/
git commit -m "feat(web): tracking health section with gap detection"
```

---

## Task 10: Stage 4 — Paper Copy Engine Section

**Files:**
- Create: `crates/web/templates/partials/paper.html`
- Modify: `crates/web/src/queries.rs` (add `paper_summary()`, `recent_paper_trades()`)
- Modify: `crates/web/src/main.rs` (add `/partials/paper` route)

**Step 1: Write queries**

```rust
pub fn paper_summary(conn: &Connection, bankroll: f64) -> Result<PaperSummary> {
    let total_pnl: f64 = conn.query_row(
        "SELECT COALESCE(SUM(pnl), 0) FROM paper_trades WHERE status != 'open'",
        [], |r| r.get(0)
    )?;
    let open_positions: i64 = conn.query_row(
        "SELECT COUNT(*) FROM paper_trades WHERE status = 'open'",
        [], |r| r.get(0)
    )?;
    let settled_wins: i64 = conn.query_row(
        "SELECT COUNT(*) FROM paper_trades WHERE status = 'settled_win'",
        [], |r| r.get(0)
    )?;
    let settled_losses: i64 = conn.query_row(
        "SELECT COUNT(*) FROM paper_trades WHERE status = 'settled_loss'",
        [], |r| r.get(0)
    )?;
    Ok(PaperSummary {
        total_pnl,
        open_positions,
        settled_wins,
        settled_losses,
        bankroll,
    })
}

pub fn recent_paper_trades(conn: &Connection, limit: usize) -> Result<Vec<PaperTradeRow>> {
    let mut stmt = conn.prepare(
        "SELECT pt.proxy_wallet, m.title, pt.side, pt.size_usdc,
                pt.entry_price, pt.status, pt.pnl, pt.created_at
         FROM paper_trades pt
         LEFT JOIN markets m ON m.condition_id = pt.condition_id
         ORDER BY pt.created_at DESC
         LIMIT ?1"
    )?;
    // ... map to PaperTradeRow
}
```

**Step 2: Create template**

Summary cards: Total PnL (green/red), Open Positions, Win/Loss ratio, Bankroll. Table of recent trades: wallet, market, side, size, entry, status, PnL.

**Step 3: Test with seeded data**

```rust
#[test]
fn test_paper_summary_calculates_pnl() {
    let conn = test_db();
    conn.execute(
        "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl)
         VALUES ('0x1', 'mirror', '0xm1', 'BUY', 100.0, 0.60, 'settled_win', 25.0)",
        [],
    ).unwrap();
    let summary = queries::paper_summary(&conn, 10000.0).unwrap();
    assert_eq!(summary.total_pnl, 25.0);
    assert_eq!(summary.settled_wins, 1);
}
```

**Step 4: Commit**

```bash
git add crates/web/
git commit -m "feat(web): paper copy engine section with PnL summary"
```

---

## Task 11: Stage 5 — Wallet Rankings Section

**Files:**
- Create: `crates/web/templates/partials/rankings.html`
- Modify: `crates/web/src/queries.rs` (add `top_rankings()`)
- Modify: `crates/web/src/main.rs` (add `/partials/rankings` route)

**Step 1: Write query**

```rust
pub fn top_rankings(conn: &Connection, window_days: i64, limit: usize) -> Result<Vec<RankingRow>> {
    let mut stmt = conn.prepare(
        "SELECT ws.proxy_wallet, ws.wscore, ws.edge_score,
                ws.consistency_score, ws.recommended_follow_mode,
                (SELECT COUNT(*) FROM trades_raw t WHERE t.proxy_wallet = ws.proxy_wallet),
                COALESCE((SELECT SUM(pnl) FROM paper_trades pt
                          WHERE pt.proxy_wallet = ws.proxy_wallet AND pt.status != 'open'), 0)
         FROM wallet_scores_daily ws
         WHERE ws.score_date = date('now') AND ws.window_days = ?1
         ORDER BY ws.wscore DESC
         LIMIT ?2"
    )?;
    // ... map to RankingRow with computed rank (1-indexed)
}
```

**Step 2: Create template**

Table: Rank, Wallet (truncated), WScore (with bar), Edge, Consistency, Trades, Paper PnL, Follow Mode. Highlight top-3 with different bg color. Show window selector (7d / 30d / 90d) — each links to the same route with a query param.

**Step 3: Test**

```rust
#[test]
fn test_rankings_ordered_by_wscore() {
    let conn = test_db();
    // Insert two wallets with different scores
    conn.execute(
        "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, edge_score, consistency_score)
         VALUES ('0x1', date('now'), 30, 0.80, 0.9, 0.7)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO wallet_scores_daily (proxy_wallet, score_date, window_days, wscore, edge_score, consistency_score)
         VALUES ('0x2', date('now'), 30, 0.60, 0.5, 0.7)",
        [],
    ).unwrap();
    let rankings = queries::top_rankings(&conn, 30, 10).unwrap();
    assert_eq!(rankings.len(), 2);
    assert_eq!(rankings[0].proxy_wallet, "0x1"); // higher score first
    assert!(rankings[0].wscore > rankings[1].wscore);
}
```

**Step 4: Commit**

```bash
git add crates/web/
git commit -m "feat(web): wallet rankings section with WScore table"
```

---

## Task 12: htmx Polling for All Sections

**Files:**
- Modify: `crates/web/templates/dashboard.html`

**Step 1: Update dashboard template to include all sections with htmx polling**

Each section loads initially as a server-rendered fragment, then polls for updates:

```html
{% extends "base.html" %}

{% block content %}
<div class="space-y-6">
    <!-- Status Strip: polls every 30s -->
    <div hx-get="/partials/status" hx-trigger="load, every 30s" hx-swap="innerHTML">
        <p class="text-gray-600 text-sm">Loading status...</p>
    </div>

    <!-- Funnel Bar: polls every 30s -->
    <div hx-get="/partials/funnel" hx-trigger="load, every 30s" hx-swap="innerHTML">
        <p class="text-gray-600 text-sm">Loading funnel...</p>
    </div>

    <!-- Stage 1: Markets -->
    <details open>
        <summary class="cursor-pointer text-lg font-semibold text-gray-200 mb-2">
            Stage 1: Market Discovery
        </summary>
        <div hx-get="/partials/markets" hx-trigger="load, every 60s" hx-swap="innerHTML">
            <p class="text-gray-600 text-sm">Loading markets...</p>
        </div>
    </details>

    <!-- Stage 2: Wallets -->
    <details open>
        <summary class="cursor-pointer text-lg font-semibold text-gray-200 mb-2">
            Stage 2: Wallet Discovery
        </summary>
        <div hx-get="/partials/wallets" hx-trigger="load, every 60s" hx-swap="innerHTML">
            <p class="text-gray-600 text-sm">Loading wallets...</p>
        </div>
    </details>

    <!-- Stage 3: Tracking -->
    <details open>
        <summary class="cursor-pointer text-lg font-semibold text-gray-200 mb-2">
            Stage 3: Long-Term Tracking
        </summary>
        <div hx-get="/partials/tracking" hx-trigger="load, every 30s" hx-swap="innerHTML">
            <p class="text-gray-600 text-sm">Loading tracking...</p>
        </div>
    </details>

    <!-- Stage 4: Paper Copy -->
    <details open>
        <summary class="cursor-pointer text-lg font-semibold text-gray-200 mb-2">
            Stage 4: Paper Copy Engine
        </summary>
        <div hx-get="/partials/paper" hx-trigger="load, every 30s" hx-swap="innerHTML">
            <p class="text-gray-600 text-sm">Loading paper trades...</p>
        </div>
    </details>

    <!-- Stage 5: Rankings -->
    <details open>
        <summary class="cursor-pointer text-lg font-semibold text-gray-200 mb-2">
            Stage 5: Wallet Rankings
        </summary>
        <div hx-get="/partials/rankings" hx-trigger="load, every 60s" hx-swap="innerHTML">
            <p class="text-gray-600 text-sm">Loading rankings...</p>
        </div>
    </details>
</div>
{% endblock %}
```

**Step 2: Test that all partial routes return 200**

```rust
#[tokio::test]
async fn test_all_partials_return_200() {
    let app = create_test_app();
    let routes = vec![
        "/partials/status",
        "/partials/funnel",
        "/partials/markets",
        "/partials/wallets",
        "/partials/tracking",
        "/partials/paper",
        "/partials/rankings",
    ];
    for route in routes {
        let response = app.clone().oneshot(
            Request::builder().uri(route).body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "Failed: {}", route);
    }
}
```

**Step 3: Commit**

```bash
git add crates/web/templates/
git commit -m "feat(web): htmx polling for all dashboard sections"
```

---

## Task 13: Config Integration (web port, DB path)

**Files:**
- Modify: `crates/common/src/config.rs` (add optional `[web]` section)
- Modify: `config/default.toml` (add `[web]` section)
- Modify: `crates/web/src/main.rs` (read config for port + DB path)

**Step 1: Add web config section**

In `config.rs`, add:

```rust
#[derive(Debug, Deserialize)]
pub struct Web {
    pub port: u16,
    pub host: String,
}
```

Add `pub web: Option<Web>` to `Config` (optional so the evaluator crate doesn't require it).

In `default.toml`, add:

```toml
[web]
port = 8080
host = "0.0.0.0"
```

**Step 2: Use config in main.rs**

```rust
let config = common::config::Config::load()?;
let db_path = config.database.path.clone();
let web = config.web.unwrap_or(Web { port: 8080, host: "0.0.0.0".to_string() });
let addr = SocketAddr::new(web.host.parse()?, web.port);
```

**Step 3: Test config parsing**

```rust
#[test]
fn test_config_with_web_section() {
    let toml = r#"
    [general]
    mode = "paper"
    log_level = "info"
    ... // all existing sections
    [web]
    port = 9090
    host = "127.0.0.1"
    "#;
    let config: Config = toml::from_str(toml).unwrap();
    assert_eq!(config.web.unwrap().port, 9090);
}
```

**Step 4: Commit**

```bash
git add crates/common/src/config.rs config/default.toml crates/web/src/main.rs
git commit -m "feat(web): config integration for web port and DB path"
```

---

## Task 14: Deploy Script Update

**Files:**
- Modify: `deploy/deploy.sh` (add web binary to deploy)
- Create: `deploy/systemd/web.service`

**Step 1: Add web service systemd unit**

Create `deploy/systemd/web.service`:

```ini
[Unit]
Description=Trader Evaluator Dashboard
After=network.target evaluator.service
Wants=evaluator.service

[Service]
Type=simple
User=evaluator
WorkingDirectory=/opt/evaluator
ExecStart=/opt/evaluator/web
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

**Step 2: Update deploy.sh**

Add `web` binary to the cross-compile and scp steps alongside `evaluator`.

**Step 3: Commit**

```bash
git add deploy/
git commit -m "feat(web): systemd service + deploy script for dashboard"
```

---

## Task 15: Integration Test — Dashboard Renders with Real DB

**Files:**
- Modify: `crates/web/src/main.rs` (or a separate integration test file)

**Step 1: Write integration test**

This test opens the real `data/evaluator.db` (if it exists) and verifies the dashboard renders with actual data. Marked `#[ignore]` for CI.

```rust
#[tokio::test]
#[ignore] // requires real DB at data/evaluator.db
async fn test_dashboard_with_real_db() {
    let db_path = "data/evaluator.db";
    if !std::path::Path::new(db_path).exists() {
        eprintln!("Skipping: no real DB found");
        return;
    }

    let state = Arc::new(AppState {
        db_path: db_path.into(),
    });
    let app = create_router_with_state(state);

    // Full page
    let resp = app.clone().oneshot(
        Request::builder().uri("/").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // All partials
    for route in &["/partials/status", "/partials/funnel", "/partials/markets",
                   "/partials/wallets", "/partials/tracking", "/partials/paper",
                   "/partials/rankings"] {
        let resp = app.clone().oneshot(
            Request::builder().uri(*route).body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Failed: {}", route);
    }
}
```

**Step 2: Run against real DB**

Run: `cargo test -p web -- --ignored`
Expected: All routes return 200, no panics from SQL queries.

**Step 3: Final verification**

Run: `cargo test --all && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: All green.

**Step 4: Commit**

```bash
git add crates/web/
git commit -m "test(web): integration test with real evaluator DB"
```

---

## Summary

| Task | What | Key Output |
|------|------|------------|
| 1 | Workspace setup | `crates/web` crate compiles |
| 2 | Axum skeleton | Server starts, `/` returns 200 |
| 3 | Base template | Tailwind dark mode + htmx CDN |
| 4 | DB queries | Read-only queries for all sections |
| 5 | Status strip | Job heartbeats, DB size, phase |
| 6 | Funnel bar | Counts + drop-off percentages |
| 7 | Markets | Top-20 MScore table |
| 8 | Wallets | Discovery source breakdown |
| 9 | Tracking | Ingestion health + gap detection |
| 10 | Paper | PnL summary + recent trades |
| 11 | Rankings | WScore leaderboard |
| 12 | htmx polling | All sections auto-refresh |
| 13 | Config | Web port from `default.toml` |
| 14 | Deploy | Systemd service + deploy script |
| 15 | Integration | Dashboard renders with real DB |
