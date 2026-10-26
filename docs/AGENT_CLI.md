# Control agents from the command line

`yinz agent` finds, launches and controls agents in terminal panes. Each target
must be an explicit terminal pane ID, such as `terminal_7` or `7`. Commands never
use the focused pane as an implicit target.

Use `yinz --session development agent ...` to select a session. Without
`--session`, Yinz resolves an inherited `YINZ_PANE_REF` to the caller's current
session, or uses the only active session outside a pane. `--session` is a root
option and belongs before `agent`. `YINZ_SESSION_NAME` alone is not a selector;
missing or stale pane context requires an explicit `--session`.
The separate `agent restart --session MODE` option selects `resume` or `fresh`.

| Command | Result |
| --- | --- |
| `agent list` | Live agents, their identities and their reported states |
| `agent watch --json` | Normalized lifecycle events, with snapshot and resume cursors |
| `agent status TARGET` | State, reporting source, readiness, conversation and run IDs |
| `agent spawn --kind KIND` | Actual pane ID, working directory and branch |
| `agent spawn --kind KIND --suppressed` | Running agent in a hidden pane |
| `agent show TARGET` | Restore the agent without changing focus or switching tabs |
| `agent hide TARGET` | Hide the agent without stopping its process |
| `agent restart TARGET --session MODE [TEXT] [--file PATH]` | Restart an exited agent in the same held pane |
| `agent prompt TARGET TEXT` | Acknowledgement that the agent accepted the prompt |
| `agent prompt TARGET --file PATH --wait` | Outcome of the submitted turn |
| `agent wait TARGET --submission ID` | Original prompt's outcome, including failure, after a timeout or dialog |
| `agent wait TARGET --until idle` | Observation of the requested state |
| `agent read TARGET --lines 80` | Last 80 terminal lines, including internal blank rows |
| `agent clear-draft TARGET --conversation ID --input-revision REV --confirm-discard` | Discard an inspected draft and verify the empty composer |
| `agent recover TARGET --conversation ID --confirm-empty [--interrupted]` | Restore readiness after inspecting and clearing the draft |
| `agent send-keys TARGET Esc` | Deliberate input to the agent's terminal UI |

All commands accept `--json`. Targeted commands except `restart` also accept
`--conversation ID`. Use the conversation ID from `agent status` to require the
same conversation on a later command. A pane ID alone does not preserve identity
between separate commands.

`agent register PANE KIND --json` records an existing pane through `agent-service`.
Registration returns the service's JSON acknowledgement; `--json` also structures
its errors. It does not establish managed prompt readiness or accept a conversation guard.

## Provider quotas in the Agents panel

Open the Agents panel with `Ctrl e a` and select a local Codex or Claude agent.
The detail view shows the local provider account's quota windows, percentage used,
and time until reset. These are account limits, shared by all panes using that
account. They are not per-agent token counts or spending totals.
Compact layouts show the highest window percentage; enlarge the panel to see
the individual windows and reset times.

The panel checks the selected provider once a minute while visible. `Ctrl r`
refreshes it immediately. A failed check keeps the last successful values and
marks them stale with their original age. Missing limits appear as unavailable;
they do not appear as zero usage. Hiding the panel stops further checks.

Codex requires an installed CLI with the app-server protocol and a ChatGPT
subscription login. Claude uses its local OAuth credentials. The server's
`CODEX_HOME` and `CLAUDE_CONFIG_DIR` settings select the local accounts. A pane
with a different environment can use a different account. SSH and Bubblewrap
agents do not display the host's local quotas. Other providers and Codex
API-key-only accounts report that subscription quotas are unsupported. Claude
requires OAuth credentials; an API key alone cannot supply these limits.

## Restart an exited agent

```sh
yinz agent restart terminal_7 --session resume "Resolve the rebase conflicts"
yinz agent restart terminal_7 --session fresh "Review this branch against main"
yinz agent restart terminal_7 --session resume --file failures.txt --json
```

The pane must be held after exit. Restart preserves its launch flags, cwd and
target, replaces the original opening prompt, and uses the provider's resume
and prompt grammar. `--session resume` requires the last reported session id;
`--session fresh` removes all session selectors. An absent prompt opens the
agent without an opening task; `--file -` reads one from stdin. Muse refuses
resume with an opening prompt. Unsafe or ambiguous restart arguments are
returned as errors.

The JSON response includes `restarted: {pane_id, session_id, command}`. The
session id is null for fresh launches until the provider reports its new one.
A successful response confirms acceptance for spawning, not provider startup.
The same pane fires `agent_exit` on its next exit. A hook plugin covering it
must close itself to reveal it.

## Launch and submit work

For Codex, Muse, and Antigravity (agy), install Yinz's lifecycle hooks before launching:

```sh
yinz hooks install --agent codex
yinz hooks install --agent muse
yinz hooks install --agent agy
```

`yinz hooks install` without `--agent` wires every agent at once.
Use `yinz hooks status --agent KIND` to inspect an existing installation.
Install the hooks on the host where the provider runs. If startup reports
`status_unavailable`, inspect the pane and hook status before retrying.
Claude Code launches receive the hooks through the existing launch planner.
Refresh older installed Claude hooks with `yinz hooks install --agent claude`
to prevent duplicate reports alongside the inline hooks, and ensure the current
Yinz binary is on the provider's `PATH`.
Antigravity hooks can also be installed at the project level with `yinz hooks install --agent agy --project`.

AGY completion requires its host-local `GetCascadeTrajectory` API to report Idle
for the submitted prompt. Hooks must inherit `ANTIGRAVITY_LS_ADDRESS` (a loopback
IP address and port) and `ANTIGRAVITY_CSRF_TOKEN`. Model responses and Stop hooks
alone do not confirm completion. Missing API access or unsupported responses
produce unavailable status. AGY CLI 1.2.14 does not supply these variables to
hooks by default, so hook installation alone cannot enable confirmed completion.

Codex launches reject Claude model aliases such as `haiku`, `sonnet`, and `opus`.
Use `--kind claude` for these aliases. Other model IDs are validated by the configured provider.

On Linux, `--bubblewrap` uses an explicitly prepared project environment.
See [Project Bubblewrap environments](BUBBLEWRAP.md) for configuration, preparation, and authentication.

`agent spawn --help` describes every launch option. Recognized kinds are
`muse`, `codex`, `claude`, `cursor`, `qwen`, `gemini`, `aider`, `cline`,
`opencode`, `pi` and `agy`. Recognition does not imply prompt support.
Loaded Scheme agent plugins add their own kinds and executable aliases.
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

To create a project and launch its first agent:

```sh
yinz agent spawn --kind codex \
  --new-project my-app --destination ~/projects/my-app --json
```

Supply both `--new-project NAME` and `--destination PATH`, or the JSON fields
`new_project` and `destination`. Flags override JSON fields. The destination is
the exact repository root; relative paths resolve against the CLI's current directory.
Its parent must already exist. The destination must be absent, including symlinks,
and neither the project name nor root may already be registered.
Yinz initializes an empty Git repository on `main`, remembers the project, and
launches the agent in its main checkout. It creates no initial commit or remote.
Placement and keyboard focus follow the same rules as other spawns.

New project launches reject `project`, `cwd`, `remote_host`, `branch`, `base`,
`worktree`, `worktree_dir`, `reuse` and `resume`, including values in JSON.
Agent, model, effort, permissions, environment and placement options still apply.
Failures after directory creation retain the directory and report its path and
whether registration succeeded. A launch failure keeps the repository and project.
Inspect them, then retry with `--project NAME --allow-outside-worktree` and the
desired agent options. Yinz does not delete resources automatically.

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
local project, branch, and worktree options.

### Suppressed agents

Use `--suppressed` to launch a hidden agent in the current tab.
Add `--tab ID` to select an existing tab. Suppressed launches preserve focus and the visible layout.
`--suppressed` cannot be combined with `--new-tab`.

Launch JSON accepts `"suppressed": true`, optionally with `"tab": ID`.
The default is false. Any CLI placement flag replaces all JSON placement fields.
The running server must confirm suppression support before Yinz sends the launch request.

```sh
yinz agent spawn --kind claude --branch review --suppressed --json
# Use the pane ID returned by spawn in subsequent commands.
yinz agent wait terminal_7 --until idle --json
yinz agent prompt terminal_7 "Review the current diff" --wait --json
yinz agent read terminal_7 --lines 80
yinz agent show terminal_7 --conversation CONVERSATION_ID --json
yinz agent hide terminal_7 --conversation CONVERSATION_ID --json
```

`agent status` and `agent list` include `is_suppressed` in JSON and text output.
Hidden agents use the existing prompt, wait, read, send-keys, recovery, restart, and lifecycle paths.
Suppression changes visibility only. Checkout policy, provider behavior, and session lifetime remain the same.

`show` restores a regular pane as tiled in its owning tab. It preserves focus and the active tab.
If the tab has no room, `show` returns `rejected` and leaves the running pane suppressed.

For an overlay, `show` retains its kind and coordinates.
Both `show` and `hide` affect the entire overlay container, including all multipane children.
For a hidden modal overlay, `show` returns `rejected` because restoration would change focus.
Use normal overlay navigation to restore a hidden modal.

For a suppressed pane-tab member or hidden stack-list member, `show` returns `rejected` without changing the group or focus.
Use pane-tab or stack navigation to select these agents.

Repeated show or hide requests succeed when the pane is already in the requested state, including `show` for an already visible modal.
Both commands also work for held agents after exit and accept `--conversation` and `--json`.

`hide` returns `rejected` if no selectable destination remains in the session.
Other children of the same overlay container do not count as destinations because they are hidden together.

### Pi

Yinz supports the interactive Pi 1.0.0 CLI. Local launches automatically load a
reporting extension from Yinz's data directory. The launch fails if Yinz cannot
write that extension. `hooks: false` suppresses this injected extension. It does
not disable extensions you installed in Pi.

For Pi processes started outside Yinz, install the same extension:

```sh
yinz hooks install --agent pi
yinz hooks status --agent pi
```

The installer uses `$PI_CODING_AGENT_DIR/extensions`, or
`~/.pi/agent/extensions` when that variable is unset or empty. Installation is
user-level only. Use `yinz hooks uninstall --agent pi` to remove it.
Installed and injected copies share one reporter, including after `/reload`.

Pi supports `--model` and these `--effort` values: `none`, `minimal`, `low`,
`medium`, `high`, `xhigh`, and `max`. Yinz translates `none` to Pi's `off`.
Pi controls permissions through extensions, so Yinz rejects `--permission-mode`.
Agent launches can include a positional opening prompt. With `agent spawn`, wait
for readiness and then use `agent prompt`.

`--resume last` maps to Pi's `--continue`. Other resume values use `--session`
and can name a session ID or file. Agent archive resumes use the same launch path.
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
   before creating another worker.

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
| `clearing_draft` | Draft clearing owns the composer until verification or queued writes finish |
| `unresolved_turn` | An accepted turn still owns the input after an interruption or an unconfirmed outcome; inspect the pane and recover only after it has stopped and its draft is empty |
| `unconsumed_input` | Input consumption is unknown. An explicit prompt clears it before delivery |

A ready agent has no reason (`null` in JSON). Older responses without the field
decode with no reason. An exited agent normally reports `prompt_unsupported`
because a live process is required.

The additive `readiness` object records every capability check from the same
screen-thread observation. Older servers omit it. Text output lists all failed
checks as `readiness_failures`, even when one failure masks another:

| Evidence | Interpretation |
| --- | --- |
| `kind_supported` | The provider supports guarded prompt input |
| `reported`, `lifecycle_matches` | A reported agent has a lifecycle for its current kind and conversation |
| `lifecycle_kind`, `lifecycle_session_id` | Null means no lifecycle. Different values identify a stale lifecycle |
| `provisional_lifecycle` | Muse has verified startup hooks but no conversation yet |
| `held`, `exited`, `agent_exited`, `suspended` | Each must be false for prompt input |
| `sync_active`, `excluded_from_sync` | Input is disabled only when both are true |
| `command_present`, `command_kind`, `process_matches` | The stored command must identify the reported provider. Foreground detection alone is insufficient |
| `bracketed_paste` | The terminal grid has bracketed paste enabled |
| `last_report_age_ms` | Time since the lifecycle last received a hook, including worker and stateless hooks. This does not prove completion |

For example, `lifecycle_missing, command_mismatch` identifies two independent
failures. Restoring hooks alone will not make that pane promptable. An agent
launched inside a shell needs a managed launch with `agent spawn`.

`send-keys` does not require bracketed paste. It still requires process and
lifecycle evidence, except for supported startup dialog handling.

To investigate missing or stale tracking:

1. Save `yinz --session SESSION agent list --json` and
   `yinz --session SESSION action list-panes --all --json` before recovery.
2. Run `yinz hooks status --agent KIND` on the agent's host and in its
   project directory. For Codex, also inspect hook trust with `/hooks`.
3. Record the binary versions, session, pane reference, conversation ID, and time.
   Save provider hook logs and the visible dialog or composer state.
4. In a disposable reproduction, set `YINZ_HOOK_TRACE=1` in the reporter's
   environment. Preserve `${TMPDIR:-/tmp}/yinz-$(id -u)/yinz-log/yinz.log`
   before restarting. The log is bounded and can contain reports from other sessions.
5. Match report IDs through transport, `screen_queue`, `state`, and `lifecycle`
   stages. With debug logging enabled, lifecycle entries identify creation,
   removal reasons, and the affected kind and conversation.

A restart discards in-memory lifecycle and submission records. A fresh parent
hook can restore lifecycle tracking, but it does not necessarily prove that
terminal input was consumed. Never infer readiness from a saved `Reported` record.
Codex's managed launcher can verify hooks and announce an idle selected
conversation before the first turn. Failed verification leaves it untracked.

For a suspected false `Waiting` state, collect the permission request's
`permission_mode` and `approvals_reviewer`, then the following tool-progress
events. The Codex reporter already reads a missing reviewer from the matching
turn context in a verified transcript. It scans up to 64 MiB backwards in
4 MiB windows, independently of the assistant-message tail. Missing identity,
unavailable transcripts, oversized records, or a context outside that budget
leave the reviewer unknown. Requests without known automatic approval evidence
still report a human wait. Confirm the visible dialog before sending any answer. A long hook
silence alone does not mean a person is needed or that the agent has finished.

A launch marker or a manual registration cannot establish lifecycle readiness. Missing
reports produce `status_unavailable`, rather than an inferred idle state.

Codex and Muse speak no session lifecycle until the first turn begins, so
their launchers announce a verified startup instead: Codex from the selected
conversation's idle status and hook capability check, Muse from the hook
wiring on disk. Muse hook verification establishes reporting capability only.
Before the first prompt, Yinz must also detect an empty Muse 1.4.2 composer
with its cursor immediately after `❯ `. Dialogs, drafts, hidden cursors,
scrolled views, and unknown layouts remain unready. An explicit prompt can replace a draft in that composer.
Before Muse binds its first conversation, its composer must be visible to distinguish input from setup dialogs.
Yinz never changes workspace trust automatically.

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

## Subscribe to agent lifecycle events

These commands use the same session socket, subscription protocol, and NDJSON envelope:

```sh
yinz --session work events --event-type agent_lifecycle
yinz --session work agent watch --json
yinz --session work agent watch --pane terminal_7 --kind codex --json
yinz --session work agent watch --project yinz --json
yinz --session work agent watch --cursor "$CURSOR" --json
```

Without `--json`, `agent watch` prints the pane, event type, and state.
`events --type` and `events --event-type` are aliases. Each JSON line has this shape:

```json
{"event":"agent_lifecycle","payload":{"schema_version":1,"event_type":"ready","cursor":"..."}}
```

The example omits metadata. Each payload includes `host_id`, `session`,
`server_epoch`, `sequence`, `observed_at_ms`, and `cursor`. Pane records also
include `pane_id`, `pane_generation`, and `pane_sequence`. Identity fields are
`kind`, `conversation_id`, `submission_id`, `turn_id`, and `run_id`.
Unknown identities are null. Human and provider-generated turns have a null
`submission_id`. Claude and AGY prompt identities appear as `provider_prompt_id`.
Tracked-turn uncertainty preserves both provider IDs, the submission ID, and the actual
`outcome`. Submission timeout uncertainty uses that submission's tracked turn. Unknown
provider IDs are null, even when a newer turn exists in the pane.
The stream excludes prompt text, transcripts, final answer text, and raw hook JSON.

Use repeated `--pane`, `--project`, `--kind`, and `--lifecycle-type` filters.
Values within one filter use OR. Different filters use AND. Filters apply only
to normalized lifecycle events. They are not authorization boundaries.
Project filtering uses the assignment and repository rules of `agent list --project`.
Project probes run outside the screen thread. Stale results do not change membership.
Each fact includes `projects_resolved_for`: the projects covered by a current completed
probe, or an empty array while membership is unresolved. If any requested project is
unresolved, matching facts immediately become `observation_uncertain` with reason
`project_membership_unresolved`. The `observed_event_type` field identifies the original
fact. Pane, kind, and lifecycle-type filters apply to that original fact.

These uncertainty records preserve identity, sequence, and cursor. Replay uses the
recorded probe coverage, so historical uncertainty remains explicit after resolution.
Subscribers without project filters receive the original facts. Completed probes restore
normal filtering for subsequent facts, including checkpoints for resolved non-members.
Closure and replacement records retain the previous identity and project membership.
External database changes require resubscription. Shared storage does not
provide cross-session event delivery.

| Event type | Meaning |
| --- | --- |
| `state_changed` | Reported state or agent metadata changed |
| `needs_input` | The agent is Waiting and `wait_needs_a_person` is true |
| `submission_acknowledged` | A provider acknowledged a turn |
| `turn_completed`, `turn_failed`, `turn_interrupted` | A correlated provider report established that outcome |
| `observation_uncertain` | Observation could not establish an outcome |
| `pane_spawned`, `pane_closed`, `pane_replaced` | A pane appeared, disappeared, or started a replacement process |
| `conversation_replaced` | Provider or conversation identity changed, including clearing the record |
| `background_tasks_changed` | Tracked Claude background tasks changed |
| `project_membership_changed` | A project probe changed membership |

Idle alone does not establish completion. Timeouts, missing confirmations, and
interrupted observation do not establish a failed or interrupted turn. Provider
outcome coverage still depends on the installed hooks and runtime confirmation.
Correlation retains the same 32-turn window as submission history. Evicting an
unfinished turn produces `observation_uncertain` with reason `turn_tracking_limit`.

### Startup, replay, and loss

A fresh stream starts with `snapshot_begin`, then matching `snapshot` records,
then `ready`. Snapshot records describe current state even when a lifecycle-type
filter excludes `state_changed`. Treat the snapshot as one transaction. Save its
cursor only after `ready`. Live records follow in server application order.
Provider timestamps do not determine this order.

Pass the last processed cursor through `--cursor` with the same filters.
Replay starts strictly after that cursor, then emits `ready` and live records.
The server retains at most 4,096 records and 8 MiB, even without subscribers.
Individual event payloads are limited to 64 KiB. Oversized payloads produce an
explicit `observation_uncertain` record with reason `payload_limit`, regardless of filters.
An oversized snapshot rejects startup. Narrowing filters can exclude that pane.
History exists only in memory. Expired history or a server restart produces
`gap`, followed by a fresh snapshot. A wrong owner, future sequence in the same
epoch, or changed filter set rejects the cursor.

`checkpoint` records advance filtered streams when a fact does not match.
Sequence gaps caused by filters do not indicate loss. Deduplicate replayed facts
by `(host_id, session, server_epoch, sequence)`. Snapshot and control records
share their boundary sequence and are not additional lifecycle facts.

The server sends startup and replay records in bounded batches. A subscriber
that exceeds its buffer or writer queue is disconnected. Reconnect from the last
processed cursor. Ctrl-C, broken stdout, EOF, or process exit removes the
subscription. Subscription loss never cancels agent work.

Existing `events` subscriptions keep their behavior. An empty event-type list
selects the existing plugin events only. Select `agent_lifecycle` explicitly.
`agent_event` continues to carry raw provider hooks. Mixed subscriptions have no
ordering guarantee between event families. Cursors cover only `agent_lifecycle`.
An older server rejects the unknown family. Upgrade that session before watching.

### Controller sequence

1. Start `agent watch --json` as a managed background command.
2. Wait for `ready` and retain the snapshot identity.
3. Submit a guarded prompt with a chosen submission ID:

   ```sh
   yinz --session work agent prompt terminal_7 --conversation "$CONVERSATION" \
     --submission review-42 --file task.txt --json
   ```

4. Match outcomes by pane generation, conversation, and submission ID.
5. Handle `needs_input` and `observation_uncertain` explicitly.
6. Save each cursor after processing its record.
7. On `gap`, reconcile current state before submitting more work.

One connection watches one session. Use separate streams and cursors for multiple
sessions. A Claude Code Monitor can own the background command where supported.
No particular Monitor API is required. The execution environment must deliver
notifications that wake the controller. Exactly-once controller side effects and
restart-surviving event history are outside this protocol.

Socket permissions are the same as for existing agent commands. Sandboxed Codex
needs the existing `--allow-yinz` launch option when its sandbox blocks access.
That option enables workspace sandbox network access. It is not a narrow read-only
permission. Watch does not change or bypass sandbox settings.

## Durable archives

`agent runs`, `run`, `history-search`, and `predecessors` query `project-store` and
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

Keyword search remains the default: full-text search runs first, with substring
fallback only when it finds no rows. Add `--regex` for Rust regex syntax:

```sh
yinz agent history-search 'panic|ERROR' --regex --project work
yinz agent history-search '(?i)rollback\s+failed' --regex --run-id RUN_ID
```

Regex matching is case-sensitive. Inline flags such as `(?i)`, `(?m)`, and `(?s)`
control case, line anchors, and matching across newlines. Each complete entry is
matched independently. Anchors refer to the entry unless multiline mode is set.
Surrounding pattern whitespace is significant. Empty patterns, NUL, invalid
syntax, look-around, and backreferences return errors, without keyword fallback.

Patterns are limited to 4 KiB, with 1 MiB limits for compiled regexes and the DFA
cache. Each search scans at most 4,000 candidate rows or 8 MiB of entry text,
newest stored entries first. Entries that exceed the remaining text budget are
not partially matched. These limits bound matching work, not database or network
latency. The existing fetch transport can return one oversized row.

Results retain the maximum of 50 hits. `truncated: true` can mean older entries
remain unsearched, the scope is incomplete, or more hits exist. Narrow the scope
to search further. There are no tuning flags or continuation cursor. Agent's
`agent_history_search` tool accepts the same mode as `"regex": true`.


### Durable tool history

New reports add `tool_call` and `tool_result` entries alongside conversation entries.
There is no historical import or new transcript capture. Automatic summaries still use only conversation entries.
`runs`, `run`, and `history-search` return JSON. `--kind` still selects the provider.

Each tool entry has optional `tool` metadata: `name`, `call_id`, `turn_id`, `worker_id`,
`parent_call_id`, `source_event`, `status`, `duration_ms`, and retained payloads.
Timestamps record receipt time. Status is `requested`, `completed`, `succeeded`,
`failed`, `cancelled`, or `denied`. A request does not prove execution.
`completed` does not prove success. Missing completions and unmatched results remain visible.
Stable worker/call/event identities suppress replay. Events without stable IDs remain separate.
`identity` hashes the original correlation fields. `metadata_truncated` and `metadata_redacted` describe changes to displayed metadata.

`arguments`, `output`, and `error` contain `text`, `format`, `original_bytes`,
`redacted`, `truncated`, and `attachments_omitted`. `format` is `text` or `json`.
Truncated JSON is a preview and may not parse. `output_unavailable` means no output body arrived.
Normal pages omit payload text and set `tool.payload_omitted: true`.
This display omission differs from irreversible capture truncation.
Retrieve one complete retained entry with:

```sh
yinz agent run RUN_ID --from-revision REVISION --limit 1
yinz agent history-search 'exec_command'
yinz agent history-search 'compiler error'
```

Arguments retain at most 16 KiB, output 64 KiB, and errors 8 KiB, including JSON encoding.
The tool object is bounded by 96 KiB. Tool metadata plus searchable text is bounded by 192 KiB.
Local and remote native input is limited to 2 MiB. Broadcasts remain limited to 256 KiB.
Redaction precedes truncation and search text generation. Credential fields, recognized textual
credentials, and private keys are replaced. Binary attachment bodies are omitted.
Yinz does not fetch resources or read files referenced by tool output.
Heuristic redaction cannot identify every secret in arbitrary text. Flags report transformations, not a secret-free guarantee.

Coverage includes events that Yinz receives. Claude can omit rejected calls.
Codex hosted tools lack hooks, and `write_stdin` can complete an earlier command without a new call hook.
Codex completion alone does not establish success. Muse payload limits can prevent hooks from starting.
Pi results during dialogs are archived without changing pane state.
OpenCode child calls retain worker attribution after verified ancestry lookup. Failed lookups suppress the report.
Antigravity uses `step:<stepIdx>` as its call ID and has no documented output body.
Other agents need an adapter that emits the shared hook contract. Executable detection alone does not capture tools.

Upgrade all Agent plugins and servers that share the database together.
The additive `agent_run_entry` v4 schema adds nullable JSON `tool` metadata.
Existing entries remain unchanged. Older declarations reject the new column as a removal.
Refresh installed adapters on each local or remote agent host:

```sh
yinz hooks install --agent pi
yinz hooks install --agent opencode
```

## Scheme agent plugins

Agent plugins are Scheme modules in the Steel runtime.
They register a descriptor with `yinz-agent-register-provider`. The host validates the
data and uses it for detection, launch, restart, and guarded terminal input.
The agent chooser and plugin APIs receive the same provider definitions.

Load an agent plugin with `shared true`. Registration requires `RunCommands`.
Each registration belongs to that plugin instance. Reload and unload remove its
registrations. Another plugin cannot replace an existing provider's kind or aliases.

```kdl
load_plugins {
    "file:/absolute/path/my-agent.scm" {
        shared true
    }
}
```

Define `agent-provider` when the plugin supplies installable hook files.
The hook installer reads this descriptor in a bounded subprocess. It does not
call `load` or permit host commands during module initialization.

```scheme
(define agent-provider
  (hash "kind" "my-agent"
        "executable" "my-agent"
        "aliases" '("my-agent-cli")
        "launch"
        (hash "model_args" '("--model" "{value}")
              "prompt_args" '("--prompt" "{value}")
              "resume_args" '("--session" "{value}")
              "resume_last_args" '("--continue")
              "permissions" (hash "manual" '("--approval" "manual")))))

(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("RunCommands"))
    (lambda (_)
      (yinz-agent-register-provider agent-provider))))
```

Argument templates contain individual arguments. Substitution of `{value}` never
splits the value or invokes a shell. Omitted templates reject unsupported options.
Use `restart` to declare option arities, conversation selectors, and commands that
must not restart. Unknown restart arguments fail without changing the pane.

Only declare `input` after the agent's hooks can acknowledge and complete specific
submitted turns. Input settings include `clear_key` (default `Ctrl c`), `submit_key`, `submit_delay_ms`,
`paste_prefix`, `newline`, and `command_prefixes`. The host retains process,
conversation, readiness, input ownership, and bracketed-paste checks.
Provider prompt acknowledgements and completions must carry a nonempty `turn_id`.
Completion must match the acknowledged turn. A paste write alone is not acceptance.
Use `yinz-agent-control` for guarded requests. `yinz-agent-send` and
`yinz-agent-prompt` also use guarded delivery for provider agents. Handle the
returned deferred with `yinz-then` and `yinz-catch` to observe submission failures.

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
yinz hooks install --agent ./my-agent.scm --dry-run
yinz hooks install --agent ./my-agent.scm
yinz hooks status --agent ./my-agent.scm
yinz hooks uninstall --agent ./my-agent.scm
```

OpenCode uses the bundled [Scheme provider](../yinz-steel-builtins/assets/plugins/agent-opencode.scm).
Yinz loads it before other background plugins. Its hook adapter uses JavaScript inside OpenCode and owns event
translation, parent-session filtering, and ordered reporting. Existing
`yinz hooks install --agent opencode` commands still work. Older installed
hook files remain accepted until reinstallation. OpenCode supports launch,
detection, restart, and lifecycle waits. It does not advertise reliable prompt
submission yet.

## Test an agent plugin

Use `yinz plugins test` with the [Scheme test harness](PLUGIN_TESTING.md).
Place unit checks in `.test.scm` files and real-session checks in
`.integration.test.scm` files beside the plugin. For example, save this as
`my-agent.test.scm`:

```scheme
(test "registers its provider" (lambda ()
  (test-load-plugin "./my-agent.scm"
    (hash "replies"
      (hash "RequestPluginPermissions" json-null
            "RegisterAgentProvider" json-null)))
  (test-event (hash "PermissionRequestResult" "Granted"))
  (define registrations
    (filter (lambda (command)
      (and (hash? command) (hash-contains? command "RegisterAgentProvider")))
      (test-commands)))
  (assert-equal (length registrations) 1)
  (assert-equal
    (hash-ref (hash-ref (car registrations) "RegisterAgentProvider") "kind")
    "my-agent")))
```

Run it with `yinz plugins test --file ./my-agent.scm --kind unit`.
For real-session checks:

- Declare `(hash "grants" '("RunCommands"))` in the test options.
- Use `(test-session "fixture" ...)` to create fake agent executables in an isolated workspace.
- Start with `(test-session "start" (hash "backgroundPlugins" (list (hash "path" "./my-agent.scm" "configuration" (hash "shared" "true")))))`.
- Use `(test-session "agent" request)` to start a real agent-control request, then poll its returned ID with `(test-session "agent-poll" id)`.
- Use `(test-session "report" ...)` to deliver canonical hooks through the normal report path.
- Observe terminal output with `test-wait-for-text` or `test-wait-for`.
- Inject stale, missing, and mismatched hooks while a prompt request is pending.

Agent controls and executable fixtures require the test's `RunCommands` grant.
Fixtures do not need an installed agent, credentials, a model service, or network access.
The [provider integration tests](../yinz-steel-builtins/assets/plugins/agent-opencode.integration.test.scm)
exercise registration, aliases, launch arguments, input bytes, custom Enter timing,
turn correlation, and status-only capabilities. Host unit tests also cover
validation, conflicting registrations, unload, restart refusal, input interruption,
timeouts, and hook-file ownership.


## Automatic draft replacement

An authorized `agent prompt` also authorizes replacement of an idle draft. Do not request separate discard permission.
CLI and plugin prompts use the same behavior. No additional flag or recovery command is required.
Automatic clearing captures the input revision internally and does not require three seconds of quiet.
Every provider supporting prompts clears before delivery, even when the composer is hidden or its layout is unrecognized.
The legacy `--preserve-draft` flag is accepted but ignored.
Active work, dialogs, and pending submissions still prevent prompt delivery.

The prompt validates all arguments and reserves its submission ID before writing.
It pastes a marker and waits for the write to complete. Then it sends the provider’s clear key.
After that write completes, it waits 250ms before delivering the requested text.
The marker makes an empty composer nonempty before clearing. Clearing does not require visual verification.
Providers must bind `clear_key` to clear the whole composer.
One timeout covers clearing and submission. Clearing failures are available through `wait --submission`, with acknowledgement false.
New input, conversation replacement, agent activity, caller disconnection, or writer failure cancels further delivery.
`ready` can remain false when Yinz cannot verify the composer. An explicit prompt still clears that input before delivery.
Status and idle waits remain read-only.

## Explicit draft clearing

Use `clear-draft` when you want to discard a draft without submitting a prompt.
Obtain authorization for that standalone discard.
Read status, inspect the composer, and use the exact revision you inspected:

```sh
yinz agent status terminal_7 --json
yinz agent clear-draft terminal_7 --conversation CONVERSATION_ID \
  --input-revision REVISION --confirm-discard --json
yinz agent prompt terminal_7 --conversation CONVERSATION_ID --file prompt.md
```

Run the prompt only after clearing returns `ok` and `ready: true`.
`input_revision` is optional for older servers. An absent revision is unknown, not zero.
`clear_draft_supported` defaults to false when an older server omits it.
It describes the current recognized layout, not a guarantee that verification will succeed.

The command requires three seconds without terminal input. Recent input causes immediate rejection.
A revision conflict requires new inspection and discard authorization. Never refresh the revision and retry automatically.
`--timeout` defaults to `30s`. `--interrupted` confirms that work stopped despite a stale Busy or Waiting report.
Interrupted recovery also requires input after the last lifecycle report and three seconds of quiet.
Background work and active submissions prevent clearing.

| Provider | Recognized layout |
|---|---|
| Claude Code | Default bordered composer with visible cursor. Placeholder text requires distinguishable dim styling. |
| Codex | Default `› ` composer and model footer with visible cursor |
| Pi | Built-in bordered editor with visible cursor (`PI_HARDWARE_CURSOR=1`) |
| Muse, AGY, custom providers/editors | Unsupported. Clear manually and use `recover --confirm-empty`. |

Recognition requires the live viewport and default editing bindings. Scrolled views, hidden cursors, dialogs,
and unfamiliar layouts fail closed. Ambiguous suggestion text, paste labels, attachments, and continuation rows never prove emptiness.
Codex 0.160.0’s fixed placeholder and Claude 2.1.289’s `Try "…"` placeholder require their captured dim style.
Identically spelled typed text remains a draft.
Claude suggestions and Pi's default hidden cursor can therefore prevent verified clearing.
Pi's existing `PI_HARDWARE_CURSOR=1` setting enables cursor verification.

For a nonempty composer, Yinz pastes an inert marker without Enter. After delivery and visible marker verification,
it sends exactly one Ctrl+C. Readiness returns only after a fresh, recognized empty composer remains stable for 250 milliseconds.
An already empty composer receives no editing input.

Human input cancels subsequent stages. Lifecycle changes, replacement, process exit, delivery failure, timeout,
and client disconnection also stop clearing. Queued writes retain ownership until they finish.
Failure can leave a partially cleared draft. Inspect the terminal before manual recovery.
Clearing preserves the conversation and does not verify task completion or promise deletion from provider history.
