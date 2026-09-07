# Arrange panes by project

Run `zellij action arrange-panes-by-project`, or press `Ctrl t`, then `a` with the default keybindings.
The action groups recognized terminal panes into one tab per project across the current session.
It requires the `hocus-agentd` and `hocus-store` builtins.

Running processes, pane IDs, and agent records stay attached to their panes.
Floating terminals keep their coordinates and pinning.
Plugins, overlays, and terminals without a known project stay in their tabs.
Suppressed panes stay in place unless they are hidden stack-list members.
Tabs that contain retained terminals, suppressed panes, or overlays remain open, including tabs with only floating terminals.
Background plugins are suppressed panes, so they also keep a tab open after its last terminal closes.
Use explicit tab close or quit to close these tabs.
Tabs with explicit names keep those names. New tabs use the project name.

Project matching uses the Firecracker host worktree first, then the agent's reported directory, then the terminal directory.
Registered projects take precedence. Other Git repositories match by remote identity, or by host and Git common directory when no remote exists.
Repository names alone do not identify a project.
Tiled panes are ordered by branch, host, and checkout directory.

Each original stack list splits by project, including its hidden terminals.
Two or more members of one project remain a stack. A single member becomes a normal pane.
Unknown members remain in the source tab. Separate original stacks stay separate in the destination tab.
Each resulting stack keeps its original visible member when possible. Otherwise, the first member in arrangement order becomes visible.

The action requests fresh repository metadata from each remote pane, even when its directory has not changed.
It waits up to 15 seconds for discovery. If a remote refresh fails or times out, that pane stays in place.
A notification reports unavailable remote metadata. Cached results cannot complete a new refresh.

## Project layout

Create `.zellij/layout.kdl` in the project's primary local checkout:

```kdl
layout {
    pane split_direction="vertical" {
        pane size="60%"
        pane stacked=true {
            pane
            pane expanded=true
        }
    }
}
```

The action reads this file on each invocation. Matching worktrees, SSH panes, and Firecracker panes share this local configuration.
Projects that exist only on remote hosts use the default arrangement.

The file accepts one `layout` block containing tiled `pane` splits, sizes, and stacks.
Commands, plugins, hooks, directory changes, floating layouts, and multiple tabs are rejected.
Unused slots are ignored. Extra panes use automatic splits when space permits.
Preserved stack lists take priority over layout-file splits. A destination with preserved lists uses automatic splits, with one slot per stack.
Stack headers and final membership must fit before any pane moves.
An invalid layout or insufficient space leaves that project untouched and shows a notification.

There is no automatic rearrangement or remote configuration download.
