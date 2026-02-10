mod metrics;
mod models;
mod queries;

use anyhow::Result;
use askama::Template;
use axum::body::Body;
use axum::extract::Query;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use models::{
    ExcludedWalletRow, FunnelStage, MarketRow, PaperSummary, PaperTradeRow, PersonaFunnelStage,
    RankingRow, SystemStatus, TrackingHealth, WalletJourney, WalletOverview, WalletRow,
};
use rand::Rng;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

pub struct AppState {
    pub db_path: PathBuf,
    pub auth_password: Option<String>,
    pub funnel_stage_infos: [String; 6],
    pub prom_handle: metrics_exporter_prometheus::PrometheusHandle,
    pub paper_bankroll_usdc: f64,
    pub max_total_exposure_pct: f64,
    pub max_daily_loss_pct: f64,
    pub max_concurrent_positions: u32,
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
const CSRF_COOKIE_NAME: &str = "evaluator_csrf";
const SESSION_DURATION_SECS: i64 = 7 * 24 * 60 * 60; // 7 days

/// Generate cryptographically secure auth token using SHA-256
fn generate_auth_token(password: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Generate cryptographically secure CSRF token
fn generate_csrf_token() -> String {
    let mut rng = rand::thread_rng();
    let token: [u8; 32] = rng.gen();
    hex::encode(token)
}

/// Iterate all cookie name/value pairs from (possibly multiple) Cookie headers.
///
/// Note: Some HTTP/2 intermediaries incorrectly join multiple Cookie headers using commas. We
/// accept both `;` and `,` separators to be robust in the presence of such proxies.
fn iter_cookie_pairs(headers: &HeaderMap) -> impl Iterator<Item = (&str, &str)> + '_ {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|cookie_header: &header::HeaderValue| cookie_header.to_str().ok())
        .flat_map(|cookie_str: &str| cookie_str.split([';', ',']))
        .filter_map(|cookie: &str| {
            let cookie = cookie.trim();
            let (name, value) = cookie.split_once('=')?;
            Some((name.trim(), value.trim()))
        })
}

fn header_has_cookie(headers: &HeaderMap, name: &str, expected_value: &str) -> bool {
    iter_cookie_pairs(headers).any(|(n, v)| n == name && v == expected_value)
}

fn header_has_cookie_name(headers: &HeaderMap, name: &str) -> bool {
    iter_cookie_pairs(headers).any(|(n, _)| n == name)
}

fn header_get_cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    iter_cookie_pairs(headers)
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.to_string())
}

/// Verify CSRF token from cookie against form submission
fn verify_csrf_token(headers: &HeaderMap, form_token: &str) -> bool {
    header_has_cookie(headers, CSRF_COOKIE_NAME, form_token)
}

/// Constant-time comparison to prevent timing attacks
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }
    result == 0
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
    let is_authenticated = header_has_cookie(request.headers(), AUTH_COOKIE_NAME, &auth_token);

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
    csrf_token: Option<String>,
}

#[derive(Template)]
#[template(path = "excluded.html")]
struct ExcludedTemplate {
    rows: Vec<ExcludedWalletRow>,
    total: i64,
    page: i64,
    page_size: i64,
    total_pages: i64,
}

#[derive(Template)]
#[template(path = "journey.html")]
struct JourneyTemplate {
    journey: WalletJourney,
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
#[template(path = "partials/persona_funnel_bar.html")]
struct PersonaFunnelBarTemplate {
    stages: Vec<PersonaFunnelStage>,
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

async fn login_form(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    // If no auth configured, redirect to dashboard
    if state.auth_password.is_none() {
        return Redirect::to("/").into_response();
    }

    // Reuse the CSRF cookie token if it already exists. This prevents multi-tab failures where
    // opening /login in another tab rotates the CSRF cookie and invalidates the first tab's form.
    let csrf_token =
        header_get_cookie_value(&headers, CSRF_COOKIE_NAME).unwrap_or_else(generate_csrf_token);
    let csrf_cookie = format!(
        "{CSRF_COOKIE_NAME}={csrf_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_DURATION_SECS}"
    );

    let response = Html(
        LoginTemplate {
            error: None,
            csrf_token: Some(csrf_token.clone()),
        }
        .to_string(),
    )
    .into_response();

    // Set CSRF cookie
    let mut response = response;
    response
        .headers_mut()
        .insert(header::SET_COOKIE, csrf_cookie.parse().unwrap());

    response
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
    csrf_token: String,
}

async fn login_submit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    // If no auth configured, just redirect
    if state.auth_password.is_none() {
        return Redirect::to("/").into_response();
    }

    // Verify CSRF token. On failure, issue a new token so the user can retry (e.g. after proxy cookie issues).
    if !verify_csrf_token(&headers, &form.csrf_token) {
        let cookie_headers = headers.get_all(header::COOKIE);
        let has_cookie = cookie_headers.iter().next().is_some();
        let has_csrf_cookie = header_has_cookie_name(&headers, CSRF_COOKIE_NAME);
        tracing::debug!(
            has_cookie,
            has_csrf_cookie,
            "login CSRF verification failed"
        );

        let new_csrf_token = generate_csrf_token();
        let csrf_cookie = format!(
            "{CSRF_COOKIE_NAME}={new_csrf_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_DURATION_SECS}"
        );

        let response = Html(
            LoginTemplate {
                error: Some("Invalid CSRF token".to_string()),
                csrf_token: Some(new_csrf_token.clone()),
            }
            .to_string(),
        )
        .into_response();

        let mut response = response;
        response
            .headers_mut()
            .insert(header::SET_COOKIE, csrf_cookie.parse().unwrap());

        return response.into_response();
    }

    // Verify password (constant-time comparison to prevent timing attacks)
    let expected_password = state.auth_password.as_ref().unwrap();
    if constant_time_eq(&form.password, expected_password) {
        // Set auth cookie
        let auth_token = generate_auth_token(&form.password);
        let auth_cookie = format!(
            "{AUTH_COOKIE_NAME}={auth_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_DURATION_SECS}"
        );

        Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header(header::SET_COOKIE, auth_cookie)
            .header(header::LOCATION, "/")
            .body(Body::empty())
            .unwrap()
            .into_response()
    } else {
        // Generate new CSRF token for the retry
        let new_csrf_token = generate_csrf_token();
        let csrf_cookie = format!("{CSRF_COOKIE_NAME}={new_csrf_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_DURATION_SECS}");

        let response = Html(
            LoginTemplate {
                error: Some("Invalid password".to_string()),
                csrf_token: Some(new_csrf_token.clone()),
            }
            .to_string(),
        )
        .into_response();

        // Set new CSRF cookie
        let mut response = response;
        response
            .headers_mut()
            .insert(header::SET_COOKIE, csrf_cookie.parse().unwrap());

        response
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
        .into_response()
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
    let stages = counts.to_stages(&state.funnel_stage_infos);
    Html(FunnelBarTemplate { stages }.to_string())
}

async fn persona_funnel_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let counts = queries::persona_funnel_counts(&conn).unwrap();
    let stages = counts.to_stages();
    Html(PersonaFunnelBarTemplate { stages }.to_string())
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
    let summary = queries::paper_summary(
        &conn,
        state.paper_bankroll_usdc,
        state.max_total_exposure_pct,
        state.max_daily_loss_pct,
        i64::from(state.max_concurrent_positions),
    )
    .unwrap();
    let trades = queries::recent_paper_trades(&conn, 20).unwrap();
    Html(PaperTemplate { summary, trades }.to_string())
}

async fn rankings_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let rankings = queries::top_rankings(&conn, 30, 20).unwrap();
    Html(RankingsTemplate { rankings }.to_string())
}

#[derive(Deserialize)]
struct ExcludedParams {
    page: Option<i64>,
    page_size: Option<i64>,
}

async fn excluded_page(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExcludedParams>,
) -> impl IntoResponse {
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(50).clamp(1, 200);
    let offset = ((page - 1) * page_size) as usize;

    let conn = open_readonly(&state).unwrap();
    let total = queries::excluded_wallets_count(&conn).unwrap();
    let rows = queries::excluded_wallets_latest(&conn, page_size as usize, offset).unwrap();

    let total_pages = ((total + page_size - 1) / page_size).max(1);
    Html(
        ExcludedTemplate {
            rows,
            total,
            page,
            page_size,
            total_pages,
        }
        .to_string(),
    )
}

async fn journey_page(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> impl IntoResponse {
    let conn = open_readonly(&state).unwrap();
    let Some(journey) = queries::wallet_journey(&conn, &wallet).unwrap() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Html(JourneyTemplate { journey }.to_string()).into_response()
}

async fn metrics_endpoint(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Opportunistically update derived gauges from SQLite so Grafana can chart drop-offs.
    // This keeps metrics fresh even if the dashboard UI isn't being opened.
    if let Ok(conn) = open_readonly(&state) {
        if let Ok(c) = queries::funnel_counts(&conn) {
            ::metrics::gauge!(
                "evaluator_pipeline_funnel_stage_count",
                "stage" => "markets_fetched"
            )
            .set(c.markets_fetched as f64);
            ::metrics::gauge!(
                "evaluator_pipeline_funnel_stage_count",
                "stage" => "markets_scored"
            )
            .set(c.markets_scored as f64);
            ::metrics::gauge!(
                "evaluator_pipeline_funnel_stage_count",
                "stage" => "wallets_discovered"
            )
            .set(c.wallets_discovered as f64);
            ::metrics::gauge!(
                "evaluator_pipeline_funnel_stage_count",
                "stage" => "wallets_tracked"
            )
            .set(c.wallets_active as f64);
            ::metrics::gauge!(
                "evaluator_pipeline_funnel_stage_count",
                "stage" => "paper_trades_total"
            )
            .set(c.paper_trades_total as f64);
            ::metrics::gauge!(
                "evaluator_pipeline_funnel_stage_count",
                "stage" => "wallets_ranked"
            )
            .set(c.wallets_ranked as f64);
        }

        if let Ok(c) = queries::persona_funnel_counts(&conn) {
            ::metrics::gauge!(
                "evaluator_persona_funnel_stage_count",
                "stage" => "wallets_discovered"
            )
            .set(c.wallets_discovered as f64);
            ::metrics::gauge!(
                "evaluator_persona_funnel_stage_count",
                "stage" => "stage1_passed"
            )
            .set(c.stage1_passed as f64);
            ::metrics::gauge!(
                "evaluator_persona_funnel_stage_count",
                "stage" => "stage2_classified"
            )
            .set(c.stage2_classified as f64);
            ::metrics::gauge!(
                "evaluator_persona_funnel_stage_count",
                "stage" => "paper_traded_wallets"
            )
            .set(c.paper_traded_wallets as f64);
            ::metrics::gauge!(
                "evaluator_persona_funnel_stage_count",
                "stage" => "follow_worthy_wallets"
            )
            .set(c.follow_worthy_wallets as f64);
        }
    }

    // Ensure exporter housekeeping runs so histograms don't grow without bound.
    state.prom_handle.run_upkeep();

    let rendered = state.prom_handle.render();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    (headers, rendered)
}

// --- Router ---

pub fn create_router() -> Router {
    Router::new().route("/", get(index))
}

pub fn create_router_with_state(state: Arc<AppState>) -> Router {
    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/login", get(login_form).post(login_submit))
        .route("/logout", get(logout))
        .route("/metrics", get(metrics_endpoint));

    // Protected routes (auth required if password is set)
    let protected_routes = Router::new()
        .route("/", get(index))
        .route("/excluded", get(excluded_page))
        .route("/journey/{wallet}", get(journey_page))
        .route("/partials/status", get(status_partial))
        .route("/partials/funnel", get(funnel_partial))
        .route("/partials/persona_funnel", get(persona_funnel_partial))
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
    let funnel_stage_infos = common::funnel::funnel_stage_infos(&config);
    let prom_handle = metrics::init_global()?;

    let state = Arc::new(AppState {
        db_path,
        auth_password,
        funnel_stage_infos,
        prom_handle,
        paper_bankroll_usdc: config.risk.paper_bankroll_usdc,
        max_total_exposure_pct: config.paper_trading.max_total_exposure_pct,
        max_daily_loss_pct: config.paper_trading.max_daily_loss_pct,
        max_concurrent_positions: config.risk.max_concurrent_positions,
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

        let cfg =
            common::config::Config::from_toml_str(include_str!("../../../config/default.toml"))
                .unwrap();
        let prom_handle = metrics::init_global().unwrap();
        let state = Arc::new(AppState {
            db_path: path,
            auth_password: None,
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            prom_handle,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: cfg.risk.max_concurrent_positions,
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

        let cfg =
            common::config::Config::from_toml_str(include_str!("../../../config/default.toml"))
                .unwrap();
        let prom_handle = metrics::init_global().unwrap();
        let state = Arc::new(AppState {
            db_path: path,
            auth_password: Some(password.to_string()),
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            prom_handle,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: cfg.risk.max_concurrent_positions,
        });
        create_router_with_state(state)
    }

    fn auth_cookie(password: &str) -> String {
        let token = generate_auth_token(password);
        format!("{AUTH_COOKIE_NAME}={token}")
    }

    /// GET /login and parse CSRF token from Set-Cookie. Required for POST /login.
    async fn get_csrf_token_from_login(app: &Router) -> String {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body_bytes.to_vec()).unwrap();
        // Parse `name="csrf_token" value="<token>"`
        let marker = "name=\"csrf_token\" value=\"";
        let start = html
            .find(marker)
            .expect("login page must include hidden csrf_token input")
            + marker.len();
        let end = html[start..].find('"').map(|i| start + i).unwrap();
        html[start..end].to_string()
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
        let csrf_token = get_csrf_token_from_login(&app).await;
        let body = format!("password=secret&csrf_token={csrf_token}");
        let cookie = format!("{CSRF_COOKIE_NAME}={csrf_token}");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .method("POST")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .header("Cookie", cookie)
                    .body(Body::from(body))
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
    async fn test_login_with_csrf_cookie_in_second_cookie_header_succeeds() {
        let app = create_test_app_with_auth("secret");
        let csrf_token = get_csrf_token_from_login(&app).await;
        let body = format!("password=secret&csrf_token={csrf_token}");

        // Some clients/proxies can send multiple Cookie headers. Ensure we accept our CSRF cookie
        // even if it's not in the first Cookie header value.
        let cookie1 = "some_other_cookie=some_value";
        let cookie2 = format!("{CSRF_COOKIE_NAME}={csrf_token}");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .method("POST")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .header("Cookie", cookie1)
                    .header("Cookie", cookie2)
                    .body(Body::from(body))
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
    }

    #[tokio::test]
    async fn test_login_with_comma_joined_cookie_header_succeeds() {
        let app = create_test_app_with_auth("secret");
        let csrf_token = get_csrf_token_from_login(&app).await;
        let body = format!("password=secret&csrf_token={csrf_token}");
        // Some proxies improperly join Cookie headers using commas instead of semicolons.
        let cookie = format!("{CSRF_COOKIE_NAME}={csrf_token}, foo=bar");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .method("POST")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .header("Cookie", cookie)
                    .body(Body::from(body))
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
    }

    #[tokio::test]
    async fn test_login_repeated_get_does_not_rotate_csrf_token() {
        let app = create_test_app_with_auth("secret");

        // First tab load.
        let token1 = get_csrf_token_from_login(&app).await;

        // Second tab load, with the browser sending the previously set cookie.
        let response2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .header("Cookie", format!("{CSRF_COOKIE_NAME}={token1}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = axum::body::to_bytes(response2.into_body(), usize::MAX)
            .await
            .unwrap();
        let html2 = String::from_utf8(body_bytes.to_vec()).unwrap();
        let marker = "name=\"csrf_token\" value=\"";
        let start = html2.find(marker).unwrap() + marker.len();
        let end = html2[start..].find('"').map(|i| start + i).unwrap();
        let token2 = &html2[start..end];

        assert_eq!(token2, token1);
    }

    #[tokio::test]
    async fn test_login_succeeds_even_if_user_submits_from_older_tab() {
        let app = create_test_app_with_auth("secret");

        // Tab A
        let token_a = get_csrf_token_from_login(&app).await;

        // Tab B (browser sends cookie from Tab A)
        let response_b = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .header("Cookie", format!("{CSRF_COOKIE_NAME}={token_a}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let set_cookie_b = response_b
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap();
        let cookie_token_b = set_cookie_b
            .split(';')
            .next()
            .unwrap()
            .split_once('=')
            .unwrap()
            .1
            .to_string();

        // User submits from Tab A while browser cookie jar has whatever Tab B last set.
        let body = format!("password=secret&csrf_token={token_a}");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .method("POST")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .header("Cookie", format!("{CSRF_COOKIE_NAME}={cookie_token_b}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn test_login_with_wrong_password_shows_error() {
        let app = create_test_app_with_auth("secret");
        let csrf_token = get_csrf_token_from_login(&app).await;
        let body = format!("password=wrong&csrf_token={csrf_token}");
        let cookie = format!("{CSRF_COOKIE_NAME}={csrf_token}");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .method("POST")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .header("Cookie", cookie)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(html.contains("Invalid password"));
    }

    #[tokio::test]
    async fn test_login_with_valid_password_missing_cookie_returns_csrf_error() {
        let app = create_test_app_with_auth("secret");
        let csrf_token = get_csrf_token_from_login(&app).await;
        let body = format!("password=secret&csrf_token={csrf_token}");
        // No Cookie header - simulates proxy stripping cookie or first POST without cookie

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .method("POST")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get("set-cookie")
            .expect("CSRF error response must set new CSRF cookie")
            .to_str()
            .unwrap();
        assert!(set_cookie.starts_with(&format!("{CSRF_COOKIE_NAME}=")));
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(html.contains("Invalid CSRF token"));
    }

    #[tokio::test]
    async fn test_login_with_wrong_csrf_cookie_returns_csrf_error() {
        let app = create_test_app_with_auth("secret");
        let csrf_token = get_csrf_token_from_login(&app).await;
        let body = format!("password=secret&csrf_token={csrf_token}");
        let wrong_cookie = format!("{CSRF_COOKIE_NAME}=wrong_token_value");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .method("POST")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .header("Cookie", wrong_cookie)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get("set-cookie")
            .expect("CSRF error response must set new CSRF cookie")
            .to_str()
            .unwrap();
        assert!(set_cookie.starts_with(&format!("{CSRF_COOKIE_NAME}=")));
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(html.contains("Invalid CSRF token"));
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
    async fn test_access_with_auth_cookie_comma_joined_succeeds() {
        let app = create_test_app_with_auth("secret");
        let cookie = format!("{}, foo=bar", auth_cookie("secret"));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Cookie", cookie)
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
    async fn test_metrics_endpoint_returns_200_and_contains_build_info() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("evaluator_web_build_info"));
    }

    #[tokio::test]
    async fn test_metrics_exports_persona_funnel_stage_gauges() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let db = Database::open(path.to_str().unwrap()).unwrap();
        db.run_migrations().unwrap();

        // Minimal data to produce a non-zero Stage 1 passed count.
        db.conn
            .execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xw1', 'HOLDER', 1)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xw2', 'HOLDER', 1)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold)
                 VALUES ('0xw1', 'STAGE1_TOO_YOUNG', 5.0, 30.0)",
                [],
            )
            .unwrap();

        drop(db); // close write connection so read-only can open
        std::mem::forget(tmp); // keep tempfile alive for this test

        let cfg =
            common::config::Config::from_toml_str(include_str!("../../../config/default.toml"))
                .unwrap();
        let prom_handle = metrics::init_global().unwrap();
        let state = Arc::new(AppState {
            db_path: path,
            auth_password: None,
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            prom_handle,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: cfg.risk.max_concurrent_positions,
        });
        let app = create_router_with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("evaluator_persona_funnel_stage_count"));
        assert!(text.contains("stage=\"stage1_passed\""));
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
    async fn test_persona_funnel_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/persona_funnel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_excluded_page_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/excluded")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Excluded Wallets"));
    }

    #[tokio::test]
    async fn test_excluded_page_paginates_latest_per_wallet() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let db = Database::open(path.to_str().unwrap()).unwrap();
        db.run_migrations().unwrap();

        db.conn
            .execute(
                "INSERT INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold, excluded_at)
                 VALUES ('0xaaaaaaaaaaaaaaaa', 'NOISE_TRADER', 60.0, 50.0, '2026-02-10 10:00:00')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO wallet_exclusions (proxy_wallet, reason, metric_value, threshold, excluded_at)
                 VALUES ('0xbbbbbbbbbbbbbbbb', 'TAIL_RISK_SELLER', 0.83, 0.80, '2026-02-10 11:00:00')",
                [],
            )
            .unwrap();

        drop(db);
        std::mem::forget(tmp);

        let cfg =
            common::config::Config::from_toml_str(include_str!("../../../config/default.toml"))
                .unwrap();
        let prom_handle = metrics::init_global().unwrap();
        let state = Arc::new(AppState {
            db_path: path,
            auth_password: None,
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            prom_handle,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: cfg.risk.max_concurrent_positions,
        });
        let app = create_router_with_state(state);

        let resp1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/excluded?page=1&page_size=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);
        let body1 = axum::body::to_bytes(resp1.into_body(), usize::MAX)
            .await
            .unwrap();
        let html1 = String::from_utf8(body1.to_vec()).unwrap();

        let resp2 = app
            .oneshot(
                Request::builder()
                    .uri("/excluded?page=2&page_size=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let html2 = String::from_utf8(body2.to_vec()).unwrap();

        assert_ne!(html1, html2);
        assert!(html1.contains("0xbbbb") || html1.contains("0xaaaa"));
        assert!(html2.contains("0xbbbb") || html2.contains("0xaaaa"));
    }

    #[tokio::test]
    async fn test_journey_unknown_wallet_returns_404() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/journey/0xdoesnotexist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_journey_known_wallet_returns_200() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let db = Database::open(path.to_str().unwrap()).unwrap();
        db.run_migrations().unwrap();

        db.conn
            .execute(
                "INSERT INTO wallets (proxy_wallet, discovered_from, discovered_at, is_active)
                 VALUES ('0xw2', 'HOLDER', '2026-02-10 09:00:00', 1)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO wallet_personas (proxy_wallet, persona, confidence, classified_at)
                 VALUES ('0xw2', 'Informed Specialist', 0.87, '2026-02-10 10:00:00')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl, created_at)
                 VALUES ('0xw2', 'mirror', '0xm1', 'BUY', 25.0, 0.60, 'settled_win', 5.0, '2026-02-10 11:00:00')",
                [],
            )
            .unwrap();

        drop(db);
        std::mem::forget(tmp);

        let cfg =
            common::config::Config::from_toml_str(include_str!("../../../config/default.toml"))
                .unwrap();
        let prom_handle = metrics::init_global().unwrap();
        let state = Arc::new(AppState {
            db_path: path,
            auth_password: None,
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            prom_handle,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: cfg.risk.max_concurrent_positions,
        });
        let app = create_router_with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/journey/0xw2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Journey"));
        assert!(html.contains("0xw2"));
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

        let cfg =
            common::config::Config::from_toml_str(include_str!("../../../config/default.toml"))
                .unwrap();
        let prom_handle = metrics::init_global().unwrap();
        let state = Arc::new(AppState {
            db_path: db_path.into(),
            auth_password: None,
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            prom_handle,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: cfg.risk.max_concurrent_positions,
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
