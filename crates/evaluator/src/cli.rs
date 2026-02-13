use anyhow::Result;
use common::db::{AsyncDb, Database};
use rusqlite::OptionalExtension;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run,
    Markets,
    Wallets,
    Wallet { address: String },
    Rankings,
    Classify,
    PickForPaper,
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
        "rankings" => Ok(Command::Rankings),
        "classify" => Ok(Command::Classify),
        "pick-for-paper" => Ok(Command::PickForPaper),
        other => Err(format!("unknown command: {other}")),
    }
}

pub fn run_command(db: &Database, cmd: Command) -> Result<()> {
    match cmd {
        Command::Run => Ok(()),
        Command::Markets => show_markets(db),
        Command::Wallets => show_wallets(db),
        Command::Wallet { address } => show_wallet(db, &address),
        Command::Rankings => show_rankings(db),
        Command::Classify => run_classify(db),
        Command::PickForPaper => show_pick_for_paper(db),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarketRow {
    pub condition_id: String,
    pub score_date: String,
    pub mscore: f64,
    pub rank: Option<i64>,
}

pub fn query_markets_today(db: &Database) -> Result<Vec<MarketRow>> {
    let mut stmt = db.conn.prepare(
        "
        SELECT condition_id, score_date, mscore, rank
        FROM market_scores
        WHERE score_date = date('now')
        ORDER BY rank ASC
        LIMIT 20
        ",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(MarketRow {
            condition_id: row.get(0)?,
            score_date: row.get(1)?,
            mscore: row.get(2)?,
            rank: row.get(3)?,
        })
    })?;

    Ok(rows.filter_map(std::result::Result::ok).collect())
}

fn show_markets(db: &Database) -> Result<()> {
    println!("Top markets (today):");
    for r in query_markets_today(db)? {
        let MarketRow {
            condition_id,
            score_date,
            mscore,
            rank,
        } = r;
        println!("{rank:>3?}  {mscore:>6.3}  {score_date}  {condition_id}");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletRow {
    pub proxy_wallet: String,
    pub discovered_from: String,
    pub is_active: i64,
    pub discovered_at: String,
}

pub fn query_wallets(db: &Database) -> Result<Vec<WalletRow>> {
    let mut stmt = db.conn.prepare(
        "
        SELECT proxy_wallet, discovered_from, is_active, discovered_at
        FROM wallets
        ORDER BY discovered_at DESC
        LIMIT 200
        ",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(WalletRow {
            proxy_wallet: row.get(0)?,
            discovered_from: row.get(1)?,
            is_active: row.get(2)?,
            discovered_at: row.get(3)?,
        })
    })?;
    Ok(rows.filter_map(std::result::Result::ok).collect())
}

fn show_wallets(db: &Database) -> Result<()> {
    println!("Wallet watchlist:");
    for r in query_wallets(db)? {
        println!(
            "{}  src={}  active={}  discovered_at={}",
            r.proxy_wallet, r.discovered_from, r.is_active, r.discovered_at
        );
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

    // On-chain features (latest 30d)
    let features_row: Option<(f64, u32, u32, u32, f64)> = db
        .conn
        .query_row(
            "SELECT total_pnl, win_count, loss_count, unique_markets, max_drawdown_pct
             FROM wallet_features_daily
             WHERE proxy_wallet = ?1 AND window_days = 30
             ORDER BY feature_date DESC LIMIT 1",
            rusqlite::params![address],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    if let Some((pnl, wins, losses, markets, dd)) = features_row {
        println!("  on_chain_pnl={pnl:.2}  wins={wins}  losses={losses}  markets={markets}  drawdown={dd:.1}%");
    }

    // WScore
    let score_row: Option<(f64, i64)> = db
        .conn
        .query_row(
            "SELECT wscore, window_days FROM wallet_scores_daily
             WHERE proxy_wallet = ?1 AND window_days = 30
             ORDER BY score_date DESC LIMIT 1",
            rusqlite::params![address],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((wscore, wd)) = score_row {
        println!("  wscore={wscore:.3} ({wd}d)");
    }

    // Rules state
    let state: Option<String> = db
        .conn
        .query_row(
            "SELECT state FROM wallet_rules_state WHERE proxy_wallet = ?1",
            rusqlite::params![address],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(state) = state {
        println!("  state={state}");
    }

    // Persona
    let persona: Option<String> = db
        .conn
        .query_row(
            "SELECT persona FROM wallet_personas WHERE proxy_wallet = ?1
             ORDER BY classified_at DESC LIMIT 1",
            rusqlite::params![address],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(persona) = persona {
        println!("  persona={persona}");
    }

    Ok(())
}

fn show_pick_for_paper(db: &Database) -> Result<()> {
    let mut stmt = db.conn.prepare(
        "SELECT s.proxy_wallet, s.wscore, s.window_days,
                COALESCE(p.persona, 'unknown') AS persona
         FROM wallet_scores_daily s
         LEFT JOIN wallet_rules_state r ON r.proxy_wallet = s.proxy_wallet
         LEFT JOIN (
             SELECT proxy_wallet, persona,
                    ROW_NUMBER() OVER (PARTITION BY proxy_wallet ORDER BY classified_at DESC) AS rn
             FROM wallet_personas
         ) p ON p.proxy_wallet = s.proxy_wallet AND p.rn = 1
         WHERE s.window_days = 30
           AND s.score_date = date('now')
           AND (r.state = 'PAPER_TRADING' OR r.state IS NULL)
         ORDER BY s.wscore DESC
         LIMIT 20",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, f64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    println!("Top wallets eligible for paper trading:");
    let mut count = 0;
    for r in rows {
        let (wallet, wscore, wd, persona) = r?;
        println!("{wscore:>6.3}  {wd}d  {persona:<25}  {wallet}");
        count += 1;
    }
    if count == 0 {
        println!("  (no wallets eligible yet — run scoring first)");
    }
    Ok(())
}

fn run_classify(_db: &Database) -> Result<()> {
    let config = common::config::Config::load()?;
    let db_path = config.database.path.clone();

    // Run in dedicated thread to avoid "runtime within runtime" when called from tokio::main
    let db_path_inner = db_path.clone();
    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let async_db = AsyncDb::open(&db_path_inner).await?;
            let classified =
                crate::jobs::run_persona_classification_once(&async_db, &config).await?;
            println!("Classified {classified} wallets (followable or excluded)");
            Ok::<_, anyhow::Error>(())
        })
    });
    #[allow(clippy::map_err_ignore)] // JoinError is opaque
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("classify thread panicked"))??;

    // Show summary
    let db = Database::open(&db_path)?;
    let (followable, exclusions): (i64, i64) = db.conn.query_row(
        "SELECT
            (SELECT COUNT(DISTINCT proxy_wallet) FROM wallet_personas),
            (SELECT COUNT(DISTINCT proxy_wallet) FROM wallet_exclusions)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    println!("  → Followable: {followable} unique wallets");
    println!("  → Excluded: {exclusions} unique wallets");
    Ok(())
}

fn show_rankings(db: &Database) -> Result<()> {
    let mut stmt = db.conn.prepare(
        "
        SELECT proxy_wallet, window_days, wscore, recommended_follow_mode
        FROM wallet_scores_daily
        WHERE score_date = date('now') AND window_days = 30
        ORDER BY wscore DESC
        LIMIT 20
        ",
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
        println!("{wscore:>6.3}  window={window_days}  mode={mode:?}  {w}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_markets_today_returns_rows() {
        let db = common::db::Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn.execute(
            "INSERT INTO market_scores (condition_id, score_date, mscore, rank) VALUES ('0x1', date('now'), 0.9, 1)",
            [],
        ).unwrap();

        let rows = query_markets_today(&db).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].condition_id, "0x1");
    }

    #[test]
    fn test_query_wallets_returns_rows() {
        let db = common::db::Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn.execute(
            "INSERT INTO wallets (proxy_wallet, discovered_from, is_active) VALUES ('0xw', 'HOLDER', 1)",
            [],
        ).unwrap();

        let rows = query_wallets(&db).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].proxy_wallet, "0xw");
    }

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
