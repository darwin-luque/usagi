use std::path::Path;

use super::Detection;
use crate::usage::{ProbeError, Usage};

pub fn fetch(_state_db: Option<&Path>) -> Result<Usage, ProbeError> {
    Err(ProbeError::Unsupported("Not built yet".into()))
}

pub fn detect(_state_db: Option<&Path>) -> Detection {
    Detection::Absent
}
