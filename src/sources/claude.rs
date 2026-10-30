use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{Detection, remote};
use crate::usage::{Meter, ProbeError, Reset, Span, Usage, Window};

const HINT: &str = "Run claude once to refresh the login.";

fn credentials_path() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::home().join(".claude"))
        .join(".credentials.json")
}

fn read_credentials(path: Option<&Path>) -> Result<Value, ProbeError> {
    if path.is_none()
        && let Some(text) = remote::keychain("Claude Code-credentials")
    {
        return remote::json(&text);
    }
    let text =
        std::fs::read_to_string(path.map(Path::to_path_buf).unwrap_or_else(credentials_path))
            .map_err(|_| remote::missing("Run claude to log in."))?;
    remote::json(&text)
}

fn login(value: &Value, now: DateTime<Utc>) -> Result<(&str, Option<String>), ProbeError> {
    let oauth = &value["claudeAiOauth"];
    if oauth["expiresAt"]
        .as_i64()
        .is_some_and(|expiry| expiry <= now.timestamp_millis())
    {
        return Err(remote::expired(HINT));
    }
    let token = oauth["accessToken"]
        .as_str()
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| remote::missing("Run claude to log in."))?;
    Ok((
        token,
        oauth["subscriptionType"].as_str().map(remote::capitalize),
    ))
}

pub fn fetch(credentials: Option<&Path>) -> Result<Usage, ProbeError> {
    let credentials = read_credentials(credentials)?;
    let now = Utc::now();
    let (token, plan) = login(&credentials, now)?;
    let value = remote::response(
        remote::agent()
            .get("https://api.anthropic.com/api/oauth/usage")
            .header("Authorization", format!("Bearer {token}"))
            .header("anthropic-beta", "oauth-2025-04-20")
            .call(),
        HINT,
    )?;
    parse(&value, plan, now)
}

fn parse(value: &Value, plan: Option<String>, _now: DateTime<Utc>) -> Result<Usage, ProbeError> {
    let object = value
        .as_object()
        .ok_or_else(|| remote::parse_error("Usage data is missing."))?;
    let mut usage = Usage {
        plan,
        ..Usage::default()
    };
    for (key, span, scope) in [
        ("five_hour", Span::FiveHours, None),
        ("seven_day", Span::Week, None),
    ] {
        add_window(&mut usage, &value[key], span, scope)?;
    }
    for (key, window) in object {
        if let Some(scope) = key.strip_prefix("seven_day_") {
            add_window(&mut usage, window, Span::Week, Some(scope.into()))?;
        }
    }
    if value["extra_usage"]["is_enabled"] == true {
        add_window(
            &mut usage,
            &value["extra_usage"],
            Span::Month,
            Some("extra".into()),
        )?;
    }
    Ok(usage)
}

fn add_window(
    usage: &mut Usage,
    value: &Value,
    span: Span,
    scope: Option<String>,
) -> Result<(), ProbeError> {
    let Some(used) = value["utilization"].as_f64() else {
        return Ok(());
    };
    let reset = match value.get("resets_at").filter(|value| !value.is_null()) {
        Some(value) => Reset::At(
            DateTime::parse_from_rfc3339(
                value
                    .as_str()
                    .ok_or_else(|| remote::parse_error("Reset time is not valid."))?,
            )
            .map_err(|_| remote::parse_error("Reset time is not valid."))?
            .to_utc(),
        ),
        None => Reset::Unknown,
    };
    usage.windows.push(Window {
        span,
        scope,
        meter: Meter::Percent { used },
        reset,
    });
    Ok(())
}

pub fn detect(credentials: Option<&Path>) -> Detection {
    let path = credentials
        .map(Path::to_path_buf)
        .unwrap_or_else(credentials_path);
    let found = path.is_file();
    #[cfg(target_os = "macos")]
    let mut found = found;
    #[cfg(target_os = "macos")]
    if credentials.is_none() {
        found |= std::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials"])
            .output()
            .is_ok_and(|output| output.status.success());
    }
    if found {
        Detection::Ready("Login found.".into())
    } else if remote::on_path("claude") || crate::config::home().join(".claude").exists() {
        Detection::Partial("Installed. No login found.".into())
    } else {
        Detection::Absent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fixture_has_exact_plan_windows_and_resets() {
        let value = remote::json(include_str!("../../tests/fixtures/claude_usage.json")).unwrap();
        let usage = parse(&value, Some("Pro".into()), Utc::now()).unwrap();
        let expected: Usage = serde_json::from_value(json!({"plan":"Pro", "windows":[
            {"span":"5h","used_percent":50.0,"resets_at":"2026-10-30T06:09:59.657888Z"},
            {"span":"week","used_percent":40.0,"resets_at":"2026-10-30T16:59:59.657910Z"}
        ]}))
        .unwrap();
        assert_eq!(usage, expected);
    }

    #[test]
    fn scoped_and_extra_usage_skip_null_and_unknown_values() {
        let value = json!({"seven_day_opus":{"utilization":12}, "seven_day_sonnet":null,
            "unknown":{"utilization":88}, "extra_usage":{"is_enabled":true,"utilization":3}});
        let usage = parse(&value, None, Utc::now()).unwrap();
        assert_eq!(
            usage.windows,
            vec![
                Window {
                    span: Span::Week,
                    scope: Some("opus".into()),
                    meter: Meter::Percent { used: 12.0 },
                    reset: Reset::Unknown
                },
                Window {
                    span: Span::Month,
                    scope: Some("extra".into()),
                    meter: Meter::Percent { used: 3.0 },
                    reset: Reset::Unknown
                },
            ]
        );
    }

    #[test]
    fn expired_login_needs_a_refresh() {
        assert_eq!(
            login(
                &json!({"claudeAiOauth":{"accessToken":"secret","expiresAt":1}}),
                Utc::now()
            ),
            Err(remote::expired(HINT))
        );
        assert!(matches!(
            login(&json!({}), Utc::now()),
            Err(ProbeError::MissingCredentials { .. })
        ));
        assert_eq!(
            login(
                &json!({"claudeAiOauth":{"accessToken":"secret","subscriptionType":"max"}}),
                Utc::now()
            )
            .unwrap()
            .1,
            Some("Max".into())
        );
    }

    #[test]
    #[ignore]
    fn live_fetch_smoke() {
        println!("Claude: {:?}", fetch(None));
    }
}
