# Developing with coding agents

This fork already covers most of an agent's development cycle. The main gaps were between existing features:
finding an agent that needs input, moving a planned task into a working session, and choosing launch settings.
This change connects those steps through the existing launcher and agent panel.

The implementation has five main parts:

| Part | Role | Source |
| --- | --- | --- |
| Agent launcher | Select an agent, project, branch, worktree, and execution environment | `default-plugins/new-agent/src/main.rs`, `yinz-runtime/src/agent/launch.rs` |
| State and history | Receive agent hooks, track panes, record runs, and produce summaries | `default-plugins/agent-service/src/main.rs`, `default-plugins/agent-proto/src/agents.rs` |
| Agent panel | Browse live agents and archived runs, read transcripts, and resume conversations | `default-plugins/agents/src/main.rs` |
| Project memory | Share projects, nested todos, and linked notes across tools and the CLI | `default-plugins/project-store/`, `default-plugins/project-data/src/todos.rs`, `yinz-cli/src/notes_todos.rs` |
| Development workflows | Run project-owned validation, review, repair, and integration steps | `yinz-deno-builtins/assets/plugins/workflow-view.ts`, `workflows/`, `extensions/models/` |

Use the shipped keybindings to open **New agent** with `Ctrl e`, then `n`.
Open the agent panel with `Ctrl e`, then `a`.
Custom configurations can bind the `new-agent` and `agents` aliases to other keys.

The panel is the agent list on the left and the agent conversation on the right.
The list has two tabs: **Agents** and **Todos**.
Its own tab bar is the keyboard route between them: `Shift+Tab` from a tab's list reaches the bar, `←` and `→` switch tabs, and `Enter`, `↓`, `Tab`, or `Esc` return to the tab's list.
`Tab` and `Shift+Tab` move focus between the panel tab bar and the active view.
Press `?` in the list for a key sheet of every key the current tab answers.
A hint line too narrow for all of its keys drops whole keys from its end.
The Agents footer ends with `? keys` when there is room.
Type `/keys` in the conversation for every key in the panel.

`Esc` steps back before it hides the panel: it leaves an event being read, a question, a search or filtered view, the tab bar, or the key sheet first.
Clicking a pane under the panel, or moving the keyboard to one, also puts the panel away.
The panel follows you to another tab without the keyboard.
Moving the keyboard to another pane there puts it away, but clicking the pane that already has the keyboard changes nothing: click the panel or press `Ctrl e a` first, then `Esc`.
`Ctrl e a` brings it back on the same tab, with the keyboard in that tab's list. `Ctrl e o` moves it one corner clockwise.
Drag the divider, or type `/split 40` or `/split 35%` in the conversation, to change how the width is shared.
The `width`, `height`, `split`, and `pad` keys in the `agent` block of `config.kdl` set the starting size (60%, 66%, 50%, and 3 by default).
agent reports a value it cannot read in the conversation and uses the default instead.

In the **Agents** list, press `w` to show only the agents waiting for input.
While an agent is waiting, the list and history footers start with `w waiting (N)`; a search or previous-runs view shows it right after `Esc/c clear`, and the waiting view reads `Esc/w all · N waiting`.
With nobody waiting, `w waiting (0)` follows the selection's own keys. A narrow pane can cut the count, like any other key.
The waiting view keeps project and branch labels and updates when agent state changes.
An agent stays in the view after it is answered, marked `no longer waiting`, until you move the selection off it.
The view stays on after `Enter` and after the panel is hidden. `w`, `c`, or `Esc` returns to the full list, and `/` or `p` leaves it for a search or the agent's previous runs.
Press `Enter` to focus the selected live agent, which also puts the panel away.
On an archived run, `Enter` moves into the run's history, and a second `Enter` reads the event.
Use `/` to search history and `r` to resume a selected archived run. A resume that starts puts the panel away.
`D` and `F` open **New agent** to duplicate or fork the selected agent.
The panel stays up under the launcher, so `Esc` in the launcher returns the keyboard to the Agents list; an agent it starts is focused, which puts the panel away.
The footer changes with the current selection to expose these actions.

Waiting means the same thing as the existing `!` marker and the `agent_list` tool's `awaiting_confirmation` field.
An agent hook takes precedence. Without hooks, the existing fallback requires an idle pane and a summary matching its current screen.
An exited agent or a stale summary does not count as waiting. The `◆` marker identifies agents with a hook feed.

In the **Todos** tab, select a task and press `n` to open **New agent**.
The launcher uses that todo's project, even when the panel was opened from another directory.
Its prompt starts with `Todo #ID:` and the task, then adds parent context and unfinished subtasks.
Review or edit the prompt, choose the agent and worktree, then select **Create agent**.
The panel stays up under the launcher, so `Esc` in the launcher returns the keyboard to the Todos list.
Launching does not change the todo's completion status. Complete an open task with `Space` after checking the result.
`Enter` edits a todo. The current project is named above the list, and `p` picks another.

The launcher now offers **Reasoning effort** and **Approval mode** alongside **Model**.
The choices come from the existing launch planner and change with the selected agent.
The default choice preserves the agent's own configuration. Selecting an agent does not carry another agent's permission override forward.
Cursor requires an explicit model when an effort is selected. Unsupported combinations produce a launch error.

Duplicate and fork still start fresh conversations in the source pane's location.
They now use the shared launch planner, which supplies Claude's state hooks on local and SSH launches.

Each session can have one **mother**: a coding agent in an overlay pane that is available on every tab.
Press `Ctrl e`, then `m`, to toggle it. With no mother running, the key opens the command palette at the agents to launch it as.
**Launch mother…** in the command palette opens the same list, and becomes **Toggle mother** once one is running.
Mother starts a fresh conversation in `YINZ_CACHE_DIR/mother/<session-key>/work`, without a worktree.
The key is a stable hash of the session name. Each session has a separate private folder.
The folder and its contents persist after mother exits and are reused on later launches. There is no automatic cleanup.
If Yinz cannot create the folder, the launch fails. The pane title is `mother: <agent>`.

On Linux, mother automatically attempts Bubblewrap isolation without project configuration or Git.
The sandbox can read host files. It can write only its working folder, private temporary storage, and the selected agent's state directories.
Codex, Claude, pi, and opencode have supported state layouts. Their configuration directory overrides are honored, including `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `PI_CODING_AGENT_DIR`, and the XDG directories.
Claude requires `/dev/fuse`, `fusermount3`, and `flock` (util-linux), with permission to mount a private FUSE filesystem.
Each launch uses a private copy-on-write view of the host configuration. Changes to hooks, skills, and settings stay private.
The `.credentials.json` file remains shared with the host. Credential writes persist immediately, including atomic replacements.
When the host replaces credentials, the next open in a running mother sees the replacement. Already-open descriptors keep their original file.
Credential symlinks, hard links, and special files are rejected. Replacement credentials have mode `0600`.
Concurrent writers follow normal filesystem ordering. Yinz does not coordinate Claude's OAuth refresh requests.

Complete Claude setup on the host first. Missing or invalid global settings stop the launch.
Yinz retries briefly if a host settings write is incomplete. Each launch validates one settings snapshot and reuses it for probes and execution.
Host account and MCP configuration changes apply to the next launch. Settings changes inside mother expire when it exits.
Conversation transcripts under `projects` persist in session-private storage. The global command-history index does not persist.
Claude's cache and state variables use private temporary storage for logs, locks, and staging. Automatic updates are disabled.
If the configuration filesystem service stops, Yinz stops the running sandbox. Termination releases its mount and temporary files. A separate cleanup process releases the mount if the launcher is killed.
Each provider has a separate saved sandbox profile. Switching providers keeps earlier profiles available for resuming runs from history.
Other providers, including plugin providers, currently use the local fallback because their complete writable state layouts are not mapped.
If Bubblewrap is unavailable or runtime probes fail (including unavailable FUSE mounts), mother launches locally and prints the reason in its pane. Claude configuration and validation errors stop the launch instead.
This fallback provides no filesystem isolation. A started agent is never retried locally.

Other providers still share agent state with host agents. Network access, hooks, and agent permissions remain unchanged.
Mother can still request host changes through Yinz. Bubblewrap restricts direct filesystem writes, not Yinz control requests.

Toggling shows mother on the tab you are on and gives it the keyboard, or hides it when it already has the keyboard.
A visible mother follows you when you switch tabs, unless another client is still on the tab you left. A hidden one stays hidden until you toggle or focus it.
If another client has mother focused, your tab switches, toggles, and focus requests cannot move or hide it.
A toggle or focus request from another tab takes you to mother's tab instead, where you share it.
Ordinary pane focus requests also fetch mother into your current tab and show it, including when it is hidden, without toggling it off.
While an editor or in-place plugin replaces mother, toggles control that replacement.
The replacement carries mother with it across tabs, and closing it restores mother.
A mother that was parked, or brought back from parking as a floating or tiled pane, returns to the overlay on the next toggle.

Mother's overlay has **Conversation** and **Overview** tabs. Conversation opens first and keeps the live terminal and drafts.
Use the existing pane-tab keyboard controls or click the strip to switch. Selection persists while the overlay exists.
Overview shows supervision state, the saved assignment, workers, attention items, and bounded lifecycle history.
Use Conversation for new assignments, worker launches, and permission decisions.
A panel load failure leaves Conversation available. Select Overview and press `r` to retry.

Automatic turns can run while Overview is selected. They wait while any client focuses Conversation, a draft exists, or Mother is busy.
Another client cannot switch away from a Conversation that you are using.
`yinz mother pause` stops future automatic turns and keeps observation active. It does not interrupt a running turn.
`yinz mother resume` permits future automatic turns. `yinz mother configure` replaces all filters, including empty filters.
Missing lifecycle hooks or startup dialogs require setup in Conversation. Install hooks with `yinz hooks install`.

`yinz mother overview` and `status` inspect supervision without changing batches or checkpoint memory.
The inbox contains a bounded journal page. Use `yinz mother journal --after <cursor> --through <high_water>` for subsequent pages.
Pages default to 50 records, permit at most 200, and have a 256 KiB record budget. Oversized records and retention gaps are explicit.
Journal cursors persist across restarts. Read all pages through the inbox boundary before acknowledging its batches.
A turn ending does not prove completion. Inspect correlated results and available archives. Missing archives or results remain unknown.

If a supervision result is unavailable, inspect its receipt and workers before continuing.
Then use Overview's review control or `yinz mother resolve <submission-id> --expect-rev <revision>`.
Work may already have occurred. Recovery preserves the unknown outcome, releases the wait, and queues reconciliation without replaying the prompt.
Stale revisions and operations still in flight cannot be resolved.

Checkpoints persist goals, scope, assignments, decisions, blockers, questions, and journal progress across conversation resets.
Checkpoint writes require the revision the author read. Legacy checkpoint fields remain readable and are preserved during upgrades.
Codex Mother uses the same network setting as `agent spawn --allow-yinz`. This permits Yinz socket connections and internet access.
Other approval and sandbox settings remain in effect.

Mother is the pane, so `/clear`, `/new`, or a replaced conversation keeps it.
Once its agent exits, the pane stops following you. The next toggle brings what is left of it to your tab and offers a new mother over it.
Launching one closes the old pane. Dismissing the offer leaves the pane to read or to restart the agent in.
Closing the pane, or moving it to another session, also ends it.
Mother uses the same server-side guards as other agents. Its Claude and Codex launches pre-approve specific Yinz coordination commands, effective on the next launch. Worker permission answers require explicit user delegation saved in its checkpoint. Session resurrection does not restore it.

`yinz agent spawn` preserves the attached client's active pane and tab.
Workers use tiled placement. When space is insufficient, the worker uses another tab or creates a tab in the background.

For managed Codex launches, do not combine `--resume ID` or `--resume last` with `--permission-mode`.
Omit `--permission-mode` to resume through Yinz's bridge, or start a new conversation with the requested mode.
Yinz rejects this combination before it creates a pane, branch, or worktree.
JSON launches with `hooks:false` bypass the bridge and retain native Codex argument handling.

On each host with installed Claude hooks, rerun `yinz hooks install --agent claude` after this update.
Refresh project installations with `--project` or `--project-dir PATH` too.
Version 10 scripts skip reporting when a structured launch supplies inline hooks.
Older scripts still produce duplicate reports until refreshed. Third-party hooks remain unchanged.
Inline hooks report through the broker on SSH panes and through the local session on local panes.
Ordinary shell launches continue to use installed hooks.

Pane replacement now replays terminal output that arrives before the replacement is placed.
Buffered output is applied before newer output, so a fast startup cannot leave a blank replacement or reverse its first output chunks.

The next useful improvements need more than a shortcut:

- **Persistent task ownership:** associate a todo with a run, then show its outcome and prevent accidental duplicate dispatch.
  This needs rules for resumed runs, several agents on one task, and shared database ownership.
- **Build coordination:** show which runs are testing the same checkout or using the same build directory.
  A useful implementation needs actual process and artifact ownership, including project-specific build commands.
- **Hook diagnostics:** distinguish missing hooks, missing transcript access, and an agent that has not reported yet.
  Existing hook status commands and the `◆` marker provide pieces of this, but the panel lacks a guided recovery action.
- **Summary request results:** report a rejected or failed summary request directly in the panel.
  The current request relies on later run updates. After about 70 seconds without a new summary, the panel says `no new summary · s to try again`, but it cannot tell a rejected request from a slow one.

These follow-ups should extend the existing run, store, and workflow ownership mechanisms.
The first iteration adds no task scheduler, new database schema, or separate build runner.

Tool history includes `tool_call` and `tool_result` entries, with worker attribution and bounded,
redacted arguments, output, and errors. Search retained names or payload text with
`yinz agent history-search 'exec_command'`. Read the complete retained payload with
`yinz agent run RUN_ID --from-revision REVISION --limit 1`. Normal pages set
`tool.payload_omitted`, which differs from capture truncation. Upgrade all database-sharing
Agent plugins and servers together for schema v4. Refresh Pi/OpenCode hooks on remote hosts too.
See [the agent CLI guide](AGENT_CLI.md#durable-tool-history) for payload fields, limits, redaction caveats, provider gaps,
and adapter refresh commands. No historical import is performed.
