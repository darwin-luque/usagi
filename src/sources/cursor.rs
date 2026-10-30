use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use super::{Detection, remote};
use crate::history::{Event, Tokens};
use crate::usage::{Meter, ProbeError, Reset, Span, Usage, Window};

const HINT: &str = "Open Cursor or run cursor-agent once to refresh the login.";

fn state_path(path: Option<&Path>) -> PathBuf {
    path.map(Path::to_path_buf).unwrap_or_else(|| {
        #[cfg(target_os = "macos")]
        let base = crate::config::home().join("Library/Application Support");
        #[cfg(not(target_os = "macos"))]
        let base = crate::config::home().join(".config");
        base.join("Cursor/User/globalStorage/state.vscdb")
    })
}

fn database_login(path: &Path) -> Option<(String, Option<String>)> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let lookup = |key: &str| -> Option<String> {
        connection
            .query_row("SELECT value FROM ItemTable WHERE key = ?", [key], |row| {
                row.get(0)
            })
            .optional()
            .ok()
            .flatten()
    };
    let token = lookup("cursorAuth/accessToken").filter(|token| !token.trim().is_empty())?;
    let plan = lookup("cursorAuth/stripeMembershipType").map(|plan| {
        if plan == "pro_plus" {
            "Pro+".into()
        } else {
            remote::capitalize(&plan)
        }
    });
    Some((token, plan))
}

fn candidates(path: Option<&Path>) -> Vec<(String, Option<String>)> {
    let mut candidates = Vec::new();
    if let Some(login) = database_login(&state_path(path)) {
        candidates.push(login);
    }
    if let Some(token) = remote::keychain("cursor-access-token") {
        candidates.push((token, None));
    }
    candidates
}

fn select_login(
    candidates: Vec<(String, Option<String>)>,
    now: DateTime<Utc>,
) -> Result<(String, Option<String>), ProbeError> {
    if candidates.is_empty() {
        return Err(remote::missing(
            "Open Cursor or run cursor-agent to log in.",
        ));
    }
    candidates
        .into_iter()
        .filter_map(|login| {
            remote::jwt_expiry(&login.0)
                .filter(|expiry| *expiry > now)
                .map(|expiry| (expiry, login))
        })
        .max_by_key(|(expiry, _)| *expiry)
        .map(|(_, login)| login)
        .ok_or_else(|| remote::expired(HINT))
}

const PAGE_SIZE: usize = 1000;
const PARALLEL_PAGES: usize = 8;

fn post(token: &str, method: &str, body: String) -> Result<Value, ProbeError> {
    let value = remote::response(
        remote::agent()
            .post(format!(
                "https://api2.cursor.sh/aiserver.v1.DashboardService/{method}"
            ))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .send(body),
        HINT,
    )?;
    authenticated(value)
}

fn authenticated(value: Value) -> Result<Value, ProbeError> {
    if value["code"] == "unauthenticated" {
        return Err(remote::expired(HINT));
    }
    Ok(value)
}

pub fn fetch(state_db: Option<&Path>) -> Result<Usage, ProbeError> {
    let now = Utc::now();
    let (token, plan) = select_login(candidates(state_db), now)?;
    let value = post(&token, "GetCurrentPeriodUsage", "{}".into())?;
    parse(&value, plan, now)
}

pub fn events(state_db: Option<&Path>, since: DateTime<Utc>) -> Result<Vec<Event>, ProbeError> {
    let now = Utc::now();
    let (token, _) = select_login(candidates(state_db), now)?;
    let token = token.as_str();
    let page = move |page: usize| {
        let body = serde_json::json!({
            "startDate": since.timestamp_millis().to_string(),
            "endDate": now.timestamp_millis().to_string(),
            "page": page,
            "pageSize": PAGE_SIZE,
        });
        parse_events(&post(token, "GetFilteredUsageEvents", body.to_string())?)
    };
    let (mut events, total) = page(1)?;
    let rest: Vec<usize> = (2..=total.div_ceil(PAGE_SIZE)).collect();
    for batch in rest.chunks(PARALLEL_PAGES) {
        let pages = std::thread::scope(|scope| {
            let handles: Vec<_> = batch
                .iter()
                .map(|&number| scope.spawn(move || page(number)))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("page thread does not panic"))
                .collect::<Vec<_>>()
        });
        for result in pages {
            events.extend(result?.0);
        }
    }
    Ok(events)
}

fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .unwrap_or(0.0)
}

fn parse_events(value: &Value) -> Result<(Vec<Event>, usize), ProbeError> {
    let items = match value.get("usageEventsDisplay") {
        None => &Vec::new(),
        Some(items) => items
            .as_array()
            .ok_or_else(|| remote::parse_error("Usage events are not a list."))?,
    };
    let events = items
        .iter()
        .filter_map(|item| {
            let usage = item.get("tokenUsage").filter(|usage| usage.is_object());
            let cents = match usage {
                Some(usage) => number(&usage["totalCents"]),
                None => number(&item["chargedCents"]),
            };
            let count = |name: &str| usage.map_or(0, |usage| number(&usage[name]) as u64);
            Some(Event {
                at: DateTime::from_timestamp_millis(
                    item["timestamp"]
                        .as_i64()
                        .or_else(|| item["timestamp"].as_str()?.parse().ok())?,
                )?,
                model: match item["model"].as_str() {
                    Some("default") => "auto",
                    Some(model) => model,
                    None => "unknown",
                }
                .to_owned(),
                tokens: Tokens {
                    input: count("inputTokens"),
                    output: count("outputTokens"),
                    cache_read: count("cacheReadTokens"),
                    cache_write: count("cacheWriteTokens"),
                },
                cost_usd: Some(cents / 100.0),
            })
        })
        .collect();
    Ok((events, number(&value["totalUsageEventsCount"]) as usize))
}

fn parse(value: &Value, plan: Option<String>, _now: DateTime<Utc>) -> Result<Usage, ProbeError> {
    let plan_usage = value["planUsage"]
        .as_object()
        .ok_or_else(|| remote::parse_error("Plan usage is missing."))?;
    let timestamp = |key: &str| -> Result<i64, ProbeError> {
        value[key]
            .as_str()
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| remote::parse_error("Billing time is not valid."))
    };
    let start = timestamp("billingCycleStart")?;
    let end = timestamp("billingCycleEnd")?;
    let seconds = end
        .checked_sub(start)
        .filter(|length| *length > 0)
        .ok_or_else(|| remote::parse_error("Billing time is not valid."))?
        / 1000;
    let reset = DateTime::from_timestamp_millis(end)
        .ok_or_else(|| remote::parse_error("Reset time is not valid."))?;
    let mut usage = Usage {
        plan,
        ..Usage::default()
    };
    for (key, scope) in [
        ("totalPercentUsed", None),
        ("autoPercentUsed", Some("auto")),
        ("apiPercentUsed", Some("api")),
    ] {
        let used = match plan_usage.get(key) {
            None => 0.0,
            Some(value) => value
                .as_f64()
                .ok_or_else(|| remote::parse_error("Used percent is not valid."))?,
        };
        usage.windows.push(Window {
            span: Span::from_seconds(seconds),
            scope: scope.map(str::to_owned),
            meter: Meter::Percent { used },
            reset: Reset::At(reset),
        });
    }
    Ok(usage)
}

pub fn detect(state_db: Option<&Path>) -> Detection {
    if !candidates(state_db).is_empty() {
        Detection::Ready("Login found.".into())
    } else if remote::on_path("cursor-agent")
        || remote::on_path("agent")
        || state_path(state_db).exists()
        || crate::config::home().join(".config/Cursor").exists()
        || Path::new("/Applications/Cursor.app").exists()
        || crate::config::home()
            .join("Applications/Cursor.app")
            .exists()
        || crate::config::home()
            .join("Library/Application Support/Cursor")
            .exists()
    {
        Detection::Partial("Installed. No login found.".into())
    } else {
        Detection::Absent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde_json::json;

    #[test]
    fn fixture_has_exact_plan_windows_and_resets() {
        let value = remote::json(include_str!("../../tests/fixtures/cursor_usage.json")).unwrap();
        let usage = parse(&value, Some("Pro+".into()), Utc::now()).unwrap();
        assert_eq!(
            usage,
            Usage {
                plan: Some("Pro+".into()),
                notes: vec![],
                windows: [None, Some("auto"), Some("api")]
                    .into_iter()
                    .map(|scope| Window {
                        span: Span::Month,
                        scope: scope.map(str::to_owned),
                        meter: Meter::Percent { used: 0.0 },
                        reset: Reset::At(DateTime::from_timestamp_millis(1795274614000).unwrap()),
                    })
                    .collect()
            }
        );
    }

    #[test]
    fn missing_proto_numbers_mean_zero_and_missing_usage_is_an_error() {
        let mut value =
            remote::json(include_str!("../../tests/fixtures/cursor_usage.json")).unwrap();
        value["planUsage"] = json!({});
        assert!(
            parse(&value, None, Utc::now())
                .unwrap()
                .windows
                .iter()
                .all(|window| window.meter == Meter::Percent { used: 0.0 })
        );
        assert!(matches!(
            parse(&json!({}), None, Utc::now()),
            Err(ProbeError::Parse(_))
        ));
    }

    #[test]
    fn unauthenticated_body_needs_a_refresh() {
        assert_eq!(
            authenticated(json!({"code":"unauthenticated"})),
            Err(remote::expired(HINT))
        );
        assert_eq!(authenticated(json!({})), Ok(json!({})));
    }

    #[test]
    fn newest_valid_token_wins_and_expired_tokens_need_a_refresh() {
        let now = DateTime::from_timestamp(100, 0).unwrap();
        let token = |exp| {
            format!(
                "a.{}.b",
                URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#))
            )
        };
        let newest = (token(300), Some("Pro+".into()));
        assert_eq!(
            select_login(
                vec![(token(200), None), newest.clone(), (token(1), None)],
                now
            ),
            Ok(newest)
        );
        assert_eq!(
            select_login(vec![(token(1), None)], now),
            Err(remote::expired(HINT))
        );
        assert!(matches!(
            select_login(vec![], now),
            Err(ProbeError::MissingCredentials { .. })
        ));
    }

    #[test]
    fn usage_events_keep_reported_cost_and_missing_numbers_mean_zero() {
        let value = json!({"totalUsageEventsCount": 3, "usageEventsDisplay": [
            {"timestamp":"1793334169846","model":"claude-opus-5-5-medium","chargedCents":999,
             "tokenUsage":{"inputTokens":60,"outputTokens":21260,"cacheWriteTokens":191165,
             "cacheReadTokens":1466454,"totalCents":167.5}},
            {"timestamp":"1788484405393","model":"cursor-grok-4.6-high-fast","chargedCents":4},
            {"timestamp":"bad","model":"skip"},
            {"timestamp":"1788484405394","model":"default","chargedCents":0}
        ]});
        let (mut events, total) = parse_events(&value).unwrap();
        assert_eq!(total, 3);
        assert_eq!(events.pop().unwrap().model, "auto");
        assert_eq!(
            events,
            vec![
                Event {
                    at: DateTime::from_timestamp_millis(1793334169846).unwrap(),
                    model: "claude-opus-5-5-medium".into(),
                    tokens: Tokens {
                        input: 60,
                        output: 21260,
                        cache_read: 1466454,
                        cache_write: 191165
                    },
                    cost_usd: Some(1.675),
                },
                Event {
                    at: DateTime::from_timestamp_millis(1788484405393).unwrap(),
                    model: "cursor-grok-4.6-high-fast".into(),
                    tokens: Tokens::default(),
                    cost_usd: Some(0.04),
                },
            ]
        );
        assert_eq!(parse_events(&json!({})).unwrap(), (vec![], 0));
        assert!(parse_events(&json!({"usageEventsDisplay": 1})).is_err());
    }

    #[test]
    #[ignore]
    fn live_fetch_smoke() {
        println!("Cursor: {:?}", fetch(None));
    }

    #[test]
    #[ignore]
    fn live_events_smoke() {
        let events = events(None, Utc::now() - chrono::Duration::days(30)).unwrap();
        println!("Cursor events: {}", events.len());
        assert!(!events.is_empty());
    }
}
