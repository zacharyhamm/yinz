# Kitty graphics conformance

The kitty graphics suite is written against the protocol specification
(`docs/`-external: kitty's `graphics-protocol.rst`), not against the implementation, so a failure
means the protocol work is not done rather than that the test is wrong. It used to carry an
expected-red set in modules ending `_red`; all nine deviations are now closed and **nothing is
skipped** — `cargo x test` and `cargo x integration-test` are both fully green.

Write new conformance tests against the spec the same way, and quote the sentence being tested.
The module doc comment of `yinz-terminal-graphics/src/kitty_graphics/mod.rs` records what was
closed and — more importantly — the twelve judgement calls that are deliberately pinned as-is,
where the spec does not say enough to call the behaviour wrong or two of its rules pull against
each other. Read those before changing anything they cover; the most load-bearing is that
animation stores and composes real frames but has no playback clock.

## Where the suite lives

The protocol engine (parser, store, `KittyGrid` placements, placeholders, replies) and Sixel are
in `yinz-terminal-graphics`, which knows nothing of the terminal grid. `Grid` and its `kitty_*`
methods, and the output that re-encodes images for the host, stay in `yinz-terminal`, so the
suite is split the same way:

* Protocol level: `yinz-terminal-graphics/src/kitty_graphics/unit/`
  (`cargo x test -p yinz-terminal-graphics`).
* Grid and host-emission levels: `yinz-terminal/src/panes/unit/grid_tests/kitty_*.rs` and
  `yinz-terminal/src/output/unit/output_kitty_spec_tests.rs` (`cargo x test -p yinz-terminal`).
* Whole app: `yinz-integration-tests/tests/kitty_graphics*.rs`.

The fixtures and the reply grammar are `kitty_graphics/test_support.rs`. `yinz-terminal`
reaches it through `panes::kitty_test_support`, which adds the helpers that drive a `Grid`, and
`yinz-integration-tests/src/kitty_protocol.rs` re-exports the builders and `KittyReply` for the
whole-app tests. A change to the reply grammar is therefore one change for every level.

The module is public and compiled in every build, the yinz binary's included. The whole-app
suite needs it outside `cfg(test)`, and gating it on a feature only the suite enabled made
`x integration-test` build this crate and every crate above it twice, once for the binary and
once for the suite. The fixtures also show up as `panes::test_support` in `yinz-terminal` and
`yinz-panes` through their `kitty_graphics::*` globs. Do not import them from there.

## Plugin panes

Plugin panes go through the same protocol as terminal panes. `PluginPane::handle_plugin_bytes`
runs the plugin's frame past a `KittyApcInterceptor` before the vte parser, exactly as
`TerminalPane::handle_pty_bytes` does - vte has no APC dispatch, so without that pre-pass a
plugin's `ESC _ G ... ESC \` is parsed into the ignore-string state and silently dropped. There is
no pty behind a plugin, so replies and forwarded queries are dropped rather than queued, and
plugins are expected to pass `q=2`.

Two constraints fall out of the plugin contract and are worth knowing before changing anything
here:

* **Unicode placeholders are the only placement style that works.** Every frame begins with
  `delete_viewport_and_scroll`, so an explicit placement anchored to a line index dangles.
  Placeholder placements are re-derived from the viewport cells each render
  (`Grid::kitty_refresh_placeholder_placements`), and `placeholder_uids` keeps the placement uid
  stable across those rebuilds so the host diff does not retransmit every frame.
* **A plugin pane holds one grid per attached client**, each with its own image-id namespace, so a
  plugin has to transmit into each of them - see hocus's `wand_transmitted`.

The plugin-side helpers are `yinz-tile/src/ui_components/image.rs`; the diacritic table they
share with this module lives in `yinz_types::kitty_placeholders`. hocus's empty-chat wand is the
first consumer, and it is also the first thing to drive frames off its own timer, which is what
the "no playback clock" note below means in practice.
