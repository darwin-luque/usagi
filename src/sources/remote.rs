#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::usage::ProbeError;

pub fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .user_agent(concat!("usagi/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    })
}

pub fn response(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    hint: &str,
) -> Result<Value, ProbeError> {
    let mut response =
        response.map_err(|_| ProbeError::Network("Cannot reach the usage server.".into()))?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|_| ProbeError::Network("Cannot read the server response.".into()))?;
    parse_response(status, &body, hint)
}

fn parse_response(status: u16, body: &str, hint: &str) -> Result<Value, ProbeError> {
    match status {
        401 | 403 => Err(expired(hint)),
        200..=299 => json(body),
        _ => Err(ProbeError::Http {
            status,
            detail: body.chars().take(200).collect(),
        }),
    }
}

pub fn json(text: &str) -> Result<Value, ProbeError> {
    serde_json::from_str(text).map_err(|_| parse_error("Cannot read JSON."))
}

pub fn parse_error(detail: &str) -> ProbeError {
    ProbeError::Parse(detail.into())
}

pub fn expired(hint: &str) -> ProbeError {
    ProbeError::ExpiredCredentials { hint: hint.into() }
}

pub fn missing(hint: &str) -> ProbeError {
    ProbeError::MissingCredentials { hint: hint.into() }
}

pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub fn jwt_expiry(token: &str) -> Option<DateTime<Utc>> {
    let payload = URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?).ok()?;
    let payload: Value = serde_json::from_slice(&payload).ok()?;
    DateTime::from_timestamp(payload.get("exp")?.as_i64()?, 0)
}

pub fn keychain(service: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("security")
            .args(["find-generic-password", "-s", service, "-w"])
            .output()
            .ok()?;
        if output.status.success() {
            return String::from_utf8(output.stdout)
                .ok()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty());
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = service;
    None
}

pub fn on_path(binary: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| executable(&path.join(binary)))
    })
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_errors_keep_the_status_and_a_short_excerpt() {
        for status in [401, 403] {
            assert_eq!(
                parse_response(status, "bad", "Log in."),
                Err(expired("Log in."))
            );
        }
        assert_eq!(
            parse_response(429, &"é".repeat(250), ""),
            Err(ProbeError::Http {
                status: 429,
                detail: "é".repeat(200),
            })
        );
        assert!(matches!(
            parse_response(200, "bad", ""),
            Err(ProbeError::Parse(_))
        ));
        assert_eq!(parse_response(200, "{}", ""), Ok(serde_json::json!({})));
    }

    #[test]
    fn transport_failure_is_a_network_error() {
        assert_eq!(
            response(Err(ureq::Error::ConnectionFailed), "Log in."),
            Err(ProbeError::Network("Cannot reach the usage server.".into()))
        );
    }

    #[test]
    fn jwt_expiry_reads_unpadded_payloads_and_rejects_bad_tokens() {
        let payload = URL_SAFE_NO_PAD.encode(r#"{"exp":1793340649}"#);
        assert_eq!(
            jwt_expiry(&format!("header.{payload}.signature")),
            DateTime::from_timestamp(1793340649, 0)
        );
        for token in ["bad", "a.!.b", "a.e30.b"] {
            assert_eq!(jwt_expiry(token), None);
        }
    }
}
