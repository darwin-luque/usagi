use std::collections::BTreeMap;
use std::ops::AddAssign;

use chrono::{
    DateTime, Datelike, Days, Duration, Local, Months, NaiveDate, NaiveDateTime, TimeZone,
    Timelike, Utc,
};
use clap::ValueEnum;
use serde::Serialize;

use crate::history::{Event, Harvest, History, Past, Tokens};
use crate::prices::Prices;
use crate::theme::Rgb;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Metric {
    Tokens,
    Cost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Grouping {
    Model,
    Day,
    Harness,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Amount {
    pub tokens: Tokens,
    pub cost_usd: f64,
    pub unpriced_tokens: u64,
}

impl Amount {
    fn add(&mut self, event: &Event) {
        self.tokens += event.tokens;
        match event.cost_usd {
            Some(cost) => self.cost_usd += cost,
            None => self.unpriced_tokens += event.tokens.total(),
        }
    }

    pub fn value(&self, metric: Metric) -> f64 {
        match metric {
            Metric::Tokens => self.tokens.total() as f64,
            Metric::Cost => self.cost_usd,
        }
    }
}

impl AddAssign for Amount {
    fn add_assign(&mut self, other: Amount) {
        self.tokens += other.tokens;
        self.cost_usd += other.cost_usd;
        self.unpriced_tokens += other.unpriced_tokens;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    pub colors: Vec<Rgb>,
    pub amount: Amount,
}

impl Row {
    fn new(label: String) -> Row {
        Row {
            label,
            colors: Vec::new(),
            amount: Amount::default(),
        }
    }

    fn add_color(&mut self, color: Rgb) {
        if !self.colors.contains(&color) {
            self.colors.push(color);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    Hour,
    Day,
    Week,
    Month,
}

impl Step {
    pub fn for_span(span: Duration) -> Step {
        if span <= Duration::hours(48) {
            Step::Hour
        } else if span <= Duration::days(92) {
            Step::Day
        } else if span <= Duration::days(730) {
            Step::Week
        } else {
            Step::Month
        }
    }

    fn floor(self, at: NaiveDateTime) -> NaiveDateTime {
        let date = at.date();
        match self {
            Step::Hour => date.and_hms_opt(at.hour(), 0, 0),
            Step::Day => date.and_hms_opt(0, 0, 0),
            Step::Week => (date - Days::new(date.weekday().num_days_from_monday().into()))
                .and_hms_opt(0, 0, 0),
            Step::Month => NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
                .and_then(|date| date.and_hms_opt(0, 0, 0)),
        }
        .expect("the start of a bucket is a valid time")
    }

    fn next(self, start: NaiveDateTime) -> NaiveDateTime {
        match self {
            Step::Hour => start + Duration::hours(1),
            Step::Day => start + Days::new(1),
            Step::Week => start + Days::new(7),
            Step::Month => start + Months::new(1),
        }
    }

    pub fn label(self, start: NaiveDateTime) -> String {
        let format = match self {
            Step::Hour => "%Y-%m-%d %H:00",
            Step::Day | Step::Week => "%Y-%m-%d",
            Step::Month => "%Y-%m",
        };
        start.format(format).to_string()
    }

    pub fn short_label(self, start: NaiveDateTime) -> String {
        let format = match self {
            Step::Hour => "%a %H:%M",
            Step::Day => "%b %d",
            Step::Week => "%b %d %Y",
            Step::Month => "%b %Y",
        };
        start.format(format).to_string()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    pub start: NaiveDateTime,
    pub amounts: Vec<Amount>,
}

impl Bucket {
    pub fn total(&self) -> Amount {
        let mut total = Amount::default();
        for amount in &self.amounts {
            total += *amount;
        }
        total
    }
}

pub enum View {
    Ranked(Vec<Row>),
    Timeline { step: Step, buckets: Vec<Bucket> },
}

impl View {
    pub fn rows(&self) -> Vec<Row> {
        match self {
            View::Ranked(rows) => rows.clone(),
            View::Timeline { step, buckets } => buckets
                .iter()
                .map(|bucket| Row {
                    label: step.label(bucket.start),
                    colors: Vec::new(),
                    amount: bucket.total(),
                })
                .collect(),
        }
    }
}

pub struct Breakdown {
    pub past: Past,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub metric: Metric,
    pub grouping: Grouping,
    pub total: Amount,
    pub view: View,
    pub harvests: Vec<Harvest>,
    pub priced: bool,
    pub recorded: Vec<&'static str>,
    pub estimated: Vec<&'static str>,
}

impl Breakdown {
    pub fn shows_cost(&self) -> bool {
        self.metric == Metric::Cost || self.priced || self.total.unpriced_tokens == 0
    }
}

fn titles(harvests: &[Harvest], recorded: bool) -> Vec<&'static str> {
    let mut titles = Vec::new();
    for harvest in harvests {
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
}

fn price(harvests: &mut [Harvest], prices: &Prices) {
    for harvest in harvests {
        if let History::Events(events) = &mut harvest.history {
            for event in events.iter_mut().filter(|event| event.cost_usd.is_none()) {
                event.cost_usd = prices.cost(&event.model, event.tokens);
            }
        }
    }
}

pub struct Request {
    pub past: Past,
    pub until: DateTime<Utc>,
    pub metric: Metric,
    pub grouping: Grouping,
    pub top: usize,
}

impl Request {
    pub fn since(&self) -> DateTime<Utc> {
        self.until - self.past.duration()
    }

    pub fn build(self, mut harvests: Vec<Harvest>, prices: Option<&Prices>) -> Breakdown {
        let recorded = titles(&harvests, true);
        let estimated = titles(&harvests, false);
        if let Some(prices) = prices {
            price(&mut harvests, prices);
        }
        let since = self.since();
        let view = match self.grouping {
            Grouping::Model => View::Ranked(by_model(&harvests, self.metric, self.top)),
            Grouping::Harness => View::Ranked(by_harness(&harvests, self.metric, self.top)),
            Grouping::Day => {
                let step = Step::for_span(self.past.duration());
                View::Timeline {
                    step,
                    buckets: buckets(&harvests, since, self.until, step, &Local),
                }
            }
        };
        Breakdown {
            past: self.past,
            since,
            until: self.until,
            metric: self.metric,
            grouping: self.grouping,
            total: total(&harvests),
            view,
            harvests,
            priced: prices.is_some(),
            recorded,
            estimated,
        }
    }
}

pub fn total(harvests: &[Harvest]) -> Amount {
    let mut total = Amount::default();
    for event in harvests.iter().flat_map(Harvest::events) {
        total.add(event);
    }
    total
}

pub fn by_model(harvests: &[Harvest], metric: Metric, top: usize) -> Vec<Row> {
    let mut rows: BTreeMap<&str, Row> = BTreeMap::new();
    for harvest in harvests {
        for event in harvest.events() {
            let row = rows
                .entry(&event.model)
                .or_insert_with(|| Row::new(event.model.clone()));
            row.amount.add(event);
            row.add_color(harvest.color);
        }
    }
    rank(rows.into_values().collect(), metric, top)
}

pub fn by_harness(harvests: &[Harvest], metric: Metric, top: usize) -> Vec<Row> {
    let rows = harvests
        .iter()
        .filter(|harvest| !harvest.events().is_empty())
        .map(|harvest| {
            let mut row = Row::new(harvest.id.clone());
            row.add_color(harvest.color);
            for event in harvest.events() {
                row.amount.add(event);
            }
            row
        })
        .collect();
    rank(rows, metric, top)
}

fn rank(mut rows: Vec<Row>, metric: Metric, top: usize) -> Vec<Row> {
    rows.sort_by(|a, b| {
        b.amount
            .value(metric)
            .total_cmp(&a.amount.value(metric))
            .then_with(|| a.label.cmp(&b.label))
    });
    if rows.len() > top {
        let mut other = Row::new("other".into());
        for row in rows.split_off(top) {
            other.amount += row.amount;
            for color in row.colors {
                other.add_color(color);
            }
        }
        rows.push(other);
    }
    rows
}

fn instant<Tz: TimeZone>(zone: &Tz, local: NaiveDateTime) -> DateTime<Utc> {
    zone.from_local_datetime(&local)
        .earliest()
        .map(|at| at.to_utc())
        .unwrap_or_else(|| local.and_utc())
}

pub fn buckets<Tz: TimeZone>(
    harvests: &[Harvest],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    step: Step,
    zone: &Tz,
) -> Vec<Bucket> {
    let end = until.with_timezone(zone).naive_local();
    let mut start = step.floor(since.with_timezone(zone).naive_local());
    let mut buckets = Vec::new();
    while start <= end {
        buckets.push(Bucket {
            start,
            amounts: vec![Amount::default(); harvests.len()],
        });
        start = step.next(start);
    }
    let instants: Vec<_> = buckets
        .iter()
        .map(|bucket| instant(zone, bucket.start))
        .collect();
    for (index, harvest) in harvests.iter().enumerate() {
        for event in harvest.events() {
            let slot = instants.partition_point(|start| *start <= event.at);
            if let Some(bucket) = slot.checked_sub(1).and_then(|slot| buckets.get_mut(slot)) {
                bucket.amounts[index].add(event);
            }
        }
    }
    buckets
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().to_utc()
    }

    pub fn event(at_text: &str, model: &str, input: u64, cost_usd: Option<f64>) -> Event {
        Event {
            at: at(at_text),
            model: model.into(),
            tokens: Tokens {
                input,
                output: 1,
                cache_read: 0,
                cache_write: 0,
            },
            cost_usd,
        }
    }

    pub fn harvest(id: &str, color: Rgb, history: History) -> Harvest {
        Harvest {
            id: id.into(),
            kind: "claude",
            title: "Claude Code",
            color,
            history,
        }
    }

    const RED: Rgb = Rgb(255, 0, 0);
    const BLUE: Rgb = Rgb(0, 0, 255);

    fn sample() -> Vec<Harvest> {
        vec![
            harvest(
                "claude",
                RED,
                History::Events(vec![
                    event("2026-10-01T10:00:00Z", "opus", 99, None),
                    event("2026-10-02T10:00:00Z", "haiku", 9, None),
                    event("2026-10-03T10:00:00Z", "shared", 29, None),
                ]),
            ),
            harvest(
                "cursor",
                BLUE,
                History::Events(vec![
                    event("2026-10-01T11:00:00Z", "shared", 20, Some(2.0)),
                    event("2026-10-04T11:00:00Z", "tiny", 0, Some(5.0)),
                ]),
            ),
            harvest("copilot", RED, History::Unsupported),
        ]
    }

    #[test]
    fn total_splits_priced_and_unpriced_tokens() {
        let total = total(&sample());
        assert_eq!(total.tokens.total(), 162);
        assert_eq!(total.cost_usd, 7.0);
        assert_eq!(total.unpriced_tokens, 140);
    }

    #[test]
    fn models_rank_by_metric_and_fold_the_rest_into_other() {
        let rows = by_model(&sample(), Metric::Tokens, 2);
        let labels: Vec<_> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(labels, ["opus", "shared", "other"]);
        assert_eq!(rows[1].colors, [RED, BLUE]);
        assert_eq!(rows[1].amount.tokens.total(), 51);
        assert_eq!(rows[2].amount.tokens.total(), 11);
        assert_eq!(rows[2].colors, [RED, BLUE]);
        let rows = by_model(&sample(), Metric::Cost, 10);
        assert_eq!(rows[0].label, "tiny");
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn harnesses_without_events_have_no_row() {
        let rows = by_harness(&sample(), Metric::Tokens, 1);
        let labels: Vec<_> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(labels, ["claude", "other"]);
        assert_eq!(rows[1].amount.cost_usd, 7.0);
    }

    #[test]
    fn bucket_size_follows_the_past_span() {
        for (span, step) in [
            (Duration::hours(48), Step::Hour),
            (Duration::hours(49), Step::Day),
            (Duration::days(92), Step::Day),
            (Duration::days(93), Step::Week),
            (Duration::days(730), Step::Week),
            (Duration::days(731), Step::Month),
        ] {
            assert_eq!(Step::for_span(span), step, "{span}");
        }
    }

    #[test]
    fn day_buckets_include_empty_days_and_keep_each_harness() {
        let buckets = buckets(
            &sample(),
            at("2026-09-30T12:00:00Z"),
            at("2026-10-05T12:00:00Z"),
            Step::Day,
            &Utc,
        );
        let labels: Vec<_> = buckets
            .iter()
            .map(|bucket| Step::Day.label(bucket.start))
            .collect();
        assert_eq!(
            labels,
            [
                "2026-09-30",
                "2026-10-01",
                "2026-10-02",
                "2026-10-03",
                "2026-10-04",
                "2026-10-05"
            ]
        );
        assert_eq!(buckets[0].total(), Amount::default());
        assert_eq!(buckets[1].amounts[0].tokens.total(), 100);
        assert_eq!(buckets[1].amounts[1].tokens.total(), 21);
        assert_eq!(buckets[1].amounts[2], Amount::default());
        assert_eq!(buckets[5].total(), Amount::default());
    }

    #[test]
    fn weeks_start_on_monday_and_months_on_the_first() {
        let weeks = buckets(
            &sample(),
            at("2026-10-01T00:00:00Z"),
            at("2026-10-20T00:00:00Z"),
            Step::Week,
            &Utc,
        );
        assert_eq!(Step::Week.label(weeks[0].start), "2026-09-28");
        assert_eq!(Step::Week.short_label(weeks[0].start), "Sep 28 2026");
        assert_eq!(weeks.len(), 4);
        assert_eq!(weeks[0].total().tokens.total(), 162);
        let months = buckets(
            &[],
            at("2025-11-15T00:00:00Z"),
            at("2026-02-01T00:00:00Z"),
            Step::Month,
            &Utc,
        );
        let labels: Vec<_> = months
            .iter()
            .map(|bucket| Step::Month.label(bucket.start))
            .collect();
        assert_eq!(labels, ["2025-11", "2025-12", "2026-01", "2026-02"]);
        let hours = buckets(
            &[],
            at("2026-10-01T10:30:00Z"),
            at("2026-10-01T12:00:00Z"),
            Step::Hour,
            &Utc,
        );
        assert_eq!(hours.len(), 3);
    }
}
