use super::check::compute_fill_probability;
use crate::db::TraderDb;
use tracing::{debug, error, info};

pub async fn settle_recording(
    db: &TraderDb,
    condition_id: &str,
    token_id: &str,
    trade_hashes: &[String],
    recording_started_at: &str,
) {
    let tid = token_id.to_string();
    let started = recording_started_at.to_string();

    let snapshots: Vec<(bool, f64, f64, f64, String)> = match db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT fillable, available_depth_usd, vwap, slippage_cents, snapshot_at
                 FROM book_snapshots WHERE token_id = ?1 AND snapshot_at >= ?2
                 ORDER BY snapshot_at ASC",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![tid, started], |row| {
                    Ok((
                        row.get::<_, i32>(0)? != 0,
                        row.get::<_, f64>(1).unwrap_or(0.0),
                        row.get::<_, f64>(2).unwrap_or(0.0),
                        row.get::<_, f64>(3).unwrap_or(0.0),
                        row.get::<_, String>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    {
        Ok(s) => s,
        Err(e) => {
            error!(token_id = token_id, error = %e, "failed to read snapshots for settlement");
            return;
        }
    };

    if snapshots.is_empty() {
        debug!(token_id = token_id, "no snapshots to settle");
        return;
    }

    let window_start = snapshots.first().map(|s| s.4.clone()).unwrap_or_default();
    let window_end = snapshots.last().map(|s| s.4.clone()).unwrap_or_default();
    // Compute actual elapsed time from timestamps instead of assuming 120s
    let elapsed_secs = chrono::DateTime::parse_from_rfc3339(&window_start)
        .ok()
        .zip(chrono::DateTime::parse_from_rfc3339(&window_end).ok())
        .map_or(120.0, |(start, end)| {
            (end - start).num_seconds().max(1) as f64
        });
    let snapshot_interval = if snapshots.len() > 1 {
        elapsed_secs / snapshots.len() as f64
    } else {
        elapsed_secs
    };

    let data: Vec<(bool, f64, f64, f64)> = snapshots
        .iter()
        .map(|(f, d, v, s, _)| (*f, *d, *v, *s))
        .collect();

    let result = compute_fill_probability(&data, &window_start, snapshot_interval);

    let cid = condition_id.to_string();
    let tid = token_id.to_string();
    let hashes_json = serde_json::to_string(trade_hashes).unwrap_or_else(|_| "[]".to_string());

    if let Err(e) = db
        .call(move |conn| {
            conn.execute(
                "INSERT INTO fillability_results
                 (condition_id, token_id, trigger_trade_hashes, snapshot_count, fillable_count,
                  fill_probability, opportunity_window_secs, avg_available_depth_usd,
                  avg_vwap, avg_slippage_cents, window_start, window_end)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![
                    cid,
                    tid,
                    hashes_json,
                    result.snapshot_count,
                    result.fillable_count,
                    result.fill_probability,
                    result.opportunity_window_secs,
                    result.avg_available_depth_usd,
                    result.avg_vwap,
                    result.avg_slippage_cents,
                    result.window_start,
                    window_end,
                ],
            )?;
            Ok(())
        })
        .await
    {
        error!(token_id = token_id, error = %e, "failed to write fillability result");
    } else {
        info!(
            token_id = token_id,
            fill_probability = result.fill_probability,
            snapshots = result.snapshot_count,
            "fillability settled"
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_book_snapshot(
    db: &TraderDb,
    condition_id: &str,
    token_id: &str,
    best_bid: Option<f64>,
    best_ask: Option<f64>,
    bid_depth_usd: f64,
    ask_depth_usd: f64,
    spread_cents: Option<f64>,
    mid_price: Option<f64>,
    fillable: bool,
    available_depth_usd: f64,
    vwap: f64,
    slippage_cents: f64,
    levels_json: &str,
) {
    let cid = condition_id.to_string();
    let tid = token_id.to_string();
    let lj = levels_json.to_string();
    let now = chrono::Utc::now().to_rfc3339();

    if let Err(e) = db
        .call(move |conn| {
            conn.execute(
                "INSERT INTO book_snapshots
                 (condition_id, token_id, best_bid, best_ask, bid_depth_usd, ask_depth_usd,
                  spread_cents, mid_price, fillable, available_depth_usd, vwap, slippage_cents,
                  levels_json, snapshot_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                rusqlite::params![
                    cid,
                    tid,
                    best_bid,
                    best_ask,
                    bid_depth_usd,
                    ask_depth_usd,
                    spread_cents,
                    mid_price,
                    i32::from(fillable),
                    available_depth_usd,
                    vwap,
                    slippage_cents,
                    lj,
                    now,
                ],
            )?;
            Ok(())
        })
        .await
    {
        error!(token_id = token_id, error = %e, "failed to insert book snapshot");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_settle_recording_writes_result() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        db.call(|conn| {
            for (fillable, depth, vwap, slip, ts) in [
                (1, 100.0, 0.50, 0.5, "2026-01-01T00:00:01Z"),
                (1, 110.0, 0.51, 0.4, "2026-01-01T00:00:02Z"),
                (0, 5.0, 0.0, 0.0, "2026-01-01T00:00:03Z"),
            ] {
                conn.execute(
                    "INSERT INTO book_snapshots (condition_id, token_id, fillable,
                     available_depth_usd, vwap, slippage_cents, snapshot_at)
                     VALUES ('cond-1', 'tok-1', ?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![fillable, depth, vwap, slip, ts],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        settle_recording(
            &db,
            "cond-1",
            "tok-1",
            &["hash-1".to_string()],
            "2026-01-01T00:00:00Z",
        )
        .await;

        let (prob, count, fillable): (f64, i32, i32) = db
            .call(|conn| {
                conn.query_row(
                    "SELECT fill_probability, snapshot_count, fillable_count
                     FROM fillability_results WHERE token_id = 'tok-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .await
            .unwrap();

        let expected_prob = 2.0 / 3.0;
        assert!((prob - expected_prob).abs() < 0.01);
        assert_eq!(count, 3);
        assert_eq!(fillable, 2);
    }

    #[tokio::test]
    async fn test_insert_book_snapshot_round_trip() {
        let db = Arc::new(TraderDb::open_memory().await.unwrap());

        insert_book_snapshot(
            &db,
            "cond-1",
            "tok-1",
            Some(0.49),
            Some(0.51),
            100.0,
            200.0,
            Some(2.0),
            Some(0.50),
            true,
            150.0,
            0.505,
            0.5,
            r#"{"bids":[],"asks":[]}"#,
        )
        .await;

        let (cid, tid, fillable): (String, String, i32) = db
            .call(|conn| {
                conn.query_row(
                    "SELECT condition_id, token_id, fillable FROM book_snapshots LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .await
            .unwrap();

        assert_eq!(cid, "cond-1");
        assert_eq!(tid, "tok-1");
        assert_eq!(fillable, 1);
    }
}
