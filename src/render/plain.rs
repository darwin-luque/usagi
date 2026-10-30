use chrono::{DateTime, Utc};

use crate::theme::{BarStyle, Rgb, Theme};
use crate::usage::{Outcome, Report};

use super::{Role, amount, bar_cells, countdown};

pub fn render(reports: &[Report], theme: &Theme, color: bool) -> String {
    render_at(reports, theme, color, Utc::now())
}

fn paint(text: &str, rgb: Option<Rgb>, modifier: &str, color: bool) -> String {
    if !color {
        return text.into();
    }
    let foreground = rgb
        .map(|Rgb(r, g, b)| format!("\x1b[38;2;{r};{g};{b}m"))
        .unwrap_or_default();
    format!("{foreground}{modifier}{text}\x1b[0m")
}

fn render_at(reports: &[Report], theme: &Theme, color: bool, now: DateTime<Utc>) -> String {
    reports
        .iter()
        .map(|report| {
            let mut lines = vec![paint(
                &format!("● {}", report.title),
                Some(report.color),
                "\x1b[1m",
                color,
            )];
            if report.id != report.kind {
                lines[0].push_str(&paint(&format!("  {}", report.id), None, "\x1b[2m", color));
            }
            match &report.outcome {
                Outcome::Usage(usage) => {
                    if let Some(plan) = &usage.plan {
                        lines[0].push_str(&paint(&format!("  {plan}"), None, "\x1b[2m", color));
                    }
                    let amount_width = usage
                        .windows
                        .iter()
                        .map(|window| amount(window.meter).len())
                        .max()
                        .unwrap_or(4)
                        .max(4);
                    for window in &usage.windows {
                        let pace = if theme.show_pace {
                            window.elapsed_percent(now)
                        } else {
                            None
                        };
                        let bar: String = bar_cells(window.meter.percent(), 28, theme.bar, pace)
                            .into_iter()
                            .map(|cell| {
                                let rgb = match cell.role {
                                    Role::Fill => report.color,
                                    Role::Track => theme.track,
                                    Role::Pace => Rgb(255, 255, 255),
                                };
                                let symbol = if !color
                                    && theme.bar == BarStyle::Blocks
                                    && cell.role == Role::Track
                                {
                                    '░'
                                } else {
                                    cell.symbol
                                };
                                paint(&symbol.to_string(), Some(rgb), "", color)
                            })
                            .collect();
                        let bar = if bar.is_empty() { " ".repeat(28) } else { bar };
                        let value = paint(
                            &format!("{:>amount_width$}", amount(window.meter)),
                            Some(theme.level_color(window.meter.percent().unwrap_or(0.0))),
                            "",
                            color,
                        );
                        let mut line = format!("  {:<12} {bar}  {value}", window.label());
                        let reset = countdown(window.reset, now);
                        if !reset.is_empty() {
                            line.push_str(&paint(&format!("  {reset}"), None, "\x1b[2m", color));
                        }
                        lines.push(line);
                    }
                    for note in &usage.notes {
                        lines.push(paint(&format!("  · {note}"), None, "\x1b[2m", color));
                    }
                }
                Outcome::Error(error) => lines.push(paint(
                    &format!("  ✗ {error}"),
                    Some(theme.danger),
                    "",
                    color,
                )),
            }
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::tests::{now, reports};

    #[test]
    fn plain_without_color_has_exact_rows() {
        let theme = Theme {
            show_pace: false,
            ..Theme::default()
        };
        assert_eq!(
            render_at(&reports(), &theme, false, now()),
            concat!(
                "● Claude Code  Pro\n",
                "  5h           ██████████████░░░░░░░░░░░░░░   50%  resets in 4h 47m\n",
                "  · Usage is current.\n\n",
                "● Codex  work\n",
                "  ✗ Network failed. Try again."
            )
        );
        assert_eq!(render_at(&[], &theme, false, now()), "");
    }

    #[test]
    fn plain_shows_pace_and_true_color() {
        let text = render_at(&reports(), &Theme::default(), true, now());
        assert!(text.contains("\x1b[38;2;217;119;87m\x1b[1m● Claude Code"));
        assert!(text.contains('│'));
        assert!(!render_at(&reports(), &Theme::default(), false, now()).contains('\x1b'));
    }

    #[test]
    fn plain_shows_spend_without_a_bar_and_rolling_without_pace() {
        use crate::usage::{Meter, Reset, Span, Window};

        let mut reports = reports();
        let Outcome::Usage(usage) = &mut reports[0].outcome else {
            unreachable!()
        };
        usage.plan = None;
        usage.notes.clear();
        usage.windows = vec![
            Window {
                span: Span::Month,
                scope: None,
                meter: Meter::Spend {
                    used_usd: 4.21,
                    limit_usd: None,
                },
                reset: Reset::Unknown,
            },
            Window {
                span: Span::Week,
                scope: Some("opus".into()),
                meter: Meter::Spend {
                    used_usd: 4.21,
                    limit_usd: Some(12.0),
                },
                reset: Reset::Rolling,
            },
        ];
        let theme = Theme {
            bar: BarStyle::Line,
            ..Theme::default()
        };
        let text = render_at(&reports[..1], &theme, false, now());
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            vec![
                "● Claude Code".to_string(),
                format!(
                    "  {:<12} {}  {:>15}",
                    "month",
                    " ".repeat(28),
                    "$4.21 spent"
                ),
                format!(
                    "  {:<12} {}  $4.21 of $12.00  rolling",
                    "week opus",
                    format!("{}{}", "━".repeat(10), "─".repeat(18))
                ),
            ]
        );
        assert!(!text.contains('│'));
    }

    #[test]
    #[ignore]
    fn print_plain_demo() {
        println!("{}", render(&reports(), &Theme::default(), true));
    }
}
