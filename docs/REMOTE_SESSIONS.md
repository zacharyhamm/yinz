# Remote sessions over SSH

Run the terminal client on your machine and keep the session server on an SSH host:

```sh
yinz attach --remote user@host
yinz attach --remote user@host work
yinz attach --remote build-box work --create
yinz --remote user@host
yinz --remote user@host --session work
```

Without a session name, Yinz discovers sessions on the specified host. It attaches to the only running session, or opens a picker on a terminal when there are several. The picker only shows sessions from that host. If no sessions are active, Yinz asks on a terminal whether to create a new session, which the remote host names; noninteractive callers get an error instead. Noninteractive callers must supply a name when discovery returns several sessions.

`yinz --remote HOST` always starts a new session on the host and attaches to it. The remote host names the session unless `--session` names it. It refuses a name that is already taken on the host, whether the session is running or can be resurrected. Without a subcommand it rejects the same local overrides as remote attach.

`--create` requires a session name. It attaches to an existing session, restores an exited session, or creates a new session. It refuses to replace an unresponsive server. Restored commands retain the normal suspended state.

## CLI commands

`--remote HOST` works before or after subcommands. Select a running session with
`--session NAME`; otherwise commands use the only running session on that host.
Zero or multiple sessions produce an error. A local shell's session and pane
identity never select a remote target.

```sh
yinz --remote build-box --session work action list-panes --json
yinz --session work events --remote build-box
yinz --remote build-box --session work agent list --json
yinz --remote build-box --session work data catalog
yinz --remote build-box --session work repl --eval '(+ 1 2)'
yinz --remote build-box --session work run --cwd /srv/project -- make test
yinz list-sessions --remote build-box --short
yinz kill-session --remote build-box work
```

Actions, pane run/edit/plugin/pipe commands, agent control and archives, data,
REPL, render subscriptions, event streams, and read-only watchers use the remote
session. Plugin lifecycle commands and logs, pane adoption, session listing,
and session kill/delete commands also support `--remote`. All-session lifecycle
commands apply to the selected SSH host and ask for confirmation on the local
terminal unless `--yes` is given.

Input files (request JSON, agent prompts, config fragments, and layout source)
and stdin are read locally, and a layout name resolves through the local
`layout_dir`. `switch-session --layout` sends the layout's text, without a
sibling swap layout file. Paths used by the session, including working
directories, edited files, and plugin locations, refer to the remote machine.
An omitted working directory uses the remote session's defaults, except that
`agent spawn` requires an absolute `--cwd` or `--project`. Plugin locations inside a layout
file are the exception to remote paths: the layout is parsed locally, so `~` and
relative `file:` locations resolve here. `--remote-host` and `pty-host --host`
still select a pane's machine independently of the machine hosting the session.

Local setup, skills, hook installation, plugin authoring/repository commands,
standalone agent launchers, and supervisor administration without a live-session
target reject `--remote`. Ordinary session commands never create a missing
session or replay a request after a connection failure.

## Setup

Install compatible Yinz versions, including the builtin plugins, on both machines. The Yinz version, client/server contract version, and remote bootstrap version must match. The remote installation must be available on the SSH command's `PATH`.

The remote bootstrap version covers the `yinz remote-session`, `yinz remote-sessions`, and `yinz remote-command` commands that SSH runs, and their JSON replies. It changes independently of the client/server contract, which stays the same across additive IPC changes. A remote that predates bootstrap versioning rejects the `--bootstrap-version` flag before it starts anything. Yinz reports that rejection as a compatibility mismatch rather than a command-line error.

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

After an attachment has rendered, Yinz automatically reconnects through sleep, network changes, and outages. The last screen stays visible with a local status row showing the host/session, attempt number, next retry, and “Ctrl-C to cancel.” The row follows terminal resizes. Keyboard, paste, and mouse input received during recovery is discarded. Ctrl-C cancels locally, restores the terminal, and exits with status 130.

The first retry starts immediately. Further retries wait 1, 2, 4, 8, 16, then 30 seconds, with ±20% jitter and a 30-second maximum delay. Each attempt shares a 30-second deadline across bootstrap, forwarding, probing, and attachment. Backoff resets after 60 seconds of healthy attachment. SSH keepalives still use a 15-second interval and allow three unanswered requests, so a silent network failure can take about 45 seconds to detect before recovery starts.

Recovery attaches to the active host and session, following session renames and picker handoffs. It never creates or resurrects a session, even when the original command used `--create`. A replacement is installed after its first render is ready, then fully repaints and receives the current terminal size and negotiated capabilities. It uses normal attachment focus selection.

Retries continue indefinitely for connection refusals, network failures, timeouts, and unknown SSH failures (whose diagnostics are retained in the client log). Confirmed authentication, host-key, compatibility, installation, forwarding-permission, and missing-session errors stop recovery with a diagnostic. Explicit server exit, detach, and session-switch messages retain their normal behavior. Initial attachment failures still exit normally without automatic retries. Remote panes have a separate reconnect policy.

| Error | Action |
| --- | --- |
| Compatibility mismatch | Install matching Yinz versions on both machines. The error reports this machine's Yinz, contract, and bootstrap versions, and the peer's when it can report them. An older peer without bootstrap versioning is named as such. |
| Authentication or host-key failure | Verify `ssh HOST yinz --version` with the same account and SSH configuration. |
| Remote Yinz missing | Add the remote Yinz installation to the noninteractive SSH command's `PATH`. |
| Session missing | Check the name. To explicitly create or resurrect it, start a new attachment with `--create`; recovery never does this. |
| Session unresponsive | Inspect the server on the remote host. Yinz will not replace its socket. |
| Forwarding failure | Check `AllowStreamLocalForwarding`, SSH key restrictions, and access to the remote session socket. |
| Invalid endpoint response | Remove stdout output from noninteractive shell startup files. The bootstrap command must return only JSON. |

Each client uses a private forwarding socket outside local session discovery. On failed recovery attempts, detach, startup failure, or client exit, Yinz removes that socket and terminates its owned SSH process.

Remote noninteractive CLI commands and automatic installation are not included. To run a CLI action remotely, invoke Yinz through SSH on the remote host.

## Verification

Run these commands from an isolated worktree:

```sh
cargo x test -p yinz-client
cargo x remote-test --build
cargo x remote-test --test -- --exact remote_client_ssh::remote_client_reconnects_after_repeated_network_cuts
```

The SSH suite uses a Linux container and real local clients. Run it on a macOS host to verify a macOS client against Linux. Linux-only results do not establish macOS compatibility.

The Linux sequential update regressions attach one client to a local session and
another client to a separate SSH session with the same name. Both clients share
one disposable executable and builtin directory. Build the pinned newer release
fixture once, then select either order:

```sh
cargo x remote-test --build --update-fixtures
cargo x remote-test --test -- --exact live_update::tests::client_update_real::sequential_ssh::local_then_remote
cargo x remote-test --test -- --exact live_update::tests::client_update_real::sequential_ssh::remote_then_local
```

`--update-fixtures` copies sources into `target/remote-test/update-release` and
compiles a newer Cargo-stamped prerelease with the same wire contracts and its own
real `target/` directory. The Debian builder avoids requiring published release
downloads or host libc compatibility. `--native` is also supported when the host
binary runs in Debian 13. Tests seed pinned release preparation receipts using
the production installation transaction, then request the production coordinator
through each running server. Release discovery and downloading are excluded;
running/installed version decisions, client reexec, SSH reconnection, retained
builtins, readiness, commit, and installation claim cleanup are exercised.

For sleep/wake verification on both Linux and macOS, attach to a session that prints a recognizable marker, sleep the client machine, and wake it after the SSH connection has expired. Verify the local status, a full replacement repaint, and continued input in the same remote process. Also change networks while attached and cancel an outage with Ctrl-C. These manual checks complement the SSH proxy tests.
