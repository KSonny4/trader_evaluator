use anyhow::Result;
use common::db::Database;
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

#[allow(dead_code)]
fn market_exposure_usdc(db: &Database, condition_id: &str) -> Result<f64> {
    let v: Option<f64> = db.conn.query_row(
        "SELECT SUM(total_size_usdc) FROM paper_positions WHERE condition_id = ?1",
        rusqlite::params![condition_id],
        |row| row.get(0),
    )?;
    Ok(v.unwrap_or(0.0))
}

#[allow(dead_code)]
fn wallet_exposure_usdc(db: &Database, proxy_wallet: &str, strategy: &str) -> Result<f64> {
    let v: Option<f64> = db.conn.query_row(
        "SELECT SUM(total_size_usdc) FROM paper_positions WHERE proxy_wallet = ?1 AND strategy = ?2",
        rusqlite::params![proxy_wallet, strategy],
        |row| row.get(0),
    )?;
    Ok(v.unwrap_or(0.0))
}

#[allow(dead_code)]
fn realized_pnl_usdc(db: &Database, strategy: &str) -> Result<f64> {
    let v: Option<f64> = db.conn.query_row(
        "SELECT SUM(pnl) FROM paper_trades WHERE strategy = ?1 AND status != 'open'",
        rusqlite::params![strategy],
        |row| row.get(0),
    )?;
    Ok(v.unwrap_or(0.0))
}

#[allow(dead_code)]
fn today_trade_count(db: &Database, strategy: &str) -> Result<u32> {
    let v: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM paper_trades WHERE strategy = ?1 AND date(created_at) = date('now')",
        rusqlite::params![strategy],
        |row| row.get(0),
    )?;
    Ok(v as u32)
}

#[allow(dead_code)]
fn upsert_position(
    db: &Database,
    proxy_wallet: &str,
    strategy: &str,
    condition_id: &str,
    side: Side,
    add_size_usdc: f64,
    entry_price: f64,
) -> Result<()> {
    let existing: Option<(i64, f64, f64)> = db
        .conn
        .query_row(
            r#"
            SELECT id, total_size_usdc, avg_entry_price
            FROM paper_positions
            WHERE proxy_wallet = ?1 AND strategy = ?2 AND condition_id = ?3 AND side = ?4
            "#,
            rusqlite::params![proxy_wallet, strategy, condition_id, side.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    match existing {
        Some((id, total_size, avg_entry)) => {
            let new_total = total_size + add_size_usdc;
            let new_avg = if new_total > 0.0 {
                (total_size * avg_entry + add_size_usdc * entry_price) / new_total
            } else {
                entry_price
            };
            db.conn.execute(
                "UPDATE paper_positions SET total_size_usdc = ?1, avg_entry_price = ?2, last_updated_at = datetime('now') WHERE id = ?3",
                rusqlite::params![new_total, new_avg, id],
            )?;
        }
        None => {
            db.conn.execute(
                r#"
                INSERT INTO paper_positions
                    (proxy_wallet, strategy, condition_id, side, total_size_usdc, avg_entry_price)
                VALUES
                    (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                rusqlite::params![
                    proxy_wallet,
                    strategy,
                    condition_id,
                    side.as_str(),
                    add_size_usdc,
                    entry_price
                ],
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub fn mirror_trade_to_paper(
    db: &Database,
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
    let strategy = "mirror";

    // Portfolio stop: halt if realized drawdown exceeds threshold.
    let realized = realized_pnl_usdc(db, strategy)?;
    let stop_usdc = bankroll_usdc * (portfolio_stop_drawdown_pct / 100.0);
    if realized < 0.0 && realized.abs() > stop_usdc {
        return Ok(MirrorDecision {
            inserted: false,
            reason: Some("portfolio_stop".to_string()),
        });
    }

    // Daily trade cap.
    if today_trade_count(db, strategy)? >= max_daily_trades {
        return Ok(MirrorDecision {
            inserted: false,
            reason: Some("max_daily_trades".to_string()),
        });
    }

    // Exposure caps.
    let market_cap = bankroll_usdc * (max_exposure_per_market_pct / 100.0);
    let wallet_cap = bankroll_usdc * (max_exposure_per_wallet_pct / 100.0);

    let market_exposure = market_exposure_usdc(db, condition_id)?;
    if market_exposure + position_size_usdc > market_cap {
        return Ok(MirrorDecision {
            inserted: false,
            reason: Some("market_exposure_cap".to_string()),
        });
    }

    let wallet_exposure = wallet_exposure_usdc(db, proxy_wallet, strategy)?;
    if wallet_exposure + position_size_usdc > wallet_cap {
        return Ok(MirrorDecision {
            inserted: false,
            reason: Some("wallet_exposure_cap".to_string()),
        });
    }

    let (entry_price, slippage_applied) = apply_slippage(observed_price, side, slippage_pct);

    db.conn.execute(
        r#"
        INSERT INTO paper_trades
            (proxy_wallet, strategy, condition_id, side, outcome, outcome_index, size_usdc, entry_price, slippage_applied, triggered_by_trade_id, status)
        VALUES
            (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'open')
        "#,
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

    upsert_position(
        db,
        proxy_wallet,
        strategy,
        condition_id,
        side,
        position_size_usdc,
        entry_price,
    )?;

    Ok(MirrorDecision {
        inserted: true,
        reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mirror_trade_creates_paper_trade() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        let res = mirror_trade_to_paper(
            &db,
            "0xwallet",
            "0xcond",
            Side::Buy,
            Some("YES"),
            Some(0),
            0.60,
            Some(1),
            100.0,
            1.0,
            10_000.0,
            10.0,
            5.0,
            100,
            15.0,
        )
        .unwrap();

        assert!(res.inserted);

        let cnt: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cnt, 1);

        let entry_price: f64 = db
            .conn
            .query_row("SELECT entry_price FROM paper_trades LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!((entry_price - 0.606).abs() < 1e-9); // 0.60 * (1 + 0.01)
    }

    #[test]
    fn test_risk_cap_blocks_oversized_trade() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        // Existing exposure in market is $900, cap is $1,000.
        db.conn
            .execute(
                r#"
            INSERT INTO paper_positions
                (proxy_wallet, strategy, condition_id, side, total_size_usdc, avg_entry_price)
            VALUES
                (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
                rusqlite::params!["0xwallet", "mirror", "0xcond", "BUY", 900.0, 0.50],
            )
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
            100.0,
            100,
            15.0,
        )
        .unwrap();

        assert!(!res.inserted);
        assert_eq!(res.reason.as_deref(), Some("market_exposure_cap"));

        let cnt: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM paper_trades", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cnt, 0);
    }

    #[test]
    fn test_portfolio_stop_halts_trading() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        // Realized PnL is -1600 on a 10k bankroll => 16% drawdown, threshold is 15%.
        db.conn.execute(
            r#"
            INSERT INTO paper_trades
                (proxy_wallet, strategy, condition_id, side, size_usdc, entry_price, status, pnl, created_at, settled_at)
            VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'), datetime('now'))
            "#,
            rusqlite::params![
                "0xwallet",
                "mirror",
                "0xcond",
                "BUY",
                100.0,
                0.50,
                "settled_loss",
                -1600.0
            ],
        )
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
            100.0,
            1.0,
            10_000.0,
            10.0,
            100.0,
            100,
            15.0,
        )
        .unwrap();

        assert!(!res.inserted);
        assert_eq!(res.reason.as_deref(), Some("portfolio_stop"));
    }
}
