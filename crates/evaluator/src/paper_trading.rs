use anyhow::Result;
use common::db::AsyncDb;
use rusqlite::OptionalExtension;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[allow(dead_code)]
impl Side {
    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct MirrorDecision {
    pub inserted: bool,
    pub reason: Option<String>,
}

#[allow(dead_code)]
fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

#[allow(dead_code)]
fn apply_slippage(entry_price: f64, side: Side, slippage_pct: f64) -> (f64, f64) {
    let factor = slippage_pct / 100.0;
    let adjusted = match side {
        Side::Buy => entry_price * (1.0 + factor),
        Side::Sell => entry_price * (1.0 - factor),
    };
    (clamp01(adjusted), slippage_pct)
}

/// Quartic taker fee on Polymarket.
/// fee = price * 0.25 * (price * (1 - price))^2
/// Max ~1.56% at p=0.50, approaches zero near p=0 or p=1.
/// ONLY applies to 15-minute crypto markets. All other markets have zero fees.
pub fn quartic_taker_fee(price: f64) -> f64 {
    let p = price.clamp(0.0, 1.0);
    p * 0.25 * (p * (1.0 - p)).powi(2)
}

/// Compute the taker fee for a trade. Returns 0.0 for non-crypto markets.
pub fn compute_taker_fee(price: f64, is_crypto_15m: bool) -> f64 {
    if is_crypto_15m {
        quartic_taker_fee(price)
    } else {
        0.0
    }
}

/// Detect if a market is a 15-minute crypto price prediction market.
/// These are the ONLY markets that charge taker fees on Polymarket.
pub fn is_crypto_15m_market(title: &str, slug: &str) -> bool {
    let text = format!("{} {}", title.to_lowercase(), slug.to_lowercase());
    let is_crypto = text.contains("btc")
        || text.contains("eth")
        || text.contains("bitcoin")
        || text.contains("ethereum");
    let is_15m = text.contains("15m") || text.contains("15 min") || text.contains("15-min");
    is_crypto && is_15m
}

/// All DB reads + writes for a single mirror decision run inside one DB call
/// closure, keeping them atomic on the SQLite background thread.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub async fn mirror_trade_to_paper(
    db: &AsyncDb,
    proxy_wallet: &str,
    condition_id: &str,
    side: Side,
    outcome: Option<&str>,
    outcome_index: Option<i32>,
    observed_price: f64,
    triggered_by_trade_id: Option<i64>,
    position_size_usdc: f64,
    slippage_pct: f64,
    bankroll_usdc: f64,
    max_exposure_per_market_pct: f64,
    max_exposure_per_wallet_pct: f64,
    max_daily_trades: u32,
    portfolio_stop_drawdown_pct: f64,
) -> Result<MirrorDecision> {
    // Position size enforcement (Strategy Bible §7.3)
    // Ensure per-trade size is within sane bounds of total bankroll.
    if position_size_usdc > bankroll_usdc * 0.5 {
        return Ok(MirrorDecision {
            inserted: false,
            reason: Some("position_size_too_large".to_string()),
        });
    }

    // Clone owned values for the 'static Send closure
    let proxy_wallet = proxy_wallet.to_string();
    let condition_id = condition_id.to_string();
    let outcome = outcome.map(std::string::ToString::to_string);

    db.call_named("paper_trading.mirror_trade_to_paper", move |conn| {
        let strategy = "mirror";

        // Portfolio stop: halt if realized drawdown exceeds threshold.
        let realized: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(pnl), 0.0) FROM paper_trades WHERE strategy = ?1 AND status != 'open'",
                rusqlite::params![strategy],
                |row| row.get(0),
            )?;
        let stop_usdc = bankroll_usdc * (portfolio_stop_drawdown_pct / 100.0);
        if realized < 0.0 && realized.abs() > stop_usdc {
            return Ok(MirrorDecision {
                inserted: false,
                reason: Some("portfolio_stop".to_string()),
            });
        }

        // Daily trade cap.
        let today_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM paper_trades WHERE strategy = ?1 AND date(created_at) = date('now')",
            rusqlite::params![strategy],
            |row| row.get(0),
        )?;
        if today_count as u32 >= max_daily_trades {
            return Ok(MirrorDecision {
                inserted: false,
                reason: Some("max_daily_trades".to_string()),
            });
        }

        // Exposure caps.
        let market_cap = bankroll_usdc * (max_exposure_per_market_pct / 100.0);
        let wallet_cap = bankroll_usdc * (max_exposure_per_wallet_pct / 100.0);

        let market_exposure: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(total_size_usdc), 0.0) FROM paper_positions WHERE condition_id = ?1",
                rusqlite::params![condition_id],
                |row| row.get(0),
            )?;
        if market_exposure + position_size_usdc > market_cap {
            return Ok(MirrorDecision {
                inserted: false,
                reason: Some("market_exposure_cap".to_string()),
            });
        }

        let wallet_exposure: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(total_size_usdc), 0.0) FROM paper_positions WHERE proxy_wallet = ?1 AND strategy = ?2",
                rusqlite::params![proxy_wallet, strategy],
                |row| row.get(0),
            )?;
        if wallet_exposure + position_size_usdc > wallet_cap {
            return Ok(MirrorDecision {
                inserted: false,
                reason: Some("wallet_exposure_cap".to_string()),
            });
        }

        let (adjusted_price, slippage_applied) = apply_slippage(observed_price, side, slippage_pct);

        let is_crypto_15m: bool = conn
            .query_row(
                "SELECT COALESCE(is_crypto_15m, 0) FROM markets WHERE condition_id = ?1",
                rusqlite::params![condition_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some_and(|v| v != 0);

        let fee = compute_taker_fee(adjusted_price, is_crypto_15m);
        let entry_price = if side == Side::Buy {
            clamp01(adjusted_price + fee)
        } else {
            clamp01(adjusted_price - fee)
        };

        conn.execute(
            "
            INSERT INTO paper_trades
                (proxy_wallet, strategy, condition_id, side, outcome, outcome_index, size_usdc, entry_price, slippage_applied, triggered_by_trade_id, status)
            VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'open')
            ",
            rusqlite::params![
                proxy_wallet,
                strategy,
                condition_id,
                side.as_str(),
                outcome,
                outcome_index,
                position_size_usdc,
                entry_price,
                slippage_applied,
                triggered_by_trade_id
            ],
        )?;

        // Upsert position (inline — same transaction).
        let existing: Option<(i64, f64, f64)> = conn
            .query_row(
                "
                SELECT id, total_size_usdc, avg_entry_price
                FROM paper_positions
                WHERE proxy_wallet = ?1 AND strategy = ?2 AND condition_id = ?3 AND side = ?4
                ",
                rusqlite::params![proxy_wallet, strategy, condition_id, side.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        match existing {
            Some((id, total_size, avg_entry)) => {
                let new_total = total_size + position_size_usdc;
                let new_avg = if new_total > 0.0 {
                    (total_size * avg_entry + position_size_usdc * entry_price) / new_total
                } else {
                    entry_price
                };
                conn.execute(
                    "UPDATE paper_positions SET total_size_usdc = ?1, avg_entry_price = ?2, last_updated_at = datetime('now') WHERE id = ?3",
                    rusqlite::params![new_total, new_avg, id],
                )?;
            }
            None => {
                conn.execute(
                    "
                    INSERT INTO paper_positions
                        (proxy_wallet, strategy, condition_id, side, total_size_usdc, avg_entry_price)
                    VALUES
                        (?1, ?2, ?3, ?4, ?5, ?6)
                    ",
                    rusqlite::params![
                        proxy_wallet,
                        strategy,
                        condition_id,
                        side.as_str(),
                        position_size_usdc,
                        entry_price
                    ],
                )?;
            }
        }

        Ok(MirrorDecision {
            inserted: true,
            reason: None,
        })
    })
    .await
}

/// Settle all open paper trades for a market that has resolved.
/// `settle_price` is 1.0 (outcome won) or 0.0 (outcome lost).
/// Returns the number of trades settled.
#[allow(dead_code)] // Called from ingestion or resolution job when wired
pub fn settle_paper_trades_for_market(
    conn: &rusqlite::Connection,
    condition_id: &str,
    settle_price: f64,
) -> Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT id, entry_price, size_usdc, side FROM paper_trades
         WHERE condition_id = ?1 AND status = 'open'",
    )?;

    let trades: Vec<(i64, f64, f64, String)> = stmt
        .query_map(rusqlite::params![condition_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .filter_map(Result::ok)
        .collect();

    let mut settled = 0;

    for (id, entry_price, size_usdc, side) in &trades {
        let pnl = if side == "BUY" {
            (settle_price - entry_price) * size_usdc
        } else {
            (entry_price - settle_price) * size_usdc
        };

        let status = if pnl >= 0.0 {
            "settled_win"
        } else {
            "settled_loss"
        };

        conn.execute(
            "UPDATE paper_trades SET status = ?1, exit_price = ?2, pnl = ?3, settled_at = datetime('now')
             WHERE id = ?4",
            rusqlite::params![status, settle_price, pnl, id],
        )?;

        settled += 1;
    }

    if settled > 0 {
        conn.execute(
            "DELETE FROM paper_positions WHERE condition_id = ?1",
            [condition_id],
        )?;
    }

    Ok(settled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::db::Database;

    #[test]
    fn test_settle_paper_trades_win() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn
            .execute(
                "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, outcome, entry_price, size_usdc, status)
                 VALUES ('0xabc', 'mirror', '0xmarket1', 'BUY', 'Yes', 0.60, 25.0, 'open')",
                [],
            )
            .unwrap();

        let settled = settle_paper_trades_for_market(&db.conn, "0xmarket1", 1.0).unwrap();
        assert_eq!(settled, 1);

        let (status, pnl): (String, f64) = db
            .conn
            .query_row(
                "SELECT status, pnl FROM paper_trades WHERE condition_id = '0xmarket1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "settled_win");
        assert!((pnl - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_settle_paper_trades_loss() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn
            .execute(
                "INSERT INTO paper_trades (proxy_wallet, strategy, condition_id, side, outcome, entry_price, size_usdc, status)
                 VALUES ('0xabc', 'mirror', '0xmarket2', 'BUY', 'Yes', 0.60, 25.0, 'open')",
                [],
            )
            .unwrap();

        let settled = settle_paper_trades_for_market(&db.conn, "0xmarket2", 0.0).unwrap();
        assert_eq!(settled, 1);

        let (status, pnl): (String, f64) = db
            .conn
            .query_row(
                "SELECT status, pnl FROM paper_trades WHERE condition_id = '0xmarket2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "settled_loss");
        assert!((pnl - (-15.0)).abs() < 0.01);
    }

    #[test]
    fn test_quartic_taker_fee() {
        let fee = quartic_taker_fee(0.60);
        assert!((fee - 0.00864).abs() < 0.0001);
    }

    #[test]
    fn test_quartic_taker_fee_at_extremes() {
        let fee_low = quartic_taker_fee(0.05);
        assert!(fee_low < 0.001);
        let fee_high = quartic_taker_fee(0.95);
        assert!(fee_high < 0.001);
        let fee_mid = quartic_taker_fee(0.50);
        assert!((fee_mid - 0.0078125).abs() < 0.0001);
    }

    #[test]
    fn test_compute_fee_conditional() {
        let fee_political = compute_taker_fee(0.60, false);
        assert!((fee_political - 0.0).abs() < 0.0001);
        let fee_crypto = compute_taker_fee(0.60, true);
        assert!((fee_crypto - 0.00864).abs() < 0.0001);
    }

    #[test]
    fn test_detect_crypto_15m_market() {
        assert!(is_crypto_15m_market(
            "Will BTC go above $100,000 by 15 min?",
            "btc-15m-above-100k"
        ));
        assert!(is_crypto_15m_market(
            "Will ETH be above $4,000 at 3:15 PM?",
            "eth-15m-above-4000"
        ));
        assert!(!is_crypto_15m_market(
            "Will Trump win the 2024 election?",
            "trump-2024-election"
        ));
        assert!(!is_crypto_15m_market(
            "Will Bitcoin reach $200k by 2026?",
            "bitcoin-200k-2026"
        ));
    }

    #[tokio::test]
    async fn test_mirror_trade_creates_paper_trade() {
        let db = AsyncDb::open(":memory:").await.unwrap();

        let res = mirror_trade_to_paper(
            &db,
            "0xwallet",
            "0xcond",
            Side::Buy,
            Some("YES"),
            Some(0),
            0.60,
            Some(1),
            25.0,
            1.0,
            10_000.0,
            10.0,
            5.0,
            100,
            15.0,
        )
        .await
        .unwrap();

        assert!(res.inserted);

        let (cnt, entry_price): (i64, f64) = db
            .call(|conn| {
                let cnt =
                    conn.query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))?;
                let entry_price =
                    conn.query_row("SELECT entry_price FROM paper_trades LIMIT 1", [], |row| {
                        row.get(0)
                    })?;
                Ok((cnt, entry_price))
            })
            .await
            .unwrap();
        assert_eq!(cnt, 1);
        assert!((entry_price - 0.606).abs() < 1e-9); // 0.60 * (1 + 0.01)
    }

    #[tokio::test]
    async fn test_risk_cap_blocks_oversized_trade() {
        let db = AsyncDb::open(":memory:").await.unwrap();

        // Existing exposure in market is $900, cap is $1,000.
        db.call(|conn| {
            conn.execute(
                "
                INSERT INTO paper_positions
                    (proxy_wallet, strategy, condition_id, side, total_size_usdc, avg_entry_price)
                VALUES
                    (?1, ?2, ?3, ?4, ?5, ?6)
                ",
                rusqlite::params!["0xwallet", "mirror", "0xcond", "BUY", 900.0, 0.50],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let res = mirror_trade_to_paper(
            &db,
            "0xwallet",
            "0xcond",
            Side::Buy,
            Some("YES"),
            Some(0),
            0.60,
            Some(1),
            200.0, // would push market exposure to 1,100
            1.0,
            10_000.0,
            10.0, // cap = 1,000
            25.0,
            100,
            15.0,
        )
        .await
        .unwrap();

        assert!(!res.inserted);
        assert_eq!(res.reason.as_deref(), Some("market_exposure_cap"));

        let cnt: i64 = db
            .call(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(cnt, 0);
    }

    #[tokio::test]
    async fn test_portfolio_stop_halts_trading() {
        let db = AsyncDb::open(":memory:").await.unwrap();

        // Realized PnL is -1600 on a 10k bankroll => 16% drawdown, threshold is 15%.
        db.call(|conn| {
            conn.execute(
                "
                INSERT INTO paper_trades
                    (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl, created_at, settled_at)
                VALUES
                    (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'), datetime('now'))
                ",
                rusqlite::params![
                    "0xwallet",
                    "mirror",
                    "0xcond",
                    "BUY",
                    25.0,
                    0.50,
                    "settled_loss",
                    -1600.0
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let res = mirror_trade_to_paper(
            &db,
            "0xwallet",
            "0xcond2",
            Side::Buy,
            Some("YES"),
            Some(0),
            0.60,
            Some(2),
            25.0,
            1.0,
            10_000.0,
            10.0,
            25.0,
            100,
            15.0,
        )
        .await
        .unwrap();

        assert!(!res.inserted);
        assert_eq!(res.reason.as_deref(), Some("portfolio_stop"));
    }

    #[tokio::test]
    async fn test_position_size_enforcement_blocks_huge_trade() {
        let db = AsyncDb::open(":memory:").await.unwrap();

        let res = mirror_trade_to_paper(
            &db,
            "0xwallet",
            "0xcond",
            Side::Buy,
            Some("YES"),
            Some(0),
            0.60,
            Some(1),
            6000.0, // 60% of 10k bankroll, should be blocked by 50% limit
            1.0,
            10_000.0,
            10.0,
            5.0,
            100,
            15.0,
        )
        .await
        .unwrap();

        assert!(!res.inserted);
        assert_eq!(res.reason.as_deref(), Some("position_size_too_large"));
    }
}
