# Deno plugins

Plugins use Wasm or Deno (JavaScript/TypeScript).
The Deno runtime lives here; `docs/DENO_PLUGINS.md` is the user-facing guide,
`docs/DENO_PARITY.md` records what it has been verified to do.

The runtime shares everything above `RunningPlugin` with Wasm: event routing,
subscriptions, rendering, and the whole command surface through the one dispatcher
`yinz_exports::run_plugin_command`. `mod.rs` holds the isolate and its lifecycle, `sdk.js` is
the guest-side SDK (every convenience lives there, never in Rust), `sdk.d.ts` is the contract the
checker holds plugins to, `ui.rs` binds isolate-local widget state and never dispatches, and
`event.rs`/`transport.rs` are the JSON boundary in each direction.

## Things to know before changing it

- **Every isolate is budgeted** (`limits.rs`): a 1 GiB V8 heap and a separate 1 GiB
  backing-store budget, counted by an allocator V8 is given at creation. Exceeding either
  permanently fails the isolate, even when JavaScript catches the allocation exception, and the
  host disposes it without running guest cleanup. Anything the host hands the guest as bytes - an
  encoded string, a live output chunk, a byte reply - goes through `CountedAllocation`, because
  `serde_v8`'s buffer types allocate outside the counted allocator. Resizable buffers and guest
  Wasm are unavailable for the same reason.
- **An isolate belongs to one executor thread.** `DenoPlugin::with_runtime` bails off its thread;
  host ops run synchronously on the executor. `RUNTIMES` is thread-local, keyed by generation.
- **Guest workers own dedicated threads** (`background_worker.rs`). Each declared `workers`
  callback gets a persistent isolate with the same module graph and limits. Its FIFO awaits
  each job before starting the next. `PluginEnv.deno_workers` routes messages through the shared
  command dispatcher and scopes replies to the parent generation. Worker host replies and ticks
  use the worker generation. Parent disposal cancels and joins every worker before replacement.
- **A generation is an instance's identity.** `NEXT_GENERATION` names each isolate; `TickWake`,
  command replies, data watches and pane-output subscriptions carry it, so a reply, tick or chunk
  addressed to an instance that has since been reloaded is dropped rather than delivered to its
  replacement. Reload order: `BeforeClose` to the old instance, its registrations cleared, then
  the replacement loads. A reload waits out a call in flight for the unload grace before
  interrupting it; an interrupted instance gets no `BeforeClose`. Unload clears them after guest cleanup. The optional `unload` hook runs once during
  normal disposal (also on reload), before promises and workers are cancelled. The pinned
  executor owns a `thread_bus::DenoReplies` mailbox from before guest initialization until
  disposal. Generation-scoped replies stay there while notifications wait on the bus, in a
  loading cache, or on an executor (including a worker). Only the owning executor claims
  them for normal delivery, with the usual subscription and permission checks. Cleanup
  atomically stops ordinary notifications and drains unclaimed replies followed by new
  arrivals in routing order, preserving each acting client, even after the plugin leaves
  the map. Failed initialization and final disposal remove the mailbox. A two-second isolate
  watchdog bounds async and synchronous cleanup; failed/interrupted isolates skip it. Shutdown keeps background jobs alive until
  plugins finish so cleanup can await `runCommand`. No hook is guaranteed after a crash or
  the session shutdown deadline.
- **Modules are confined** (`loader.rs`): an import is an explicit relative `.js/.mjs/.ts/.mts`
  inside the entrypoint's folder after symlink resolution, plus `yinz:plugin`. Repository
  plugins use the canonical repository root instead, carried in `PluginConfig.repository_root`
  with the installed commit and resolved entrypoint. Managed roots remain pinned until reload. Builtins resolve
  relative imports within the compiled `yinz-deno-builtins/assets/plugins` tree, including
  shared and nested modules, with no filesystem fallback. The checker and strict authoring use
  the same module graph.
  Remote, npm, JSR and `node:` specifiers are refused, and so is any `http(s)://` plugin URL
  (`yinz_runtime::plugin_location`). Sources are bounded before parsing
  (8 MiB a file, 64 MiB a graph). Each instance keeps its source maps, so diagnostics name the
  TypeScript line. Module initialization must complete without a pending top-level `await`.
- **Parsing happens in a subprocess** (`worker.rs`): SWC's recursively allocated native ASTs may
  only be parsed, inspected, transpiled or dropped in a disposable `--deno-compilation-worker`
  process with a 30 s deadline, so a pathological source cannot take the server down.
  `worker.rs` here is only that process's entry point. The request/response types and the
  client that spawns, feeds, bounds and reaps it are in `yinz-plugin-authoring/src/worker.rs`,
  shared by both sides, so change the wire format there. The
  content-keyed transpile cache in `loader.rs` (path, source digest, declarations mode) is what
  keeps that from being one spawn per module per client per reload; compiled-in builtins are
  pinned in it, the rest is LRU-bounded. Errors are not cached.
- **The checker is its own isolate** (`checker.rs`): the bundled TypeScript compiler (provenance
  in `compiler/provenance.json`, hash-checked by a test) runs offline with ES2022 declarations
  and `sdk.d.ts`; plugin source is data to it, never a module it evaluates. `analysis.rs` runs in
  the worker and is where Agent's inferred grants and the lint findings come from.
- **Strict authoring is separate from loading.** `authoring.rs` stages the loader's confined
  dependency graph, formats only the requested file, then runs the compiler and pinned native
  Oxlint/tsgolint. `authoring/` owns the shared policies, hashes and licenses; xtask installs the
  tools independently of Wasm builtins. `Syntax` stays lightweight; `Format` and `Prepare`
  return formatted source. The worker deadline covers the complete process group.
- **The authoring entry points link no engine.** `yinz-plugin-authoring` holds what the CLI,
  `PluginCommand::CheckPlugin` and Agent's native tests call (`check_source`, `write`,
  `format_file`, `log_tail`), the analysis client and the source-size budget. It reaches
  analysis, the checker and strict authoring only through the worker, so it must not depend on
  `deno_core`, `deno_ast` or this crate; `ENGINE_FREE` in `xtask/src/metadata.rs` enforces that
  for it and for Agent. Anything needing V8 or SWC belongs on the worker side, here.
  `yinz_plugin_host::plugin_authoring` re-exports it for the server and the CLI. Its
  always-on `worker::spawn_count` is what this crate's loader and plugin tests assert deltas
  against.
- **Events keep their serde shape.** `update(event)` and `on(type, payload, raw)` receive the
  externally tagged Rust enum as JSON - positional tuples, `null` in absent slots - with two
  deliberate exceptions built in Rust: `PaneRenderReport` is re-keyed `terminal_3` (`event.rs`),
  and the live-output chunks of `RunCommandOutput` and `PaneOutput` are real `Uint8Array`s
  (`RuntimeInstance::call_event`). Normalization is additive: `yinz.event()` and `onEvent()`
  decode in the SDK, so builtins that destructure tuples keep working. Delivery order is `update`,
  then pipe waiters, then the timer sweep, then the `on` handlers; the reserved pipes the SDK
  consumes (RPC, completions, Agent extensions, default keybindings) never reach `pipe`.
- **Permissions are pre-flighted in `dispatch`** so a refused command throws in the guest with
  the fix named - and a plugin whose request is unanswered is told to wait for
  `PermissionRequestResult` while one that never asked is told to ask. The parent and its worker isolates share
  one permission coordinator beside the worker registry. Only the FIFO head reaches the host,
  and later requests carry earlier grants because the host replaces the set. The parent processes
  each host answer once. A private bridge callback completes the requesting isolate's promise;
  ordinary `PermissionRequestResult` observers never settle requests. Agent pre-grants the literal requests
  `analysis.rs` finds, so an authored plugin never prompts in a pane the loop cannot see.
- **`console` goes to the log.** Core's console would write to the server's stdout, so the SDK
  installs a frozen one whose methods reach `[deno:<file>]`, available from module top level.

## Where the tests are

`unit/host_sdk.rs`, `unit/*_sdk.rs` and the `sdk_tests` module in `mod.rs` run the SDK against a
JSON fake host in-process; `unit/checker.rs` compiles TypeScript fixtures - serialized Rust
values included, and every ```` ```ts ```` block of the guide - against `sdk.d.ts`;
`unit/analysis.rs` covers inferred grants and the lint rules; `unit/limits.rs` exhausts budgets
in supervised subprocesses; `unit/loader.rs` and `unit/worker.rs` cover confinement, the cache
and containment in the compilation subprocess, while the worker transport's deadlines, frame
limits, early exits and reaping are tested in `yinz-plugin-authoring/src/unit/worker.rs` beside
`plugin_authoring.rs`'s own tests; `unit/builtins.rs` loads every shipped builtin and reference
example; `../unit/plugin_tests/deno_plugins.rs` drives a real plugin thread; and
`yinz-integration-tests/tests/deno_plugins.rs` runs whole sessions. Agent serves the guide and
`sdk.d.ts` through `agent/src/plugin_docs.rs`, whose tests hold every `##` heading of the guide
to a section name and every section under 16 KB.

## Shared host boundary

Keep host authority in `yinz_exports::run_plugin_command`. Wasm and Deno use this dispatcher
for commands, permissions, subscriptions, and lifecycle operations. SDK conveniences belong in
`sdk.js`. Widget bindings only manage isolate-local state and rendering.

Do not expose filesystem, process, or network access around the dispatcher. Imports stay confined
after symlink resolution. Reply decoding lives in `response.rs`; `yinz-tile/src/shim.rs` defines
the corresponding Wasm contract. Keep command and reply coverage tests when adding a command.
