---
name: yinz-agents
description: Launch, inspect, and coordinate Yinz agents with guarded prompts and correlated outcomes, and inspect their durable archives.
---

Inside a Yinz pane, inherited `YINZ_PANE_REF` resolves the current session and caller. Start with `yinz agent list --json`.
Outside a pane, run `yinz list-sessions --short` and select the intended name with the root option: `yinz --session NAME agent ...` (before `agent`).
`YINZ_SESSION_NAME` alone does not select a session. If pane context is missing or stale, pass `--session` explicitly.
A sandboxed Codex cannot reach Yinz ("connection was not permitted"). Spawn Codex orchestrators with `--allow-yinz`, which also allows internet access, or `--permission-mode skip`.
Use `agent list --project NAME` to filter by project. Explicit tab assignments take precedence over checkout discovery.
`yinz action project-list` lists remembered projects; `yinz action context` also shows them with the caller's pane.
Basic live operations do not require Hocus chat, `hocus-agentd`, or `hocus-store`.
Registration requires `hocus-agentd`. Archive queries require `hocus-store`.

Guarded prompts support Claude Code, Codex, Muse, Pi, and Antigravity (`agy`). Other kinds support launch and inspection.
Before a first Codex, Muse, or AGY launch, check `yinz hocus hooks status --agent KIND` and install missing hooks with `yinz hocus hooks install --agent KIND` on the target host.
Claude receives inline hooks; refresh older installed Claude hooks with the install command to prevent duplicate reports. Local Pi launches inject their reporting extension automatically; SSH Pi needs the extension installed remotely, and Firecracker does not provision it.
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

Use the actual `spawned.pane_id`, `spawned.cwd`, and `spawned.branch` returned by spawn.
If spawn fails, inspect `spawned` and `launch_resources`: a pane, branch, or worktree may already exist. Reconcile those resources before retrying. Guest-only VM launches can return `cwd: null`; inspect the guest rather than treating a host path as its directory.
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
The reply is `target_changed`; wait for idle, then use the new conversation ID. Other slash commands need `send-keys`, and typing `/clear` or `/new` through `send-keys` leaves the pane needing `recover`.

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

`agent spawn --file PATH` or `--file -` reads launch options from JSON. CLI options override file values.
Spawn rejects `prompt` and `reuse_prompt`, including fields in JSON. Use spawn, wait, then prompt.
Use `--reuse` only when the existing branch or worktree is the intended target.
Otherwise collisions produce a fresh branch and worktree. Spawn then reports `requested_branch` beside the actual `branch`.
A reused worktree reports `reused: true`, and `occupied_by` lists live agents already working there; two agents in one checkout can overwrite each other.
Yinz adds a nested worktree folder such as `.worktrees/` to the repository's local `.git/info/exclude`. Project defaults supply worktree directories and VM images.
Read `agent spawn --help` for model, effort, permissions, resume, and Firecracker options.
For SSH, provide an absolute `--cwd` and `--remote-host`. Verify the remote worktree and explicitly use `--allow-outside-worktree`.
SSH launches cannot use local project, branch, worktree, or VM options. General pane creation owns arbitrary programs.
Register an existing pane with `agent register PANE_ID KIND --json` for tracking. Registration alone does not enable guarded prompts or turn a shell-launched agent into a managed command pane.
`agent list` also shows an agent started by hand in a shell as source `detected` with `not_ready_reason: unmanaged`.
Yinz cannot prompt it; relaunch it with `agent spawn` to control it.

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
to search further. There are no tuning flags or continuation cursor. Hocus's
`agent_history_search` tool accepts the same mode as `"regex": true`.
