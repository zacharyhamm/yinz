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
  unavailable result. Syntax-only colors and dynamic gradients are outside this
  descriptor.
- Codex CLI **0.162.1**: the pinned provider's read-only `config/read` API resolves
  machine policy, trusted project layers, environment roots and CLI TOML
  overrides. The pinned v2 profile file's theme is inserted below enabled
  project and session layers. Profiles changing discovery/policy are unavailable.
  The two-face 0.5.1 palettes, six model palettes, custom `.tmTheme` scopes and
  fixed status/diff colors are supported. Terminal-dependent automatic syntax
  selection, contrast correction and shaded surfaces produce a partial result:
  only the independently resolved definitions are mapped.

Other versions return unavailable, with original colors preserved. A single
launch diagnostic records partial/unavailable resolution and unresolved
collisions. Metadata contains normalized colors, semantic expressions, token
names and source provenance, never whole configuration or environment values.

## Matching

Foreground, background and underline channels are independent. Explicit
foreground/background pairs take precedence over individual colors. Equivalent
semantic expressions merge. Equally specific conflicting meanings, including
known unsupported syntax or decorative tokens, remain unchanged. Decisions
compare expressions rather than destination RGB values, so theme switches do
not change collision policy. There is no hue classification or nearby-color
matching. Identical emitted styles cannot distinguish their underlying meaning.

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
The parser follows the [Claude theme reference](https://code.claude.com/docs/en/terminal-config#create-a-custom-theme).

Codex data and behavior follow the official
[`rust-v0.162.1` source](https://github.com/openai/codex/tree/rust-v0.162.1/codex-rs/tui),
particularly `render/highlight.rs`, `render/model_themes.rs`, `style.rs`,
`diff_render.rs`, and the config loader. Model descriptor JSON contains only
scope names and normalized palette values. Dependencies pin the exact syntect
and two-face versions used by this release.

To support another version, verify its loader, emitted colors and theme formats,
add a versioned descriptor, and extend resolver/render/launch fixtures before
allowing that version. Never broaden the version gate based on semver alone.
Theme files are read only at launch, and lookups compile outside rendering.
