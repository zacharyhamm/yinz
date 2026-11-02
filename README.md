yinz is not zellij

A terminal multiplexer for managing agents. Started as a zellij plugin, but
zellij plugins couldn't do what I wanted, so I forked zellij. Many agent turns
later, we have yinz.

Hit "Ctrl+o+l" to launch the command palette. Or Ctrl+alt+p. Projects register
automatically when you launch an agent in a git repository. A project is defined
by the git remote, or if that is not set, just the path. "Create new project…"
in the palette makes a folder and git repository, then opens an agent or
terminal in it.

Yinz automatically rethemes agents launched via "Spawn". Type "theme" in the
command palette to choose a theme.

Type "config" in the command palette to access configuration options. One you
might want to set: max_agent_width, which controls how wide an agent panel can
be, so that agents stay readable.

See running agents with "Ctrl+e+a".

Yinz works best with a modern terminal emulator. I use kitty and ghostty. Yinz
makes use of the kitty graphics protocol.

Yinz is pretty fast! It runs `notcurses-demo` pretty well, which causes upstream
zellij to panic.

Yinz is plugin driven. Plugins are written in scheme using a fork of "steel". It
has a vim approach to plugins, just point it at a git repository (local or
remote): [Load Scheme plugins from Git repositories](docs/GIT_PLUGINS.md)

Yinz has good remote support. Use something like tailscale to make your remote
server accessible over the internet. Build yinz on the remote (we have no
automation for this right now), and on the client. Then from the client type:

`yinz attach --remote $HOSTNAME`

yinz will forward its unix socket over SSH back to the client. It will
automatically reconnect if the SSH connection is dropped. Feel free to close
your laptop and let it sleep. When you open it back up, yinz will reconnect to
your remote session automatically.

Yinz is mostly developed on Linux, although the client is tested on macOS a bit.

This is a personal project: it does what I want it to for my work. But it has
extensive plugin hooks, and can be heavily customized.

It surely has bugs. When I run into an issue, I tend to fix it on the spot. It
is pretty stable though, I haven't run into a panic in a long time.

Yinz works best with agents if you install the skills and hooks:

`yinz hooks install`

`yinz skills install`

## Install

Download the archive for your platform and `SHA256SUMS` from
[GitHub Releases](https://github.com/zacharyhamm/yinz/releases). Linux builds
target x86_64 or ARM64 on Ubuntu 24.04 and compatible newer glibc systems; macOS
builds target Apple Silicon on macOS 15 or newer. Linux runtime libraries
include glibc, libgcc, libstdc++, and zlib; each archive includes
`DEPENDENCIES.txt` with its exact linked libraries. macOS releases are not
signed or notarized.

For example, on Linux x86_64, from the directory containing both downloads:

```sh
sha256sum --ignore-missing -c SHA256SUMS
tar -xzf yinz-x86_64-unknown-linux-gnu.tar.gz
./install.sh
```

On macOS use `shasum -a 256 --ignore-missing -c SHA256SUMS` and the
`yinz-aarch64-apple-darwin.tar.gz` archive. Confirm the downloaded archive
reports `OK` before extracting it. The installer defaults to
`$HOME/.local/bin/yinz`; ensure `$HOME/.local/bin` is on your `PATH`. Builtins
go to `${XDG_DATA_HOME:-$HOME/.local/share}/yinz` on Linux (XDG_DATA_HOME must
be absolute) or `$HOME/Library/Application Support/org.Yinz-Contributors.Yinz`
on macOS.

Override destinations with
`./install.sh --bin-dir /path/to/bin --data-dir /path/to/data`. When using a
custom data directory, launch with `yinz --data-dir /path/to/data`. Install the
entire archive: copying only the binary, including via cargo-binstall, does not
install the required builtin plugins.

To build from this checkout, run:

```sh
cargo x install "$HOME/.local/bin/yinz"
```

This builds a release binary and installs builtin Scheme files into the default
Yinz data directory, under `plugins/builtins/`. Choose another binary
destination by changing the path. To select a data directory, use:

```sh
cargo x install "$HOME/.local/bin/yinz" --data-dir /path/to/data
yinz --data-dir /path/to/data
```

Reinstalling replaces shipped builtin files, including local edits. It preserves
unrelated files and user plugins. The sources are installed before the binary.
Copying only the binary does not produce a complete installation. There is no
embedded fallback or automatic runtime installation.

For development, `cargo x run` installs builtin files into its selected data
directory. Use `cargo x run --quick-run` for `target/dev-data`. `cargo x build`
does not install builtin files.

## Publishing a release

After merging the release workflow, update the workspace version in `Cargo.toml`
and the corresponding lockfile entries, then commit those changes. Tag that
commit with the matching version and push the tag:

```sh
git tag -a v0.1.0 -m "Yinz v0.1.0"
git push origin v0.1.0
```

The release workflow builds all three native archives, checks installation and
executable versions, and publishes them with SHA-256 checksums and generated
notes. Tags such as `v0.2.0-rc.1` publish prereleases and must also match the
workspace version. No personal token is required; GitHub Actions must be enabled
for the repository. Only the publishing job receives repository write
permission.

A failed upload leaves a draft; rerun the workflow to replace its assets and
finish publishing. Already published releases are never overwritten. Release
builds do not run the full workspace or integration test suites.
