use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::history::Tokens;
use crate::sources::remote;
use crate::usage::ProbeError;

const URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const FRESH: Duration = Duration::from_secs(24 * 3600);
const SUFFIXES: [&str; 7] = [
    "-low",
    "-medium",
    "-high",
    "-xhigh",
    "-max",
    "-fast",
    "-thinking",
];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Price {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

impl Price {
    fn cost(&self, tokens: Tokens) -> f64 {
        tokens.input as f64 * self.input
            + tokens.output as f64 * self.output
            + tokens.cache_read as f64 * self.cache_read
            + tokens.cache_write as f64 * self.cache_write
    }
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prices(HashMap<String, Price>);

impl Prices {
    pub fn from_list(value: &Value) -> Prices {
        let Some(models) = value.as_object() else {
            return Prices::default();
        };
        Prices(
            models
                .iter()
                .filter_map(|(name, entry)| {
                    let input = entry["input_cost_per_token"].as_f64()?;
                    Some((
                        name.clone(),
                        Price {
                            input,
                            output: entry["output_cost_per_token"].as_f64()?,
                            cache_read: entry["cache_read_input_token_cost"]
                                .as_f64()
                                .unwrap_or(input),
                            cache_write: entry["cache_creation_input_token_cost"]
                                .as_f64()
                                .unwrap_or(input),
                        },
                    ))
                })
                .collect(),
        )
    }

    fn find(&self, model: &str) -> Option<&Price> {
        if let Some(price) = self.0.get(model) {
            return Some(price);
        }
        let lower = model.to_lowercase();
        if let Some(price) = self.0.get(&lower) {
            return Some(price);
        }
        let mut name = lower.rsplit('/').next().unwrap_or(&lower);
        loop {
            if let Some(price) = self.0.get(name) {
                return Some(price);
            }
            name = shorter(name)?;
        }
    }

    pub fn cost(&self, model: &str, tokens: Tokens) -> Option<f64> {
        self.find(model).map(|price| price.cost(tokens))
    }
}

fn shorter(name: &str) -> Option<&str> {
    if let Some(rest) = SUFFIXES.iter().find_map(|suffix| name.strip_suffix(suffix)) {
        return Some(rest);
    }
    let (rest, date) = name.rsplit_once('-')?;
    (date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit())).then_some(rest)
}

fn cache_path() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::home().join(".cache"))
        .join("usagi")
        .join("prices.json")
}

fn read_cache(path: &Path) -> Option<(Prices, Duration)> {
    let age = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .unwrap_or_default();
    let prices = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some((prices, age))
}

fn download() -> Result<Prices, ProbeError> {
    let value = remote::response(remote::agent().get(URL).call(), "")?;
    let prices = Prices::from_list(&value);
    if prices.0.is_empty() {
        return Err(remote::parse_error("The price list is empty."));
    }
    Ok(prices)
}

fn save(path: &Path, prices: &Prices) -> std::io::Result<()> {
    let folder = path.parent().unwrap_or(path);
    std::fs::create_dir_all(folder)?;
    let temporary = folder.join(format!("prices.{}.json", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(prices)?)?;
    std::fs::rename(temporary, path)
}

pub fn load(refresh: bool) -> Option<Prices> {
    let path = cache_path();
    let cached = read_cache(&path);
    match cached {
        Some((prices, age)) if !refresh || age < FRESH => Some(prices),
        _ if !refresh => None,
        stale => match download() {
            Ok(prices) => {
                let _ = save(&path, &prices);
                Some(prices)
            }
            Err(_) => stale.map(|(prices, _)| prices),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prices() -> Prices {
        Prices::from_list(&json!({
            "sample_spec": {"input_cost_per_token": "x"},
            "claude-opus-5-5": {"input_cost_per_token": 4e-6, "output_cost_per_token": 2e-5,
                "cache_read_input_token_cost": 2e-7, "cache_creation_input_token_cost": 5e-6},
            "gpt-6.1-sol": {"input_cost_per_token": 2e-6, "output_cost_per_token": 1e-5},
            "claude-haiku-4-5": {"input_cost_per_token": 1e-6, "output_cost_per_token": 5e-6},
            "image-only": {"output_cost_per_token": 1.0}
        }))
    }

    #[test]
    fn list_keeps_token_prices_and_fills_cache_prices_from_input() {
        let prices = prices();
        assert_eq!(prices.0.len(), 3);
        assert_eq!(
            prices.find("gpt-6.1-sol"),
            Some(&Price {
                input: 2e-6,
                output: 1e-5,
                cache_read: 2e-6,
                cache_write: 2e-6
            })
        );
    }

    #[test]
    fn lookup_strips_case_provider_effort_and_date() {
        let prices = prices();
        let opus = prices.find("claude-opus-5-5");
        for model in [
            "Claude-Opus-5-5",
            "anthropic/claude-opus-5-5",
            "claude-opus-5-5-medium",
            "claude-opus-5-5-thinking-high",
            "claude-opus-5-5-xhigh-fast",
            "claude-opus-5-5-20261001",
        ] {
            assert_eq!(prices.find(model), opus, "{model}");
        }
        assert!(prices.find("claude-haiku-4-5-20251001").is_some());
        assert!(prices.find("gpt-6.1-sol-max").is_some());
        assert_eq!(prices.find("unknown-model-high"), None);
        assert_eq!(prices.find("opus"), None);
    }

    #[test]
    fn cost_multiplies_each_token_type_by_its_price() {
        let cost = prices()
            .cost(
                "claude-opus-5-5",
                Tokens {
                    input: 1_000_000,
                    output: 100_000,
                    cache_read: 10_000_000,
                    cache_write: 200_000,
                },
            )
            .unwrap();
        assert!((cost - (4.0 + 2.0 + 2.0 + 1.0)).abs() < 1e-9);
    }

    #[test]
    #[ignore]
    fn live_price_list_has_known_models() {
        let prices = download().unwrap();
        assert!(prices.find("claude-opus-5-5").is_some());
        assert!(prices.find("gpt-6.1-sol").is_some());
    }
}
