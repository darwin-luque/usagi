mod config;
mod kinds;
mod probe;
mod render;
mod sources;
mod theme;
mod usage;

use std::io::IsTerminal;
use std::path::Path;
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use config::{Config, Harness, Source};
use sources::Detection;
use theme::Rgb;
use usage::Outcome;

#[derive(Parser)]
#[command(
    name = "usagi",
    version,
    about = "Show the usage limits of your harnesses."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    #[command(about = "Show the usage of your harnesses now.")]
    Usage(UsageArgs),
    #[command(about = "Show usage and keep it up to date.")]
    Live(LiveArgs),
    #[command(about = "Find harnesses and save a config file.")]
    Init {
        #[arg(long, help = "Replace the config file.")]
        force: bool,
    },
    #[command(about = "Show configured and supported harnesses.")]
    List,
    #[command(about = "Add a harness to the config file.")]
    Add {
        kind: String,
        #[arg(long, help = "Set the harness id.")]
        id: Option<String>,
        #[arg(long, help = "Set the color in #RRGGBB format.")]
        color: Option<Rgb>,
        #[arg(long, help = "Set the command that reads usage.")]
        run: Option<String>,
    },
    #[command(about = "Remove a harness from the config file.")]
    Remove { id: String },
    #[command(about = "Enable a harness.")]
    Enable { id: String },
    #[command(about = "Disable a harness.")]
    Disable { id: String },
    #[command(about = "Set a harness color or restore its default.")]
    Color { id: String, color: String },
    #[command(about = "Read or edit the config file.")]
    Config {
        #[command(subcommand)]
        command: ConfigAction,
    },
}

#[derive(Args, Default)]
struct UsageArgs {
    #[arg(long, conflicts_with_all = ["raw", "json"], help = "Keep usage up to date.")]
    live: bool,
    #[arg(long, conflicts_with = "json", help = "Print raw usage.")]
    raw: bool,
    #[arg(long, help = "Print usage as JSON.")]
    json: bool,
    #[arg(long, help = "Show only this harness id.")]
    only: Vec<String>,
    #[arg(long, requires = "live", value_parser = parse_seconds, help = "Set the time between updates in seconds.")]
    every: Option<u64>,
    #[arg(long, help = "Turn off color output.")]
    no_color: bool,
}

#[derive(Args)]
struct LiveArgs {
    #[arg(long, help = "Show only this harness id.")]
    only: Vec<String>,
    #[arg(long, value_parser = parse_seconds, help = "Set the time between updates in seconds.")]
    every: Option<u64>,
}

#[derive(Subcommand)]
enum ConfigAction {
    #[command(about = "Show the config file path.")]
    Path,
    #[command(about = "Show the config file text.")]
    Show,
    #[command(about = "Open the config file in your editor.")]
    Edit,
}

fn parse_seconds(text: &str) -> std::result::Result<u64, String> {
    text.parse::<u64>()
        .ok()
        .filter(|seconds| *seconds >= config::MIN_REFRESH_SECONDS)
        .ok_or_else(|| format!("Use {} seconds or more.", config::MIN_REFRESH_SECONDS))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match execute(cli.command.unwrap_or(Action::Usage(UsageArgs::default()))) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("usagi: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn execute(action: Action) -> Result<u8> {
    let path = config::path();
    match action {
        Action::Usage(args) => return show_usage(&path, args),
        Action::Live(args) => {
            return show_usage(
                &path,
                UsageArgs {
                    live: true,
                    only: args.only,
                    every: args.every,
                    ..UsageArgs::default()
                },
            );
        }
        Action::Init { force } => {
            if path.try_exists()? && !force {
                bail!("The config file exists. Use --force to replace it.");
            }
            let config = detect_config(true);
            config.save(&path)?;
            println!(
                "Saved {} harnesses to {}",
                config.harnesses.len(),
                path.display()
            );
        }
        Action::List => list(&path)?,
        Action::Add {
            kind,
            id,
            color,
            run,
        } => {
            let info = kinds::find(&kind).with_context(|| {
                format!(
                    "Unknown kind \"{kind}\". Valid kinds: {}.",
                    kinds::KINDS
                        .iter()
                        .map(|kind| kind.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            let mut source = (info.default_source)();
            if kind == "command" {
                let run = run
                    .filter(|run| !run.trim().is_empty())
                    .context("The command kind needs --run.")?;
                source = Source::Command { run };
            } else if run.is_some() {
                bail!("Use --run only with the command kind.");
            }
            let mut harness = Harness::new(source);
            if let Some(id) = id {
                harness.id = id;
            }
            harness.color = color;
            let mut config = Config::load(&path)?.unwrap_or_default();
            let id = harness.id.clone();
            config.add(harness)?;
            config.save(&path)?;
            println!("Added {id}.");
        }
        Action::Remove { id } => {
            let mut config = load_required(&path)?;
            config.remove(&id)?;
            config.save(&path)?;
            println!("Removed {id}.");
        }
        Action::Enable { id } => set_enabled(&path, &id, true)?,
        Action::Disable { id } => set_enabled(&path, &id, false)?,
        Action::Color { id, color } => {
            let mut config = load_required(&path)?;
            config.harness_mut(&id)?.color = if color == "default" {
                None
            } else {
                Some(color.parse::<Rgb>().map_err(anyhow::Error::msg)?)
            };
            config.save(&path)?;
        }
        Action::Config { command } => match command {
            ConfigAction::Path => println!("{}", path.display()),
            ConfigAction::Show => print!(
                "{}",
                std::fs::read_to_string(&path)
                    .with_context(|| format!("Cannot read {}", path.display()))?
            ),
            ConfigAction::Edit => edit(&path)?,
        },
    }
    Ok(0)
}

fn load_required(path: &Path) -> Result<Config> {
    Config::load(path)?.context("No config file. Run usagi init to save one.")
}

fn set_enabled(path: &Path, id: &str, enabled: bool) -> Result<()> {
    let mut config = load_required(path)?;
    config.harness_mut(id)?.enabled = enabled;
    config.save(path)
}

struct Paint(bool);

impl Paint {
    fn rgb(&self, color: Rgb, text: &str) -> String {
        let Rgb(r, g, b) = color;
        if self.0 {
            format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn dim(&self, text: &str) -> String {
        if self.0 {
            format!("\x1b[2m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn detection(&self, detection: &Detection) -> String {
        let theme = theme::Theme::default();
        match detection {
            Detection::Ready(_) => self.rgb(theme.calm, &format!("{:<8}", "ready")),
            Detection::Partial(_) => self.rgb(theme.warn, &format!("{:<8}", "partial")),
            Detection::Absent => self.dim(&format!("{:<8}", "absent")),
        }
    }
}

fn detection_detail(detection: &Detection) -> &str {
    match detection {
        Detection::Ready(detail) | Detection::Partial(detail) => detail,
        Detection::Absent => "Not found.",
    }
}

fn detect_config(print: bool) -> Config {
    let paint = Paint(use_color(false));
    let mut config = Config::default();
    for kind in kinds::KINDS.iter().filter(|kind| kind.name != "command") {
        let source = (kind.default_source)();
        let detection = sources::detect(&source);
        if print {
            println!(
                "  {} {:<12} {:<16} {} {}",
                paint.rgb(kind.color, "●"),
                kind.name,
                kind.title,
                paint.detection(&detection),
                paint.dim(detection_detail(&detection))
            );
        }
        if matches!(detection, Detection::Ready(_)) {
            config.harnesses.push(Harness::new(source));
        }
    }
    config
}

fn use_color(no_color: bool) -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() && !no_color
}

fn list(path: &Path) -> Result<()> {
    let paint = Paint(use_color(false));
    match Config::load(path)? {
        Some(config) => {
            println!("Harnesses in {}", path.display());
            if config.harnesses.is_empty() {
                println!("  {}", paint.dim("None. Use usagi add <kind> to add one."));
            }
            for harness in &config.harnesses {
                let state = if harness.enabled { "on" } else { "off" };
                println!(
                    "  {} {:<12} {:<16} {}  {}",
                    paint.rgb(harness.color(), "●"),
                    harness.id,
                    harness.source.kind().title,
                    paint.rgb(harness.color(), &harness.color().to_string()),
                    if harness.enabled {
                        state.to_string()
                    } else {
                        paint.dim(state)
                    }
                );
            }
        }
        None => println!("No config file. Run usagi init to save one."),
    }
    println!();
    println!("Supported kinds");
    for kind in kinds::KINDS {
        let detection = sources::detect(&(kind.default_source)());
        println!(
            "  {} {:<12} {:<16} {} {}",
            paint.rgb(kind.color, "●"),
            kind.name,
            kind.title,
            paint.detection(&detection),
            paint.dim(kind.method)
        );
    }
    Ok(())
}

fn show_usage(path: &Path, args: UsageArgs) -> Result<u8> {
    let mut config = match Config::load(path)? {
        Some(config) => config,
        None => {
            eprintln!(
                "No config file. usagi shows the harnesses it found. Run usagi init to save them."
            );
            let config = detect_config(false);
            if config.harnesses.is_empty() {
                eprintln!("usagi found no harness. Run usagi list to see the supported harnesses.");
                return Ok(1);
            }
            config
        }
    };
    if let Some(seconds) = args.every {
        config.refresh_seconds = seconds;
    }
    let harnesses = config.enabled(&args.only)?;
    if args.live {
        render::live::run(&config, harnesses)?;
        return Ok(0);
    }
    let reports = probe::probe_all(&harnesses);
    let output = if args.raw {
        for report in &reports {
            if let Outcome::Error(error) = &report.outcome {
                eprintln!("usagi: {}: {error}", report.id);
            }
        }
        render::raw::render(&reports)
    } else if args.json {
        render::json::render(&reports)
    } else {
        render::plain::render(&reports, &config.theme, use_color(args.no_color))
    };
    print!("{output}");
    Ok(
        if reports
            .iter()
            .any(|report| matches!(report.outcome, Outcome::Usage(_)))
        {
            0
        } else {
            1
        },
    )
}

fn edit(path: &Path) -> Result<()> {
    if !path.try_exists()? {
        Config::default().save(path)?;
    }
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".into());
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("usagi-editor")
        .arg(path)
        .status()
        .context("Cannot open the editor.")?;
    if !status.success() {
        bail!("The editor fails. Check the config file.");
    }
    load_required(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_accepts_the_minimum_refresh_time_and_repeated_ids() {
        let cli = Cli::try_parse_from([
            "usagi", "live", "--every", "15", "--only", "work", "--only", "home",
        ])
        .unwrap();
        let Some(Action::Live(args)) = cli.command else {
            panic!("Expected the live command");
        };
        assert_eq!(args.every, Some(config::MIN_REFRESH_SECONDS));
        assert_eq!(args.only, ["work", "home"]);
        assert!(Cli::try_parse_from(["usagi", "live", "--every", "14"]).is_err());
    }
}
