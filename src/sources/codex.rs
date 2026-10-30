use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{Detection, jsonl, remote};
use crate::history::{Event, Tokens};
use crate::usage::{Meter, ProbeError, Reset, Span, Usage, Window};

const HINT: &str = "Run codex once to refresh the login.";
const LOGIN_HINT: &str = "Usage limits need a ChatGPT login. Run codex login.";

fn home_path(home: Option<&Path>) -> PathBuf {
    home.map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .unwrap_or_else(|| crate::config::home().join(".codex"))
}

pub fn events(home: Option<&Path>, since: DateTime<Utc>) -> Result<Vec<Event>, ProbeError> {
    let mut events = Vec::new();
    jsonl::read(&home_path(home).join("sessions"), since, |file| {
        let mut rollout = Rollout::default();
        jsonl::lines(file, |line| {
            if let Some(event) = rollout.read(line)
                && event.at >= since
            {
                events.push(event);
            }
        })
    })?;
    Ok(events)
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Totals {
    input: u64,
    cached: u64,
    cache_write: u64,
    output: u64,
}

impl Totals {
    fn parse(value: &Value) -> Totals {
        let count = |name: &str| value[name].as_u64().unwrap_or(0);
        Totals {
            input: count("input_tokens"),
            cached: count("cached_input_tokens"),
            cache_write: count("cache_write_input_tokens"),
            output: count("output_tokens"),
        }
    }

    fn since(self, previous: Totals) -> Totals {
        Totals {
            input: self.input.saturating_sub(previous.input),
            cached: self.cached.saturating_sub(previous.cached),
            cache_write: self.cache_write.saturating_sub(previous.cache_write),
            output: self.output.saturating_sub(previous.output),
        }
    }

    fn tokens(self) -> Tokens {
        Tokens {
            input: self
                .input
                .saturating_sub(self.cached)
                .saturating_sub(self.cache_write),
            output: self.output,
            cache_read: self.cached,
            cache_write: self.cache_write,
        }
    }
}

#[derive(Default)]
struct Rollout {
    model: Option<String>,
    totals: Totals,
}

impl Rollout {
    fn read(&mut self, line: &str) -> Option<Event> {
        if !line.contains("\"turn_context\"") && !line.contains("\"token_count\"") {
            return None;
        }
        let value: Value = serde_json::from_str(line).ok()?;
        let payload = &value["payload"];
        if value["type"] == "turn_context" {
            if let Some(model) = payload["model"].as_str() {
                self.model = Some(model.to_owned());
            }
            return None;
        }
        if value["type"] != "event_msg" || payload["type"] != "token_count" {
            return None;
        }
        let totals = Totals::parse(payload["info"].get("total_token_usage")?);
        let delta = totals.since(self.totals);
        self.totals = totals;
        if delta == Totals::default() {
            return None;
        }
        Some(Event {
            at: DateTime::parse_from_rfc3339(value["timestamp"].as_str()?)
                .ok()?
                .to_utc(),
            model: self.model.clone().unwrap_or_else(|| "unknown".into()),
            tokens: delta.tokens(),
            cost_usd: None,
        })
    }
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
    fn token_events_are_deltas_of_the_running_total_with_the_current_model() {
        let context =
            |model: &str| json!({"type":"turn_context","payload":{"model":model}}).to_string();
        let count = |time: &str, input: u64, cached: u64, output: u64| {
            json!({"timestamp":time,"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"input_tokens":input,"cached_input_tokens":cached,
                "cache_write_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":1,
                "total_tokens":input + output}}}})
            .to_string()
        };
        let lines = [
            count("2026-10-29T10:00:00Z", 100, 0, 5),
            context("gpt-6.1-sol"),
            count("2026-10-29T10:01:00Z", 300, 150, 15),
            count("2026-10-29T10:01:01Z", 300, 150, 15),
            json!({"type":"event_msg","payload":{"type":"token_count","info":null}}).to_string(),
            context("gpt-6-astra"),
            count("2026-10-29T10:02:00Z", 1300, 950, 115),
            "broken".into(),
        ];
        let mut rollout = Rollout::default();
        let events: Vec<_> = lines.iter().filter_map(|line| rollout.read(line)).collect();
        let summary: Vec<_> = events
            .iter()
            .map(|event| (event.model.as_str(), event.tokens))
            .collect();
        let tokens = |input, output, cache_read| Tokens {
            input,
            output,
            cache_read,
            cache_write: 0,
        };
        assert_eq!(
            summary,
            [
                ("unknown", tokens(100, 5, 0)),
                ("gpt-6.1-sol", tokens(50, 10, 150)),
                ("gpt-6-astra", tokens(200, 100, 800)),
            ]
        );
        assert_eq!(
            events[2].at,
            DateTime::parse_from_rfc3339("2026-10-29T10:02:00Z").unwrap()
        );
    }

    #[test]
    fn cache_writes_are_not_counted_as_input() {
        let totals = Totals {
            input: 100,
            cached: 30,
            cache_write: 20,
            output: 7,
        };
        assert_eq!(
            totals.tokens(),
            Tokens {
                input: 50,
                output: 7,
                cache_read: 30,
                cache_write: 20
            }
        );
    }

    #[test]
    #[ignore]
    fn live_fetch_smoke() {
        println!("Codex: {:?}", fetch(None));
    }
}
