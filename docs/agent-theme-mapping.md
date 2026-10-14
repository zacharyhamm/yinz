# Managed agent colors

`agent_theme_mapping "semantic"` is the default. Managed Claude and Codex
launches capture their source palette on the execution host, after environment
and working-directory setup. Reporting hooks can be disabled independently.
Use `agent_theme_mapping "off"` to restore the application colors immediately.
Theme changes, previews, cancellation, selection, and translucent panes use the
same rendering path. Original terminal cells and capture data are unchanged.

The snapshot belongs to one run. Restart/resume captures again; changing an
agent's own theme while it runs is outside this feature. Existing scrollback
keeps its original snapshot. Supervisors retain optional versioned metadata for
adoption. Managed launches from existing shell panes use the same lifecycle as dedicated
agent panes. Unmanaged programs and shell output between launches have no palette
mapping. Piped or redirected stdout is passed through unchanged: palette probes,
retention, and terminal announcements are skipped entirely.
Resolution errors never prevent a launch. The launcher supervises the provider,
preserving exit status, signals, and foreground terminal stop/resume behavior.

Start/end events carry protocol version 1 and a unique launch ID. Duplicate starts
are idempotent; a stale end cannot clear a newer launch. Matching end and dedicated
pane teardown clear active metadata. PTY events work over local and remote sessions;
supervisor retention is best effort.

Palette ownership is stored in compact column spans alongside the original cells.
Writes and erasures take the active palette; untouched text, scrolling, reflow,
and alternate screens retain their associations. Ordinary ANSI captures contain
original SGR and no palette metadata. Pane and tab warps include optional version 1
metadata with a deduplicated snapshot table, associations to the serialized ANSI
bytes, and the active launch. Content and metadata share the transfer budget and
are trimmed together. Historical ownership is restored before the current launch
and buffered output. Missing metadata uses legacy replay; invalid metadata is
ignored.

## Supported descriptors

- Claude Code **2.1.296**: six built-in presets, custom JSON inheritance, RGB,
  ANSI names and ANSI-256 overrides. Resolves the invocation's config root,
  legacy preferences, user/project/local settings, `--setting-sources`,
  `--settings`, and the platform managed settings file. Cached remote policy,
  managed settings directories, host-managed/cowork configuration, inherited
  repository-local theme settings requiring ownership checks, and invocation
  flags that change config or the working directory inside Claude produce an
  unavailable result. The built-in code/diff renderer is covered, including
  its separate syntax palettes and RGB-to-256 conversion. Automatic mode includes
  both light and dark palettes. Other highlighters and dynamic gradients remain
  outside this descriptor.

  Static labels, mascot foreground, informational accents, spinner palette endpoints
  and usage indicators follow accent/subtle roles. Diff-summary
  foregrounds and code-renderer gutters use success/error. Line, dimmed and word
  backgrounds retain their respective diff roles, including custom diff overrides.

  Code text and punctuation use foreground, comments use muted, and keywords,
  operators, types and identifiers use accent. Strings and regex use success.
  Numbers and literals use warning. Attributes such as bold and dim remain intact.
  Shared source colors follow the collision rules below. Independent subagent and
  rainbow colors pass through unless they share a mapped source color.
- Codex CLI **0.162.1**: the pinned provider's read-only `config/read` API resolves
  machine policy, trusted project layers, environment roots and CLI TOML
  overrides. The pinned v2 profile file's theme is inserted below enabled
  project and session layers. Profiles changing discovery/policy are unavailable.
  The two-face 0.5.1 palettes, six model palettes, custom `.tmTheme` scopes and
  fixed status/diff colors are supported. Successful foreground/background
  terminal-query replies enrich that launch's palette with composer/plan and
  submitted-prompt surfaces, secondary text, table separators, warnings, accents,
  and selection colors. The derivations match the provider's blending, contrast
  correction and ANSI-256 quantization. Automatic syntax selection, decorative
  animations and analytics-specific palettes remain outside this descriptor.

Codex observations use the colors actually returned to the application, whether
answered in the pane, forwarded from the host, or supplied by a successful cached
fallback. Both foreground and background are required for derived colors. Missing
or invalid replies leave dependent styles unchanged. Replies from an older launch
cannot enrich a newer launch. The first samples stay fixed through theme changes
and previews; Yinz recompiles only their semantic destinations. Existing cells,
blank fills and completed frames share the enriched palette, so the editor repaints
without application output or a restart. Handoffs preserve observations and derived
definitions in the existing snapshot format, including partial observations.
Local supervisors also retain these enriched snapshots for crash recovery via
`pty-host adopt`. Updates apply only to the active launch, and duplicate startup
reports cannot overwrite observations. Selected secondary text uses the theme's
selection foreground and background after resolving Codex's contrast corrections.

Each descriptor applies to every release of its provider, identified by the
product name in `--version` output. Later releases are assumed to keep the
verified release's configuration loader, theme formats and palettes. Resolution
fails, preserving original colors, only when the current theme cannot be
determined or theme files cannot be parsed. When the running release differs
from the verified one, the partial or unavailable detail names both versions.
A release that changes a palette without changing its format is not detected:
colors it emits that coincide with a descriptor color take that color's role.
A single launch diagnostic records partial/unavailable resolution and color
collisions. Metadata contains normalized colors, semantic expressions, token
names and source provenance, never whole configuration or environment values.

## Matching

Foreground, background and underline channels are independent. Explicit
foreground/background pairs take precedence over individual colors. Equivalent
semantic expressions merge. Claude and Codex prefer mapped roles over unmapped
syntax or decorative tokens sharing the same color. Conflicting mapped roles use
the stable role order (base text, then accent, then status colors). Codex's explicit
accent takes priority. Collisions remain in diagnostics. Other providers leave
equally specific conflicting meanings unchanged. Decisions
compare expressions rather than destination RGB values, so theme switches do
not change collision policy. There is no hue classification or nearby-color
matching. Identical emitted styles cannot distinguish their underlying meaning.
For example, Claude's light code palette shares colors between numbers and types,
so both use accent. Its indexed removal line and word fills can coincide, so both
use the line background. ANSI gutters can share UI accent colors and follow accent.

Normal text/background use the terminal theme (falling back to unselected text).
Muted/subtle text blend 65%/40% foreground over background. Accents use
`text_unselected.emphasis_1`; success/error use `exit_code_success.base` and
`exit_code_error.base`; warnings use `exit_code_error.emphasis_0`. Selection uses
`text_selected.base`/`background`. Raised surfaces blend 8% foreground; diff
backgrounds blend 18% success/error (10% dimmed, 30% word). Shimmer variants blend
25% toward foreground. Bold, italic, underline and reverse remain intact.

## Descriptor provenance and maintenance

Claude palette data was extracted from the installed 2.1.296 Linux binary
(SHA-256 `24972e3bc859fab2b46ed4c1e51f7d6130f06d3bd550811a114640de3370d0de`).
Descriptor revision `palettes-v2` also includes the built-in code/diff renderer in
`chunk-48q0pbgp.js`: palettes `Q`, scope maps `G`/`P`/`he`, override handling `ke`,
and color emission `N`/`fe`. Dark addition fills use explicit indexed alternatives.
ANSI code palettes leave diff backgrounds at terminal default unless overridden.

The shared render fixture contains representative light code/diff SGR, including
plain file output, unchanged lines, gutters, word fills, syntax and indexed colors.
Resolver tests check its definitions and collision results against the descriptor.
New launches capture this expanded descriptor. Relaunch existing Claude sessions
to capture the added mappings. Earlier output retains its original launch snapshot.
The parser follows the [Claude theme reference](https://code.claude.com/docs/en/terminal-config#create-a-custom-theme).

Codex data and behavior follow the official
[`rust-v0.162.1` source](https://github.com/openai/codex/tree/rust-v0.162.1/codex-rs/tui),
particularly `render/highlight.rs`, `render/model_themes.rs`, `style.rs`,
`style/contrast.rs`, `color.rs`, `terminal_palette.rs`, `diff_render.rs`, and the
config loader. Model descriptor JSON contains only scope names and normalized palette values. Dependencies pin the exact syntect
and two-face versions used by this release.

When a provider release changes its loader, emitted colors or theme formats,
add a versioned descriptor, extend resolver/render/launch fixtures and update the
verified version.
Theme files are read only at launch, and lookups compile outside rendering.
