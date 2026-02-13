//! Event replay: query event_log table and republish events to EventBus.
//!
//! Used for debugging by replaying production events locally.

use crate::event_bus::EventBus;
use crate::events::{OperationalEvent, PipelineEvent};
use anyhow::Result;
use common::db::Database;

/// A row from the event_log table.
#[derive(Debug, Clone)]
pub struct EventLogRow {
    pub id: i64,
    pub event_type: String,
    pub event_data: String,
    pub emitted_at: String,
}

/// Query event_log rows filtered by date range and optional event type.
///
/// - `from`: inclusive start date (YYYY-MM-DD)
/// - `to`: optional inclusive end date (YYYY-MM-DD); defaults to `from` if None
/// - `event_type_filter`: optional filter on `event_type` column (e.g. "pipeline")
pub fn query_event_log(
    db: &Database,
    from: &str,
    to: Option<&str>,
    event_type_filter: Option<&str>,
) -> Result<Vec<EventLogRow>> {
    let to_date = to.unwrap_or(from);

    let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
        if let Some(et) = event_type_filter {
            (
                "SELECT id, event_type, event_data, emitted_at FROM event_log \
                 WHERE date(emitted_at) >= date(?1) AND date(emitted_at) <= date(?2) \
                 AND event_type = ?3 \
                 ORDER BY id ASC"
                    .to_string(),
                vec![
                    Box::new(from.to_string()),
                    Box::new(to_date.to_string()),
                    Box::new(et.to_string()),
                ],
            )
        } else {
            (
                "SELECT id, event_type, event_data, emitted_at FROM event_log \
                 WHERE date(emitted_at) >= date(?1) AND date(emitted_at) <= date(?2) \
                 ORDER BY id ASC"
                    .to_string(),
                vec![Box::new(from.to_string()), Box::new(to_date.to_string())],
            )
        };

    let mut stmt = db.conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(AsRef::as_ref).collect();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        Ok(EventLogRow {
            id: row.get(0)?,
            event_type: row.get(1)?,
            event_data: row.get(2)?,
            emitted_at: row.get(3)?,
        })
    })?;

    Ok(rows.filter_map(std::result::Result::ok).collect())
}

/// Replay events from the database to the EventBus.
///
/// Returns `(replayed, skipped)` counts.
pub fn replay_events(
    db: &Database,
    bus: &EventBus,
    from: &str,
    to: Option<&str>,
    event_type_filter: Option<&str>,
) -> Result<(usize, usize)> {
    let rows = query_event_log(db, from, to, event_type_filter)?;
    let total = rows.len();
    let mut replayed = 0;
    let mut skipped = 0;

    for (i, row) in rows.iter().enumerate() {
        match row.event_type.as_str() {
            "pipeline" => match serde_json::from_str::<PipelineEvent>(&row.event_data) {
                Ok(event) => {
                    let _ = bus.publish_pipeline(event);
                    replayed += 1;
                }
                Err(e) => {
                    tracing::warn!(id = row.id, error = %e, "skipping malformed pipeline event");
                    skipped += 1;
                }
            },
            "operational" => match serde_json::from_str::<OperationalEvent>(&row.event_data) {
                Ok(event) => {
                    let _ = bus.publish_operational(event);
                    replayed += 1;
                }
                Err(e) => {
                    tracing::warn!(id = row.id, error = %e, "skipping malformed operational event");
                    skipped += 1;
                }
            },
            other => {
                tracing::warn!(
                    id = row.id,
                    event_type = other,
                    "skipping unknown event type"
                );
                skipped += 1;
            }
        }

        if (i + 1) % 100 == 0 || i + 1 == total {
            tracing::info!(
                progress = i + 1,
                total,
                replayed,
                skipped,
                emitted_at = %row.emitted_at,
                "replay progress"
            );
        }
    }

    Ok((replayed, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_bus::EventBus;
    use crate::events::PipelineEvent;
    use chrono::Utc;

    fn setup_db_with_events() -> Database {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        // Insert pipeline events
        let event1 = PipelineEvent::MarketsScored {
            markets_scored: 100,
            events_ranked: 50,
            completed_at: Utc::now(),
        };
        let event2 = PipelineEvent::WalletsDiscovered {
            market_id: "market-1".to_string(),
            wallets_added: 10,
            discovered_at: Utc::now(),
        };

        db.conn
            .execute(
                "INSERT INTO event_log (event_type, event_data, emitted_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    "pipeline",
                    serde_json::to_string(&event1).unwrap(),
                    "2026-02-10 12:00:00"
                ],
            )
            .unwrap();

        db.conn
            .execute(
                "INSERT INTO event_log (event_type, event_data, emitted_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    "pipeline",
                    serde_json::to_string(&event2).unwrap(),
                    "2026-02-11 14:00:00"
                ],
            )
            .unwrap();

        // Insert operational event
        let op_event = crate::events::OperationalEvent::JobCompleted {
            job_name: "test_job".to_string(),
            duration_ms: 500,
            completed_at: Utc::now(),
        };
        db.conn
            .execute(
                "INSERT INTO event_log (event_type, event_data, emitted_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    "operational",
                    serde_json::to_string(&op_event).unwrap(),
                    "2026-02-10 13:00:00"
                ],
            )
            .unwrap();

        db
    }

    #[test]
    fn test_query_event_log_returns_all_events_in_range() {
        let db = setup_db_with_events();
        let rows = query_event_log(&db, "2026-02-10", Some("2026-02-11"), None).unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn test_query_event_log_filters_by_date() {
        let db = setup_db_with_events();
        let rows = query_event_log(&db, "2026-02-10", Some("2026-02-10"), None).unwrap();
        // Only events from Feb 10 (2 events)
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_query_event_log_single_date_uses_from_as_to() {
        let db = setup_db_with_events();
        let rows = query_event_log(&db, "2026-02-11", None, None).unwrap();
        // Only events from Feb 11 (1 event)
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn test_query_event_log_filters_by_type() {
        let db = setup_db_with_events();
        let rows =
            query_event_log(&db, "2026-02-10", Some("2026-02-11"), Some("pipeline")).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.event_type == "pipeline"));
    }

    #[test]
    fn test_query_event_log_returns_empty_for_no_match() {
        let db = setup_db_with_events();
        let rows = query_event_log(&db, "2020-01-01", Some("2020-01-01"), None).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn test_query_event_log_ordered_by_id() {
        let db = setup_db_with_events();
        let rows = query_event_log(&db, "2026-02-10", Some("2026-02-11"), None).unwrap();
        for w in rows.windows(2) {
            assert!(w[0].id < w[1].id);
        }
    }

    #[test]
    fn test_replay_events_publishes_pipeline_events() {
        let db = setup_db_with_events();
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe_pipeline();

        let (replayed, skipped) = replay_events(
            &db,
            &bus,
            "2026-02-10",
            Some("2026-02-10"),
            Some("pipeline"),
        )
        .unwrap();
        assert_eq!(replayed, 1);
        assert_eq!(skipped, 0);

        // Verify event was published
        let event = rx.try_recv().unwrap();
        match event {
            PipelineEvent::MarketsScored { markets_scored, .. } => assert_eq!(markets_scored, 100),
            _ => panic!("expected MarketsScored"),
        }
    }

    #[test]
    fn test_replay_events_publishes_operational_events() {
        let db = setup_db_with_events();
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe_operational();

        let (replayed, skipped) = replay_events(
            &db,
            &bus,
            "2026-02-10",
            Some("2026-02-10"),
            Some("operational"),
        )
        .unwrap();
        assert_eq!(replayed, 1);
        assert_eq!(skipped, 0);

        let event = rx.try_recv().unwrap();
        match event {
            crate::events::OperationalEvent::JobCompleted { job_name, .. } => {
                assert_eq!(job_name, "test_job");
            }
            _ => panic!("expected JobCompleted"),
        }
    }

    #[test]
    fn test_replay_events_skips_malformed_data() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn
            .execute(
                "INSERT INTO event_log (event_type, event_data, emitted_at) VALUES (?1, ?2, ?3)",
                rusqlite::params!["pipeline", "not valid json", "2026-02-10 12:00:00"],
            )
            .unwrap();

        let bus = EventBus::new(16);
        let _rx = bus.subscribe_pipeline();

        let (replayed, skipped) = replay_events(&db, &bus, "2026-02-10", None, None).unwrap();
        assert_eq!(replayed, 0);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn test_replay_events_skips_unknown_event_type() {
        let db = Database::open(":memory:").unwrap();
        db.run_migrations().unwrap();

        db.conn
            .execute(
                "INSERT INTO event_log (event_type, event_data, emitted_at) VALUES (?1, ?2, ?3)",
                rusqlite::params!["unknown_type", "{}", "2026-02-10 12:00:00"],
            )
            .unwrap();

        let bus = EventBus::new(16);
        let _rx = bus.subscribe_pipeline();

        let (replayed, skipped) = replay_events(&db, &bus, "2026-02-10", None, None).unwrap();
        assert_eq!(replayed, 0);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn test_replay_events_mixed_types() {
        let db = setup_db_with_events();
        let bus = EventBus::new(16);
        let _p_rx = bus.subscribe_pipeline();
        let _o_rx = bus.subscribe_operational();

        let (replayed, skipped) =
            replay_events(&db, &bus, "2026-02-10", Some("2026-02-11"), None).unwrap();
        assert_eq!(replayed, 3);
        assert_eq!(skipped, 0);
    }
}
