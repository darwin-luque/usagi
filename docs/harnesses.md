# Harness map

This page lists how each AI harness exposes its usage limits, and how usagi reads them. usagi never sends a prompt to a model. It reads local login files and calls usage endpoints only, so a check costs zero tokens.

The harnesses do not share one usage format. usagi converts each format into one normalized shape: a list of windows, each with a span (`5h`, `day`, `week`, `month`), an optional scope, a used percent or a dollar spend, and a reset time. See [Custom command](custom-command.md) for the shape.

## Status values

| Status | Meaning |
| --- | --- |
| Built in, verified | usagi has a source for it. The source was tested against the live service on 2026-10-29. |
| Built in, not verified | usagi has a source for it. The source follows the public format, but no live account was available for a test. |
| Use `command` | usagi has no source for it. Write a small script and add it with the `command` kind. |

## Summary

| Harness | Kind | Status | Windows | Where the numbers come from |
| --- | --- | --- | --- | --- |
| Claude Code | `claude` | Built in, verified | 5h, week, week per model, extra usage | Anthropic OAuth usage endpoint |
| Codex | `codex` | Built in, verified | 5h, week | ChatGPT usage endpoint |
| Cursor and cursor-agent | `cursor` | Built in, verified | month (billing cycle): total, auto, api | Cursor dashboard endpoint |
| GitHub Copilot | `copilot` | Built in, verified | month per quota | Copilot user endpoint |
| OpenCode Go | `opencode-go` | Built in, verified | 5h, week, month in dollars | Local OpenCode database |
| Pi | `pi` | Built in, not verified | 5h, day, week, month in dollars | Local Pi session files |
| Hermes Agent | `hermes` | Built in, not verified | 5h, day, week, month in dollars | Local Hermes database |
| Gemini CLI | `command` | Use `command` | day per model | Google Code Assist quota endpoint |
| Any other harness | `command` | Use `command` | Any | Your script |

## Claude Code

- **Kind:** `claude`
- **Credentials:** the macOS Keychain item `Claude Code-credentials`. On Linux and Windows, `~/.claude/.credentials.json`. `CLAUDE_CONFIG_DIR` moves that file. The config option `credentials` sets a file path.
- **Request:** `GET https://api.anthropic.com/api/oauth/usage` with the header `anthropic-beta: oauth-2025-04-20`.
- **Response:** `five_hour` and `seven_day` objects with `utilization` (percent) and `resets_at`. Plans with model limits also send `seven_day_opus` or `seven_day_sonnet`. `extra_usage` shows paid extra usage.
- **Login expiry:** Claude Code refreshes its own token. usagi does not refresh it. If the token expired, run `claude` once.
- **Token history:** `usagi breakdown` reads the session files in `~/.claude/projects/**/*.jsonl`, subagent files included. `CLAUDE_CONFIG_DIR` moves the folder. Each assistant message has `message.usage`. Claude Code writes a message once per content block, so usagi counts each `message.id` and `requestId` pair once. The files have no cost, so usagi estimates it from public API prices.
- **Known gap:** usagi prices all cache writes at the 5-minute cache rate. Claude Code often writes to the 1-hour cache, which costs more (`message.usage.cache_creation.ephemeral_1h_input_tokens`). So the Claude cost estimate is low when a session uses the 1-hour cache. A fix would count 1-hour cache writes on their own and price them with `cache_creation_input_token_cost_above_1hr` from the price list.

## Codex

- **Kind:** `codex`
- **Credentials:** `~/.codex/auth.json`. `CODEX_HOME` moves it. The config option `home` sets the folder.
- **Request:** `GET https://chatgpt.com/backend-api/wham/usage` with the `ChatGPT-Account-Id` header.
- **Response:** `rate_limit.primary_window` (5h) and `rate_limit.secondary_window` (week). Each has `used_percent`, `limit_window_seconds`, and `reset_at`. usagi gets the span from `limit_window_seconds`, so a plan change does not break it.
- **API key login:** an API key has no plan windows. usagi asks you to run `codex login` with a ChatGPT account.
- **Offline record:** Codex also writes the same numbers into `~/.codex/sessions/**/rollout-*.jsonl` after each turn. usagi uses the endpoint because it is current even when no session ran.
- **Token history:** `usagi breakdown` reads the `token_count` lines in the same rollout files. Each line has the running total of the session, so usagi counts the change from the line before. The model comes from the last `turn_context` line. The files have no cost, so usagi estimates it from public API prices.

## Cursor

- **Kind:** `cursor`
- **Credentials:** usagi reads two logins and uses the newer valid one.
  - The Cursor app: key `cursorAuth/accessToken` in `state.vscdb`. On macOS it is in `~/Library/Application Support/Cursor/User/globalStorage/`. On Linux it is in `~/.config/Cursor/User/globalStorage/`. The config option `state_db` sets the path.
  - cursor-agent: the macOS Keychain item `cursor-access-token`.
- **Request:** `POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage` with an empty JSON body.
- **Response:** `billingCycleStart`, `billingCycleEnd`, and `planUsage` with `totalPercentUsed`, `autoPercentUsed`, and `apiPercentUsed`. A missing number means zero.
- **Login expiry:** if both logins expired, open Cursor or run `cursor-agent` once.
- **Token history:** `usagi breakdown` sends `POST https://api2.cursor.sh/aiserver.v1.DashboardService/GetFilteredUsageEvents` with the same login and reads all pages. Each event has the model, `tokenUsage`, and the cost in cents. Events without `tokenUsage` are request-based calls. They count with zero tokens and their charged cost.

## GitHub Copilot

- **Kind:** `copilot`
- **Credentials:** `GH_TOKEN`, `GITHUB_TOKEN`, or the output of `gh auth token`. The config option `token_command` sets another command.
- **Request:** `GET https://api.github.com/copilot_internal/user`.
- **Response:** `quota_snapshots` with one entry per quota (`chat`, `completions`, `premium_interactions`). Each entry has `percent_remaining` and `unlimited`. `quota_reset_date` gives the monthly reset. usagi shows only quotas with a limit.
- **Token history:** none. Copilot has no token record that usagi can read, so `usagi breakdown` skips it with a note.

## OpenCode Go

- **Kind:** `opencode-go`
- **Why local:** OpenCode Go shows usage only in the web console at opencode.ai. It has no usage endpoint for the CLI.
- **Data:** OpenCode records the dollar cost of each message in `~/.local/share/opencode/opencode.db` (table `session_message`, or `message` in old versions). usagi adds the cost of each `opencode-go` message inside rolling windows.
- **Limits:** the default limits are $12 for 5 hours, $30 for a week, and $60 for a month. These are the values in the OpenCode Go docs. The newest docs show no monthly limit. If your plan changes, set new values under `limits`.
- **Accuracy:** the result is an estimate. It does not include usage from other computers.
- **Token history:** `usagi breakdown` reads every assistant message in the same database, from all providers. Each message has the model, `tokens`, and `cost`. Reasoning tokens count as output.

## Pi

- **Kind:** `pi`
- **Why local:** Pi has no plan of its own. It uses the plan or API key of its provider. To see Claude or ChatGPT plan limits, also add the `claude` or `codex` harness.
- **Data:** each assistant message in `~/.pi/agent/sessions/**/*.jsonl` has `message.usage.cost.total`. `PI_CODING_AGENT_DIR` moves the folder.
- **Limits:** none by default. usagi then shows your spend for 5h, day, week, and month. Set `limits` to see a percent.
- **Token history:** `usagi breakdown` reads `message.model` and `message.usage` (`input`, `output`, `cacheRead`, `cacheWrite`, `cost.total`) from the same files.

## Hermes Agent

- **Kind:** `hermes`
- **Why local:** Hermes uses the plan or API key of its provider.
- **Data:** table `sessions` in `~/.hermes/state.db` has `actual_cost_usd` and `estimated_cost_usd`. `HERMES_HOME` moves the folder. Hermes records cost per session, so a long session counts at its last activity time.
- **Limits:** none by default. Set `limits` to see a percent.
- **Token history:** `usagi breakdown` reads `model`, `input_tokens`, `output_tokens`, `reasoning_tokens`, `cache_read_tokens`, `cache_write_tokens`, and the cost from the same `sessions` table. A missing column counts as zero. Reasoning tokens count as output. Each session counts as one event at its last activity time.

## Gemini CLI

- **Kind:** use `command`.
- **Data:** Gemini CLI sends quota requests to `https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota` with the OAuth login in `~/.gemini/oauth_creds.json`. The response has one bucket per model with `remainingFraction` and `resetTime`. The quota resets each day.
- **Why not built in:** no Gemini login was available for a test. Write a script that sends this request and prints the normalized JSON.

## Other harnesses

For Amp, Factory Droid, Kiro, Windsurf, and others, usagi knows no usage endpoint. If a harness shows usage in a command or a file, wrap it with the `command` kind. `command` harnesses have no token history, so `usagi breakdown` skips them with a note.
