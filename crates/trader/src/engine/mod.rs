pub mod detector;
pub mod mirror;
pub mod settlement;
pub mod watcher;

use crate::config::TraderConfig;
use crate::db::TraderDb;
use crate::polymarket::TraderPolymarketClient;
use crate::risk::RiskManager;
use crate::types::{TradingMode, WalletStatus};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Handle to a running wallet watcher task.
struct WatcherHandle {
    cancel: CancellationToken,
    #[allow(dead_code)] // Stored for graceful shutdown via join
    handle: JoinHandle<()>,
}

/// Information about a followed wallet loaded from DB.
#[derive(Debug, Clone)]
pub struct FollowedWallet {
    pub proxy_wallet: String,
    #[allow(dead_code)] // Loaded from DB for future display
    pub label: Option<String>,
    #[allow(dead_code)] // Loaded from DB for lifecycle management
    pub status: WalletStatus,
    pub trading_mode: TradingMode,
    #[allow(dead_code)] // Loaded from DB for per-wallet risk overrides
    pub max_exposure_pct: Option<f64>,
    pub estimated_bankroll_usd: Option<f64>,
    pub last_trade_seen_hash: Option<String>,
}

/// The wallet engine orchestrates all wallet watchers.
pub struct WalletEngine {
    db: Arc<TraderDb>,
    client: Arc<TraderPolymarketClient>,
    config: Arc<TraderConfig>,
    risk: Arc<RiskManager>,
    watchers: HashMap<String, WatcherHandle>,
    halted: Arc<AtomicBool>,
}

impl WalletEngine {
    pub fn new(
        db: Arc<TraderDb>,
        client: Arc<TraderPolymarketClient>,
        config: Arc<TraderConfig>,
        risk: Arc<RiskManager>,
    ) -> Self {
        Self {
            db,
            client,
            config,
            risk,
            watchers: HashMap::new(),
            halted: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Load active wallets from DB and spawn watchers for each.
    pub async fn restore_watchers(&mut self) -> Result<()> {
        let wallets = self.load_active_wallets().await?;
        info!(count = wallets.len(), "restoring wallet watchers from DB");

        for wallet in wallets {
            if let Err(e) = self.spawn_watcher(wallet) {
                error!(error = %e, "failed to spawn restored watcher");
            }
        }

        Ok(())
    }

    /// Follow a new wallet — insert into DB and spawn a watcher.
    pub async fn follow_wallet(
        &mut self,
        proxy_wallet: String,
        label: Option<String>,
        estimated_bankroll_usd: Option<f64>,
        trading_mode: TradingMode,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let wallet_addr = proxy_wallet.clone();
        let label_clone = label.clone();
        let mode_str = trading_mode.to_string();
        let now_clone = now.clone();

        self.db
            .call(move |conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO followed_wallets
                     (proxy_wallet, label, status, trading_mode, estimated_bankroll_usd, added_at, updated_at)
                     VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        wallet_addr,
                        label_clone,
                        mode_str,
                        estimated_bankroll_usd,
                        now_clone,
                        now_clone,
                    ],
                )?;
                Ok(())
            })
            .await
            .context("failed to insert followed wallet")?;

        info!(wallet = %proxy_wallet, mode = %trading_mode, "following wallet");

        let wallet_info = FollowedWallet {
            proxy_wallet,
            label,
            status: WalletStatus::Active,
            trading_mode,
            max_exposure_pct: None,
            estimated_bankroll_usd,
            last_trade_seen_hash: None,
        };

        self.spawn_watcher(wallet_info)?;
        Ok(())
    }

    /// Stop following a wallet — cancel watcher and update DB.
    pub async fn unfollow_wallet(&mut self, proxy_wallet: &str) -> Result<()> {
        if let Some(handle) = self.watchers.remove(proxy_wallet) {
            handle.cancel.cancel();
            // Don't await the handle — let it clean up in background
        }

        let addr = proxy_wallet.to_string();
        let now = chrono::Utc::now().to_rfc3339();
        self.db
            .call(move |conn| {
                conn.execute(
                    "UPDATE followed_wallets SET status = 'removed', updated_at = ?1 WHERE proxy_wallet = ?2",
                    rusqlite::params![now, addr],
                )?;
                Ok(())
            })
            .await
            .context("failed to update wallet status to removed")?;

        info!(wallet = proxy_wallet, "unfollowed wallet");
        Ok(())
    }

    /// Pause a wallet — stop executing trades but keep watching.
    pub async fn pause_wallet(&mut self, proxy_wallet: &str) -> Result<()> {
        let addr = proxy_wallet.to_string();
        let now = chrono::Utc::now().to_rfc3339();
        self.db
            .call(move |conn| {
                conn.execute(
                    "UPDATE followed_wallets SET status = 'paused', updated_at = ?1 WHERE proxy_wallet = ?2",
                    rusqlite::params![now, addr],
                )?;
                Ok(())
            })
            .await
            .context("failed to pause wallet")?;

        info!(wallet = proxy_wallet, "paused wallet");
        Ok(())
    }

    /// Resume a paused wallet.
    pub async fn resume_wallet(&mut self, proxy_wallet: &str) -> Result<()> {
        let addr = proxy_wallet.to_string();
        let now = chrono::Utc::now().to_rfc3339();
        self.db
            .call(move |conn| {
                conn.execute(
                    "UPDATE followed_wallets SET status = 'active', updated_at = ?1 WHERE proxy_wallet = ?2",
                    rusqlite::params![now, addr],
                )?;
                Ok(())
            })
            .await
            .context("failed to resume wallet")?;

        // If no watcher running, load metadata from DB and spawn one
        if !self.watchers.contains_key(proxy_wallet) {
            let addr = proxy_wallet.to_string();
            let wallet = self
                .db
                .call(move |conn| {
                    conn.query_row(
                        "SELECT proxy_wallet, label, trading_mode, max_exposure_pct, estimated_bankroll_usd, last_trade_seen_hash
                         FROM followed_wallets WHERE proxy_wallet = ?1",
                        [&addr],
                        |row| {
                            let mode_str: String = row.get(2)?;
                            Ok(FollowedWallet {
                                proxy_wallet: row.get(0)?,
                                label: row.get(1)?,
                                status: WalletStatus::Active,
                                trading_mode: TradingMode::from_str_loose(&mode_str)
                                    .unwrap_or(TradingMode::Paper),
                                max_exposure_pct: row.get(3)?,
                                estimated_bankroll_usd: row.get(4)?,
                                last_trade_seen_hash: row.get(5)?,
                            })
                        },
                    )
                })
                .await
                .context("failed to load wallet metadata for resume")?;
            self.spawn_watcher(wallet)?;
        }

        info!(wallet = proxy_wallet, "resumed wallet");
        Ok(())
    }

    /// Emergency halt all trading.
    pub fn halt_all(&self) {
        self.halted.store(true, Ordering::SeqCst);
        warn!("ALL TRADING HALTED");
    }

    /// Resume trading after halt.
    pub fn resume_all(&self) {
        self.halted.store(false, Ordering::SeqCst);
        info!("trading resumed");
    }

    pub fn is_halted(&self) -> bool {
        self.halted.load(Ordering::SeqCst)
    }

    pub fn watcher_count(&self) -> usize {
        self.watchers.len()
    }

    /// Shut down all watchers gracefully.
    #[allow(dead_code)] // Used in tests for clean shutdown
    pub async fn shutdown(&mut self) {
        info!(
            count = self.watchers.len(),
            "shutting down all wallet watchers"
        );
        for (addr, handle) in self.watchers.drain() {
            handle.cancel.cancel();
            if let Err(e) = handle.handle.await {
                error!(wallet = %addr, error = %e, "watcher task panicked on shutdown");
            }
        }
    }

    fn spawn_watcher(&mut self, wallet: FollowedWallet) -> Result<()> {
        let addr = wallet.proxy_wallet.clone();

        if self.watchers.contains_key(&addr) {
            warn!(wallet = %addr, "watcher already running, skipping");
            return Ok(());
        }

        let cancel = CancellationToken::new();
        let db = Arc::clone(&self.db);
        let client = Arc::clone(&self.client);
        let config = Arc::clone(&self.config);
        let risk = Arc::clone(&self.risk);
        let halted = Arc::clone(&self.halted);
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            watcher::run_watcher(db, client, config, risk, wallet, halted, cancel_clone).await;
        });

        self.watchers.insert(addr, WatcherHandle { cancel, handle });
        Ok(())
    }

    async fn load_active_wallets(&self) -> Result<Vec<FollowedWallet>> {
        self.db
            .call(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT proxy_wallet, label, status, trading_mode, max_exposure_pct, estimated_bankroll_usd, last_trade_seen_hash
                     FROM followed_wallets WHERE status = 'active'",
                )?;
                let wallets = stmt
                    .query_map([], |row| {
                        let status_str: String = row.get(2)?;
                        let mode_str: String = row.get(3)?;
                        Ok(FollowedWallet {
                            proxy_wallet: row.get(0)?,
                            label: row.get(1)?,
                            status: WalletStatus::from_str_loose(&status_str)
                                .unwrap_or(WalletStatus::Active),
                            trading_mode: TradingMode::from_str_loose(&mode_str)
                                .unwrap_or(TradingMode::Paper),
                            max_exposure_pct: row.get(4)?,
                            estimated_bankroll_usd: row.get(5)?,
                            last_trade_seen_hash: row.get(6)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(wallets)
            })
            .await
            .context("failed to load active wallets")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> TraderConfig {
        let content = std::fs::read_to_string("config/trader.toml").unwrap();
        TraderConfig::from_str(&content).unwrap()
    }

    fn test_engine(db: Arc<TraderDb>) -> (WalletEngine, Arc<TraderPolymarketClient>) {
        let client = Arc::new(TraderPolymarketClient::new(
            "https://data-api.polymarket.com",
            200,
        ));
        let config = Arc::new(test_config());
        let risk = Arc::new(RiskManager::new(Arc::clone(&db), config.risk.clone()));
        let engine = WalletEngine::new(db, Arc::clone(&client), config, risk);
        (engine, client)
    }

    #[tokio::test]
    async fn test_follow_and_unfollow_wallet() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let (mut engine, _client) = test_engine(Arc::clone(&db));

        // Follow a wallet
        engine
            .follow_wallet(
                "0xtest123".to_string(),
                Some("test".to_string()),
                Some(5000.0),
                TradingMode::Paper,
            )
            .await
            .unwrap();

        assert_eq!(engine.watcher_count(), 1);

        // Check DB
        let count: i64 = db
            .call(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM followed_wallets WHERE proxy_wallet = '0xtest123'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(count, 1);

        // Unfollow
        engine.unfollow_wallet("0xtest123").await.unwrap();
        assert_eq!(engine.watcher_count(), 0);

        // Check DB status is removed
        let status: String = db
            .call(|conn| {
                conn.query_row(
                    "SELECT status FROM followed_wallets WHERE proxy_wallet = '0xtest123'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(status, "removed");

        engine.shutdown().await;
    }

    #[tokio::test]
    async fn test_halt_and_resume() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let (engine, _client) = test_engine(db);

        assert!(!engine.is_halted());
        engine.halt_all();
        assert!(engine.is_halted());
        engine.resume_all();
        assert!(!engine.is_halted());
    }

    #[tokio::test]
    async fn test_restore_watchers_empty() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let (mut engine, _client) = test_engine(db);
        engine.restore_watchers().await.unwrap();
        assert_eq!(engine.watcher_count(), 0);

        engine.shutdown().await;
    }

    #[tokio::test]
    async fn test_duplicate_follow_replaces() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());
        let (mut engine, _client) = test_engine(db);

        engine
            .follow_wallet(
                "0xdup".to_string(),
                Some("first".to_string()),
                None,
                TradingMode::Paper,
            )
            .await
            .unwrap();

        // Follow same wallet again — should not double-spawn
        engine
            .follow_wallet(
                "0xdup".to_string(),
                Some("second".to_string()),
                None,
                TradingMode::Live,
            )
            .await
            .unwrap();

        // Only 1 watcher since spawn skips duplicates
        assert_eq!(engine.watcher_count(), 1);

        engine.shutdown().await;
    }
}
