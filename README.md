yinz is not zellij

A terminal multiplexer for managing agents. Started as a zellij plugin, but
zellij plugins couldn't do what I wanted, so I forked zellij. Many agent turns
later, we have yinz.

Hit "Ctrl+o+l" to launch the command palette. Or Ctrl+alt+p. Projects register
automatically when you launch an agent in a git repository. A project is defined
by the git remote, or if that is not set, just the path.

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

Yinz is plugin driven. Plugins are written in scheme using a fork of "steel":

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

[Load Scheme plugins from Git repositories](docs/GIT_PLUGINS.md)

## Install

There are no binary releases at this time. From this checkout, run:

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
