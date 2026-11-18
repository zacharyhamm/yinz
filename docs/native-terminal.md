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
    baseline_offset 2
    underline_offset -2
    underline_thickness 1
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

`font_features` passes OpenType features to the shaper. It is one node with zero
or more string arguments:

```kdl
terminal {
    font_features "ss01" "zero" "-calt" "+dlig"
}
```

Each entry is a four-character OpenType tag of printable ASCII (`0x21`–`0x7e`),
optionally prefixed with one `+` (turn it on, the default) or `-` (turn it off).
Only on/off values exist; forms such as `"cv01=2"` are rejected. Each invalid entry
is an error that points at that entry, for example `terminal font_features: "ss1"
is not a font feature; use a four-character OpenType tag such as "ss01", optionally
prefixed with + to turn it on or - to turn it off`. Non-string entries (`4`, `true`,
`null`), named properties, type annotations, children and a second `font_features`
node in one block are errors too. A bare `font_features` node explicitly requests
no features; omission inherits an earlier layer, and the default is none. The
spelling and syntax match the pinned upstream source; there is no alias.

The list reaches the shaper in its written order, without deduplication; a
repeated or conflicting tag is passed through as written, as upstream does.
Precedence with `ligatures`/`font_ligatures` also follows upstream. An empty list
leaves the ligature setting alone. With ligatures off, `liga`, `calt` and `dlig`
are turned off unless the list names that tag, so `ligatures false` with `"+calt"`
re-enables contextual alternates. With ligatures on, `"-calt"` disables them. Any
listed feature turns run shaping on, even with ligatures off; an unrelated feature
such as `"zero"` does not re-enable ligatures. Features shape runs only: combining
and emoji clusters shape without them, and every glyph keeps its cell, so column
ownership, cursor coordinates and selection are unchanged.

Validation checks only the entry syntax. A tag changes rendering only when the
face shaping that text provides it; a tag the font lacks is accepted and has no
effect. The embedded Iosevka Term faces provide only `calt` (the contextual arrow
joins, such as `->`) and `ccmp`, so with the default font the example above changes
only `-calt`, and `ligatures` toggles `calt`. `ss01`, `zero`, `dlig` and `liga`
need a font that provides them, chosen with `font` (with `system_fonts` on). When
the primary font lacks listed tags, an info line naming them is written to the
normal log described under font diagnostics below, at startup and whenever the
font or the list changes.

Across layers a later list, including a bare node, replaces the earlier list
whole; tags are not merged. Invalid drop-ins are skipped as a whole. Serialization
writes one node with each entry as an argument (a bare node for an empty list) and
round-trips. Managed writes replace the node as one setting and preserve siblings;
invalid managed writes leave the file unchanged.

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

`baseline_offset` moves the glyph baseline in logical pixels: positive values
lower it and negative values raise it. It defaults to `0` and accepts one plain
integer from `-64` through `64` inclusive. Strings, fractional values (including
`2.0`), booleans, null, type annotations, named/multiple arguments, children and
repeated settings are errors. Omission inherits; explicit `0` resets an earlier
layer. Serialization and managed writes preserve that zero and sibling settings.
For example, `terminal { baseline_offset -2; }` raises the baseline two logical
pixels. This is a pixel offset, independent of the platform's font-point units.

The physical offset is `baseline_offset * zoom * monitor scale`, rounded once to
the nearest physical pixel (halfway values away from zero), then added to the
natural baseline and half of any line-height change. The result clamps inside
the cell, from physical pixel 1 to the cell height. Decorations follow the bounded
baseline and remain inside the cell. Different style families and fallback glyphs
retain existing fitting/cropping; full-cell emoji can be constrained differently
from Latin, so extreme offsets need not translate every glyph's ink equally.
Offset alone preserves physical cell dimensions, grid, hit testing, cursor and
selection ownership, Copy text and cell/text-area pixel reports.

`underline_offset` moves text underlines down with positive values and up with
negative values. `underline_thickness` adds to the font's stroke with positive
values and thins it with negative values. Each accepts one plain integer from
`-64` through `64` inclusive, using the same syntax as `baseline_offset`. With
`terminal { underline_offset -2; underline_thickness 1; }`, the underline moves
up two logical pixels and thickens by one. Omission inherits the setting from the
earlier layers. Explicit `0` restores the font-derived value, which is also the
default when no layer supplies the setting. Drop-ins load in lexical order and
each key takes its value from the last valid layer that supplies it.

The terminal multiplies each adjustment by zoom and monitor scale, including
fractional values, then rounds the result once to the nearest physical pixel.
The stroke clamps to at least one pixel and at most the cell height. The position
and each pattern stroke stay inside the row. Straight, double, dotted, dashed and
curly underlines, as well as the IME preedit underline, use the regular face's
metrics, including over fallback glyphs and selected text. Strikeout and the
underline cursor have separate geometry.

Startup and saved reload use the same underline settings. An underline-only
reload updates the affected solid geometry and retains the loaded fonts, glyph
and image resources. Cell dimensions, the grid, hit testing, pixel-size reports,
selection and copied text stay the same. Combined changes to baseline, spacing,
or shaping also refresh the resources those settings affect.

Startup and saved file/drop-in reload use the same offset. Baseline-only reloads
reuse the metric path described below: faces, discovery and independent cluster
shaping survive; effective baseline/decoration or style-fit changes invalidate
cell-dependent glyphs and atlases and redraw retained rows. When baseline rendering
and style fitting agree and other rendering inputs are unchanged, equivalent saves
(including omission versus explicit zero) preserve fonts, atlases, clean retained
rows and their row buffers. Existing terminal damage still refreshes affected rows;
combined visual or shaping changes refresh the resources they require, including
run shaping for ligature or feature changes. With unchanged image content and
placements, pane geometry, cell dimensions and held-out regions, baseline-only
text changes retain inline-image placement and vertex resources. Images refresh
when their own inputs or the renderer's scene/reset state require it.
Unsaved `action reconfigure` refuses an effective offset change atomically; use
`--write` or edit the saved file. The server compares configured integers with
default zero, even when a particular window's font/scale would clamp different
integers to identical pixels. Explicit
zero equals omission only when the effective request is already zero. Saved
changes reach native windows asynchronously through file reload and the next
frame. Invalid main-file reloads preserve the active setting until a valid
correction; invalid drop-ins are logged and skipped as a whole.

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

Changing `cell_width`, `line_height`, `baseline_offset`, `underline_offset`,
`underline_thickness`, `ligatures`, `font_ligatures` or `font_features` with
`action reconfigure` requires
`--write`, or an edit to the session's saved configuration/drop-in. An unsaved
change to any effective value refuses the entire fragment, including sibling
settings, for all requested clients, with save guidance. Equivalent values and
omissions remain valid runtime requests; explicit metric `1.0` equals omission
when the effective value is already the default. For `font_features`, omission and
a bare node are both no features and `"calt"` equals `"+calt"`; a reordered list
counts as a change. An invalid fragment, with or without `--write`, is refused with
the same per-entry message as file validation, such as the `"ss1"` error above; it
names the entry rather than drawing the source pointer that `yinz setup --check`
shows. For example, disable and then restore ligatures:

```sh
yinz --session work action reconfigure --write --kdl 'terminal { ligatures false; }'
yinz --session work action reconfigure --write --kdl 'terminal { ligatures true; }'
```

For each explicitly requested underline key, the server compares the logical
value with the winning value in the session's saved main file and drop-ins. A
missing saved key counts as zero. Only the winning saved file value establishes
equivalence. CLI refusals return a nonzero
status with save guidance. Attached plugins get enqueue success and the session
server log records any subsequent refusal. The plugin config write-failure event
reports persistence failures.

The session watches the main file and config directory it started with. `--write`
persists to that directory's `config.d/yinz-managed.kdl`. If a lexically later
drop-in supplies a key, change the winning drop-in to apply the desired value.
If an underline change requires a managed write and that write fails, the entire
fragment stays unapplied. Correct the reported setting or restore a readable
regular file at the reported path, then retry. Use KDL for configuration and
layout files, plain text for pane contents files such as a layout's
`preview.txt`, and a usable regular file for the managed lock. An invalid main
file leaves the current native settings in use, and invalid drop-ins are skipped
as whole layers. A managed save can succeed while the main file is unusable or
missing when the fragment can be validated against the built-in defaults.
The session watcher reports unreadable configuration or layout inputs in the
session log and retains the last working configuration. Restore the reported
path to resume automatic reload. An optional swap layout or pane contents file
can also be removed to use the layout's fallback. The watcher also retries
malformed KDL in the selected primary layout or swap layout. Correcting the
reported layout or removing an optional swap applies the pending configuration
without another edit to the main file or a drop-in.

On a session file-update notification, each native window rereads its own
`--config` and `--config-dir` layers. Use shared watched paths for automatic
updates. Native configuration inputs must be
readable regular files. Symlinks to regular files work. Startup errors appear
on stderr. Opening or switching sessions also checks the selected layout and
its swap and pane contents files. With a configured layouts directory, an
implicit or bare built-in name such as `default` selects the built-in unless
`default.kdl` exists in that directory. If both `default` and `default.kdl`
exist there, the extensionless file takes precedence. Use an explicit path
such as `./layouts/default` to select an extensionless file directly. For a
new native session, the server checks the same selected file, swap and pane
contents as it opens them, and reports any unusable input.

A `SwitchSession` keybinding gets tab metadata from the primary layout when
available. Its swap is read only when that layout
is selected. If a selected input is reported as unusable, restore a readable
regular file or remove an optional input to use the layout's fallback before
retrying. A graphical native window connected to a server from the same build
keeps its source session active until the destination supplies its first
structured frame. If the destination fails to open or render, the window shows
the error and keeps the source available. Press Escape to dismiss the notice.
Correct the reported input and use the same switch action again.

If the window cannot read its configuration on reload, it keeps its
current settings and reports the error in the client log. After repairing a
private path, reopen the window or make a valid edit to shared watched settings
to trigger another reload.
Successful `--write` completion confirms persistence. Reload and redraw follow
asynchronously when the window can load the files and the edited values win.

Startup applies the effective ligature setting; a live
ligature-only change clears cached shaped runs and redraws retained text rows.
Faces, discovery, independent grapheme/emoji shaping, raster glyphs and atlases
remain cached. Cells, column ownership, cursor coordinates, selection and copied
source text stay unchanged. Ligature-only changes keep physical cell metrics and
the grid unchanged, so they emit no resize or pixel-size reports. Combining and
emoji clusters still shape with programming ligatures disabled; cursor, pane,
style and fallback boundaries retain their existing behavior.

Startup applies the effective feature list. A live change to `font_features`
alone, or together with `ligatures`, takes the same path as a ligature-only change:
it clears cached shaped runs and redraws retained text rows, and keeps faces,
discovery, cluster shaping, raster glyphs, atlases, cells, cursor, selection, Copy
text and physical geometry, so it emits no resize or pixel-size reports. Upstream
rebuilds all fonts for any font change; this lighter path is a Yinz adaptation, and
tests check that its output equals a fresh build. Combined with a `cell_width` or
`line_height` change, it follows the metric path below and still clears shaped runs.
Combined with a family, weight, size, zoom or monitor-scale change, it follows the
replacement path.

Metric-only changes retain loaded faces, discovery, character/cluster coverage
choices and independent grapheme shaping. They recompute physical metrics and
style-face fitting, discard cell-dependent rasters, cluster/procedural sprites
and atlases, and redraw retained rows when effective metrics or fitting change.
Width/fitting changes also clear cell-relative run shaping. Multipliers that round
to identical effective output retain those resources and send no geometry reports.
A simultaneous ligature toggle or feature-list change still clears run shaping,
including when only the line height changes. Other effective font
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

Focused `font_features` checks cover entry validation and spans, omission versus
an empty list, round trips, wholesale layering through sorted drop-ins, managed
writes, unsaved `action reconfigure` refusal, production font options and
precedence with the ligature setting. The unprovided-tag log is unit-tested
against the embedded faces. Warmed reload checks with embedded Iosevka
Term toggle its `calt` arrow join through feature-only, feature-and-ligature,
feature-and-metric and replacement updates, comparing live scenes with fresh
caches. Linux headless EGL pixel checks compare live output with fresh renders at
two cursor positions. Wayland/X11 desktop and macOS rendering of this setting were
not checked.

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

Issue #107 baseline-offset observations (2026-11-15): this worktree's debug
binary ran in bounded foreground Linux/Sway Wayland sessions on DP-6 at
5120×2880, scale 2, using embedded Iosevka Term, 12 points and padding 4.
Cropped captures of a visible native window showed `+2` lowering Latin ink by
4 physical pixels and `-2` raising it by 4, with the same Latin/four-style,
CJK and emoji sample retained. Saved `--write` changes and direct drop-in edits
reloaded; an invalid main file retained the prior placement, and a corrected
file applied the negative offset. Unsaved reconfiguration returned save guidance.
After the window was resized and the sample repainted, offset-only changes kept
the 98×18 grid, 16×40 cell pixels and 1568×720 text-area pixels; native CSI 16t,
CSI 14t and pane TIOCGWINSZ replies agreed before/after the changes.

These observations establish Wayland scale-2 placement and file-event delivery.
Earlier obscured/expired-window crops are excluded. Desktop keyboard zoom/reset,
drag selection/clipboard, physical normal-scale and mixed-DPI transitions remain
outstanding acceptance checks; X11 and macOS were unavailable. Portable CPU tests
and Linux EGL pixel tests separately cover signed/boundary offsets, fractional
scales, zoom/reset, distinct style families, CJK, emoji/combining/ZWJ clusters,
selection/Copy, geometry, resource invalidation and clamped-equivalent updates.
They do not substitute for the outstanding desktop observations.

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
