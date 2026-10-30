use crate::usage::{Outcome, Report, Reset};

pub fn render(reports: &[Report]) -> String {
    let mut lines = Vec::new();
    for report in reports {
        let Outcome::Usage(usage) = &report.outcome else {
            continue;
        };
        for window in &usage.windows {
            let label = match &window.scope {
                Some(scope) => format!("{}:{scope}", window.span.label()),
                None => window.span.label(),
            };
            let percent = window
                .meter
                .percent()
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| "-".into());
            let reset = match window.reset {
                Reset::At(at) => at.timestamp().to_string(),
                _ => "-".into(),
            };
            lines.push(format!("{}\t{label}\t{percent}\t{reset}", report.id));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::tests::reports;
    use crate::usage::{Meter, Span, Window};

    #[test]
    fn raw_shows_scopes_unknown_values_and_skips_errors() {
        let mut reports = reports();
        let Outcome::Usage(usage) = &mut reports[0].outcome else {
            unreachable!()
        };
        usage.windows.push(Window {
            span: Span::Week,
            scope: Some("opus".into()),
            meter: Meter::Spend {
                used_usd: 4.21,
                limit_usd: None,
            },
            reset: Reset::Rolling,
        });
        let timestamp = usage.windows[0].reset;
        let Reset::At(at) = timestamp else {
            unreachable!()
        };
        assert_eq!(
            render(&reports),
            format!(
                "claude\t5h\t50.0\t{}\nclaude\tweek:opus\t-\t-",
                at.timestamp()
            )
        );
        assert_eq!(render(&[]), "");
    }
}
