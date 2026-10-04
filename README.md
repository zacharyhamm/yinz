yinz is not zellij

[Run shells and coding agents in Firecracker VMs](docs/FIRECRACKER.md)

[Load TypeScript plugins from Git repositories](docs/GIT_PLUGINS.md)

## Install from a visible pane

Run the project-owned Swamp workflow from this repository. Select an existing
local Yinz session and an absolute checkout path:

```sh
# First run: install to ~/.local/bin/yinz.
swamp workflow run @hammz/yinz-install \
  --input session=dev --input checkout=/absolute/path/to/yinz

# Save a different destination for subsequent runs in this Swamp repository.
swamp workflow run @hammz/yinz-install \
  --input session=dev --input checkout=/absolute/path/to/yinz \
  --input 'destination=~/bin/yinz'

# Read the saved destination.
swamp data query 'modelName == "yinz-install" && name == "config"' --select content
```

The optional `binary` input selects the Yinz CLI executable. Its default is
`yinz`. Destinations must be absolute or start with `~/`, which expands to the
executing user's home directory. The workflow creates the destination's parent
directory and remembers the resolved path, including when installation fails.

The stable `yinz-install` model instance serializes runs. Each run opens a pane
with `cargo x install <destination>` in the selected checkout, using the normal
release profile and builtin installation behavior. It waits up to six hours and
succeeds only when the pane reports exit status zero. The pane stays open for
inspection. Swamp saves its ID as `pane` and its completion response as `result`.

A timeout or disconnection fails the run. The workflow does not retry pane
creation. Inspect the selected session before starting another run because the
install command can still be running.
