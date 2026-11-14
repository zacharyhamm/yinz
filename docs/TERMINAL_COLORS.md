# Terminal colors in shared panes

A pane is shared. Several clients can view it at once, each in a host terminal
with its own colors and dark or light appearance. Applications in the pane ask
the terminal about its colors: `OSC 10 ; ?` (foreground), `OSC 11 ; ?`
(background), `OSC 4 ; n ; ?` (palette register `n`), `CSI ? 996 n`
(appearance), and `CSI ? 2031 h` (subscribe to appearance changes). Yinz answers
all of these from one **application baseline** that belongs to the session. The
answers never depend on which viewer is attached, typed last, or reported last.

## Answer order

For `OSC 10`, `OSC 11` and `OSC 4`, the pane answers itself, in this order:

1. The application's own override: an earlier `OSC 10`/`OSC 11`/`OSC 4` set
   from inside the pane. `OSC 110`, `OSC 111`, `OSC 104` and `RIS` remove it.
2. An explicit pane color set with `yinz action set-pane-color` (foreground and
   background only).
3. The session's application baseline.

These queries are never forwarded to a client's host terminal. Clipboard and
pixel-size queries are still forwarded as before.

## What sets the baseline

The baseline comes only from the session's own configuration:

- the session-creating client's configuration (config file plus its command
  line) when the session starts;
- a config file reload from disk, which replaces the session theme. This means
  the creator's command-line theme options last only until the next
  config-file change, as other runtime settings already do;
- saved changes: `yinz action reconfigure --write` and plugin saves. Only the
  saved fragment is applied, never the writing client's whole configuration;
- session-wide unsaved changes: `yinz action reconfigure` without `--write`, or
  a reconfigure while no client is attached;
- settings-editor and theme-picker saves, for the session or permanently;
- the session-wide `SetDarkTheme`, `SetLightTheme` and `ToggleTheme` actions.
  `ToggleTheme` flips the mode applications are currently told.

These never change it:

- an attaching client's own theme options (`theme`, `theme_dark`,
  `theme_light`, `explicit_theme_hue` from its config or command line);
- one client's unsaved plugin reconfigure;
- a host terminal's color replies (`OSC 10/11/4`) and appearance reports
  (`DSR 997`);
- theme previews in the theme picker.

## How the baseline is resolved

- With both `theme_dark` and `theme_light` defined, the mode is the manual pin,
  then `explicit_theme_hue`, then dark. The colors are the terminal block of the
  theme for that mode.
- Otherwise pins are ignored. The mode is `explicit_theme_hue`, or else the
  brightness of the static `theme`'s background. The colors are the static
  theme's terminal block.
- Without a terminal block, applications get xterm's default colors: ANSI 0-15
  as in xterm, foreground `#e5e5e5` on `#000000` in dark mode and `#000000` on
  `#ffffff` in light mode. This applies to the default theme (`default`) and to
  any theme that has no `terminal` block. The mode is `explicit_theme_hue` if
  the session sets it, otherwise the brightness of the theme's background. The
  built-in default palette counts as dark, so applications are told dark
  whatever any viewer's terminal looks like.
- Registers 16-255 are always xterm's 6x6x6 color cube and 24-step grayscale
  ramp.

`CSI ? 996 n` answers the baseline mode. Panes that sent `CSI ? 2031 h` get one
`CSI ? 997 ; 1 n` (dark) or `CSI ? 997 ; 2 n` (light) when the baseline mode
changes, and nothing for a configuration reload that leaves the mode alone.

## Rendering and chrome

When the baseline has a terminal block, panes are painted with the same block
applications are told about, so what an application is told matches what it is
painted on. A theme preview is the one exception: while it is open, panes render
the previewed block. Cancelling or saving returns them to the baseline.

Without a terminal block, panes keep each viewer's host default colors and host
ANSI palette, while applications are told the xterm fallback. The two can then
differ: a light host background behind an application told the background is
black, or the host's ANSI 0-15 against xterm's in `OSC 4` answers. See the
remedies under [Visible changes](#visible-changes).

Chrome (frames, tab and status bars, plugin styling) is different. It still
follows the last client's own theme options and host appearance reports, and
plugins still receive `HostTerminalThemeChanged` events. Plugins are not shared
PTYs. Per-viewer rendering is later work.

Nested Yinz sessions and remote panes are parsed by the same terminal, so a
guest session and a remote application get the same answers as a local one.
Screen captures, dump-screen and web renders use the baseline's terminal block
when one is configured, and host default colors otherwise.

## Visible changes

- Applications no longer see a viewer's real host colors. Without a configured
  terminal block they see the xterm defaults above. Configure a theme with a
  `terminal` block to give them specific colors.
- Light terminals, and terminals that follow the operating system's light or
  dark appearance: without a terminal block, applications are told dark (a black
  background and `CSI ? 997 ; 1 n`) even on a light terminal, so adaptive tools
  such as editors and pagers pick dark colors. A host appearance change,
  including the operating system switching between light and dark, no longer
  reaches applications in panes. To tell them light, set the session's own
  configuration:
  - put `explicit_theme_hue "light"` in the config file, or
  - run `yinz action reconfigure 'explicit_theme_hue "light"'` for the running
    session (add `--write` to keep it), or
  - save it from the settings editor.

  This selects the light fallback (`#000000` on `#ffffff`) and sends one
  `CSI ? 997 ; 2 n` to panes that subscribed. An attaching client's own
  `--explicit-theme-hue` or config option only restyles chrome. Alternatively,
  configure a theme with a `terminal` block, or a `theme_dark`/`theme_light`
  pair and switch with `SetLightTheme`, `SetDarkTheme` or `ToggleTheme`.
  Automatically following each viewer's appearance is later per-viewer work.
- With a `theme_dark`/`theme_light` pair and no pin, a viewer whose terminal
  turns light gets light chrome, but pane colors and application answers stay on
  the dark theme. Switch them for everyone with `SetLightTheme`, `ToggleTheme`,
  `explicit_theme_hue` or a session-wide reconfigure.
