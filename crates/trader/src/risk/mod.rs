pub mod fidelity;
pub mod portfolio;
pub mod slippage;
pub mod wallet;

use crate::config::{PerWalletRiskConfig, PortfolioRiskConfig, RiskConfig};
use crate::db::TraderDb;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Centralized risk manager shared across all watchers via Arc.
pub struct RiskManager {
    db: Arc<TraderDb>,
    config: Arc<RwLock<RiskConfig>>,
    halted: Arc<std::sync::atomic::AtomicBool>,
}

impl RiskManager {
    pub fn new(db: Arc<TraderDb>, config: RiskConfig) -> Self {
        Self {
            db,
            config: Arc::new(RwLock::new(config)),
            halted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Check all risk gates before executing a trade.
    /// Returns Ok(()) if trade is allowed, Err with reason if blocked.
    pub async fn check_trade(
        &self,
        proxy_wallet: &str,
        trade_size_usd: f64,
        bankroll_usd: f64,
    ) -> Result<(), RiskRejection> {
        // Check global halt
        if self.halted.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(RiskRejection::GlobalHalt);
        }

        let config = self.config.read().await.clone();

        // Portfolio-level checks
        let portfolio_state = self.load_portfolio_state().await;
        self.check_portfolio(
            &config.portfolio,
            &portfolio_state,
            trade_size_usd,
            bankroll_usd,
        )?;

        // Per-wallet checks
        let wallet_state = self.load_wallet_state(proxy_wallet).await;
        self.check_wallet(
            &config.per_wallet,
            &wallet_state,
            trade_size_usd,
            bankroll_usd,
        )?;

        Ok(())
    }

    fn check_portfolio(
        &self,
        config: &PortfolioRiskConfig,
        state: &RiskState,
        trade_size_usd: f64,
        bankroll_usd: f64,
    ) -> Result<(), RiskRejection> {
        let max_exposure = bankroll_usd * config.max_total_exposure_pct / 100.0;
        if state.total_exposure_usd + trade_size_usd > max_exposure {
            return Err(RiskRejection::PortfolioExposure {
                current: state.total_exposure_usd,
                limit: max_exposure,
            });
        }

        let max_daily_loss = bankroll_usd * config.max_daily_loss_pct / 100.0;
        if state.daily_pnl < -max_daily_loss {
            return Err(RiskRejection::PortfolioDailyLoss {
                current: state.daily_pnl,
                limit: -max_daily_loss,
            });
        }

        let max_weekly_loss = bankroll_usd * config.max_weekly_loss_pct / 100.0;
        if state.weekly_pnl < -max_weekly_loss {
            return Err(RiskRejection::PortfolioWeeklyLoss {
                current: state.weekly_pnl,
                limit: -max_weekly_loss,
            });
        }

        if state.open_positions >= i64::from(config.max_concurrent_positions) {
            return Err(RiskRejection::MaxPositions {
                current: state.open_positions,
                limit: i64::from(config.max_concurrent_positions),
            });
        }

        Ok(())
    }

    fn check_wallet(
        &self,
        config: &PerWalletRiskConfig,
        state: &RiskState,
        trade_size_usd: f64,
        bankroll_usd: f64,
    ) -> Result<(), RiskRejection> {
        let max_exposure = bankroll_usd * config.max_exposure_pct / 100.0;
        if state.total_exposure_usd + trade_size_usd > max_exposure {
            return Err(RiskRejection::WalletExposure {
                current: state.total_exposure_usd,
                limit: max_exposure,
            });
        }

        let max_daily_loss = bankroll_usd * config.daily_loss_pct / 100.0;
        if state.daily_pnl < -max_daily_loss {
            return Err(RiskRejection::WalletDailyLoss {
                current: state.daily_pnl,
                limit: -max_daily_loss,
            });
        }

        let max_weekly_loss = bankroll_usd * config.weekly_loss_pct / 100.0;
        if state.weekly_pnl < -max_weekly_loss {
            return Err(RiskRejection::WalletWeeklyLoss {
                current: state.weekly_pnl,
                limit: -max_weekly_loss,
            });
        }

        // Drawdown check
        if state.peak_pnl > 0.0 {
            let drawdown_pct = (state.peak_pnl - state.current_pnl) / state.peak_pnl * 100.0;
            if drawdown_pct > config.max_drawdown_pct {
                return Err(RiskRejection::WalletDrawdown {
                    drawdown_pct,
                    limit: config.max_drawdown_pct,
                });
            }
        }

        Ok(())
    }

    /// Update risk config at runtime.
    pub async fn update_config(&self, new_config: RiskConfig) {
        *self.config.write().await = new_config;
        info!("risk config updated at runtime");
    }

    pub async fn get_config(&self) -> RiskConfig {
        self.config.read().await.clone()
    }

    pub fn halt(&self) {
        self.halted.store(true, std::sync::atomic::Ordering::SeqCst);
        warn!("risk manager: HALT activated");
    }

    pub fn resume(&self) {
        self.halted
            .store(false, std::sync::atomic::Ordering::SeqCst);
        info!("risk manager: resumed");
    }

    async fn load_portfolio_state(&self) -> RiskState {
        self.db
            .call(|conn| {
                conn.query_row(
                    "SELECT total_exposure_usd, daily_pnl, weekly_pnl, peak_pnl, current_pnl, open_positions
                     FROM risk_state WHERE key = 'portfolio'",
                    [],
                    |row| {
                        Ok(RiskState {
                            total_exposure_usd: row.get(0)?,
                            daily_pnl: row.get(1)?,
                            weekly_pnl: row.get(2)?,
                            peak_pnl: row.get(3)?,
                            current_pnl: row.get(4)?,
                            open_positions: row.get(5)?,
                        })
                    },
                )
            })
            .await
            .unwrap_or(RiskState::default())
    }

    async fn load_wallet_state(&self, wallet: &str) -> RiskState {
        let addr = wallet.to_string();
        self.db
            .call(move |conn| {
                conn.query_row(
                    "SELECT total_exposure_usd, daily_pnl, weekly_pnl, peak_pnl, current_pnl, open_positions
                     FROM risk_state WHERE key = ?1",
                    [addr],
                    |row| {
                        Ok(RiskState {
                            total_exposure_usd: row.get(0)?,
                            daily_pnl: row.get(1)?,
                            weekly_pnl: row.get(2)?,
                            peak_pnl: row.get(3)?,
                            current_pnl: row.get(4)?,
                            open_positions: row.get(5)?,
                        })
                    },
                )
            })
            .await
            .unwrap_or(RiskState::default())
    }
}

#[derive(Debug, Default)]
pub struct RiskState {
    pub total_exposure_usd: f64,
    pub daily_pnl: f64,
    pub weekly_pnl: f64,
    pub peak_pnl: f64,
    pub current_pnl: f64,
    pub open_positions: i64,
}

#[derive(Debug, Clone)]
pub enum RiskRejection {
    GlobalHalt,
    PortfolioExposure {
        current: f64,
        limit: f64,
    },
    PortfolioDailyLoss {
        current: f64,
        limit: f64,
    },
    PortfolioWeeklyLoss {
        current: f64,
        limit: f64,
    },
    MaxPositions {
        current: i64,
        limit: i64,
    },
    WalletExposure {
        current: f64,
        limit: f64,
    },
    WalletDailyLoss {
        current: f64,
        limit: f64,
    },
    WalletWeeklyLoss {
        current: f64,
        limit: f64,
    },
    WalletDrawdown {
        drawdown_pct: f64,
        limit: f64,
    },
    SlippageKill {
        avg_slippage: f64,
        threshold: f64,
    },
    LowFidelity {
        fidelity_pct: f64,
        min_required: f64,
    },
}

impl std::fmt::Display for RiskRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GlobalHalt => write!(f, "global halt active"),
            Self::PortfolioExposure { current, limit } => {
                write!(
                    f,
                    "portfolio exposure ${current:.2} exceeds limit ${limit:.2}"
                )
            }
            Self::PortfolioDailyLoss { current, limit } => {
                write!(
                    f,
                    "portfolio daily loss ${current:.2} exceeds limit ${limit:.2}"
                )
            }
            Self::PortfolioWeeklyLoss { current, limit } => {
                write!(
                    f,
                    "portfolio weekly loss ${current:.2} exceeds limit ${limit:.2}"
                )
            }
            Self::MaxPositions { current, limit } => {
                write!(f, "max positions {current} reached (limit {limit})")
            }
            Self::WalletExposure { current, limit } => {
                write!(f, "wallet exposure ${current:.2} exceeds limit ${limit:.2}")
            }
            Self::WalletDailyLoss { current, limit } => {
                write!(
                    f,
                    "wallet daily loss ${current:.2} exceeds limit ${limit:.2}"
                )
            }
            Self::WalletWeeklyLoss { current, limit } => {
                write!(
                    f,
                    "wallet weekly loss ${current:.2} exceeds limit ${limit:.2}"
                )
            }
            Self::WalletDrawdown {
                drawdown_pct,
                limit,
            } => write!(
                f,
                "wallet drawdown {drawdown_pct:.1}% exceeds limit {limit:.1}%"
            ),
            Self::SlippageKill {
                avg_slippage,
                threshold,
            } => write!(
                f,
                "avg slippage {avg_slippage:.2}c exceeds kill threshold {threshold:.2}c"
            ),
            Self::LowFidelity {
                fidelity_pct,
                min_required,
            } => write!(
                f,
                "copy fidelity {fidelity_pct:.1}% below minimum {min_required:.1}%"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_risk_config() -> RiskConfig {
        RiskConfig {
            portfolio: PortfolioRiskConfig {
                max_total_exposure_pct: 15.0,
                max_daily_loss_pct: 3.0,
                max_weekly_loss_pct: 8.0,
                max_concurrent_positions: 20,
            },
            per_wallet: PerWalletRiskConfig {
                max_exposure_pct: 5.0,
                daily_loss_pct: 2.0,
                weekly_loss_pct: 5.0,
                max_drawdown_pct: 15.0,
                min_copy_fidelity_pct: 80.0,
            },
        }
    }

    #[tokio::test]
    async fn test_risk_check_passes_empty_state() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let rm = RiskManager::new(db, test_risk_config());

        let result = rm.check_trade("0xtest", 25.0, 1000.0).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_risk_check_global_halt() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let rm = RiskManager::new(db, test_risk_config());
        rm.halt();

        let result = rm.check_trade("0xtest", 25.0, 1000.0).await;
        assert!(matches!(result, Err(RiskRejection::GlobalHalt)));
    }

    #[tokio::test]
    async fn test_portfolio_exposure_limit() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        // Insert portfolio state at near-limit exposure
        let now = chrono::Utc::now().to_rfc3339();
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO risk_state (key, total_exposure_usd, daily_pnl, weekly_pnl, peak_pnl, current_pnl, open_positions, updated_at)
                 VALUES ('portfolio', 140.0, 0.0, 0.0, 0.0, 0.0, 5, ?1)",
                [now],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let rm = RiskManager::new(db, test_risk_config());

        // $140 exposure + $25 = $165, limit is $150 (15% of $1000)
        let result = rm.check_trade("0xtest", 25.0, 1000.0).await;
        assert!(matches!(
            result,
            Err(RiskRejection::PortfolioExposure { .. })
        ));
    }

    #[tokio::test]
    async fn test_wallet_exposure_limit() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        let now = chrono::Utc::now().to_rfc3339();
        let now2 = now.clone();
        db.call(move |conn| {
            // Portfolio state is fine
            conn.execute(
                "INSERT INTO risk_state (key, total_exposure_usd, daily_pnl, weekly_pnl, peak_pnl, current_pnl, open_positions, updated_at)
                 VALUES ('portfolio', 50.0, 0.0, 0.0, 0.0, 0.0, 2, ?1)",
                [&now],
            )?;
            // But wallet is near limit
            conn.execute(
                "INSERT INTO risk_state (key, total_exposure_usd, daily_pnl, weekly_pnl, peak_pnl, current_pnl, open_positions, updated_at)
                 VALUES ('0xtest', 40.0, 0.0, 0.0, 0.0, 0.0, 1, ?1)",
                [&now2],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let rm = RiskManager::new(db, test_risk_config());

        // Wallet $40 + $25 = $65, limit $50 (5% of $1000)
        let result = rm.check_trade("0xtest", 25.0, 1000.0).await;
        assert!(matches!(result, Err(RiskRejection::WalletExposure { .. })));
    }

    #[tokio::test]
    async fn test_max_positions_limit() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        let now = chrono::Utc::now().to_rfc3339();
        db.call(move |conn| {
            conn.execute(
                "INSERT INTO risk_state (key, total_exposure_usd, daily_pnl, weekly_pnl, peak_pnl, current_pnl, open_positions, updated_at)
                 VALUES ('portfolio', 50.0, 0.0, 0.0, 0.0, 0.0, 20, ?1)",
                [now],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let rm = RiskManager::new(db, test_risk_config());
        let result = rm.check_trade("0xtest", 25.0, 1000.0).await;
        assert!(matches!(result, Err(RiskRejection::MaxPositions { .. })));
    }

    #[tokio::test]
    async fn test_risk_config_update() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let rm = RiskManager::new(db, test_risk_config());

        let config = rm.get_config().await;
        assert!((config.portfolio.max_total_exposure_pct - 15.0).abs() < f64::EPSILON);

        let mut new_config = test_risk_config();
        new_config.portfolio.max_total_exposure_pct = 25.0;
        rm.update_config(new_config).await;

        let config = rm.get_config().await;
        assert!((config.portfolio.max_total_exposure_pct - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_risk_rejection_display() {
        let r = RiskRejection::PortfolioExposure {
            current: 140.0,
            limit: 150.0,
        };
        assert!(r.to_string().contains("140.00"));
        assert!(r.to_string().contains("150.00"));
    }
}
