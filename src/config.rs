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
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

impl Config {
    pub fn load(path: &Path) -> Result<Option<Config>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("Cannot read {}", path.display())),
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
                bail!("Two harnesses use the id \"{}\". Each id must be unique", harness.id);
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
        assert_eq!(config.harnesses[1].source, Source::OpencodeGo { db: None, limits: Limits::opencode_go() });
        assert!(!config.harnesses[1].enabled);
        assert_eq!(config.theme.warn_at, 60.0);
        assert_eq!(config.theme.danger_at, 90.0);
        let again: Config = toml::from_str(&toml::to_string_pretty(&config).unwrap()).unwrap();
        assert_eq!(again, config);
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut config = Config::default();
        config.harnesses.push(Harness::new(Source::Codex { home: None }));
        config.harnesses.push(Harness::new(Source::Codex { home: None }));
        assert!(config.validate().is_err());
    }
}
