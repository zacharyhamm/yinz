# Developing with coding agents

This fork already covers most of an agent's development cycle. The main gaps were between existing features:
finding an agent that needs input, moving a planned task into a working session, and choosing launch settings.
This change connects those steps through the existing launcher and agent panel.

The implementation has five main parts:

| Part | Role | Source |
| --- | --- | --- |
| Agent launcher | Select an agent, project, branch, worktree, and execution environment | `default-plugins/hocus-new-agent/src/main.rs`, `zellij-runtime/src/agent/launch.rs` |
| State and history | Receive agent hooks, track panes, record runs, and produce summaries | `default-plugins/hocus-agentd/src/main.rs`, `default-plugins/hocus-proto/src/agents.rs` |
| Agent panel | Browse live agents and archived runs, read transcripts, and resume conversations | `default-plugins/hocus-agents/src/main.rs` |
| Project memory | Share projects, nested todos, and linked notes across tools and the CLI | `default-plugins/hocus-store/`, `default-plugins/hocus-data/src/todos.rs`, `zellij-cli/src/notes_todos.rs` |
| Development workflows | Run project-owned validation, review, repair, and integration steps | `zellij-deno-builtins/assets/plugins/workflow-view.ts`, `workflows/`, `extensions/models/` |

Use the shipped keybindings to open **New agent** with `Ctrl e`, then `n`.
Open the agent panel with `Ctrl e`, then `a`.
Custom configurations can bind the `hocus-new-agent` and `hocus-agents` aliases to other keys.

The panel is the agent list on the left and the hocus conversation on the right.
The list has four tabs: **Agents**, **Todos**, **VMs**, and **Swamp**.
Its own tab bar is the keyboard route between them: `Shift+Tab` from a tab's list reaches the bar, `←` and `→` switch tabs, and `Enter`, `↓`, `Tab`, or `Esc` return to the tab's list.
`Tab` and `Shift+Tab` walk one ring of five stops: the panel's tab bar, the tab's list, its second half (the history on Agents, the details on VMs), the conversation's tab bar, and the conversation's input box.
Press `?` in the list for a key sheet of every key the current tab answers.
A hint line too narrow for all of its keys drops whole keys from its end.
The Agents footer, the Swamp list header, and the VMs divider give up trailing keys to end with `? keys`; other hint lines add it only when there is room left, and a line drawn while you type into a box, where `?` is a character, has none.
Type `/keys` in the conversation for every key in the panel.

`Esc` steps back before it hides the panel: it leaves an event being read, a question, a search or filtered view, the tab bar, or the key sheet first.
Clicking a pane under the panel, or moving the keyboard to one, also puts the panel away.
The panel follows you to another tab without the keyboard.
Moving the keyboard to another pane there puts it away, but clicking the pane that already has the keyboard changes nothing: click the panel or press `Ctrl e a` first, then `Esc`.
`Ctrl e a` brings it back on the same tab, with the keyboard in that tab's list. `Ctrl e o` moves it one corner clockwise.
Drag the divider, or type `/split 40` or `/split 35%` in the conversation, to change how the width is shared.
The `width`, `height`, `split`, and `pad` keys in the `hocus` block of `config.kdl` set the starting size (60%, 66%, 50%, and 3 by default).
hocus reports a value it cannot read in the conversation and uses the default instead.

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

The **Swamp** tab lists swamp-club issues as the host's `swamp auth login` sees them.
Opening the tab does not ask for permission. Press `r` to allow it; once allowed, later loads ask by themselves.
`/` searches titles and bodies, `s`, `t`, `f`, and `u` filter by status, type, source, and assignee, `o` changes the sort, and `c` or `Esc` clears the search and every filter.
`Enter` opens an issue and `Esc` returns to the list, with the search and filters still applied.
`a` asks, then sends the selected issue to the hocus conversation to triage.
`Ctrl r`, or `r`, reloads the issues. On every tab, `Ctrl r` refreshes what that tab shows.

The launcher now offers **Reasoning effort** and **Approval mode** alongside **Model**.
The choices come from the existing launch planner and change with the selected agent.
The default choice preserves the agent's own configuration. Selecting an agent does not carry another agent's permission override forward.
Cursor requires an explicit model when an effort is selected. Unsupported combinations produce a launch error.

Duplicate and fork still start fresh conversations in the source pane's location.
They now use the shared launch planner, which supplies Claude's state hooks on local and SSH launches.
Firecracker launches continue to use the hooks installed in the guest.
See [Firecracker VMs](FIRECRACKER.md) for installation, images, launch, sync, and recovery.

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
