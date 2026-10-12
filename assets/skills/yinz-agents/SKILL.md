---
name: yinz-agents
description: Launch, inspect, and coordinate Yinz agents with guarded prompts and correlated outcomes, find the session's mother agent, and inspect their durable archives.
---

Inside a Yinz pane, inherited `YINZ_PANE_REF` resolves the current session and caller. Start with `yinz agent list --json`.
Outside a pane, run `yinz list-sessions --short` and select the intended name with the root option: `yinz --session NAME agent ...` (before `agent`).
`YINZ_SESSION_NAME` alone does not select a session. If pane context is missing or stale, pass `--session` explicitly.
A sandboxed Codex cannot reach Yinz ("connection was not permitted"). Spawn Codex orchestrators with `--allow-yinz`, which also allows internet access, or `--permission-mode skip`.
Use `agent list --project NAME` to filter by project. Explicit tab assignments take precedence over checkout discovery.
`yinz action project-list` lists remembered projects; `yinz action context` also shows them with the caller's pane.
Basic live operations do not require `agent-service` or `project-store`.
Registration requires `agent-service`. Archive queries require `project-store`.

Guarded prompts support Claude Code, Codex, Muse, Pi, and Antigravity (`agy`). Other kinds support launch and inspection.
Before a first Codex, Muse, or AGY launch, check `yinz hooks status --agent KIND` and install missing hooks with `yinz hooks install --agent KIND` on the target host.
Claude receives inline hooks; refresh older installed Claude hooks with the install command to prevent duplicate reports. Local Pi launches inject their reporting extension automatically; SSH Pi needs the extension installed remotely.
AGY completion additionally requires its hooks to inherit `ANTIGRAVITY_LS_ADDRESS` and `ANTIGRAVITY_CSRF_TOKEN` for the local completion API. AGY CLI 1.2.14 does not supply these automatically; without them completion cannot be confirmed.
Do not replace unavailable guarded prompting with raw input. Use general pane input only for deliberate interaction with unsupported kinds.

A pane ID identifies the current terminal target, such as `terminal_3` (`3` also works).
A conversation ID identifies the provider conversation and guards against replacement.
A submission ID identifies one prompt operation. A turn ID comes from the provider and may be absent.
A run ID identifies a durable archive record. Do not substitute one ID for another.

Choose the requested provider and its model. Select the actual project, a task-specific branch, and the intended Git baseline before launch.
`--branch` creates a separate worktree from `main` by default. Pass `--base REF` explicitly when delegating work from another branch or commit. Uncommitted changes are not copied into the new worktree.
For a local checkout without a remembered project, use `--cwd PATH` instead of `--project NAME`.

```sh
yinz agent spawn --kind KIND --project PROJECT --branch TASK_BRANCH --base BASE_REF --json
yinz agent wait terminal_3 --until idle --timeout 2m --json
yinz agent prompt terminal_3 --file prompt.md --conversation CONVERSATION_ID --submission SUBMISSION_ID --wait --timeout 10m --json
```

Claude Haiku launch example:

```sh
yinz agent spawn --kind claude --model haiku --cwd . --branch review-auth --base main --json
```

Use the actual `spawned.pane_id`, `spawned.tab_id`, `spawned.cwd`, and `spawned.branch` returned by spawn.
`spawned.created_tab_id` identifies a tab created by this launch, including automatic overflow.
If spawn fails, inspect `spawned` and `launch_resources`: a pane, tab, branch, or worktree may already exist. Reconcile those resources before retrying.
The idle wait already returns status in `agents`; use its `agent.session_id` as the conversation guard and check `ready`, `status_available`, and `prompt_supported` before prompting.
Completed prompt responses include the answer in `message` when the provider reports it. Use `status` for a fresh observation and `read` to diagnose missing readiness, dialogs, or missing output; neither is an automatic extra step.
If startup remains `Spawned` or reports `status_unavailable`, read the pane and check hooks. Answer a visible startup dialog deliberately with `send-keys`, within the user's authorization, then wait again. A Codex worktree trust prompt accepts `y Enter`; do not approve trust without authorization.
For a multi-line prompt, write it to a file or pass a heredoc with `--file -`:

```sh
yinz agent prompt terminal_3 --file - --conversation CONVERSATION_ID --wait --json <<'EOF'
Fix the failing test in calc.py, then run the tests.
EOF
```

Muse hook verification alone does not establish readiness. Before the first prompt, Yinz requires an empty Muse 1.4.2 composer with a visible cursor after `❯ `. Dialogs, drafts, scrolling, and unknown layouts remain unready. Yinz never changes workspace trust automatically.

A fresh Muse pane reports no conversation until its first turn binds one: submit that first prompt unpinned, then pin `--conversation` afterwards.
An idle state alone does not establish readiness or prove task success.
A completed turn does not verify the resulting work. Run the checks that the task requires.

Live control commands return `AgentResponse` with `--json`. Registration returns the service's JSON acknowledgement; its `--json` flag also makes errors structured. Check `outcome` even when a control command exits nonzero.
`submitted` means the provider acknowledged the prompt. `completed` means that correlated turn ended.
Handle `needs_input`, `failed`, `interrupted`, `target_changed`, and `target_closed` before continuing.
Save `submission_id` from prompt responses. To retain it even if the command's output is lost, choose a unique `--submission ID` before sending (1–128 ASCII letters, digits, `.`, `_`, or `-`). The CLI otherwise generates an ID.
After `needs_input`, read the dialog, answer deliberately with `send-keys` within the user's authorization, then retrieve the original turn by its submission ID. Use the same retrieval after `timed_out` or a lost connection:

```sh
yinz agent wait terminal_3 --submission SUBMISSION_ID --conversation CONVERSATION_ID --timeout 10m --json
```

This returns the original turn's outcome, including failure; `acknowledged` and `turn_id` describe that submission. It can return `needs_input` again while a dialog remains.
For parallel workers, submit without `--wait`, save each pane/conversation/submission triple, and wait on those submissions later.
Do not combine `--submission` with `--until`. A plain `wait --until idle` observes readiness and returns the last completed turn's text, which does not establish the outcome of your task.
Only the last 32 submissions per pane/conversation are retained in memory; target replacement, exit, closure, and server restart discard them. Unknown or expired IDs return `rejected`. A retained ID cannot be submitted again. An unavailable result does not prove the work never ran: inspect before any retry rather than treating the ID as durable deduplication.
When Claude runs a command or subagent in the background, `completed` carries only its interim text and lists `background_tasks`.
Use a separate `wait --until idle --conversation ID` for that background work's final text. A long-running background server keeps the state wait pending until its timeout.

To start a fresh conversation in the same pane, prompt exactly `/clear` (Claude) or `/new` (Codex or Pi).
The reply is `target_changed`; wait for idle, then use the new conversation ID. Other slash commands need `send-keys`, and typing `/clear` or `/new` through `send-keys` does not establish readiness.

`agent read --lines N` returns at most N terminal lines, including internal blank rows.
Trailing blank rows do not count toward the limit. Blank rows at both ends are removed.
Whitespace-only rows count as blank. Text and JSON retain the same content, including ghost suggestions.
Plain terminal text cannot establish whether the composer contains a draft. Do not infer a draft from suggestion text or strip suggestions heuristically.
If recovery is necessary, inspect the composer in the terminal UI and clear any actual draft deliberately.
Only then use `agent recover PANE --conversation ID --confirm-empty --json`, after confirming the conversation is unchanged.
If your tools expose only `agent read` text, ask the user to verify the stopped agent and empty composer in the terminal before recovering. Esc alone is not verification and can restore an interrupted prompt into the composer.

For `send-keys`, pass one key per argument: `Esc`, `Enter`, or `Down Enter`. Quote modifiers such as `"Ctrl u"`.
`Ctrl+u`, `ctrl-u`, `C-u`, `Escape`, and `Return` are unsupported aliases.
Recovery requires a live agent and no active submission or paste write. It normally requires idle; `--interrupted` permits the stopped busy/waiting case described below.
Recovery ends unresolved submissions as `interrupted`; it does not prove whether their work ran. Inspect that work before deciding whether to submit a new task.
Pass `--conversation ""` when the pane reports no session (fresh Muse pane before its first turn); `send-keys` dialog answers need no recovery once later output shows the empty composer, provided no newer manual input arrived.
Claude reports nothing when Esc interrupts a turn, so it stays `Busy`. After verifying in the
terminal UI that the agent stopped, clear any draft (Esc can restore the interrupted prompt), then add
`--interrupted` to reset it to idle. This also applies to a `Waiting` agent whose dialog you
dismissed. It needs input, such as Esc, after the agent's last hook and 3 seconds without a
hook since.

When a worker is done, close its pane: `yinz action close-pane --pane-id terminal_3`.
Closing the pane leaves its branch and worktree in place. Remove them only when the work is merged or
deliberately discarded and no pane still runs there: `git worktree remove PATH`, then `git branch -d BRANCH`.

Choose the initial destination with `--tab ID` or `--new-tab [NAME]`. These options are mutually exclusive.
`--tab 0` is valid. IDs identify tabs in the selected Yinz session, not tab-bar positions.
Existing-tab names are not supported. A missing or closed destination fails without fallback, even if another tab receives the same ID.
`--new-tab` creates an unfocused tab. An optional name must be nonblank and contain no control characters.
Without either option, automatic placement can use a roomier tab or create one.
All forms preserve keyboard focus for every attached client. Explicit tiled insertion can end fullscreen in the destination tab.
A matching running server is required. The CLI checks support before sending explicit placement.

`--project` selects checkout settings independently of the destination tab. Spawn does not assign or change the tab's project.
For `agent list --project`, an assigned tab project still takes precedence over checkout discovery.
Thus, a pane launched from one project's checkout can appear under another project's assigned tab.
Placement is temporary. A later pane move or `arrange-panes-by-project` can relocate the pane.

```sh
yinz agent spawn --kind claude --project work --branch review --tab 7 --json
yinz agent spawn --kind claude --cwd . --branch review --new-tab review --json
```

On failure, `launch_resources.tab_id` is a known destination, not proof that a live pane exists there.
`launch_resources.created_tab_id` identifies a created tab that remains or whose cleanup is uncertain.
Yinz removes only an unused, empty, launch-created tab after pending layout work has finished.
Tabs with layout content, other activity, or uncertain ownership remain. The failure message describes cleanup.
Branches and worktrees remain for recovery. After a timeout, inspect the session before retrying. A late launch can still complete.
The existing create-tab, spawn, then `pane-move-to-tab` sequence remains available for recovery.
Wasm and Deno `SpawnAgent` APIs retain automatic placement.

`agent spawn --file PATH` or `--file -` reads launch options from JSON. CLI options override file values.
JSON placement accepts `"tab": 7`, `"new_tab": true`, or `"new_tab": "review"`.
Null means unspecified. `"new_tab": false` does not request a tab.
A CLI placement option replaces the file's entire placement choice.
Spawn rejects `prompt` and `reuse_prompt`, including fields in JSON. Use spawn, wait, then prompt.
Use `--reuse` only when the existing branch or worktree is the intended target.
Otherwise collisions produce a fresh branch and worktree. Spawn then reports `requested_branch` beside the actual `branch`.
A reused worktree reports `reused: true`, and `occupied_by` lists live agents already working there; two agents in one checkout can overwrite each other.
Yinz adds a nested worktree folder such as `.worktrees/` to the repository's local `.git/info/exclude`. Project defaults supply worktree directories.
Read `agent spawn --help` for model, effort, permissions and resume options.
For SSH, provide an absolute `--cwd` and `--remote-host`. Verify the remote worktree and explicitly use `--allow-outside-worktree`.
SSH launches cannot use local project, branch, or worktree options. General pane creation owns arbitrary programs.
Register an existing pane with `agent register PANE_ID KIND --json` for tracking. Registration alone does not enable guarded prompts or turn a shell-launched agent into a managed command pane.
`agent list` also shows an agent started by hand in a shell as source `detected` with `not_ready_reason: unmanaged`.
Yinz cannot prompt it; relaunch it with `agent spawn` to control it.

A session has at most one mother: an agent in an overlay pane that the user toggles with `Ctrl e`, then `m`, on any tab.
Only the user launches it, from that key or the command palette's **Launch mother…** entry. No CLI command creates, registers, or toggles a mother.
Mother starts in a persistent private folder at `YINZ_CACHE_DIR/mother/<session-key>/work`, with the pane title `mother: <agent>`.
It attempts Bubblewrap isolation automatically. Unsupported agent state layouts or sandbox setup failures produce a visible explanation and a local, unsandboxed launch.
Sandboxed mother can write its own folder and shared agent state, and can still request host changes through Yinz.
Mother is listed by `agent list` like any managed agent and uses the same server-side guards. Its Claude and Codex launches pre-approve specific Yinz coordination commands; worker permission answers still require explicit user delegation.
It stays mother across `/clear`, `/new`, and conversation changes. After its agent exits, the user's next toggle shows the pane and offers a new mother, and launching one closes the old pane; closing the pane or moving it to another session also ends it. Session resurrection does not restore it.
Plugins see its terminal pane ID in `SessionInfo.mother`, which is `null` when none is running.
`SessionInfo.mother_supervision` reports watching, processing, queued, paused, degraded, or needs_attention with pending and blocker counts.
Mother receives a local yinz-mother skill and a durable inbox. Use `yinz mother status`, `inbox`, `checkpoint`, `ack`, `configure`, `pause`, and `resume` to inspect and control supervision. Automatic deliveries never clear drafts or interrupt active turns.

```sh
yinz agent runs --project work --limit 20 --offset 0
yinz agent run RUN_ID --from-revision 1 --limit 20
yinz agent history-search 'rollback' --project work
yinz agent predecessors RUN_ID
```

Archive commands always return JSON; `--json` is accepted and also reports their errors as JSON.
With `--json`, live commands also report input errors, such as an empty prompt, as a `rejected` `AgentResponse`. Malformed arguments still print plain errors.
For `runs`, continue with `--offset NEXT_OFFSET` while `more` is true. For `run`, pass `next_revision` as `--from-revision` while `more` is true. Preserve filters across pages.
`history-search` returns `truncated` rather than a continuation cursor; narrow the filters or query when it is true.
Use both `--host-id HOST` and `--run-session SESSION` for one server's records. The root `--session` selects the server handling the query; it does not filter the archive.
Live `agent list --json` entries include `host_id` and `session`; archive run headers include them too. Use those values rather than substituting a hostname or provider conversation ID.
Workflow runs are separate from agent turns and agent archive runs.

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

Tool history includes `tool_call` and `tool_result` entries, with worker attribution and bounded,
redacted arguments, output, and errors. Search retained names or payload text with
`yinz agent history-search 'exec_command'`. Read the complete retained payload with
`yinz agent run RUN_ID --from-revision REVISION --limit 1`. Normal pages set
`tool.payload_omitted`, which differs from capture truncation. Upgrade all database-sharing
Agent plugins and servers together for schema v4. Refresh Pi/OpenCode hooks on remote hosts too.
See [the agent CLI guide](../../../docs/AGENT_CLI.md#durable-tool-history) for payload fields, limits, redaction caveats, provider gaps,
and adapter refresh commands. No historical import is performed.

An authorized `agent prompt` already authorizes replacing an idle draft. Do not ask for separate discard permission.
Submit the guarded prompt directly. Claude, Codex, and Pi default composers support automatic clearing.
The command verifies that the composer is empty before delivery. It does not interrupt active work or answer dialogs.
One timeout covers clearing and submission. Inspect failures with `wait --submission`; clearing failures have acknowledgement false.
Unsupported providers or layouts return a clearing limitation. Status and idle waits are read-only.

For standalone draft discard without a prompt, obtain authorization before calling `agent clear-draft`.
Inspect `agent status TARGET --json` and the composer.
Pass the exact `input_revision` and conversation from that inspection:

```sh
yinz agent clear-draft TARGET --conversation ID --input-revision REV --confirm-discard --json
```

Never substitute zero for an absent revision. Never refresh a stale revision and retry automatically.
Wait for three seconds without input before the request. Only `ok` with refreshed `ready: true` permits a guarded prompt.
Default timeout is 30 seconds. Add `--interrupted` only after verifying that work stopped despite stale Busy/Waiting state.
That flag also requires input after the last lifecycle report and three seconds of quiet.

Clearing recognizes Claude Code, Codex, and Pi default composers with visible cursors and default bindings.
Muse, AGY, custom editors, hidden cursors, and unfamiliar layouts require manual clearing and `recover --confirm-empty`.
Ambiguous suggestions, attachments, and paste labels do not prove emptiness.
Pi needs its existing `PI_HARDWARE_CURSOR=1` setting for cursor verification. Verification can fail after partial clearing.
Inspect failures instead of retrying. Clearing preserves the conversation but does not verify task completion or remove provider history.

Use `yinz agent prompt --preserve-draft` for supervised worker assignments. This refuses a draft or unknown composer without clearing input. Save a submission ID before delivery and inspect that ID after a lost reply instead of resending the prompt.
