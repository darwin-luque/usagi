use std::path::Path;

use super::Detection;
use crate::usage::{ProbeError, Usage};

pub fn fetch(_home: Option<&Path>) -> Result<Usage, ProbeError> {
    Err(ProbeError::Unsupported("Not built yet".into()))
}

pub fn detect(_home: Option<&Path>) -> Detection {
    Detection::Absent
}
