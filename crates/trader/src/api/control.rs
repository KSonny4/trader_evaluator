use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::AppState;
use crate::config::RiskConfig;

#[derive(Serialize)]
pub struct ControlResponse {
    pub message: String,
}

pub async fn halt(State(state): State<Arc<AppState>>) -> Json<ControlResponse> {
    state.engine.lock().await.halt_all();
    Json(ControlResponse {
        message: "all trading halted".to_string(),
    })
}

pub async fn resume(State(state): State<Arc<AppState>>) -> Json<ControlResponse> {
    state.engine.lock().await.resume_all();
    Json(ControlResponse {
        message: "trading resumed".to_string(),
    })
}

#[derive(Serialize)]
pub struct RiskResponse {
    pub portfolio: RiskStateResponse,
}

#[derive(Serialize)]
pub struct RiskStateResponse {
    pub total_exposure_usd: f64,
    pub daily_pnl: f64,
    pub weekly_pnl: f64,
    pub total_pnl: f64,
    pub open_positions: i64,
    pub is_halted: bool,
}

pub async fn get_risk(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RiskResponse>, StatusCode> {
    let risk_state = state
        .db
        .call(|conn| {
            match conn.query_row(
                "SELECT total_exposure_usd, daily_pnl, weekly_pnl, current_pnl, open_positions, is_halted
                 FROM risk_state WHERE key = 'portfolio'",
                [],
                |row| {
                    Ok(RiskStateResponse {
                        total_exposure_usd: row.get(0)?,
                        daily_pnl: row.get(1)?,
                        weekly_pnl: row.get(2)?,
                        total_pnl: row.get(3)?,
                        open_positions: row.get(4)?,
                        is_halted: row.get::<_, i64>(5)? != 0,
                    })
                },
            ) {
                Ok(s) => Ok(s),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(RiskStateResponse {
                    total_exposure_usd: 0.0,
                    daily_pnl: 0.0,
                    weekly_pnl: 0.0,
                    total_pnl: 0.0,
                    open_positions: 0,
                    is_halted: false,
                }),
                Err(e) => Err(e),
            }
        })
        .await
        .map_err(|_db_err| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(RiskResponse {
        portfolio: risk_state,
    }))
}

#[derive(Deserialize)]
pub struct UpdateRiskRequest {
    pub risk: RiskConfig,
}

pub async fn update_risk(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateRiskRequest>,
) -> Result<Json<ControlResponse>, StatusCode> {
    state.risk.update_config(req.risk).await;
    Ok(Json(ControlResponse {
        message: "risk parameters updated".to_string(),
    }))
}
