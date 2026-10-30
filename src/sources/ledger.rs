use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};

use crate::config::Limits;
use crate::usage::{Meter, Reset, Span, Window};

pub struct Spend {
    pub at: DateTime<Utc>,
    pub usd: f64,
}

pub const HORIZON: Duration = Duration::days(30);

pub fn windows(entries: &[Spend], limits: &Limits, now: DateTime<Utc>) -> Vec<Window> {
    [
        (Span::FiveHours, limits.five_hours),
        (Span::Day, limits.day),
        (Span::Week, limits.week),
        (Span::Month, limits.month),
    ]
    .into_iter()
    .filter(|(_, limit)| limits.is_empty() || limit.is_some())
    .map(|(span, limit_usd)| Window {
        span,
        scope: None,
        meter: Meter::Spend {
            used_usd: entries
                .iter()
                .filter(|entry| entry.at > now - span.length() && entry.at <= now)
                .map(|entry| entry.usd)
                .fold(0.0, |sum, usd| sum + usd),
            limit_usd,
        },
        reset: Reset::Rolling,
    })
    .collect()
}

pub fn default_data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::home().join(".local/share"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_windows_exclude_start_and_future_entries() {
        let now = Utc::now();
        let entries: Vec<_> = [
            (-31 * 24, 100.0),
            (-30 * 24, 32.0),
            (-7 * 24, 16.0),
            (-24, 8.0),
            (-5, 4.0),
            (-1, 2.0),
            (0, 1.0),
            (1, 100.0),
        ]
        .into_iter()
        .map(|(hours, usd)| Spend {
            at: now + Duration::hours(hours),
            usd,
        })
        .collect();
        let result = windows(&entries, &Limits::default(), now);
        assert_eq!(
            result.iter().map(|w| w.span).collect::<Vec<_>>(),
            vec![Span::FiveHours, Span::Day, Span::Week, Span::Month]
        );
        for (window, expected) in result.iter().zip([3.0, 7.0, 15.0, 31.0]) {
            assert_eq!(
                window.meter,
                Meter::Spend {
                    used_usd: expected,
                    limit_usd: None
                }
            );
            assert_eq!(window.reset, Reset::Rolling);
        }
    }

    #[test]
    fn configured_limits_select_windows() {
        let result = windows(&[], &Limits::opencode_go(), Utc::now());
        assert_eq!(
            result.iter().map(|w| w.span).collect::<Vec<_>>(),
            vec![Span::FiveHours, Span::Week, Span::Month]
        );
        assert_eq!(
            result[0].meter,
            Meter::Spend {
                used_usd: 0.0,
                limit_usd: Some(12.0)
            }
        );
    }
}
