# Firecracker panes

Zellij can run agent panes in Firecracker microVMs. The Linux package includes the
`zellij-firecracker` provider. Zellij connects to each VM through the existing remote-pane
transport.

A host worktree identifies a VM. Sessions that use the same worktree share the VM. The provider
stops the VM after the last session releases it.

## One-command development setup

The included provider supports x86_64 Linux. It requires:

- `/dev/kvm` with read and write access for the user
- Firecracker on `PATH`
- `curl`, `git`, `rsync`, `sha256sum`, `ssh`, `ssh-keygen`, `ssh-keyscan`, `unshare`,
  `unsquashfs`, and `mkfs.ext4`
- `ip`, `iptables`, and `systemd` for the one-time host network setup

From a Zellij checkout, build the binaries and run the interactive setup:

```sh
cargo x install /path/to/zellij
```

On x86_64 Linux, `cargo x install` runs the interactive setup automatically when Firecracker is
on `PATH` and `/dev/kvm` is readable and writable. Otherwise, build and run it explicitly with
`cargo x build` and `target/debug/zellij-firecracker setup`.

`setup` uses the current directory as the project, installs the provider at
`/usr/local/bin/zellij-firecracker` when needed, configures the host network, and builds the
project's `.zellij/firecracker-image.sh` as a 64 GiB sparse `zellij-dev` image. It also writes the
provider and a 90-second timeout to Zellij's managed `config.d` fragment, then runs `doctor`.
Every `sudo` operation is announced before it runs.

An existing valid `zellij-dev` image is reused. Rebuild it, including current Codex and Claude Code
releases, with:

```sh
target/debug/zellij-firecracker setup --rebuild
```

The full interface is:

```text
zellij-firecracker setup
  [--project PATH]
  [--name IMAGE]
  [--zellij PATH]
  [--disk-size-gib N]
  [--rebuild]
```

Zellij is resolved from `--zellij`, beside the provider, or from `PATH`, in that order. The image
builder downloads pinned Firecracker CI kernel and Ubuntu 24.04 root file system artifacts and
verifies their SHA-256 checksums.

If `~/.codex/auth.json` exists, setup offers to copy it to `/root/.codex/auth.json` in each VM. It
separately offers the same for `~/.claude/.credentials.json`. Setup records only the selected file
mappings; it never prints credential contents, preserves existing mappings, and does not duplicate
them on later runs.

For Hocus, set the project's `firecracker_image` to `zellij-dev`, or select that image in the New
Agent dialog. A Firecracker image selection requires a Firecracker VM name.

## Advanced and manual setup

`cargo x install` installs `zellij-firecracker` beside `zellij` on Linux. Distribution packages
must also install both binaries. The individual setup commands remain available:

```sh
zellij-firecracker image build --zellij /usr/bin/zellij
sudo zellij-firecracker network setup --user "$USER"
zellij-firecracker doctor
```

The network command creates one bridge and 16 user-owned TAP devices, a delegated cgroup,
restrictive nftables rules, and a systemd unit that restores these resources after boot. Configure
the provider manually when not using `setup`:

```kdl
firecracker_provider "/usr/bin/zellij-firecracker"
firecracker_provider_timeout_secs 90
```

Use `--output` to select a different image directory. Use `--disk-size-gib` to change the sparse
disk size. The builder writes its provider configuration to
`$XDG_CONFIG_HOME/zellij/firecracker.json`.

## Build a project image

Image builds are explicit. Add a script at `.zellij/firecracker-image.sh` in the project. Then run:

```sh
zellij-firecracker image build --name rust-dev --project /absolute/path/to/project
```

The builder runs the script inside the staged guest root file system. It writes the image to
`$XDG_DATA_HOME/zellij/firecracker/images/rust-dev.ext4`.

Use `--customize /absolute/path/to/script` when the script is not in a project. Image names use
the same character rules as VM names. The provider never builds a missing image during agent
startup.

A Hocus project can store `firecracker_image`. `agent_spawn` and the New Agent dialog can override
that value for one agent. An image requires a Firecracker VM name.

## Configure credentials

Add credential files to `firecracker.json` when an agent needs a host credential:

```json
{
  "credential_files": [
    {"host": "~/.codex/auth.json", "guest": "/root/.codex/auth.json"}
  ]
}
```

The provider copies each file after SSH is ready. It sets mode `0600` in the guest. Host paths
must be absolute or start with `~/`. Guest paths must be absolute and cannot contain parent path
components.

The network command accepts `--subnet`, `--taps`, and `--dns`. It uses
`172.31.0.0/24`, 16 TAP devices, and `1.1.1.1` by default. Remove the host network with:

```sh
sudo zellij-firecracker network teardown --user "$USER"
```

This is a single-user developer provider. It is not a multi-user VM service. Each VM runs as the
Zellij user in a user and mount namespace. Firecracker applies its seccomp filter. The provider
also applies CPU, memory, and process limits through cgroup v2.

The network permits DNS through the configured resolver and outbound TCP ports 22, 80, and 443.
It blocks guest access to the host and private networks. The provider does not use the privileged
Firecracker jailer. Use a dedicated host or a privileged VM service when the host user is not a
trusted boundary.

Configure one executable and, optionally, its timeout:

```kdl
firecracker_provider "/absolute/path/to/provider"
firecracker_provider_timeout_secs 90
```

Open a guest login shell or run a command in the guest:

```sh
zellij action new-pane --firecracker review
zellij action new-pane --firecracker review --cwd /workspace/repo -- cargo test
```

Names contain 1–64 ASCII letters, digits, `.`, `_`, or `-`. `--firecracker` cannot be combined
with `--remote-host` or a plugin pane. Multiple panes can use the same VM.

## Provider protocol

Zellij executes the configured path directly. It writes one JSON object to stdin and reads one
JSON object from stdout. It does not invoke a shell. Every message has `version: 2`.

Requests:

```json
{"version":2,"action":"ensure","session":"work","name":"review","image":"rust-dev","worktree":"/home/me/src/project"}
{"version":2,"action":"sync","session":"work","name":"review"}
{"version":2,"action":"stop","session":"work","name":"review"}
{"version":2,"action":"stop_session","session":"work"}
{"version":2,"action":"list","session":null}
```

An `ensure` response supplies an SSH URL. It can also supply the guest working directory, SSH key,
and known-hosts file:

```json
{"version":2,"ssh_host":"ssh://root@172.31.0.2:22","cwd":"/workspace","ssh_identity_file":"/home/me/.local/share/zellij/firecracker/id_ed25519","ssh_known_hosts_file":"/home/me/.local/state/zellij/firecracker/vms/abc/known_hosts"}
```

`sync`, `stop`, and `stop_session` return `{"version":2}`. `list` returns:

```json
{"version":2,"vms":[{"session":"work","name":"review","state":"running"}]}
```

The guest must have a matching Zellij binary on `PATH`. OpenSSH must permit Unix-socket
forwarding. Provider failures, invalid JSON, output over 1 MiB, and timeouts are reported to the
pane or management command.

## Lifecycle and recovery

The provider copies the host worktree to `/workspace` when it starts a worktree VM. It keeps a
standalone Git repository in the guest. It does not copy the host worktree's `.git` file or
directory.

The provider copies guest files back when the last session releases the VM. Before that copy, it
checks every host file and symlink except `.git`. If the host changed, the provider refuses the
copy and retains the guest disk for recovery. A manual sync uses the same check:

```sh
zellij firecracker sync review [--session work]
```

Zellij closes the session's panes before it sends `stop_session` during a normal session exit.
Detaching does not stop VMs. A killed server can leave VMs running. Inspect and stop them with:

```sh
zellij firecracker list [--session work] [--json]
zellij firecracker stop review [--session work]
```

The provider retains disks from crashed VMs, failed provisioning, and failed workspace syncs.
Recovery directories are under `$XDG_STATE_HOME/zellij/firecracker/vms/recovery`. The provider
deletes recovery directories after seven days.

Firecracker panes cannot be warped to another session. A worktree VM can still have leases from
more than one session.

## Agents

The Hocus `agent_spawn` tool and New Agent dialog accept `firecracker` and `firecracker_image`.
They also support project, branch, and worktree setup. Hocus creates the checkout on the host.
The provider then synchronizes that checkout with `/workspace` in the VM.

Claude hook events use a pane-scoped control sequence on the authenticated SSH terminal. Zellij
updates the pane agent record and broadcasts the event to plugins. The control sequence does not
open a host listener. Configure Codex in a custom image to run `zellij --pty-broker report codex`
for equivalent hook reporting.

Zellij passes the remote pane's terminal and locale allowlist. It also passes environment values
that `agent_spawn` explicitly supplies. It does not copy the rest of the host environment. Use
`credential_files` for required credentials.

The provider stores VM state under `$XDG_STATE_HOME/zellij/firecracker`. It stores the base image
under `$XDG_DATA_HOME/zellij/firecracker`. A crashed server can leave a VM running, so use the
management commands above to inspect and stop it.
