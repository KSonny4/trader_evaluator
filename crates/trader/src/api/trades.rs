use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::AppState;

#[derive(Deserialize)]
pub struct TradesQuery {
    pub wallet: Option<String>,
    pub status: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Serialize)]
pub struct TradeResponse {
    pub id: i64,
    pub proxy_wallet: String,
    pub condition_id: String,
    pub side: String,
    pub their_price: f64,
    pub their_size_usd: f64,
    pub our_size_usd: f64,
    pub our_entry_price: f64,
    pub slippage_applied: f64,
    pub fee_applied: f64,
    pub sizing_method: String,
    pub trading_mode: String,
    pub status: String,
    pub pnl: Option<f64>,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct PositionResponse {
    pub proxy_wallet: String,
    pub condition_id: String,
    pub side: String,
    pub total_size_usd: f64,
    pub avg_entry_price: f64,
    pub share_count: f64,
    pub unrealized_pnl: f64,
}

#[derive(Serialize)]
pub struct PnlResponse {
    pub total_pnl: f64,
    pub realized_pnl: f64,
    pub open_trades: i64,
    pub settled_trades: i64,
    pub win_count: i64,
    pub loss_count: i64,
    pub win_rate: f64,
}

pub async fn get_trades(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<TradesQuery>,
) -> Result<Json<Vec<TradeResponse>>, StatusCode> {
    let wallet = query.wallet;
    let status = query.status;
    let limit = query.limit.unwrap_or(100).min(1000);

    let trades = state
        .db
        .call(move |conn| {
            let mut sql = String::from(
                "SELECT id, proxy_wallet, condition_id, side, their_price, their_size_usd,
                 our_size_usd, our_entry_price, slippage_applied, fee_applied,
                 sizing_method, trading_mode, status, pnl, created_at
                 FROM trader_trades WHERE 1=1",
            );
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(ref w) = wallet {
                sql.push_str(&format!(" AND proxy_wallet = ?{}", params.len() + 1));
                params.push(Box::new(w.clone()));
            }
            if let Some(ref s) = status {
                sql.push_str(&format!(" AND status = ?{}", params.len() + 1));
                params.push(Box::new(s.clone()));
            }
            sql.push_str(&format!(
                " ORDER BY created_at DESC LIMIT ?{}",
                params.len() + 1
            ));
            params.push(Box::new(limit));

            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(AsRef::as_ref).collect();

            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(param_refs.as_slice(), |row| {
                    Ok(TradeResponse {
                        id: row.get(0)?,
                        proxy_wallet: row.get(1)?,
                        condition_id: row.get(2)?,
                        side: row.get(3)?,
                        their_price: row.get(4)?,
                        their_size_usd: row.get(5)?,
                        our_size_usd: row.get(6)?,
                        our_entry_price: row.get(7)?,
                        slippage_applied: row.get(8)?,
                        fee_applied: row.get(9)?,
                        sizing_method: row.get(10)?,
                        trading_mode: row.get(11)?,
                        status: row.get(12)?,
                        pnl: row.get(13)?,
                        created_at: row.get(14)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .map_err(|_db_err| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(trades))
}

pub async fn get_positions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PositionResponse>>, StatusCode> {
    let positions = state
        .db
        .call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT proxy_wallet, condition_id, side, total_size_usd, avg_entry_price, share_count, unrealized_pnl
                 FROM trader_positions ORDER BY proxy_wallet, condition_id",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(PositionResponse {
                        proxy_wallet: row.get(0)?,
                        condition_id: row.get(1)?,
                        side: row.get(2)?,
                        total_size_usd: row.get(3)?,
                        avg_entry_price: row.get(4)?,
                        share_count: row.get(5)?,
                        unrealized_pnl: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .map_err(|_db_err| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(positions))
}

pub async fn get_pnl(State(state): State<Arc<AppState>>) -> Result<Json<PnlResponse>, StatusCode> {
    let pnl = state
        .db
        .call(|conn| {
            let total_pnl: f64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(pnl), 0.0) FROM trader_trades WHERE pnl IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0.0);

            let realized_pnl: f64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(pnl), 0.0) FROM trader_trades WHERE status IN ('settled_win', 'settled_loss')",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0.0);

            let open_trades: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM trader_trades WHERE status = 'open'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            let settled_trades: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM trader_trades WHERE status IN ('settled_win', 'settled_loss')",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            let win_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM trader_trades WHERE status = 'settled_win'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            let loss_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM trader_trades WHERE status = 'settled_loss'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            let win_rate = if settled_trades > 0 {
                win_count as f64 / settled_trades as f64
            } else {
                0.0
            };

            Ok(PnlResponse {
                total_pnl,
                realized_pnl,
                open_trades,
                settled_trades,
                win_count,
                loss_count,
                win_rate,
            })
        })
        .await
        .map_err(|_db_err| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(pnl))
}
