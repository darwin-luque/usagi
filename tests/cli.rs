use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Sandbox {
    directory: PathBuf,
    config: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "usagi-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config = directory.join("config.toml");
        Self { directory, config }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_usagi"));
        for name in [
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "PI_CODING_AGENT_DIR",
            "HERMES_HOME",
        ] {
            command.env_remove(name);
        }
        command
            .env("HOME", &self.directory)
            .env("PATH", "/usr/bin:/bin")
            .env("USAGI_CONFIG", &self.config)
            .env("NO_COLOR", "1")
            .args(args)
            .output()
            .unwrap()
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.config).unwrap()
    }

    fn success(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn add_creates_config_and_rejects_duplicate_ids() {
    let sandbox = Sandbox::new();
    sandbox.success(&["add", "codex"]);
    let text = sandbox.text();
    assert!(text.contains("id = \"codex\""));
    assert!(text.contains("kind = \"codex\""));
    let output = sandbox.run(&["add", "codex"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Use --id"));
    assert_eq!(sandbox.text(), text);
    sandbox.success(&["add", "codex", "--id", "work"]);
    assert!(sandbox.text().contains("id = \"work\""));
}

#[test]
fn color_sets_and_restores_the_default() {
    let sandbox = Sandbox::new();
    sandbox.success(&["add", "codex", "--color", "#123456"]);
    assert!(sandbox.text().contains("color = \"#123456\""));
    sandbox.success(&["color", "codex", "#AABBCC"]);
    assert!(sandbox.text().contains("color = \"#AABBCC\""));
    sandbox.success(&["color", "codex", "default"]);
    assert!(!sandbox.text().contains("color ="));
    let before = sandbox.text();
    assert_eq!(
        sandbox.run(&["color", "codex", "bad"]).status.code(),
        Some(2)
    );
    assert_eq!(sandbox.text(), before);
}

#[test]
fn disable_enable_and_remove_update_the_config() {
    let sandbox = Sandbox::new();
    sandbox.success(&["add", "codex"]);
    sandbox.success(&["disable", "codex"]);
    assert!(sandbox.text().contains("enabled = false"));
    assert_eq!(
        sandbox.run(&["usage", "--only", "codex"]).status.code(),
        Some(1)
    );
    sandbox.success(&["enable", "codex"]);
    assert!(!sandbox.text().contains("enabled = false"));
    sandbox.success(&["remove", "codex"]);
    assert!(!sandbox.text().contains("id = \"codex\""));
    assert_eq!(sandbox.run(&["remove", "codex"]).status.code(), Some(2));
}

#[test]
fn list_shows_harnesses_and_all_supported_kinds() {
    let sandbox = Sandbox::new();
    let output = sandbox.success(&["list"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("No config file. Run usagi init"));
    sandbox.success(&["add", "codex", "--id", "work", "--color", "#123456"]);
    sandbox.success(&["disable", "work"]);
    let output = sandbox.success(&["list"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("● work         Codex            #123456  off"));
    assert!(!text.contains('\u{1b}'));
    for kind in [
        "claude",
        "codex",
        "cursor",
        "copilot",
        "opencode-go",
        "pi",
        "hermes",
        "command",
    ] {
        assert!(
            text.lines()
                .any(|line| line.starts_with(&format!("  ● {kind} ")))
        );
    }
    assert!(text.contains("Reads the Codex login"));
    assert!(text.contains("absent"));
}

#[test]
fn config_path_and_show_print_the_selected_file() {
    let sandbox = Sandbox::new();
    let output = sandbox.success(&["config", "path"]);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        sandbox.config.to_str().unwrap()
    );
    sandbox.success(&["add", "codex"]);
    let output = sandbox.success(&["config", "show"]);
    assert_eq!(String::from_utf8(output.stdout).unwrap(), sandbox.text());
}

#[test]
fn invalid_arguments_and_unknown_ids_fail_with_help() {
    let sandbox = Sandbox::new();
    sandbox.success(&["add", "codex"]);
    for args in [
        vec!["usage", "--raw", "--json"],
        vec!["usage", "--live", "--raw"],
        vec!["usage", "--live", "--json"],
        vec!["usage", "--every", "15"],
        vec!["live", "--every", "14"],
        vec!["usage", "--live", "--every", "0"],
        vec!["add", "command"],
        vec!["add", "codex", "--run", "echo {}"],
    ] {
        assert_eq!(sandbox.run(&args).status.code(), Some(2), "{args:?}");
    }
    let output = sandbox.run(&["add", "unknown"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Valid kinds: claude, codex"));
    let output = sandbox.run(&["usage", "--only", "missing"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Known ids: codex"));
}

#[test]
fn raw_usage_reports_failures_on_stderr() {
    let sandbox = Sandbox::new();
    sandbox.success(&["add", "command", "--run", "echo broken >&2; exit 3"]);
    let output = sandbox.run(&["usage", "--raw"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("usagi: command: "));
}

#[test]
fn init_refuses_to_replace_config_without_force() {
    let sandbox = Sandbox::new();
    sandbox.success(&["add", "codex"]);
    let before = sandbox.text();
    let output = sandbox.run(&["init"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--force"));
    assert_eq!(sandbox.text(), before);
    let output = sandbox.success(&["init", "--force"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains(sandbox.config.to_str().unwrap()));
    assert!(!sandbox.text().contains("id = \"codex\""));
}

#[test]
fn raw_usage_reads_command_json() {
    let sandbox = Sandbox::new();
    sandbox.success(&[
        "add",
        "command",
        "--run",
        "echo '{\"plan\":\"Test\",\"windows\":[{\"span\":\"5h\",\"used_percent\":40}]}'",
    ]);
    let output = sandbox.success(&["usage", "--raw"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("40"));
    assert!(text.contains("5h"));
}

#[test]
fn config_edit_uses_visual_and_checks_the_saved_file() {
    let sandbox = Sandbox::new();
    let script = sandbox.directory.join("editor.sh");
    std::fs::write(&script, "printf 'refresh_seconds = 1\\n' > \"$1\"\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_usagi"))
        .env("USAGI_CONFIG", &sandbox.config)
        .env("VISUAL", format!("sh '{}'", script.display()))
        .env("EDITOR", "false")
        .args(["config", "edit"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("refresh_seconds must be 15 or more"));
    assert_eq!(sandbox.text(), "refresh_seconds = 1\n");
}

#[test]
fn breakdown_json_adds_local_claude_and_codex_tokens() {
    let sandbox = Sandbox::new();
    for kind in ["claude", "codex", "copilot"] {
        sandbox.success(&["add", kind]);
    }
    let time = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let claude = sandbox.directory.join(".claude/projects/demo");
    std::fs::create_dir_all(&claude).unwrap();
    let message = format!(
        r#"{{"type":"assistant","requestId":"r1","timestamp":"{time}","message":{{"id":"m1","model":"claude-opus-5-5","usage":{{"input_tokens":2,"output_tokens":10,"cache_read_input_tokens":300,"cache_creation_input_tokens":40}}}}}}"#
    );
    std::fs::write(claude.join("s.jsonl"), format!("{message}\n{message}\n")).unwrap();
    let codex = sandbox.directory.join(".codex/sessions/2026/10/29");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(
        codex.join("rollout-demo.jsonl"),
        format!(
            "{}\n{}\n",
            r#"{"type":"turn_context","payload":{"model":"gpt-6.1-sol"}}"#,
            format_args!(
                r#"{{"timestamp":"{time}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":1000,"cached_input_tokens":400,"cache_write_input_tokens":0,"output_tokens":50}}}}}}}}"#
            )
        ),
    )
    .unwrap();
    let output = sandbox.success(&[
        "breakdown",
        "--only",
        "claude",
        "--only",
        "codex",
        "--only",
        "copilot",
        "--json",
    ]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["total"]["tokens"],
        serde_json::json!({"input":602,"output":60,"cache_read":700,"cache_write":40,"total":1402})
    );
    assert_eq!(value["rows"][0]["label"], "gpt-6.1-sol");
    assert_eq!(value["rows"][1]["label"], "claude-opus-5-5");
    let statuses: Vec<_> = value["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|harness| {
            (
                harness["id"].as_str().unwrap(),
                harness["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        [
            ("claude", "ok"),
            ("codex", "ok"),
            ("copilot", "unsupported")
        ]
    );
    let output = sandbox.success(&["breakdown", "--by", "day", "--past", "2d"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Breakdown  last 2 days  tokens"));
    assert!(text.contains("By hour"));
    assert!(text.contains("No token history for copilot."));
    let output = sandbox.run(&["breakdown", "--past", "2fortnights"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("24h, 30d"));
}

#[test]
fn usage_without_config_shows_detection_guidance() {
    let sandbox = Sandbox::new();
    for args in [vec![], vec!["usage"], vec!["live"]] {
        let output = sandbox.run(&args);
        assert_eq!(output.status.code(), Some(1));
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(
            "No config file. usagi shows the harnesses it found. Run usagi init to save them."
        ));
        assert!(
            error
                .contains("usagi found no harness. Run usagi list to see the supported harnesses.")
        );
        assert!(!sandbox.config.exists());
    }
}
