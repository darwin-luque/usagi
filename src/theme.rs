use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl FromStr for Rgb {
    type Err = String;

    fn from_str(text: &str) -> Result<Rgb, String> {
        let hex = text.trim().trim_start_matches('#');
        let valid = hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit());
        if !valid {
            return Err(format!("\"{text}\" is not a color. Use the format #RRGGBB"));
        }
        let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0);
        Ok(Rgb(channel(0), channel(2), channel(4)))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Rgb, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BarStyle {
    #[default]
    Blocks,
    Line,
    Dots,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub bar: BarStyle,
    pub warn_at: f64,
    pub danger_at: f64,
    pub warn: Rgb,
    pub danger: Rgb,
    pub calm: Rgb,
    pub track: Rgb,
    pub show_pace: bool,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme {
            bar: BarStyle::Blocks,
            warn_at: 75.0,
            danger_at: 90.0,
            warn: Rgb(0xE5, 0xC0, 0x7B),
            danger: Rgb(0xE0, 0x6C, 0x75),
            calm: Rgb(0x98, 0xC3, 0x79),
            track: Rgb(0x3B, 0x3F, 0x4A),
            show_pace: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Calm,
    Warn,
    Danger,
}

impl Theme {
    pub fn level(&self, percent: f64) -> Level {
        if percent >= self.danger_at {
            Level::Danger
        } else if percent >= self.warn_at {
            Level::Warn
        } else {
            Level::Calm
        }
    }

    pub fn level_color(&self, percent: f64) -> Rgb {
        match self.level(percent) {
            Level::Calm => self.calm,
            Level::Warn => self.warn,
            Level::Danger => self.danger,
        }
    }
}
