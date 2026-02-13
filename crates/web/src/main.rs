mod metrics;
mod models;
mod queries;

use anyhow::Result;
use askama::Template;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Json, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use models::{
    EventRow, ExcludedWalletRow, FunnelStage, LastRunStats, MarketRow, PaperSummary, PaperTradeRow,
    PersonaFunnelStage, RankingRow, SuitablePersonaRow, SystemStatus, TrackingHealth,
    UnifiedFunnelStage, WalletJourney, WalletRow,
};
use rand::Rng;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tower_http::trace::TraceLayer;

pub struct AppState {
    pub db_path: PathBuf,
    pub auth_password: Option<String>,
    pub funnel_stage_infos: [String; 6],
    // Used to avoid async runtime starvation when DB reads are slow.
    pub db_semaphore: Arc<Semaphore>,
    pub db_timeout: Duration,
    // Test-only knob to simulate a slow disk / slow sqlite open.
    pub db_open_delay: Duration,
    pub paper_bankroll_usdc: f64,
    pub max_total_exposure_pct: f64,
    pub max_daily_loss_pct: f64,
    pub max_concurrent_positions: i64,
    // Rate limiter for login attempts
    pub login_rate_limiter: Arc<LoginRateLimiter>,
    /// Gamma API base URL for Polymarket profile fetch (optional; when set, wallet display uses profile name).
    pub gamma_api_url: Option<String>,
    /// HTTP client for outbound requests (e.g. Polymarket profile).
    pub http_client: Option<reqwest::Client>,
}

/// Open a read-only connection to the evaluator DB.
/// Each request gets a fresh connection — SQLite WAL handles concurrent reads fine.
pub fn open_readonly(state: &AppState) -> Result<Connection> {
    if !state.db_open_delay.is_zero() {
        std::thread::sleep(state.db_open_delay);
    }
    let conn = Connection::open_with_flags(
        &state.db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    Ok(conn)
}

/// Run a DB query without blocking tokio worker threads.
///
/// We limit concurrent DB work and apply a timeout to keep the dashboard responsive even under
/// severe IO pressure.
async fn with_db<R, F>(state: Arc<AppState>, f: F) -> Result<R>
where
    R: Send + 'static,
    F: FnOnce(&Connection) -> Result<R> + Send + 'static,
{
    let permit = state.db_semaphore.clone().acquire_owned().await?;
    let timeout = state.db_timeout;

    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let conn = open_readonly(&state)?;
        f(&conn)
    });

    match tokio::time::timeout(timeout, handle).await {
        Ok(joined) => joined?,
        Err(_) => Err(anyhow::anyhow!("db query timed out after {timeout:?}")),
    }
}

// --- Cookie-based Auth Middleware ---

const AUTH_COOKIE_NAME: &str = "evaluator_auth";
const CSRF_COOKIE_NAME: &str = "evaluator_csrf";
const SESSION_DURATION_SECS: i64 = 7 * 24 * 60 * 60; // 7 days

// --- Rate Limiting for Login ---

/// Simple in-memory rate limiter for login attempts
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct LoginRateLimiter {
    attempts: Arc<Mutex<HashMap<String, Vec<u64>>>>,
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl LoginRateLimiter {
    pub fn new() -> Self {
        Self {
            attempts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if the client IP is rate limited (5 attempts per minute)
    #[allow(clippy::significant_drop_tightening)] // lock needed for retain + len; Clippy's suggestion is invalid
    pub fn is_rate_limited(&self, client_ip: &str) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let count = {
            let mut attempts = self.attempts.lock().unwrap();
            let client_attempts = attempts.entry(client_ip.to_string()).or_default();
            client_attempts.retain(|&timestamp| now - timestamp < 60);
            client_attempts.len()
        };
        count >= 5
    }

    /// Record a login attempt
    pub fn record_attempt(&self, client_ip: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.attempts
            .lock()
            .unwrap()
            .entry(client_ip.to_string())
            .or_default()
            .push(now);
    }

    /// Extract client IP from request
    fn extract_client_ip(req: &Request<Body>) -> String {
        req.headers()
            .get("x-forwarded-for")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.split(',').next())
            .or_else(|| req.headers().get("x-real-ip").and_then(|h| h.to_str().ok()))
            .or_else(|| {
                req.headers()
                    .get("cf-connecting-ip")
                    .and_then(|h| h.to_str().ok())
            })
            .unwrap_or("unknown")
            .to_string()
    }
}

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

// --- Security Headers Middleware ---

/// Rate limiting middleware: only applies to POST /login (actual login attempts).
/// GET /login, GET /logout, etc. pass through without counting.
async fn login_rate_limit_middleware(
    State(limiter): State<Arc<LoginRateLimiter>>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() != Method::POST || request.uri().path() != "/login" {
        return next.run(request).await;
    }

    let client_ip = LoginRateLimiter::extract_client_ip(&request);

    if limiter.is_rate_limited(&client_ip) {
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header("Retry-After", "60")
            .body(Body::from(
                "Too many login attempts. Please try again in 60 seconds.",
            ))
            .unwrap()
            .into_response();
    }

    limiter.record_attempt(&client_ip);
    next.run(request).await
}

/// Add security headers to all responses
async fn security_headers_middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;

    let headers = response.headers_mut();

    // Content Security Policy - allow Tailwind and HTMX CDNs; partials fetched from same origin
    headers.insert(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' 'unsafe-inline' https://cdn.tailwindcss.com https://unpkg.com; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' https://cdn.tailwindcss.com; frame-ancestors 'none';".parse().unwrap(),
    );

    // Prevent clickjacking
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());

    // Prevent MIME type sniffing
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());

    // XSS Protection (legacy but still useful)
    headers.insert("X-XSS-Protection", "1; mode=block".parse().unwrap());

    // Referrer Policy
    headers.insert(
        "Referrer-Policy",
        "strict-origin-when-cross-origin".parse().unwrap(),
    );

    // Strict Transport Security (only in production)
    headers.insert(
        "Strict-Transport-Security",
        "max-age=31536000; includeSubDomains".parse().unwrap(),
    );

    response
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
#[template(path = "wallet_scorecard.html")]
struct ScorecardTemplate {
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
#[template(path = "partials/unified_funnel_bar.html")]
struct UnifiedFunnelBarTemplate {
    stages: Vec<UnifiedFunnelStage>,
}

#[derive(Template)]
#[template(path = "partials/async_funnel_bar.html")]
struct AsyncFunnelBarTemplate {
    stats: LastRunStats,
}

#[derive(Template)]
#[template(path = "partials/markets.html")]
struct MarketsTemplate {
    markets: Vec<MarketRow>,
}

#[derive(Template)]
#[template(path = "partials/events.html")]
struct EventsTemplate {
    events: Vec<EventRow>,
    events_selected: i64,
    events_evaluated: i64,
}

#[derive(Template)]
#[template(path = "partials/wallets.html")]
struct WalletsTemplate {
    wallets: Vec<WalletRow>,
}

#[derive(Template)]
#[template(path = "partials/suitable_personas.html")]
struct SuitablePersonasTemplate {
    personas: Vec<SuitablePersonaRow>,
    suitable_count: i64,
    evaluated_count: i64,
    excluded_count: i64,
    recent_exclusions: Vec<ExcludedWalletRow>,
}

#[derive(Template)]
#[template(path = "partials/personas_summary_bar.html")]
struct PersonasSummaryBarTemplate {
    suitable_count: i64,
    evaluated_count: i64,
    excluded_count: i64,
}

#[derive(Template)]
#[template(path = "partials/paper_traded_wallets.html")]
struct PaperTradedWalletsTemplate {
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
    let db_path_str = state.db_path.to_string_lossy().to_string();
    match with_db(state.clone(), move |conn| {
        queries::system_status(conn, &db_path_str)
    })
    .await
    {
        Ok(status) => Html(StatusStripTemplate { status }.to_string()).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn unified_funnel_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match with_db(state.clone(), move |conn| {
        let counts = queries::unified_funnel_counts(conn)?;
        Ok(counts.to_stages())
    })
    .await
    {
        Ok(stages) => Html(UnifiedFunnelBarTemplate { stages }.to_string()).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn async_funnel_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match with_db(state.clone(), queries::last_run_stats).await {
        Ok(stats) => Html(AsyncFunnelBarTemplate { stats }.to_string()).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn markets_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match with_db(state.clone(), queries::top_markets_today).await {
        Ok(markets) => Html(MarketsTemplate { markets }.to_string()).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn events_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match with_db(state.clone(), move |conn| {
        let events = queries::top_events(conn, 10)?;
        let (events_selected, events_evaluated) = queries::events_counts(conn)?;
        Ok(EventsTemplate {
            events,
            events_selected,
            events_evaluated,
        })
    })
    .await
    {
        Ok(tmpl) => Html(tmpl.to_string()).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn wallets_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match with_db(state.clone(), move |conn| {
        let wallets = queries::recent_wallets(conn, 10)?;
        Ok(wallets)
    })
    .await
    {
        Ok(wallets) => Html(WalletsTemplate { wallets }.to_string()).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn suitable_personas_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match with_db(state.clone(), move |conn| {
        let personas = queries::suitable_personas_wallets(conn, 20)?;
        let (suitable_count, evaluated_count) = queries::suitable_personas_counts(conn)?;
        let excluded_count = queries::excluded_wallets_count(conn)?;
        let recent_exclusions = queries::excluded_wallets_latest(conn, 5, 0)?;
        Ok((
            personas,
            suitable_count,
            evaluated_count,
            excluded_count,
            recent_exclusions,
        ))
    })
    .await
    {
        Ok((personas, suitable_count, evaluated_count, excluded_count, recent_exclusions)) => Html(
            SuitablePersonasTemplate {
                personas,
                suitable_count,
                evaluated_count,
                excluded_count,
                recent_exclusions,
            }
            .to_string(),
        )
        .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn personas_summary_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match with_db(state.clone(), move |conn| {
        let (suitable_count, evaluated_count) = queries::suitable_personas_counts(conn)?;
        let excluded_count = queries::excluded_wallets_count(conn)?;
        Ok((suitable_count, evaluated_count, excluded_count))
    })
    .await
    {
        Ok((suitable_count, evaluated_count, excluded_count)) => Html(
            PersonasSummaryBarTemplate {
                suitable_count,
                evaluated_count,
                excluded_count,
            }
            .to_string(),
        )
        .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn paper_traded_wallets_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match with_db(state.clone(), move |conn| {
        let wallets = queries::paper_traded_wallets_list(conn, 20)?;
        Ok(wallets)
    })
    .await
    {
        Ok(wallets) => Html(PaperTradedWalletsTemplate { wallets }.to_string()).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn rankings_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match with_db(state.clone(), move |conn| {
        queries::follow_worthy_rankings(conn, None)
    })
    .await
    {
        Ok(rankings) => Html(RankingsTemplate { rankings }.to_string()).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
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
    match with_db(state.clone(), move |conn| {
        let total = queries::excluded_wallets_count(conn)?;
        let rows = queries::excluded_wallets_latest(conn, page_size as usize, offset)?;
        Ok((total, rows))
    })
    .await
    {
        Ok((total, rows)) => {
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
            .into_response()
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

/// Fetch Polymarket display name (name or pseudonym) from Gamma API. Returns None on error or if unset.
async fn fetch_polymarket_display_name(
    client: &reqwest::Client,
    gamma_api_url: &str,
    proxy_wallet: &str,
) -> Option<String> {
    let base = gamma_api_url.trim_end_matches('/');
    let url = format!("{base}/public-profile");
    let resp = client
        .get(&url)
        .query(&[("address", proxy_wallet)])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let name = json.get("name").and_then(|v| v.as_str()).map(str::trim);
    let pseudonym = json
        .get("pseudonym")
        .and_then(|v| v.as_str())
        .map(str::trim);
    name.filter(|s| !s.is_empty())
        .or_else(|| pseudonym.filter(|s| !s.is_empty()))
        .map(String::from)
}

async fn journey_page(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> impl IntoResponse {
    match with_db(state.clone(), move |conn| {
        queries::wallet_journey(conn, &wallet)
    })
    .await
    {
        Ok(Some(mut journey)) => {
            if let (Some(client), Some(url)) =
                (state.http_client.as_ref(), state.gamma_api_url.as_deref())
            {
                if let Some(name) =
                    fetch_polymarket_display_name(client, url, &journey.proxy_wallet).await
                {
                    journey.wallet_display_label = name;
                }
            }
            Html(JourneyTemplate { journey }.to_string()).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct WalletTradesQuery {
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_trades_limit")]
    limit: u32,
}

fn default_trades_limit() -> u32 {
    20
}

#[derive(Serialize)]
struct WalletTradesResponse {
    trades: Vec<models::WalletTradeRow>,
    total: u64,
}

#[derive(Debug, Deserialize)]
struct WalletActivityQuery {
    offset: u32,
    #[serde(default = "default_positions_limit")]
    limit: u32,
}

#[derive(Serialize)]
struct WalletActivityResponse {
    activities: Vec<models::WalletActivityRow>,
    total: u64,
}

#[derive(Debug, Deserialize)]
struct WalletPositionsQuery {
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_positions_limit")]
    limit: u32,
}

fn default_positions_limit() -> u32 {
    20
}

#[derive(Serialize)]
struct WalletPositionsResponse {
    positions: Vec<models::WalletPositionRow>,
    total: u64,
}

async fn scorecard_page(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> impl IntoResponse {
    match with_db(state.clone(), move |conn| {
        queries::wallet_journey(conn, &wallet)
    })
    .await
    {
        Ok(Some(mut journey)) => {
            if let (Some(client), Some(url)) =
                (state.http_client.as_ref(), state.gamma_api_url.as_deref())
            {
                if let Some(name) =
                    fetch_polymarket_display_name(client, url, &journey.proxy_wallet).await
                {
                    journey.wallet_display_label = name;
                }
            }
            Html(ScorecardTemplate { journey }.to_string()).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("DB unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn wallet_trades_json(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
    Query(q): Query<WalletTradesQuery>,
) -> impl IntoResponse {
    match with_db(state.clone(), move |conn| {
        queries::wallet_trades_page(conn, &wallet, q.offset, q.limit)
    })
    .await
    {
        Ok((trades, total)) => Json(WalletTradesResponse { trades, total }).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(WalletTradesResponse {
                trades: vec![],
                total: 0,
            }),
        )
            .into_response(),
    }
}

async fn wallet_positions_json(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
    Query(q): Query<WalletPositionsQuery>,
) -> impl IntoResponse {
    let limit = q.limit.min(100);
    match with_db(state.clone(), move |conn| {
        queries::wallet_positions_page(conn, &wallet, q.offset, limit)
    })
    .await
    {
        Ok((positions, total)) => Json(WalletPositionsResponse {
            positions,
            total: total as u64,
        })
        .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(WalletPositionsResponse {
                positions: vec![],
                total: 0,
            }),
        )
            .into_response(),
    }
}

async fn wallet_active_positions_json(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
    Query(q): Query<WalletPositionsQuery>,
) -> impl IntoResponse {
    let limit = q.limit.min(100);
    match with_db(state.clone(), move |conn| {
        queries::wallet_active_positions_page(conn, &wallet, q.offset, limit)
    })
    .await
    {
        Ok((positions, total)) => Json(WalletPositionsResponse {
            positions,
            total: total as u64,
        })
        .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(WalletPositionsResponse {
                positions: vec![],
                total: 0,
            }),
        )
            .into_response(),
    }
}

async fn wallet_closed_positions_json(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
    Query(q): Query<WalletPositionsQuery>,
) -> impl IntoResponse {
    let limit = q.limit.min(100);
    match with_db(state.clone(), move |conn| {
        queries::wallet_closed_positions_page(conn, &wallet, q.offset, limit)
    })
    .await
    {
        Ok((positions, total)) => Json(WalletPositionsResponse {
            positions,
            total: total as u64,
        })
        .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(WalletPositionsResponse {
                positions: vec![],
                total: 0,
            }),
        )
            .into_response(),
    }
}

async fn wallet_activity_json(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
    Query(q): Query<WalletActivityQuery>,
) -> impl IntoResponse {
    let limit = q.limit.min(100);
    match with_db(state.clone(), move |conn| {
        queries::wallet_activity_page(conn, &wallet, q.offset, limit)
    })
    .await
    {
        Ok((activities, total)) => Json(WalletActivityResponse {
            activities,
            total: total as u64,
        })
        .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(WalletActivityResponse {
                activities: vec![],
                total: 0,
            }),
        )
            .into_response(),
    }
}

async fn spawn_derived_gauges_updater(state: Arc<AppState>) {
    // Best-effort: these are derived metrics for UI/Grafana; failures should never take down web.
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;

        if let Ok(c) = with_db(state.clone(), queries::funnel_counts).await {
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

        if let Ok(c) = with_db(state.clone(), queries::persona_funnel_counts).await {
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
        .layer(middleware::from_fn_with_state(
            state.login_rate_limiter.clone(),
            login_rate_limit_middleware,
        )); // Apply rate limiting only to login

    // Protected routes (auth required if password is set)
    let protected_routes = Router::new()
        .route("/", get(index))
        .route("/excluded", get(excluded_page))
        .route("/journey/{wallet}", get(journey_page))
        .route("/wallet/{wallet}", get(scorecard_page))
        .route("/wallet/{wallet}/trades", get(wallet_trades_json))
        .route("/wallet/{wallet}/positions", get(wallet_positions_json))
        .route(
            "/wallet/{wallet}/active-positions",
            get(wallet_active_positions_json),
        )
        .route(
            "/wallet/{wallet}/closed-positions",
            get(wallet_closed_positions_json),
        )
        .route("/wallet/{wallet}/activity", get(wallet_activity_json))
        .route("/partials/status", get(status_partial))
        .route("/partials/async_funnel", get(async_funnel_partial))
        .route("/partials/unified_funnel", get(unified_funnel_partial))
        .route("/partials/markets", get(markets_partial))
        .route("/partials/events", get(events_partial))
        .route("/partials/wallets", get(wallets_partial))
        .route(
            "/partials/suitable_personas",
            get(suitable_personas_partial),
        )
        .route("/partials/personas_summary", get(personas_summary_partial))
        .route(
            "/partials/paper_traded_wallets",
            get(paper_traded_wallets_partial),
        )
        .route("/partials/rankings", get(rankings_partial))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    public_routes
        .merge(protected_routes)
        .layer(middleware::from_fn(security_headers_middleware)) // Security headers for all responses
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load config — use [web] section if present, otherwise defaults
    let config = common::config::Config::load()?;

    let (dispatch, _otel_guard) =
        common::observability::build_dispatch("evaluator-web", &config.general.log_level);
    tracing::dispatcher::set_global_default(dispatch).map_err(anyhow::Error::msg)?;

    // Prometheus endpoint for web service health. Alloy scrapes this on localhost:3000.
    let metrics_addr: SocketAddr = ([127, 0, 0, 1], 3000).into();
    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Prefix("evaluator_".to_string()),
            &[
                1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0,
                10000.0,
            ],
        )
        .map_err(anyhow::Error::from)?
        .with_http_listener(metrics_addr)
        .install()
        .map_err(anyhow::Error::msg)?;
    let db_path = PathBuf::from(&config.database.path);
    let web_port = config.web.as_ref().map_or(8080, |w| w.port);
    let web_host = config
        .web
        .as_ref()
        .map_or("0.0.0.0".to_string(), |w| w.host.clone());
    let auth_password = config.web.as_ref().and_then(|w| w.auth_password.clone());
    let funnel_stage_infos = common::funnel::funnel_stage_infos(&config);
    metrics::init()?;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok();
    let state = Arc::new(AppState {
        db_path,
        auth_password,
        funnel_stage_infos,
        db_semaphore: Arc::new(Semaphore::new(8)),
        login_rate_limiter: Arc::new(LoginRateLimiter::new()),
        db_timeout: Duration::from_secs(5),
        db_open_delay: Duration::ZERO,
        paper_bankroll_usdc: config.risk.paper_bankroll_usdc,
        max_total_exposure_pct: config.paper_trading.max_total_exposure_pct,
        max_daily_loss_pct: config.paper_trading.max_daily_loss_pct,
        max_concurrent_positions: i64::from(config.risk.max_concurrent_positions),
        gamma_api_url: Some(config.polymarket.gamma_api_url.clone()),
        http_client,
    });

    tokio::spawn(spawn_derived_gauges_updater(state.clone()));

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
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tower::ServiceExt;

    #[test]
    fn test_funnel_info_icon_uses_css_tooltip_data_tip() {
        let info = "a>=b and \"quoted\"".to_string();
        let stages = vec![FunnelStage {
            label: "Markets".to_string(),
            count: 1,
            drop_pct: None,
            bg_color: "bg-gray-800".to_string(),
            drop_color: String::new(),
            info,
        }];

        let html = FunnelBarTemplate { stages }.to_string();

        // We intentionally avoid native `title` tooltips because some environments suppress them.
        assert!(html.contains("class=\"help-tip"));
        assert!(html.contains("data-tip=\""));
        // The funnel bar must not clip tooltips vertically; Tailwind's `overflow-x-auto` sets
        // `overflow-y: hidden`, so we use a custom scroll class instead.
        assert!(html.contains("funnel-scroll"));
        // Ensure the tooltip content is present and HTML-escaped for attributes.
        assert!(html.contains("a&gt;=b") || html.contains("a&#62;=b"));
        assert!(
            html.contains("&quot;quoted&quot;") || html.contains("&#34;quoted&#34;"),
            "expected quotes to be HTML-escaped in attribute"
        );
        // Ensure the help-tip span itself has no `title=...` attribute (avoid native tooltips).
        let help_tip_start = html
            .find("class=\"help-tip")
            .expect("render must include help-tip span");
        let help_tip_tag_end = html[help_tip_start..]
            .find('>')
            .map(|i| help_tip_start + i)
            .expect("help-tip tag must have a closing '>'");
        let help_tip_tag = &html[help_tip_start..help_tip_tag_end];
        assert!(
            !help_tip_tag.contains("title=\""),
            "help-tip span should not use native title tooltips"
        );
    }

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
        metrics::init().unwrap();
        let state = Arc::new(AppState {
            db_path: path,
            auth_password: None,
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            db_semaphore: Arc::new(Semaphore::new(8)),
            login_rate_limiter: Arc::new(LoginRateLimiter::new()),
            db_timeout: Duration::from_secs(5),
            db_open_delay: Duration::ZERO,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: i64::from(cfg.risk.max_concurrent_positions),
            gamma_api_url: None,
            http_client: None,
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
        metrics::init().unwrap();
        let state = Arc::new(AppState {
            db_path: path,
            auth_password: Some(password.to_string()),
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            db_semaphore: Arc::new(Semaphore::new(8)),
            login_rate_limiter: Arc::new(LoginRateLimiter::new()),
            db_timeout: Duration::from_secs(5),
            db_open_delay: Duration::ZERO,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: i64::from(cfg.risk.max_concurrent_positions),
            gamma_api_url: None,
            http_client: None,
        });
        create_router_with_state(state)
    }

    fn create_test_app_with_auth_and_db_delay(password: &str, db_open_delay: Duration) -> Router {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let db = Database::open(path.to_str().unwrap()).unwrap();
        db.run_migrations().unwrap();
        drop(db);
        std::mem::forget(tmp);

        let cfg =
            common::config::Config::from_toml_str(include_str!("../../../config/default.toml"))
                .unwrap();
        metrics::init().unwrap();
        let state = Arc::new(AppState {
            db_path: path,
            auth_password: Some(password.to_string()),
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            db_semaphore: Arc::new(Semaphore::new(8)),
            db_timeout: Duration::from_secs(5),
            db_open_delay,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: i64::from(cfg.risk.max_concurrent_positions),
            login_rate_limiter: Arc::new(LoginRateLimiter::new()),
            gamma_api_url: None,
            http_client: None,
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_login_not_blocked_by_slow_db_queries() {
        fn http_get(
            addr: SocketAddr,
            path: &str,
            extra_headers: &[(&str, &str)],
            timeout: Duration,
        ) -> std::io::Result<String> {
            let mut stream = TcpStream::connect(addr)?;
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;

            let mut req =
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
            for (k, v) in extra_headers {
                req.push_str(&format!("{k}: {v}\r\n"));
            }
            req.push_str("\r\n");

            stream.write_all(req.as_bytes())?;
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf)?;
            Ok(String::from_utf8_lossy(&buf).to_string())
        }

        // Run a real server on a dedicated runtime thread, so the requests behave like production:
        // handlers are polled by the server runtime, not inline in the test future.
        let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<tokio::sync::oneshot::Sender<()>>();

        thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async move {
                let app =
                    create_test_app_with_auth_and_db_delay("secret", Duration::from_millis(400));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();

                let (sd_tx, sd_rx) = tokio::sync::oneshot::channel::<()>();
                addr_tx.send(addr).unwrap();
                shutdown_tx.send(sd_tx).unwrap();

                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = sd_rx.await;
                    })
                    .await
                    .unwrap();
            });
        });

        let addr = addr_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let sd = shutdown_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // Start two DB-backed requests that will occupy tokio worker threads if DB opens happen on them.
        let cookie = auth_cookie("secret");
        let addr1 = addr;
        let addr2 = addr;
        let cookie1 = cookie.clone();
        let cookie2 = cookie.clone();

        let h1 = thread::spawn(move || {
            let _ = http_get(
                addr1,
                "/partials/status",
                &[("Cookie", cookie1.as_str())],
                Duration::from_secs(2),
            );
        });
        let h2 = thread::spawn(move || {
            let _ = http_get(
                addr2,
                "/partials/status",
                &[("Cookie", cookie2.as_str())],
                Duration::from_secs(2),
            );
        });

        // Give the server a moment to accept and start processing the partial requests.
        thread::sleep(Duration::from_millis(25));

        // /login should stay responsive. In the broken implementation this times out (no worker threads left).
        let login_resp = http_get(addr, "/login", &[], Duration::from_millis(120));
        assert!(
            login_resp.is_ok(),
            "/login timed out while DB partials were in flight"
        );

        // Cleanup
        let _ = sd.send(());
        let _ = h1.join();
        let _ = h2.join();
    }

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
    async fn test_unified_funnel_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/unified_funnel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_unified_funnel_partial_contains_stages() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/unified_funnel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Events"));
        assert!(html.contains("All wallets"));
        assert!(html.contains("Suitable personas wallets"));
        assert!(html.contains("Actively paper traded"));
        assert!(html.contains("Worth following"));
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
        metrics::init().unwrap();
        let state = Arc::new(AppState {
            db_path: path,
            auth_password: None,
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            db_semaphore: Arc::new(Semaphore::new(8)),
            db_timeout: Duration::from_secs(5),
            db_open_delay: Duration::ZERO,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: i64::from(cfg.risk.max_concurrent_positions),
            login_rate_limiter: Arc::new(LoginRateLimiter::new()),
            gamma_api_url: None,
            http_client: None,
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
        metrics::init().unwrap();
        let state = Arc::new(AppState {
            db_path: path,
            auth_password: None,
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            db_semaphore: Arc::new(Semaphore::new(8)),
            db_timeout: Duration::from_secs(5),
            db_open_delay: Duration::ZERO,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: i64::from(cfg.risk.max_concurrent_positions),
            login_rate_limiter: Arc::new(LoginRateLimiter::new()),
            gamma_api_url: None,
            http_client: None,
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
        assert!(html.contains("No markets scored."));
    }

    #[tokio::test]
    async fn test_events_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_events_partial_empty_shows_message() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("No events scored."));
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
    async fn test_wallets_partial_contains_table_or_empty_message() {
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
        assert!(
            html.contains("Wallet") && html.contains("Source")
                || html.contains("No wallets discovered yet")
        );
    }

    #[tokio::test]
    async fn test_suitable_personas_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/suitable_personas")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_personas_summary_partial_returns_200_and_counts() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/personas_summary")
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
        assert!(
            html.contains("data-tip="),
            "personas summary should have tooltips"
        );
        assert!(
            html.contains("text-green-400")
                && html.contains("text-amber-400")
                && html.contains("text-red-400")
        );
    }

    #[tokio::test]
    async fn test_paper_traded_wallets_partial_returns_200() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/paper_traded_wallets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_paper_traded_wallets_partial_empty_shows_message() {
        let app = create_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/partials/paper_traded_wallets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("No wallets actively paper traded yet"));
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
            "/partials/async_funnel",
            "/partials/unified_funnel",
            "/partials/markets",
            "/partials/events",
            "/partials/wallets",
            "/partials/suitable_personas",
            "/partials/personas_summary",
            "/partials/paper_traded_wallets",
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
        assert!(html.contains("hx-get=\"/partials/unified_funnel\""));
        assert!(html.contains("hx-get=\"/partials/events\""));
        assert!(html.contains("hx-get=\"/partials/wallets\""));
        assert!(html.contains("hx-get=\"/partials/suitable_personas\""));
        assert!(html.contains("hx-get=\"/partials/paper_traded_wallets\""));
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
        assert!(html.contains("No wallet scores."));
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
        metrics::init().unwrap();
        let state = Arc::new(AppState {
            db_path: db_path.into(),
            auth_password: None,
            funnel_stage_infos: common::funnel::funnel_stage_infos(&cfg),
            db_semaphore: Arc::new(Semaphore::new(8)),
            db_timeout: Duration::from_secs(5),
            db_open_delay: Duration::ZERO,
            paper_bankroll_usdc: cfg.risk.paper_bankroll_usdc,
            max_total_exposure_pct: cfg.paper_trading.max_total_exposure_pct,
            max_daily_loss_pct: cfg.paper_trading.max_daily_loss_pct,
            max_concurrent_positions: i64::from(cfg.risk.max_concurrent_positions),
            login_rate_limiter: Arc::new(LoginRateLimiter::new()),
            gamma_api_url: None,
            http_client: None,
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
