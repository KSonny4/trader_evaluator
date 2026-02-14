use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::AppState;
use crate::types::TradingMode;

#[derive(Deserialize)]
pub struct FollowRequest {
    pub proxy_wallet: String,
    pub label: Option<String>,
    pub estimated_bankroll_usd: Option<f64>,
    pub trading_mode: Option<String>,
}

#[derive(Serialize)]
pub struct WalletResponse {
    pub proxy_wallet: String,
    pub label: Option<String>,
    pub status: String,
    pub trading_mode: String,
    pub estimated_bankroll_usd: Option<f64>,
    pub added_at: String,
}

#[derive(Serialize)]
pub(crate) struct MessageResponse {
    pub message: String,
}

fn is_valid_wallet_address(addr: &str) -> bool {
    // Standard Ethereum address: 0x + 40 hex chars
    let is_eth = addr.len() == 42
        && addr.starts_with("0x")
        && addr[2..].chars().all(|c| c.is_ascii_hexdigit());
    // Polymarket proxy wallet (no prefix): 64 hex chars
    let is_proxy = addr.len() == 64 && addr.chars().all(|c| c.is_ascii_hexdigit());
    // Polymarket proxy wallet (with prefix): 0x + 64 hex chars
    let is_proxy_prefixed = addr.len() == 66
        && addr.starts_with("0x")
        && addr[2..].chars().all(|c| c.is_ascii_hexdigit());
    is_eth || is_proxy || is_proxy_prefixed
}

pub async fn follow_wallet(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FollowRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<MessageResponse>)> {
    if !is_valid_wallet_address(&req.proxy_wallet) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(MessageResponse {
                message: "invalid wallet address: must be 0x+40 hex (ETH), 64 hex (proxy), or 0x+64 hex (proxy with prefix)"
                    .to_string(),
            }),
        ));
    }

    if let Some(ref mode_str) = req.trading_mode {
        if TradingMode::from_str_loose(mode_str).is_none() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(MessageResponse {
                    message: format!(
                        "invalid trading_mode: {mode_str} (expected 'paper' or 'live')"
                    ),
                }),
            ));
        }
    }

    let mode = req
        .trading_mode
        .as_deref()
        .and_then(TradingMode::from_str_loose)
        .unwrap_or(TradingMode::Paper);

    state
        .engine
        .lock()
        .await
        .follow_wallet(
            req.proxy_wallet.clone(),
            req.label,
            req.estimated_bankroll_usd,
            mode,
        )
        .await
        .map_err(|_db_err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(MessageResponse {
                    message: "failed to follow wallet".to_string(),
                }),
            )
        })?;

    Ok((
        StatusCode::CREATED,
        Json(MessageResponse {
            message: format!("now following {}", req.proxy_wallet),
        }),
    ))
}

pub async fn unfollow_wallet(
    State(state): State<Arc<AppState>>,
    Path(addr): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    state
        .engine
        .lock()
        .await
        .unfollow_wallet(&addr)
        .await
        .map_err(|_db_err| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(MessageResponse {
        message: format!("unfollowed {addr}"),
    }))
}

pub async fn list_wallets(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<WalletResponse>>, StatusCode> {
    let wallets = state
        .db
        .call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT proxy_wallet, label, status, trading_mode, estimated_bankroll_usd, added_at
                 FROM followed_wallets
                 WHERE status != 'removed'
                 ORDER BY added_at DESC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(WalletResponse {
                        proxy_wallet: row.get(0)?,
                        label: row.get(1)?,
                        status: row.get(2)?,
                        trading_mode: row.get(3)?,
                        estimated_bankroll_usd: row.get(4)?,
                        added_at: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .map_err(|_db_err| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(wallets))
}

pub async fn pause_wallet(
    State(state): State<Arc<AppState>>,
    Path(addr): Path<String>,
) -> Result<Json<MessageResponse>, StatusCode> {
    state
        .engine
        .lock()
        .await
        .pause_wallet(&addr)
        .await
        .map_err(|_db_err| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(MessageResponse {
        message: format!("paused {addr}"),
    }))
}

pub async fn resume_wallet(
    State(state): State<Arc<AppState>>,
    Path(addr): Path<String>,
) -> Result<Json<MessageResponse>, StatusCode> {
    state
        .engine
        .lock()
        .await
        .resume_wallet(&addr)
        .await
        .map_err(|_db_err| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(MessageResponse {
        message: format!("resumed {addr}"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_wallet_address_eth() {
        assert!(is_valid_wallet_address(
            "0x1234567890abcdef1234567890abcdef12345678"
        ));
    }

    #[test]
    fn test_is_valid_wallet_address_proxy_no_prefix() {
        // 64 hex chars — Polymarket proxy wallet without 0x
        assert!(is_valid_wallet_address(
            "d67aeff736bfa5e32b269803f0809e84c07b61060e6eb520be9bc8aae30ed129"
        ));
    }

    #[test]
    fn test_is_valid_wallet_address_proxy_with_prefix() {
        // 0x + 64 hex chars — Polymarket proxy wallet with 0x prefix
        assert!(is_valid_wallet_address(
            "0xd67aeff736bfa5e32b269803f0809e84c07b61060e6eb520be9bc8aae30ed129"
        ));
    }

    #[test]
    fn test_is_valid_wallet_address_rejects_short() {
        assert!(!is_valid_wallet_address("0x1234"));
    }

    #[test]
    fn test_is_valid_wallet_address_rejects_non_hex() {
        assert!(!is_valid_wallet_address(
            "0xZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ"
        ));
    }

    #[test]
    fn test_is_valid_wallet_address_rejects_empty() {
        assert!(!is_valid_wallet_address(""));
    }
}
