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
- `chroot`, `curl`, `git`, `rsync`, `sha256sum`, `ssh`, `ssh-keygen`, `ssh-keyscan`, `unshare`,
  `unsquashfs`, and `mkfs.ext4`
- `ip`, `iptables`, and `systemd` for the one-time host network setup
- optionally, a cgroup v2 subtree delegated to the user, which in practice means a systemd
  user session - see "Resource limits" below

From a Zellij checkout, install the binaries, then run the interactive setup explicitly:

```sh
cargo x install /path/to/zellij
/path/to/zellij-firecracker setup --zellij /path/to/zellij
```

For a development build instead, run `cargo x build` and
`target/debug/zellij-firecracker setup`.

`setup` uses the current directory as the project, installs the provider at
`/usr/local/bin/zellij-firecracker` when needed, configures the host network, and builds the
project's `.zellij/firecracker-image.sh` as a 64 GiB sparse `zellij-dev` image. It also writes the
provider and a 90-second inactivity timeout to Zellij's managed `config.d` fragment, then runs
`doctor`. Every `sudo` operation is announced before it runs.

A VM that names no image boots the machine's default one, `rootfs` in
`$XDG_CONFIG_HOME/zellij/firecracker.json`. `setup` points that at the image it built or reused,
so `setup --name <image>` is how a machine chooses its default, and it says so when the default
moves. `doctor` refuses a default that does not exist.

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

The network command creates one bridge and 16 user-owned TAP devices, restrictive iptables
rules, and a systemd unit that restores these resources after boot. Its
default subnet is `10.<uid-high>.<uid-low>.0/24`; UIDs above 65535 must pass `--subnet` to avoid
collisions. An explicit `--subnet` always takes precedence. Configure the provider manually when
not using `setup`:

```kdl
firecracker_provider "/usr/bin/zellij-firecracker"
firecracker_provider_timeout_secs 90
```

The timeout is how long the provider may go without reporting progress, not a limit on the whole
request: booting a guest and copying a large worktree into it take as long as they take.

Use `--output` to select a different image directory. Use `--disk-size-gib` to change the sparse
disk size. The builder writes its provider configuration to
`$XDG_CONFIG_HOME/zellij/firecracker.json`.

## Build a project image

Image builds are explicit. Add a script at `.zellij/firecracker-image.sh` in the project. Then run:

```sh
zellij-firecracker image build --name rust-dev --project /absolute/path/to/project
```

The builder runs the script inside the staged guest root file system. It writes the image to
`$XDG_DATA_HOME/zellij/firecracker/images/rust-dev.ext4`. Unlike `setup`, `image build` leaves a
working default image alone; it only adopts what it built as the default when there is none.

The build is unprivileged: it re-execs itself under `unshare --user`, and the chroot itself runs in
a further mount and PID namespace so the `/proc` and `/dev` mounts the script needs are gone before
the image is written. Three consequences are worth knowing:

- The builder maps your subordinate id ranges into the namespace (`unshare --map-auto`) when
  `/etc/subuid` has an entry for you and `newuidmap` is installed, which is what lets packages
  `chown` to system users. Without them only uid 0 is mapped and any such package fails; the build
  says so when it falls back. Either way apt gets a temporary drop-in setting
  `APT::Sandbox::User "root"` and pinning conffiles to the installed version, removed again before
  the image is written.
- The script runs with a fixed environment — `HOME=/root`, `TMPDIR=/tmp`,
  `DEBIAN_FRONTEND=noninteractive`, and a `PATH` starting `/root/.cargo/bin` — not the host's. Only
  `http_proxy`, `https_proxy` and `no_proxy` are carried over.
- Host keys under `/etc/ssh` are deleted after the script runs, so installing `openssh-server`
  cannot bake shared keys into the image; each VM generates its own on first boot.

If `unshare --user` fails with `EPERM`, check
`sysctl kernel.apparmor_restrict_unprivileged_userns` — Ubuntu 24.04 sets it to `1`.

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

The network command accepts `--subnet`, `--taps`, and `--dns`. By default it uses the
uid-derived `10.<uid-high>.<uid-low>.0/24` subnet described above, 16 TAP devices, and `1.1.1.1`. Remove the host network with:

```sh
sudo zellij-firecracker network teardown --user "$USER"
```

`network setup` and `network apply` converge on the configured state from whatever state the host
is in, so both are safe to re-run; the boot unit relies on it. Every rule the provider puts in
`INPUT`, `FORWARD`, or nat `POSTROUTING` carries an `-m comment --comment
"zellij-firecracker:<uid>"` tag, and cleanup finds its rules in the live table by that tag, by the
bridge name, or by the jump to its own chain — never by reconstructing the spec from the current
configuration, which is what used to break when the subnet changed.

A failed run undoes only what that run created. A bridge or TAP that was already up is left alone,
because it may be carrying another session's VMs. Anything that could not be undone is printed
with the command that removes it.

`network teardown` works without `/etc/zellij-firecracker/<uid>.json`: everything but the subnet
follows from the UID. Use it to clean up after a setup that failed before it recorded its
configuration.

### Troubleshooting

**`iptables: Chain already exists.`** — a host configured by a provider older than this change.
Its rules were written for a different subnet, so the old cleanup could not match them, the
leftover jump kept the chain referenced, and re-creating the chain failed on every run. Current
versions recover on their own; nothing needs to be done by hand.

**`cgroup ... is not delegated to this user`** - only appears when `require_resource_limits`
is on. The provider has nowhere to put a VM's resource limits and was told not to start one
without them. Either run Zellij under a delegated subtree or drop the key; both are covered
under "Resource limits" below.

**A warning about a NAT rule "that cannot be proven to be ours"** — a `POSTROUTING -s <subnet> -j
MASQUERADE` for a subnet this host no longer uses. A NAT rule names no interface and, if it
predates the tag, carries no identifying comment, so the provider refuses to guess and reports it
instead. It blocks nothing. Remove it with the `iptables -t nat -D ...` command the warning prints,
once you have confirmed it is not another tool's.

This is a single-user developer provider. It is not a multi-user VM service. Each VM runs as the
Zellij user in a user and mount namespace. Firecracker applies its seccomp filter.

### Resource limits

The provider also applies CPU, memory, and process limits through cgroup v2: each VM gets a
child of the cgroup subtree systemd delegates to the user (`user@<uid>.service`), found from
the provider's own cgroup. These bound the *host-side* Firecracker process - its page cache,
threads, and file descriptors. The guest's own RAM and vCPUs are capped by Firecracker itself
from `memory_mib` and `vcpu_count` either way.

A delegated subtree can only be granted by root, and plenty of hosts have none - a plain ssh
login, or a logind session scope, which systemd creates undelegated. So the limits are best
effort by default: a provider that finds no subtree warns on stderr, which Zellij logs, and
starts the VM anyway. `doctor` reports the subtree it would use, or says it will run without
one, and succeeds either way.

Set `require_resource_limits` in `firecracker.json` to make the missing subtree fatal instead,
on a host where an unbounded VMM is not acceptable:

```json
{"require_resource_limits": true}
```

To satisfy it, run Zellij inside the user manager's tree, which needs no privileges:

```sh
systemd-run --user --scope -- zellij
```

or have root delegate a subtree once per boot (`/sys/fs/cgroup` is a fresh tmpfs at every
boot). The leaf below is not optional: cgroup v2 forbids a cgroup from holding both processes
and enabled subtree controllers.

```sh
sudo mkdir -p /sys/fs/cgroup/zellij/main
sudo sh -c 'echo "+cpu +memory +pids" > /sys/fs/cgroup/zellij/cgroup.subtree_control'
sudo chown -R "$USER" /sys/fs/cgroup/zellij
sudo sh -c "echo $$ > /sys/fs/cgroup/zellij/main/cgroup.procs"
```

Without the limits, nothing bounds a Firecracker process that misbehaves. That is the default
because the Zellij user is already a trusted boundary on the kind of host this provider is
meant for - see the paragraph below - not because the limits do not matter.

The network permits DNS through the configured resolver and outbound TCP ports 22, 80, and 443.
It blocks guest access to the host and private networks. The provider does not use the privileged
Firecracker jailer. Use a dedicated host or a privileged VM service when the host user is not a
trusted boundary.

Configure one executable and, optionally, its inactivity timeout:

```kdl
firecracker_provider "/absolute/path/to/provider"
firecracker_provider_timeout_secs 90
```

Zellij resets the timeout whenever the provider writes anything. A provider reports a long phase
with `progress: <what it is doing>` lines on stderr; Zellij keeps those out of error messages and
names the last one when it reports a provider that went silent. A provider that says nothing for
the whole timeout is killed together with every process it started.

Open a guest login shell or run a command in the guest:

```sh
zellij action new-pane --firecracker review
zellij action new-pane --firecracker review --cwd /workspace/repo -- cargo test
zellij action new-pane --firecracker review --firecracker-image rust-dev \
    --firecracker-worktree /home/me/src/project
```

Names contain 1–64 ASCII letters, digits, `.`, `_`, or `-`. `--firecracker` cannot be combined
with `--remote-host` or a plugin pane. Multiple panes can use the same VM.

`--firecracker-image` boots the VM from a named image instead of the machine's default, and
`--firecracker-worktree` names an absolute host path to mirror into it. Both need
`--firecracker`. A pane joining a VM that is already running has to name the same image it was
started with; naming a different one is refused rather than silently ignored.

A layout names the VM with `firecracker_vm`, and optionally its image and the host worktree to
mirror into it. A pane with `firecracker_vm` and no `command` opens the guest's login shell. A
`cwd` on such a pane is a guest path:

```kdl
pane command="cargo" firecracker_vm="review" firecracker_image="rust-dev" firecracker_worktree="/home/me/src/project" {
    args "test"
}
pane firecracker_vm="review" cwd="/workspace"
```

`zellij action dump-layout` and session resurrection write these back, so a Firecracker pane
comes back in its VM rather than as a local command. The same three names are children of a
`Run` keybinding action.

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
{"version":2,"action":"rename_session","from":"work","to":"review"}
```

An `ensure` response supplies an SSH URL. It can also supply the guest working directory, SSH key,
and known-hosts file:

```json
{"version":2,"ssh_host":"ssh://root@172.31.0.2:22","cwd":"/workspace","ssh_identity_file":"/home/me/.local/share/zellij/firecracker/id_ed25519","ssh_known_hosts_file":"/home/me/.local/state/zellij/firecracker/vms/abc/known_hosts"}
```

`rename_session` moves every VM `from` owns or leases to `to` and returns `{"version":2}`.
Zellij sends it when the session is renamed, because `zellij firecracker stop` and `sync`
resolve the session by its current name. It is additive to version 2: a provider that rejects
it keeps the VMs under the old name, Zellij keeps using that name for them, and the server log
says so.

`sync`, `stop`, and `stop_session` return `{"version":2}` plus, when the worktree merge kept
the host copy of a file, one `warnings` line per such file:

```json
{"version":2,"warnings":["/repo/src/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/src/a.rs.guest"]}
```

`list` returns the running VMs and the retained disks:

```json
{"version":2,"vms":[{"session":"work","name":"review","state":"running"}],
 "recoveries":[{"path":"/state/recovery/<id>-<time>-sync-failed","reason":"sync-failed","retained_at":1756700000,"session":"work","name":"review","worktree":"/repo"}]}
```

Both `warnings` and `recoveries` are optional in the response; a provider that omits them is
read as having none.

The guest must have a matching Zellij binary on `PATH`. OpenSSH must permit Unix-socket
forwarding. Provider failures, invalid JSON, output over 1 MiB, and timeouts are reported to the
pane or management command. A timeout kills the provider and its helpers, not the VM: the next
request for the same VM resumes its provisioning.

## Lifecycle and recovery

The provider copies the host worktree to the configured `guest_cwd` (`/workspace` by default)
when it starts a worktree VM, creating that directory if the image does not have it. It keeps a
standalone Git repository in the guest. It does not copy the host worktree's `.git` file or
directory. Alongside the guest disk it records a baseline: one content hash per file, symlink,
and directory of the worktree as it was copied, with ignored outputs and `.git` left out.
Timestamps are not part of it, so a save that changes nothing is not a change.

The provider reports each phase - hashing and staging the worktree, cloning the image, waiting
for the guest's ssh, copying into the guest - so the request is never cut short while work is
going on. `boot_timeout_secs` in `firecracker.json` (60 by default) bounds only the wait for the
guest's ssh; a copy that stalls fails through rsync's own 60-second I/O timeout. If the provider
is killed part-way through provisioning, the VM stays up and is listed as `provisioning`; the
next request for the same VM, image, and worktree resumes the copy, which is incremental, rather
than starting a new disk. Releasing a VM that never finished provisioning retains its disk
without merging anything back: a partial copy would read as deletions.

When the last session releases the VM, the provider pulls the guest's copy and merges it into
the host worktree file by file against that baseline:

- a file changed only in the guest, or deleted only there, is applied to the host;
- a file changed only on the host is kept;
- a file changed on both sides keeps the host copy, and the guest copy is written beside it as
  `<file>.guest`. A file the host changed and the guest deleted keeps the host copy. Each such
  file is reported as a warning, which `zellij firecracker stop` prints and a normal session
  exit writes to the server log.

Nothing the guest did is lost in a merge, so a merge conflict does not retain the disk. A merge
that cannot be attempted at all — the guest unreachable, rsync failing — does.

A merge is applied to the host worktree whole or not at all: every step is recorded before it
is taken, and a step that fails - a directory the host has made read-only, say - rolls the
earlier ones back. A guest that turned a directory into a file while the host still keeps
ignored outputs in that directory is refused before anything is touched.

A manual sync merges the same way while the VM keeps running, then tells the guest exactly
what the merge decided - the host's own changes, the host copy of each conflict with the
`.guest` copy beside it, and the host's deletions - and records the merged tree as the new
baseline. Nothing else in the guest is touched, so a file an agent writes while the sync runs
survives it and is merged by the next one:

```sh
zellij firecracker sync review [--session work]
```

Zellij sends `stop_session` whenever the server exits on its own — the last client or pane
closing, a kill-session, or a fatal server error — after it has closed the session's panes.
Detaching does not stop VMs. Only a server that never gets to exit on its own, killed with
`SIGKILL` or crashed, leaves VMs running. Inspect and stop them with:

```sh
zellij firecracker list [--session work] [--json]
zellij firecracker stop review [--session work]
```

`list` prints one tab-separated `session`, `name`, `state` line per VM, or
`No Firecracker VMs found.`, then any retained disks under a `Retained disks:` header as
tab-separated `session`, `name`, `reason`, `path` lines, with `-` for a session or name the
provider does not know. With `--json` it prints the provider's listing as one object:

```json
{"vms":[{"session":"work","name":"review","state":"running"}],
 "recoveries":[{"path":"/state/recovery/<id>-<time>-sync-failed","reason":"sync-failed","retained_at":1756700000,"session":"work","name":"review","worktree":"/repo"}]}
```

Both arrays are always present; `session`, `name`, and `worktree` in a recovery entry are
`null` when unknown.

The provider retains disks from crashed VMs, failed provisioning, and workspace syncs that could
not run. Recovery directories are under `$XDG_STATE_HOME/zellij/firecracker/vms/recovery`, each
with the VM's `state.json`, its worktree baseline, and a `recovery.json` naming the reason.
`zellij firecracker list` shows them. The provider deletes recovery directories after seven days.

Firecracker panes cannot be warped to another session. A worktree VM can still have leases from
more than one session.

## Agents

The Hocus `agent_spawn` tool and New Agent dialog accept `firecracker` and `firecracker_image`.
They also support project, branch, and worktree setup. Hocus creates the checkout on the host.
The provider then synchronizes that checkout with the configured `guest_cwd` in the VM.

Claude hook events use a pane-scoped control sequence on the authenticated SSH terminal. Zellij
updates the pane agent record and broadcasts the event to plugins. The control sequence does not
open a host listener. Zellij honours it only on Firecracker panes: a local pane reports through
`zellij action agent-report`, and an ordinary ssh pane is not trusted to write agent records,
since any program in it could print the sequence. Configure Codex in a custom image to run
`zellij --pty-broker report codex` for equivalent hook reporting.

Zellij passes the remote pane's terminal and locale allowlist. It also passes environment values
that `agent_spawn` explicitly supplies. It does not copy the rest of the host environment. Use
`credential_files` for required credentials.

The provider stores VM state under `$XDG_STATE_HOME/zellij/firecracker`. It stores the base image
under `$XDG_DATA_HOME/zellij/firecracker`. A crashed server can leave a VM running, so use the
management commands above to inspect and stop it.
