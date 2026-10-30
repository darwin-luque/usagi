use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::kinds::{self, KindInfo};
use crate::theme::{Rgb, Theme};

pub const MIN_REFRESH_SECONDS: u64 = 15;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_refresh_seconds")]
    pub refresh_seconds: u64,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default, rename = "harness")]
    pub harnesses: Vec<Harness>,
}

fn default_refresh_seconds() -> u64 {
    60
}

impl Default for Config {
    fn default() -> Config {
        Config {
            refresh_seconds: default_refresh_seconds(),
            theme: Theme::default(),
            harnesses: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Harness {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    #[serde(default = "enabled_by_default", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub source: Source,
}

fn enabled_by_default() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

impl Harness {
    pub fn new(source: Source) -> Harness {
        Harness {
            id: source.kind().name.to_string(),
            color: None,
            enabled: true,
            source,
        }
    }

    pub fn color(&self) -> Rgb {
        self.color.unwrap_or(self.source.kind().color)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Source {
    Claude {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credentials: Option<PathBuf>,
    },
    Codex {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<PathBuf>,
    },
    Cursor {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state_db: Option<PathBuf>,
    },
    Copilot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_command: Option<String>,
    },
    OpencodeGo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        db: Option<PathBuf>,
        #[serde(default = "Limits::opencode_go")]
        limits: Limits,
    },
    Pi {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sessions: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Limits::is_empty")]
        limits: Limits,
    },
    Hermes {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        db: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Limits::is_empty")]
        limits: Limits,
    },
    Command {
        run: String,
    },
}

impl Source {
    pub fn kind(&self) -> &'static KindInfo {
        let name = match self {
            Source::Claude { .. } => "claude",
            Source::Codex { .. } => "codex",
            Source::Cursor { .. } => "cursor",
            Source::Copilot { .. } => "copilot",
            Source::OpencodeGo { .. } => "opencode-go",
            Source::Pi { .. } => "pi",
            Source::Hermes { .. } => "hermes",
            Source::Command { .. } => "command",
        };
        kinds::find(name).expect("every source variant has a registry entry")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hours: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<f64>,
}

impl Limits {
    pub fn opencode_go() -> Limits {
        Limits {
            five_hours: Some(12.0),
            day: None,
            week: Some(30.0),
            month: Some(60.0),
        }
    }

    pub fn is_empty(&self) -> bool {
        self == &Limits::default()
    }
}

pub fn path() -> PathBuf {
    if let Some(path) = std::env::var_os("USAGI_CONFIG") {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("usagi").join("config.toml")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

impl Config {
    pub fn add(&mut self, harness: Harness) -> Result<()> {
        if self.harnesses.iter().any(|item| item.id == harness.id) {
            bail!(
                "The id \"{}\" is in use. Use --id to set a new id.",
                harness.id
            );
        }
        if harness.id.trim().is_empty() {
            bail!("A harness needs an id.");
        }
        self.harnesses.push(harness);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<Harness> {
        self.harness_mut(id)?;
        let index = self
            .harnesses
            .iter()
            .position(|item| item.id == id)
            .unwrap();
        Ok(self.harnesses.remove(index))
    }

    pub fn harness_mut(&mut self, id: &str) -> Result<&mut Harness> {
        let known = self.known_ids();
        self.harnesses
            .iter_mut()
            .find(|item| item.id == id)
            .with_context(|| format!("Unknown id \"{id}\". Known ids: {known}."))
    }

    pub fn enabled(&self, only: &[String]) -> Result<Vec<Harness>> {
        for id in only {
            if !self.harnesses.iter().any(|item| item.id == *id) {
                bail!("Unknown id \"{id}\". Known ids: {}.", self.known_ids());
            }
        }
        Ok(self
            .harnesses
            .iter()
            .filter(|item| item.enabled && (only.is_empty() || only.contains(&item.id)))
            .cloned()
            .collect())
    }

    fn known_ids(&self) -> String {
        if self.harnesses.is_empty() {
            "none".into()
        } else {
            self.harnesses
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    pub fn load(path: &Path) -> Result<Option<Config>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("Cannot read {}", path.display()));
            }
        };
        let config: Config =
            toml::from_str(&text).with_context(|| format!("Cannot parse {}", path.display()))?;
        config.validate()?;
        Ok(Some(config))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot make {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text).with_context(|| format!("Cannot write {}", path.display()))
    }

    pub fn validate(&self) -> Result<()> {
        if self.refresh_seconds < MIN_REFRESH_SECONDS {
            bail!("refresh_seconds must be {MIN_REFRESH_SECONDS} or more");
        }
        let mut seen = std::collections::HashSet::new();
        for harness in &self.harnesses {
            if harness.id.trim().is_empty() {
                bail!("A harness has no id");
            }
            if !seen.insert(harness.id.as_str()) {
                bail!(
                    "Two harnesses use the id \"{}\". Each id must be unique",
                    harness.id
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_survives_a_round_trip_through_toml() {
        let text = r##"
refresh_seconds = 30

[theme]
warn_at = 60.0

[[harness]]
id = "claude"
kind = "claude"
color = "#112233"

[[harness]]
id = "go"
kind = "opencode-go"
enabled = false

[[harness]]
id = "mine"
kind = "command"
run = "echo {}"
"##;
        let config: Config = toml::from_str(text).unwrap();
        assert_eq!(config.harnesses.len(), 3);
        assert_eq!(config.harnesses[0].color(), Rgb(0x11, 0x22, 0x33));
        assert_eq!(
            config.harnesses[1].source,
            Source::OpencodeGo {
                db: None,
                limits: Limits::opencode_go()
            }
        );
        assert!(!config.harnesses[1].enabled);
        assert_eq!(config.theme.warn_at, 60.0);
        assert_eq!(config.theme.danger_at, 90.0);
        let again: Config = toml::from_str(&toml::to_string_pretty(&config).unwrap()).unwrap();
        assert_eq!(again, config);
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut config = Config::default();
        config
            .harnesses
            .push(Harness::new(Source::Codex { home: None }));
        config
            .harnesses
            .push(Harness::new(Source::Codex { home: None }));
        assert!(config.validate().is_err());
    }
    #[test]
    fn add_rejects_used_and_empty_ids_without_changes() {
        let mut config = Config::default();
        let harness = Harness::new(Source::Codex { home: None });
        config.add(harness.clone()).unwrap();
        let error = config.add(harness.clone()).unwrap_err().to_string();
        assert!(error.contains("--id"));
        let mut empty = harness.clone();
        empty.id = " ".into();
        assert!(config.add(empty).is_err());
        assert_eq!(config.harnesses, vec![harness]);
    }

    #[test]
    fn remove_returns_the_harness_and_keeps_other_harnesses() {
        let mut config = Config::default();
        let codex = Harness::new(Source::Codex { home: None });
        let claude = Harness::new(Source::Claude { credentials: None });
        config.add(codex.clone()).unwrap();
        config.add(claude.clone()).unwrap();
        assert_eq!(config.remove("codex").unwrap(), codex);
        assert_eq!(config.harnesses, vec![claude]);
        let error = config.remove("missing").unwrap_err().to_string();
        assert!(error.contains("claude"));
        assert!(error.contains("missing"));
    }

    #[test]
    fn harness_mut_changes_the_selected_harness_and_lists_known_ids() {
        let mut config = Config::default();
        config
            .add(Harness::new(Source::Codex { home: None }))
            .unwrap();
        config.harness_mut("codex").unwrap().enabled = false;
        assert!(!config.harnesses[0].enabled);
        let error = config.harness_mut("missing").unwrap_err().to_string();
        assert!(error.contains("Known ids: codex"));
        assert!(
            Config::default()
                .harness_mut("missing")
                .unwrap_err()
                .to_string()
                .contains("Known ids: none")
        );
    }

    #[test]
    fn enabled_filters_ids_and_skips_disabled_harnesses() {
        let mut config = Config::default();
        let codex = Harness::new(Source::Codex { home: None });
        config.add(codex.clone()).unwrap();
        config
            .add(Harness::new(Source::Claude { credentials: None }))
            .unwrap();
        config.harness_mut("claude").unwrap().enabled = false;
        assert_eq!(config.enabled(&[]).unwrap(), vec![codex.clone()]);
        assert_eq!(
            config.enabled(&["codex".into(), "codex".into()]).unwrap(),
            vec![codex]
        );
        assert!(config.enabled(&["claude".into()]).unwrap().is_empty());
        let error = config.enabled(&["missing".into()]).unwrap_err().to_string();
        assert!(error.contains("codex, claude"));
        assert!(error.contains("missing"));
    }
}
