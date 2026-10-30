use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};

use super::{
    Detection,
    ledger::{self, HORIZON, Spend},
};
use crate::config::Limits;
use crate::history::{Event, Tokens};
use crate::usage::{ProbeError, Usage};

fn database_path(path: Option<&Path>) -> PathBuf {
    path.map(Path::to_path_buf)
        .unwrap_or_else(|| ledger::default_data_home().join("opencode/opencode.db"))
}

fn open(path: Option<&Path>) -> Result<Connection, ProbeError> {
    Connection::open_with_flags(database_path(path), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| ProbeError::Parse(error.to_string()))
}

fn is_v2(db: &Connection) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'session_message')",
        [],
        |row| row.get(0),
    )
}

pub fn fetch(path: Option<&Path>, limits: &Limits) -> Result<Usage, ProbeError> {
    read_database(&open(path)?, limits, Utc::now())
}

pub fn events(path: Option<&Path>, since: DateTime<Utc>) -> Result<Vec<Event>, ProbeError> {
    read_events(&open(path)?, since)
}

type MessageRow = (i64, Option<String>, Option<String>, Option<f64>);

fn read_events(db: &Connection, since: DateTime<Utc>) -> Result<Vec<Event>, ProbeError> {
    let read = || -> rusqlite::Result<Vec<MessageRow>> {
        let sql = if is_v2(db)? {
            "SELECT time_created, json_extract(data, '$.model.id'), json_extract(data, '$.tokens'), json_extract(data, '$.cost') FROM session_message WHERE type = 'assistant' AND time_created >= ?1"
        } else {
            "SELECT time_created, json_extract(data, '$.modelID'), json_extract(data, '$.tokens'), json_extract(data, '$.cost') FROM message WHERE time_created >= ?1 AND json_extract(data, '$.role') = 'assistant'"
        };
        let mut statement = db.prepare(sql)?;
        statement
            .query_map([since.timestamp_millis()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect()
    };
    Ok(read()
        .map_err(|error| ProbeError::Parse(error.to_string()))?
        .into_iter()
        .filter_map(|(at, model, tokens, cost_usd)| {
            let tokens: serde_json::Value = serde_json::from_str(tokens.as_deref()?).ok()?;
            let count = |value: &serde_json::Value| value.as_u64().unwrap_or(0);
            Some(Event {
                at: DateTime::from_timestamp_millis(at)?,
                model: model.unwrap_or_else(|| "unknown".into()),
                tokens: Tokens {
                    input: count(&tokens["input"]),
                    output: count(&tokens["output"]) + count(&tokens["reasoning"]),
                    cache_read: count(&tokens["cache"]["read"]),
                    cache_write: count(&tokens["cache"]["write"]),
                },
                cost_usd,
            })
        })
        .collect())
}

fn read_database(
    db: &Connection,
    limits: &Limits,
    now: DateTime<Utc>,
) -> Result<Usage, ProbeError> {
    let read = || -> rusqlite::Result<Vec<(i64, f64)>> {
        let sql = if is_v2(db)? {
            "SELECT time_created, json_extract(data, '$.cost') FROM session_message WHERE type = 'assistant' AND json_extract(data, '$.model.providerID') = 'opencode-go' AND time_created >= ?1 AND json_extract(data, '$.cost') IS NOT NULL"
        } else {
            "SELECT time_created, json_extract(data, '$.cost') FROM message WHERE json_extract(data, '$.role') = 'assistant' AND json_extract(data, '$.providerID') = 'opencode-go' AND time_created >= ?1 AND json_extract(data, '$.cost') IS NOT NULL"
        };
        let mut statement = db.prepare(sql)?;
        statement
            .query_map([(now - HORIZON).timestamp_millis()], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect()
    };
    parse_rows(
        &read().map_err(|error| ProbeError::Parse(error.to_string()))?,
        limits,
        now,
    )
}

fn parse_rows(
    rows: &[(i64, f64)],
    limits: &Limits,
    now: DateTime<Utc>,
) -> Result<Usage, ProbeError> {
    let entries = rows
        .iter()
        .map(|&(at, usd)| {
            DateTime::from_timestamp_millis(at)
                .map(|at| Spend { at, usd })
                .ok_or_else(|| ProbeError::Parse("The message time is not valid.".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Usage {
        plan: Some("Go".into()),
        windows: ledger::windows(&entries, limits, now),
        notes: vec![
            "Cost is an estimate from the local OpenCode database.".into(),
            "Usage from other computers is not included.".into(),
        ],
    })
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| {
            path.join(name).is_file() || cfg!(windows) && path.join(format!("{name}.exe")).is_file()
        })
    })
}

pub fn detect(path: Option<&Path>) -> Detection {
    let exists = database_path(path).is_file();
    let authenticated =
        std::fs::read_to_string(ledger::default_data_home().join("opencode/auth.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .is_some_and(|value| {
                value
                    .as_object()
                    .is_some_and(|object| object.contains_key("opencode-go"))
            });
    if exists && authenticated {
        Detection::Ready("Local database and login found.".into())
    } else if exists || on_path("opencode") {
        Detection::Partial("Check the OpenCode Go login and database.".into())
    } else {
        Detection::Absent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::Meter;
    use chrono::Duration;

    #[test]
    fn v2_uses_go_assistant_costs_and_ignores_migrated_v1_rows() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE session_message(type TEXT, time_created INTEGER, data TEXT); CREATE TABLE message(time_created INTEGER, data TEXT);").unwrap();
        let now = Utc::now();
        let fixture = include_str!("../../tests/fixtures/opencode_session_message.json");
        let cost = serde_json::from_str::<serde_json::Value>(fixture).unwrap()["cost"]
            .as_f64()
            .unwrap();
        for (hours, kind, data) in [
            (-1, "assistant", fixture),
            (-6, "assistant", fixture),
            (-8 * 24, "assistant", fixture),
            (-31 * 24, "assistant", fixture),
            (1, "assistant", fixture),
            (-1, "user", fixture),
            (
                -1,
                "assistant",
                r#"{"model":{"providerID":"other"},"cost":99}"#,
            ),
            (
                -1,
                "assistant",
                r#"{"model":{"providerID":"opencode-go"},"cost":null}"#,
            ),
        ] {
            db.execute(
                "INSERT INTO session_message VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    kind,
                    (now + Duration::hours(hours)).timestamp_millis(),
                    data
                ],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO message VALUES (?1, ?2)",
            rusqlite::params![
                now.timestamp_millis(),
                r#"{"role":"assistant","providerID":"opencode-go","cost":99}"#
            ],
        )
        .unwrap();
        let usage = read_database(&db, &Limits::default(), now).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Go"));
        for (window, count) in usage.windows.iter().zip([1.0, 2.0, 2.0, 3.0]) {
            assert_eq!(
                window.meter,
                Meter::Spend {
                    used_usd: count * cost,
                    limit_usd: None
                }
            );
        }
    }

    #[test]
    fn v1_database_reports_go_assistant_costs() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE message(time_created INTEGER, data TEXT);")
            .unwrap();
        let now = Utc::now();
        for data in [
            r#"{"role":"assistant","providerID":"opencode-go","cost":2.5}"#,
            r#"{"role":"user","providerID":"opencode-go","cost":99}"#,
            r#"{"role":"assistant","providerID":"other","cost":99}"#,
        ] {
            db.execute(
                "INSERT INTO message VALUES (?1, ?2)",
                rusqlite::params![now.timestamp_millis(), data],
            )
            .unwrap();
        }
        let usage = read_database(&db, &Limits::opencode_go(), now).unwrap();
        assert_eq!(
            usage.windows[0].meter,
            Meter::Spend {
                used_usd: 2.5,
                limit_usd: Some(12.0)
            }
        );
    }

    #[test]
    fn events_read_tokens_from_every_provider_in_v1_and_v2() {
        let since = Utc::now() - Duration::days(1);
        let fixture = include_str!("../../tests/fixtures/opencode_session_message.json");
        let v2 = Connection::open_in_memory().unwrap();
        v2.execute_batch("CREATE TABLE session_message(type TEXT, time_created INTEGER, data TEXT); CREATE TABLE message(time_created INTEGER, data TEXT);").unwrap();
        for (kind, hours, data) in [
            ("assistant", -1, fixture),
            (
                "assistant",
                -1,
                r#"{"model":{"id":"other-model","providerID":"anthropic"},"tokens":{"input":5}}"#,
            ),
            ("assistant", -48, fixture),
            ("user", -1, fixture),
        ] {
            v2.execute(
                "INSERT INTO session_message VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    kind,
                    (Utc::now() + Duration::hours(hours)).timestamp_millis(),
                    data
                ],
            )
            .unwrap();
        }
        let events = read_events(&v2, since).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].model, "glm-5.3-flash");
        assert_eq!(
            events[0].tokens,
            Tokens {
                input: 38205,
                output: 1632 + 4130,
                cache_read: 5440,
                cache_write: 0
            }
        );
        assert_eq!(events[0].cost_usd, Some(0.00877495));
        assert_eq!(events[1].model, "other-model");
        assert_eq!(events[1].tokens.total(), 5);
        assert_eq!(events[1].cost_usd, None);

        let v1 = Connection::open_in_memory().unwrap();
        v1.execute_batch("CREATE TABLE message(time_created INTEGER, data TEXT);")
            .unwrap();
        for data in [
            r#"{"role":"assistant","modelID":"kimi","providerID":"x","cost":1.5,"tokens":{"input":1,"output":2,"reasoning":3,"cache":{"read":4,"write":5}}}"#,
            r#"{"role":"user","modelID":"kimi","tokens":{"input":99}}"#,
        ] {
            v1.execute(
                "INSERT INTO message VALUES (?1, ?2)",
                rusqlite::params![Utc::now().timestamp_millis(), data],
            )
            .unwrap();
        }
        assert_eq!(
            read_events(&v1, since).unwrap(),
            vec![Event {
                at: read_events(&v1, since).unwrap()[0].at,
                model: "kimi".into(),
                tokens: Tokens {
                    input: 1,
                    output: 5,
                    cache_read: 4,
                    cache_write: 5
                },
                cost_usd: Some(1.5),
            }]
        );
    }

    #[test]
    #[ignore]
    fn live_fetch_smoke() {
        let result = fetch(None, &Limits::opencode_go());
        println!("{result:#?}");
        assert!(result.is_ok());
    }
}
