use chrono::Utc;

use crate::config::Harness;
use crate::sources;
use crate::usage::Report;

pub fn probe(harness: &Harness) -> Report {
    Report {
        id: harness.id.clone(),
        kind: harness.source.kind().name,
        title: harness.source.kind().title.to_string(),
        color: harness.color(),
        fetched_at: Utc::now(),
        outcome: sources::fetch(&harness.source).into(),
    }
}

pub fn probe_all(harnesses: &[Harness]) -> Vec<Report> {
    each(harnesses, probe)
}

pub fn each<T: Send>(harnesses: &[Harness], read: impl Fn(&Harness) -> T + Sync) -> Vec<T> {
    let read = &read;
    std::thread::scope(|scope| {
        let handles: Vec<_> = harnesses
            .iter()
            .map(|harness| scope.spawn(move || read(harness)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("probe thread does not panic"))
            .collect()
    })
}
