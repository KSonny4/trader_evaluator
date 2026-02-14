use anyhow::Result;
use common::db::{AsyncDb, Database};
use rusqlite::OptionalExtension;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run,
    Markets,
    Wallets,
    Wallet {
        address: String,
    },
    Rankings,
    Classify {
        limit: Option<usize>,
    },
    PickForPaper,
    ReplayEvents {
        from: String,
        to: Option<String>,
        event_type: Option<String>,
    },
    RetryFailedEvents {
        limit: usize,
    },
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
        "classify" => parse_classify_args(args),
        "pick-for-paper" => Ok(Command::PickForPaper),
        "replay-events" => parse_replay_events_args(args),
        "retry-failed-events" => parse_retry_failed_events_args(args),
        other => Err(format!("unknown command: {other}")),
    }
}

fn parse_classify_args<I>(args: I) -> std::result::Result<Command, String>
where
    I: Iterator<Item = String>,
{
    let mut limit: Option<usize> = None;

    for arg in args {
        if let Some(val) = arg.strip_prefix("--limit=") {
            limit = Some(val.parse::<usize>().map_err(|_e| {
                format!("invalid --limit value: {val}\nusage: evaluator classify [--limit=N]")
            })?);
        } else {
            return Err(format!(
                "unknown flag for classify: {arg}\n\
                 usage: evaluator classify [--limit=N]"
            ));
        }
    }

    Ok(Command::Classify { limit })
}

fn parse_replay_events_args<I>(args: I) -> std::result::Result<Command, String>
where
    I: Iterator<Item = String>,
{
    let mut from: Option<String> = None;
    let mut to: Option<String> = None;
    let mut event_type: Option<String> = None;

    for arg in args {
        if let Some(val) = arg.strip_prefix("--from=") {
            from = Some(val.to_string());
        } else if let Some(val) = arg.strip_prefix("--to=") {
            to = Some(val.to_string());
        } else if let Some(val) = arg.strip_prefix("--type=") {
            event_type = Some(val.to_string());
        } else {
            return Err(format!(
                "unknown flag for replay-events: {arg}\n\
                 usage: evaluator replay-events --from=YYYY-MM-DD [--to=YYYY-MM-DD] [--type=pipeline|operational]"
            ));
        }
    }

    let from = from.ok_or_else(|| {
        "replay-events requires --from=YYYY-MM-DD\n\
         usage: evaluator replay-events --from=YYYY-MM-DD [--to=YYYY-MM-DD] [--type=pipeline|operational]"
            .to_string()
    })?;

    Ok(Command::ReplayEvents {
        from,
        to,
        event_type,
    })
}

fn parse_retry_failed_events_args<I>(args: I) -> std::result::Result<Command, String>
where
    I: Iterator<Item = String>,
{
    let mut limit: usize = 10;

    for arg in args {
        if let Some(val) = arg.strip_prefix("--limit=") {
            limit = val.parse::<usize>().map_err(|_e| {
                format!("invalid --limit value: {val}\nusage: evaluator retry-failed-events [--limit=N]")
            })?;
        } else {
            return Err(format!(
                "unknown flag for retry-failed-events: {arg}\n\
                 usage: evaluator retry-failed-events [--limit=N]"
            ));
        }
    }

    Ok(Command::RetryFailedEvents { limit })
}

pub fn run_command(db: &Database, cmd: Command) -> Result<()> {
    match cmd {
        Command::Run => Ok(()),
        Command::Markets => show_markets(db),
        Command::Wallets => show_wallets(db),
        Command::Wallet { address } => show_wallet(db, &address),
        Command::Rankings => show_rankings(db),
        Command::Classify { limit } => run_classify(db, limit),
        Command::PickForPaper => show_pick_for_paper(db),
        Command::ReplayEvents {
            from,
            to,
            event_type,
        } => run_replay_events(db, &from, to.as_deref(), event_type.as_deref()),
        Command::RetryFailedEvents { limit } => run_retry_failed_events(db, limit),
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

fn run_classify(_db: &Database, limit: Option<usize>) -> Result<()> {
    let config = common::config::Config::load()?;
    let db_path = config.database.path.clone();

    // Run in dedicated thread to avoid "runtime within runtime" when called from tokio::main
    let db_path_inner = db_path.clone();
    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let async_db = AsyncDb::open(&db_path_inner).await?;
            let classified =
                crate::jobs::run_persona_classification_once(&async_db, &config, None, limit).await?;
            let limit_msg = limit.map(|l| format!(" (limited to {l})")).unwrap_or_default();
            println!("Classified {classified} wallets{limit_msg} (followable or excluded)");
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

fn run_replay_events(
    db: &Database,
    from: &str,
    to: Option<&str>,
    event_type: Option<&str>,
) -> Result<()> {
    let bus = crate::event_bus::EventBus::new(1024);
    let _pipeline_rx = bus.subscribe_pipeline();
    let _operational_rx = bus.subscribe_operational();

    println!(
        "Replaying events from={from} to={} type={:?}",
        to.unwrap_or(from),
        event_type
    );

    let (replayed, skipped) = crate::events::replay::replay_events(db, &bus, from, to, event_type)?;

    println!("Replay complete: {replayed} replayed, {skipped} skipped");
    Ok(())
}

fn run_retry_failed_events(_db: &Database, limit: usize) -> Result<()> {
    let config = common::config::Config::load()?;
    let db_path = config.database.path.clone();

    // Use a dedicated thread to run async DLQ operations (same pattern as run_classify)
    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let async_db = AsyncDb::open(&db_path).await?;

            // Show current counts
            let counts = crate::events::dlq::failed_event_counts(&async_db).await?;
            println!("Failed event counts:");
            if counts.is_empty() {
                println!("  (no failed events)");
                return Ok::<_, anyhow::Error>(());
            }
            for (status, count) in &counts {
                println!("  {status}: {count}");
            }

            // Get pending events
            let pending = crate::events::dlq::get_pending_failed_events(&async_db, limit).await?;
            if pending.is_empty() {
                println!("\nNo pending events to retry.");
                return Ok(());
            }

            println!("\nRetrying {} failed events:", pending.len());

            // Re-publish each event to the event bus
            let bus = crate::event_bus::EventBus::new(1024);
            let _pipeline_rx = bus.subscribe_pipeline();
            let _operational_rx = bus.subscribe_operational();

            let mut retried = 0;
            let mut failed = 0;
            for event in &pending {
                let ok: bool = match event.event_type.as_str() {
                    "pipeline" => {
                        match serde_json::from_str::<crate::events::PipelineEvent>(
                            &event.event_data,
                        ) {
                            Ok(pe) => bus.publish_pipeline(pe).is_ok(),
                            Err(e) => {
                                println!("  [SKIP] id={} cannot parse: {e}", event.id);
                                failed += 1;
                                continue;
                            }
                        }
                    }
                    "operational" => {
                        match serde_json::from_str::<crate::events::OperationalEvent>(
                            &event.event_data,
                        ) {
                            Ok(oe) => bus.publish_operational(oe).is_ok(),
                            Err(e) => {
                                println!("  [SKIP] id={} cannot parse: {e}", event.id);
                                failed += 1;
                                continue;
                            }
                        }
                    }
                    other => {
                        println!("  [SKIP] id={} unknown type: {other}", event.id);
                        failed += 1;
                        continue;
                    }
                };

                if ok {
                    crate::events::dlq::mark_event_retried(&async_db, event.id).await?;
                    println!("  [OK] id={} type={}", event.id, event.event_type);
                    retried += 1;
                } else {
                    println!("  [FAIL] id={} no subscribers", event.id);
                    failed += 1;
                }
            }

            println!("\nRetry complete: {retried} retried, {failed} failed");
            Ok(())
        })
    });
    #[allow(clippy::map_err_ignore)]
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("retry-failed-events thread panicked"))??;
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

    #[test]
    fn test_parse_replay_events_with_from_only() {
        let cmd = parse_args(
            vec![
                "evaluator".to_string(),
                "replay-events".to_string(),
                "--from=2026-02-10".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(
            cmd,
            Command::ReplayEvents {
                from: "2026-02-10".to_string(),
                to: None,
                event_type: None,
            }
        );
    }

    #[test]
    fn test_parse_replay_events_with_all_flags() {
        let cmd = parse_args(
            vec![
                "evaluator".to_string(),
                "replay-events".to_string(),
                "--from=2026-02-10".to_string(),
                "--to=2026-02-12".to_string(),
                "--type=pipeline".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(
            cmd,
            Command::ReplayEvents {
                from: "2026-02-10".to_string(),
                to: Some("2026-02-12".to_string()),
                event_type: Some("pipeline".to_string()),
            }
        );
    }

    #[test]
    fn test_parse_replay_events_missing_from_returns_error() {
        let result =
            parse_args(vec!["evaluator".to_string(), "replay-events".to_string()].into_iter());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("--from"));
    }

    #[test]
    fn test_parse_replay_events_unknown_flag_returns_error() {
        let result = parse_args(
            vec![
                "evaluator".to_string(),
                "replay-events".to_string(),
                "--from=2026-02-10".to_string(),
                "--unknown=value".to_string(),
            ]
            .into_iter(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown flag"));
    }

    #[test]
    fn test_parse_retry_failed_events_default_limit() {
        let cmd = parse_args(
            vec!["evaluator".to_string(), "retry-failed-events".to_string()].into_iter(),
        )
        .unwrap();
        assert_eq!(cmd, Command::RetryFailedEvents { limit: 10 });
    }

    #[test]
    fn test_parse_retry_failed_events_with_limit() {
        let cmd = parse_args(
            vec![
                "evaluator".to_string(),
                "retry-failed-events".to_string(),
                "--limit=25".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(cmd, Command::RetryFailedEvents { limit: 25 });
    }

    #[test]
    fn test_parse_retry_failed_events_invalid_limit() {
        let result = parse_args(
            vec![
                "evaluator".to_string(),
                "retry-failed-events".to_string(),
                "--limit=abc".to_string(),
            ]
            .into_iter(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid --limit"));
    }

    #[test]
    fn test_parse_retry_failed_events_unknown_flag() {
        let result = parse_args(
            vec![
                "evaluator".to_string(),
                "retry-failed-events".to_string(),
                "--unknown=value".to_string(),
            ]
            .into_iter(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown flag"));
    }
}
