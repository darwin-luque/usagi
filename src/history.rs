use std::ops::{Add, AddAssign};
use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::config::Harness;
use crate::theme::Rgb;
use crate::usage::ProbeError;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    pub fn total(self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

impl Add for Tokens {
    type Output = Tokens;

    fn add(self, other: Tokens) -> Tokens {
        Tokens {
            input: self.input + other.input,
            output: self.output + other.output,
            cache_read: self.cache_read + other.cache_read,
            cache_write: self.cache_write + other.cache_write,
        }
    }
}

impl AddAssign for Tokens {
    fn add_assign(&mut self, other: Tokens) {
        *self = *self + other;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub at: DateTime<Utc>,
    pub model: String,
    pub tokens: Tokens,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum History {
    Events(Vec<Event>),
    Unsupported,
    Failed(ProbeError),
}

impl From<Result<Vec<Event>, ProbeError>> for History {
    fn from(result: Result<Vec<Event>, ProbeError>) -> History {
        match result {
            Ok(events) => History::Events(events),
            Err(error) => History::Failed(error),
        }
    }
}

pub struct Harvest {
    pub id: String,
    pub kind: &'static str,
    pub title: &'static str,
    pub color: Rgb,
    pub history: History,
}

impl Harvest {
    pub fn events(&self) -> &[Event] {
        match &self.history {
            History::Events(events) => events,
            History::Unsupported | History::Failed(_) => &[],
        }
    }
}

pub fn read_all(harnesses: &[Harness], since: DateTime<Utc>) -> Vec<Harvest> {
    crate::probe::each(harnesses, |harness| Harvest {
        id: harness.id.clone(),
        kind: harness.source.kind().name,
        title: harness.source.kind().title,
        color: harness.color(),
        history: crate::sources::events(&harness.source, since),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl Unit {
    fn parse(text: &str) -> Option<Unit> {
        Some(match text {
            "s" | "sec" | "secs" | "second" | "seconds" => Unit::Second,
            "m" | "min" | "mins" | "minute" | "minutes" => Unit::Minute,
            "h" | "hr" | "hrs" | "hour" | "hours" => Unit::Hour,
            "d" | "day" | "days" => Unit::Day,
            "w" | "wk" | "wks" | "week" | "weeks" => Unit::Week,
            "mo" | "mon" | "month" | "months" => Unit::Month,
            "y" | "yr" | "yrs" | "year" | "years" => Unit::Year,
            _ => return None,
        })
    }

    fn seconds(self) -> i64 {
        match self {
            Unit::Second => 1,
            Unit::Minute => 60,
            Unit::Hour => 3_600,
            Unit::Day => 86_400,
            Unit::Week => 7 * 86_400,
            Unit::Month => 30 * 86_400,
            Unit::Year => 365 * 86_400,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Unit::Second => "second",
            Unit::Minute => "minute",
            Unit::Hour => "hour",
            Unit::Day => "day",
            Unit::Week => "week",
            Unit::Month => "month",
            Unit::Year => "year",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Past {
    count: i64,
    unit: Unit,
}

impl Past {
    pub fn duration(self) -> Duration {
        Duration::seconds(self.count * self.unit.seconds())
    }

    pub fn label(self) -> String {
        let plural = if self.count == 1 { "" } else { "s" };
        format!("{} {}{plural}", self.count, self.unit.name())
    }
}

impl FromStr for Past {
    type Err = String;

    fn from_str(text: &str) -> Result<Past, String> {
        let text = text.trim().to_lowercase();
        let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let (number, unit) = text.split_at(digits);
        let unit = Unit::parse(unit.trim()).ok_or_else(|| {
            format!(
                "\"{text}\" is not a time span. Use a number and a unit, like 24h, 30d, or 2weeks"
            )
        })?;
        number
            .parse::<i64>()
            .ok()
            .filter(|count| *count > 0)
            .filter(|count| {
                count
                    .checked_mul(unit.seconds())
                    .and_then(Duration::try_seconds)
                    .is_some_and(|span| span < Duration::days(36_500))
            })
            .map(|count| Past { count, unit })
            .ok_or_else(|| {
                format!("\"{text}\" is not a time span. Use a whole number from 1 up to 100 years")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn past_reads_every_unit_spelling() {
        for (text, seconds, label) in [
            ("24h", 86_400, "24 hours"),
            ("10s", 10, "10 seconds"),
            ("1day", 86_400, "1 day"),
            ("2weeks", 14 * 86_400, "2 weeks"),
            ("3month", 90 * 86_400, "3 months"),
            ("5years", 5 * 365 * 86_400, "5 years"),
            ("90 m", 5_400, "90 minutes"),
            ("30D", 30 * 86_400, "30 days"),
            ("1mo", 30 * 86_400, "1 month"),
            ("2 HRS", 7_200, "2 hours"),
            ("1wk", 7 * 86_400, "1 week"),
            ("1y", 365 * 86_400, "1 year"),
            ("5 min", 300, "5 minutes"),
        ] {
            let past: Past = text.parse().unwrap();
            assert_eq!(past.duration(), Duration::seconds(seconds), "{text}");
            assert_eq!(past.label(), label);
        }
    }

    #[test]
    fn past_rejects_zero_negative_and_unknown_units() {
        for text in [
            "0d",
            "-1d",
            "d",
            "10",
            "10 fortnights",
            "",
            "1.5h",
            "999999999999y",
        ] {
            assert!(text.parse::<Past>().is_err(), "{text}");
        }
        assert!("3x".parse::<Past>().unwrap_err().contains("24h, 30d"));
    }

    #[test]
    fn tokens_add_by_type() {
        let mut tokens = Tokens {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
        };
        tokens += tokens;
        assert_eq!(tokens.total(), 20);
        assert_eq!(tokens.cache_write, 8);
    }
}
