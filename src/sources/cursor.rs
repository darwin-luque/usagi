use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use super::{Detection, remote};
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

pub fn fetch(state_db: Option<&Path>) -> Result<Usage, ProbeError> {
    let now = Utc::now();
    let (token, plan) = select_login(candidates(state_db), now)?;
    let value = remote::response(
        remote::agent()
            .post("https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .send("{}"),
        HINT,
    )?;
    parse(&value, plan, now)
}

fn parse(value: &Value, plan: Option<String>, _now: DateTime<Utc>) -> Result<Usage, ProbeError> {
    if value["code"] == "unauthenticated" {
        return Err(remote::expired(HINT));
    }
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
            parse(&json!({"code":"unauthenticated"}), None, Utc::now()),
            Err(remote::expired(HINT))
        );
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
    #[ignore]
    fn live_fetch_smoke() {
        println!("Cursor: {:?}", fetch(None));
    }
}
