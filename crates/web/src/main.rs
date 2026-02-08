mod models;
mod queries;

use anyhow::Result;
use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::{routing::get, Router};
use models::{
    FunnelStage, MarketRow, PaperSummary, PaperTradeRow, RankingRow, SystemStatus, TrackingHealth,
    WalletOverview, WalletRow,
};
use rusqlite::{Connection, OpenFlags};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

pub struct AppState {
    pub db_path: PathBuf,
}

/// Open a read-only connection to the evaluator DB.
/// Each request gets a fresh connection — SQLite WAL handles concurrent reads fine.
pub fn open_readonly(state: &AppState) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        &state.db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    Ok(conn)
}

// --- Templates ---

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate;

#[derive(Template)]
#[template(path = "partials/status_strip.html")]
struct StatusStripTemplate {
    status: SystemStatus,
}

#[derive(Template)]
#[template(path = "partials/funnel_bar.html")]
struct FunnelBarTemplate {
    stages: Vec<FunnelStage>,
}

#[derive(Template)]
#[template(path = "partials/markets.html")]
struct MarketsTemplate {
    markets: Vec<MarketRow>,
}

#[derive(Template)]
#[template(path = "partials/wallets.html")]
struct WalletsTemplate {
    overview: WalletOverview,
    wallets: Vec<WalletRow>,
}

#[derive(Template)]
#[template(path = "partials/tracking.html")]
struct TrackingTemplate {
    health: Vec<TrackingHealth>,
    stale: Vec<String>,
}

#[derive(Template)]
#[template(path = "partials/paper.html")]
struct PaperTemplate {
    summary: PaperSummary,
    trades: Vec<PaperTradeRow>,
}

#[derive(Template)]
#[template(path = "partials/rankings.html")]
struct RankingsTemplate {
    rankings: Vec<RankingRow>,
}

// --- Handlers ---

async fn index() -> impl IntoResponse {
    Html(DashboardTemplate.to_string())
}

async fn status_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let db_path_str = state.db_path.to_str().unwrap_or(":memory:");
    let status = queries::system_status(&conn, db_path_str).unwrap();
    Html(StatusStripTemplate { status }.to_string())
}

async fn funnel_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let counts = queries::funnel_counts(&conn).unwrap();
    let stages = counts.to_stages();
    Html(FunnelBarTemplate { stages }.to_string())
}

async fn markets_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let markets = queries::top_markets_today(&conn).unwrap();
    Html(MarketsTemplate { markets }.to_string())
}

async fn wallets_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let overview = queries::wallet_overview(&conn).unwrap();
    let wallets = queries::recent_wallets(&conn, 20).unwrap();
    Html(WalletsTemplate { overview, wallets }.to_string())
}

async fn tracking_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let health = queries::tracking_health(&conn).unwrap();
    let stale = queries::stale_wallets(&conn).unwrap();
    Html(TrackingTemplate { health, stale }.to_string())
}

async fn paper_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let summary = queries::paper_summary(&conn, 10000.0).unwrap();
    let trades = queries::recent_paper_trades(&conn, 20).unwrap();
    Html(PaperTemplate { summary, trades }.to_string())
}

async fn rankings_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let rankings = queries::top_rankings(&conn, 30, 20).unwrap();
    Html(RankingsTemplate { rankings }.to_string())
}

// --- Router ---

pub fn create_router() -> Router {
    // For the index route (no DB needed), we still need the state for partials
    Router::new().route("/", get(index))
}

pub fn create_router_with_state(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/partials/status", get(status_partial))
        .route("/partials/funnel", get(funnel_partial))
        .route("/partials/markets", get(markets_partial))
        .route("/partials/wallets", get(wallets_partial))
        .route("/partials/tracking", get(tracking_partial))
        .route("/partials/paper", get(paper_partial))
        .route("/partials/rankings", get(rankings_partial))
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Load config — use [web] section if present, otherwise defaults
    let config = common::config::Config::load()?;
    let db_path = PathBuf::from(&config.database.path);
    let web_port = config.web.as_ref().map_or(8080, |w| w.port);
    let web_host = config
        .web
        .as_ref()
        .map_or("0.0.0.0".to_string(), |w| w.host.clone());

    let state = Arc::new(AppState { db_path });

    let app = create_router_with_state(state);
    let addr: SocketAddr = format!("{}:{}", web_host, web_port).parse()?;
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
    use common::db::Database;
    use tower::ServiceExt;

    fn create_test_app() -> Router {
        // For tests using partials, we need an in-memory DB with schema.
        // But axum state needs a path — we'll use a temp file.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let db = Database::open(path.to_str().unwrap()).unwrap();
        db.run_migrations().unwrap();
        drop(db); // close write connection so read-only can open

        // Leak the tempfile to keep it alive for the test
        std::mem::forget(tmp);

        let state = Arc::new(AppState { db_path: path });
        create_router_with_state(state)
    }

    #[tokio::test]
    async fn test_index_returns_200() {
        let app = create_router();
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_index_contains_dashboard_title() {
        let app = create_router();
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Trader Evaluator"));
        assert!(html.contains("htmx.org"));
        assert!(html.contains("tailwindcss"));
    }

    #[tokio::test]
    async fn test_status_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_status_partial_contains_phase() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Phase:"));
        assert!(html.contains("Foundation"));
    }

    #[tokio::test]
    async fn test_funnel_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/funnel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_funnel_partial_contains_stages() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/funnel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Markets"));
        assert!(html.contains("Scored"));
        assert!(html.contains("Wallets"));
        assert!(html.contains("Ranked"));
    }

    #[tokio::test]
    async fn test_markets_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/markets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_markets_partial_empty_shows_message() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/markets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("No markets scored today"));
    }

    #[tokio::test]
    async fn test_wallets_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/wallets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_wallets_partial_contains_overview() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/wallets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Total"));
        assert!(html.contains("Active"));
        assert!(html.contains("Holders"));
    }

    #[tokio::test]
    async fn test_tracking_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/tracking")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_tracking_partial_contains_data_types() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/tracking")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Trades"));
        assert!(html.contains("Activity"));
        assert!(html.contains("Positions"));
        assert!(html.contains("Holders"));
    }

    #[tokio::test]
    async fn test_paper_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/paper")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_paper_partial_empty_shows_message() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/paper")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("No paper trades yet"));
    }

    #[tokio::test]
    async fn test_rankings_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/rankings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_all_partials_return_200() {
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
            let app = create_test_app();
            let response = app
                .oneshot(Request::builder().uri(route).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "Route {} did not return 200",
                route
            );
        }
    }

    #[tokio::test]
    async fn test_dashboard_contains_htmx_partials() {
        let app = create_router();
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("hx-get=\"/partials/status\""));
        assert!(html.contains("hx-get=\"/partials/funnel\""));
        assert!(html.contains("hx-get=\"/partials/markets\""));
        assert!(html.contains("hx-get=\"/partials/wallets\""));
        assert!(html.contains("hx-get=\"/partials/tracking\""));
        assert!(html.contains("hx-get=\"/partials/paper\""));
        assert!(html.contains("hx-get=\"/partials/rankings\""));
        assert!(html.contains("every 30s"));
        assert!(html.contains("every 60s"));
    }

    #[tokio::test]
    async fn test_rankings_partial_empty_shows_message() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/rankings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("No wallet scores for today"));
    }
}
