use std::process::Command;

use chrono::{NaiveDate, NaiveTime};
use serde_json::Value;

use super::{Detection, remote};
use crate::usage::{Meter, ProbeError, Reset, Span, Usage, Window};

const HINT: &str = "Run gh auth login.";

fn command_token(command: &mut Command) -> Option<String> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .and_then(|text| nonempty(&text))
}

fn nonempty(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.into())
    }
}

fn env_token() -> Option<String> {
    ["GH_TOKEN", "GITHUB_TOKEN"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().and_then(|text| nonempty(&text)))
}

fn token(token_command: Option<&str>) -> Result<String, ProbeError> {
    let token = match token_command {
        Some(command) => command_token(Command::new("sh").args(["-c", command])),
        None => env_token().or_else(|| command_token(Command::new("gh").args(["auth", "token"]))),
    };
    token.ok_or_else(|| remote::missing(HINT))
}

pub fn fetch(token_command: Option<&str>) -> Result<Usage, ProbeError> {
    let token = token(token_command)?;
    let value = remote::response(
        remote::agent()
            .get("https://api.github.com/copilot_internal/user")
            .header("Authorization", format!("token {token}"))
            .header("Accept", "application/json")
            .call(),
        HINT,
    )?;
    parse(&value)
}

fn parse(value: &Value) -> Result<Usage, ProbeError> {
    let date = value["quota_reset_date"]
        .as_str()
        .and_then(|text| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
        .ok_or_else(|| remote::parse_error("Reset date is not valid."))?;
    let reset = Reset::At(date.and_time(NaiveTime::MIN).and_utc());
    let snapshots = value["quota_snapshots"]
        .as_object()
        .ok_or_else(|| remote::parse_error("Quota data is missing."))?;
    let mut usage = Usage {
        plan: value["copilot_plan"].as_str().map(remote::capitalize),
        ..Usage::default()
    };
    for (key, snapshot) in snapshots {
        match snapshot["unlimited"].as_bool() {
            Some(true) => continue,
            Some(false) => {}
            None => return Err(remote::parse_error("Quota limit is missing.")),
        }
        let remaining = snapshot["percent_remaining"]
            .as_f64()
            .ok_or_else(|| remote::parse_error("Remaining percent is missing."))?;
        let scope = if key == "premium_interactions" {
            "premium"
        } else {
            key
        };
        usage.windows.push(Window {
            span: Span::Month,
            scope: Some(scope.into()),
            meter: Meter::Percent {
                used: 100.0 - remaining,
            },
            reset,
        });
    }
    if !snapshots.is_empty() && usage.windows.is_empty() {
        usage.notes.push("No limits on this plan.".into());
    }
    Ok(usage)
}

pub fn detect(_token_command: Option<&str>) -> Detection {
    if env_token().is_some()
        || (remote::on_path("gh")
            && command_token(Command::new("gh").args(["auth", "token"])).is_some())
    {
        Detection::Ready("Login found.".into())
    } else if crate::config::home().join(".copilot").exists() {
        Detection::Partial("Installed. No login found.".into())
    } else {
        Detection::Absent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use serde_json::json;

    #[test]
    fn fixture_has_exact_plan_and_no_limited_windows() {
        let value = remote::json(include_str!("../../tests/fixtures/copilot_user.json")).unwrap();
        assert_eq!(
            parse(&value).unwrap(),
            Usage {
                plan: Some("Business".into()),
                windows: vec![],
                notes: vec!["No limits on this plan.".into()]
            }
        );
    }

    #[test]
    fn limited_snapshots_show_percent_scope_and_midnight_reset() {
        let value = json!({"copilot_plan":"pro","quota_reset_date":"2026-11-01","quota_snapshots":{
            "premium_interactions":{"unlimited":false,"percent_remaining":72.5},
            "chat":{"unlimited":false,"percent_remaining":90},
            "completions":{"unlimited":true}
        }});
        let reset = Reset::At(
            DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
                .unwrap()
                .to_utc(),
        );
        assert_eq!(
            parse(&value).unwrap(),
            Usage {
                plan: Some("Pro".into()),
                notes: vec![],
                windows: vec![
                    Window {
                        span: Span::Month,
                        scope: Some("chat".into()),
                        meter: Meter::Percent { used: 10.0 },
                        reset
                    },
                    Window {
                        span: Span::Month,
                        scope: Some("premium".into()),
                        meter: Meter::Percent { used: 27.5 },
                        reset
                    },
                ]
            }
        );
    }

    #[test]
    fn empty_token_command_needs_login() {
        assert_eq!(token(Some("printf '  '")), Err(remote::missing(HINT)));
        assert_eq!(
            token(Some("printf ' sample-token\\n'")),
            Ok("sample-token".into())
        );
        assert_eq!(token(Some("exit 1")), Err(remote::missing(HINT)));
    }

    #[test]
    #[ignore]
    fn live_fetch_smoke() {
        println!("Copilot: {:?}", fetch(None));
    }
}
