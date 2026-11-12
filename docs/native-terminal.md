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

Font settings reload automatically with the session's configuration updates,
including drop-ins. A `config.d` directory first created while the session is
running is discovered during the normal polling/debounce cycle.

Changing `ligatures` or `font_ligatures` with `action reconfigure` requires
`--write`, or an edit to the session's saved configuration/drop-in. An unsaved
change to the effective ligature value refuses the entire fragment, including
sibling settings, with save guidance. Equivalent values and omitted ligatures
remain valid runtime requests. For example, disable and then restore ligatures:

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

Other effective font or metric changes (including mixed changes with ligatures)
replace face, shaping, glyph and atlas caches while preserving visible content
and the selected Copy text. When a grid resize reflows text, the local highlight follows a unique match of the selected
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
