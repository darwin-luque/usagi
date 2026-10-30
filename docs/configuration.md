# Configuration

usagi reads one TOML file. The default path is `~/.config/usagi/config.toml`. `XDG_CONFIG_HOME` changes the base folder. `USAGI_CONFIG` sets the full path. `usagi config path` prints the path in use.

The `usagi init`, `add`, `remove`, `enable`, `disable`, and `color` commands write this file for you. You can also edit it with `usagi config edit`. usagi checks the file after the editor closes.

## Example

```toml
refresh_seconds = 60

[theme]
bar = "blocks"
warn_at = 75.0
danger_at = 90.0
show_pace = true

[[harness]]
id = "claude"
kind = "claude"

[[harness]]
id = "codex"
kind = "codex"
color = "#22C55E"

[[harness]]
id = "go"
kind = "opencode-go"

[harness.limits]
five_hours = 12.0
week = 30.0

[[harness]]
id = "gemini"
kind = "command"
run = "~/bin/gemini-usage"
enabled = false
```

## Top-level keys

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `refresh_seconds` | integer | `60` | The time between two checks in live mode. The minimum is 15. |
| `theme` | table | See below | The look of the bars. |
| `harness` | array of tables | Empty | The harnesses to check, in display order. |

## Theme keys

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `bar` | `"blocks"`, `"line"`, or `"dots"` | `"blocks"` | The bar characters. |
| `warn_at` | number | `75.0` | At this percent and higher, the percent text uses the `warn` color. |
| `danger_at` | number | `90.0` | At this percent and higher, the percent text uses the `danger` color. |
| `calm` | color | `"#98C379"` | The percent color under `warn_at`. |
| `warn` | color | `"#E5C07B"` | The percent color from `warn_at`. |
| `danger` | color | `"#E06C75"` | The percent color from `danger_at`. Errors also use it. |
| `track` | color | `"#3B3F4A"` | The color of the empty part of a bar. |
| `show_pace` | boolean | `true` | Shows a `│` mark on the bar at the part of the window that passed. If the bar fill is past the mark, you use the limit faster than the window passes. |

A color is a string in the format `#RRGGBB`.

## Harness keys

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | Required | A unique name. Commands such as `usagi remove <id>` use it. |
| `kind` | string | Required | The source type. See the [harness map](harnesses.md). |
| `color` | color | The kind color | The bar and title color. |
| `enabled` | boolean | `true` | `false` keeps the entry but skips it. |

Each kind also accepts these keys:

| Kind | Key | Meaning |
| --- | --- | --- |
| `claude` | `credentials` | The path of a Claude credentials JSON file. |
| `codex` | `home` | The Codex folder. The default is `CODEX_HOME` or `~/.codex`. |
| `cursor` | `state_db` | The path of the Cursor app `state.vscdb` file. |
| `copilot` | `token_command` | A command that prints a GitHub token. |
| `opencode-go` | `db`, `limits` | The OpenCode database path and the dollar limits. |
| `pi` | `sessions`, `limits` | The Pi sessions folder and the dollar limits. |
| `hermes` | `db`, `limits` | The Hermes database path and the dollar limits. |
| `command` | `run` | The command to run. Required. |

## Limits

`limits` is a table with the keys `five_hours`, `day`, `week`, and `month`. Each value is a dollar amount. usagi shows a bar only for the windows that have a limit. If `limits` is empty, usagi shows the spend for all four windows with no bar.

## Default colors

| Kind | Color |
| --- | --- |
| `claude` | `#D97757` |
| `codex` | `#10A37F` |
| `cursor` | `#4F8EF7` |
| `copilot` | `#8957E5` |
| `opencode-go` | `#F2C94C` |
| `pi` | `#C678DD` |
| `hermes` | `#56B6C2` |
| `command` | `#ABB2BF` |
