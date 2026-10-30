use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{Detection, remote};
use crate::usage::{Meter, ProbeError, Reset, Span, Usage, Window};

const HINT: &str = "Run codex once to refresh the login.";
const LOGIN_HINT: &str = "Usage limits need a ChatGPT login. Run codex login.";

fn home_path(home: Option<&Path>) -> PathBuf {
    home.map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .unwrap_or_else(|| crate::config::home().join(".codex"))
}

fn read_auth(home: &Path) -> Result<Value, ProbeError> {
    let text =
        std::fs::read_to_string(home.join("auth.json")).map_err(|_| remote::missing(LOGIN_HINT))?;
    remote::json(&text)
}

fn login(value: &Value, now: DateTime<Utc>) -> Result<(&str, &str), ProbeError> {
    let tokens = value
        .get("tokens")
        .filter(|value| value.is_object())
        .ok_or_else(|| remote::missing(LOGIN_HINT))?;
    let token = tokens["access_token"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| remote::missing(LOGIN_HINT))?;
    if remote::jwt_expiry(token).is_some_and(|expiry| expiry <= now) {
        return Err(remote::expired(HINT));
    }
    let account = tokens["account_id"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| remote::missing(LOGIN_HINT))?;
    Ok((token, account))
}

pub fn fetch(home: Option<&Path>) -> Result<Usage, ProbeError> {
    let auth = read_auth(&home_path(home))?;
    let now = Utc::now();
    let (token, account) = login(&auth, now)?;
    let value = remote::response(
        remote::agent()
            .get("https://chatgpt.com/backend-api/wham/usage")
            .header("Authorization", format!("Bearer {token}"))
            .header("ChatGPT-Account-Id", account)
            .call(),
        HINT,
    )?;
    parse(&value, now)
}

fn parse(value: &Value, _now: DateTime<Utc>) -> Result<Usage, ProbeError> {
    if !value.is_object() {
        return Err(remote::parse_error("Usage data is missing."));
    }
    let mut usage = Usage {
        plan: value["plan_type"].as_str().map(remote::capitalize),
        ..Usage::default()
    };
    add_windows(&mut usage, &value["rate_limit"], None)?;
    add_windows(&mut usage, &value["code_review_rate_limit"], Some("review"))?;
    if let Some(items) = value["additional_rate_limits"].as_array() {
        for item in items {
            let scope = item["limit_name"]
                .as_str()
                .or_else(|| item["metered_feature"].as_str());
            add_windows(&mut usage, &item["rate_limit"], scope)?;
        }
    }
    if value["credits"]["has_credits"] == true {
        let balance = &value["credits"]["balance"];
        let balance = balance
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| balance.to_string());
        usage.notes.push(format!("Credits: {balance}"));
    }
    Ok(usage)
}

fn add_windows(usage: &mut Usage, rate: &Value, scope: Option<&str>) -> Result<(), ProbeError> {
    if !rate.is_object() {
        return Ok(());
    }
    for key in ["primary_window", "secondary_window"] {
        let Some(value) = rate.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        let used = value["used_percent"]
            .as_f64()
            .ok_or_else(|| remote::parse_error("Used percent is missing."))?;
        let seconds = value["limit_window_seconds"]
            .as_i64()
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| remote::parse_error("Window length is not valid."))?;
        let reset = value["reset_at"]
            .as_i64()
            .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
            .ok_or_else(|| remote::parse_error("Reset time is not valid."))?;
        usage.windows.push(Window {
            span: Span::from_seconds(seconds),
            scope: scope.map(str::to_owned),
            meter: Meter::Percent { used },
            reset: Reset::At(reset),
        });
    }
    Ok(())
}

pub fn detect(home: Option<&Path>) -> Detection {
    let home = home_path(home);
    if read_auth(&home).is_ok_and(|value| value["tokens"].is_object()) {
        Detection::Ready("Login found.".into())
    } else if home.exists() || remote::on_path("codex") {
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
        let value = remote::json(include_str!("../../tests/fixtures/codex_usage.json")).unwrap();
        assert_eq!(
            parse(&value, Utc::now()).unwrap(),
            Usage {
                plan: Some("Plus".into()),
                notes: vec![],
                windows: vec![
                    Window {
                        span: Span::FiveHours,
                        scope: None,
                        meter: Meter::Percent { used: 54.0 },
                        reset: Reset::At(DateTime::from_timestamp(1793340649, 0).unwrap())
                    },
                    Window {
                        span: Span::Week,
                        scope: None,
                        meter: Meter::Percent { used: 8.0 },
                        reset: Reset::At(DateTime::from_timestamp(1793927449, 0).unwrap())
                    },
                ]
            }
        );
    }

    #[test]
    fn missing_tokens_and_expired_jwt_need_login() {
        assert_eq!(
            login(&json!({"OPENAI_API_KEY":"unused"}), Utc::now()),
            Err(remote::missing(LOGIN_HINT))
        );
        let token = format!("a.{}.b", URL_SAFE_NO_PAD.encode(r#"{"exp":1}"#));
        assert_eq!(
            login(
                &json!({"tokens":{"access_token":token,"account_id":"account"}}),
                Utc::now()
            ),
            Err(remote::expired(HINT))
        );
    }

    #[test]
    fn review_extra_limits_and_credits_are_visible() {
        let window = json!({"primary_window":{"used_percent":3,"limit_window_seconds":86400,"reset_at":1793340649}});
        let value = json!({"code_review_rate_limit":window,"additional_rate_limits":[
            {"limit_name":"models","rate_limit":window}, {"metered_feature":"tools","rate_limit":window}, null
        ],"credits":{"has_credits":true,"balance":"12.5"}});
        let usage = parse(&value, Utc::now()).unwrap();
        assert_eq!(usage.notes, ["Credits: 12.5"]);
        assert_eq!(usage.windows.len(), 3);
        for (window, scope) in usage.windows.iter().zip(["review", "models", "tools"]) {
            assert_eq!(
                window,
                &Window {
                    span: Span::Day,
                    scope: Some(scope.into()),
                    meter: Meter::Percent { used: 3.0 },
                    reset: Reset::At(DateTime::from_timestamp(1793340649, 0).unwrap())
                }
            );
        }
    }

    #[test]
    #[ignore]
    fn live_fetch_smoke() {
        println!("Codex: {:?}", fetch(None));
    }
}
