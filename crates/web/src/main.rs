mod models;
mod queries;

use anyhow::Result;
use askama::Template;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::{routing::get, Router};
use base64::Engine as _;
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
    pub auth_password: Option<String>,
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

// --- Basic Auth Middleware ---

/// Returns 401 Unauthorized if auth_password is configured and credentials don't match.
/// If auth_password is None, all requests pass through (no auth).
async fn basic_auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(password) = &state.auth_password else {
        return next.run(request).await;
    };

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let authenticated = auth_header
        .and_then(|h| h.strip_prefix("Basic "))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .is_some_and(|decoded| {
            // Format is "username:password" — we only check the password part
            decoded
                .split_once(':')
                .is_some_and(|(_, pw)| pw == password)
        });

    if authenticated {
        next.run(request).await
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"Evaluator Dashboard\"",
            )
            .body(Body::from("Unauthorized"))
            .unwrap()
    }
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
        .layer(middleware::from_fn_with_state(
            state.clone(),
            basic_auth_middleware,
        ))
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
    let auth_password = config.web.as_ref().and_then(|w| w.auth_password.clone());

    let state = Arc::new(AppState {
        db_path,
        auth_password,
    });

    let app = create_router_with_state(state);
    let addr: SocketAddr = format!("{web_host}:{web_port}").parse()?;
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

        let state = Arc::new(AppState {
            db_path: path,
            auth_password: None,
        });
        create_router_with_state(state)
    }

    fn create_test_app_with_auth(password: &str) -> Router {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let db = Database::open(path.to_str().unwrap()).unwrap();
        db.run_migrations().unwrap();
        drop(db);
        std::mem::forget(tmp);

        let state = Arc::new(AppState {
            db_path: path,
            auth_password: Some(password.to_string()),
        });
        create_router_with_state(state)
    }

    fn basic_auth_header(user: &str, pass: &str) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        format!("Basic {encoded}")
    }

    // --- Auth tests ---

    #[tokio::test]
    async fn test_auth_returns_401_without_credentials() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_returns_401_with_wrong_password() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Authorization", basic_auth_header("admin", "wrong"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_returns_200_with_correct_password() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Authorization", basic_auth_header("admin", "secret"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_disabled_when_no_password() {
        let app = create_test_app(); // auth_password: None
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_partials_also_protected() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_www_authenticate_header_present() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let www_auth = response
            .headers()
            .get("www-authenticate")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(www_auth.contains("Basic"));
    }

    // --- Existing tests ---

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
                "Route {route} did not return 200"
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

    #[tokio::test]
    #[ignore] // requires real DB at data/evaluator.db
    async fn test_dashboard_with_real_db() {
        let db_path = "data/evaluator.db";
        if !std::path::Path::new(db_path).exists() {
            eprintln!("Skipping: no real DB found at {db_path}");
            return;
        }

        let state = Arc::new(AppState {
            db_path: db_path.into(),
            auth_password: None,
        });
        let app = create_router_with_state(state);

        // Full page
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // All partials
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
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(route).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "Route {route} failed with real DB"
            );
        }
    }
}
