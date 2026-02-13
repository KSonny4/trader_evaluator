use crate::config::TradingConfig;
use crate::db::TraderDb;
use crate::polymarket::RawTrade;
use crate::risk::fidelity;
use crate::risk::slippage;
use crate::risk::wallet as risk_wallet;
use crate::risk::{RiskManager, RiskRejection};
use crate::types::{FidelityOutcome, Side, TradingMode};
use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};

/// Result of a mirror attempt.
#[derive(Debug)]
pub struct MirrorResult {
    pub executed: bool,
    pub trade_id: Option<i64>,
    pub reason: Option<String>,
}

/// Quartic taker fee (Polymarket): fee = price * 0.25 * (price * (1 - price))^2
/// ONLY for crypto 15-min markets. All other markets: 0 fee.
pub fn quartic_taker_fee(price: f64) -> f64 {
    let p = price.clamp(0.0, 1.0);
    p * 0.25 * (p * (1.0 - p)).powi(2)
}

/// Check if a market is a crypto 15-minute market based on title/condition_id.
/// In a full implementation, this would query market metadata.
/// For now, we always assume non-crypto (fee = 0) since we don't have market metadata in trader DB.
pub fn compute_taker_fee(price: f64, is_crypto_15m: bool) -> f64 {
    if is_crypto_15m {
        quartic_taker_fee(price)
    } else {
        0.0
    }
}

/// Attempt to mirror a detected trade from a followed wallet.
/// Applies: proportional sizing, slippage, fees, risk checks, fidelity logging.
#[allow(clippy::too_many_arguments)]
pub async fn mirror_trade(
    db: &Arc<TraderDb>,
    risk: &Arc<RiskManager>,
    config: &TradingConfig,
    trade: &RawTrade,
    proxy_wallet: &str,
    trading_mode: TradingMode,
    detection_delay_ms: i64,
    estimated_their_bankroll: f64,
) -> Result<MirrorResult> {
    let trade_hash = crate::polymarket::TraderPolymarketClient::trade_hash(trade);

    // Parse trade fields
    let side = trade
        .side
        .as_deref()
        .and_then(Side::from_str_loose)
        .ok_or_else(|| anyhow::anyhow!("missing or invalid side"))?;

    let their_price: f64 = trade
        .price
        .as_deref()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("missing or invalid price"))?;

    let their_size_usd: f64 = trade
        .size
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    // Calculate our trade size
    let our_size_usd = if config.use_proportional_sizing {
        // Scale their trade size by our bankroll / their bankroll ratio
        let ratio = config.bankroll_usd / estimated_their_bankroll;
        let scaled = their_size_usd * ratio;
        // Clamp to per-trade max
        scaled.min(config.per_trade_size_usd)
    } else {
        config.per_trade_size_usd
    };

    // Check risk gates
    match risk
        .check_trade(proxy_wallet, our_size_usd, config.bankroll_usd)
        .await
    {
        Ok(()) => {}
        Err(rejection) => {
            let outcome = match &rejection {
                RiskRejection::GlobalHalt => FidelityOutcome::SkippedPortfolioRisk,
                RiskRejection::PortfolioExposure { .. }
                | RiskRejection::PortfolioDailyLoss { .. }
                | RiskRejection::PortfolioWeeklyLoss { .. }
                | RiskRejection::MaxPositions { .. } => FidelityOutcome::SkippedPortfolioRisk,
                RiskRejection::WalletExposure { .. }
                | RiskRejection::WalletDailyLoss { .. }
                | RiskRejection::WalletWeeklyLoss { .. }
                | RiskRejection::WalletDrawdown { .. } => FidelityOutcome::SkippedWalletRisk,
                RiskRejection::SlippageKill { .. } => FidelityOutcome::SkippedWalletRisk,
                RiskRejection::LowFidelity { .. } => FidelityOutcome::SkippedWalletRisk,
            };

            let reason = rejection.to_string();
            fidelity::log_fidelity(
                db,
                proxy_wallet,
                trade.condition_id.as_deref().unwrap_or(""),
                &trade_hash,
                outcome,
                Some(&reason),
            )
            .await?;

            warn!(
                wallet = proxy_wallet,
                reason = %reason,
                "mirror trade rejected by risk"
            );

            return Ok(MirrorResult {
                executed: false,
                trade_id: None,
                reason: Some(reason),
            });
        }
    }

    // Apply slippage
    let slippage_pct = config.slippage_default_cents / 100.0;
    let our_entry_price = match side {
        Side::Buy => (their_price + slippage_pct).min(0.99),
        Side::Sell => (their_price - slippage_pct).max(0.01),
    };

    // Apply fee (assume non-crypto for now — later can check market metadata)
    let fee = compute_taker_fee(our_entry_price, false);
    let our_entry_price_with_fee = match side {
        Side::Buy => our_entry_price + fee,
        Side::Sell => our_entry_price - fee,
    };

    let sizing_method = if config.use_proportional_sizing {
        "proportional"
    } else {
        "fixed"
    };

    let condition_id = trade.condition_id.clone().unwrap_or_default();
    let their_timestamp = trade.timestamp.unwrap_or(0);

    // Insert the trade
    let trade_id = insert_paper_trade(
        db,
        proxy_wallet,
        &condition_id,
        side,
        trade.outcome.as_deref(),
        trade.outcome_index,
        their_price,
        their_size_usd,
        &trade_hash,
        their_timestamp,
        our_size_usd,
        our_entry_price_with_fee,
        slippage_pct,
        fee,
        sizing_method,
        detection_delay_ms,
        trading_mode,
    )
    .await?;

    // Upsert position
    upsert_position(
        db,
        proxy_wallet,
        &condition_id,
        side,
        our_size_usd,
        our_entry_price_with_fee,
    )
    .await?;

    // Update risk state
    risk_wallet::update_wallet_risk_state(db, proxy_wallet, our_size_usd, 0.0).await?;
    risk_wallet::update_portfolio_risk_state(db, our_size_usd, 0.0).await?;

    // Log fidelity: COPIED
    fidelity::log_fidelity(
        db,
        proxy_wallet,
        &condition_id,
        &trade_hash,
        FidelityOutcome::Copied,
        None,
    )
    .await?;

    // Log slippage
    slippage::log_slippage(
        db,
        proxy_wallet,
        &condition_id,
        their_price,
        our_entry_price_with_fee,
        fee,
        &trade_hash,
        trade_id,
        detection_delay_ms,
    )
    .await?;

    info!(
        wallet = proxy_wallet,
        side = %side,
        condition_id = %condition_id,
        size = our_size_usd,
        price = our_entry_price_with_fee,
        mode = %trading_mode,
        "paper trade executed"
    );

    Ok(MirrorResult {
        executed: true,
        trade_id: Some(trade_id),
        reason: None,
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert_paper_trade(
    db: &Arc<TraderDb>,
    proxy_wallet: &str,
    condition_id: &str,
    side: Side,
    outcome: Option<&str>,
    outcome_index: Option<i32>,
    their_price: f64,
    their_size_usd: f64,
    their_trade_hash: &str,
    their_timestamp: i64,
    our_size_usd: f64,
    our_entry_price: f64,
    slippage_applied: f64,
    fee_applied: f64,
    sizing_method: &str,
    detection_delay_ms: i64,
    trading_mode: TradingMode,
) -> Result<i64> {
    let addr = proxy_wallet.to_string();
    let cid = condition_id.to_string();
    let side_str = side.to_string();
    let outcome_str = outcome.map(str::to_string);
    let hash = their_trade_hash.to_string();
    let method = sizing_method.to_string();
    let mode = trading_mode.to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let trade_id = db
        .call(move |conn| {
            conn.execute(
                "INSERT INTO trader_trades (proxy_wallet, condition_id, side, outcome, outcome_index,
                 their_price, their_size_usd, their_trade_hash, their_timestamp,
                 our_size_usd, our_entry_price, slippage_applied, fee_applied,
                 sizing_method, detection_delay_ms, trading_mode, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 'open', ?17)",
                rusqlite::params![
                    addr, cid, side_str, outcome_str, outcome_index,
                    their_price, their_size_usd, hash, their_timestamp,
                    our_size_usd, our_entry_price, slippage_applied, fee_applied,
                    method, detection_delay_ms, mode, now,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    Ok(trade_id)
}

async fn upsert_position(
    db: &Arc<TraderDb>,
    proxy_wallet: &str,
    condition_id: &str,
    side: Side,
    size_usd: f64,
    entry_price: f64,
) -> Result<()> {
    let addr = proxy_wallet.to_string();
    let cid = condition_id.to_string();
    let side_str = side.to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let shares = if entry_price > 0.0 {
        size_usd / entry_price
    } else {
        0.0
    };

    db.call(move |conn| {
        conn.execute(
            "INSERT INTO trader_positions (proxy_wallet, condition_id, side, total_size_usd, avg_entry_price, share_count, last_updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(proxy_wallet, condition_id, side) DO UPDATE SET
                total_size_usd = total_size_usd + ?4,
                avg_entry_price = (avg_entry_price * share_count + ?5 * ?6) / (share_count + ?6),
                share_count = share_count + ?6,
                last_updated_at = ?7",
            rusqlite::params![addr, cid, side_str, size_usd, entry_price, shares, now],
        )?;
        Ok(())
    })
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TraderConfig;

    async fn setup_test_db() -> Arc<TraderDb> {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        db.call(|conn| {
            conn.execute(
                "INSERT INTO followed_wallets (proxy_wallet, status, trading_mode, added_at, updated_at)
                 VALUES ('0xtest', 'active', 'paper', '2026-01-01', '2026-01-01')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        db
    }

    fn test_config() -> TradingConfig {
        let config = TraderConfig::load("config/trader.toml").unwrap();
        config.trading
    }

    fn make_raw_trade(price: &str, size: &str) -> RawTrade {
        RawTrade {
            id: Some("t-1".to_string()),
            proxy_wallet: Some("0xtest".to_string()),
            condition_id: Some("cond-1".to_string()),
            asset: None,
            size: Some(size.to_string()),
            price: Some(price.to_string()),
            timestamp: Some(1700000000),
            outcome: Some("Yes".to_string()),
            outcome_index: Some(0),
            side: Some("BUY".to_string()),
            transaction_hash: None,
        }
    }

    #[test]
    fn test_quartic_taker_fee() {
        // At p=0.50: fee = 0.50 * 0.25 * (0.50 * 0.50)^2 = 0.125 * 0.0625 ≈ 0.0078125
        let fee = quartic_taker_fee(0.50);
        assert!((fee - 0.0078125).abs() < 1e-10);

        // At p=0 and p=1: fee ≈ 0
        assert!(quartic_taker_fee(0.0).abs() < 1e-10);
        assert!(quartic_taker_fee(1.0).abs() < 1e-10);

        // At p=0.25
        let fee_25 = quartic_taker_fee(0.25);
        assert!(fee_25 > 0.0);
        assert!(fee_25 < 0.01);
    }

    #[test]
    fn test_compute_taker_fee_non_crypto() {
        assert!((compute_taker_fee(0.50, false)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_taker_fee_crypto() {
        let fee = compute_taker_fee(0.50, true);
        assert!(fee > 0.0);
    }

    #[tokio::test]
    async fn test_mirror_trade_happy_path() {
        let db = setup_test_db().await;
        let config = test_config();
        let risk_config = TraderConfig::load("config/trader.toml").unwrap().risk;
        let risk = Arc::new(RiskManager::new(Arc::clone(&db), risk_config));

        let trade = make_raw_trade("0.50", "200.0");

        let result = mirror_trade(
            &db,
            &risk,
            &config,
            &trade,
            "0xtest",
            TradingMode::Paper,
            500,
            5000.0,
        )
        .await
        .unwrap();

        assert!(result.executed);
        assert!(result.trade_id.is_some());

        // Verify trade in DB
        let count: i64 = db
            .call(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM trader_trades WHERE proxy_wallet = '0xtest'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(count, 1);

        // Verify position created
        let pos_count: i64 = db
            .call(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM trader_positions WHERE proxy_wallet = '0xtest'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(pos_count, 1);

        // Verify fidelity logged as COPIED
        let fidelity: String = db
            .call(|conn| {
                conn.query_row(
                    "SELECT outcome FROM copy_fidelity_log WHERE proxy_wallet = '0xtest'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(fidelity, "COPIED");
    }

    #[tokio::test]
    async fn test_mirror_trade_risk_rejected() {
        let db = setup_test_db().await;
        let config = test_config();
        let risk_config = TraderConfig::load("config/trader.toml").unwrap().risk;
        let risk = Arc::new(RiskManager::new(Arc::clone(&db), risk_config));
        risk.halt();

        let trade = make_raw_trade("0.50", "200.0");

        let result = mirror_trade(
            &db,
            &risk,
            &config,
            &trade,
            "0xtest",
            TradingMode::Paper,
            100,
            5000.0,
        )
        .await
        .unwrap();

        assert!(!result.executed);
        assert!(result.reason.is_some());
        assert!(result.reason.unwrap().contains("halt"));

        // Fidelity should show SKIPPED
        let fidelity: String = db
            .call(|conn| {
                conn.query_row(
                    "SELECT outcome FROM copy_fidelity_log WHERE proxy_wallet = '0xtest'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert!(fidelity.starts_with("SKIPPED"));
    }

    #[tokio::test]
    async fn test_proportional_sizing() {
        let db = setup_test_db().await;
        let mut config = test_config();
        config.use_proportional_sizing = true;
        config.bankroll_usd = 1000.0;
        config.per_trade_size_usd = 25.0;

        let risk_config = TraderConfig::load("config/trader.toml").unwrap().risk;
        let risk = Arc::new(RiskManager::new(Arc::clone(&db), risk_config));

        // Their trade: $200 on $5000 bankroll → 4%. Our bankroll $1000 → $40, clamped to $25
        let trade = make_raw_trade("0.50", "200.0");

        let result = mirror_trade(
            &db,
            &risk,
            &config,
            &trade,
            "0xtest",
            TradingMode::Paper,
            100,
            5000.0,
        )
        .await
        .unwrap();

        assert!(result.executed);
        let trade_id = result.trade_id.unwrap();

        let our_size: f64 = db
            .call(move |conn| {
                conn.query_row(
                    "SELECT our_size_usd FROM trader_trades WHERE id = ?1",
                    [trade_id],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();

        // $200 * (1000/5000) = $40, clamped to $25
        assert!((our_size - 25.0).abs() < f64::EPSILON);
    }
}
