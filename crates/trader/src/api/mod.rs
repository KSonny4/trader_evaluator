pub mod control;
pub mod trades;
pub mod wallets;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::db::TraderDb;
use crate::engine::WalletEngine;
use crate::risk::RiskManager;

/// Shared application state available to all handlers.
pub struct AppState {
    pub db: Arc<TraderDb>,
    pub engine: Mutex<WalletEngine>,
    pub risk: Arc<RiskManager>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub api_key: Option<String>,
}

pub fn router(state: Arc<AppState>) -> Router {
    // Health endpoint is always public (no auth)
    let public = Router::new().route("/api/health", get(health));

    // Protected routes require bearer token (if api_key is configured)
    let protected = Router::new()
        .route("/api/status", get(status))
        // Wallet management
        .route(
            "/api/wallets",
            get(wallets::list_wallets).post(wallets::follow_wallet),
        )
        .route(
            "/api/wallets/{addr}",
            axum::routing::delete(wallets::unfollow_wallet),
        )
        .route(
            "/api/wallets/{addr}/pause",
            axum::routing::post(wallets::pause_wallet),
        )
        .route(
            "/api/wallets/{addr}/resume",
            axum::routing::post(wallets::resume_wallet),
        )
        // Trading data
        .route("/api/trades", get(trades::get_trades))
        .route("/api/positions", get(trades::get_positions))
        .route("/api/pnl", get(trades::get_pnl))
        // Control
        .route("/api/halt", axum::routing::post(control::halt))
        .route("/api/resume", axum::routing::post(control::resume))
        .route(
            "/api/risk",
            get(control::get_risk).put(control::update_risk),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    public.merge(protected).with_state(state)
}

/// Bearer token auth middleware. Skipped when no api_key is configured.
async fn auth_middleware(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let Some(api_key) = &state.api_key else {
        return next.run(req).await; // No key configured = dev mode
    };

    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if constant_time_eq(token.as_bytes(), api_key.as_bytes()) {
                next.run(req).await
            } else {
                StatusCode::UNAUTHORIZED.into_response()
            }
        }
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// Constant-time comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    uptime_secs: i64,
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = chrono::Utc::now()
        .signed_duration_since(state.started_at)
        .num_seconds();

    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: uptime,
    })
}

#[derive(Serialize)]
struct StatusResponse {
    running: bool,
    halted: bool,
    followed_wallets: i64,
    active_watchers: usize,
    open_positions: i64,
    uptime_secs: i64,
}

async fn status(State(state): State<Arc<AppState>>) -> Result<Json<StatusResponse>, StatusCode> {
    let uptime = chrono::Utc::now()
        .signed_duration_since(state.started_at)
        .num_seconds();

    let engine = state.engine.lock().await;
    let halted = engine.is_halted();
    let active_watchers = engine.watcher_count();
    drop(engine);

    let (wallets, positions) = state
        .db
        .call(|conn| {
            let wallets: i64 = conn.query_row(
                "SELECT COUNT(*) FROM followed_wallets WHERE status = 'active'",
                [],
                |row| row.get(0),
            )?;
            let positions: i64 =
                conn.query_row("SELECT COUNT(*) FROM trader_positions", [], |row| {
                    row.get(0)
                })?;
            Ok((wallets, positions))
        })
        .await
        .map_err(|_db_err| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(StatusResponse {
        running: true,
        halted,
        followed_wallets: wallets,
        active_watchers,
        open_positions: positions,
        uptime_secs: uptime,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TraderConfig;
    use crate::polymarket::TraderPolymarketClient;
    use crate::risk::RiskManager;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn test_app() -> (Router, Arc<AppState>) {
        let config = TraderConfig::load("config/trader.toml").unwrap();
        let client = Arc::new(TraderPolymarketClient::new(
            &config.polymarket.data_api_url,
            config.polymarket.rate_limit_delay_ms,
        ));
        let config = Arc::new(config);

        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let risk = Arc::new(RiskManager::new(Arc::clone(&db), config.risk.clone()));
        let engine = WalletEngine::new(
            Arc::clone(&db),
            client,
            Arc::clone(&config),
            Arc::clone(&risk),
        );

        let state = Arc::new(AppState {
            db: Arc::clone(&db),
            engine: Mutex::new(engine),
            risk,
            started_at: chrono::Utc::now(),
            api_key: None, // No auth in tests
        });
        let app = router(Arc::clone(&state));
        (app, state)
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let (app, _state) = test_app().await;
        let req = Request::builder()
            .uri("/api/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(json["uptime_secs"].as_i64().unwrap() >= 0);
    }

    #[tokio::test]
    async fn test_status_endpoint_empty() {
        let (app, _state) = test_app().await;
        let req = Request::builder()
            .uri("/api/status")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["running"], true);
        assert_eq!(json["halted"], false);
        assert_eq!(json["followed_wallets"], 0);
        assert_eq!(json["open_positions"], 0);
    }

    #[tokio::test]
    async fn test_list_wallets_empty() {
        let (app, _state) = test_app().await;
        let req = Request::builder()
            .uri("/api/wallets")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(json.is_empty());
    }
}
