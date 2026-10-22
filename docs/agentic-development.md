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
| Projects | Remember projects and manage their configuration through the CLI | `yinz-steel-builtins/assets/plugins/project-store.scm`, `default-plugins/project-data/src/projects.rs`, `yinz-cli/src/projects.rs` |

Use the shipped keybindings to open **New agent** with `Ctrl e`, then `n`.
Open the agent panel with `Ctrl e`, then `a`.
Custom configurations can bind the `new-agent` and `agents` aliases to other keys.

The panel is the agent list on the left and the agent conversation on the right.
The panel contains the **Agents** list.
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

The launcher now offers **Reasoning effort** and **Approval mode** alongside **Model**.
The choices come from the existing launch planner and change with the selected agent.
The default choice preserves the agent's own configuration. Selecting an agent does not carry another agent's permission override forward.
Cursor requires an explicit model when an effort is selected. Unsupported combinations produce a launch error.

Duplicate and fork still start fresh conversations in the source pane's location.
They now use the shared launch planner, which supplies Claude's state hooks on local and SSH launches.

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

These follow-ups should extend the existing run and store ownership mechanisms.
The first iteration adds no task scheduler, new database schema, or separate build runner.

Tool history includes `tool_call` and `tool_result` entries, with worker attribution and bounded,
redacted arguments, output, and errors. Search retained names or payload text with
`yinz agent history-search 'exec_command'`. Read the complete retained payload with
`yinz agent run RUN_ID --from-revision REVISION --limit 1`. Normal pages set
`tool.payload_omitted`, which differs from capture truncation. Upgrade all database-sharing
Agent plugins and servers together for schema v4. Refresh Pi/OpenCode hooks on remote hosts too.
See [the agent CLI guide](AGENT_CLI.md#durable-tool-history) for payload fields, limits, redaction caveats, provider gaps,
and adapter refresh commands. No historical import is performed.
