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
| Project memory | Share projects, nested todos, and linked notes across tools and the CLI | `default-plugins/hocus-store/`, `default-plugins/hocus-proto/src/todos.rs`, `zellij-cli/src/notes_todos.rs` |
| Development workflows | Run project-owned validation, review, repair, and integration steps | `zellij-config/assets/plugins/workflow-view.ts`, `workflows/`, `extensions/models/` |

Use the shipped keybindings to open **New agent** with `Ctrl e`, then `n`.
Open the agent panel with `Ctrl e`, then `a`.
Custom configurations can bind the `hocus-new-agent` and `hocus-agents` aliases to other keys.

In the **Agents** list, press `w` to show agents waiting for input. Press `w` again to restore the full list.
The footer shows the waiting count. The filtered list retains project and branch labels and updates when agent state changes.
Press `Enter` to focus the selected live agent. Use `/` to search history and `r` to resume a selected archived run.
The footer changes with the current selection to expose these existing actions.

Waiting means the same thing as the existing `!` marker and the `agent_list` tool's `awaiting_confirmation` field.
An agent hook takes precedence. Without hooks, the existing fallback requires an idle pane and a summary matching its current screen.
An exited agent or a stale summary does not count as waiting. The `◆` marker identifies agents with a hook feed.

In the **Todos** tab, select a task and press `n` to open **New agent**.
The launcher uses that todo's project, even when the panel was opened from another directory.
Its prompt includes the selected task, parent context, and unfinished subtasks.
Review or edit the prompt, choose the agent and worktree, then select **Create agent**.
Launching does not change the todo's completion status. Complete an open task after checking the result.

The launcher now offers **Reasoning effort** and **Approval mode** alongside **Model**.
The choices come from the existing launch planner and change with the selected agent.
The default choice preserves the agent's own configuration. Selecting an agent does not carry another agent's permission override forward.
Cursor requires an explicit model when an effort is selected. Unsupported combinations produce a launch error.

Duplicate and fork still start fresh conversations in the source pane's location.
They now use the shared launch planner, which supplies Claude's state hooks on local and SSH launches.
Firecracker launches continue to use the hooks installed in the guest.

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
  The current request relies on later run updates and can leave `summarising…` visible when no result arrives.

These follow-ups should extend the existing run, store, and workflow ownership mechanisms.
The first iteration adds no task scheduler, new database schema, or separate build runner.
