# Firecracker VMs

Run a shell or coding agent in a Linux VM from Zellij. Each VM starts from a
copy of a base image. A linked host worktree is copied into `/workspace` in
the guest. File changes return to the host on sync or final close.

An image is a reusable template. A VM is a running copy. Rebuilding an image
affects new VMs. It does not update VMs that are already running.

## Install the host tools

This provider requires **Linux x86_64**, access to `/dev/kvm`, user namespaces,
and systemd for persistent networking. Run setup as your normal user. Setup
uses sudo for provider installation and networking.

1. Install this fork of Zellij and its `zellij-firecracker` companion. From
   this checkout, with the [build prerequisites](../AGENTS.md#build-and-test):

   ```sh
   mkdir -p "$HOME/.local/bin"
   cargo x install "$HOME/.local/bin/zellij"
   export PATH="$HOME/.local/bin:$PATH"
   ```

2. Install the image and network tools. On Ubuntu or Debian:

   ```sh
   sudo apt-get install curl git iproute2 iptables e2fsprogs rsync \
       openssh-client squashfs-tools uidmap util-linux coreutils sudo
   test -r /dev/kvm && test -w /dev/kvm && echo "KVM access ready"
   ```

   If the KVM check fails, follow the official
   [KVM access instructions](https://github.com/firecracker-microvm/firecracker/blob/main/docs/getting-started.md#kvm).
   A missing device can require enabling virtualization on the host. Group
   membership changes require a new login.

3. Install the `firecracker` executable on your PATH. Use the official
   [Firecracker installation instructions](https://github.com/firecracker-microvm/firecracker/blob/main/docs/getting-started.md#getting-a-firecracker-binary).
   This is separate from the `zellij-firecracker` companion. Check it with:

   ```sh
   firecracker --version
   ```

## Create an image and configure Zellij

From the project directory:

```sh
zellij-firecracker setup --name my-project
```

Setup checks prerequisites, installs a trusted provider if needed, configures
networking, builds or reuses the image, writes configuration, and runs doctor.
It makes this image the host's default for launches without a selected image.
Existing images are reused. Use `--rebuild` after changing the image recipe
or the Zellij binary that runs in the guest.

If `.zellij/firecracker-image.sh` exists in the project, setup runs it inside
the image to install your development tools. Without it, setup creates a
base Ubuntu image with Zellij, Git, and file sync tools. That base does not install coding agents.
The [recipe in this checkout](../.zellij/firecracker-image.sh) installs Rust,
build tools, Codex, and Claude Code. Image builds can take several minutes.

For a project that does not have a recipe, create a minimal one:

```sh
mkdir -p .zellij
cat > .zellij/firecracker-image.sh <<'EOF'
#!/bin/sh
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends build-essential git rsync
apt-get clean
rm -rf /var/lib/apt/lists/*
EOF
zellij-firecracker setup --name my-project --rebuild
```

Add your agent and project dependencies to the recipe before launching them.
The script runs as root inside the image. Do not put credentials in an image
recipe. Setup separately asks whether to copy detected Codex and Claude
credentials when a VM starts or is joined. Declining leaves those credentials
on the host.

Setup uses a 64 GiB sparse disk by default. Allocated disk usage grows as
files are added. Use `--disk-size-gib N` to change the capacity. Each VM has
its own writable disk. The default VM has 2 vCPUs and 2048 MiB of memory.

Use `--zellij /absolute/path/to/zellij` to select the guest binary. It must run
on Ubuntu 24.04. A host binary linked against newer glibc can fail in the guest.
Use a compatible build or a static `x86_64-unknown-linux-musl` build in that case.

For additional images after setup:

```sh
zellij-firecracker image build --name another-project --project /absolute/project/path
```

Unlike setup, an explicit `image build --project` requires that project's
recipe. Use `--customize /path/to/script.sh` for a standalone recipe, or omit
both options for a base image. `image build` defaults to 8 GiB and preserves
an existing default image selection. Use `--help` for all options.

## Launch and verify

Start a new Zellij session after first-time setup so it reads the provider
configuration. Reattaching to a session started before setup does not load it.
From a shell inside the new session, in your project:

```sh
zellij action new-pane --firecracker review \
    --firecracker-image my-project --firecracker-worktree "$PWD"
```

The new pane shows launch phases, then opens a shell in the guest workspace.
Run `pwd` there to check the guest directory. Paths passed as `--cwd` refer
to the guest. The host directory belongs in `--firecracker-worktree`.

To launch an agent, press **Ctrl e**, then **n**. Choose the project and agent,
and enable **Run in Firecracker**. Enter a branch name to create a worktree,
or clear **Create and switch branch** to use the selected checkout. Then
select **Create agent**. The image field fills with the project's configured
image when available. An empty field uses the host default. Enter `my-project`
to select that image explicitly.

Panes using the same host worktree share a VM, including across sessions.
Use a separate Git worktree when you need a separate VM. To open another pane
in an existing VM, use its name from the listing:

```sh
zellij firecracker list
zellij action new-pane --firecracker review
```

## Save changes and close

Open **Ctrl e**, then **a**, and select **VMs** for status, worktree, panes,
resource usage, and actions. Press **?** for the available keys.

From the CLI:

```sh
zellij firecracker changes review
zellij firecracker sync review
zellij firecracker close review
```

Use the name returned by `list`. Add `--session SESSION` when managing a VM
from another session or when no session is selected.

Sync copies file changes between the linked worktree and guest. It does not
transfer Git commits or ignored files. If both sides changed a file, the host
copy stays and the guest copy is saved beside it as `.guest` or `.guest.N`.
Review the warnings and resolve those copies before committing on the host.

Close ends that session's VM panes and releases its share of the VM. The final
release merges files, stops the VM, and deletes its writable disk. Another
session's lease keeps a shared VM running. Closing one pane alone can leave
the VM running until its session ends or you explicitly close the VM.

**A VM without a linked host worktree has no automatic file export.** Copy out
files you need before final close. Its writable disk is then deleted.

## Diagnose and recover

Run `zellij-firecracker doctor` to check the configured host. Configuration is
normally in `~/.config/zellij/firecracker.json`. Setup also writes the provider
path to Zellij's `config.d` directory. Both paths appear in setup output.

| Problem | Next step |
| --- | --- |
| Firecracker is not configured | Run `zellij-firecracker setup` from the project, then start a new Zellij session. |
| An image does not exist | Run `setup --name IMAGE` or choose an image you already built. |
| Package installation fails when changing file ownership | Check subordinate UID/GID ranges in `/etc/subuid` and `/etc/subgid`, and install `uidmap`. The image builder needs mappings for guest system users. |
| A TAP device is missing after reboot | Run `sudo zellij-firecracker network setup --user "$USER"`, then doctor. |
| No TAP device is free | Close unused VMs, or increase the pool with `network setup --user "$USER" --taps N` under sudo. |
| Agent command is missing | Install it in the image recipe, rebuild the image, and launch a new VM. |
| VM cannot start Zellij | Check the guest binary's compatibility and the launch error's console log. |

If provisioning or final sync fails, the provider retains the disk when
possible and reports its location. `zellij firecracker list` and the VMs
panel show retained disks. **They expire seven days after retention.**
Copy a needed disk outside the recovery directory before that deadline.

For example, use the recovery directory shown by the listing:

```sh
cp --sparse=always /path/from/list/rootfs.ext4 ./recovered-rootfs.ext4
mkdir recovered-workspace
debugfs -R 'rdump /workspace recovered-workspace' ./recovered-rootfs.ext4
```

`debugfs` comes with `e2fsprogs` and opens the disk read-only by default.
If you configured a different guest working directory, replace `/workspace`.
The example extracts files into `recovered-workspace/workspace/`. Inspect them
before copying them over the host worktree.
