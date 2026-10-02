# Control agents from the command line

`yinz agent` finds, launches and controls agents in terminal panes. Each target
must be an explicit terminal pane ID, such as `terminal_7` or `7`. Commands never
use the focused pane as an implicit target.

Use `yinz --session development agent ...` to select a session. Without
`--session`, Yinz resolves an inherited `YINZ_PANE_REF` to the caller's current
session, or uses the only active session outside a pane. `--session` is a root
option and belongs before `agent`. `YINZ_SESSION_NAME` alone is not a selector;
missing or stale pane context requires an explicit `--session`.

| Command | Result |
| --- | --- |
| `agent list` | Live agents, their identities and their reported states |
| `agent status TARGET` | State, reporting source, readiness, conversation and run IDs |
| `agent spawn --kind KIND` | Actual pane ID, working directory and branch |
| `agent prompt TARGET TEXT` | Acknowledgement that the agent accepted the prompt |
| `agent prompt TARGET --file PATH --wait` | Outcome of the submitted turn |
| `agent wait TARGET --submission ID` | Original prompt's outcome, including failure, after a timeout or dialog |
| `agent wait TARGET --until idle` | Observation of the requested state |
| `agent read TARGET --lines 80` | Last 80 terminal lines, including internal blank rows |
| `agent recover TARGET --conversation ID --confirm-empty [--interrupted]` | Restore readiness after inspecting and clearing the draft |
| `agent send-keys TARGET Esc` | Deliberate input to the agent's terminal UI |

All commands accept `--json`. Targeted commands also accept
`--conversation ID`. Use the conversation ID from `agent status` to require the
same conversation on a later command. A pane ID alone does not preserve identity
between separate commands.

`agent register PANE KIND --json` records an existing pane through `hocus-agentd`.
Registration returns the service's JSON acknowledgement; `--json` also structures
its errors. It does not establish managed prompt readiness or accept a conversation guard.

## Launch and submit work

For Codex, Muse, and Antigravity (agy), install Yinz's lifecycle hooks before launching:

```sh
yinz hocus hooks install --agent codex
yinz hocus hooks install --agent muse
yinz hocus hooks install --agent agy
```

`yinz hocus hooks install` without `--agent` wires every agent at once.
Use `yinz hocus hooks status --agent KIND` to inspect an existing installation.
Install the hooks on the host where the provider runs. If startup reports
`status_unavailable`, inspect the pane and hook status before retrying.
Claude Code launches receive the hooks through the existing launch planner.
Refresh older installed Claude hooks with `yinz hocus hooks install --agent claude`
to prevent duplicate reports alongside the inline hooks, and ensure the current
Yinz binary is on the provider's `PATH`.
Antigravity hooks can also be installed at the project level with `yinz hocus hooks install --agent agy --project`.

AGY completion requires its host-local `GetCascadeTrajectory` API to report Idle
for the submitted prompt. Hooks must inherit `ANTIGRAVITY_LS_ADDRESS` (a loopback
IP address and port) and `ANTIGRAVITY_CSRF_TOKEN`. Model responses and Stop hooks
alone do not confirm completion. Missing API access or unsupported responses
produce unavailable status. AGY CLI 1.2.14 does not supply these variables to
hooks by default, so hook installation alone cannot enable confirmed completion.

Codex launches reject Claude model aliases such as `haiku`, `sonnet`, and `opus`.
Use `--kind claude` for these aliases. Other model IDs are validated by the configured provider.

`agent spawn --help` describes every launch option. Recognized kinds are
`muse`, `codex`, `claude`, `cursor`, `qwen`, `gemini`, `aider`, `cline`,
`opencode`, `pi` and `agy`. Recognition does not imply prompt support.
Loaded TypeScript agent plugins add their own kinds and executable aliases.
Kind names ignore case. Recognized executable aliases, such as `cursor-agent`,
remain valid.

Use `--file PATH` for a JSON launch object, or `--file -` for standard input.
Flags override the corresponding JSON fields. JSON can also specify `command`,
`args`, `env` and `hooks`. The command must identify a recognized agent.
Spawn rejects opening `prompt` and `reuse_prompt` fields. Submit work after readiness.

Recognized custom executables receive the same argument translation and lifecycle
wiring as their canonical kind. `command` preserves the executable path and
overrides the kind's default program; an explicit `kind` must agree with the
recognized executable.

Managed Codex and Muse launches require a directly executable agent or shim.
Interpreter and wrapper forms such as `node /opt/codex.js` or `env FLAG=1 codex`
are rejected; put environment settings in the JSON `env` object instead.
For other indirect launches, `resume` is also rejected when its translation
requires a leading subcommand, such as `"resume":"last"` for Cursor. Use a direct shim,
or supply the complete invocation in `args` and omit `resume`. Raw Codex or Muse
invocations using `hooks: false` follow the same resume restriction and forgo the
managed launcher.

```sh
printf '%s\n' '{"command":"/opt/bin/claude","args":[],"branch":"review"}' \
  | yinz agent spawn --file - --cwd . --json
```

`--project` selects a registered project. `--cwd` selects a directory within that
project, or a local directory when no project is specified.
Without either option, local launches use the current directory.
`--branch` creates a separate worktree by default, based on `main` under `.worktrees/`.
Use `--base` and `--worktree-dir` to override those defaults.
Choose the baseline deliberately when delegating from a feature checkout:
`--base REF` selects the branch or commit the worker should start from.
Uncommitted changes in the orchestrator's checkout are not copied into a new worktree.
`--worktree false` switches the selected checkout instead.
`--model`, `--effort` and `--permission-mode` use the selected agent's supported values.
`--resume` accepts a conversation ID or `last`.

For a linked Codex worktree, Yinz honors its explicit trust decision. A worktree
without a decision inherits trust from a trusted primary checkout. If trust is
absent or explicitly untrusted, Yinz displays the canonical path and asks
`Trust this worktree? [y/N]`. Approval saves trust for
only that worktree. Answer with `y` or `yes`, ignoring case. Empty input, decline,
EOF, or interruption cancels the launch.

Read a fresh pane that remains `Spawned` to check for this prompt. To approve it:

```sh
yinz agent read terminal_7 --lines 30 --json
yinz agent send-keys terminal_7 y Enter --json
yinz agent wait terminal_7 --until idle --timeout 2m --json
```

Wait for `ready:true` before submitting a prompt. Approval continues startup
through the Codex bridge and its hook verification. Configuration errors or a
concurrent trust change stop the launch. Inspect the diagnostic and retry.
Explicit standalone Codex launches retain their existing behavior.

SSH launches use `--remote-host`, an absolute remote `--cwd`, and
`--allow-outside-worktree`. They require an existing remote checkout and reject
local project, branch, worktree and VM options.
`--firecracker` selects a named session VM. `--firecracker-image` selects its image.
A branch provides a host worktree for the VM. Without a branch or project,
the VM uses its guest workspace.

### Pi

Yinz supports the interactive Pi 1.0.0 CLI. Local launches automatically load a
reporting extension from Yinz's data directory. The launch fails if Yinz cannot
write that extension. `hooks: false` suppresses this injected extension. It does
not disable extensions you installed in Pi.

For Pi processes started outside Yinz, install the same extension:

```sh
yinz hocus hooks install --agent pi
yinz hocus hooks status --agent pi
```

The installer uses `$PI_CODING_AGENT_DIR/extensions`, or
`~/.pi/agent/extensions` when that variable is unset or empty. Installation is
user-level only. Use `yinz hocus hooks uninstall --agent pi` to remove it.
Installed and injected copies share one reporter, including after `/reload`.

Pi supports `--model` and these `--effort` values: `none`, `minimal`, `low`,
`medium`, `high`, `xhigh`, and `max`. Yinz translates `none` to Pi's `off`.
Pi controls permissions through extensions, so Yinz rejects `--permission-mode`.
Hocus launches can include a positional opening prompt. With `agent spawn`, wait
for readiness and then use `agent prompt`.

`--resume last` maps to Pi's `--continue`. Other resume values use `--session`
and can name a session ID or file. Hocus archive resumes use the same launch path.
Restart removes opening prompts and session selectors while preserving supported
launch options. It rejects print, JSON, RPC, and ambiguous command arguments.

Pi prompt acknowledgement requires the conversation, prompt fingerprint, and
turn ID to match. Completion waits for `agent_settled`. A final assistant error
or abort produces `failed` or `interrupted`. Blocking extension UI prompts report
`waiting`. Yinz cannot observe invisible waits inside third-party extensions.

`agent prompt TARGET /new` starts a new Pi conversation. Other slash commands and
`!` shell input require deliberate terminal input through `agent send-keys`.

SSH launches require Pi and Yinz's hooks to be installed on the remote machine.
Yinz removes its local extension argument and preserves user extension arguments.
Firecracker launches do not provision Pi or its reporting extension automatically.

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
     --kind codex --cwd . --branch review-auth --base main --json
   ```

   Read `spawned.pane_id`, `spawned.branch` and `spawned.cwd` from the response.
   A branch collision can change the requested branch name. `--reuse` permits an
   existing checkout. `--allow-outside-worktree` permits a launch outside a
   linked Git worktree. Launches use the existing agent launch planner.
   Status retains the launch branch when known. It does not probe Git for later
   branch changes.

   A failed spawn can still have created resources. Inspect `launch_resources`
   and `spawned` in its response before retrying; `spawned` identifies a pane
   when one exists. `launch_resources` retains `cwd`, `branch`, `requested_branch`,
   and `reused` when known. Failed Git operations may report an attempted path or
   branch that was not fully created; inspect it first. Reconcile the reported resources
   rather than creating another worker blindly. A guest-only VM launch reports
   `cwd: null` when its directory is unknown; do not substitute a host directory.

2. Wait for lifecycle reports to confirm that the agent is idle:

   ```sh
   yinz --session development agent wait terminal_7 --until idle --timeout 30s --json
   ```

   A successful spawn does not establish readiness. Startup, authentication or
   an initial question can prevent the agent from becoming ready.
   The wait response already includes status in `agents`. Check `ready`,
   `status_available` and `prompt_supported`, and keep `agent.session_id` for the
   conversation guard. A separate `status` command is only needed for a fresh
   observation.

3. Submit the task and wait for its outcome:

   ```sh
   yinz --session development agent prompt terminal_7 \
     --file review-request.md --conversation CONVERSATION_ID \
     --submission SUBMISSION_ID --wait --timeout 10m --json
   ```

   Replace the placeholders with the reported conversation and a unique submission
   ID for this prompt. Fresh Muse panes have no conversation until the first turn;
   omit that guard for the first prompt only. Use `message` from a completed
   response as the answer. `agent read` is for terminal diagnostics or missing
   output, rather than a required step after every successful prompt.

   After `needs_input`, inspect the dialog with `agent read` and answer with
   `agent send-keys` within the user's authorization. Then wait for the same
   submission as described below. This preserves its success or failure outcome.

Direct text and standard input use the same submission path:

```sh
yinz agent prompt terminal_7 "Review the current diff"
git diff | yinz agent prompt terminal_7 --file - --wait --json
```

Prompts must contain UTF-8 text and fit within 1 MiB. Yinz converts CRLF line
endings to LF and removes trailing newlines before submission. It preserves
other spaces and internal newlines. Terminal control characters are rejected,
except tabs and newlines.
For Claude, Yinz removes U+200B (zero-width space) and U+202E (right-to-left
override) before pasting. All other characters, including emoji joiners, remain
unchanged. Text that becomes empty is rejected. Codex text is not normalized this way.
`prompt` accepts task text, exact Claude `/clear`, and exact Codex or Pi `/new`.
It rejects other `/` commands, including `/exit`, and Claude and Pi `!` shell input.
Use `agent send-keys` for command input that does not produce prompt acknowledgement.

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
| `pending_submission` | An earlier submission still owns the input and has not been acknowledged |
| `unresolved_turn` | An accepted turn still owns the input after an interruption or an unconfirmed outcome; inspect the pane and recover only after it has stopped and its draft is empty |
| `unconsumed_input` | Terminal input is newer than the last confirmed consumed input revision |

A ready agent has no reason (`null` in JSON). Older responses without the field
decode with no reason. An exited agent normally reports `prompt_unsupported`
because a live process is required.

A launch marker or a manual registration cannot establish lifecycle readiness. Missing
reports produce `status_unavailable`, rather than an inferred idle state.

Codex and Muse speak no session lifecycle until the first turn begins, so
their launchers announce a verified startup instead: Codex from the selected
conversation's idle status and hook capability check, Muse from the hook
wiring on disk. Muse hook verification establishes reporting capability only.
Before the first prompt, Yinz must also detect an empty Muse 1.4.2 composer
with its cursor immediately after `❯ `. Dialogs, drafts, hidden cursors,
scrolled views, and unknown layouts remain unready. Yinz never changes workspace
trust automatically.

The Muse announcement carries no session id. The first submit binds the
conversation, so use `--conversation` only after that first acknowledgement.
An unwired Muse pane stays `status_unavailable`.

Prompt submission supports Claude Code, Codex, Muse, Pi, and Antigravity (agy) integrations with
lifecycle hooks. The terminal must support bracketed paste. Prompt submission
and key input require a verified agent command pane. An agent started manually
inside a shell can appear in listings without satisfying that requirement.

Yinz rejects prompts when the agent is busy, waiting or already has a pending
submission. Unconsumed manual input also makes the pane unready. There is no
prompt queue. The server registers observation before it writes the prompt.
Yinz waits for paste delivery, then waits 100 ms before sending Enter. It checks
input ownership and the target again before Enter. For the first Muse prompt,
it also checks that the cursor remains inside the composer. This delay does not prove
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

Readiness requires a known consumed input revision that matches the pane's current
revision. Startup reports establish that revision. A matching CLI prompt report
uses the revision captured when Yinz sent the prompt. Manual acceptance alone
cannot restore readiness because Yinz cannot identify which input revision it
consumed. Tool and completion reports do not resolve an unknown revision.
Input revisions restart at zero when the pane's process respawns, including when
scrollback is retained. Input typed after respawn still prevents startup reports
from establishing readiness.

A matching prompt report acknowledges acceptance. Codex and Muse correlation also
requires a provider turn ID. Pi requires the turn ID generated by its reporting extension. Claude Code can use conversation identity and the
prompt fingerprint when its hooks omit turn IDs. Fingerprints normalize CRLF to
LF and trim outer whitespace before hashing. Internal whitespace remains
significant. This canonicalization does not change the text sent to the terminal.
Pruned hooks retain the same fingerprint. Conflicting input invalidates
the pending operation. An unrelated conversation cannot complete the wait.

## Outcomes and deadlines

Agent state and operation outcome are separate. An idle agent can have finished,
failed or been interrupted. For Claude Code, Codex, Muse, Pi, and Antigravity (agy), `agent wait --until idle`
requires reported idle state and prompt readiness. For other agents, including
OpenCode, matching lifecycle reports of idle state can satisfy the wait even
though `ready` and `prompt_supported` remain false. Other state waits observe the
requested state. A state wait does not prove that a particular task completed
successfully.

To continue observing a particular prompt, use its `submission_id`:

```sh
yinz agent wait terminal_7 --submission SUBMISSION_ID \
  --conversation CONVERSATION_ID --timeout 10m --json
```

This returns the original turn's outcome, including `failed` or `interrupted`,
rather than replacing it with `ok` when the agent becomes idle. It can return
`needs_input` while a dialog remains; inspect and answer the dialog deliberately,
then wait with the same submission ID. Use this command after a prompt timeout
or disconnection as well. `--submission` and an explicit `--until` are mutually
exclusive. A submission wait concerns that turn; a separate idle wait can observe
Claude background tasks and their later notification turns.

`prompt --submission ID` accepts a caller-chosen identifier of 1–128 ASCII letters,
digits, `.`, `_`, or `-`. Choose a unique ID per prompt and save it before sending
when you need to recover from lost command output. Otherwise, the CLI generates
an ID. Responses for admitted prompts carry `submission_id`, and transport errors
echo the client-known ID. For concurrent workers, submit without `--wait`, retain
each pane/conversation/submission triple, and wait on those IDs later.

The server retains only the last 32 submissions per pane/conversation in memory.
Target replacement, exit, closure, and server restart discard this history. Unknown or expired
IDs return `rejected`; reusing a retained ID for another prompt is refused without
sending input. These IDs are not durable idempotency keys. An unavailable result
does not prove a task never ran, so inspect the work before deciding to resubmit.

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

1. Read the pane output and inspect the composer in the terminal UI.
2. Clear any actual draft with deliberate key input. Confirm that the composer is empty.
3. Confirm that status reports the same conversation as idle.
4. Run `yinz agent recover TARGET --conversation ID --confirm-empty`.

Plain terminal text includes ghost suggestions and cannot establish whether the
composer contains a draft. Do not infer a draft from suggestion text or strip
suggestions heuristically. `--confirm-empty` requires deliberate confirmation
through the terminal UI, not an assumption based on `agent read`.
An orchestrator with only text-reading tools must ask the user to verify the
stopped agent and empty composer in the terminal before running recovery. Sending
Esc and immediately asserting `--confirm-empty` does not meet this requirement.

Recovery requires a live supported agent, no active submission, and no paste
write in flight. It clears abandoned claims and pending Enter delivery, and
records the current input revision as consumed. It sends no terminal input and
does not acknowledge the old prompt. Any unresolved submissions become `interrupted`,
with task completion explicitly unverified. Inspect their work before submitting a new task.
Pass an empty `--conversation` when the pane reports no session, as a fresh
Muse pane does before its first turn binds one; startup dialog keys sent with
`agent send-keys` need no recovery when later output shows the empty composer.
Newer manual input still requires recovery.

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
`submission_id`, `acknowledged`, `turn_id` and `message`. With `--json`, connection
and local input errors also produce a JSON response on stdout and return `1`.
Malformed CLI arguments still produce plain errors on stderr and return `2`.

`agent read --lines N` returns at most N terminal lines, including output below a
scrolled viewport. Whitespace-only rows count as blank. Trailing blank rows do not
count toward the limit. Internal blank rows count and remain in the output.
Leading blank rows in the selected output are removed. Nonblank lines are preserved
verbatim. An entirely blank terminal returns an empty string. The default is 80, and the accepted range is
1–100000. Text and JSON responses contain the same retained content.
Reading does not change the viewport or selection. Ghost suggestions remain visible
and cannot prove that the composer contains a draft or is empty.

When intervention is necessary, inspect the output before sending keys:

```sh
yinz agent read terminal_7 --conversation ID --lines 80
yinz agent send-keys terminal_7 --conversation ID Esc
```

Pass one key per argument. Quote modifiers, such as `"Ctrl u"`. Named keys include
`Esc`, `Enter`, and `Down`. For example, `send-keys TARGET Down Enter` sends two keys.
The aliases `Ctrl+u`, `ctrl-u`, `C-u`, `Escape`, and `Return` are unsupported.

Key input does not imply a new prompt acknowledgement. Use `agent status` or
`agent wait` to observe what happens next. `wait --until` accepts `busy`, `waiting`,
`idle`, and `exited`.

## Durable archives

`agent runs`, `run`, `history-search`, and `predecessors` query `hocus-store` and
always return JSON. They describe durable archive run IDs, distinct from provider
conversation IDs, turn IDs, and the live submission IDs above.

List pages expose boolean `more` and numeric `next_offset`; continue `agent runs`
with `--offset NEXT_OFFSET`. A run timeline exposes boolean `more` and numeric
`next_revision`; continue `agent run RUN_ID` with `--from-revision NEXT_REVISION`.
Keep the original filters when continuing. `history-search` instead exposes
`truncated`; narrow its query or filters when the returned results are incomplete.

The root `--session NAME` selects the server handling the query, not the archived
records' owner. Use both `--host-id HOST` and `--run-session SESSION` on `runs` or
`history-search` to scope records to one server.

## TypeScript agent plugins

Agent plugins are TypeScript modules in the existing Deno plugin runtime.
They register data with `yinz.agent.registerProvider()`. The host validates the
data and uses it for detection, launch, restart, and guarded terminal input.
The agent chooser and plugin APIs receive the same provider definitions.

Load an agent plugin with `shared true`. Registration requires `RunCommands`.
Each registration belongs to that plugin instance. Reload and unload remove its
registrations. Another plugin cannot replace an existing provider's kind or aliases.

```kdl
load_plugins {
    "file:/absolute/path/my-agent.ts" {
        shared true
    }
}
```

Export `agentProvider` when the plugin supplies installable hook files.
The hook installer evaluates this export in a bounded subprocess. It does not
run `load()` or permit host operations during module initialization.

```ts
import yinz, { definePlugin, type AgentProvider } from "yinz:plugin";

export const agentProvider: AgentProvider = {
  kind: "my-agent",
  executable: "my-agent",
  aliases: ["my-agent-cli"],
  launch: {
    model_args: ["--model", "{value}"],
    prompt_args: ["--prompt", "{value}"],
    resume_args: ["--session", "{value}"],
    resume_last_args: ["--continue"],
    permissions: { manual: ["--approval", "manual"] },
  },
};

export default definePlugin({
  async load() {
    await yinz.waitForPermissions(["RunCommands"]);
    yinz.agent.registerProvider(agentProvider);
  },
});
```

Argument templates contain individual arguments. Substitution of `{value}` never
splits the value or invokes a shell. Omitted templates reject unsupported options.
Use `restart` to declare option arities, conversation selectors, and commands that
must not restart. Unknown restart arguments fail without changing the pane.

Only declare `input` after the agent's hooks can acknowledge and complete specific
submitted turns. Input settings include `submit_key`, `submit_delay_ms`,
`paste_prefix`, `newline`, and `command_prefixes`. The host retains process,
conversation, readiness, input ownership, and bracketed-paste checks.
Provider prompt acknowledgements and completions must carry a nonempty `turn_id`.
Completion must match the acknowledged turn. A paste write alone is not acceptance.
Use `yinz.agent.control()` for guarded requests. Existing `send()` and `prompt()`
also use guarded delivery for provider agents. Await `send()` to observe submission
failures from these providers.

Hook adapters emit the existing `HookEvent` format through
`yinz action agent-report --hook-json --kind KIND`. Events include `SessionStart`,
`UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `Stop`, and `SessionEnd`.
Include the same `session_id` in every event. For prompt acknowledgement, include
the submitted `prompt` or its `prompt_sha256` and the provider's `turn_id`.
For completion, include that `turn_id` and optional `last_assistant_message`.
Hook adapters must use the remote broker route when `YINZ_REMOTE_PANE_TOKEN` is set.

Each `hooks` entry declares a relative `path`, `base` (`config` or `home`),
ownership `marker`, `content`, and optional `executable` flag. The installer refuses
to overwrite a file without that marker. Template placeholders are `__YINZ_BIN__`
for a JSON-quoted binary path, `__YINZ_SHELL__` for a shell-quoted path, and
`__TOKEN_ENV__` for the JSON-quoted remote token variable name.

```sh
yinz hocus hooks install --agent ./my-agent.ts --dry-run
yinz hocus hooks install --agent ./my-agent.ts
yinz hocus hooks status --agent ./my-agent.ts
yinz hocus hooks uninstall --agent ./my-agent.ts
```

OpenCode uses the bundled [TypeScript provider](../yinz-deno-builtins/assets/plugins/agent-opencode.ts).
Yinz loads it before other background plugins. Its hook adapter owns event
translation, parent-session filtering, and ordered reporting. Existing
`yinz hocus hooks install --agent opencode` commands still work. Older installed
hook files remain accepted until reinstallation. OpenCode supports launch,
detection, restart, and lifecycle waits. It does not advertise reliable prompt
submission yet.

## Test an agent plugin

Use the existing `yinz:testing` API and `yinz plugins test` runner.
Place unit checks in `.test.ts` files and real-session checks in
`.integration.test.ts` files beside the plugin.

- Use `loadPlugin()` to inspect registration commands and test plugin callbacks.
- Use `session.fixture()` to create fake agent executables in an isolated workspace.
- Start the session with `backgroundPlugins: [{path: "./my-agent.ts", configuration: {shared: "true"}}]`.
- Use asynchronous `session.agent()` requests to exercise real agent control.
- Use `session.report()` to deliver canonical hooks through the normal report path.
- Observe actual terminal output with `session.waitForText()` or `session.waitFor()`.
- Inject stale, missing, and mismatched hooks while a prompt request is pending.

Session agent controls require the test's `RunCommands` grant. Fixtures do not need
an installed agent, credentials, a model service, or network access.
The [provider integration tests](../yinz-deno-builtins/assets/plugins/agent-opencode.integration.test.ts)
exercise registration, aliases, launch arguments, input bytes, custom Enter timing,
turn correlation, and status-only capabilities. Host unit tests also cover
validation, conflicting registrations, unload, restart refusal, input interruption,
timeouts, and hook-file ownership.
