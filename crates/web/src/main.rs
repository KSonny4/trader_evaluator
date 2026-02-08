mod models;
#[allow(dead_code)] // Query functions built ahead of handlers (tasks 7-11 will wire them)
mod queries;

use anyhow::Result;
use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::{routing::get, Router};
use models::{FunnelStage, SystemStatus};
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
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Default DB path — will be configurable in Task 13
    let db_path = PathBuf::from("data/evaluator.db");
    let state = Arc::new(AppState { db_path });

    let app = create_router_with_state(state);
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
}
