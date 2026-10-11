# Project Bubblewrap environments

On Linux, `yinz agent spawn --bubblewrap` runs an agent in a prepared project environment.
Install Bubblewrap 0.10.0 or newer (`bwrap`) on the host first. The host must permit user namespaces.
Yinz does not install operating system packages. Version validation happens before provisioning or
checkout changes, including for declarations without optional mounts. Descriptor-based mounts require
[Bubblewrap 0.10.0](https://github.com/containers/bubblewrap/releases/tag/v0.10.0).

Each primary Git checkout has one persistent private home. Its contained worktrees share that home.
Build output stays in each worktree. Yinz never sets `CARGO_TARGET_DIR`.

The sandbox has network access and full Yinz control through the shared control directory.
**Full Yinz control permits host actions outside Bubblewrap.** This feature restricts direct filesystem
access. It does not restrict all agent authority. An explicitly declared read-only mount exposes its
contents to the agent. Only accept declarations from projects you trust.

## Declare the environment

Create `.yinz/bubblewrap.kdl` in the primary checkout. Yinz resolves that checkout through Git's
worktree metadata, even when preparation starts in a linked worktree.

Alternatively, create `bubblewrap.kdl` in the active Yinz configuration directory:

```kdl
project "/absolute/path/to/repository" {
    path "${SANDBOX_HOME}/.local/bin"
    check "git" "--version"
}
```

Use `yinz --config-dir /path/to/config bubblewrap prepare` to select another configuration directory.
A matching global block replaces the complete project declaration. Paths are canonicalized.
Duplicate global project paths are errors, including aliases through symlinks.

Both locations accept the same body:

| Declaration | Effect |
| --- | --- |
| `ro-bind "source" "destination"` | Mount a host file or directory read-only. |
| `path "directory"` | Append one absolute directory to the sandbox executable search path. |
| `env "NAME" "value"` | Set an environment variable. |
| `inherit-env "NAME"` | Require and copy a variable from the process that prepares or launches the environment. |
| `seed "source" "destination"` | Copy a file into the private home during preparation, with mode `0600`. |
| `setup "program" "argument" ...` | Run a provisioning command during preparation. |
| `check "program" "argument" ...` | Run a requirement check after setup and before launch. |

Host source paths can be relative to the primary checkout. Destinations must be absolute.
Seed destinations must be inside `${SANDBOX_HOME}`. These explicit substitutions are available in arguments:

- `${HOST_HOME}`: the host user's home at preparation time.
- `${REPOSITORY}`: the canonical primary checkout path.
- `${SANDBOX_HOME}`: `/home/yinz` inside the sandbox.

Yinz does not expand `~`, shell variables, or command substitutions. To use shell syntax, explicitly
run `sh -c`. Setup commands run in order and must tolerate existing installations.

Unknown declarations, named arguments, missing sources, and overlapping mount destinations are errors.
Declarations cannot replace repository, system, runtime, or private-home mounts, or `/etc/passwd` and `/etc/group`.
A `ro-bind` source needs no read permission: sockets, FIFOs, execute-only files, and search-only directories mount.
`HOME`, `PATH`, `TMPDIR`, the XDG directory variables, `YINZ_*`, and `CARGO_TARGET_DIR` are reserved.
The spawn API also rejects overrides of sandbox boundary variables.

## Prepare and launch

1. Inspect the declaration and its provisioning commands.
2. Run `yinz bubblewrap prepare --project /path/to/repository`.
3. Launch an agent with `yinz agent spawn --kind codex --branch my-change --bubblewrap --allow-yinz`.

Codex's own sandbox settings still apply inside Bubblewrap. `--allow-yinz` enables network access
in Codex's workspace-write mode so its tools can connect to the Yinz socket. API callers can pass
`args: ["-c", "sandbox_workspace_write.network_access=true"]` for the same setting.

Without `--project`, preparation uses the current directory.
Preparation accepts the declaration, copies seed files, runs setup, and runs checks.
It marks the environment ready only when all commands succeed.

Launch uses the saved declaration. Editing either KDL file has no effect until preparation succeeds again.
Launch checks run once in the server, before the pane's process starts. The pane shows their progress.
After the declared checks, Yinz confirms that the agent's program resolves on the sandbox `PATH`.
A failed check, a missing program, or an unprepared environment fails the spawn:
`yinz agent spawn` and plugin callers receive the error instead of a pane holding one.
Each check suite has a 10-second total deadline, including the mandatory namespace and mount probe.
Preparation check output stays in `prepare.log`; launch check stdout and stderr append to private
`launch.log` in the environment metadata directory. Each log is emptied once it exceeds 1 MiB.
Launch never provisions tools. Repeated preparation reruns setup and seed operations.
Failed preparation preserves the private home and logs but makes the environment unavailable.

Before sending a Bubblewrap spawn, the CLI requires the server to confirm `agent-bubblewrap-v1` support.
This check also applies to JSON input with `bubblewrap: true`.
Explicit tab placement requires a separate confirmation. Both checks must succeed before the CLI sends the spawn.
Each check has a five-second deadline. Failed negotiation sends no spawn and asks you to restart the session with this Yinz version.

Accepted declarations and preparation logs are under the user's Yinz data directory:
`bubblewrap/environments/<repository-hash>/`. Private homes are stored separately under
`bubblewrap/homes/<repository-hash>/`. The hash uses the canonical repository path.
These management files are not mounted into the sandbox.

Preparation takes an exclusive environment lock. Concurrent agents hold shared locks for their process lifetimes.
Stop the agents before preparing their environment again.

The primary checkout is writable at its original absolute path. This includes nested repositories,
`.git`, and contained worktrees. External worktrees and required Git metadata outside the primary
checkout are rejected. Symlinks do not expose host files that have no sandbox mount.

The sandbox contains read-only system tools, libraries, DNS settings, and certificate files.
`/etc/alternatives` is included, so Debian-family links such as `/usr/bin/cc` resolve.
`/etc/passwd` and `/etc/group` are generated for the launch: they name only the invoking user, with
`/home/yinz` as its home, and `nobody`. Host account files are not mounted.
Each launch has its own `/tmp`, runtime directories, devices, and process namespace.
Yinz exposes its executable, reporting assets, and shared control directory.
The host's `YINZ_SOCK_DIR` is mounted read-only at `/run/yinz-control/<contract-directory>`.
The sandbox uses `YINZ_SOCKET_DIR=/run/yinz-control`.
This mount exposes other local Yinz control sockets and supervisor metadata, including entries created after launch.
It permits socket connections but prevents file creation and deletion through the mount.
Read-only mounting does not restrict actions available through those sockets.

Commands and reports follow the pane's current supervisor owner after a transfer, including sessions created after sandbox launch.
They can reconnect when a server replaces its session socket.
The shared control directory must remain in place. If that directory itself is replaced, relaunch the sandbox.

Managed Codex uses a dedicated app-server on a private Unix socket. It cannot use the host's shared daemon.
The private backend ends with the pane.

## Rust and Codex example

This example uses host-provided `curl`, Git, a C compiler, and libraries under `/usr`.
Install any required operating system development packages on the host first.
It installs user-space tools in the private home.

```kdl
path "${SANDBOX_HOME}/.cargo/bin"
path "${SANDBOX_HOME}/.local/bin"
env "CARGO_HOME" "${SANDBOX_HOME}/.cargo"
env "RUSTUP_HOME" "${SANDBOX_HOME}/.rustup"

// Expose a host-installed Codex executable. Adjust the source to your installation.
ro-bind "${HOST_HOME}/.local/bin/codex" "/opt/codex"
// Use a private launcher so the recognized executable name remains codex.
setup "sh" "-c" "mkdir -p $HOME/.local/bin; ln -sf /opt/codex $HOME/.local/bin/codex"

setup "sh" "-c" "if [ ! -x $HOME/.cargo/bin/rustup ]; then curl --fail --show-error --location https://sh.rustup.rs -o /tmp/rustup-init.sh && sh /tmp/rustup-init.sh -y --no-modify-path; fi"
setup "rustup" "toolchain" "install" "stable"
setup "yinz" "hooks" "install" "--agent" "codex"
check "cargo" "--version"
check "codex" "--version"

// Optional: seed an existing login. This copies credentials into the private home.
seed "${HOST_HOME}/.codex/auth.json" "${SANDBOX_HOME}/.codex/auth.json"
```

A wrapper executable can require additional host files. Add explicit read-only mounts for those files,
or install the agent entirely in the private home during setup.
For other authentication methods, prefer `inherit-env` for credentials. Do not commit credentials in project KDL.
Literal `env` values remain stored in the KDL file and the accepted declaration.

Yinz passes environment values through a private, unnamed temporary file using Bubblewrap's
[`--args FD`](https://github.com/containers/bubblewrap/blob/main/bwrap.xml) option.
The file contains NUL-separated arguments. Values do not appear in Bubblewrap's command line, and stdin remains available to the agent.
This transport does not hide credentials from the agent, the owning user, or root.
Secrets explicitly included in command arguments remain visible in those arguments.
Preparation repeats seed copies, so it can replace credentials changed inside the private home.

The shared API accepts `bubblewrap: true` in `AgentSpawnSpec`, including CLI JSON files and
`yinz.agent.spawn({...})`. Omission keeps the existing unsandboxed launch behavior.
The interactive chooser is unchanged. SSH cannot be combined with Bubblewrap.
A failed check, unavailable Bubblewrap, or sandbox startup failure never falls back to an unsandboxed launch.

## Relaunching a sandboxed agent

The sandbox is part of where a pane runs, like an SSH host. The pane's command
stays the agent's own, and plugins see the target as `{Bubblewrap: {root}}`. Everything that starts
a command from an existing pane keeps it:

- Re-running or restarting the pane runs the launch checks again and re-enters the environment.
- Fork and Duplicate in the command palette start the new agent in the same environment.
- A run records its environment. Resuming it from history returns to the private home that holds its conversation.
- A saved or resurrected layout writes `bubblewrap="/path/to/repository"` on the pane.
  The same property works in a hand-written layout on a pane with a `command`.

Missing global project paths are skipped with a diagnostic. Invalid project blocks and
other path resolution errors still stop preparation. Run preparation again to recover
from a truncated or incompatible acceptance file. Each attempt clears readiness before
provisioning; only successful preparation makes the environment available again.
