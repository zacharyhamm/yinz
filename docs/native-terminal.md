# Native terminal windows

`yinz terminal` opens a GPU-rendered window on Linux or macOS. With one local
session it attaches to that session; with none it creates one. When several
sessions exist, specify the session name:

```sh
yinz terminal my-session
yinz terminal my-session --create
yinz terminal --working-directory ~/project --startup-mode maximized
```

Bare `yinz` continues to run inside the current terminal. Native and terminal
clients can attach to the same session. A native window needs a server from a
build with structured-render support; restart an older session before attaching.
Closing a window detaches its client and leaves the session running.

Native support is enabled by the default `native-terminal` Cargo feature.
For a build without graphical dependencies, use `--no-default-features` (and
`--features vendored_curl` if the installation needs bundled curl).
The native command reports that support is disabled in such a build.

Linux needs an X11 or Wayland desktop, libxkbcommon, and an EGL driver supporting
OpenGL ES 3. macOS uses an OpenGL core context. Fonts and their licenses are
embedded in the binary; fontconfig on Linux and system font discovery on macOS
provide additional fallback fonts when available.

## Configuration and input

Add optional settings to `config.kdl`. Omitted settings retain their defaults;
configuration drop-ins override individual fields. The session's theme may supply
terminal foreground, background, and ANSI colors through its existing terminal
palette. Configuration reloads also reload native settings.

```kdl
terminal {
    font "Iosevka Term"
    font_bold "Iosevka Term"
    font_italic "Iosevka Term"
    font_bold_italic "Iosevka Term"
    font_size 12.0
    font_weight 400
    ligatures true
    system_fonts true
    line_height 1.0
    cell_width 1.0
    padding 4.0
    opacity 1.0
    blur false
    cursor_style "block"
    cursor_blink true
    startup_mode "remember"
    macos_option_as_alt "none"
    open_links true
    middle_click_paste true
}
```

`font` selects the regular family. `font_bold`, `font_italic` and
`font_bold_italic` optionally select separate families for those styles; otherwise
they use the corresponding face of the regular family. `font_family` remains an
input alias for `font`. Serialization writes `font`. A single `terminal` block
must use only one regular-family spelling, even when both values agree; duplicate
settings are errors.

Drop-ins in `config.d/*.kdl` load in filename order after the main configuration.
Later fields override earlier fields, in either alias spelling. A changed regular
family logs the previous and winning requests and later-layer precedence. Invalid
drop-ins are logged with their paths and skipped as a whole. Invalid main
configuration prevents startup. Omission inherits the earlier
request; an empty or whitespace-only string explicitly clears it. A blank `font`
uses embedded defaults; a blank style family resumes regular-family selection.
Managed configuration writes replace either regular-family spelling as one setting.

`ligatures` enables programming ligatures by default. `font_ligatures` is a
compatibility input alias; serialization writes only `ligatures`. Both spellings
require one plain boolean (`true` or `false`). A block must contain only one
ligature setting: repeated or mixed spellings are rejected, even when values
agree. Across layers either spelling overrides the earlier value, including
explicit `false`; omission inherits. Managed writes replace either spelling as
one logical setting and preserve sibling settings. Invalid managed writes leave
the file unchanged.

The default is embedded Iosevka Term at 12 points, weight 400. `font_weight`
accepts integers 100–900: regular and italic request that weight, while bold and
bold italic request `min(weight + 300, 900)`. Installed static fonts use the closest
available face, so an exact requested weight is not guaranteed. Embedded faces
choose regular/italic through weight 500 and bold/bold italic above it. Named
families with unavailable styles or weights log the requested family/style/weight
and actual adopted face; missing families fall back nonfatally. Variable fonts use
their supported weight range. `system_fonts false` disables installed-family lookup
and keeps embedded fonts and fallback. Unavailable discovery is diagnosed separately
from disabled discovery. CJK, emoji and symbols retain the existing fallback stack;
style faces with different metrics are fitted into the regular face's cell.

`cell_width` and `line_height` independently multiply the font's natural cell width
and height. Both default to `1.0` and accept one plain finite number from `0.5`
through `3.0` inclusive (integers are accepted). Strings, type annotations, repeated
settings and out-of-range values are errors. Omission inherits a previous layer;
explicit `1.0` resets that dimension to natural spacing. For example,
`terminal { cell_width 1.25; }` widens spacing without changing line height,
`terminal { line_height 1.5; }` increases vertical spacing, and
`terminal { cell_width 1.25; line_height 1.5; }` combines them. These are spacing
multipliers, not font points: change `font_size` to change the glyph size itself.
CJK and emoji retain their logical column widths.

Font points convert to pixels using the platform convention (96 pixels/inch on
Linux, 72 on macOS). Zoom and monitor scale apply once to that font size; spacing
multipliers remain dimensionless. Natural physical cell dimensions round first,
then the multipliers apply and round again, with a minimum of one physical pixel.
Glyphs center horizontally in the adjusted cell and the baseline moves by half
the height change, bounded inside the cell along with decorations. This retains
the pinned source's rounding, including at fractional scales. Physical padding is
subtracted before floor-dividing the window by physical cell dimensions; the grid
is at least one row/column (at most 4096). A minimized zero-size window defers
geometry updates. Cursor/selection rectangles, hit testing, graphics and reported
cell/text-area pixels all use these same cells. Text-area pixels exclude padding
and unused edge pixels: they equal rows/columns times cell height/width.

Font settings reload automatically with the session's configuration updates,
including drop-ins. A `config.d` directory first created while the session is
running is discovered during the normal polling/debounce cycle.

Changing `cell_width`, `line_height`, `ligatures` or `font_ligatures` with
`action reconfigure` requires
`--write`, or an edit to the session's saved configuration/drop-in. An unsaved
change to any effective value refuses the entire fragment, including sibling
settings, for all requested clients, with save guidance. Equivalent values and
omissions remain valid runtime requests; explicit metric `1.0` equals omission
when the effective value is already the default. For example, disable and then restore ligatures:

```sh
yinz --session work action reconfigure --write --kdl 'terminal { ligatures false; }'
yinz --session work action reconfigure --write --kdl 'terminal { ligatures true; }'
```

`--write` persists into the session's managed drop-in. Write failures are errors.
Attached native windows update through file reload; successful command completion
does not mean the next native frame has already been drawn.

Startup applies the effective ligature setting; a live
ligature-only change clears cached shaped runs and redraws retained text rows.
Faces, discovery, independent grapheme/emoji shaping, raster glyphs and atlases
remain cached. Cells, column ownership, cursor coordinates, selection and copied
source text stay unchanged. Ligature-only changes keep physical cell metrics and
the grid unchanged, so they emit no resize or pixel-size reports. Combining and
emoji clusters still shape with programming ligatures disabled; cursor, pane,
style and fallback boundaries retain their existing behavior.

Metric-only changes retain loaded faces, discovery, character/cluster coverage
choices and independent grapheme shaping. They recompute physical metrics and
style-face fitting, discard cell-dependent rasters, cluster/procedural sprites
and atlases, and redraw retained rows when effective metrics or fitting change.
Width/fitting changes also clear cell-relative run shaping. Multipliers that round
to identical effective output retain those resources and send no geometry reports.
A simultaneous ligature toggle still clears run shaping. Other effective font
changes (family, weight, font size, zoom or monitor scale) replace affected face,
shaping, glyph and atlas caches. Both paths preserve visible content and selected
Copy text. When a grid resize reflows text, the local highlight follows a unique match of the selected
display units and surrounding context. If content moves offscreen or its location
is ambiguous (including pane-layout changes), the highlight disappears and Copy
retains the original text until a new selection, Escape, typing or session switch.
The structured frames do not identify hard versus soft wraps, so Copy preserves
the original selection’s newline semantics. Equivalent settings
and paint-only changes retain font caches. Padding changes update geometry without
reloading fonts. Zoom and monitor-scale changes use the same path; final raster
sizes are bounded to 4–512 physical pixels with a diagnostic when adjusted, without
changing configured point values. Cell/text-area reports refresh when their values
change; grid resize reports refresh when rows/columns change. New connections and
explicit size queries always receive current reports. Invalid reloads retain the
working native settings. Replacing installed font files in place is not a watched
configuration source; restart to reload those files.

Focused ligature checks cover both configuration spellings, sorted drop-ins,
managed writes and warmed enabled/disabled/enabled transitions through the
production font-update path. Linux headless EGL pixel checks compare live output
with independent fresh renders, including cursor interruption, selection and
Copy preservation, four styles, CJK and emoji. Headless EGL does not establish
Wayland/X11 desktop or macOS GL behavior. A separate Wayland smoke check for
this ligature change at scale 2 captured joined programming symbols, unjoined
symbols after a live legacy-alias drop-in, and the restored joined output after
a canonical drop-in. Desktop selection/clipboard behavior was not established
by that capture; the deterministic application and GPU checks cover selection
and Copy. X11 and macOS execution were unavailable.

An earlier font-settings change recorded a native Wayland smoke check at scale 2
for drop-in reload and CSI cell/text-area replies. Its powered-off display
prevented visual capture and manual selection/zoom checks; that observation is
prior evidence, not a ligature-toggle visual check.

Fresh issue #106 smoke evidence (2026-11-13): this worktree's debug `yinz`
ran on Linux/Sway Wayland, output DP-6 at 5120×2880 and scale 2, with embedded
Iosevka Term (`system_fonts false`, 12 points, padding 4). Saved drop-in reloads
produced the following measured physical geometry. CSI 16t cell replies, CSI 14t
text-area replies and the pane's TIOCGWINSZ pixel/grid values agreed:

| Width × height multiplier | Cell pixels | Grid columns × rows | Text-area pixels |
|---|---|---|---|
| 1 × 1 | 16 × 40 | 157 × 67 | 2512 × 2680 |
| 1.5 × 1 | 24 × 40 | 104 × 67 | 2496 × 2680 |
| 1 × 1.25 | 16 × 50 | 157 × 54 | 2512 × 2700 |
| 1.5 × 1.25 | 24 × 50 | 104 × 54 | 2496 × 2700 |
| Reset to 1 × 1 | 16 × 40 | 157 × 67 | 2512 × 2680 |
| Reset, resized window | 16 × 40 | 88 × 19 | 1408 × 760 |

Cropped desktop captures showed the changed horizontal/vertical spacing in four
styles, CJK, emoji and box drawing, followed by reset and window resize. The
fixture repainted after grid changes. Earlier one-shot raw-output captures became
blank when server reflow moved that output into scrollback; full server dumps
retained the text. Those captures do not establish viewport preservation, and
this change does not alter server scrolling policy. Structured application and
headless EGL tests separately exercise combining/ZWJ units, retained rendering,
selection/Copy and resize reanchoring, including zoom and simulated scale changes.

Desktop keyboard zoom/reset and drag/clipboard interaction were not exercised in
this run. Only one physical output was available, so no real mixed-DPI monitor
transition was verified. X11 (`DISPLAY` unset) and macOS were unavailable. These
remain desktop acceptance limitations; headless EGL and simulated scale matrices
do not substitute for those checks. Historical smoke observations above retain
their original scope and are not current issue #106 verification.

Font diagnostics go to the normal log, `$TMPDIR/yinz-<UID>/yinz-log/yinz.log`
(`/tmp` when `TMPDIR` is unset).

`startup_mode` accepts `windowed`, `maximized`, `fullscreen`, and `remember`.
The last window size is stored in Yinz's cache. `cursor_style` accepts `block`,
`bar`, and `underline`. `macos_option_as_alt` accepts `none`, `left`, `right`,
and `both`.

Use Ctrl+Shift+C/V (Command+C/V on macOS) for copy/paste. Hold Shift while
dragging to select locally when the session handles mouse events. Ctrl-click
opens a hyperlink. The copy shortcut copies that local selection. The middle
button pastes clipboard text when enabled. Ctrl+Shift+plus/minus/0 or
Command+plus/minus/0 changes or resets font zoom. Clipboard writes work; host
queries do not receive clipboard contents.

## Diagnostics

The hidden `--headless` option exercises the same session and structured frame
path without opening a window. `--rows`, `--cols`, and `--duration-secs` bound a
capture. `--record PATH` also works with graphical windows.

```sh
yinz terminal demo --create --headless --duration-secs 5 --record demo.frames
```

A recording contains repeated little-endian u32 frame lengths followed by frame
bytes. Decode each frame with `yinz_types::structured_render::decode`.

Windows, remote web attachment, tray/background services, desktop session/inbox
panels, and application registration are outside this first port.

## Upstream source

The rendering, font, input, frame codec, and graphics-consumer code is adapted
from the Zellij checkout at commit
`c8585459c37186fae1321727ad8dd4c96480ef41`. The upstream MIT license and individual
font licenses are retained under `yinz-native-terminal/`. Session startup,
configuration integration, and structured frame production use Yinz's existing
crate boundaries and IPC transport.
