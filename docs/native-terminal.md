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
clients can attach to the same session. A native window needs a server that advertises native render negotiation on
its attachment. Update and restart an older server or use an ANSI attachment.
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
    padding_top 2.0
    padding_left 6.0
    padding_balance false
    padding_color "background"
    minimum_contrast 1.0
    opacity 1.0
    opacity_mode "background"
    blur false
    cursor_style "block"
    cursor_blink true
    cursor_trail true
    cursor_unfocused_hollow true
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

`padding` sets the logical padding on all four sides (default 0).
`padding_top`, `padding_right`, `padding_bottom` and `padding_left` override
single sides. Each side resolves as its own key, then `padding`, then 0. This
happens after drop-ins merge, field by field, so a side set in any layer beats a
shorthand from any layer. A later `padding` does not clear an earlier
`padding_left`; set the side again to change it. All five keys take one plain
number (integer or decimal) from 0 to 256 logical pixels. Upstream allows up to
1000; Yinz keeps its existing 256 limit for every padding key. Errors name the
setting and point at its node. Negative, non-finite, typed (`(px)4`), named or
string values are rejected, as are repeated keys and unknown spellings such as
`padding_x`. Serialization writes each set key separately.

`padding_balance true` centers the grid in the space that padding leaves. By
default (`false`) the grid starts at the top-left padding and every leftover
pixel goes to the right and bottom. With balancing, half of each axis's
leftover (rounded down) goes before the grid, and an odd pixel goes after it.
`padding_balance` takes one plain boolean.

`padding_color` chooses what the padding shows. `"background"` (the default when
the key is omitted) leaves the padding as the window clear colour. `"extend"`
continues the background of the nearest edge cell outward: each row's first and
last cell colour the left and right strips, each column's first and last cell
colour the top and bottom strips, and the four corners use the corner cells. The
right and bottom strips include any partial-cell remainder and the balancing
offset, so every pixel outside the grid is covered. In an opaque window only
backgrounds that differ from the default background are painted. With `opacity`
below 1 in `opacity_mode "background"`, only cells with an explicit background or
reverse video are painted, and default-background edges stay see-through like the
grid. In `opacity_mode "everything"` the padding is painted as in an opaque window
and fades with the rest of the window. Selection, the cursor,
IME preedit and link hover do not extend into the padding; the switch notice
draws above it. Colours come from the session theme's effective palette and follow
configuration and theme reloads (native theme adaptation is a separate issue).
The padding is recomputed on every frame, so new content, scrolling, resizing,
scale or zoom changes and reloads show on the next frame. The value is one plain
string matched ASCII-case-insensitively, like the pinned source (`"Extend"` is
accepted); serialization always writes lowercase. Other strings such as
`"mirror"`, non-strings, type annotations, properties, children, extra arguments
and repeated keys are errors that name both valid values (or the duplicate) and
point at the node. Startup reads it from the configuration. Saved file and
drop-in edits reload live and only redraw, without rebuilding fonts, rows or
geometry; a later layer wins and omission inherits.

`minimum_contrast` sets the lowest contrast ratio allowed between displayed text
and its background. It takes one plain number (integer or decimal) from 1 to 21;
the default when omitted is 1, which turns it off and draws colours exactly as
before. The ratio is the WCAG relative-luminance contrast ratio, so 4.5 and 7 are
the usual readability levels and 21 means black on white. When a cell's text
colour falls below the minimum against its background, the text colour is blended
toward white or toward black in 255 steps, and whichever reaches the minimum with
the smaller move wins. When neither can reach it, the text is drawn in whichever
of white or black contrasts more. This is the pinned source's algorithm and range.
The adjustment applies per cell to the colour actually displayed: named, indexed,
truecolor and default foregrounds, after dim (faint) and reverse video are applied,
against the cell's effective background. It also applies to explicit underline
colours, to text under a block cursor (contrasted against the cursor colour) and
to the cursor trail. Selected text swaps the already contrasted text and
background colours; the ratio is symmetric, so selected text keeps the minimum.
Concealed (hidden) text and blinked-out text are not made visible. Only the
displayed colour changes: theme colours, the palette and stored cell colours are
never modified. Setting the value back to 1, or omitting it, restores the original
colours exactly; a lower value above 1 recomputes the adjustment from the unchanged
stored colours, so pairs still below it stay adjusted.
Errors name the setting and its range and point at the node: values outside 1..21
such as `0.9` or `22`, non-finite numbers, strings, booleans, a bare node, type
annotations, properties, children, extra arguments and repeated keys. Startup uses
the configured value. Saved file and drop-in edits reload live and take effect on
the next frame; a change only redraws, without rebuilding fonts or geometry, and
content and selection are kept. A palette or theme reload re-adjusts text against
the new colours. A later layer wins and omission inherits.

`opacity` sets how opaque the window is, from 0 (fully see-through) to 1 (opaque,
the default when omitted). It takes one plain number (integer or decimal). Errors
name the setting and its range and point at the node: values outside 0..1 such as
`-0.01` or `1.01`, non-finite numbers, strings such as `"0.5"`, booleans, a bare
node, type annotations, properties, children, extra arguments and repeated keys.
This is not the multiplexer's `background_opacity`, the percentage background
opacity of Agent, the command palette and translucent overlays inside the
session.

`opacity_mode` chooses what `opacity` applies to, with the pinned source's
spelling and semantics. `"background"` (the default when omitted) lets only the
default background show through, including a session theme's terminal
background when it matches the window's own `theme`, and keeps text, coloured cells, the cursor and images solid.
`"everything"` fades the whole window evenly. The value is one plain string
matched ASCII-case-insensitively (`"Everything"` is accepted);
serialization always writes lowercase. Other strings such as `"sideways"`,
non-strings, a bare node, type annotations, properties, children, extra arguments
and repeated keys are errors that name both valid values (or the duplicate) and
point at the node. With `opacity 1` the mode makes no difference.

| Layer | `"background"` | `"everything"` |
|---|---|---|
| Window clear, default-background cells (including a theme's terminal background in the window's own background colour), default padding | `opacity` | `opacity` |
| Explicit and reverse-video backgrounds, selection | solid | `opacity` |
| Text, underlines and other decorations | solid | `opacity` |
| Cursor | solid | `opacity` |
| Cursor trail | blends over the clear | `opacity` |
| Inline images, at every depth | solid | `opacity` |
| `padding_color "extend"` fills | solid; default edges see-through | `opacity` |
| Scroll-animation layer | solid | `opacity` |
| Session switch notice | solid | solid |

`"everything"` fades the finished frame by `opacity`, so the window keeps its
look and the desktop shows through evenly. `"background"` instead leaves the
default background unpainted over a see-through clear. The session switch notice
is a Yinz addition drawn after the fade, so failure guidance stays readable in
both modes. A session theme with a `terminal` block (every bundled theme) gives
panes a concrete default background. The window draws its clear in its own
background colour, taken from the `theme` option in the configuration files it
reads. When a pane's theme background has that same colour, native windows treat
pane cells on it as default background: in `"background"` mode they show through
like the rest of the clear. When the colours differ, those cells stay solid in
the colour the session paints, so a pane never shows another theme's background
behind its text. This happens with a `theme_dark`/`theme_light` pair, which
renders panes in the pinned or hue-selected variant (dark by default) whatever
`theme` names, while a theme preview is shown, and when `theme` is unset or names
a theme without a `terminal` block. To let pane backgrounds show through with a
pair, set `theme` to the variant in use, or use `"everything"`. A background a
program sets explicitly, even to the theme's colour, stays solid, as does a pane
default background set for one pane. The built-in `default` theme has no
`terminal` block, so its pane cells already use the default background.
`"everything"` is not affected.

Startup uses the configured values. Saved file and drop-in edits reload live:
a change to `opacity` or `opacity_mode` rebuilds the drawn rows once on the next
frame, without rebuilding fonts or geometry, and content and selection are kept.
A `blur` change does not redraw (see below). A later layer wins and omission
inherits. An invalid edit keeps the working settings and logs the reload error.

See-through pixels need a compositor. Wayland compositors always composite. On
X11 a compositing manager must be running and the window needs a 32-bit ARGB
visual. On macOS Yinz asks for a transparent OpenGL config, as the pinned source
does, and makes the window non-opaque only while the effective opacity is below 1,
again after each reload. Among the configs the display offers, Yinz prefers one
that reports transparency, then one that does not say, then one that cannot, and
among equals the most antialiasing samples. EGL reports transparency from the X11
visual's alpha channel, or from the config's alpha bits on Wayland, not from
whether a compositor runs. Xorg and Xwayland normally offer an ARGB visual, so on
X11 Yinz also checks for a compositing manager, which owns the
`_NET_WM_CM_S<screen>` selection, when the window opens and on each reload while
`opacity` is below 1. When the chosen config cannot show transparency, or no X11
compositing manager is running, the window draws opaque instead of darkened and
logs one warning to the client log (`$TMPDIR/yinz-<UID>/yinz-log/yinz.log`), for
example: `terminal opacity 0.6 was requested, but no X11 compositing manager is
running (no _NET_WM_CM_S<screen> selection owner), so the window stays opaque;
start a compositor and save the configuration again to apply it`. When the config
is the cause the message reads `...but the display offers no transparent surface
(is a compositor running?), so the window stays opaque`. A compositor started
later applies on the next saved configuration change; Yinz does not watch for it
in between, and a compositor that stops while the window is open leaves it
darkened until that reload. The warning repeats only after transparency was
available again. When the X server cannot be asked, Yinz logs that and keeps the
configured opacity. Run a compositor or set `opacity 1`.

Observed outcomes for this change, on Linux with Mesa 26.2 and this build. The
outputs were headless sway 1.12 outputs over a magenta background, with text, an
explicit background, a Kitty image and `padding_color "extend"` padding. Physical
monitors were not used.

- Wayland and Xwayland under sway (composited): `"everything"` at 0.6 and 0.3
  blended every probed layer evenly with the desktop, for example the default
  background (24,19,30) became (116,11,120) at 0.6. `"background"` at 0.6 left
  `padding_color "background"` padding see-through at the same blend, and pane
  cells on the theme background stayed solid. That was before native windows
  treated a theme background as the default background (described above); the
  repaired behaviour is covered by scene and wire tests and was not observed
  again on these outputs. Live edits
  switching the mode and opacity applied without changing the grid, the cell
  pixel size or the text-area pixel size reported by CSI 14t and 16t. An invalid
  `opacity_mode "sideways"` kept the working settings and logged the error.
- X11 without a compositing manager (rootful Xwayland 24 on headless sway, no
  window manager): before the compositing manager check, both modes looked
  opaque and darkened by the opacity, and no warning was logged. With the check,
  `"everything"` at 0.5 drew opaque at full intensity (the status bar kept
  (175,255,0)) and logged the X11 compositing manager warning once.
- Xwayland managed by sway's own X11 window manager, which owns
  `_NET_WM_CM_S0`: `"everything"` at 0.5 logged no warning and kept the
  configured opacity.
- macOS: not verified in this change.

`blur` asks the compositor to blur what shows through the window. It takes one
plain boolean; omission means `false`. Bare `blur`, extra arguments, properties,
children, type annotations and other values such as `1`, `"true"` or `null`
fail at the node with `terminal blur must be a plain boolean true or false`; a
repeated key reports `duplicate terminal setting blur`. Blur affects only
see-through pixels and never makes the window transparent itself, as in the
pinned source, so it shows only while `opacity` is below 1 and transparency is
available, in either `opacity_mode`. Whether it applies depends on the windowing
system:

| Platform | Blur |
|---|---|
| KDE Plasma (Wayland) | Applied to the whole window through `org_kde_kwin_blur_manager` |
| Other Wayland compositors (sway, GNOME and others) | Unsupported; one warning, which notes when the compositor offers the staging `ext_background_effect_manager_v1` that Yinz does not use yet |
| X11, including Xwayland | Unsupported; one warning |
| macOS | Applied through winit's window background blur, visible only while the window is see-through |

Yinz uses winit's blur, which binds only KDE's Wayland protocol, does nothing on
X11 and reports no capability. When a Wayland window opens, Yinz therefore lists
the compositor's globals once, on winit's own connection, to tell whether blur
applies. winit binds the blur protocol only at startup, so a compositor that
starts offering it later needs a new window. If the listing fails, Yinz logs that
and requests blur anyway, reporting its support as unknown.

Startup applies the configured value right after the window is created, before
the first frame. Saved file and drop-in edits apply live without a redraw and
without changing fonts, geometry, content or the selection; `blur false` (or
removing the setting) clears an applied blur, and an invalid edit keeps the
working value. A reload that arrives before the window exists is applied when
it opens. On an unsupported backend Yinz does not call winit at all. Unsaved
`action reconfigure` cannot change blur; see the reconfigure section.

Each outcome is logged once to the client log
(`$TMPDIR/yinz-<UID>/yinz-log/yinz.log`) and again only when it changes. Turning
blur off logs nothing and re-arms the messages, so enabling it again reports
again. Warnings:

- X11: ``terminal blur was requested, but the window uses the X11 backend
  (including Xwayland), where Yinz has no blur protocol, so nothing is blurred;
  run the native terminal under a Wayland compositor that offers
  org_kde_kwin_blur_manager (KDE Plasma), or set `blur false` ``
- Wayland without the protocol: ``terminal blur was requested, but the Wayland
  compositor does not offer org_kde_kwin_blur_manager, so nothing is blurred;
  use a compositor that offers it (KDE Plasma), or set `blur false` ``, followed
  by `(the compositor offers ext_background_effect_manager_v1, which Yinz does
  not use yet)` when it does.
- Opacity below 1 on a display without a transparent surface, after the opacity
  warning above: ``terminal blur was requested, but the window stays opaque
  because the display offers no transparent surface (see the terminal opacity
  warning), so blur has no visible effect; it applies once transparency is
  available, or set `blur false` ``. An unsupported backend reports its own
  warning instead.

On a capable backend Yinz logs at info level `terminal blur is requested through
<protocol>`, or `..., but has no visible effect while terminal opacity is 1;
lower opacity to see it` at opacity 1. When support is unknown the text reads
`although the compositor's blur support is unknown` instead of naming the
protocol.

Observed blur outcomes for this change, from the ignored
`native_graphical_blur_reports_platform_outcome` test run on Linux under sway
1.12 (wlroots), whose Wayland registry offers neither
`org_kde_kwin_blur_manager` nor `ext_background_effect_manager_v1`, with this
build. Each run started with `blur true`, then saved an unchanged `blur true`, a
`blur false` and a `blur true` again:

- Wayland under sway, at `opacity 1` and `opacity 0.6`: the Wayland unsupported
  warning, once; the unchanged reload and disabling logged nothing, and
  re-enabling logged the same warning again.
- Xwayland under sway (`WAYLAND_DISPLAY` unset), at `opacity 1` and
  `opacity 0.6`: the X11 unsupported warning, with the same reload behaviour.
- KDE Plasma (Wayland) and KWin on X11: not verified; no such host was
  available. From the source, KDE Plasma Wayland logs `terminal blur is
  requested through org_kde_kwin_blur_manager` at opacity below 1 and the
  no-visible-effect line at opacity 1; KWin on X11 reports the X11 warning.
- macOS: not verified, and not compiled, in this change. From the source, it
  logs `terminal blur is requested through macOS window background blur` at
  opacity below 1 and the no-visible-effect line at opacity 1.

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
the pinned source's rounding, including at fractional scales. Each padding side
converts to physical pixels on its own, as `round(logical × scale)`. Left plus
right padding is subtracted from the width, and top plus bottom from the height,
before floor-dividing by the physical cell dimensions. The grid is at least one
row/column (at most 4096). A minimized zero-size window defers geometry updates.
The grid origin is the left and top padding, plus the balancing offset when
enabled. Drawing, the IME cursor area, the switch notice, pointer-to-cell
conversion for mouse reports, selection and link clicks, and the window
observation log all use that one origin. A pointer inside the padding maps to the
nearest edge cell. Cursor/selection rectangles, hit testing, graphics and reported
cell/text-area pixels all use these same cells. Text-area pixels exclude padding
and unused edge pixels: they equal rows/columns times cell height/width.
Gestures inside the padding belong to a separate issue.

Font settings reload automatically with the session's configuration updates,
including drop-ins. A `config.d` directory first created while the session is
running is discovered during the normal polling/debounce cycle.

Changing `cell_width`, `line_height`, `baseline_offset`, `underline_offset`,
`underline_thickness`, `ligatures`, `font_ligatures`, `font_features`, effective
padding (`padding`, `padding_top`, `padding_right`, `padding_bottom`,
`padding_left`), `padding_balance`, effective `padding_color`, effective
`minimum_contrast`, effective `opacity`, effective `opacity_mode` or effective
`blur` with `action reconfigure` requires
`--write`, or an edit to the session's saved configuration/drop-in. An unsaved
change to any effective value refuses the entire fragment, including sibling
settings, for all requested clients, with save guidance. Equivalent values and
omissions remain valid runtime requests; explicit metric `1.0` equals omission
when the effective value is already the default. For `font_features`, omission and
a bare node are both no features and `"calt"` equals `"+calt"`; a reordered list
counts as a change. Padding compares the resolved sides and balance, so
`padding_left 4` with a current `padding 4` is equivalent; `padding_color
"background"` equals omission, and so do `minimum_contrast 1`, `opacity 1`,
`opacity_mode "background"` and `blur false`. The refusal names the
key that caused each change: `padding` when the shorthand changed a side, or the
side's own key. Before this change, unsaved padding was accepted but never
reached native windows, which read padding only from files.
`cursor_unfocused_hollow` also requires `--write` or a saved edit. It is compared
with the session's effective saved files, where omission means `true`, so an
explicit `true` against an unset saved value is equivalent. An invalid fragment, with or without `--write`, is refused with
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
structured frame. While you wait, the notice names the destination. If the
source has already shown its first repaint, the notice names it as the session
receiving input. Otherwise, pane input waits for the first repaint. A second
switch request leaves the current attempt in progress. Press Escape without
modifiers to cancel the pending attachment and keep the source open. A
destination session that has already started opening may still be available
later. Use the same switch action to try again.

If the destination fails to open or render, the window shows the error and
keeps the source available. Press Escape to dismiss the error notice.

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
and paint-only changes retain font caches. Padding and balance changes update
geometry and the grid origin without reloading fonts. Reload, resize, zoom and
monitor-scale changes recompute the physical sides and origin, so pointer
targeting follows the drawn cells. An origin-only change, such as toggling
balance or resizing within one cell, sends no cell, text-area or grid report. Zoom and monitor-scale changes use the same path; final raster
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
Set either or both startup dimensions in the host configuration or a drop-in.

```kdl
terminal {
    initial_columns 100
    initial_rows 30
}
```

Each setting accepts a whole number from 2 to 1000 cells. For each axis, an
explicit `--cols` or `--rows` takes precedence, followed by the cached physical
size in `remember` mode, the configured cell count, and the default of 120
columns or 40 rows. A missing or invalid cache falls back to the configured
size. The CLI startup mode takes precedence over the configured mode.

Windowed startup adds left plus right padding to the requested columns and top
plus bottom padding to the requested rows, at the display's measured scale. Wayland limits the correction to the inner size already granted by the
compositor. At fractional scales Yinz fits cell-based axes to whole logical
pixels and chooses the closest legal physical extent for each remembered axis.
The grant can leave fewer cells than requested, even on a large display. X11
waits briefly for initial window-manager placement before correcting at the
observed scale. Subsequent moves and resizes use the live window geometry.
Maximized and fullscreen extents follow the compositor or window manager.

Very large windowed or remembered requests are reduced to fit drawable and
cell limits. Yinz checks actual opening grants while placement or the correction
is pending and reports a sizing error if a grant exceeds either limit. Session
layouts must first fit the provisional grid of 120 by 40, or the explicit
`--cols` and `--rows` values subject to the graphical cell limit. If Yinz
rejects the initial layout for lack of room, retry with larger explicit
`--rows` or `--cols`. The headless diagnostic uses its explicit flags or 120 by 40.

Save these settings in the host configuration or a drop-in, or use
`yinz action reconfigure --write`. Explicit axis values require a save even
when they match the current values. An axes-only save affects new windows
according to drop-in order and preserves the open window's viewport and content.
A saved fragment with other live settings still applies those settings to the
running session.

Run the desktop fixture at an observed 1× scale and again at 2×. It reports the
granted extent, scale, cell metrics, per-side physical padding, balance, grid
origin and viewport containing the pane marker for each mode. Its configuration
uses asymmetric, balanced padding. The test checks each side's rounding at the
observed scale, and checks that the observed grid and origin match the fit
formula above. In windowed mode, once the grid settles, it rewrites the
configuration to move the top padding and turn balancing off. It then expects the
reload to move the origin without changing the grid. The test checks Wayland's configured state and reads `_NET_WM_STATE`
with `xprop` on X11. Confirm the mode independently in the desktop when that
observation is unavailable, including on macOS.

```sh
cargo x test -p yinz --test native_terminal --run-ignored only --no-tests fail --no-capture -E 'test(=native_graphical_startup_fits_configured_grid)'
```

For exact-grid checks, arrange for floating windows with enough granted space
for the configured cells and use a desktop that honors the requested modes.
Set `YINZ_NATIVE_TEST_FLOATING=1` for that run. Windowed should settle at 100 by
30. Remember should fit the saved 800 physical pixels in width, with the explicit
30 rows checked independently. A tiling grant or a mode the desktop declines
is reported as limited desktop coverage.

The fixture does not move windows between outputs or send pointer input, so check
padding by hand for those cases. Use asymmetric, balanced padding (the fixture's
`padding 4; padding_left 6; padding_top 3; padding_balance true`). Move the window
between outputs with different scales. After each move, the logged observation's
sides should match the new scale and its origin should match the fit formula.
Content must not cross the far padding. Then run a mouse-reporting program such as
`printf '\033[?1003h'; cat -v` and click cell centres and points just inside each
cell edge near all four sides. Shift-drag a selection too. The reported cells and
the highlight should match the cells under the pointer. Repeat after a balance
reload and after a zoom change.

Issue #110 padding observation (2026-11-22): this worktree's debug binary ran the
desktop fixture on Linux Wayland, on one 5120×2880 output at scale 2. The
compositor tiled the windows, so exact floating fit was not checked. The run
passed with these observations, given as left/top/right/bottom sides:

| Mode | Extent | Sides | Balance | Origin | Grid |
|---|---|---|---|---|---|
| Remember, maximized | 5088×2780 | 12/6/8/8 | on | 18, 9 | 316×69 |
| Fullscreen | 5120×2880 | 12/6/8/8 | on | 18, 19 | 318×71 |
| Windowed, after reload | 5088×2780 | 12/2/8/8 | off | 12, 2 | 316×69 |

The windowed reload moved the origin without changing the grid. The desktop did
not apply maximized mode. Mixed-DPI moves, the manual pointer checks above, X11
and macOS were not exercised.

`cursor_style` accepts `block`, `bar`, and `underline`. `cursor_trail` enables (`true`, the default) or disables (`false`) the swept comet trail drawn behind the cursor when it jumps at least two cells in the native terminal window. `macos_option_as_alt` accepts `none`, `left`, `right`,
and `both`.

`cursor_unfocused_hollow` takes a plain boolean, `true` by default (the pinned
source's default and the earlier always-hollow behaviour). While the window is
unfocused, `true` draws an outlined block that does not blink, whatever
`cursor_style`, `cursor_blink` or the program requests. The outline leaves the
cell's colours alone and its stroke follows the physical cell metrics, so it
scales with HiDPI. `false` keeps the configured `cursor_style` and `cursor_blink`
while unfocused, or the program's requested shape and blink when those are unset.
Regaining focus restores them. A hidden cursor stays hidden in every combination,
and the composed session state still decides which pane or overlay owns the
cursor, including IME preedit. Other values, typed values, extra arguments,
children and duplicates are rejected at the node, as for `cursor_trail`.
Startup reads the setting with the rest of the terminal block, and a later
drop-in wins. A file reload applies it to the cursor only: when the effective
cursor changes, only the cursor row is rebuilt, without font, cache or geometry
work. A focus change does the same, and a repeated focus event does nothing.
Content and selection are preserved.

Issue #115 observations (2026-11-24): this worktree's debug binary ran in
bounded foreground headless Sway 1.12 sessions (GLES2 renderer) on a
1200×600 output at scale 2. A second empty output received focus, and a
scratch `zwp_virtual_keyboard_v1` client gave the headless seat a keyboard, so
the window got real keyboard enter/leave events. Without that keyboard Sway
reported the window unfocused, but the client got no focus event and kept its
configured cursor. Captures with embedded Iosevka Term at 12 points showed:

| Case | Wayland (scale 2) | X11 (Xwayland) |
|---|---|---|
| Default, unfocused, bar/underline/block | Outline, 2-pixel stroke on a 16-pixel cell | Outline |
| Focus regained | Configured bar/underline/block | Same |
| `false`, unfocused | Configured bar/underline/block | Same |
| `false` with `cursor_blink true`, unfocused | Bar blinks | Same |
| Default with `cursor_blink true`, unfocused | Outline, no blinking over 3 captures | Same |
| Drop-in reload to `false` while focused, then unfocus | Bar | Same |
| Drop-in reload to `true` while unfocused | Outline immediately | Same |
| Invalid drop-in (`cursor_unfocused_hollow 1`) | Drop-in skipped, main file applies | Same |

The session's own desktop output, an active selection under the cursor (no
pointer device in the headless seat; selection is covered by the window tests),
mixed-DPI moves and macOS were not exercised.

Use Ctrl+Shift+C/V (Command+C/V on macOS) for copy/paste. Hold Shift while
dragging to select locally when the session handles mouse events. Ctrl-click
opens a hyperlink. The copy shortcut copies that local selection. The middle
button pastes clipboard text when enabled. Ctrl+Shift+plus/minus/0 or
Command+plus/minus/0 changes or resets font zoom. Clipboard writes work; host
queries do not receive clipboard contents.

## Structured rendering protocol

An interactive attachment starts in ANSI mode. The server advertises negotiation
revision 1 in optional field 9 of the attachment's own `Connected` message.
A probe on a separate socket gives no authority to send an offer. On revision 1
the native client offers its frame versions, feature mask and image media. The
server sends its decision before the first full native repaint. Ordinary controls
can precede either message. Graphical startup and switching process controls in
order while waiting. A new window displays a waiting notice and accepts pane
input after applying its first repaint. A staged window promotes the destination
after applying its first repaint. While waiting, the window answers host queries
on the destination connection. A failed opening leaves the source window available.

When both peers support attachment generations, the opening `Connected` also
advertises a 16-byte generation. The viewer echoes it on its offer. The server
tags the decision and subsequent messages, and the viewer tags its ACKs and
input on that same connection. Reattaching to the same session starts a new
generation. The viewer discards messages for a different generation before
applying a frame, changing the window or answering a host query. During a
switch, the destination confirms its first repaint on its own socket. If the
destination fails, the source remains available. Peers that omit the optional
generation continue to use the existing revision-1 exchange.

The server discards a message tagged with another generation before decoding
it, whatever its kind. A previous attachment's late ACK, resize, withdrawal or
exit therefore cannot release the replacement's media, open its transfer window,
change its grid or detach it. Messages on the old connection affect only the old
attachment: its ACKs release its own media, and closing it releases the rest.
A replacement starts its own transfers from the first chunk under its own ids.

When an ANSI client switches sessions or recovers its server connection, the
host terminal may still owe a reply to the previous connection. The client waits for the
old Primary DA barrier or OSC 52 clipboard reply before sending a new query
of the same kind. A request that reaches its deadline while waiting uses the
usual timeout fallback. Host theme, color and pixel-size reports still update
the client while it waits.

When a color report crosses the first repaint, the terminal and the keyboard
share one byte stream. The client gives digits that extend a legal RGB or hex
color to the report. If the terminal sent `#abc` before repaint and then sends
`def`, typing `z` before BEL applies `#abcdef` and delivers only `z`. Typing
all of `defz` produces the same result. A digit beyond a legal hex width goes
to the pane if it arrived after repaint. For example, `#abc` followed by `d`
and BEL applies `#abc` and types `d`.

A key that interrupts a complete value ends its color digits. With `#abc`
followed by `z12` and BEL, the pane receives `z12`. An incomplete value can
resume after a key. With `#abcd` followed by those same bytes, the pane
receives `z` and the color becomes `#abcd12`. Digits can also be lost to an
unfinished RGB channel. With `rgb:1111/` followed by `z12!` and BEL, the
pane receives `z!` and the previous color stays in effect.

While the color report is open, Escape can begin the terminal's two-byte
string terminator (Escape followed by backslash). The client holds it across
idle deadlines, even after Enter. A backslash finishes the report without
sending a cancellation to the pane. A different next byte distinguishes the
Escape from a terminator. A complete arrow key or encoded Escape can therefore
be used while the report is open. Pressing Escape twice also distinguishes
the first press, which reaches the pane on its usual deadline. A lone Escape
pressed after the report ends uses that deadline as well.

If a source clipboard reply is open at handoff, the client discards bytes
that could belong to that reply until BEL or Escape followed by backslash.
This includes letters typed on the replacement, even after Enter. Enter,
Ctrl-C, spaces and complete arrow keys reach the keyboard handler while the
client continues draining the old reply. A terminal string or bracketed paste
can carry its own terminator. That terminator leaves the old query pending.
Inside a paste, the client discards bytes that fit the unfinished clipboard
field. Once clipboard data has started, pasted semicolons and question marks
reach the pane. A color or graphics probe reply that spans the replacement's
first repaint still updates that attachment.
The old reply's own terminator releases the next waiting clipboard query, and
typing after it works normally. If the terminal never sends the terminator,
ambiguous typing continues to be discarded and waiting clipboard queries can
time out.

On a legacy terminal, Alt+Shift+P can send the same bytes as the start of a
device control string (DCS). If this happens during the clipboard drain, even
a following BEL, Enter or Ctrl-C can belong to that string. The client waits
up to two seconds from the ambiguous prefix for the string terminator and
then the clipboard terminator. If both arrive, typing and clipboard queries
resume. Otherwise the client detaches, even while a frame is waiting on a
full terminal. Before restoring the terminal, it discards terminal input until
none arrives for half a second, for at most five seconds, then flushes the
terminal's input queue. The rest of the clipboard reply therefore does not
reach the shell that gets the terminal back. Keys typed during that interval
are discarded as well. The client then restores the terminal and displays a
diagnostic. The session stays available to reattach from a fresh terminal.

| Client and server | Attachment behavior |
|---|---|
| ANSI client with either server | Ordinary ANSI render messages (server tag 1), including when other clients use native frames. |
| Older boolean native client with revision-1 server | Legacy support (client tag 42) selects the v4 legacy profile with inline images. No negotiation result is sent. False returns to ANSI. True can resume the same profile with a full repaint and the next sequence. A revision-1 offer requires a fresh attachment once the legacy profile has been used. |
| Revision-1 native client with an older or silent server | Missing advertisement on Connected or an opening timeout ends native startup with endpoint-specific recovery guidance. |
| Revision-1 native client with an unfamiliar advertised revision, including zero | Reports the received and supported revisions and stops before sending an offer. |
| Revision-1 endpoints with no common frame version | An explicit rejection lists the first eight offered versions, the count of any remaining entries, and the writable versions. An empty offer appears as `[]`. If the server cannot deliver a negotiation result, it closes that attachment. Other attachments keep their profiles. |
| Revision-1 endpoints with a common version | Client request tag 45 and server result tag 36 establish a profile. The receiver checks the selection against its offer and proof. Frames use server tag 35 and cumulative ACKs use client tag 43. |

Use `yinz attach NAME` locally or `yinz attach --remote HOST NAME` for SSH.
If the forward has no host label, substitute the host used to establish it.
The native library can consume an already-established forward. The native CLI
opens local sessions. Opening another remote destination requires a fresh forward,
including when a local session happens to have the requested name. The source
remains available for input and subsequent frames.

| Forwarded AttachClient fields | Values |
|---|---|
| Terminal assets | Reported size, host-terminal environment, timezone and requested tab/pane focus. |
| Source retention | True for graphical switching, false for headless capture. |
| Session requests, in-place update, source update, checked native layout | False. Update and bootstrap fields are empty. |
| Config, config directory, clean, layout, data directory, maximum panes, cwd, debug and other local options | Wire defaults. The destination server supplies its session settings. |

### Frame features and image media

The server selects the highest version it can write that the client can read.
Today's server writes v4. Native clients read v2, v3 and v4. Only selected
features and media can appear in a live stream. Unknown offer bits are ignored.
Each attachment has its own profile. A local shared viewer, a forwarded portable
viewer and an ANSI client can watch the same pane at once, and each receives
images in its own medium.

| Feature bit | Meaning and current production behavior |
|---|---|
| Geometry (1) | Pane bounds locate graphics and constrain clipping to the owning pane. The record lists the visible scene; see [Pane and overlay composition](#pane-and-overlay-composition). |
| Links (2) | Hyperlink table and cell references. A full repaint carries every mapping its cells use; deltas carry only new or retargeted entries. See [Unicode cells, clusters and hyperlinks](#unicode-cells-clusters-and-hyperlinks). Disabled clients receive plain cells. |
| Kitty (4), Sixel (8) | Independently selectable. Both require geometry and inline media. Kitty also follows the server's graphics setting. Text-only clients get cells without graphics placeholders. |
| Row shifts (16) | Move existing cells within the current grid. The reader can apply shifts. The writer currently sends ordinary cell deltas. |
| Scroll metadata (32) | Signed lines for a pane indexed in the preceding applicable geometry. Requires v3 or later and geometry. A producer must establish geometry first and omit hints on geometry replacement or reset. A consumer must resolve the index before applying the hint. Current live peers omit this feature. |

Inline (media bit 1) carries image bytes on the connection and works across a
forward. Shared (bit 2) is an optional addition for Kitty RGBA on a proven local
attachment. A local viewer creates a private file in its canonical temporary
directory with two independent random tokens. It offers the directory, generated
basename and first token. The server checks the file under its own pinned
directory and returns the second token. Both peers validate the proof. A failed
probe selects portable output. A forwarded client always offers portable media.

Chunked (bit 4) splits portable Kitty RGBA into transfer chunks carried by
ordinary frames and paced by ACKs (see
[Pacing, budgets and cleanup](#pacing-budgets-and-cleanup)). Every revision-1
reader offers it, and the server selects it only with Kitty. It is optional
on acceptance, as Shared is. A reader accepts an agreement without it, so an
older server keeps sending inline images to a newer reader. An older reader
never offers the bit, so it is never sent chunks. Chunk, cancel and transfer
residency records appear only on attachments that agreed Chunked, and the
reader fails a frame that carries them otherwise. Portable frames never carry
a usable server path: the reader refuses a shared-file residency unless Shared
was agreed.

For an agreed shared transfer the server stages a Kitty upload of at least
256 KiB as a file when the viewer's media budget has room. A chunked viewer
without shared media receives such an upload as a chunked transfer. Smaller
uploads travel inline. For a chunked viewer they travel inline only while they
fit the frame's media allowance; the rest become transfers. Viewers with
neither shared nor chunked media receive inline bytes. The reader requires a generated image basename directly
beneath its pinned directory. It opens without following symlinks and verifies
ownership, private mode, regular-file type and exact RGBA length before copying.
A failing live read fails the frame. A placement needs an owned image and a
resolvable pane anchor. A palette image crossing pane boundaries uses a text
meter. Image and placement identifiers are unsigned 64-bit values throughout the live
scene. Row shifts apply to fully contained rectangles on incremental frames of
the same grid size. A clear, full repaint or resize starts a new baseline.
Media failure requires a new attachment and full repaint.

### Pane and overlay composition

A native frame carries the cells an ANSI client of the same tab receives. These
are the tiled, floating and overlay panes, with toasts, menus, the command
palette and the permission modal drawn over them. Translucent overlays blend the
same way. A clipped or occluded cell appears in neither output.

The geometry record lists every visible element of that scene, bottom to top:

1. Tiled panes, in pane-id order. A tiled fullscreen pane appears alone. Each
   member of a classic stack has a rect, so a collapsed member's one-row title
   has its own rect with no content rows.
2. Floating panes, in stacking order. All of them appear while the floating
   layer is shown, and only pinned ones while it is hidden. A floating
   fullscreen pane covers the others.
3. Overlay panes.
4. Screen overlays as popups, in paint order: toasts or the notification
   browser, the notification return cue, the context menu, the command palette,
   toasts over a palette that owns input, and the permission modal.

A hidden pane appears in neither the cells nor the geometry. Hidden panes
include those behind a tiled fullscreen pane, an inactive pane tab, a member
the stacked pane list does not show, and an unpinned float while its layer is
hidden.

| Flag | Meaning for viewers |
|---|---|
| `PANE_FRAMED` | The pane has a frame or title inset around its content. |
| `PANE_FOCUSED` | This client's focused pane. On a popup: the overlay takes this client's input. The palette, context menu, permission modal and notification browser do; toasts do not. |
| `PANE_FLOATING` | A floating or overlay pane, which can be moved and resized. |
| `PANE_UNDER_FLOATING` | A tiled pane while the floating layer is shown with at least one pane. Pinned floats left up while the layer is hidden do not set it. Viewers treat these panes as covered: their pointer is inert and a selection is not remapped into them. |
| `PANE_POPUP` | A screen overlay. Its insets are zero, so its border belongs to the popup rather than to the pane beneath. A sliding toast's rect covers only the columns it has revealed. |
| `PANE_SELECTABLE`, `PANE_WANTS_MOUSE`, `PANE_PLUGIN_UI` | The pane accepts focus, its program has requested mouse reporting, or it is plugin chrome without text selection. |

Popups never own images. A Kitty placement or Sixel anchor beneath a toast or
the palette belongs to the pane under it. It is clipped to that pane and
validated against it. Hiding images beneath overlays is a graphics placement
concern, not an ownership one. Popups coming and going elsewhere leave a local
selection mapped. A toast or menu painted over the selected text hides the
highlight but keeps the copy. A sliding toast changes its rect every frame, so
the viewer rebuilds its scene for the length of the animation.

The frame header carries the cursor that client would see on an ANSI host. It
shows only in the client's focused pane. It is hidden where a floating pane,
toast or context menu covers it. When the palette owns input, its caret
replaces the pane cursor in its input field. The palette hides the cursor when
it has no input field, for example while busy. The permission modal hides the
cursor. A hidden cursor keeps its position and shape for when it shows again.

Each client receives its own frames. Focus flags, the cursor and the
presentation offset are per client. A context menu or palette belongs to the
client that opened it, while toasts reach every client. A viewer larger than
its tab centers it. Cells, geometry, the cursor and image anchors then share
one offset.

The tab follows the smallest attached client. A recipient is smaller than its
tab only while a reflow is pending or refused, for example for a stack that
cannot fit. Cells are then cropped to the recipient's grid and the cursor is
hidden until it is back inside. Rects keep their tab extent, because image
anchors are validated against them.

A full repaint carries the complete cells, geometry and cursor. Full repaints
follow a new attachment, a resize, a presentation change or a display clear.
Reattaching starts a fresh sequence.

### Unicode cells, clusters and hyperlinks

The server's grid emulates one code point per cell, as Zellij does with grapheme
mode (DECSET 2027) off. A combining mark, zero-width joiner, variation selector
or other zero-width code point from U+0300 on (and the soft hyphen) is attached
to the cell before it. A cell keeps at most 32 code points, and later marks are
dropped. A mark with no cell before it on the row is dropped. Kitty placeholder
diacritics stay image coordinates and never attach to a placeholder cell. Code
points with their own width, such as skin-tone modifiers, regional indicators
and the person after a joiner, keep cells of their own. Cell widths, wrapping,
erasing, insertion, deletion and reflow therefore behave exactly as before.

Attached text is kept in a per-screen-thread table. Identical texts share an
entry, entries are reference counted and freed IDs are reused. IDs are 23 bits,
as in Zellij, and the table holds at most 32 MiB. When it is full, new marks are
dropped and a warning is logged once. Copying a selection, screen dumps, pane
contents (plain and with ANSI) and session migration include attached text.
Migration stores the text, not table IDs, in an optional field. The snapshot
format version does not change. Older snapshots restore without marks, and an
older server that receives a newer snapshot ignores the marks. Search and
highlight rules match base characters only. Search also drops the code points
the grid would attach (zero width from U+0300 on, and the soft hyphen) from
the typed or pasted search text, so a copied selection finds the text it was
copied from, and `cafe` and `cafe` + U+0301 find the same cells. A search
text made only of such code points matches nothing.

Frames group the cells of one grapheme cluster when serialized. A group is the
run that Unicode's grapheme rules join, of at most 32 cells or 255 columns. It
is split where the effective style changes, including a selection or highlight
edge. In a native frame a cell whose text has more than one code point carries a
cluster ID from that frame's cluster record. Each group member after the first
has `ATTR_GROUP_CONTINUE`; wide continuation cells never set it. The per-frame
record holds entries of at most 4 KiB and at most 1 MiB in total. Past either
limit a cell carries only its first code point, and the frame stays valid. A
narrow cell's VS16 is dropped so the glyph keeps its one column. Built-in UI
text (pane tabs, notifications, modals) gets the same clusters as pane content,
because a width-0 entry in a composed chunk joins the glyph before it.

ANSI clients receive the same units. Text whose widths all agree, including a
letter with combining marks, is written as is. A group, or a cell whose width
a host could draw differently, is written as an erase of its columns (`ECH`),
its text, then an absolute cursor position (`CUP`) after its last column. So
does every piece of a cluster that a style, selection or group-size edge
splits, because a terminal that joins graphemes draws those pieces as one
glyph. Later cells stay on their columns with or without mode 2027, and
centered clients translate the `CUP` like every other cursor move. A cropped
client, watcher or native viewer never receives part of a unit. The visible
columns where a cluster or wide character does not fit are blanked with its
background, without its link or other attributes.

Each native viewer has a table of at most 255 hyperlink IDs. A link's identity
is the program's explicit OSC 8 `id=` together with its URI, or the URI alone
when the program gave no ID. Two explicit IDs for one URI are distinct links;
detected URLs are keyed by URI. A URI longer than 65535 bytes is sent as a plain
cell. The full mapping works as follows:

- A full repaint clears the receiver's table together with its cells, and the
  frame carries an entry for every ID its cells use.
- A delta carries only IDs allocated or retargeted in that frame. The receiver
  merges them into its table.
- The server shadows the ID shown in every viewport cell. Before assigning IDs
  for a frame it releases every cell the frame writes, including edge blanks.
  When the table is full, it retargets only an ID that no shadowed cell shows and
  that no row emitted earlier in the same frame uses. With none free, the cell
  is sent without a link.

The scrolling and replay work that adds row shifts must shift this shadow with
the cells (as Zellij's `LinkTable::shift` does) before it releases written
cells. Otherwise a shifted cell keeps an ID the server considers free.

### Pacing, budgets and cleanup

Frames are paced by the writer: each client has a bounded send queue of 5000
messages and 64 MiB of render allocations, and the screen defers rendering a
tab while every recipient's writer is behind. A slow viewer among healthy ones
is not waited for. The native client applies frames through a bounded queue,
so slow application backs up its socket. There is no one-frame-in-flight
window.

Shared and chunked media are paced by ACKs. Every negotiated viewer has its own
media budget: at most 32 retained entries and 128 MiB, including any withdrawn
or sealed prefix that viewer still holds. Staging reserves from the budget
before creating a file or copying a transfer buffer. A refused reservation
creates neither. Budgets are not shared, so a stalled viewer can never delay
another viewer's images. Any image up to the 100 MiB shared limit fits a
viewer with nothing outstanding.

When the budget is exhausted, the image is deferred. That frame omits the
image's residency and placement and takes down any placement that showed
another image at the same position. Cells, links, cursor, geometry, sideband
and deletes are still delivered in order. The server logs one warning per
overload episode. When the viewer's next ACK releases media, the screen
re-renders the deferred panes and the image arrives as a shared file or
transfer. A transfer abandoned before its first chunk returns its share at
once, and the next ACK re-renders the deferred panes as if it had released
media. A budget refusal never falls back to inline bytes. An image above
the 100 MiB shared limit is dropped with a warning. A chunked viewer whose
file write fails receives a transfer instead. Inline fallback remains only for
viewers with neither shared nor chunked media, images below 256 KiB and a
failed file write without chunked media.

A chunked transfer holds one lease for its whole image. One frame carries at
most 8 MiB of media to a chunked viewer, counting inline residencies and chunk
bytes, so a 100 MiB image takes 13 frames. A transfer that spans frames
sends new chunk bytes only after the viewer ACKs the last frame that carried
chunks, or that ran out of allowance for them. At most one such frame is
unacknowledged. Throughput for those transfers is about 8 MiB per round trip.
A transfer that fits what remains of the frame's allowance is sent whole in
that frame without waiting, as long as whole transfers in unacknowledged frames
stay within one more 8 MiB allowance. A program that redraws an image of up to
8 MiB faster than the round trip therefore sees replacements arrive in the
frame that deletes the image they replace, without a blank gap, until that
allowance is spent. Later replacements wait for the ACK of the last chunk
frame, like a transfer that spans frames, and their position stays empty
meanwhile. Each lease counts against the viewer's budget until its frame's
ACK. Unacknowledged chunk bytes therefore stay within two allowances, well
below the writer's 64 MiB limit, even for a viewer that falls behind other
viewers of its tab. A placement whose transfer
is incomplete is withheld like a deferred one, without a warning, and the ACK
that opens the window re-renders its pane. A larger replacement leaves its
position empty until its last chunk arrives. The frame that
carries the last chunk also carries the residency and placement. The server
keeps its copy of the pixels until the last chunk, then only the lease, which
accounts for the client's assembly.

A viewer that reads frames but never ACKs keeps at most its budget and sees
later large images deferred. No ACK deadline applies outside a sealed drain. A
viewer that stops reading while another viewer of its tab stays current keeps
receiving queued frames until its writer limit disconnects it. When every viewer of a
tab is behind, including a sole viewer that stops reading, the screen defers
rendering that tab. Such a viewer neither reaches the writer limit nor stages
more media, and keeps what it holds until it resumes or closes.

The server classifies each ACK before the screen sees it. A future ACK, above
the last written frame or the sealed high mark, is rejected and releases
nothing. Duplicate and stale ACKs are accepted and release nothing more. An
ACK carrying another attachment's generation is ignored, so a reused client
number never releases the current attachment's files.

Staged files are unlinked when their consuming frame is ACKed, and otherwise
when the viewer detaches, its peer disappears, a write to it fails, it
withdraws and then ACKs or closes, its sealed drain is abandoned, or the
session exits. Each file returns its share of the budget only after it is
unlinked. Transfer leases are released on the same occasions. Turning Kitty
support off for a viewer, or a frame that clears its images, also abandons its
pending transfers.

An attach that leaves a tab's size and pane-frame style unchanged does not
clear the displays of its existing viewers, so their retained images and
in-flight transfers continue. A frame that clears starts a new baseline and
sends the visible images again. Resizes and changes to the layout or pane-frame
style produce such frames. An attach can cause them too: a smaller arriving
client shrinks the tab it joins, and an arriving client whose configuration
sets another pane-frame style applies it to every tab.

Limitations: a crash, `SIGKILL` or panic exit can leave `yinz-native-image-*`
files in the temporary directory. Budgets are per viewer, so the total grows
with the number of attached shared or chunked viewers. For a peer without
chunked media, a frame carrying more than 64 MiB of sub-threshold inline images
still ends native output as described below.

`yinz_runtime::native_media::MediaBudget` reserves `MediaLease`s, and each
lease returns its entry and bytes when dropped
(`NativeRenderState::media_budget`).

### Sequence, withdrawal and oversized frames

Frames belong to one attachment and arrive in order, beginning with a full
repaint. The viewer ACKs a sequence only after successfully applying every
covered frame and copying all data needed later. For a residency copied in
frame R, ACK(R) or a later contiguous ACK releases the staging file, even if the
GPU draws later. ACK of an earlier frame, receipt, socket write, queueing,
recording and header decoding alone are insufficient.

A chunked transfer staged in frame T takes its wire id when its first chunk is
emitted, so first chunks carry strictly increasing ids within an attachment.
Ids need not be contiguous. A transfer abandoned before its first chunk (it is
replaced, hidden, cleared, cancelled, withdrawn or sealed) leaves no record and
returns its lease at once. A started transfer is completed in frame C, which
carries the last chunk followed by its `Transfer` residency, or abandoned by a
`TransferCancel` naming its id, or by a frame that clears. Its lease is released
by ACK(C), or by the ACK of the cancelling or clearing frame. ACK(T) alone
releases nothing.

The reader validates every transfer before allocating. A new id must exceed
every id started on the attachment and begin at offset 0, with a total of at
most 100 MiB. At most 32 transfers may be incomplete, with at most 128 MiB of
declared totals. Each chunk continues at exactly the received length and stays
within its total. A cancel names an incomplete transfer. A `Transfer` residency
names a transfer completed in the same frame whose length matches the RGBA
dimensions. A transfer completed but not consumed by its frame, or any
violation, fails the live frame. Incomplete transfers are discarded at the
start of a frame that clears or fully repaints, and on a new attachment, so a
late or reused id cannot complete.

Duplicate frames have no repeated side effects. Duplicate and stale ACKs are
idempotent. Future ACKs cannot release files. A missing predecessor or failed
consuming frame ends the stream at the last successfully applied sequence.
Reattach for a fresh full repaint. ACKs never cross attachments. Chunk records
carry no attachment generation of their own: their envelopes are generation
tagged, and both peers keep transfer state per attachment.

A revision-1 withdrawal stops production at the screen transition. The ordered
Withdrawn result names the last emitted sequence H. The client can apply and ACK
the in-flight prefix through H or close and abandon it. The server retains that
prefix's media until application is acknowledged or the connection closes.
Started transfers that have not completed are retained at H; unstarted ones are
released at the withdrawal.
A different native profile requires a fresh attachment. A duplicate offer
cannot reset the stream.

An individual frame allocation above the writer's 64 MiB limit, or an encoded
protobuf envelope above the 64 MiB IPC limit, ends native output to that viewer.
The writer delivers the preceding writable FIFO prefix, then an ordered error
and write-side EOF. The read half remains available for cumulative ACKs and
client exit. A pre-write envelope refusal can shorten an already sealed prefix.
An actual partial-write or socket failure abandons the broken stream. A drain
with no socket-write or advancing ACK progress for 30 seconds is abandoned.
Local socket progress can include bytes buffered by a forward. Only the
receiver's application ACK releases media during an orderly drain. Other
viewers continue receiving output. Chunked media keeps every frame far below
these limits. A frame can still exceed them through cells alone, such as an
explicit-margin presentation for a very large terminal, or through Sixel, which
stays inline as a cropped visible slice. Peers without chunked media remain
subject to them for large inline images.

## Diagnostics

Native windows log configuration fallbacks to the client log
(`$TMPDIR/yinz-<UID>/yinz-log/yinz.log`), including the opacity warning when the
window must stay opaque and the `terminal blur` warnings when the backend cannot
blur or the window is opaque; see the opacity and blur sections above.

The hidden `--headless` option exercises the same session and structured frame
path without opening a window. `--rows`, `--cols`, and `--duration-secs` bound a
capture. `--record PATH` also works with graphical windows. Both modes open the
recording for replacement after applying the first repaint. A local graphical
window checks the recording destination before opening the session. To record
to a named pipe, start its reader before the first repaint. If the pipe has no
reader at that point, the capture exits with an error naming the pipe. A duration
limit, normal detach or host-initiated disconnect before the first repaint
produces a nonzero exit and names the untouched recording path. The message
identifies a host-initiated disconnect. If a forwarded attachment loses its
server connection and receives a disconnect reason, its recovery command
addresses the remote session.
A headless session switch ends the capture with a nonzero exit and reports the
source, requested destination and last applied frame, or that the switch
preceded the first repaint.

```sh
yinz terminal demo --create --headless --duration-secs 5 --record demo.frames
```

A recording contains repeated little-endian u32 frame lengths followed by frame
bytes. Decode each frame with `yinz_types::structured_render::decode`. Inline
and chunked images are replayable from the recording: a capture contains the
chunk, cancel and transfer residency records, and replay assembles them with
the reader's rules, reporting and skipping what a live reader would refuse. A
raw capture of shared media records its temporary pathname. The referenced file can disappear after its consuming
frame is acknowledged or its attachment closes.

Windows, remote web attachment, tray/background services, desktop session/inbox
panels, and application registration are outside this first port.

## Upstream source

The rendering, font, input, frame codec, and graphics-consumer code is adapted
from the Zellij checkout at commit
`c8585459c37186fae1321727ad8dd4c96480ef41`. The upstream MIT license and individual
font licenses are retained under `yinz-native-terminal/`. Session startup,
configuration integration, and structured frame production use Yinz's existing
crate boundaries and IPC transport.
