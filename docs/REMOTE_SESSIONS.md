# Remote sessions over SSH

Run the terminal client on your machine and keep the session server on an SSH host:

```sh
yinz attach --remote user@host work
yinz attach --remote build-box work --create
```

The session name is required. `--create` attaches to an existing session, restores an exited session, or creates a new session. It refuses to replace an unresponsive server. Restored commands retain the normal suspended state.

## Setup

Install compatible Yinz versions, including the builtin plugins, on both machines. The Yinz version, client/server contract version, and remote bootstrap version must match. The remote installation must be available on the SSH command's `PATH`.

The remote bootstrap version covers the `yinz remote-session` and `yinz remote-sessions` commands that SSH runs, and their JSON replies. It changes independently of the client/server contract, which stays the same across additive IPC changes. A remote that predates bootstrap versioning rejects the `--bootstrap-version` flag before it starts anything. Yinz reports that rejection as a compatibility mismatch rather than a command-line error.

Use OpenSSH on both machines. The SSH server must allow Unix-socket forwarding (`AllowStreamLocalForwarding yes`). macOS and Linux can serve either role. Windows is not supported.

Configure key or agent authentication and verify the host key before attaching:

```sh
ssh build-box yinz --version
```

Yinz uses `BatchMode=yes`, so it cannot ask for passwords or key passphrases. Load encrypted keys into your SSH agent first. Host-key verification follows your SSH configuration.

Use `~/.ssh/config` for host aliases, ports, identity files, and jump hosts:

```sshconfig
Host build-box
    HostName build.example.com
    User developer
    Port 2222
    IdentityFile ~/.ssh/id_ed25519
    ProxyJump gateway.example.com
```

The client owns a separate foreground SSH connection. Connection multiplexing is disabled for that connection. See the [OpenSSH configuration reference](https://man.openbsd.org/ssh_config.5).

## Configuration and session switching

The remote host resolves session configuration, layouts, plugins, working directories, and socket paths. Local configuration controls terminal-side behavior, such as keyboard protocol support. `--config` and `--config-dir` select local terminal configuration. They do not change the remote session.

Remote attach rejects `--index`, `--create-background`, `--force-run-commands`, layout overrides, data-directory overrides, and the `options` subcommand. Configure session behavior on the remote host before attaching.

A named session switch stays on the same SSH host. The client creates or restores that named session if necessary. Unnamed switch/create requests and switch requests with layout or directory overrides fail with an error.

## Local sessions from other builds

Local sessions share one socket directory per client/server contract version (`…/yinz/cv1/`). Builds with the same contract version list, attach to, and warp panes between each other's sessions. A build that only adds messages keeps the contract version, and gates the new messages on a capability each peer advertises.

Switching sessions across machines from the session palette needs a client that advertises session requests. A client from an older build can still open the palette. It lists the current server's running and saved sessions and switches between them in place. Reattach with a current Yinz build to see local and remote destinations.

## Disconnects and errors

Detach normally to leave the remote session running. To reattach, run the same attach command again.

If the connection fails, Yinz restores the terminal and exits with an error. The remote session remains available for manual reattachment. Yinz does not reconnect automatically. SSH keepalives use a 15-second interval and allow three unanswered requests, so a silent network failure can take about 45 seconds to detect.

| Error | Action |
| --- | --- |
| Compatibility mismatch | Install matching Yinz versions on both machines. The error reports this machine's Yinz, contract, and bootstrap versions, and the peer's when it can report them. An older peer without bootstrap versioning is named as such. |
| Authentication or host-key failure | Verify `ssh HOST yinz --version` with the same account and SSH configuration. |
| Remote Yinz missing | Add the remote Yinz installation to the noninteractive SSH command's `PATH`. |
| Session missing | Check the name or add `--create`. |
| Session unresponsive | Inspect the server on the remote host. Yinz will not replace its socket. |
| Forwarding failure | Check `AllowStreamLocalForwarding`, SSH key restrictions, and access to the remote session socket. |
| Invalid endpoint response | Remove stdout output from noninteractive shell startup files. The bootstrap command must return only JSON. |

Each client uses a private forwarding socket outside local session discovery. On detach, startup failure, or client exit, Yinz removes that socket and terminates its owned SSH process.

Remote noninteractive CLI commands and automatic installation are not included. To run a CLI action remotely, invoke Yinz through SSH on the remote host.

## Verification

Run these commands from an isolated worktree:

```sh
cargo x ci check
cargo x remote-test --build
cargo x remote-test --test
```

The SSH suite uses a Linux container and real local clients. Run it on a macOS host to verify a macOS client against Linux. Linux-only results do not establish macOS compatibility.
