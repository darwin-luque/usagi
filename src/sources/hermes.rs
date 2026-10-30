use std::path::Path;

use super::Detection;
use crate::config::Limits;
use crate::usage::{ProbeError, Usage};

pub fn fetch(_path: Option<&Path>, _limits: &Limits) -> Result<Usage, ProbeError> {
    Err(ProbeError::Unsupported("Not built yet".into()))
}

pub fn detect(_path: Option<&Path>) -> Detection {
    Detection::Absent
}
