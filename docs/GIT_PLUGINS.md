# Steel Scheme plugins from Git repositories

Declare repository plugins in the `plugins` block of your configuration:

```kdl
plugins {
    my-panel repo="https://github.com/example/yinz-panel.git" branch="main"
    local-tools repo="~/src/yinz-tools" entrypoint="src/main.scm" {
        greeting "Hello"
    }
}
```

`entrypoint` defaults to `main.scm`. The entrypoint must be a relative `.scm`, `.ss`, or `.sld` file inside the repository.
Use one revision property: `branch`, `tag`, or `commit`. Without a revision property, the plugin follows the remote default branch.
Tags and commits stay pinned. Repository properties are separate from the configuration sent to the plugin.
Use either `repo` or `location` for each alias.

Remote repositories support HTTPS, SSH URLs, SSH paths such as `git@github.com:owner/repo.git`, and the Git protocol.
Yinz uses native Git and your existing authentication. Git runs without interactive prompts or checkout hooks.
Installations and lock waits have a 120-second limit per operation. Ctrl-C cancels CLI installations and updates.

Local paths and `file:` URLs load directly. Relative paths use the active configuration directory, including declarations from `config.d`.
Local repositories remain under your control. Yinz does not fetch, checkout, reset, or write them.
Revision properties are invalid for local repositories.

## Install, update, and reload

```sh
yinz plugins install my-panel
yinz plugins status my-panel
yinz plugins update my-panel
yinz plugins start my-panel
yinz plugins reload my-panel
```

Omit aliases from `install`, `update`, or `status` to select all configured repository plugins.
Remote plugins also install on first use. Installed plugins load without network access.
`status` shows the source, selector, entrypoint, and selected commit without fetching.
`plugins check my-panel` checks the installed plugin with the repository import boundary.

Updates select a detached commit under `<data-dir>/plugin-repositories`. Aliases with the same repository and selector share an installation.
Yinz validates their configured entrypoints before selecting the new commit. A failed installation or update preserves the previous selection.
Previous revisions remain on disk.

Running plugins retain their loaded revision. This includes workers, delayed imports, and instances for newly attached clients.
Reload the plugin explicitly to use the selected update. Configuration reload refreshes alias declarations for future launches.
`plugins reload` resolves configured aliases first, then falls back to files in the authored plugin directory.
Reload requires an existing identity. Use `plugins start` for an initial background launch.
See [plugin lifecycle and permissions](PLUGINS.md) for identity selection, runtime status, and permission modals.

## Imports and identity

Use `require` with explicit relative Scheme imports inside the canonical repository root. A nested entrypoint can import `../shared.scm` within that root.
Imports and entrypoints cannot escape the root through symlinks.
The existing source, module graph, and runtime resource limits apply.
The SDK is available without an import. Network imports and external packages are not available.

The declared source identifies permissions and private data. Selecting another installed commit does not change that identity.
Saved layouts preserve repository declarations with an encoded `git://plugin?repo=…&entrypoint=…` descriptor.

Managed revisions remain immutable. Edit a local repository to develop a plugin, or use the existing commands for authored plugin files.
This version does not run build hooks or read a repository manifest. It has no automatic reload, revision cleanup, or rollback command.
Builtin Scheme sources are embedded in the server.
