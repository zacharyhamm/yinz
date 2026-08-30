# The plugin render pipeline, and why input lags behind it

This is a write-up of structural problems in how Zellij delivers events to plugins and gets
their output back on screen. Of the candidate fixes at the end, 1 and 2 (batching on the plugin
thread, one render per plugin per batch) are done and marked so; 3 to 5 are not. It was written
while fixing hocus's input lag during a streaming reply, where the costs below were the
amplifier rather than the cause — the cause was hocus redrawing its whole transcript ten times a
second, which is fixed. What follows is what remains, so the work can be picked up deliberately
rather than rediscovered.

The short version: **a keystroke aimed at a plugin pane crosses four strictly-FIFO queues, and
nothing anywhere gives it priority over the traffic already in them.** A plugin receiving a lot
of events fills those queues with its own work, and the user's typing waits behind it.

## The path a keystroke takes

1. **The route thread.** `route_action` sends `ScreenInstruction::WriteCharacter` and then
   *blocks* waiting for it to complete — `route.rs:300-325`, with the wait at `route.rs:2342`
   and `ACTION_COMPLETION_TIMEOUT` (1 second) at `route.rs:55`. While blocked, this thread is
   not reading the next keystroke off the client's socket, so one slow key delays the key after
   it. `Action::Write` also sends a `ClearScroll` first, making it two round-trips.

2. **The screen thread** (unbounded, `lib.rs:2209`). The key waits behind everything already
   queued, including every `PluginBytes` message. Each of those is not cheap: `handle_plugin_bytes`
   (`panes/plugin_pane.rs:236-256`) throws away the plugin's entire viewport and re-parses the
   new one through VTE, synchronously. Superseded renders for the same pane are not dropped,
   even though each one discards the previous viewport anyway.

   Reaching the plugin, the key becomes `PluginInstruction::Update(vec![(pid, client, Event::Key)])`
   at `tab/mod.rs:4943`.

3. **The plugin thread** (unbounded, `lib.rs:2216`, loop after `drain_updates` in `plugins/mod.rs`). `Event::Key`
   is the *same* `PluginInstruction::Update` variant as every other event — streamed HTTP
   chunks, pane updates, timers (`plugins/mod.rs:504`). `Bus::recv` (`thread_bus.rs:220`) is a
   plain `crossbeam::Select` with no priority. There is no way to say a key matters more.

4. **The pinned executor** (unbounded `std::sync::mpsc`, `pinned_executor.rs:104`). Each plugin
   is permanently assigned to one thread (`pinned_executor.rs:146-205`); the pool is
   `num_cpus::get().clamp(4, 16)` (`wasm_bridge.rs:273`), and plugins assigned while all threads
   are idle commonly land together on thread 0. The key's job queues behind the plugin's pending
   jobs, each of which holds `running_plugin.lock()` across its `update()` **and** its `render()`.

## What makes those queues deep

**One `update()`, one full `render()` and one `PluginBytes` per event, per plugin — as it
was.** `update_plugins` (`wasm_bridge.rs`) used to walk the `Vec` it received in a nested loop,
and each matching event became its own executor job ending in its own
`send_to_screen(ScreenInstruction::PluginBytes(..))`: `apply_event_to_plugin` called the guest
`update`, and if it returned true, `render_plugin_for_clients` re-rendered the whole viewport,
once per client. Ten events that each asked for a repaint produced ten renders and ten VTE
re-parses, even when they arrived together and only the last one's output would ever have been
seen. Candidates 1 and 2 below fixed this: the plugin thread merges the `Update`s waiting on its
bus, and a plugin gets its share of the batch in one job and renders once at the end.

The per-message shape still holds for pipe messages (`WasmBridge::pipe_messages`).

**Every render asset sends a message back.** `screen/render.rs:30` returns `UnblockCliPipes` to
the plugin thread per asset, and `apply_events_to_plugin` (`wasm_bridge.rs`) sends one from the
*non*-rendering branch too. So each batch a plugin takes costs at least two plugin-thread
messages, not one — per batch now rather than per event, but still there.

**Copies.** `update_plugins` (`wasm_bridge.rs`) clones each event into the per-plugin batch it
hands the executor (`event.clone()` while collecting `events`), and again into
`cached_events_for_pending_plugins` for every plugin still loading. A streamed HTTP body is
cloned at least twice on the way to the plugin.

**Renaming a pane is expensive.** `ScreenInstruction::RenamePane`
(`screen/thread_main.rs:3310`) re-renders and calls `log_and_report_session_state()`, which
rebuilds a `PaneManifest` across every tab and every pane plus all `TabInfo`s, and pushes
`PaneUpdate` + `TabUpdate` at *every subscribed plugin, per client*
(`screen/plugin_state.rs:128-160`, `:335-344`). A plugin that reasserts its own title every render
turns each of its frames into a burst of events for every other plugin — and for itself. This
one now short-circuits when the name has not actually changed (`Tab::rename_pane` returns
whether it moved), but the underlying cost of a real rename is unchanged.

## The only throttle that exists

`BackgroundJob::RenderToClients` is debounced at 10 ms (`background_jobs.rs:221` `REPAINT_DELAY_MS`,
debouncer at `:636-682`), reached via `screen/render.rs:13-33`. That coalesces the final *paint*
to clients and nothing upstream of it — not the plugin `update()`, not the plugin `render()`,
not the per-`PluginBytes` VTE parse, and none of the four queues.

`RenderBlocker` (`screen/mod.rs:432-460`) is the opposite of a throttle: it *delays* rendering
until pending plugins have rendered, with a 100 ms timeout.

## Candidate fixes, roughly in the order they look worth doing

1. **Drain and merge on the plugin thread.** *Done.* After `bus.recv()`, `drain_updates`
   (`plugins/mod.rs`, next to the thread's loop) `Bus::try_recv`s the `Update(..)` messages
   waiting behind it — at most `MAX_UPDATE_DRAIN` per pass — and merges them into one
   `update_plugins` call. The first message of another kind stops the drain and is handled next,
   so nothing is reordered. Events are never merged (`tab-bar` and `strider` step once per
   *event*, not per line), with one exception: `coalesce_hovers` keeps only the newest
   `Mouse::Hover` per (plugin, client) in a batch, since a hover is a position sample. On its own
   this still does not help a key that is genuinely behind other events.

2. **Dirty-mark and render once per drained batch.** *Done.* `update_plugins`
   (`wasm_bridge.rs`) now submits one executor job per plugin instance carrying every event of
   the batch that plugin takes, and `apply_events_to_plugin` runs them in order — each under the
   client it was directed at — and renders once at the end if any asked to. This decouples "how
   many events arrived" from "how many times we render and re-parse a viewport": a wheel flood
   of N notches is N cheap `update()` calls and one `render()` per client. The plugin's pending
   CLI pipe changes are drained after *each* event (`pipes_to_block_or_unblock`) and merged so
   the last change to a pipe wins — one drain per batch would let an unblock override a later
   block — and the merged set rides on that one render, or on the one `UnblockCliPipes` when
   nothing rendered. A guest error mid-batch drops the rest of the batch but still delivers the
   pipe changes made before it. `forward_events_to_cli_subscribers` collapses a run of
   consecutive state reports to the newest of each kind, since merged batches can carry several,
   without moving a report past the non-report events around it.

3. **Drop superseded `PluginBytes` on the screen thread** (`screen/thread_main.rs:290`). For a
   given `(plugin_id, client_id)`, only the newest asset can matter — `handle_plugin_bytes`
   discards the previous viewport regardless. Every one dropped is a full VTE parse removed from
   the keystroke's critical path.

4. **A priority lane for input.** The honest fix for the head-of-line blocking. Either a
   separate high-priority channel the plugin thread's `Select` prefers, or splitting
   `Event::Key`/`Event::Mouse` out of `PluginInstruction::Update` so they can be. This is the
   only change that bounds keystroke latency rather than merely reducing the queue depth it
   waits behind.

5. **Reconsider the route thread's blocking wait** (`route.rs:2342`). A key that does not need a
   completion signal should not stall the client's whole input loop for up to a second.

## A deferred hocus item

The transcript cache is now per-message, so a streaming frame redraws only the message that
grew (`default-plugins/hocus/src/ui.rs`, `TranscriptCache`). Within that one message the parse
is still from scratch, which leaves an O(reply²) term over the length of a single reply. It has
not mattered — a reply is small next to a conversation, and the benchmark
(`default-plugins/hocus/benches/transcript.rs`, `transcript/streaming_frame`) is flat — but the
fix if it ever does is to reuse the parse of the stable prefix, cutting at the last block
boundary. The risk is entirely in choosing that boundary: a `\n\n` inside a fenced code block,
a list, or a table is not a block boundary, and getting it wrong corrupts the render rather than
merely slowing it.
