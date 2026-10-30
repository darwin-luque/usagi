use super::Detection;
use crate::usage::{ProbeError, Usage};

pub fn fetch(_token_command: Option<&str>) -> Result<Usage, ProbeError> {
    Err(ProbeError::Unsupported("Not built yet".into()))
}

pub fn detect(_token_command: Option<&str>) -> Detection {
    Detection::Absent
}
