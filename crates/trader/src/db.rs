use anyhow::{Context, Result};
use tokio_rusqlite::Connection;
use tracing::info;

/// Async database wrapper for the trader's own SQLite database.
pub struct TraderDb {
    conn: Connection,
}

impl TraderDb {
    pub async fn open(path: &str) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create DB directory: {}", parent.display())
                })?;
            }
        }

        let conn = Connection::open(path)
            .await
            .with_context(|| format!("failed to open trader DB: {path}"))?;

        // Set pragmas
        conn.call(|conn| {
            conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 5000;",
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to set DB pragmas: {e}"))?;

        let db = Self { conn };
        db.run_migrations().await?;
        Ok(db)
    }

    /// Open an in-memory database (for tests).
    #[cfg(test)]
    pub async fn open_memory() -> Result<Self> {
        let conn = Connection::open(":memory:")
            .await
            .context("failed to open in-memory DB")?;

        conn.call(|conn| {
            conn.execute_batch("PRAGMA foreign_keys = ON;")?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to set pragmas: {e}"))?;

        let db = Self { conn };
        db.run_migrations().await?;
        Ok(db)
    }

    /// Execute a closure on the database connection.
    /// The closure receives `&mut rusqlite::Connection`.
    pub async fn call<F, R>(&self, function: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<R, rusqlite::Error> + Send + 'static,
        R: Send + 'static,
    {
        self.conn
            .call(function)
            .await
            .map_err(|e| anyhow::anyhow!("DB call failed: {e}"))
    }

    async fn run_migrations(&self) -> Result<()> {
        self.conn
            .call(|conn| {
                run_migrations_sync(conn)?;
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .map_err(|e| anyhow::anyhow!("failed to run trader DB migrations: {e}"))?;
        info!("trader DB migrations complete");
        Ok(())
    }
}

fn run_migrations_sync(conn: &mut rusqlite::Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER NOT NULL
        );",
    )?;

    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let migrations: Vec<(&str, &str)> = vec![
        ("001", include_str!("../migrations/001_initial.sql")),
        ("002", include_str!("../migrations/002_book_snapshots.sql")),
    ];

    for (i, (_name, sql)) in migrations.iter().enumerate() {
        let version = (i + 1) as i64;
        if version > current_version {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [version],
            )?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_open_memory_db() {
        let db = TraderDb::open_memory().await.unwrap();

        // Verify tables exist
        let tables: Vec<String> = db
            .call(|conn| {
                let mut stmt = conn
                    .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
                let rows = stmt
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<String>, _>>()?;
                Ok(rows)
            })
            .await
            .unwrap();

        assert!(tables.contains(&"followed_wallets".to_string()));
        assert!(tables.contains(&"trader_trades".to_string()));
        assert!(tables.contains(&"trader_positions".to_string()));
        assert!(tables.contains(&"copy_fidelity_log".to_string()));
        assert!(tables.contains(&"follower_slippage_log".to_string()));
        assert!(tables.contains(&"risk_state".to_string()));
        assert!(tables.contains(&"trade_events".to_string()));
        assert!(tables.contains(&"book_snapshots".to_string()));
        assert!(tables.contains(&"fillability_results".to_string()));
    }

    #[tokio::test]
    async fn test_migrations_idempotent() {
        let db = TraderDb::open_memory().await.unwrap();

        // Run migrations again — should not fail
        db.call(|conn| {
            run_migrations_sync(conn)?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_insert_and_query_followed_wallet() {
        let db = TraderDb::open_memory().await.unwrap();

        db.call(|conn| {
            conn.execute(
                "INSERT INTO followed_wallets (proxy_wallet, label, status, trading_mode, max_exposure_pct, estimated_bankroll_usd, added_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    "0xabc123",
                    "test_wallet",
                    "active",
                    "paper",
                    5.0,
                    5000.0,
                    "2026-01-01T00:00:00Z",
                    "2026-01-01T00:00:00Z",
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let wallet_label: String = db
            .call(|conn| {
                conn.query_row(
                    "SELECT label FROM followed_wallets WHERE proxy_wallet = ?1",
                    ["0xabc123"],
                    |row| row.get(0),
                )
            })
            .await
            .unwrap();

        assert_eq!(wallet_label, "test_wallet");
    }

    #[tokio::test]
    async fn test_open_file_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test_trader.db");
        let db = TraderDb::open(path.to_str().unwrap()).await.unwrap();

        let count: i64 = db
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM followed_wallets", [], |row| {
                    row.get(0)
                })
            })
            .await
            .unwrap();

        assert_eq!(count, 0);
    }
}
