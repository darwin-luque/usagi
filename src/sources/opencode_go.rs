use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};

use super::{Detection, ledger, remote};
use crate::config::Limits;
use crate::history::{Event, Tokens};
use crate::usage::{Meter, ProbeError, Reset, Span, Usage, Window};

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

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
const LOGIN_HINT: &str = "Create a valid API key in OpenCode Console and save it with opencode auth login for OpenCode Go. Signing in again does not renew an expired key.";

fn auth_path() -> PathBuf {
    ledger::default_data_home().join("opencode/auth.json")
}

fn api_key(text: &str) -> Result<String, ProbeError> {
    let value = remote::json(text)?;
    let auth = &value["opencode-go"];
    if auth["type"].as_str() != Some("api") {
        return Err(remote::missing(LOGIN_HINT));
    }
    auth["key"]
        .as_str()
        .filter(|key| !key.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| remote::missing(LOGIN_HINT))
}

pub fn fetch(_path: Option<&Path>, _limits: &Limits) -> Result<Usage, ProbeError> {
    let text = std::fs::read_to_string(auth_path()).map_err(|_| remote::missing(LOGIN_HINT))?;
    fetch_usage(USAGE_URL, &api_key(&text)?)
}

fn fetch_usage(url: &str, key: &str) -> Result<Usage, ProbeError> {
    let value = remote::response(
        remote::agent()
            .get(url)
            .header("Authorization", format!("Bearer {key}"))
            .call(),
        LOGIN_HINT,
    )?;
    parse_usage(value)
}

fn parse_usage(value: serde_json::Value) -> Result<Usage, ProbeError> {
    let windows = [
        ("rolling", Span::FiveHours),
        ("weekly", Span::Week),
        ("monthly", Span::Month),
    ]
    .into_iter()
    .map(|(name, span)| {
        let window = &value["usage"][name];
        let used = window["percent"]
            .as_f64()
            .filter(|percent| percent.is_finite() && *percent >= 0.0)
            .ok_or_else(|| {
                remote::parse_error(&format!("Missing or invalid Go {name} percent."))
            })?;
        let reset = window["resetsAt"]
            .as_str()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .ok_or_else(|| remote::parse_error(&format!("Missing or invalid Go {name} reset.")))?;
        Ok(Window {
            span,
            scope: None,
            meter: Meter::Percent { used },
            reset: Reset::At(reset.to_utc()),
        })
    })
    .collect::<Result<Vec<_>, ProbeError>>()?;
    Ok(Usage {
        plan: Some("Go".into()),
        windows,
        notes: vec![],
    })
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

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| {
            path.join(name).is_file() || cfg!(windows) && path.join(format!("{name}.exe")).is_file()
        })
    })
}

pub fn detect(path: Option<&Path>) -> Detection {
    let authenticated = std::fs::read_to_string(auth_path())
        .ok()
        .and_then(|text| api_key(&text).ok())
        .is_some();
    if authenticated {
        Detection::Ready("OpenCode Go API key found.".into())
    } else if database_path(path).is_file() || on_path("opencode") {
        Detection::Partial("Log in to OpenCode Go with opencode auth login.".into())
    } else {
        Detection::Absent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::Meter;
    use chrono::Duration;

    fn dashboard_usage() -> serde_json::Value {
        serde_json::json!({"usage": {
            "rolling": {"status": "ok", "percent": 0, "resetsAt": "2026-11-04T23:00:00Z"},
            "weekly": {"status": "ok", "percent": 2, "resetsAt": "2026-11-09T00:00:00Z"},
            "monthly": {"status": "rate-limited", "percent": 100, "resetsAt": "2026-11-14T23:00:00Z"}
        }})
    }

    #[test]
    fn dashboard_percentages_and_subscription_resets_are_preserved() {
        let usage = parse_usage(dashboard_usage()).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Go"));
        assert!(usage.notes.is_empty());
        assert_eq!(usage.windows.len(), 3);
        for (window, (span, used, reset)) in usage.windows.iter().zip([
            (Span::FiveHours, 0.0, "2026-11-04T23:00:00Z"),
            (Span::Week, 2.0, "2026-11-09T00:00:00Z"),
            (Span::Month, 100.0, "2026-11-14T23:00:00Z"),
        ]) {
            assert_eq!(window.span, span);
            assert_eq!(window.meter, Meter::Percent { used });
            assert_eq!(window.reset, Reset::At(reset.parse().unwrap()));
        }
    }

    #[test]
    fn incomplete_or_malformed_usage_is_not_reported_as_zero() {
        for name in ["rolling", "weekly", "monthly"] {
            for field in ["percent", "resetsAt"] {
                for invalid in [
                    serde_json::Value::Null,
                    serde_json::json!("invalid"),
                    serde_json::json!(-1),
                ] {
                    let mut value = dashboard_usage();
                    value["usage"][name][field] = invalid;
                    assert!(matches!(parse_usage(value), Err(ProbeError::Parse(_))));
                }
            }
        }
        assert!(parse_usage(serde_json::json!({})).is_err());
    }

    #[test]
    fn credentials_require_a_go_api_key() {
        assert_eq!(
            api_key(r#"{"opencode-go":{"type":"api","key":"test-key"}}"#).unwrap(),
            "test-key"
        );
        for text in [
            "{}",
            r#"{"opencode":{"type":"api","key":"zen-key"}}"#,
            r#"{"opencode-go":{"type":"oauth","access":"test"}}"#,
            r#"{"opencode-go":{"type":"api","key":" "}}"#,
        ] {
            assert!(matches!(
                api_key(text),
                Err(ProbeError::MissingCredentials { .. })
            ));
        }
        assert!(matches!(api_key("bad json"), Err(ProbeError::Parse(_))));
    }

    #[test]
    fn usage_request_authenticates_and_handles_errors_without_local_estimates() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        for (status, body) in [
            (200, dashboard_usage().to_string()),
            (401, "{}".into()),
            (403, "{}".into()),
            (429, "quota request rate limited".into()),
            (500, "server unavailable".into()),
            (200, "{}".into()),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/usage", listener.local_addr().unwrap());
            let malformed = body == "{}";
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    assert_eq!(stream.read(&mut byte).unwrap(), 1);
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("GET /usage HTTP/1.1"));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer test-key\r\n")
                );
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let result = fetch_usage(&url, "test-key");
            server.join().unwrap();
            match status {
                200 if malformed => assert!(matches!(result, Err(ProbeError::Parse(_)))),
                200 => assert_eq!(result.unwrap(), parse_usage(dashboard_usage()).unwrap()),
                401 | 403 => assert!(matches!(result, Err(ProbeError::ExpiredCredentials { .. }))),
                _ => assert!(
                    matches!(result, Err(ProbeError::Http { status: code, .. }) if code == status)
                ),
            }
        }
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
