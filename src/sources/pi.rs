use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    Detection, jsonl,
    ledger::{self, HORIZON, Spend},
};
use crate::config::Limits;
use crate::history::{Event, Tokens};
use crate::usage::{ProbeError, Usage};

fn sessions_path(path: Option<&Path>) -> PathBuf {
    path.map(Path::to_path_buf).unwrap_or_else(|| {
        std::env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::config::home().join(".pi/agent"))
            .join("sessions")
    })
}

pub fn fetch(path: Option<&Path>, limits: &Limits) -> Result<Usage, ProbeError> {
    let now = Utc::now();
    let lines = read_sessions(&sessions_path(path), now - HORIZON)?;
    parse_lines(&lines, limits, now)
}

fn read_sessions(path: &Path, since: DateTime<Utc>) -> Result<Vec<String>, ProbeError> {
    let read = || -> std::io::Result<Vec<String>> {
        let mut lines = Vec::new();
        for file in jsonl::files(path, since)? {
            jsonl::lines(&file, |line| lines.push(line.to_owned()))?;
        }
        Ok(lines)
    };
    read().map_err(|error| ProbeError::Parse(format!("Cannot read Pi sessions. {error}")))
}

pub fn events(path: Option<&Path>, since: DateTime<Utc>) -> Result<Vec<Event>, ProbeError> {
    let mut events = Vec::new();
    jsonl::read(&sessions_path(path), since, |file| {
        jsonl::lines(file, |line| {
            if let Some(event) = parse_event(line)
                && event.at >= since
            {
                events.push(event);
            }
        })
    })?;
    Ok(events)
}

fn assistant_message(line: &str) -> Option<(DateTime<Utc>, Value)> {
    let mut value: Value = serde_json::from_str(line).ok()?;
    if value["type"] != "message" || value["message"]["role"] != "assistant" {
        return None;
    }
    let at = if let Some(timestamp) = value["message"].get("timestamp") {
        DateTime::from_timestamp_millis(timestamp.as_i64()?)?
    } else {
        DateTime::parse_from_rfc3339(value["timestamp"].as_str()?)
            .ok()?
            .to_utc()
    };
    Some((at, value["message"].take()))
}

fn parse_event(line: &str) -> Option<Event> {
    let (at, message) = assistant_message(line)?;
    let usage = &message["usage"];
    let count = |name: &str| usage[name].as_u64().unwrap_or(0);
    Some(Event {
        at,
        model: message["model"].as_str().unwrap_or("unknown").to_owned(),
        tokens: Tokens {
            input: count("input"),
            output: count("output"),
            cache_read: count("cacheRead"),
            cache_write: count("cacheWrite"),
        },
        cost_usd: usage["cost"]["total"].as_f64(),
    })
}

fn parse_lines(lines: &[String], limits: &Limits, now: DateTime<Utc>) -> Result<Usage, ProbeError> {
    let entries: Vec<_> = lines
        .iter()
        .filter_map(|line| {
            let (at, message) = assistant_message(line)?;
            Some(Spend {
                at,
                usd: message["usage"]["cost"]["total"].as_f64()?,
            })
        })
        .collect();
    Ok(Usage {
        plan: None,
        windows: ledger::windows(&entries, limits, now),
        notes: vec![
            "Cost is an estimate from local Pi sessions.".into(),
            "Pi uses the plan of its provider. Add the claude or codex harness to see plan limits."
                .into(),
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
    if sessions_path(path).is_dir() {
        Detection::Ready("Local sessions found.".into())
    } else if on_path("pi") {
        Detection::Partial("Check the Pi sessions folder.".into())
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
    fn recursive_sessions_skip_bad_lines_and_use_message_time() {
        let root = std::env::temp_dir().join(format!(
            "usagi-pi-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let now = Utc::now();
        let message = |hours, cost| {
            serde_json::json!({"type":"message", "timestamp":"2000-01-01T00:00:00Z", "message":{
            "role":"assistant", "timestamp":(now + Duration::hours(hours)).timestamp_millis(), "usage":{"cost":{"total":cost}}
        }}).to_string()
        };
        let iso = serde_json::json!({"type":"message", "timestamp":now.to_rfc3339(), "message":{
            "role":"assistant", "usage":{"cost":{"total":0.5}}
        }})
        .to_string();
        std::fs::write(
            root.join("one.jsonl"),
            [
                message(-1, 1.0),
                "broken".into(),
                iso,
                r#"{"type":"message","message":{"role":"user"}}"#.into(),
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(
            root.join("nested/two.jsonl"),
            [
                message(-6, 2.0),
                message(-48, 4.0),
                message(-8 * 24, 8.0),
                message(-31 * 24, 99.0),
                message(1, 99.0),
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(root.join("skip.txt"), message(-1, 99.0)).unwrap();
        let old = std::fs::File::create(root.join("old.jsonl")).unwrap();
        std::io::Write::write_all(&mut &old, message(-1, 99.0).as_bytes()).unwrap();
        old.set_times(std::fs::FileTimes::new().set_modified((now - Duration::days(31)).into()))
            .unwrap();
        assert!(matches!(detect(Some(&root)), Detection::Ready(_)));
        let lines = read_sessions(&root, now - HORIZON).unwrap();
        let usage = parse_lines(&lines, &Limits::default(), now).unwrap();
        for (window, used_usd) in usage.windows.iter().zip([1.5, 3.5, 7.5, 15.5]) {
            assert_eq!(
                window.meter,
                Meter::Spend {
                    used_usd,
                    limit_usd: None
                }
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
