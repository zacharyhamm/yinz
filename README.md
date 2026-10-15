yinz is not zellij

[Load Scheme plugins from Git repositories](docs/GIT_PLUGINS.md)

## Install

From this checkout, run:

```sh
cargo x install "$HOME/.local/bin/yinz"
```

This builds a release binary and installs the builtin plugins. Choose another destination by changing the path.

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
