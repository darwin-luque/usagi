use std::path::Path;

use super::Detection;
use crate::usage::{ProbeError, Usage};

pub fn fetch(_credentials: Option<&Path>) -> Result<Usage, ProbeError> {
    Err(ProbeError::Unsupported("Not built yet".into()))
}

pub fn detect(_credentials: Option<&Path>) -> Detection {
    Detection::Absent
}
