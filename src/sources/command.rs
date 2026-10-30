use super::Detection;
use crate::usage::{ProbeError, Usage};

pub fn fetch(_run: &str) -> Result<Usage, ProbeError> {
    Err(ProbeError::Unsupported("Not built yet".into()))
}

pub fn detect(_run: &str) -> Detection {
    Detection::Absent
}
