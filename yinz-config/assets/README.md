# Configuration assets

Default configuration, layouts, and themes are loaded by `yinz-config`. The builtin TypeScript
plugins live in `yinz-deno-builtins/assets/plugins/`.

## Ghostty theme ports

691 builtin themes are ported from [iTerm2 Color Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes/tree/31756e77934045c66b28f59463ddb2870606d7ca/ghostty),
commit `31756e77934045c66b28f59463ddb2870606d7ca`, the 725-theme revision pinned by
Ghostty's `ghostty-themes-release-20261005-151109-31756e7.tgz` dependency.
The upstream collection's [license notice](licenses/ghostty-themes-MIT.txt) applies;
individual themes retain their authors' copyrights and licenses.

Use the lowercase hyphenated Yinz name, for example `theme "adventure-time"`.
Camel case is split, punctuation becomes a hyphen, and `+` becomes `plus`.
The [complete inventory](../src/unit/fixtures/ghostty-themes.tsv) maps every original
name to its Yinz name and records whether it was imported or already present. Its
last column preserves source foreground, background, and ANSI colors 0–15 for tests.

Terminal foreground, background, and ANSI colors are copied exactly. UI text uses
the source foreground/background and selection colors; ANSI accents supply emphasis,
ribbons, frames, exit codes, and multiplayer colors, with yellow as the orange accent.
UI foregrounds move only as far toward black or white as needed for 4.5:1 contrast.
The unselected ribbon uses a readable foreground as its background and hover color.
When selected and unselected ribbon backgrounds coincide, the selected ribbon uses
the theme's selected text colors to keep the active tab distinct.
When the source selection background equals the ordinary background, it moves one
eighth toward black or white so selected rows remain visible. All UI colors use RGB
and do not depend on the host terminal palette.
Bundled themes are cached so repeated configuration loads do not reparse the catalog.

Yinz's theme format has no cursor color or cursor text fields. Ghostty selection
colors inform UI selection styles rather than terminal text selection. This is a
one-time bundled import, with no runtime importer or automatic synchronization.
Existing Yinz themes are unchanged, including the following 34 equivalent themes:

| Ghostty name | Existing Yinz name |
| --- | --- |
| Atelier Sulphurpool | `atelier-sulphurpool` |
| Atom One Dark | `onedark` |
| Ayu | `ayu-dark` |
| Ayu Light | `ayu-light` |
| Ayu Mirage | `ayu-mirage` |
| Catppuccin Frappe | `catppuccin-frappe` |
| Catppuccin Latte | `catppuccin-latte` |
| Catppuccin Macchiato | `catppuccin-macchiato` |
| Catppuccin Mocha | `catppuccin-mocha` |
| Dayfox | `dayfox` |
| Dracula | `dracula` |
| Everforest Dark Med | `everforest-dark` |
| Everforest Light Med | `everforest-light` |
| Flexoki Dark | `flexoki-dark` |
| Gruber Darker | `gruber-darker` |
| Gruvbox Dark | `gruvbox-dark` |
| Gruvbox Light | `gruvbox-light` |
| Iceberg Dark | `iceberg-dark` |
| Iceberg Light | `iceberg-light` |
| Kanagawa Wave | `kanagawa` |
| Molokai | `molokai-dark` |
| Night Owl | `night-owl` |
| Nightfox | `nightfox` |
| Nord | `nord` |
| One Half Dark | `one-half-dark` |
| Pencil Light | `pencil-light` |
| Terafox | `terafox` |
| TokyoNight | `tokyo-night` |
| TokyoNight Day | `tokyo-night-light` |
| TokyoNight Night | `tokyo-night-dark` |
| TokyoNight Storm | `tokyo-night-storm` |
| Vesper | `vesper` |
| iTerm2 Solarized Dark | `solarized-dark` |
| iTerm2 Solarized Light | `solarized-light` |
