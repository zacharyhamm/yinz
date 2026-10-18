yinz is not zellij

[Load Scheme plugins from Git repositories](docs/GIT_PLUGINS.md)

## Install

From this checkout, run:

```sh
cargo x install "$HOME/.local/bin/yinz"
```

This builds a release binary and installs builtin Scheme files into the default Yinz data directory, under `plugins/builtins/`.
Choose another binary destination by changing the path. To select a data directory, use:

```sh
cargo x install "$HOME/.local/bin/yinz" --data-dir /path/to/data
yinz --data-dir /path/to/data
```

Reinstalling replaces shipped builtin files, including local edits. It preserves unrelated files and user plugins.
The sources are installed before the binary. Copying only the binary does not produce a complete installation.
There is no embedded fallback or automatic runtime installation.

For development, `cargo x run` installs builtin files into its selected data directory.
Use `cargo x run --quick-run` for `target/dev-data`. `cargo x build` does not install builtin files.

## Tab groups

The upper tab-bar row selects a group. The lower row shows that group's tabs.
Startup tabs belong to **Default**. Press **Ctrl+t**, then **g** to create a group
with a fresh terminal tab. In Tab mode, **Shift+h** and **Shift+l** switch groups;
**1–9** and the usual tab navigation keys select tabs within the current group.
Switching groups restores the last tab you used there.
Press **Ctrl+t**, then **{** or **}** to move the focused pane to the previous or next group.
The pane moves to that group's last-used tab, and focus follows it in Normal mode.
These shortcuts wrap around and skip groups without a loaded tab.

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
