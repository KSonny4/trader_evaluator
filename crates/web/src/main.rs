#[allow(dead_code)]
mod models;
#[allow(dead_code)]
mod queries;

use anyhow::Result;
use askama::Template;
use axum::response::{Html, IntoResponse};
use axum::{routing::get, Router};
use rusqlite::{Connection, OpenFlags};
use std::net::SocketAddr;
use std::path::PathBuf;

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

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate;

async fn index() -> impl IntoResponse {
    Html(DashboardTemplate.to_string())
}

pub fn create_router() -> Router {
    Router::new().route("/", get(index))
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
    use tower::ServiceExt;

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
}
