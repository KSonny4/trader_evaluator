mod models;
mod queries;

use anyhow::Result;
use askama::Template;
use axum::body::Body;
use axum::extract::Request;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use models::{
    FunnelStage, MarketRow, PaperSummary, PaperTradeRow, RankingRow, SystemStatus, TrackingHealth,
    WalletOverview, WalletRow,
};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
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

// --- Cookie-based Auth Middleware ---

const AUTH_COOKIE_NAME: &str = "evaluator_auth";

/// Simple auth token (in production, use a proper session store)
fn generate_auth_token(password: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    password.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Redirects to /login if auth_password is configured and user is not authenticated.
/// If auth_password is None, all requests pass through (no auth).
async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    // If no password is configured, allow all requests
    if state.auth_password.is_none() {
        return next.run(request).await;
    }

    // Check auth cookie
    let auth_token = generate_auth_token(state.auth_password.as_ref().unwrap());
    let is_authenticated = request
        .headers()
        .get(header::COOKIE)
        .and_then(|cookie_header| cookie_header.to_str().ok())
        .is_some_and(|cookie_str| {
            cookie_str.split(';').any(|cookie| {
                let cookie = cookie.trim();
                cookie.starts_with(&format!("{AUTH_COOKIE_NAME}="))
                    && cookie == format!("{AUTH_COOKIE_NAME}={auth_token}")
            })
        });

    if is_authenticated {
        next.run(request).await
    } else {
        // Check if this is an HTMX request (indicated by HX-Request header)
        let is_htmx = request.headers().get("HX-Request").is_some();

        if is_htmx {
            // For HTMX requests, return a special response that triggers a full page redirect
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("HX-Redirect", "/login")
                .body(Body::from("Session expired. Please log in again."))
                .unwrap()
        } else {
            // Regular request - redirect to login
            Redirect::to("/login").into_response()
        }
    }
}

// --- Templates ---

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate;

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: Option<String>,
}

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

async fn login_form(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // If no auth configured, redirect to dashboard
    if state.auth_password.is_none() {
        return Redirect::to("/").into_response();
    }

    Html(LoginTemplate { error: None }.to_string()).into_response()
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
}

async fn login_submit(
    State(state): State<Arc<AppState>>,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    // If no auth configured, just redirect
    if state.auth_password.is_none() {
        return Redirect::to("/").into_response();
    }

    // Verify password
    if form.password == *state.auth_password.as_ref().unwrap() {
        // Set auth cookie
        let auth_token = generate_auth_token(&form.password);
        let cookie = format!(
            "{AUTH_COOKIE_NAME}={auth_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800"
        );

        Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header(header::SET_COOKIE, cookie)
            .header(header::LOCATION, "/")
            .body(Body::empty())
            .unwrap()
            .into_response()
    } else {
        // Show error
        Html(
            LoginTemplate {
                error: Some("Invalid password".to_string()),
            }
            .to_string(),
        )
        .into_response()
    }
}

async fn logout() -> impl IntoResponse {
    // Clear auth cookie
    let cookie = format!("{AUTH_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");

    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::SET_COOKIE, cookie)
        .header(header::LOCATION, "/login")
        .body(Body::empty())
        .unwrap()
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
    Router::new().route("/", get(index))
}

pub fn create_router_with_state(state: Arc<AppState>) -> Router {
    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/login", get(login_form).post(login_submit))
        .route("/logout", get(logout));

    // Protected routes (auth required if password is set)
    let protected_routes = Router::new()
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
            auth_middleware,
        ));

    public_routes.merge(protected_routes).with_state(state)
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

    fn auth_cookie(password: &str) -> String {
        let token = generate_auth_token(password);
        format!("{AUTH_COOKIE_NAME}={token}")
    }

    // --- Auth tests (updated for cookie-based auth) ---

    #[tokio::test]
    async fn test_auth_redirects_to_login_without_cookie() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER); // 302 redirect
        let location = response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(location, "/login");
    }

    #[tokio::test]
    async fn test_login_page_shows_without_auth() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_login_with_correct_password_sets_cookie() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .method("POST")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("password=secret"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(location, "/");

        // Check that cookie is set
        let set_cookie = response
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set_cookie.contains(AUTH_COOKIE_NAME));
    }

    #[tokio::test]
    async fn test_login_with_wrong_password_shows_error() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .method("POST")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from("password=wrong"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Invalid password"));
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
    async fn test_no_auth_redirects_when_no_password() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Should redirect to / since no auth needed
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn test_access_with_valid_cookie() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Cookie", auth_cookie("secret"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_logout_clears_cookie() {
        let app = create_test_app_with_auth("secret");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/logout")
                    .header("Cookie", auth_cookie("secret"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(location, "/login");

        // Check that cookie is cleared
        let set_cookie = response
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set_cookie.contains("Max-Age=0"));
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
