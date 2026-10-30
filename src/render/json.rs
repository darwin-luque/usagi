use crate::usage::Report;

pub fn render(reports: &[Report]) -> String {
    serde_json::to_string_pretty(reports).expect("Reports contain JSON values")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::tests::reports;

    #[test]
    fn json_preserves_used_percent_and_error_fields() {
        let text = render(&reports());
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value[0]["usage"]["windows"][0]["used_percent"], 50.0);
        assert_eq!(value[1]["error"], "Network failed. Try again.");
        assert!(text.contains("\n  {"));
        assert_eq!(render(&[]), "[]");
    }
}
