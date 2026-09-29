# Control agents from the command line

`yinz agent` finds, launches and controls agents in terminal panes. Each target
must be an explicit pane ID, such as `terminal_7`. Commands never use the focused
pane as an implicit target.

Use `yinz --session development agent ...` to select a session. Without
`--session`, Yinz uses the current session or the only active session.

| Command | Result |
| --- | --- |
| `agent list` | Live agents, their identities and their reported states |
| `agent status TARGET` | State, reporting source, readiness, conversation and run IDs |
| `agent spawn --kind KIND` | Actual pane ID, working directory and branch |
| `agent prompt TARGET TEXT` | Acknowledgement that the agent accepted the prompt |
| `agent prompt TARGET --file PATH --wait` | Outcome of the submitted turn |
| `agent wait TARGET --until idle` | Observation of the requested state |
| `agent read TARGET --lines 80` | Last lines of terminal output |
| `agent send-keys TARGET Esc` | Deliberate input to the agent's terminal UI |

All commands accept `--json`. Targeted commands also accept
`--conversation ID`. Use the conversation ID from `agent status` to require the
same conversation on a later command. A pane ID alone does not preserve identity
between separate commands.

## Launch and submit work

For Codex, install Yinz's lifecycle hooks before launching:

```sh
yinz hocus hooks install --agent codex
```

Claude Code launches receive the hooks through the existing launch planner.

1. Launch an agent in a separate worktree:

   ```sh
   yinz --session development agent spawn \
     --kind codex --cwd . --branch review-auth --json
   ```

   Read `spawned.pane_id`, `spawned.branch` and `spawned.cwd` from the response.
   A branch collision can change the requested branch name. `--reuse` permits an
   existing checkout. `--allow-outside-worktree` permits a launch outside a
   linked Git worktree. Launches use the existing agent launch planner.
   Status retains the launch branch when known. It does not probe Git for later
   branch changes.

2. Wait for lifecycle reports to confirm that the agent is idle:

   ```sh
   yinz --session development agent wait terminal_7 --until idle --timeout 30s --json
   ```

   A successful spawn does not establish readiness. Startup, authentication or
   an initial question can prevent the agent from becoming ready.

3. Submit the task and wait for its outcome:

   ```sh
   yinz --session development agent prompt terminal_7 \
     --file review-request.md --wait --timeout 10m --json
   ```

   To pin a previously inspected conversation, add `--conversation ID`.

Direct text and standard input use the same submission path:

```sh
yinz agent prompt terminal_7 "Review the current diff"
git diff | yinz agent prompt terminal_7 --file - --wait --json
```

Prompts must contain UTF-8 text and fit within 1 MiB. Yinz converts CRLF line
endings to LF and removes trailing newlines before submission. It preserves
other spaces and internal newlines. Terminal control characters are rejected,
except tabs and newlines.

## Readiness and identity

The state vocabulary is `busy`, `waiting`, `idle` and `exited`. Reporting sources
distinguish launch metadata (`Spawned`), manual registration (`Registered`) and
agent reports (`Reported`). JSON retains these existing state and source names.

Status includes `status_available`, `ready` and `prompt_supported`. A launch
marker or a manual registration cannot establish lifecycle readiness. Missing
reports produce `status_unavailable`, rather than an inferred idle state.

Prompt submission initially supports Claude Code and Codex integrations with
lifecycle hooks. The terminal must support bracketed paste. Prompt submission
and key input require a verified agent command pane. An agent started manually
inside a shell can appear in listings without satisfying that requirement.

Yinz rejects prompts when the agent is busy, waiting or already has a pending
submission. Unconsumed manual input also makes the pane unready. There is no
prompt queue. The server registers observation before it writes the prompt.
A terminal write alone is not acceptance.
Readiness requires a known consumed input revision that matches the pane's current
revision. Startup reports establish that revision. A matching CLI prompt report
uses the revision captured when Yinz sent the prompt. Manual acceptance alone
cannot restore readiness because Yinz cannot identify which input revision it
consumed. Tool and completion reports do not resolve an unknown revision.
Input revisions restart at zero when the pane's process respawns, including when
scrollback is retained. Input typed after respawn still prevents startup reports
from establishing readiness.

A matching prompt report acknowledges acceptance. Codex correlation also
requires a provider turn ID. Claude Code can use conversation identity and the
prompt fingerprint when its hooks omit turn IDs. Conflicting input invalidates
the pending operation. An unrelated conversation cannot complete the wait.

## Outcomes and deadlines

Agent state and operation outcome are separate. An idle agent can have finished,
failed or been interrupted. For Claude Code and Codex, `agent wait --until idle`
requires reported idle state and prompt readiness. For other agents, including
OpenCode, matching lifecycle reports of idle state can satisfy the wait even
though `ready` and `prompt_supported` remain false. Other state waits observe the
requested state. A state wait does not prove that a particular task completed
successfully.

| Outcome | Meaning |
| --- | --- |
| `ok` | The query, state wait or deliberate key operation succeeded |
| `submitted` | A matching report acknowledged prompt acceptance |
| `completed` | The acknowledged turn ended normally |
| `needs_input` | A person must answer a question or approve an action |
| `failed` | The agent reported failure, or the requested operation failed |
| `interrupted` | The turn was interrupted or input conflicted with the submission |
| `timed_out` | The observation deadline expired |
| `target_changed` | The original agent or conversation no longer occupies the target |
| `target_closed` | The target pane closed |
| `status_unavailable` | Lifecycle or process evidence is insufficient |
| `rejected` | Preconditions or request validation failed |

`completed` confirms the end of a turn. It does not verify the agent's claims,
code changes or test results. Automatic policy waits do not produce
`needs_input` unless a person must respond.

The default deadline is 30 seconds. `prompt --wait` defaults to 10 minutes.
`--timeout` accepts durations from `1ms` to `24h` and includes acceptance time.
Timeout does not cancel work already submitted or prove that the work failed.
Yinz keeps an unresolved submission claim until lifecycle reports resolve it or
the target changes. New prompts are rejected while that claim remains. Manual
input ends the caller's wait as `interrupted`, but it cannot recall a submission
already in transit. Its claim can remain until the original turn is resolved.
A retry cannot take ownership of that turn's completion.
Inspect status and output before deciding what to do next. Requests are never
retried automatically after an uncertain reply.

Exit status is `0` for `ok`, `submitted` and `completed`. Other operation outcomes
return `1`. CLI syntax errors return `2`. With `--json`, inspect `outcome`,
`acknowledged`, `turn_id` and `message`. Connection and local input errors go to
standard error and return `1`.

`agent read` returns the latest retained terminal lines, including output below
a scrolled viewport. Reading does not change the viewport or selection.

When intervention is necessary, inspect the output before sending keys:

```sh
yinz agent read terminal_7 --conversation ID --lines 80
yinz agent send-keys terminal_7 --conversation ID Esc
```

Key input does not imply a new prompt acknowledgement. Use `agent status` or
`agent wait` to observe what happens next.
