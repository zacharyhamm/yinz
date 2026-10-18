yinz is not zellij

[Load Scheme plugins from Git repositories](docs/GIT_PLUGINS.md)

## Install

From this checkout, run:

```sh
cargo x install "$HOME/.local/bin/yinz"
```

This builds a release binary and installs the builtin plugins. Choose another destination by changing the path.

## Tab groups

The upper tab-bar row selects a group. The lower row shows that group's tabs.
Startup tabs belong to **Default**. Press **Ctrl+t**, then **g** to create a group
with a fresh terminal tab. In Tab mode, **Shift+h** and **Shift+l** switch groups;
**1–9** and the usual tab navigation keys select tabs within the current group.
Switching groups restores the last tab you used there.

Use the command palette to create, switch, rename, or close a tab group. Closing a
group asks for confirmation and closes its tabs and panes. A group also disappears
when its last tab closes. Names and membership survive session resurrection.

Custom layouts need `pane size=2` for the `tab-bar` plugin to show both rows.
A one-row tab bar still shows the current group's tabs. Custom keymaps can bind
`NewTabGroup`, `GoToPreviousTabGroup`, `GoToNextTabGroup`, and `GoToTabInGroup 1`.
Explicit tab IDs, names, and existing `GoToTab` positions remain session-wide.

## Agent exit

The default agent-exit hook keeps the pane available. Press Enter to restart the agent or Esc to close the pane.
For a local or sandboxed secondary worktree, Delete opens a cleanup confirmation.
Cleanup requires a clean worktree and commits merged into the local base branch or its freshly fetched origin branch.
The hook checks the checkout identity again before removal and guards branch deletion with its recorded revision.

Set the base branch in the primary checkout's `.yinz/agent-exit.kdl`:

```kdl
agent_exit {
    default_branch "main"
}
```

The default is `main`. This file supports only `default_branch`.
Managed Swamp commands, plugins, and bundled assets have been removed without compatibility aliases or migration tooling.
Existing external runtime data and running Swamp processes are not deleted or stopped automatically.
