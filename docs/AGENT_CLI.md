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
| `agent recover TARGET --conversation ID --confirm-empty [--interrupted]` | Restore readiness after inspecting and clearing the draft |
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
Codex launches reject Claude model aliases such as `haiku`, `sonnet`, and `opus`.
Use `--kind claude` for these aliases. Other model IDs are validated by the configured provider.

`agent spawn --help` describes every launch option. Recognized kinds are
`muse`, `codex`, `claude`, `cursor`, `qwen`, `gemini`, `aider`, `cline`,
`opencode` and `pi`. Recognition does not imply prompt support.
Kind names ignore case. Recognized executable aliases, such as `cursor-agent`,
remain valid.

Use `--file PATH` for a JSON launch object, or `--file -` for standard input.
Flags override the corresponding JSON fields. JSON can also specify `command`,
`args`, `env` and `hooks`. The command must identify a recognized agent.
Spawn rejects opening `prompt` and `reuse_prompt` fields. Submit work after readiness.

```sh
printf '%s\n' '{"command":"/opt/bin/claude","args":[],"branch":"review"}' \
  | yinz agent spawn --file - --cwd . --json
```

`--project` selects a registered project. `--cwd` selects a directory within that
project, or a local directory when no project is specified.
Without either option, local launches use the current directory.
`--branch` creates a separate worktree by default, based on `main` under `.worktrees/`.
Use `--base` and `--worktree-dir` to override those defaults.
`--worktree false` switches the selected checkout instead.
`--model`, `--effort` and `--permission-mode` use the selected agent's supported values.
`--resume` accepts a conversation ID or `last`.

SSH launches use `--remote-host`, an absolute remote `--cwd`, and
`--allow-outside-worktree`. They require an existing remote checkout and reject
local project, branch, worktree and VM options.
`--firecracker` selects a named session VM. `--firecracker-image` selects its image.
A branch provides a host worktree for the VM. Without a branch or project,
the VM uses its guest workspace.

### Run `yinz` from inside a Codex agent

Codex runs shell commands in its shared daemon rather than in the pane. Yinz's
Codex bridge passes the pane's `YINZ_SESSION_NAME`, `YINZ_PANE_ID` and
`YINZ_PANE_REF` to each conversation it opens, so `yinz agent` and
`yinz action context` inside Codex find the session and the calling pane
without `--session`.

Codex's Linux sandbox blocks Unix socket connections unless the sandbox has
network access. A Codex agent in a sandboxed mode, such as the default
`workspace-write` or `--permission-mode on-request`, cannot reach Yinz. Spawn a
Codex orchestrator with `--allow-yinz`, which adds
`-c sandbox_workspace_write.network_access=true` and also allows internet access,
or with `--permission-mode skip`. Without either, Yinz commands report that the
connection was not permitted, and `list-sessions` marks sessions `UNREACHABLE`.

```sh
yinz agent spawn --kind codex --permission-mode on-request --allow-yinz \
  --cwd . --branch orchestrate --json
```

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
`prompt` accepts task text and exact Claude `/clear`. It rejects other `/`
commands, including `/exit`, and Claude `!` shell input. Use `agent send-keys`
for command input that does not produce prompt acknowledgement.

## Readiness and identity

The state vocabulary is `busy`, `waiting`, `idle` and `exited`. Reporting sources
distinguish launch metadata (`Spawned`), manual registration (`Registered`) and
agent reports (`Reported`). JSON retains these existing state and source names.

Status includes `status_available`, `ready`, `prompt_supported` and
`not_ready_reason`. List and status expose the reason in JSON and text output.
When several conditions block readiness, the first matching reason applies:

| Reason | Meaning |
| --- | --- |
| `prompt_unsupported` | Lifecycle, process or terminal evidence does not permit prompt submission |
| `not_idle` | The agent is busy, waiting or exited |
| `pending_submission` | An earlier submission still owns the input, including an unresolved claim after a timeout |
| `unconsumed_input` | Terminal input is newer than the last confirmed consumed input revision |

A ready agent has no reason (`null` in JSON). Older responses without the field
decode with no reason. An exited agent normally reports `prompt_unsupported`
because a live process is required.

A launch marker or a manual registration cannot establish lifecycle readiness. Missing
reports produce `status_unavailable`, rather than an inferred idle state.

Prompt submission initially supports Claude Code and Codex integrations with
lifecycle hooks. The terminal must support bracketed paste. Prompt submission
and key input require a verified agent command pane. An agent started manually
inside a shell can appear in listings without satisfying that requirement.

Yinz rejects prompts when the agent is busy, waiting or already has a pending
submission. Unconsumed manual input also makes the pane unready. There is no
prompt queue. The server registers observation before it writes the prompt.
Yinz waits for paste delivery, then waits 100 ms before sending Enter. It checks
input ownership and the target again before Enter. This delay does not prove
acceptance. Only a matching hook acknowledges the prompt.
Claude can collapse a long or multi-line paste into `<pasted_content>`.
For ordinary instructions, Yinz first types `Follow the instructions in the pasted text: `
so Claude treats the pasted text as instructions.
Inputs that start with `/`, `!` or `#` receive no instruction lead-in.
Acknowledgement, run history and pane titles ignore the lead-in and the wrapper.

Command execution and prompt acknowledgement are separate results.
In the Claude Code 2.1.285 live checks, native `!` shell input executed without a `UserPromptSubmit` hook.
The prompt wait timed out even when the command printed its expected output.
`prompt` now rejects this input before writing to the terminal.
This Claude version treats `#` as ordinary input and does not open a memory editor.
See the [live prefix checks](agent-cli-findings/prefix-probes.md) for the observed behavior.

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
prompt fingerprint when its hooks omit turn IDs. Fingerprints normalize CRLF to
LF and trim outer whitespace before hashing. Internal whitespace remains
significant. This canonicalization does not change the text sent to the terminal.
Pruned hooks retain the same fingerprint. Conflicting input invalidates
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
| `target_not_found` | The initial lookup found no pane at the requested ID |
| `not_agent` | The initial lookup found a pane without an agent record |
| `target_exited` | The requested agent's process already exited |
| `status_unavailable` | Lifecycle or process evidence is insufficient |
| `rejected` | Preconditions or request validation failed |

An initial lookup without `--conversation` does not imply a target transition.
Conversation pins and pending operations can report `target_changed` or `target_closed` when their target disappears.

`completed` confirms the end of a turn. It does not verify the agent's claims,
code changes or test results. Automatic policy waits do not produce
`needs_input` unless a person must respond. Worker permission requests, questions and
elicitation also produce `needs_input` for their parent conversation. The pane
stays `waiting` until all worker waits clear, then shows the latest parent state.

The default deadline is 30 seconds. `prompt --wait` defaults to 10 minutes.
`--timeout` accepts durations from `1ms` to `24h` and includes acceptance time.
Timeout does not cancel work already submitted or prove that the work failed.
An unacknowledged timeout means acceptance is uncertain. An acknowledged timeout
means the turn was accepted, but Yinz did not observe completion before the deadline.
Failure and interruption messages retain the provider's message when available.
Yinz keeps an unresolved submission claim until lifecycle reports resolve it or
the target changes. New prompts are rejected while that claim remains. Manual
input ends the caller's wait as `interrupted`, but it cannot recall a submission
already in transit. Its claim can remain until the original turn is resolved.
A retry cannot take ownership of that turn's completion. An unacknowledged
`Stop` cannot resolve the claim or restore readiness. Valid late acknowledgement
and completion can still resolve the original submission.

Codex's bridge observes the final `turn/completed` notification with status `failed`
and reports it through the existing `StopFailure` lifecycle path.
An acknowledged `prompt --wait` returns `failed` with the turn ID and provider message.
The resolved claim is released after any outstanding paste write finishes.
Readiness returns when no newer input blocks it.
Only the selected conversation's current turn can report this failure.
Hidden helper threads, stale or duplicate failures, transient errors and transport
disconnects do not fail the turn.
See the [Codex turn protocol](https://learn.chatgpt.com/docs/app-server#turn-events).

Recovery advice applies only to the same live conversation with unresolved input.
Do not recover a closed, exited or replaced target.
After inspection confirms that the agent stopped and unresolved input remains:

1. Read the pane output and inspect its current draft.
2. Clear the draft, using deliberate key input if necessary.
3. Confirm that status reports the same conversation as idle.
4. Run `yinz agent recover TARGET --conversation ID --confirm-empty`.

Recovery requires a live supported agent, no active submission, and no paste
write in flight. It clears abandoned claims and pending Enter delivery, and
records the current input revision as consumed. It sends no terminal input and
does not acknowledge the old prompt. A subsequent prompt can then be submitted.

Claude reports nothing when Esc interrupts a turn, so its record stays busy.
Esc can restore the interrupted prompt into the composer. Clear that draft first.
After the pane output shows that the agent stopped, add `--interrupted`. It also
accepts a busy or waiting agent, and resets its record to idle. The pane must have
received input, such as Esc, after the agent's last hook, and no hook, including
a subagent's, may have arrived in the 3 seconds since. A waiting agent is always
quiet, so quiet alone cannot show that its dialog is gone, and a hook still on its
way is not overtaken. Plugins then receive an `Interrupt` event for the agent.
Codex reports its own interrupt and returns to idle. Hooks that the cancelled
turn sends afterwards, such as a tool call finishing, do not make it busy again.
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
