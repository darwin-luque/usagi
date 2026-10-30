use serde::Serialize;

use crate::breakdown::{Amount, Breakdown, Bucket, Metric, Row, Step, View};
use crate::history::{Harvest, History, Tokens};
use crate::theme::{BarStyle, Rgb};

use super::{Role, bar_cells, paint};

const KINDS: [(&str, Rgb, char); 4] = [
    ("input", Rgb(0x61, 0xAF, 0xEF), '█'),
    ("output", Rgb(0xC6, 0x78, 0xDD), '▓'),
    ("cache read", Rgb(0x98, 0xC3, 0x79), '▒'),
    ("cache write", Rgb(0xE5, 0xC0, 0x7B), '░'),
];
const CHART_ROWS: usize = 8;
const ROW_BAR: usize = 24;
const LABEL_LIMIT: usize = 32;

fn kinds(tokens: Tokens) -> [u64; 4] {
    [
        tokens.input,
        tokens.output,
        tokens.cache_read,
        tokens.cache_write,
    ]
}

pub fn token_count(count: u64) -> String {
    let count = count as f64;
    for (size, suffix) in [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")] {
        if count >= size {
            let scaled = count / size;
            return if scaled >= 100.0 {
                format!("{scaled:.0}{suffix}")
            } else {
                format!("{scaled:.1}{suffix}")
            };
        }
    }
    format!("{count}")
}

pub fn dollars(usd: f64) -> String {
    format!("${usd:.2}")
}

fn metric_text(metric: Metric, value: f64) -> String {
    match metric {
        Metric::Tokens => token_count(value as u64),
        Metric::Cost => dollars(value),
    }
}

fn percent(part: f64, whole: f64) -> String {
    let share = part / whole * 100.0;
    if whole <= 0.0 || share <= 0.0 {
        "0%".into()
    } else if share < 0.1 {
        "<0.1%".into()
    } else if share < 10.0 {
        format!("{share:.1}%")
    } else {
        format!("{share:.0}%")
    }
}

fn list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).into(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

fn shorten(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.into();
    }
    let mut short: String = text.chars().take(width.saturating_sub(1)).collect();
    short.push('…');
    short
}

pub fn render(breakdown: &Breakdown, color: bool, width: usize) -> String {
    let dim = |text: &str| paint(text, None, "\x1b[2m", color);
    let metric = breakdown.metric;
    let mut lines = vec![
        format!(
            "{}  {}  {}",
            paint("Breakdown", None, "\x1b[1m", color),
            format_args!("last {}", breakdown.past.label()),
            dim(match metric {
                Metric::Tokens => "tokens",
                Metric::Cost => "cost",
            })
        ),
        dim(&format!(
            "{} to {}",
            breakdown
                .since
                .with_timezone(&chrono::Local)
                .format("%b %d %Y %H:%M"),
            breakdown
                .until
                .with_timezone(&chrono::Local)
                .format("%b %d %Y %H:%M")
        )),
        String::new(),
    ];
    let total = breakdown.total;
    let tokens_text = format!("{} tokens", token_count(total.tokens.total()));
    let cost_text = dollars(total.cost_usd);
    let (main, second) = match metric {
        Metric::Tokens => (tokens_text, breakdown.shows_cost().then_some(cost_text)),
        Metric::Cost => (cost_text, Some(tokens_text)),
    };
    let mut headline = format!("  {}", paint(&main, None, "\x1b[1m", color));
    if let Some(second) = second {
        headline.push_str(&format!("   {}", dim(&second)));
    }
    lines.push(headline);
    lines.push(String::new());
    lines.extend(token_types(total.tokens, color, width));
    lines.push(String::new());
    match &breakdown.view {
        View::Ranked(rows) => {
            lines.push(format!(
                "  {}",
                dim(&format!("By {}", grouping_name(breakdown)))
            ));
            lines.extend(ranked(rows, metric, &breakdown.total, color));
        }
        View::Timeline { step, buckets } => {
            lines.push(format!("  {}", dim(&format!("By {}", step_name(*step)))));
            lines.extend(chart(
                &breakdown.harvests,
                buckets,
                *step,
                metric,
                color,
                width,
            ));
        }
    }
    let notes = notes(breakdown);
    if !notes.is_empty() {
        lines.push(String::new());
        lines.extend(notes.iter().map(|note| dim(&format!("  · {note}"))));
    }
    lines.join("\n")
}

fn grouping_name(breakdown: &Breakdown) -> &'static str {
    match breakdown.grouping {
        crate::breakdown::Grouping::Model => "model",
        crate::breakdown::Grouping::Harness => "harness",
        crate::breakdown::Grouping::Day => "day",
    }
}

fn step_name(step: Step) -> &'static str {
    match step {
        Step::Hour => "hour",
        Step::Day => "day",
        Step::Week => "week",
        Step::Month => "month",
    }
}

fn token_types(tokens: Tokens, color: bool, width: usize) -> Vec<String> {
    let counts = kinds(tokens);
    let total = tokens.total();
    let bar_width = width.saturating_sub(4).clamp(10, 60);
    let mut bar = String::new();
    let mut start = 0;
    let mut sum = 0;
    for ((_, rgb, glyph), count) in KINDS.iter().zip(counts) {
        sum += count;
        let end = if total == 0 {
            0
        } else {
            (sum as f64 / total as f64 * bar_width as f64).round() as usize
        };
        let symbol = if color { '█' } else { *glyph };
        bar.push_str(&paint(
            &symbol.to_string().repeat(end - start),
            Some(*rgb),
            "",
            color,
        ));
        start = end;
    }
    if total == 0 {
        bar = paint(&"·".repeat(bar_width), None, "\x1b[2m", color);
    }
    let legend = KINDS
        .iter()
        .zip(counts)
        .map(|((name, rgb, glyph), count)| {
            let symbol = if color { '█' } else { *glyph };
            format!(
                "{} {name} {} {}",
                paint(&symbol.to_string(), Some(*rgb), "", color),
                token_count(count),
                paint(&percent(count as f64, total as f64), None, "\x1b[2m", color)
            )
        })
        .collect::<Vec<_>>()
        .join("   ");
    vec![format!("  {bar}"), format!("  {legend}")]
}

fn ranked(rows: &[Row], metric: Metric, total: &Amount, color: bool) -> Vec<String> {
    if rows.is_empty() {
        return vec![paint("  No usage in this time.", None, "\x1b[2m", color)];
    }
    let top = rows[0].amount.value(metric);
    let whole = total.value(metric);
    let label_width = rows
        .iter()
        .map(|row| row.label.chars().count())
        .max()
        .unwrap_or(0)
        .min(LABEL_LIMIT);
    let dots_width = rows.iter().map(|row| row.colors.len()).max().unwrap_or(0);
    let amounts: Vec<_> = rows
        .iter()
        .map(|row| metric_text(metric, row.amount.value(metric)))
        .collect();
    let amount_width = amounts.iter().map(String::len).max().unwrap_or(0);
    rows.iter()
        .zip(amounts)
        .map(|(row, amount)| {
            let value = row.amount.value(metric);
            let dots: String = row
                .colors
                .iter()
                .map(|rgb| paint("●", Some(*rgb), "", color))
                .collect();
            let fill = row.colors.first().copied().unwrap_or(Rgb(0xAB, 0xB2, 0xBF));
            let share = if top > 0.0 { value / top * 100.0 } else { 0.0 };
            let bar: String = bar_cells(Some(share), ROW_BAR, BarStyle::Blocks, None)
                .into_iter()
                .map(|cell| match cell.role {
                    Role::Fill => paint(&cell.symbol.to_string(), Some(fill), "", color),
                    Role::Track | Role::Pace => " ".into(),
                })
                .collect();
            format!(
                "  {:<label_width$}  {dots}{}  {bar}  {amount:>amount_width$}  {}",
                shorten(&row.label, label_width),
                " ".repeat(dots_width - row.colors.len()),
                paint(
                    &format!("{:>5}", percent(value, whole)),
                    None,
                    "\x1b[2m",
                    color
                )
            )
        })
        .collect()
}

const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

fn chart(
    harvests: &[Harvest],
    buckets: &[Bucket],
    step: Step,
    metric: Metric,
    color: bool,
    width: usize,
) -> Vec<String> {
    let columns = merge(buckets, width);
    let values: Vec<Vec<f64>> = columns
        .iter()
        .map(|(_, amounts)| amounts.iter().map(|amount| amount.value(metric)).collect())
        .collect();
    let peak = values
        .iter()
        .map(|column| column.iter().sum::<f64>())
        .fold(0.0, f64::max);
    let top_label = metric_text(metric, peak);
    let axis_width = top_label.chars().count().max(1);
    let gap = if columns.len() * 2 <= available(width, axis_width) {
        " "
    } else {
        ""
    };
    let units = (CHART_ROWS * 8) as f64;
    let stacks: Vec<Vec<usize>> = values
        .iter()
        .map(|column| {
            let mut sum = 0.0;
            column
                .iter()
                .map(|value| {
                    sum += value;
                    if peak <= 0.0 {
                        0
                    } else if sum > 0.0 {
                        ((sum / peak * units).round() as usize).max(1)
                    } else {
                        0
                    }
                })
                .collect()
        })
        .collect();
    let mut lines = Vec::new();
    for row in (0..CHART_ROWS).rev() {
        let axis = match row {
            r if r == CHART_ROWS - 1 => top_label.clone(),
            0 => "0".into(),
            _ => String::new(),
        };
        let cells: String = stacks
            .iter()
            .map(|stack| {
                let top = stack.last().copied().unwrap_or(0);
                let low = row * 8;
                let fill = top.saturating_sub(low).min(8);
                if fill == 0 {
                    return format!(" {gap}");
                }
                let unit = (low + fill) - 1;
                let owner = stack.iter().position(|bound| *bound > unit).unwrap_or(0);
                format!(
                    "{}{gap}",
                    paint(
                        &EIGHTHS[fill].to_string(),
                        harvests.get(owner).map(|harvest| harvest.color),
                        "",
                        color
                    )
                )
            })
            .collect();
        lines.push(
            format!(
                "  {} │{}",
                paint(&format!("{axis:>axis_width$}"), None, "\x1b[2m", color),
                cells
            )
            .trim_end()
            .to_string(),
        );
    }
    let span = columns.len() * (1 + gap.len());
    lines.push(format!(
        "  {} └{}",
        " ".repeat(axis_width),
        "─".repeat(span)
    ));
    let labels = axis_labels(
        &columns
            .iter()
            .map(|(start, _)| step.short_label(*start))
            .collect::<Vec<_>>(),
        1 + gap.len(),
        span,
    );
    lines.push(paint(
        &format!("  {}  {labels}", " ".repeat(axis_width)),
        None,
        "\x1b[2m",
        color,
    ));
    let legend: Vec<_> = harvests
        .iter()
        .filter(|harvest| !harvest.events().is_empty())
        .map(|harvest| {
            format!(
                "{} {}",
                paint("●", Some(harvest.color), "", color),
                harvest.id
            )
        })
        .collect();
    if !legend.is_empty() {
        lines.push(format!("  {}", legend.join("   ")));
    }
    lines
}

fn available(width: usize, axis_width: usize) -> usize {
    width.saturating_sub(axis_width + 6).max(8)
}

fn merge(buckets: &[Bucket], width: usize) -> Vec<(chrono::NaiveDateTime, Vec<Amount>)> {
    let room = available(width, 8);
    let size = buckets.len().div_ceil(room).max(1);
    buckets
        .chunks(size)
        .map(|chunk| {
            let mut amounts = chunk[0].amounts.clone();
            for bucket in &chunk[1..] {
                for (sum, amount) in amounts.iter_mut().zip(&bucket.amounts) {
                    *sum += *amount;
                }
            }
            (chunk[0].start, amounts)
        })
        .collect()
}

fn axis_labels(labels: &[String], cell: usize, span: usize) -> String {
    let mut line = vec![' '; span];
    let mut place = |text: &str, at: usize| {
        let chars: Vec<char> = text.chars().collect();
        let start = at.min(span.saturating_sub(chars.len()));
        let end = start + chars.len();
        let free = line[start.saturating_sub(1)..(end + 1).min(span)]
            .iter()
            .all(|c| *c == ' ');
        if end <= span && free {
            line[start..end].copy_from_slice(&chars);
        }
    };
    if let Some(first) = labels.first() {
        place(first, 0);
    }
    if let Some(last) = labels.last().filter(|_| labels.len() > 1) {
        place(last, span);
    }
    if labels.len() > 2 {
        let middle = &labels[labels.len() / 2];
        let at = (labels.len() / 2 * cell).saturating_sub(middle.chars().count() / 2);
        place(middle, at);
    }
    line.into_iter().collect::<String>().trim_end().to_string()
}

fn notes(breakdown: &Breakdown) -> Vec<String> {
    let mut notes = Vec::new();
    let unsupported: Vec<_> = breakdown
        .harvests
        .iter()
        .filter(|harvest| matches!(harvest.history, History::Unsupported))
        .map(|harvest| harvest.id.as_str())
        .collect();
    if !unsupported.is_empty() {
        notes.push(format!("No token history for {}.", list(&unsupported)));
    }
    for harvest in &breakdown.harvests {
        if let History::Failed(error) = &harvest.history {
            notes.push(format!("{} failed. {error}", harvest.id));
        }
    }
    if !breakdown.shows_cost() {
        notes.push("Use --type cost to see the cost.".into());
        return notes;
    }
    if breakdown.total.unpriced_tokens > 0 {
        notes.push(format!(
            "{} tokens have no price and add no cost.",
            token_count(breakdown.total.unpriced_tokens)
        ));
    }
    let titles = |recorded: bool| {
        let mut titles: Vec<&str> = Vec::new();
        for harvest in &breakdown.harvests {
            if harvest
                .events()
                .iter()
                .any(|event| event.cost_usd.is_some() == recorded)
                && !titles.contains(&harvest.title)
            {
                titles.push(harvest.title);
            }
        }
        titles
    };
    let recorded = titles(true);
    match recorded.as_slice() {
        [] => {}
        [title] => notes.push(format!("Cost for {title} is what {title} reports.")),
        many => notes.push(format!(
            "Cost for {} is what each harness reports.",
            list(many)
        )),
    }
    notes
}

#[derive(Serialize)]
struct TokensJson {
    #[serde(flatten)]
    tokens: Tokens,
    total: u64,
}

impl From<Tokens> for TokensJson {
    fn from(tokens: Tokens) -> TokensJson {
        TokensJson {
            tokens,
            total: tokens.total(),
        }
    }
}

#[derive(Serialize)]
struct TotalJson {
    tokens: TokensJson,
    cost_usd: Option<f64>,
    unpriced_tokens: u64,
}

#[derive(Serialize)]
struct RowJson {
    label: String,
    tokens: TokensJson,
    cost_usd: Option<f64>,
}

#[derive(Serialize)]
struct HarnessJson<'a> {
    id: &'a str,
    kind: &'a str,
    status: &'static str,
    events: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct BreakdownJson<'a> {
    past: String,
    since: String,
    until: String,
    #[serde(rename = "type")]
    metric: Metric,
    by: crate::breakdown::Grouping,
    #[serde(skip_serializing_if = "Option::is_none")]
    step: Option<Step>,
    total: TotalJson,
    rows: Vec<RowJson>,
    harnesses: Vec<HarnessJson<'a>>,
}

pub fn json(breakdown: &Breakdown) -> String {
    let cost = |amount: &Amount| breakdown.shows_cost().then_some(amount.cost_usd);
    let value = BreakdownJson {
        past: breakdown.past.label(),
        since: breakdown.since.to_rfc3339(),
        until: breakdown.until.to_rfc3339(),
        metric: breakdown.metric,
        by: breakdown.grouping,
        step: match &breakdown.view {
            View::Timeline { step, .. } => Some(*step),
            View::Ranked(_) => None,
        },
        total: TotalJson {
            tokens: breakdown.total.tokens.into(),
            cost_usd: cost(&breakdown.total),
            unpriced_tokens: breakdown.total.unpriced_tokens,
        },
        rows: breakdown
            .view
            .rows()
            .into_iter()
            .map(|row| RowJson {
                label: row.label,
                tokens: row.amount.tokens.into(),
                cost_usd: cost(&row.amount),
            })
            .collect(),
        harnesses: breakdown
            .harvests
            .iter()
            .map(|harvest| {
                let (status, error) = match &harvest.history {
                    History::Events(_) => ("ok", None),
                    History::Unsupported => ("unsupported", None),
                    History::Failed(error) => ("error", Some(error.to_string())),
                };
                HarnessJson {
                    id: &harvest.id,
                    kind: harvest.kind,
                    status,
                    events: harvest.events().len(),
                    error,
                }
            })
            .collect(),
    };
    serde_json::to_string_pretty(&value).expect("the breakdown is valid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breakdown::tests::{at, event, harvest};
    use crate::breakdown::{Grouping, Request};
    use crate::usage::ProbeError;

    fn build(grouping: Grouping, metric: Metric) -> Breakdown {
        Request {
            past: "3d".parse().unwrap(),
            until: at("2026-10-04T12:00:00Z"),
            metric,
            grouping,
            top: 10,
        }
        .build(vec![
            harvest(
                "claude",
                Rgb(1, 2, 3),
                History::Events(vec![
                    event("2026-10-02T10:00:00Z", "opus", 1_999_999, None),
                    event("2026-10-03T10:00:00Z", "haiku", 499_999, None),
                ]),
            ),
            harvest("copilot", Rgb(4, 5, 6), History::Unsupported),
            harvest(
                "codex",
                Rgb(7, 8, 9),
                History::Failed(ProbeError::Parse("Bad file.".into())),
            ),
        ])
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(token_count(845), "845");
        assert_eq!(token_count(845_000), "845K");
        assert_eq!(token_count(12_340_000), "12.3M");
        assert_eq!(token_count(1_500_000_000), "1.5B");
        assert_eq!(dollars(45.666), "$45.67");
        assert_eq!(percent(1.0, 3.0), "33%");
        assert_eq!(percent(1.0, 300.0), "0.3%");
        assert_eq!(percent(1.0, 3000.0), "<0.1%");
        assert_eq!(percent(0.0, 3.0), "0%");
        assert_eq!(list(&["a", "b", "c"]), "a, b, and c");
    }

    #[test]
    fn plain_shows_totals_rows_and_notes() {
        let text = render(&build(Grouping::Model, Metric::Tokens), false, 80);
        let lines: Vec<_> = text.lines().collect();
        assert!(lines[0].starts_with("Breakdown  last 3 days  tokens"));
        assert_eq!(lines[3], "  2.5M tokens");
        assert!(lines[6].contains("█ input 2.5M 100%   ▓ output 2 <0.1%"));
        assert_eq!(lines[8], "  By model");
        assert_eq!(
            lines[9],
            format!("  opus   ●  {}  2.0M    80%", "█".repeat(24))
        );
        assert_eq!(
            lines[10],
            format!("  haiku  ●  {}{}  500K    20%", "██████", " ".repeat(18))
        );
        assert!(text.contains("  · No token history for copilot."));
        assert!(text.contains("  · codex failed. Response format is not known. Bad file."));
        assert!(text.contains("  · Use --type cost to see the cost."));
        assert!(!text.contains('\x1b'));
    }

    #[test]
    fn day_chart_has_one_column_per_day_and_axis_labels() {
        let text = render(&build(Grouping::Day, Metric::Tokens), false, 80);
        let chart: Vec<_> = text
            .lines()
            .skip_while(|line| !line.contains("By day"))
            .take_while(|line| !line.is_empty())
            .collect();
        assert_eq!(chart.len(), 12);
        assert!(chart[1].trim_start().starts_with("2.0M │"));
        assert!(chart[8].trim_start().starts_with("0 │"));
        assert!(chart[1].contains('█'));
        assert!(chart[9].contains("└────────"));
        assert!(chart[11].contains("● claude"));
    }

    #[test]
    fn json_has_a_stable_shape() {
        let value: serde_json::Value =
            serde_json::from_str(&json(&build(Grouping::Harness, Metric::Cost))).unwrap();
        assert_eq!(value["type"], "cost");
        assert_eq!(value["by"], "harness");
        assert_eq!(value["total"]["tokens"]["input"], 2_499_998);
        assert_eq!(value["total"]["tokens"]["total"], 2_500_000);
        assert_eq!(value["total"]["cost_usd"], 0.0);
        assert_eq!(value["total"]["unpriced_tokens"], 2_500_000);
        assert_eq!(value["rows"][0]["label"], "claude");
        assert_eq!(value["harnesses"][1]["status"], "unsupported");
        assert_eq!(value["harnesses"][2]["status"], "error");
        assert_eq!(
            value["harnesses"][2]["error"],
            "Response format is not known. Bad file."
        );
        assert!(value["harnesses"][0].get("error").is_none());
    }
}
