use crate::config::{Limits, Source};
use crate::theme::Rgb;

pub struct KindInfo {
    pub name: &'static str,
    pub title: &'static str,
    pub color: Rgb,
    pub method: &'static str,
    pub default_source: fn() -> Source,
}

pub const KINDS: &[KindInfo] = &[
    KindInfo {
        name: "claude",
        title: "Claude Code",
        color: Rgb(0xD9, 0x77, 0x57),
        method: "Reads the Claude Code login and asks the Anthropic usage endpoint.",
        default_source: || Source::Claude { credentials: None },
    },
    KindInfo {
        name: "codex",
        title: "Codex",
        color: Rgb(0x10, 0xA3, 0x7F),
        method: "Reads the Codex login and asks the ChatGPT usage endpoint.",
        default_source: || Source::Codex { home: None },
    },
    KindInfo {
        name: "cursor",
        title: "Cursor",
        color: Rgb(0x4F, 0x8E, 0xF7),
        method: "Reads the Cursor or cursor-agent login and asks the Cursor dashboard endpoint.",
        default_source: || Source::Cursor { state_db: None },
    },
    KindInfo {
        name: "copilot",
        title: "GitHub Copilot",
        color: Rgb(0x89, 0x57, 0xE5),
        method: "Reads the GitHub CLI token and asks the Copilot quota endpoint.",
        default_source: || Source::Copilot { token_command: None },
    },
    KindInfo {
        name: "opencode-go",
        title: "OpenCode Go",
        color: Rgb(0xF2, 0xC9, 0x4C),
        method: "Adds the cost of each OpenCode Go message in the local OpenCode database.",
        default_source: || Source::OpencodeGo { db: None, limits: Limits::opencode_go() },
    },
    KindInfo {
        name: "pi",
        title: "Pi",
        color: Rgb(0xC6, 0x78, 0xDD),
        method: "Adds the cost of each assistant message in the local Pi session files.",
        default_source: || Source::Pi { sessions: None, limits: Limits::default() },
    },
    KindInfo {
        name: "hermes",
        title: "Hermes Agent",
        color: Rgb(0x56, 0xB6, 0xC2),
        method: "Adds the cost of each session in the local Hermes state database.",
        default_source: || Source::Hermes { db: None, limits: Limits::default() },
    },
    KindInfo {
        name: "command",
        title: "Custom command",
        color: Rgb(0xAB, 0xB2, 0xBF),
        method: "Runs your command and reads usage JSON from its output.",
        default_source: || Source::Command { run: String::new() },
    },
];

pub fn find(name: &str) -> Option<&'static KindInfo> {
    KINDS.iter().find(|kind| kind.name == name)
}
