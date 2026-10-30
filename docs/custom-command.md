# Custom command

The `command` kind connects any harness to usagi. usagi runs your command and reads usage JSON from its standard output. It uses the same JSON shape as `usagi usage --json`.

## Add a command harness

```sh
usagi add command --id gemini --run "~/bin/gemini-usage" --color "#4285F4"
```

usagi runs the command with `sh -c`. The command must stop in 20 seconds and exit with status 0.

## Output shape

```json
{
  "plan": "Pro",
  "windows": [
    { "span": "5h", "used_percent": 22.5, "resets_at": "2026-10-30T06:10:00Z" },
    { "span": "week", "scope": "opus", "used_percent": 61 },
    { "span": "month", "spend_usd": 4.2, "limit_usd": 20, "rolling": true }
  ],
  "notes": ["Text that usagi shows under the bars."]
}
```

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `plan` | string | No | The plan name. usagi shows it next to the title. |
| `windows` | array | No | One entry for each limit. |
| `windows[].span` | string | Yes | `5h`, `day`, `week`, `month`, or a number with `m`, `h`, or `d` (for example `90m`). |
| `windows[].scope` | string | No | A label for a part of the plan, for example a model name. |
| `windows[].used_percent` | number | One of the two | The used part of the limit, from 0 to 100. |
| `windows[].spend_usd` | number | One of the two | The dollars used in the window. If both fields are present, usagi uses this one. |
| `windows[].limit_usd` | number | No | The dollar limit. With `spend_usd`, usagi calculates the percent. |
| `windows[].resets_at` | string | No | The reset time in RFC 3339 format. |
| `windows[].rolling` | boolean | No | `true` if the window moves with time and has no fixed reset. |
| `notes` | array of strings | No | Extra lines to show. |

## Example: OpenRouter credit

This example is not verified by usagi. It uses the OpenRouter key endpoint, which returns the spend and limit of an API key.

```sh
#!/bin/sh
curl -s https://openrouter.ai/api/v1/key -H "Authorization: Bearer $OPENROUTER_API_KEY" |
  jq '{plan: "OpenRouter", windows: [{span: "month", spend_usd: .data.usage_monthly, limit_usd: .data.limit}]}'
```
