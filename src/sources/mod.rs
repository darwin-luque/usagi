mod claude;
mod codex;
mod command;
mod copilot;
mod cursor;
mod hermes;
mod jsonl;
mod ledger;
mod opencode_go;
mod pi;
mod remote;

use chrono::{DateTime, Utc};

use crate::config::Source;
use crate::history::History;
use crate::usage::{ProbeError, Usage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detection {
    Ready(String),
    Partial(String),
    Absent,
}

pub fn fetch(source: &Source) -> Result<Usage, ProbeError> {
    match source {
        Source::Claude { credentials } => claude::fetch(credentials.as_deref()),
        Source::Codex { home } => codex::fetch(home.as_deref()),
        Source::Cursor { state_db } => cursor::fetch(state_db.as_deref()),
        Source::Copilot { token_command } => copilot::fetch(token_command.as_deref()),
        Source::OpencodeGo { db, limits } => opencode_go::fetch(db.as_deref(), limits),
        Source::Pi { sessions, limits } => pi::fetch(sessions.as_deref(), limits),
        Source::Hermes { db, limits } => hermes::fetch(db.as_deref(), limits),
        Source::Command { run } => command::fetch(run),
    }
}

pub fn events(source: &Source, since: DateTime<Utc>) -> History {
    match source {
        Source::Claude { .. } => claude::events(since).into(),
        Source::Codex { home } => codex::events(home.as_deref(), since).into(),
        Source::OpencodeGo { db, .. } => opencode_go::events(db.as_deref(), since).into(),
        Source::Pi { sessions, .. } => pi::events(sessions.as_deref(), since).into(),
        Source::Hermes { db, .. } => hermes::events(db.as_deref(), since).into(),
        Source::Cursor { .. } | Source::Copilot { .. } | Source::Command { .. } => {
            History::Unsupported
        }
    }
}

pub fn detect(source: &Source) -> Detection {
    match source {
        Source::Claude { credentials } => claude::detect(credentials.as_deref()),
        Source::Codex { home } => codex::detect(home.as_deref()),
        Source::Cursor { state_db } => cursor::detect(state_db.as_deref()),
        Source::Copilot { token_command } => copilot::detect(token_command.as_deref()),
        Source::OpencodeGo { db, .. } => opencode_go::detect(db.as_deref()),
        Source::Pi { sessions, .. } => pi::detect(sessions.as_deref()),
        Source::Hermes { db, .. } => hermes::detect(db.as_deref()),
        Source::Command { run } => command::detect(run),
    }
}
