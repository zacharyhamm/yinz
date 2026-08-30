# The plugin render pipeline, and why input lags behind it

This is a write-up of structural problems in how Zellij delivers events to plugins and gets
their output back on screen. Of the candidate fixes at the end, 1, 2 and 4 (batching on the
plugin thread, one render per plugin per batch, input priority in the executor) are done and
marked so; 3 and 5 are not. It was written while fixing hocus's input lag during a streaming
reply, where the costs below were the amplifier rather than the cause — the cause was hocus
redrawing its whole transcript ten times a second, which is fixed. A second round of lag ("hocus
is laggy while agents work") turned out to be a different amplifier again, the agent-hook
fan-out — measured, fixed and written up in its own section below, along with the
instrumentation that now makes this whole pipeline measurable instead of guessable.

The short version: **a keystroke aimed at a plugin pane crosses several FIFO queues, and until
the executor learned to prefer input, nothing anywhere gave it priority over the traffic already
in them.** A plugin receiving a lot of events fills those queues with its own work, and the
user's typing waits behind it. (One correction to this doc's original claim: the *screen*
thread's bus is not one strict FIFO — it is two receivers, an unbounded lane for the route
thread and a bounded(50) lane for pty+plugin traffic (`lib.rs:2107-2112`), joined by a
random-pick `crossbeam::Select`, so a keystroke does not wait behind every queued `PluginBytes`
there. The bounded lane matters the other way around: a sender into a full screen queue blocks —
see the executor note below.)

## The path a keystroke takes

1. **The route thread.** `route_action` sends `ScreenInstruction::WriteCharacter` and then
   *blocks* waiting for it to complete — `route.rs:309-326`, with the wait at `route.rs:2362`
   and `ACTION_COMPLETION_TIMEOUT` (1 second) at `route.rs:55`. While blocked, this thread is
   not reading the next keystroke off the client's socket, so one slow key delays the key after
   it. `Action::Write` also sends a `ClearScroll` first, making it two round-trips — and every
   non-mouse action additionally broadcasts an `Event::InputReceived` update to the plugin
   thread before dispatch (`route.rs:279-289`).

2. **The screen thread** (two lanes, `lib.rs:2107-2112`: unbounded for the route thread,
   bounded(50) for pty+plugin, random-pick select — see the intro's correction). Each
   `PluginBytes` is not cheap: `handle_plugin_bytes`
   (`panes/plugin_pane.rs:236-256`) throws away the plugin's entire viewport and re-parses the
   new one through VTE, synchronously. Superseded renders for the same pane are not dropped,
   even though each one discards the previous viewport anyway.

   Reaching the plugin, the key becomes `PluginInstruction::Update(vec![(pid, client, Event::Key)])`
   at `tab/mod.rs:4970`. Notably, the plugin-pane key path does *not* set `should_update_ui`,
   so `log_and_report_session_state` is not on it.

3. **The plugin thread** (unbounded, `lib.rs:2114`, loop after `drain_updates` in `plugins/mod.rs`). `Event::Key`
   is the *same* `PluginInstruction::Update` variant as every other event — streamed HTTP
   chunks (already coalesced at the producer into ≥8 KiB slices, `background_jobs.rs`), pane
   updates, timers (`plugins/mod.rs:126`). `Bus::recv` (`thread_bus.rs:221`) is a plain
   `crossbeam::Select` with no priority; the priority now lives one hop later, in the executor
   (candidate 4 below).

4. **The pinned executor** (unbounded `std::sync::mpsc`, `pinned_executor.rs`). Each plugin is
   permanently assigned to one thread; the pool is `num_cpus::get().clamp(4, 16)`
   (`wasm_bridge.rs:272`). A plugin now gets an empty thread while one exists, workers pick
   input-bearing plugins' jobs first (per-plugin order preserved — see candidate 4), and the
   plugin's lock is dropped before the bounded screen send. The lock still spans a batch's
   `update()`s **and** its `render()`.

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

3. **Drop superseded `PluginBytes` on the screen thread** (`screen/thread_main.rs:276`). For a
   given `(plugin_id, client_id)`, only the newest asset can matter — `handle_plugin_bytes`
   discards the previous viewport regardless. Every one dropped is a full VTE parse removed from
   the keystroke's critical path.

4. **A priority lane for input.** *Done, in the executor rather than on the plugin thread's
   bus.* Every executor job now carries a tag — its plugin, a label for the stats log, and
   whether the batch holds a `Key`/`Mouse`/`PastedText` (`pinned_executor.rs`, `JobTag`;
   `update_plugins` sets it via `summarize_batch_for_stats`). Each worker keeps its queue
   locally and picks the next job with `pick_next_job`: per-plugin order is absolute (the
   `batching.rs` invariants), but a plugin with an input job waiting anywhere in its queue goes
   ahead of its thread-mates. Two more changes serve the same goal: `register_plugin` now gives
   a plugin an empty thread while one exists (idle-time loads used to pile everything onto
   thread 0, so a keystroke queued behind every daemon's work), and the events/pipe jobs drop
   the plugin's lock before sending `PluginBytes` into the bounded screen channel, so a lagging
   screen thread can no longer stall the plugin's whole queue transitively. What remains undone
   of the original idea: a lane on the *plugin thread's* bus itself, which matters only when
   that thread's drain loop is the bottleneck — the measurements below say the executor was.

5. **Reconsider the route thread's blocking wait** (`route.rs:2342`). A key that does not need a
   completion signal should not stall the client's whole input loop for up to a second.

## The agent-hook fan-out, and how to measure this pipeline

The second round of "typing in hocus is laggy" (2026-08) was not the streaming path at all. With
two coding agents working in panes, their `zellij action agent-report --hook-json` hooks fired
~3/s, and the live session's server burned ~85% of a core — ~95% of it on the plugin executor
threads, ~1.5 CPU-seconds per hook event. Typing waited behind all of it.

What one hook event cost, as measured (each × its own amplification):

1. **Every departing connection re-broadcast the whole session.** A hook's CLI client — and
   every session *probe*, which is a connection too — ended in `ScreenInstruction::RemoveClient`,
   whose handler reported the full session state three times over (`remove_client` itself,
   `resize_to_clients`, and once more directly). At ~3.3 connections per hook that was ~10 full
   reports per hook, each a fresh `PaneManifest` + `TabInfo`s + a `SessionUpdate` carrying a
   clone of **every known session's** `SessionInfo`. Fixed: the handler skips the reflow and the
   report for a connection that never attached (`is_client_attached`), and a real detach reports
   once, via `resize_to_clients`.
2. **`SetPaneAgent` rebuilt tab state per record flip.** A working agent flips its record's
   `detail` on every `PreToolUse`/`PostToolUse`. `report_pane_state` (`screen/plugin_state.rs`)
   now rebuilds only the pane manifest. It also mirrors that manifest into the cached
   `SessionInfo`, its background snapshot and `SessionUpdate`, because `SessionInfo` embeds it;
   the redundant `TabUpdate` remains gone.
3. **hocus-agentd republished an unchanged world.** `publish()` ran unconditionally on
   `PaneUpdate`, `TabUpdate`, `CommandChanged`, `CwdChanged`, the 5s heartbeat and every hook —
   each a full clone + `serde_json::to_string` piped to every subscriber (hocus, hocus-store,
   hocus-agents), each of whom parsed and merged it. Now a snapshot identical to the last one
   sent is withheld (`last_published`), except to a subscriber that just arrived.

Still deliberately unfixed: the `agent_hook` `KeybindPipe` broadcast to every plugin (≤64 KiB,
one encode/decode per plugin per hook, ~1.5 ms per plugin in a debug build) — cheap once the
above landed.

### Measuring it

- **Executor stats, always on.** Each worker logs one line per ~10 s to the zellij log
  (`plugin-exec-N job stats (last 10.0s): hocus[5]: 60 jobs, run 527ms (max 17ms PaneUpdate),
  wait 464ms (max 17ms) [PaneUpdate×30 473ms, pipe:agent_hook×29 41ms]`): per plugin, job count,
  run and queue-wait time, and the breakdown by job label. An input-bearing job that waited
  ≥100 ms is logged the moment it starts — that wait *is* keystroke latency.
- **`scripts/hook-storm.sh <pane_id> [rate] [secs]`** fires synthetic
  `PreToolUse`/`PostToolUse` hook events at a pane (default 3/s × 30 s, the measured live rate),
  flipping the record each time like a real agent.
- **`scripts/plugin-thread-cpu.sh <session> [secs] [hooks]`** samples the server's
  `plugin-exec-*`/`screen` threads' CPU per second from `/proc` and totals it, per hook when
  told how many were sent. Both honor `ZELLIJ_BIN` for a checkout build.

Storm at 3/s for 30 s against a fresh one-tab session with hocus open and one client attached,
debug build, before the three fixes and after the original fan-out fix. The latter predates the
consistency correction that restored one `SessionUpdate` per changed agent record because
`SessionInfo` embeds the pane manifest:

| metric | before | after original fan-out fix |
|---|---|---|
| full session-state reports during the run | 1210 | 23 |
| hocus executor jobs / 10 s | 804 (247 `SessionUpdate`, 262 `PaneUpdate`, 238 `TabUpdate`) | 60 (30 `PaneUpdate`, 30 `pipe:agent_hook`) |
| hocus run time / 10 s | 4.1 s | 0.5 s |
| hocus max queue wait | 653 ms | 18 ms |
| plugin-thread CPU per hook, all threads | ~700 ms | ~70–140 ms |

(Current per-hook work is the intended `PaneUpdate`, the required `SessionUpdate`, and the
per-plugin `agent_hook` pipe. Spreading plugins one-per-thread trades a little total CPU for the
latency isolation — hocus no longer shares a queue with anything.)

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
