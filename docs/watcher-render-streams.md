# Watcher render streams

This describes how watchers are rendered as of the `notcurses-demo-perf` branch
(October 2026, after `aa5f2de20`), why that design is a replay, and a proposed
redesign that gives watchers their own diff caches. Nothing here is built yet;
it is a starting point for a later branch.

A *watcher* is a client that mirrors another client's view, the *followed*
client (`Screen::followed_client_id`). Watchers are not plugin clients and never
own a tab position, focus or mode of their own. They get the followed client's
tab, padded or cropped to the watcher's own size, without overlays such as toasts,
menus and palettes.

## How watchers render today

`Screen::render_to_clients_traced` (`yinz-screen/src/screen/render.rs`) runs two
passes in the same tick, over the same set of tabs (`Screen::deferred_tabs`):

1. **The regular pass** renders each tab for its regular clients
   (`Tab::render_panes(&mut output, None)`). When there are watchers, it arms
   recording for the followed client (`Output::pane_change_client`) and records
   which of that client's caches it consumed:
   - `Output::rendered_pane_contents`: panes whose content output reached the
     followed client (`yinz-panes/src/ui/pane_contents_and_ui.rs`, both the shared
     and the per-client paths).
   - `Output::rendered_pane_frames`: panes whose frame was re-emitted for it,
     including stack-list header frames.
   - `Output::rendered_pane_boundaries`: its tiled boundaries diff was non-empty
     (`TiledPanes::render`).
   - The display-clear flag is read before the pass (`replay_clear`), because the
     pass clears it (`Tab::hide_cursor_and_clear_display_as_needed`).
2. **The watcher pass** takes those sets and undoes what the regular pass
   consumed, then renders again with `client_id_override = Some(followed)`:
   - `Pane::prepare_content_replay(followed)` re-marks the content. Since this
     branch, Grid keeps the lines its last render read
     (`OutputBuffer::consume` / `replay_last_read`, `Grid::replay_last_render`),
     so only those lines are sent again.
   - `Pane::invalidate_frame(followed)` drops the frame cache entry.
   - `Tab::invalidate_boundaries(followed)` drops the boundaries cache entry.
   - A forced repaint, a display clear or a new watcher (`repaint_whole`) calls
     `Tab::set_force_render()` and clears the watcher's screen.

   Only the followed client's stream from this pass is kept and serialized for
   each watcher. Cursor state is the one cache that already has a watcher key:
   `CursorMemoKey::{Client(id), Watchers(followed)}` in `yinz-tab/src/tab/mod.rs`.

When the followed client has departed and watchers remain, there is no regular
pass for it. The sets stay empty and the watcher pass is the only consumer of
the diffs.

## Why it is a replay

Every per-client diff cache has exactly one consumer:

| Cache | Where | Consumed by |
| --- | --- | --- |
| Changed lines | `OutputBuffer::changed_lines`, `should_update_all_lines` | `Grid::read_changes` |
| Forced completed-text repaint | `Grid::repaint_completed_text` | `Grid::render` |
| Pane needs rendering | `TerminalPane` → `Grid::should_render` (one bool); `PluginPane::should_render` per client | `Pane::render` |
| Frames | `TerminalPane::frame`, `PluginPane::frame`, keyed by `ClientId` | `Pane::render_frame` |
| Boundaries | `TiledPanes::client_id_to_boundaries`, keyed by `ClientId` | `TiledPanes::render` |
| Display clear | `Tab::should_clear_display_before_rendering` | `Tab::hide_cursor_and_clear_display_as_needed` |
| Terminal title | `window_title` memo, shared by all clients | `render_terminal_title_if_needed` |

Watchers share the followed client's keys, so whichever pass runs first eats the
diff. The replay restores what the regular pass ate. That only works because
both passes run back to back with no input in between.

## Known limits of the replay

- **Each new per-client cache needs its own replay hook.** Forgetting one means
  watchers silently miss updates. The shared title memo is one cache with no
  hook today: watchers miss title changes.
- **Toast damage over-paints watchers.** Toasts damage panes with full-viewport
  repaints before the regular pass. Those panes are recorded, so watchers get
  full repaints of panes for toasts they never see.
- **The watcher pass rewrites regular clients' caches.** It renders for every
  client on the tab plus the followed one, and throws away all but the followed
  stream. This is harmless only because both passes see the same state.
- **Watchers cannot be paced on their own.** The replay needs both passes in the
  same tick, so a slow regular client and a healthy watcher are coupled through
  `deferred_tabs`.
- **Watcher graphics follow the followed client's capabilities.** Sixel output
  checks the followed client's support; Kitty output is withheld from watchers.

## Proposed design: a watcher stream key

Generalize `CursorMemoKey` into a render stream key used by every per-client
diff cache:

```rust
enum RenderStream {
    Client(ClientId),
    /// Every watcher following this client shares one stream.
    Watchers(ClientId),
}
```

### Content

The changed-line set needs a second consumer. It is marked once per printed
character, so the cost lands on the pty ingest hot path. Two ways to do it:

- **Mark both sets.** `OutputBuffer::update_line` inserts into the client set
  and, while the pane is watched, the watcher set. That costs one more bitset
  insert per character, gated on a flag. It is simple and each stream clears
  only its own set.
- **Merge lazily.** The client render ORs the lines it consumed into a pending
  watcher set when it clears (`OutputBuffer::consume` already keeps them). The
  watcher reads `pending ∪ current client lines` and clears only `pending`. To
  avoid sending a line twice, it must also remember which current client lines
  it has already seen, and leave those out when the client clears them later.
  That costs nothing on ingest, at the price of more bookkeeping.

Either way, a watcher stream also needs its own `should_render` flag and its
own `repaint_completed_text`. `Grid::render` has side effects beyond line
selection that must not run twice or run per stream:

- reaping overwritten sixel images (`drain_image_ids_to_reap`, `reap_images`)
- settling and re-anchoring Kitty placements
- updating `completed_text` and `completed_sixel`

Split those from "which lines to emit" so both streams read the same grid
state.

Plugin panes already keep a grid per client. The watcher stream reads the
followed client's grid with its own render flag.

### Frames and boundaries

- Key `TerminalPane::frame`, `PluginPane::frame` and
  `TiledPanes::client_id_to_boundaries` by `RenderStream`.
  `Pane::render_frame` has to take the stream, because it also uses `client_id`
  for frame color overrides, pending agent escapes and the rename prompt.
- `reset_boundaries`, `set_force_render` and `force_render_panes_intersecting`
  invalidate every stream, as they do every client today.

### Tab-level state

- Make the display-clear flag per stream, replacing `replay_clear`.
- Scope `repaint_whole` to the watcher stream, so a new watcher no longer forces
  a full repaint on every regular client.
- Key the terminal title memo by stream.

### The watcher pass

Render only the followed client in the watcher pass, as `clients_in_frame`
already does for cursors. Every `client_id_override` site adds the override to
the tab's client set today: `TiledPanes`, `FloatingPanes`, `OverlayPanes` and
`Tab`. Once the watcher pass renders only its own stream, it no longer touches
the regular streams' caches.

### What goes away

- `Output::{pane_change_client, rendered_pane_contents, rendered_pane_frames,
  rendered_pane_boundaries}` and the recording in `pane_contents_and_ui.rs` and
  `TiledPanes::render`.
- `Pane::prepare_content_replay` and `Pane::invalidate_frame`.
- `OutputBuffer::consume` and `replay_last_read`, and `Grid::replay_last_render`.
- The replay locals and plumbing in `render.rs`.
- `Tab::invalidate_boundaries` stays: `Tab::remove_client_pane_state` uses it.

### What it enables

- Watchers paced independently of regular clients, since nothing ties the two
  passes to one tick.
- No over-paint from overlays the watcher does not see.
- New per-client caches are correct for watchers as soon as they take a stream key.

## Risks and order of work

- **Ingest cost.** Measure the content change with `cargo x bench` against a
  saved baseline before anything else (see the bench skill). If marking both
  sets shows up, use the lazy merge.
- **`set_should_render(false)` callers.** Some code suppresses a render by
  clearing the flag. Each caller must say which stream it means, or clear both.
- **Pane trait churn.** `render_frame`'s signature changes for every `Pane`
  implementation, including the test `MockPane`s.

A workable order, with tests green after each step:

1. Add `RenderStream`; key frames and boundaries by it. Keep the replay for content.
2. Make the watcher pass render only the followed stream; delete the frame and
   boundary replay.
3. Add the second content consumer and per-stream render flags; delete the
   content replay.
4. Make the display clear, `repaint_whole` and the title memo per stream; delete
   the rest of the replay.
5. Optionally, pace watchers independently.

## Tests that pin today's behavior

- `yinz-screen/src/unit/screen_tests/render_pacing.rs`: the watcher pacing and
  replay tests, including `watcher_replays_shared_boundaries_after_a_split`,
  `output_on_another_tab_does_not_repaint_watched_contents`,
  `hover_only_changes_wake_a_healthy_watcher`,
  `watchers_receive_only_the_lines_that_changed` and
  `a_slow_regular_client_does_not_hold_back_a_healthy_watcher`.
- `yinz-screen/src/unit/screen_tests/clients.rs`: departed followed clients,
  retained plugin state, watcher cursor tests.
- `context_menu.rs`, `command_palette.rs`, `capabilities.rs`, `osc7.rs`,
  `plugin_broadcasts.rs`, `subscribers.rs`, `waiting_pulse.rs`, `sizing.rs` and
  `quit.rs` under `yinz-screen/src/unit/screen_tests/`: watcher cases for each
  feature.
- `yinz-tab/src/tab/unit/tab_integration_tests/cursor.rs`: separate cursor
  state for watchers and the followed client.
- `yinz-integration-tests/tests/cli.rs`: `watcher_client_functionality`
  (snapshot) and
  `watcher_larger_than_the_followed_content_is_padded_to_its_own_size`.
