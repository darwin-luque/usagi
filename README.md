# usagi

usagi shows how much of your AI plan limits you used, for all your agent harnesses in one place. It reads Claude Code, Codex, Cursor, GitHub Copilot, OpenCode Go, Pi, Hermes Agent, and any harness that you connect with a script.

usagi spends zero model tokens. It reads the login files that your harnesses already have and calls their usage endpoints. It never sends a prompt.

```
● Claude Code  Pro
  5h          ██████▏░░░░░░░░░░░░░░░░░░░░░   22%  resets in 4h 47m
  week        ██████████▍░░░░░░░░░░░░│░░░░   37%  resets in 15h 34m

● Codex  Plus
  5h          █████████░░░░░░░░░░░░░░░░░░░   32%  resets in 4h 46m
  week        █▍░░░░░░░░░░░░░░░░░░░░░░░░░░    5%  resets in 6d 23h
```

## Install

usagi needs Rust 1.85 or later.

```sh
cargo install --git https://github.com/darwin-luque/usagi
```

To install from a local copy, run `cargo install --path .` in the repository folder.

Each [release](https://github.com/darwin-luque/usagi/releases) also has binaries for Linux, macOS, and Windows.

## Start

1. Find your harnesses and save them in the config file:

   ```sh
   usagi init
   ```

2. Show your usage now:

   ```sh
   usagi usage
   ```

3. Keep a live view open in the terminal:

   ```sh
   usagi live
   ```

   Press `r` to check now. Press `q` to quit.

## Commands

| Command | What it does |
| --- | --- |
| `usagi usage` | Shows the usage now. `usagi` with no command does the same. |
| `usagi usage --live`, `usagi live` | Opens the live view. It checks again every `refresh_seconds`. |
| `usagi usage --raw` | Prints one line per window: id, span, percent, and reset time as Unix seconds, with tabs between them. |
| `usagi usage --json` | Prints all data as JSON. |
| `usagi usage --only claude --only codex` | Checks only these harnesses. |
| `usagi init` | Finds the installed harnesses and writes the config file. |
| `usagi list` | Shows your harnesses and all supported kinds. |
| `usagi add <kind>` | Adds a harness. Use `--id`, `--color`, and `--run` (for `command`). |
| `usagi remove <id>` | Removes a harness. |
| `usagi enable <id>`, `usagi disable <id>` | Turns a harness on or off and keeps its settings. |
| `usagi color <id> <#RRGGBB>` | Sets the color of a harness. `default` sets the kind color again. |
| `usagi config path`, `show`, `edit` | Prints, shows, or edits the config file. |

## Use raw output in scripts

The raw output has one line per window:

```
claude	5h	22.0	1793340600
claude	week	37.0	1793393999
codex	5h	32.0	1793340649
```

For example, this command prints the Claude 5h percent:

```sh
usagi usage --raw --only claude | awk '$2 == "5h" { print $3 }'
```

## Documents

- [Harness map](docs/harnesses.md) tells how each harness exposes usage and how usagi reads it.
- [Configuration](docs/configuration.md) lists all config keys, themes, and limits.
- [Custom command](docs/custom-command.md) tells how to connect a harness that usagi does not know.

## Releases

usagi uses [semantic versioning](https://semver.org). All changes go to `main` through pull requests. The pull request title is the commit message, and it must follow [Conventional Commits](https://www.conventionalcommits.org):

| Title | Next version from 0.1.0 |
| --- | --- |
| `fix: ...` | 0.1.1 |
| `feat: ...` | 0.2.0 |
| `feat!: ...` or a `BREAKING CHANGE:` footer | 0.2.0 before 1.0, then the next major version |
| `docs: ...`, `chore: ...`, `test: ...`, `refactor: ...`, `ci: ...` | No release |

After each merge, [release-please](https://github.com/googleapis/release-please) opens or updates a release pull request. It changes the version in `Cargo.toml` and adds the changes to `CHANGELOG.md`. Merge that pull request to tag the version, publish the GitHub release, and attach the binaries.

## Privacy

usagi reads your login tokens only to call the usage endpoint of the same service. It does not write, refresh, or send tokens anywhere else. It does not store usage data.
