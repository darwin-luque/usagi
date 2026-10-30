use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};

use crate::config::{Config, Harness};
use crate::theme::{Rgb, Theme};
use crate::usage::{Outcome, Report};

use super::{Role, amount, bar_cells, detail};

struct State {
    harnesses: Vec<Harness>,
    reports: BTreeMap<String, Report>,
    last_update: Option<DateTime<Utc>>,
    refreshing: bool,
    refresh_seconds: u64,
}

enum Control {
    Refresh,
    Stop,
}

enum Update {
    Started,
    Reports(Vec<Report>),
}

fn worker(
    harnesses: Vec<Harness>,
    interval: Duration,
    control: Receiver<Control>,
    updates: Sender<Update>,
) {
    loop {
        if updates.send(Update::Started).is_err() {
            break;
        }
        let reports = crate::probe::probe_all(&harnesses);
        if updates.send(Update::Reports(reports)).is_err() {
            break;
        }
        match control.recv_timeout(interval) {
            Ok(Control::Refresh) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

pub fn run(config: &Config, harnesses: Vec<Harness>) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let (control_tx, control_rx) = mpsc::channel();
    let (updates_tx, updates_rx) = mpsc::channel();
    let mut state = State {
        harnesses: harnesses.clone(),
        reports: BTreeMap::new(),
        last_update: None,
        refreshing: true,
        refresh_seconds: config.refresh_seconds,
    };
    let interval = Duration::from_secs(config.refresh_seconds.max(1));
    std::thread::spawn(move || worker(harnesses, interval, control_rx, updates_tx));
    let result = (|| -> anyhow::Result<()> {
        loop {
            for update in updates_rx.try_iter() {
                match update {
                    Update::Started => state.refreshing = true,
                    Update::Reports(reports) => {
                        state.reports = reports
                            .into_iter()
                            .map(|report| (report.id.clone(), report))
                            .collect();
                        state.last_update = Some(Utc::now());
                        state.refreshing = false;
                    }
                }
            }
            terminal.draw(|frame| draw(frame, &state, &config.theme, Utc::now()))?;
            if event::poll(Duration::from_millis(250))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char('r') if !state.refreshing => {
                        control_tx.send(Control::Refresh)?;
                        state.refreshing = true;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    let _ = control_tx.send(Control::Stop);
    ratatui::restore();
    result
}

fn color(Rgb(r, g, b): Rgb) -> Color {
    Color::Rgb(r, g, b)
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn window_line(
    window: &crate::usage::Window,
    report: &Report,
    theme: &Theme,
    width: u16,
    columns: (usize, usize),
    now: DateTime<Utc>,
) -> Line<'static> {
    let (value_width, reset_width) = columns;
    let label = format!("{:<12}", window.label());
    let value = format!("{:>value_width$}", amount(window.meter));
    let reset = detail(window, now);
    let reset = if reset_width == 0 {
        reset
    } else {
        format!("  {reset:<reset_width$}")
    };
    let fixed_width = label.chars().count() + value.chars().count() + reset.chars().count() + 3;
    let bar_width = usize::from(width).saturating_sub(fixed_width);
    let pace = if theme.show_pace {
        window.elapsed_percent(now)
    } else {
        None
    };
    let mut spans = vec![Span::raw(format!("{label} "))];
    if window.meter.percent().is_none() {
        spans.push(Span::raw(" ".repeat(bar_width)));
    }
    for cell in bar_cells(window.meter.percent(), bar_width, theme.bar, pace) {
        let foreground = match cell.role {
            Role::Fill => color(report.color),
            Role::Track => color(theme.track),
            Role::Pace => Color::White,
        };
        spans.push(Span::styled(
            cell.symbol.to_string(),
            Style::default().fg(foreground),
        ));
    }
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        value,
        Style::default().fg(color(
            theme.level_color(window.meter.percent().unwrap_or(0.0)),
        )),
    ));
    spans.push(Span::styled(reset, dim()));
    Line::from(spans)
}

fn card_lines(
    report: Option<&Report>,
    theme: &Theme,
    width: u16,
    now: DateTime<Utc>,
) -> Vec<Line<'static>> {
    match report.map(|report| (&report.outcome, report)) {
        None => vec![Line::styled("Loading…", dim())],
        Some((Outcome::Error(error), _)) => vec![Line::styled(
            format!("✗ {error}"),
            Style::default().fg(color(theme.danger)),
        )],
        Some((Outcome::Usage(usage), report)) => {
            let widest = |text: &dyn Fn(&crate::usage::Window) -> String| {
                usage
                    .windows
                    .iter()
                    .map(|window| text(window).chars().count())
                    .max()
                    .unwrap_or(0)
            };
            let columns = (
                widest(&|window| amount(window.meter)).max(4),
                widest(&|window| detail(window, now)),
            );
            let mut lines: Vec<_> = usage
                .windows
                .iter()
                .map(|window| window_line(window, report, theme, width, columns, now))
                .collect();
            lines.extend(
                usage
                    .notes
                    .iter()
                    .map(|note| Line::styled(format!("· {note}"), dim())),
            );
            lines
        }
    }
}

fn draw(frame: &mut Frame, state: &State, theme: &Theme, now: DateTime<Utc>) {
    let area = frame.area();
    let updated = state
        .last_update
        .map(|at| format!("updated {}", at.with_timezone(&Local).format("%H:%M:%S")))
        .unwrap_or_else(|| "loading".into());
    let progress = if state.refreshing && state.last_update.is_some() {
        " · refreshing"
    } else {
        ""
    };
    let header = Line::from(vec![
        Span::styled("usagi", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!(
                "  live · every {}s · {updated}{progress} · r refresh · q quit",
                state.refresh_seconds
            ),
            dim(),
        ),
    ]);
    frame.render_widget(
        header,
        Rect::new(area.x, area.y, area.width, area.height.min(1)),
    );
    let columns = if area.width < 140 { 1 } else { 2 };
    let gap = if columns == 2 { 2 } else { 0 };
    let card_width = area.width.saturating_sub(gap) / columns;
    let bottom = area.bottom();
    let mut y = area.y.saturating_add(2);
    for row in state.harnesses.chunks(usize::from(columns)) {
        let mut cards = Vec::new();
        let mut height = 3;
        for harness in row {
            let report = state.reports.get(&harness.id);
            let inner_width = card_width.saturating_sub(4);
            let lines = card_lines(report, theme, inner_width, now);
            let content_height: usize = lines
                .iter()
                .map(|line| {
                    line.width()
                        .max(1)
                        .div_ceil(usize::from(inner_width).max(1))
                })
                .sum();
            height = height.max(content_height.saturating_add(2).min(u16::MAX as usize) as u16);
            cards.push((harness, report, lines));
        }
        if y >= bottom {
            break;
        }
        height = height.min(bottom - y);
        for (column, (harness, report, lines)) in cards.into_iter().enumerate() {
            let kind = harness.source.kind();
            let title = report
                .map(|report| report.title.as_str())
                .unwrap_or(kind.title);
            let mut title = format!(" ● {title}");
            if harness.id != kind.name {
                title.push_str(&format!(" · {}", harness.id));
            }
            if let Some(Report {
                outcome: Outcome::Usage(usage),
                ..
            }) = report
                && let Some(plan) = &usage.plan
            {
                title.push_str(&format!(" · {plan}"));
            }
            title.push(' ');
            let rgb = report
                .map(|report| report.color)
                .unwrap_or_else(|| harness.color());
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(color(rgb)))
                .title(Line::styled(
                    title,
                    Style::default().fg(color(rgb)).add_modifier(Modifier::BOLD),
                ))
                .padding(ratatui::widgets::Padding::horizontal(1));
            let rect = Rect::new(
                area.x + column as u16 * (card_width + gap),
                y,
                card_width,
                height,
            );
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .block(block),
                rect,
            );
        }
        y = y.saturating_add(height).saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Source;
    use crate::render::tests::{now, reports};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn state(loaded: bool) -> State {
        let mut codex = Harness::new(Source::Codex { home: None });
        codex.id = "work".into();
        State {
            harnesses: vec![Harness::new(Source::Claude { credentials: None }), codex],
            reports: if loaded {
                reports()
                    .into_iter()
                    .map(|report| (report.id.clone(), report))
                    .collect()
            } else {
                BTreeMap::new()
            },
            last_update: loaded.then(now),
            refreshing: !loaded,
            refresh_seconds: 60,
        }
    }

    fn screen(width: u16, loaded: bool) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal
            .draw(|frame| draw(frame, &state(loaded), &Theme::default(), now()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn live_draws_titles_percent_and_error_in_both_layouts() {
        for width in [80, 120] {
            let screen = screen(width, true);
            for text in [
                "Claude Code · Pro",
                "Codex · work",
                "50%",
                "Network failed. Try again.",
                "resets in 4h 47m",
                "r refresh · q quit",
            ] {
                assert!(screen.contains(text), "Missing {text} at {width} columns");
            }
        }
    }

    #[test]
    fn live_shows_loading_before_the_first_update() {
        let screen = screen(80, false);
        assert!(screen.contains("loading"));
        assert_eq!(screen.matches("Loading…").count(), 2);
    }

    #[test]
    fn live_worker_refreshes_at_once_and_stops() {
        let (control_tx, control_rx) = mpsc::channel();
        let (updates_tx, updates_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            worker(
                Vec::new(),
                Duration::from_secs(3600),
                control_rx,
                updates_tx,
            );
        });
        for refresh in [false, true] {
            if refresh {
                control_tx.send(Control::Refresh).unwrap();
            }
            assert!(matches!(
                updates_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                Update::Started
            ));
            assert!(
                matches!(updates_rx.recv_timeout(Duration::from_secs(2)).unwrap(), Update::Reports(reports) if reports.is_empty())
            );
        }
        control_tx.send(Control::Stop).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn live_handles_small_terminal_areas() {
        for width in [0, 1, 12, 40] {
            for height in [0, 1, 3, 10] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| draw(frame, &state(true), &Theme::default(), now()))
                    .unwrap();
            }
        }
    }
}
