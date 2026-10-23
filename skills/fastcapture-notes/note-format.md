# The finding note format

A finding is one Markdown file in the notebook's inbox folder, UTF-8, with YAML front matter.

## File name

`<inbox>\<title>.md`. The title with `\ / : * ? " < > |` and control characters replaced by spaces, at most 80 characters, without trailing dots or spaces. If that leaves a name Windows reserves (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, in any case), add ` note`. If the name is taken, add ` 2`, ` 3`… before `.md`.

## Front matter

```yaml
---
type: finding
finding: dead-end
captured: 2026-10-23T14:05
author: claude-code
model: "claude-opus-5-5"
repo: "C:/Projects/Payments"
remote: "github.com/acme/payments"
branch: main
commit: 3f9a1c2
files: ["src/ledger/batch.rs:120-180", "config/kafka.toml"]
tags: [kafka, payments]
summary: "Raising max.poll.records made the lag worse: each poll then outlasts max.poll.interval.ms."
status: inbox
---
```

| Field | Required | Value |
|---|---|---|
| `type` | yes | Always `finding`. |
| `finding` | yes | `investigation`, `decision`, `gotcha`, `how-to` or `dead-end` |
| `captured` | yes | Local time, `YYYY-MM-DDTHH:MM`, from a command, never guessed: `date +%Y-%m-%dT%H:%M` or `Get-Date -Format yyyy-MM-ddTHH:mm` |
| `author` | yes | The agent: `claude-code`, `opencode`, `copilot`, `codex`, … |
| `summary` | yes | One sentence, in double quotes, that answers the question on its own |
| `status` | yes | Always `inbox`. The user files it. |
| `model` | no | The model's ID, in double quotes |
| `repo` | when in a repo | The repo's root, `/` separators, in double quotes |
| `remote` | no | The `origin` remote, without the scheme or `.git` |
| `branch`, `commit` | when in a repo | `git branch --show-current`, `git rev-parse --short HEAD` |
| `files` | when about code | Paths relative to `repo`, with `:line` or `:from-to` when it helps |
| `tags` | no | A few lowercase words |

Don't write `suggested_home:`, `capture_id:` or `source:`. FastCapture, when it's installed, adds the home, and a finding has no raw source.

Strings that hold `:` or `#`, or that start with `[`, `{` or `'`, go in double quotes. Inside double quotes, never write `"` or `\`: the apps that read the notebook don't undo escapes, so they would show the backslashes. Write `'` instead of `"`, and `/` in paths.

## Body

```markdown
# <The finding, stated as a claim>

> <The summary again>

<The sections for its kind, below>

## Related

- [[Ledger batch]]
- [[Kafka consumer settings]]
```

- Under `## Related`, link only notes you actually found in the notebook, by their title or path without `.md`: `[[Ledger batch]]` or `[[Payments/Ledger batch]]`. Leave the section out when you found none.
- Code references are `path:line` relative to the repo, so they stay valid on another machine.
- Commands go in fenced blocks, with the shell named (` ```powershell `, ` ```sh `).
- Keep it short: what you'd want to read in six months, not a log of the session.

### investigation

```markdown
## Question
## What's going on
## Evidence
## Open questions
```

`Evidence` holds what showed it: the measurement, the log line, the code path. Leave `Open questions` out when there are none.

### decision

```markdown
## Context
## Decision
## Why
## Rejected
```

`Rejected` lists each alternative with the reason it lost.

### gotcha

```markdown
## The trap
## Why it happens
## How to avoid it
```

### how-to

```markdown
## When you need this
## Steps
## Check it worked
```

### dead-end

```markdown
## What we tried
## What happened
## Why it failed
## Try instead
```

Leave `Try instead` out when you don't know. A dead end is worth keeping even without an alternative.

## Example

```markdown
---
type: finding
finding: dead-end
captured: 2026-10-23T14:05
author: claude-code
model: "claude-opus-5-5"
repo: "C:/Projects/Payments"
branch: main
commit: 3f9a1c2
files: ["src/ledger/batch.rs:120-180", "config/kafka.toml"]
tags: [kafka, payments]
summary: "Raising max.poll.records made the lag worse: each poll then outlasts max.poll.interval.ms."
status: inbox
---

# Raising max.poll.records makes the ledger batch's lag worse

> Raising max.poll.records made the lag worse: each poll then outlasts max.poll.interval.ms.

## What we tried

Raised `max.poll.records` from 500 to 5000 in `config/kafka.toml`, to drain the backlog faster during the nightly ledger batch.

## What happened

Lag went from ~40k to ~110k, and the consumer group rebalanced every few minutes.

## Why it failed

`src/ledger/batch.rs:120-180` handles a poll's records one at a time, ~80 ms each. 5000 records take ~400 s, more than `max.poll.interval.ms` (300 s), so the broker drops the consumer and the group rebalances.

## Try instead

Keep 500 and process each poll's records in parallel, or raise `max.poll.interval.ms` along with it.

## Related

- [[Ledger batch]]
```
