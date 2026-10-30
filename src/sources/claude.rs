use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{Detection, jsonl, remote};
use crate::history::{Event, Tokens};
use crate::usage::{Meter, ProbeError, Reset, Span, Usage, Window};

const HINT: &str = "Run claude once to refresh the login.";

fn config_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::home().join(".claude"))
}

fn credentials_path() -> PathBuf {
    config_dir().join(".credentials.json")
}

pub fn events(since: DateTime<Utc>) -> Result<Vec<Event>, ProbeError> {
    read_events(&config_dir().join("projects"), since)
}

fn read_events(root: &Path, since: DateTime<Utc>) -> Result<Vec<Event>, ProbeError> {
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    jsonl::read(root, since, |file| {
        jsonl::lines(file, |line| {
            if let Some((key, event)) = parse_line(line)
                && event.at >= since
                && key.is_none_or(|key| seen.insert(key))
            {
                events.push(event);
            }
        })
    })?;
    Ok(events)
}

fn parse_line(line: &str) -> Option<(Option<String>, Event)> {
    if !line.contains("\"usage\"") {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    if value["type"] != "assistant" {
        return None;
    }
    let message = &value["message"];
    let usage = message.get("usage").filter(|usage| usage.is_object())?;
    let model = message["model"].as_str()?;
    if model == "<synthetic>" {
        return None;
    }
    let key = match (message["id"].as_str(), value["requestId"].as_str()) {
        (None, None) => None,
        (id, request) => Some(format!("{}:{}", id.unwrap_or(""), request.unwrap_or(""))),
    };
    let count = |name: &str| usage[name].as_u64().unwrap_or(0);
    Some((
        key,
        Event {
            at: DateTime::parse_from_rfc3339(value["timestamp"].as_str()?)
                .ok()?
                .to_utc(),
            model: model.to_owned(),
            tokens: Tokens {
                input: count("input_tokens"),
                output: count("output_tokens"),
                cache_read: count("cache_read_input_tokens"),
                cache_write: count("cache_creation_input_tokens"),
            },
            cost_usd: None,
        },
    ))
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
    fn each_message_counts_once_across_content_blocks_and_subagents() {
        let root = std::env::temp_dir().join(format!(
            "usagi-claude-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir_all(root.join("project/session/subagents")).unwrap();
        let line = |id: &str, request: &str, model: &str, time: &str| {
            json!({"type":"assistant","requestId":request,"timestamp":time,"message":{
                "id":id,"model":model,"usage":{"input_tokens":2,"output_tokens":10,
                "cache_read_input_tokens":300,"cache_creation_input_tokens":40}}})
            .to_string()
        };
        std::fs::write(
            root.join("project/main.jsonl"),
            [
                line("m1", "r1", "claude-opus-5-5", "2026-10-29T10:00:00Z"),
                line("m1", "r1", "claude-opus-5-5", "2026-10-29T10:00:01Z"),
                line("m2", "r2", "<synthetic>", "2026-10-29T10:00:00Z"),
                line("m3", "r3", "claude-opus-5-5", "2026-09-01T10:00:00Z"),
                r#"{"type":"user","message":{"usage":{}}}"#.into(),
                "broken \"usage\"".into(),
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(
            root.join("project/session/subagents/agent.jsonl"),
            line("m4", "r4", "claude-haiku-5-5", "2026-10-29T11:00:00Z"),
        )
        .unwrap();
        let since = DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
            .unwrap()
            .to_utc();
        let events = read_events(&root, since).unwrap();
        let models: Vec<_> = events.iter().map(|event| event.model.as_str()).collect();
        assert_eq!(models, ["claude-opus-5-5", "claude-haiku-5-5"]);
        assert_eq!(
            events[0].tokens,
            Tokens {
                input: 2,
                output: 10,
                cache_read: 300,
                cache_write: 40
            }
        );
        assert_eq!(events[0].cost_usd, None);
        assert_eq!(read_events(&root.join("missing"), since), Ok(vec![]));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore]
    fn live_fetch_smoke() {
        println!("Claude: {:?}", fetch(None));
    }
}
