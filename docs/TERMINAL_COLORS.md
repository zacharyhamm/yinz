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
  `#ffffff` in light mode.
- Registers 16-255 are always xterm's 6x6x6 color cube and 24-step grayscale
  ramp.

`CSI ? 996 n` answers the baseline mode. Panes that sent `CSI ? 2031 h` get one
`CSI ? 997 ; 1 n` (dark) or `CSI ? 997 ; 2 n` (light) when the baseline mode
changes, and nothing for a configuration reload that leaves the mode alone.

## Rendering and chrome

Panes are painted with the same terminal block applications are told about.
What an application is told therefore matches what it is painted on. A theme
preview is the one exception: while it is open, panes render the previewed
block. Cancelling or saving returns them to the baseline.

Chrome (frames, tab and status bars, plugin styling) is different. It still
follows the last client's own theme options and host appearance reports, and
plugins still receive `HostTerminalThemeChanged` events. Plugins are not shared
PTYs. Per-viewer rendering is later work.

Nested Yinz sessions and remote panes are parsed by the same terminal, so a
guest session and a remote application get the same answers as a local one.
Screen captures, dump-screen and web renders use the baseline palette.

## Visible changes

- Applications no longer see a viewer's real host colors. Without a configured
  terminal block they see the xterm defaults above. Configure a theme with a
  `terminal` block to give them specific colors.
- With a `theme_dark`/`theme_light` pair and no pin, a viewer whose terminal
  turns light gets light chrome, but pane colors and application answers stay on
  the dark theme. Switch them for everyone with `SetLightTheme`, `ToggleTheme`,
  `explicit_theme_hue` or a session-wide reconfigure.
