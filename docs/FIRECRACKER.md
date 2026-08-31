# Firecracker panes

Zellij can open panes in named, session-scoped Firecracker VMs. The Linux package includes the
`zellij-firecracker` provider. Zellij asks the provider for an SSH endpoint and then uses the
existing remote-pane transport.

## Set up the included provider

The included provider supports x86_64 Linux. It requires:

- `/dev/kvm` with read and write access for the user
- Firecracker on `PATH`
- `curl`, `sha256sum`, `ssh-keygen`, `ssh-keyscan`, `unshare`, `unsquashfs`, and `mkfs.ext4`
- `ip`, `nft`, and `systemd` for the one-time host network setup

`cargo x install` installs `zellij-firecracker` beside `zellij` on Linux. Distribution packages
must also install both binaries.

Complete these steps once:

1. Build the guest image as your normal user:

   ```sh
   zellij-firecracker image build --zellij /usr/bin/zellij
   ```

   The builder downloads pinned Firecracker CI kernel and Ubuntu 24.04 root file system artifacts.
   It verifies both SHA-256 checksums. It then creates an 8 GiB sparse ext4 image and an SSH key.

2. Install the host network as root for the Zellij user:

   ```sh
   sudo zellij-firecracker network setup --user "$USER"
   ```

   This command creates one bridge, 16 user-owned TAP devices, and nftables NAT rules. It also
   installs a systemd unit that restores the network after boot.

3. Check the setup:

   ```sh
   zellij-firecracker doctor
   ```

4. Add the provider to the Zellij configuration:

   ```kdl
   firecracker_provider "/usr/bin/zellij-firecracker"
   firecracker_provider_timeout_secs 90
   ```

Use `--output` to select a different image directory. Use `--disk-size-gib` to change the sparse
disk size. The builder writes its provider configuration to
`$XDG_CONFIG_HOME/zellij/firecracker.json`.

The network command accepts `--subnet`, `--taps`, and `--dns`. It uses
`172.31.0.0/24`, 16 TAP devices, and `1.1.1.1` by default. Remove the host network with:

```sh
sudo zellij-firecracker network teardown --user "$USER"
```

This is a single-user developer provider. It does not use the Firecracker jailer and it is not a
multi-user VM service. Each VM runs as the Zellij user. The provider deletes each writable guest
disk when the VM stops.

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

An `ensure` response supplies an SSH URL. It can also supply the guest working directory, SSH key,
and known-hosts file:

```json
{"version":1,"ssh_host":"ssh://root@172.31.0.2:22","cwd":"/workspace","ssh_identity_file":"/home/me/.local/share/zellij/firecracker/id_ed25519","ssh_known_hosts_file":"/home/me/.local/state/zellij/firecracker/vms/abc/known_hosts"}
```

`stop` and `stop_session` return `{"version":1}`. `list` returns:

```json
{"version":1,"vms":[{"session":"work","name":"review","state":"running"}]}
```

The guest must have a matching Zellij binary on `PATH`. OpenSSH must permit Unix-socket
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

The provider stores VM state under `$XDG_STATE_HOME/zellij/firecracker`. It stores the base image
under `$XDG_DATA_HOME/zellij/firecracker`. A crashed server can leave a VM running, so use the
management commands above to inspect and stop it.
