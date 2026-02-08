use anyhow::Result;
use common::db::Database;
use rusqlite::OptionalExtension;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run,
    Markets,
    Wallets,
    Wallet { address: String },
    PaperPnl,
    Rankings,
}

pub fn parse_args<I>(mut args: I) -> std::result::Result<Command, String>
where
    I: Iterator<Item = String>,
{
    // Drop argv[0].
    let _ = args.next();

    let Some(cmd) = args.next() else {
        return Ok(Command::Run);
    };

    match cmd.as_str() {
        "run" => Ok(Command::Run),
        "markets" => Ok(Command::Markets),
        "wallets" => Ok(Command::Wallets),
        "wallet" => {
            let address = args
                .next()
                .ok_or_else(|| "usage: evaluator wallet <address>".to_string())?;
            Ok(Command::Wallet { address })
        }
        "paper-pnl" => Ok(Command::PaperPnl),
        "rankings" => Ok(Command::Rankings),
        other => Err(format!("unknown command: {other}")),
    }
}

pub fn run_command(db: &Database, cmd: Command) -> Result<()> {
    match cmd {
        Command::Run => Ok(()),
        Command::Markets => show_markets(db),
        Command::Wallets => show_wallets(db),
        Command::Wallet { address } => show_wallet(db, &address),
        Command::PaperPnl => show_paper_pnl(db),
        Command::Rankings => show_rankings(db),
    }
}

fn show_markets(db: &Database) -> Result<()> {
    let mut stmt = db.conn.prepare(
        r#"
        SELECT condition_id, score_date, mscore, rank
        FROM market_scores_daily
        WHERE score_date = date('now')
        ORDER BY rank ASC
        LIMIT 20
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })?;

    println!("Top markets (today):");
    for r in rows {
        let (condition_id, score_date, mscore, rank) = r?;
        println!(
            "{rank:>3?}  {mscore:>6.3}  {score_date}  {condition_id}",
            rank = rank
        );
    }
    Ok(())
}

fn show_wallets(db: &Database) -> Result<()> {
    let mut stmt = db.conn.prepare(
        r#"
        SELECT proxy_wallet, discovered_from, is_active, discovered_at
        FROM wallets
        ORDER BY discovered_at DESC
        LIMIT 200
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    println!("Wallet watchlist:");
    for r in rows {
        let (w, src, active, at) = r?;
        println!("{w}  src={src}  active={active}  discovered_at={at}");
    }
    Ok(())
}

fn show_wallet(db: &Database, address: &str) -> Result<()> {
    println!("Wallet: {address}");

    let wallet_row: Option<(String, i64)> = db
        .conn
        .query_row(
            "SELECT discovered_from, is_active FROM wallets WHERE proxy_wallet = ?1",
            rusqlite::params![address],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((src, active)) = wallet_row {
        println!("  discovered_from={src}  is_active={active}");
    } else {
        println!("  (not in wallets table)");
    }

    let trades: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM trades_raw WHERE proxy_wallet = ?1",
        rusqlite::params![address],
        |row| row.get(0),
    )?;
    println!("  trades_raw rows={trades}");

    let pnl: Option<f64> = db.conn.query_row(
        "SELECT SUM(pnl) FROM paper_trades WHERE proxy_wallet = ?1 AND status != 'open'",
        rusqlite::params![address],
        |row| row.get(0),
    )?;
    println!("  paper_pnl_usdc={}", pnl.unwrap_or(0.0));

    Ok(())
}

fn show_paper_pnl(db: &Database) -> Result<()> {
    let pnl: Option<f64> = db.conn.query_row(
        "SELECT SUM(pnl) FROM paper_trades WHERE status != 'open'",
        [],
        |row| row.get(0),
    )?;
    println!("Paper PnL (settled): {}", pnl.unwrap_or(0.0));
    Ok(())
}

fn show_rankings(db: &Database) -> Result<()> {
    let mut stmt = db.conn.prepare(
        r#"
        SELECT proxy_wallet, window_days, wscore, recommended_follow_mode
        FROM wallet_scores_daily
        WHERE score_date = date('now') AND window_days = 30
        ORDER BY wscore DESC
        LIMIT 20
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;

    println!("Wallet rankings (30d):");
    for r in rows {
        let (w, window_days, wscore, mode) = r?;
        println!(
            "{wscore:>6.3}  window={window_days}  mode={mode:?}  {w}",
            mode = mode
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_defaults_to_run() {
        let cmd = parse_args(vec!["evaluator".to_string()].into_iter()).unwrap();
        assert_eq!(cmd, Command::Run);
    }

    #[test]
    fn test_parse_wallet_command() {
        let cmd = parse_args(
            vec![
                "evaluator".to_string(),
                "wallet".to_string(),
                "0xabc".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(
            cmd,
            Command::Wallet {
                address: "0xabc".to_string()
            }
        );
    }
}
