# Themes

Select a theme with `theme "gruvbox-light"` in `config.kdl`, or use the Change theme command.
Themes with a `terminal` block apply their foreground, background, and 16 ANSI colors automatically to terminal panes.
The host terminal keeps its own colors outside Yinz.

Use [example.kdl](example.kdl) for a complete example.
Add this optional block inside a theme, alongside its UI styles or legacy palette:

```kdl
terminal {
    foreground "#d8dee9"
    background 46 52 64
    black "#3b4252"
    red "#bf616a"
    green "#a3be8c"
    yellow "#ebcb8b"
    blue "#81a1c1"
    magenta "#b48ead"
    cyan "#88c0d0"
    white "#e5e9f0"
    bright_black "#4c566a"
    bright_red "#bf616a"
    bright_green "#a3be8c"
    bright_yellow "#ebcb8b"
    bright_blue "#81a1c1"
    bright_magenta "#b48ead"
    bright_cyan "#8fbcbb"
    bright_white "#eceff4"
}
```

When the block is present, all 18 colors are required.
Each color accepts `"#RGB"`, `"#RRGGBB"`, or three integers from 0 to 255.
Indexed colors are not valid in this block. Duplicate or unknown color names are errors.
Themes without the block remain valid and use host colors. The built-in `ansi` theme intentionally uses host colors.

Application color overrides take priority over explicit pane colors, which take priority over the theme.
Resetting an application override reveals the pane color or theme. Clearing a pane color reveals the theme.
Color queries return the effective local color, or go to the host when no local value applies.

Theme selection, configuration reload, and `theme_dark`/`theme_light` switching update existing panes and new panes.
The theme picker previews terminal colors. Cancelling a preview or failing to save restores the previous theme.
Pane background opacity still applies to themed colors.
Cursor colors, indexed colors 16–255, and application truecolor are not replaced by themes.

Managed Claude and Codex launches also map known agent palette colors to the active
theme. Set `agent_theme_mapping "off"` to disable this mapping. Codex UI colors
take priority when syntax highlighting shares the same source color, so matching
syntax colors change too. An explicit Codex accent wins over other semantic roles;
otherwise base text takes priority over accents and status colors. Syntax-only
colors remain unchanged.

Built-in palettes and their source links are in [the theme assets](../../yinz-config/assets/themes).
The Yinz-specific palettes have explicit matching colors. Published palettes are attributed beside each `terminal` block.
