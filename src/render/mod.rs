pub mod breakdown;
pub mod json;
pub mod live;
pub mod plain;
pub mod raw;

use chrono::{DateTime, Utc};

use crate::theme::{BarStyle, Rgb};
use crate::usage::{Meter, Reset, Window};

pub fn paint(text: &str, rgb: Option<Rgb>, modifier: &str, color: bool) -> String {
    if !color {
        return text.into();
    }
    let foreground = rgb
        .map(|Rgb(r, g, b)| format!("\x1b[38;2;{r};{g};{b}m"))
        .unwrap_or_default();
    format!("{foreground}{modifier}{text}\x1b[0m")
}

pub fn countdown(reset: Reset, now: DateTime<Utc>) -> String {
    match reset {
        Reset::Rolling => "rolling".into(),
        Reset::Unknown => String::new(),
        Reset::At(at) => {
            let seconds = (at - now).num_seconds();
            if seconds <= 0 {
                return "resets now".into();
            }
            let minutes = (seconds + 59) / 60;
            if minutes >= 1440 {
                format!("resets in {}d {}h", minutes / 1440, minutes % 1440 / 60)
            } else if minutes >= 60 {
                format!("resets in {}h {}m", minutes / 60, minutes % 60)
            } else {
                format!("resets in {minutes}m")
            }
        }
    }
}

pub fn amount(meter: Meter) -> String {
    match (meter.percent(), meter) {
        (Some(percent), _) => format!("{percent:.0}%"),
        (None, Meter::Spend { used_usd, .. }) => format!("${used_usd:.2}"),
        (None, Meter::Percent { .. }) => String::new(),
    }
}

pub fn detail(window: &Window, now: DateTime<Utc>) -> String {
    let reset = countdown(window.reset, now);
    let spend = match window.meter {
        Meter::Spend {
            used_usd,
            limit_usd: Some(limit),
        } => format!("${used_usd:.2} of ${limit:.2}"),
        Meter::Spend {
            limit_usd: None, ..
        } => "spent".into(),
        Meter::Percent { .. } => return reset,
    };
    if reset.is_empty() {
        spend
    } else {
        format!("{spend} · {reset}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Fill,
    Track,
    Pace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarCell {
    pub symbol: char,
    pub role: Role,
}

pub fn bar_cells(
    percent: Option<f64>,
    width: usize,
    style: BarStyle,
    pace: Option<f64>,
) -> Vec<BarCell> {
    let Some(percent) = percent else {
        return Vec::new();
    };
    let filled = percent.clamp(0.0, 100.0) / 100.0 * width as f64;
    let marker = pace.filter(|value| value.is_finite()).map(|value| {
        ((value.clamp(0.0, 100.0) / 100.0 * width as f64) as usize).min(width.saturating_sub(1))
    });
    (0..width)
        .map(|index| {
            if marker == Some(index) {
                return BarCell {
                    symbol: '│',
                    role: Role::Pace,
                };
            }
            let remainder = (filled - index as f64).clamp(0.0, 1.0);
            let (symbol, fill) = match style {
                BarStyle::Blocks => {
                    let eighths = (remainder * 8.0).floor() as usize;
                    (
                        ['█', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'][eighths],
                        eighths > 0,
                    )
                }
                BarStyle::Line => (if remainder >= 0.5 { '━' } else { '─' }, remainder >= 0.5),
                BarStyle::Dots => (if remainder >= 0.5 { '●' } else { '·' }, remainder >= 0.5),
            };
            BarCell {
                symbol,
                role: if fill { Role::Fill } else { Role::Track },
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::theme::Rgb;
    use crate::usage::{Outcome, ProbeError, Report, Span, Usage, Window};
    use chrono::Duration;

    pub fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-30T00:00:00Z")
            .unwrap()
            .to_utc()
    }

    pub fn reports() -> Vec<Report> {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/claude_usage.json")).unwrap();
        vec![
            Report {
                id: "claude".into(),
                kind: "claude",
                title: "Claude Code".into(),
                color: Rgb(217, 119, 87),
                fetched_at: now(),
                outcome: Outcome::Usage(Usage {
                    plan: Some("Pro".into()),
                    windows: vec![Window {
                        span: Span::FiveHours,
                        scope: None,
                        meter: Meter::Percent {
                            used: fixture["five_hour"]["utilization"].as_f64().unwrap(),
                        },
                        reset: Reset::At(now() + Duration::hours(4) + Duration::minutes(47)),
                    }],
                    notes: vec!["Usage is current.".into()],
                }),
            },
            Report {
                id: "work".into(),
                kind: "codex",
                title: "Codex".into(),
                color: Rgb(16, 163, 127),
                fetched_at: now(),
                outcome: Outcome::Error(ProbeError::Network("Try again.".into())),
            },
        ]
    }

    #[test]
    fn countdown_shows_days_hours_minutes_and_reset_states() {
        for (duration, expected) in [
            (Duration::hours(51), "resets in 2d 3h"),
            (Duration::minutes(287), "resets in 4h 47m"),
            (Duration::minutes(12), "resets in 12m"),
            (Duration::seconds(1), "resets in 1m"),
            (Duration::zero(), "resets now"),
            (Duration::seconds(-1), "resets now"),
        ] {
            assert_eq!(countdown(Reset::At(now() + duration), now()), expected);
        }
        assert_eq!(countdown(Reset::Rolling, now()), "rolling");
        assert_eq!(countdown(Reset::Unknown, now()), "");
    }

    #[test]
    fn amount_shows_percent_and_detail_shows_dollars() {
        let window = |meter, reset| Window {
            span: crate::usage::Span::Week,
            scope: None,
            meter,
            reset,
        };
        assert_eq!(amount(Meter::Percent { used: 22.4 }), "22%");
        assert_eq!(amount(Meter::Percent { used: 22.6 }), "23%");
        let limited = Meter::Spend {
            used_usd: 4.21,
            limit_usd: Some(12.0),
        };
        let open = Meter::Spend {
            used_usd: 4.21,
            limit_usd: None,
        };
        assert_eq!(amount(limited), "35%");
        assert_eq!(amount(open), "$4.21");
        assert_eq!(
            detail(&window(limited, Reset::Rolling), now()),
            "$4.21 of $12.00 · rolling"
        );
        assert_eq!(detail(&window(open, Reset::Unknown), now()), "spent");
        assert_eq!(
            detail(&window(Meter::Percent { used: 1.0 }, Reset::Rolling), now()),
            "rolling"
        );
    }

    #[test]
    fn bars_show_each_style_partial_blocks_and_pace() {
        let symbols = |style| {
            bar_cells(Some(50.0), 4, style, None)
                .iter()
                .map(|cell| cell.symbol)
                .collect::<String>()
        };
        assert_eq!(symbols(BarStyle::Blocks), "████");
        assert_eq!(symbols(BarStyle::Line), "━━──");
        assert_eq!(symbols(BarStyle::Dots), "●●··");
        for eighth in 1..8 {
            let cells = bar_cells(Some(eighth as f64 * 12.5), 1, BarStyle::Blocks, None);
            assert_eq!(
                cells[0].symbol,
                ['▏', '▎', '▍', '▌', '▋', '▊', '▉'][eighth - 1]
            );
            assert_eq!(cells[0].role, Role::Fill);
        }
        let cells = bar_cells(Some(50.0), 4, BarStyle::Blocks, Some(75.0));
        assert_eq!(
            cells.iter().map(|cell| cell.role).collect::<Vec<_>>(),
            [Role::Fill, Role::Fill, Role::Track, Role::Pace]
        );
        assert_eq!(cells[3].symbol, '│');
        assert_eq!(
            bar_cells(Some(0.0), 4, BarStyle::Line, Some(100.0))[3].role,
            Role::Pace
        );
    }

    #[test]
    fn bars_handle_missing_zero_and_excess_usage() {
        assert!(bar_cells(None, 28, BarStyle::Blocks, Some(10.0)).is_empty());
        assert!(bar_cells(Some(50.0), 0, BarStyle::Blocks, None).is_empty());
        assert!(
            bar_cells(Some(120.0), 4, BarStyle::Blocks, None)
                .iter()
                .all(|cell| cell.role == Role::Fill)
        );
        assert!(
            bar_cells(Some(-10.0), 4, BarStyle::Blocks, None)
                .iter()
                .all(|cell| cell.role == Role::Track)
        );
    }
}
