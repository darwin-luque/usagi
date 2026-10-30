use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::Detection;
use crate::usage::{ProbeError, Usage};

static NEXT_OUTPUT: AtomicU64 = AtomicU64::new(0);

struct OutputFile {
    file: Option<File>,
    path: PathBuf,
}

impl OutputFile {
    fn create() -> std::io::Result<Self> {
        loop {
            let sequence = NEXT_OUTPUT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("usagi-command-{}-{sequence}", std::process::id()));
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        file: Some(file),
                        path,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn text(&mut self) -> std::io::Result<String> {
        let file = self.file.as_mut().expect("output file is open");
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

impl Drop for OutputFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn fetch(run: &str) -> Result<Usage, ProbeError> {
    run_with_timeout(run, Duration::from_secs(20))
}

fn run_with_timeout(run: &str, timeout: Duration) -> Result<Usage, ProbeError> {
    if run.trim().is_empty() {
        return Err(ProbeError::Command(
            "The run value is empty. Set run in the config.".into(),
        ));
    }
    let command_error = |error: std::io::Error| ProbeError::Command(error.to_string());
    let mut stdout = OutputFile::create().map_err(command_error)?;
    let mut stderr = OutputFile::create().map_err(command_error)?;
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", run]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", run]);
        command
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(
            stdout
                .file
                .as_ref()
                .expect("output file is open")
                .try_clone()
                .map_err(command_error)?,
        )
        .stderr(
            stderr
                .file
                .as_ref()
                .expect("output file is open")
                .try_clone()
                .map_err(command_error)?,
        )
        .spawn()
        .map_err(command_error)?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(command_error(error));
            }
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProbeError::Command(
                "The command did not stop in 20 seconds.".into(),
            ));
        }
        std::thread::sleep(
            Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())),
        );
    };
    if !status.success() {
        return Err(ProbeError::Command(
            stderr
                .text()
                .map_err(command_error)?
                .chars()
                .take(200)
                .collect(),
        ));
    }
    parse_output(&stdout.text().map_err(command_error)?)
}

fn parse_output(text: &str) -> Result<Usage, ProbeError> {
    serde_json::from_str(text).map_err(|error| ProbeError::Parse(error.to_string()))
}

pub fn detect(run: &str) -> Detection {
    if run.trim().is_empty() {
        Detection::Absent
    } else {
        Detection::Ready("Command set.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{Meter, Reset, Span};

    #[test]
    fn command_reads_normalized_usage_json() {
        let run = if cfg!(windows) {
            r#"echo {"plan":"Custom","windows":[{"span":"5h","spend_usd":3,"limit_usd":12,"rolling":true}]}"#
        } else {
            r#"echo '{"plan":"Custom","windows":[{"span":"5h","spend_usd":3,"limit_usd":12,"rolling":true}]}'"#
        };
        let usage = fetch(run).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Custom"));
        assert_eq!(usage.windows[0].span, Span::FiveHours);
        assert_eq!(
            usage.windows[0].meter,
            Meter::Spend {
                used_usd: 3.0,
                limit_usd: Some(12.0)
            }
        );
        assert_eq!(usage.windows[0].reset, Reset::Rolling);
        assert_eq!(detect(run), Detection::Ready("Command set.".into()));
    }

    #[test]
    fn command_failure_reports_stderr() {
        assert_eq!(
            fetch(if cfg!(windows) {
                "echo failed >&2 & exit 7"
            } else {
                "printf failed >&2; exit 7"
            })
            .unwrap_err(),
            ProbeError::Command(
                if cfg!(windows) {
                    "failed \r\n"
                } else {
                    "failed"
                }
                .into()
            )
        );
    }

    #[test]
    fn command_rejects_invalid_json() {
        assert!(matches!(fetch("echo invalid"), Err(ProbeError::Parse(_))));
    }

    #[test]
    fn empty_command_explains_config() {
        assert_eq!(
            fetch("  ").unwrap_err(),
            ProbeError::Command("The run value is empty. Set run in the config.".into())
        );
        assert_eq!(detect(""), Detection::Absent);
    }

    #[cfg(unix)]
    #[test]
    fn command_timeout_stops_the_process() {
        let start = Instant::now();
        assert_eq!(
            run_with_timeout("exec sleep 10", Duration::from_millis(50)).unwrap_err(),
            ProbeError::Command("The command did not stop in 20 seconds.".into())
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn command_failure_limits_stderr_to_200_characters() {
        let run = format!(
            "printf '%s' '{}'; printf '%s' '{}' >&2; exit 1",
            "x".repeat(100_000),
            "é".repeat(210)
        );
        assert_eq!(
            fetch(&run).unwrap_err(),
            ProbeError::Command("é".repeat(200))
        );
    }
}
