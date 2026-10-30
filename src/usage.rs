use std::fmt;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    FiveHours,
    Day,
    Week,
    Month,
    Other { seconds: i64 },
}

impl Span {
    pub fn from_seconds(seconds: i64) -> Span {
        match seconds {
            18_000 => Span::FiveHours,
            86_400 => Span::Day,
            604_800 => Span::Week,
            2_419_200..=2_678_400 => Span::Month,
            _ => Span::Other { seconds },
        }
    }

    pub fn length(self) -> Duration {
        match self {
            Span::FiveHours => Duration::hours(5),
            Span::Day => Duration::days(1),
            Span::Week => Duration::days(7),
            Span::Month => Duration::days(30),
            Span::Other { seconds } => Duration::seconds(seconds),
        }
    }

    pub fn label(self) -> String {
        match self {
            Span::FiveHours => "5h".into(),
            Span::Day => "day".into(),
            Span::Week => "week".into(),
            Span::Month => "month".into(),
            Span::Other { seconds } if seconds % 86_400 == 0 => format!("{}d", seconds / 86_400),
            Span::Other { seconds } if seconds % 3_600 == 0 => format!("{}h", seconds / 3_600),
            Span::Other { seconds } => format!("{}m", seconds / 60),
        }
    }

    pub fn parse(text: &str) -> Option<Span> {
        let span = match text.trim() {
            "5h" => Span::FiveHours,
            "day" | "1d" | "24h" => Span::Day,
            "week" | "7d" => Span::Week,
            "month" | "30d" => Span::Month,
            other => {
                let (number, unit) = other.split_at(other.len().checked_sub(1)?);
                let number: i64 = number.parse().ok().filter(|n| *n > 0)?;
                let seconds = match unit {
                    "m" => number * 60,
                    "h" => number * 3_600,
                    "d" => number * 86_400,
                    _ => return None,
                };
                Span::from_seconds(seconds)
            }
        };
        Some(span)
    }
}

impl Serialize for Span {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.label())
    }
}

impl<'de> Deserialize<'de> for Span {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Span, D::Error> {
        let text = String::deserialize(deserializer)?;
        Span::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown span \"{text}\". Use 5h, day, week, month, or a number with m, h, or d"
            ))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Meter {
    Percent {
        used: f64,
    },
    Spend {
        used_usd: f64,
        limit_usd: Option<f64>,
    },
}

impl Meter {
    pub fn percent(self) -> Option<f64> {
        match self {
            Meter::Percent { used } => Some(used),
            Meter::Spend {
                used_usd,
                limit_usd: Some(limit),
            } if limit > 0.0 => Some(used_usd / limit * 100.0),
            Meter::Spend { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reset {
    At(DateTime<Utc>),
    Rolling,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(into = "WireWindow", try_from = "WireWindow")]
pub struct Window {
    pub span: Span,
    pub scope: Option<String>,
    pub meter: Meter,
    pub reset: Reset,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireWindow {
    pub span: Span,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rolling: bool,
}

impl From<Window> for WireWindow {
    fn from(window: Window) -> WireWindow {
        let (spend_usd, limit_usd) = match window.meter {
            Meter::Percent { .. } => (None, None),
            Meter::Spend {
                used_usd,
                limit_usd,
            } => (Some(used_usd), limit_usd),
        };
        WireWindow {
            span: window.span,
            scope: window.scope,
            used_percent: window.meter.percent(),
            spend_usd,
            limit_usd,
            resets_at: match window.reset {
                Reset::At(at) => Some(at),
                Reset::Rolling | Reset::Unknown => None,
            },
            rolling: window.reset == Reset::Rolling,
        }
    }
}

impl TryFrom<WireWindow> for Window {
    type Error = String;

    fn try_from(wire: WireWindow) -> Result<Window, String> {
        let meter = match (wire.spend_usd, wire.used_percent) {
            (Some(used_usd), _) => Meter::Spend {
                used_usd,
                limit_usd: wire.limit_usd,
            },
            (None, Some(used)) => Meter::Percent { used },
            (None, None) => {
                return Err(format!(
                    "The \"{}\" window needs used_percent or spend_usd",
                    wire.span.label()
                ));
            }
        };
        let reset = match (wire.resets_at, wire.rolling) {
            (Some(at), _) => Reset::At(at),
            (None, true) => Reset::Rolling,
            (None, false) => Reset::Unknown,
        };
        Ok(Window {
            span: wire.span,
            scope: wire.scope,
            meter,
            reset,
        })
    }
}

impl Window {
    pub fn label(&self) -> String {
        match &self.scope {
            Some(scope) => format!("{} {}", self.span.label(), scope),
            None => self.span.label(),
        }
    }

    pub fn elapsed_percent(&self, now: DateTime<Utc>) -> Option<f64> {
        let Reset::At(reset) = self.reset else {
            return None;
        };
        let length = self.span.length().num_seconds() as f64;
        let left = (reset - now).num_seconds() as f64;
        Some(((length - left) / length * 100.0).clamp(0.0, 100.0))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default)]
    pub windows: Vec<Window>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProbeError {
    MissingCredentials { hint: String },
    ExpiredCredentials { hint: String },
    Http { status: u16, detail: String },
    Network(String),
    Parse(String),
    Command(String),
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProbeError::MissingCredentials { hint } => write!(f, "No credentials found. {hint}"),
            ProbeError::ExpiredCredentials { hint } => write!(f, "Credentials expired. {hint}"),
            ProbeError::Http { status, detail } => {
                write!(f, "Server sent status {status}. {detail}")
            }
            ProbeError::Network(detail) => write!(f, "Network failed. {detail}"),
            ProbeError::Parse(detail) => write!(f, "Response format is not known. {detail}"),
            ProbeError::Command(detail) => write!(f, "Command failed. {detail}"),
        }
    }
}

impl std::error::Error for ProbeError {}

impl Serialize for ProbeError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    pub color: crate::theme::Rgb,
    pub fetched_at: DateTime<Utc>,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Usage(Usage),
    Error(ProbeError),
}

impl From<Result<Usage, ProbeError>> for Outcome {
    fn from(result: Result<Usage, ProbeError>) -> Outcome {
        match result {
            Ok(usage) => Outcome::Usage(usage),
            Err(error) => Outcome::Error(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_parse_from_labels_and_seconds() {
        assert_eq!(Span::parse("5h"), Some(Span::FiveHours));
        assert_eq!(Span::parse("7d"), Some(Span::Week));
        assert_eq!(Span::parse("90m"), Some(Span::Other { seconds: 5_400 }));
        assert_eq!(Span::parse("soon"), None);
        assert_eq!(Span::from_seconds(2_592_000), Span::Month);
        assert_eq!(Span::Other { seconds: 7_200 }.label(), "2h");
    }

    #[test]
    fn hand_written_window_json_parses() {
        let usage: Usage = serde_json::from_str(
            r#"{"plan":"Pro","windows":[
                {"span":"5h","used_percent":22.5,"resets_at":"2026-10-30T06:10:00Z"},
                {"span":"month","scope":"api","spend_usd":3.0,"limit_usd":12.0,"rolling":true}
            ]}"#,
        )
        .unwrap();
        assert_eq!(usage.windows[0].meter, Meter::Percent { used: 22.5 });
        assert_eq!(usage.windows[1].meter.percent(), Some(25.0));
        assert_eq!(usage.windows[1].reset, Reset::Rolling);
        assert_eq!(usage.windows[1].label(), "month api");
    }

    #[test]
    fn window_without_a_meter_is_rejected() {
        let result: Result<Window, _> = serde_json::from_str(r#"{"span":"5h"}"#);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("used_percent or spend_usd")
        );
    }

    #[test]
    fn serialized_usage_reads_back_unchanged() {
        let usage = Usage {
            plan: None,
            windows: vec![Window {
                span: Span::Week,
                scope: None,
                meter: Meter::Spend {
                    used_usd: 1.5,
                    limit_usd: Some(30.0),
                },
                reset: Reset::Unknown,
            }],
            notes: vec![],
        };
        let text = serde_json::to_string(&usage).unwrap();
        assert!(text.contains("\"used_percent\":5.0"));
        assert_eq!(serde_json::from_str::<Usage>(&text).unwrap(), usage);
    }

    #[test]
    fn pace_is_the_share_of_the_window_that_passed() {
        let now = DateTime::parse_from_rfc3339("2026-10-30T00:00:00Z")
            .unwrap()
            .to_utc();
        let window = Window {
            span: Span::FiveHours,
            scope: None,
            meter: Meter::Percent { used: 10.0 },
            reset: Reset::At(now + Duration::hours(4)),
        };
        assert_eq!(window.elapsed_percent(now), Some(20.0));
    }
}
