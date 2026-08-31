# Firecracker panes

Zellij can open panes in named, session-scoped Firecracker VMs through an external provider.
The provider owns VM images, jailer setup, networking, repository setup, credentials, and SSH
readiness. Zellij only asks it for an SSH endpoint and then uses the existing remote-pane
transport.

Configure one executable and, optionally, its timeout:

```kdl
firecracker_provider "/absolute/path/to/provider"
firecracker_provider_timeout_secs 60
```

Open a guest login shell or run a command in the guest:

```sh
zellij action new-pane --firecracker review
zellij action new-pane --firecracker review --cwd /workspace/repo -- cargo test
```

Names contain 1–64 ASCII letters, digits, `.`, `_`, or `-`. `--firecracker` cannot be combined
with `--remote-host` or a plugin pane. Multiple panes may use the same VM name; the provider's
`ensure` operation must return the same VM for the same session and name.

## Provider protocol

Zellij executes the configured path directly, writes one JSON object to stdin, and reads one
JSON object from stdout. It does not invoke a shell. Every message has `version: 1`.

Requests:

```json
{"version":1,"action":"ensure","session":"work","name":"review"}
{"version":1,"action":"stop","session":"work","name":"review"}
{"version":1,"action":"stop_session","session":"work"}
{"version":1,"action":"list","session":null}
```

An `ensure` response supplies an SSH URL and may supply the guest working directory used when
the pane request omitted `--cwd`:

```json
{"version":1,"ssh_host":"ssh://zellij@127.0.0.1:2201","cwd":"/workspace"}
```

`stop` and `stop_session` return `{"version":1}`. `list` returns:

```json
{"version":1,"vms":[{"session":"work","name":"review","state":"running"}]}
```

The guest must have a matching Zellij binary on `PATH`, and OpenSSH must permit Unix-socket
forwarding. Provider failures, invalid JSON, output over 1 MiB, and timeouts are reported to the
pane or management command.

## Lifecycle and recovery

Zellij closes the session's panes before sending `stop_session` during a normal session exit.
Detaching does not stop VMs. A killed or crashed server may leave VMs running; inspect and stop
them through the provider with:

```sh
zellij firecracker list [--session work] [--json]
zellij firecracker stop review [--session work]
```

Firecracker panes cannot be warped to another session because their VM remains owned by the
originating session.

## Agents

The hocus `agent_spawn` tool and New Agent dialog accept a `firecracker` VM name. `cwd` is a
guest path. VM agents cannot request host project, branch, or worktree setup. Agent-report hooks
are disabled by default and `hooks: true` is rejected because the guest has no authenticated
reporting bridge. Zellij passes only the remote pane's existing terminal/locale allowlist and
environment values explicitly supplied to `agent_spawn`; it does not copy host credentials or
the rest of the host environment.

This repository does not include a production Firecracker provider or VM image.
