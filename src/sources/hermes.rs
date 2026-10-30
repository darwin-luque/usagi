use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};

use super::{
    Detection,
    ledger::{self, Spend},
};
use crate::config::Limits;
use crate::history::{Event, Tokens};
use crate::usage::{ProbeError, Usage};

fn database_path(path: Option<&Path>) -> PathBuf {
    path.map(Path::to_path_buf).unwrap_or_else(|| {
        std::env::var_os("HERMES_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::config::home().join(".hermes"))
            .join("state.db")
    })
}

fn open(path: Option<&Path>) -> Result<Connection, ProbeError> {
    Connection::open_with_flags(database_path(path), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| ProbeError::Parse(error.to_string()))
}

struct Columns(Vec<String>);

impl Columns {
    fn read(db: &Connection) -> rusqlite::Result<Columns> {
        let mut statement = db.prepare("PRAGMA table_info(sessions)")?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Columns(names))
    }

    fn first(&self, names: &[&str], fallback: &str) -> String {
        let present: Vec<_> = names
            .iter()
            .copied()
            .filter(|name| self.0.iter().any(|column| column == name))
            .collect();
        match present.as_slice() {
            [] => fallback.into(),
            [name] => (*name).to_string(),
            _ => format!("COALESCE({})", present.join(", ")),
        }
    }

    fn time(&self) -> String {
        self.first(&["last_activity_at", "ended_at", "started_at"], "NULL")
    }

    fn cost(&self) -> String {
        self.first(&["actual_cost_usd", "estimated_cost_usd"], "NULL")
    }
}

fn session_time(at: f64) -> Option<DateTime<Utc>> {
    let milliseconds = if at > 1e12 { at } else { at * 1000.0 };
    if !milliseconds.is_finite()
        || milliseconds < i64::MIN as f64
        || milliseconds >= i64::MAX as f64
    {
        return None;
    }
    DateTime::from_timestamp_millis(milliseconds as i64)
}

pub fn fetch(path: Option<&Path>, limits: &Limits) -> Result<Usage, ProbeError> {
    read_database(&open(path)?, limits, Utc::now())
}

pub fn events(path: Option<&Path>, since: DateTime<Utc>) -> Result<Vec<Event>, ProbeError> {
    read_events(&open(path)?, since)
}

type SessionRow = (Option<f64>, Option<String>, [Option<i64>; 5], Option<f64>);

fn read_events(db: &Connection, since: DateTime<Utc>) -> Result<Vec<Event>, ProbeError> {
    let read = || -> rusqlite::Result<Vec<SessionRow>> {
        let columns = Columns::read(db)?;
        let count = |name: &str| format!("CAST({} AS INTEGER)", columns.first(&[name], "0"));
        let sql = format!(
            "SELECT {}, {}, {}, {}, {}, {}, {}, {} FROM sessions",
            columns.time(),
            columns.first(&["model"], "NULL"),
            count("input_tokens"),
            count("output_tokens"),
            count("reasoning_tokens"),
            count("cache_read_tokens"),
            count("cache_write_tokens"),
            columns.cost(),
        );
        let mut statement = db.prepare(&sql)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    [
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ],
                    row.get(7)?,
                ))
            })?
            .collect()
    };
    Ok(read()
        .map_err(|error| ProbeError::Parse(error.to_string()))?
        .into_iter()
        .filter_map(|(at, model, counts, cost_usd)| {
            let [input, output, reasoning, cache_read, cache_write] =
                counts.map(|count| count.unwrap_or(0).max(0) as u64);
            Some(Event {
                at: session_time(at?).filter(|at| *at >= since)?,
                model: model.unwrap_or_else(|| "unknown".into()),
                tokens: Tokens {
                    input,
                    output: output + reasoning,
                    cache_read,
                    cache_write,
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
    let read = || -> rusqlite::Result<Vec<(Option<f64>, f64)>> {
        let columns = Columns::read(db)?;
        let at = columns.time();
        let cost = columns.cost();
        let mut statement = db.prepare(&format!(
            "SELECT {at}, {cost} FROM sessions WHERE {cost} IS NOT NULL"
        ))?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    };
    parse_rows(
        &read().map_err(|error| ProbeError::Parse(error.to_string()))?,
        limits,
        now,
    )
}

fn parse_rows(
    rows: &[(Option<f64>, f64)],
    limits: &Limits,
    now: DateTime<Utc>,
) -> Result<Usage, ProbeError> {
    let entries = rows
        .iter()
        .map(|&(at, usd)| {
            let at = at.ok_or_else(|| ProbeError::Parse("The session has no time.".into()))?;
            session_time(at)
                .map(|at| Spend { at, usd })
                .ok_or_else(|| ProbeError::Parse("The session time is not valid.".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Usage {
        plan: None,
        windows: ledger::windows(&entries, limits, now),
        notes: vec![
            "Cost is an estimate from the local Hermes database.".into(),
            "Hermes records cost per session. A session counts at its last activity time.".into(),
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
    if database_path(path).is_file() {
        Detection::Ready("Local database found.".into())
    } else if on_path("hermes") || crate::config::home().join(".hermes").is_dir() {
        Detection::Partial("Check the Hermes database.".into())
    } else {
        Detection::Absent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::Meter;
    use chrono::Duration;

    fn check_database(actual_column: bool) {
        let path = std::env::temp_dir().join(format!(
            "usagi-hermes-{}-{}-{}.db",
            actual_column,
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let db = Connection::open(&path).unwrap();
        db.execute_batch(&format!("CREATE TABLE sessions(started_at REAL, ended_at REAL, last_activity_at REAL, estimated_cost_usd REAL{});", if actual_column { ", actual_cost_usd REAL" } else { "" })).unwrap();
        let now = Utc::now();
        for (hours, cost) in [
            (-1, 1.0),
            (-6, 2.0),
            (-48, 4.0),
            (-8 * 24, 8.0),
            (-31 * 24, 99.0),
            (1, 99.0),
        ] {
            db.execute(
                "INSERT INTO sessions (started_at, estimated_cost_usd) VALUES (?1, ?2)",
                rusqlite::params![
                    (now + Duration::hours(hours)).timestamp_millis() as f64 / 1000.0,
                    cost
                ],
            )
            .unwrap();
        }
        db.execute("INSERT INTO sessions (started_at, ended_at, last_activity_at, estimated_cost_usd) VALUES (0, 0, ?1, 0.5)", [now.timestamp_millis() as f64]).unwrap();
        db.execute(
            "INSERT INTO sessions (started_at, ended_at, estimated_cost_usd) VALUES (0, ?1, 0.25)",
            [now.timestamp_millis() as f64 / 1000.0],
        )
        .unwrap();
        db.execute(
            "INSERT INTO sessions (started_at) VALUES (?1)",
            [now.timestamp()],
        )
        .unwrap();
        if actual_column {
            db.execute(
                "UPDATE sessions SET actual_cost_usd = 0 WHERE estimated_cost_usd = 1",
                [],
            )
            .unwrap();
        }
        drop(db);
        assert!(matches!(detect(Some(&path)), Detection::Ready(_)));
        let usage = fetch(Some(&path), &Limits::default()).unwrap();
        for (window, used_usd) in usage.windows.iter().zip([1.75, 3.75, 7.75, 15.75]) {
            assert_eq!(
                window.meter,
                Meter::Spend {
                    used_usd: used_usd - if actual_column { 1.0 } else { 0.0 },
                    limit_usd: None
                }
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn sessions_use_actual_cost_and_last_activity_time() {
        check_database(true);
    }

    #[test]
    fn older_sessions_use_estimated_cost() {
        check_database(false);
    }
}
