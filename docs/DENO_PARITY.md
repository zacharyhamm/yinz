# Deno capability and verification status

Current audit: 2026-09-21. Start with "Builtin migration audit" below for the gaps found in actual
Wasm builtin dependencies and the changes enabling their ports.

The historical 2026-09-11 audit examined the host, guest SDK, tooling, and docs.
Its findings and earlier verification remain below. Lua support was subsequently removed; see [migration](LUA_REMOVAL.md).
The seven changes from that audit are implemented in this checkout:
exact numeric inputs, runtime/checker quotas, host-path tests and queue ownership,
complete raw SDK declarations, filesystem/configuration helpers, persistence helpers,
and runtime-aware Hocus authoring. The existing asynchronous SDK remains in place.

This is a capability record, not a claim of full Wasm/Lua/Deno parity.
**Verified** below means the stated outcome has passed a named Linux test. Raw access
and compiler inventory coverage do not establish every command's host behavior.
Native macOS verification requires a macOS host and remains outstanding.

## Builtin migration audit (2026-09-21)

Command coverage did not cover every capability used by the Wasm builtins. This audit also traced
their filesystem calls, rendering libraries, editor state, request IDs, module structure, and worker paths.
The following gaps are now implemented:

| Missing capability | Existing consumers | Deno replacement |
|---|---|---|
| Read large files without an external process | Hocus legacy chat/prompt imports, hocus-store legacy data | `fs.read(path, {offset, maxBytes})`, through the shared permissioned dispatcher |
| Direct editor state, history, completion, undo | Hocus prompt editor | Native text-input editor methods and `handleInputKey` |
| Lossless wrapping with source offsets | Hocus input layout | `text.wrapInput` |
| Styled Markdown lines and strike-through | Hocus transcript composition | `markdown.lines`, `StyledText.content`, and `strike` |
| Fuzzy scores and character indices | Strider, file-finder, session-manager, layout-manager | `text.fuzzyFilter`, using the existing Rust matcher |
| PNG transmission and placeholder rows | Hocus animated wand | `ui.image`, using the existing Kitty sequence builders |
| Random UUID request IDs | Session-manager, plugin-manager | `randomUUID`, using the existing host UUID library |
| Multiple embedded source modules | Large builtin ports and shared modules | Confined relative imports from the compiled asset tree |

The existing Deno commands already cover filesystem metadata/list/write/append/remove/mkdir,
process execution and streamed output, HTTP, pane/session actions, data persistence and watches,
plugin messages and pipes, permissions, and subscriptions. Persistent named workers already support
independent state, asynchronous host replies, FIFO jobs, and parent cancellation. Themed components,
Markdown, Unicode widths, and monotonic time also existed before this audit.

The audit covered all 18 production Wasm builtins. `shared-test` and `fixture-plugin-for-tests`
remain test harnesses, and `hocus-proto` is a Rust library rather than a plugin.

| Builtins inspected | Existing subsystem coverage |
|---|---|
| tab-bar, compact-bar, status-bar | State events, key/action rendering, themed components, Unicode widths, persistent cache files |
| about, quit-confirmation, multiple-select, configuration | Key/mouse events, controls, timers, pane actions, rebinding and configuration |
| session-manager, plugin-manager, layout-manager | Session/plugin/layout commands and events, plus the new fuzzy matcher and UUID helper |
| strider, file-finder, link | Scans, filesystem watches, metadata/list/read/remove, folder changes, editor actions, pipes |
| hocus | Streamed HTTP/commands, revision-guarded data, plugin authoring, workers, plus the new editor, Markdown, file, and image APIs |
| hocus-agentd, hocus-agents, hocus-new-agent, hocus-store | Headless lifecycle, agent/pane events, host commands, shared data, pipes, project configuration, forms |

The filesystem addition preserves the 8 MiB per-call limit and the old whole-file failure behavior.
It does not grant access outside existing roots. Ranged reads reopen files and do not promise a snapshot.
Embedded imports remain confined to compiled sources. No npm, remote imports, raw Deno I/O, or unrestricted workers were added.

This prepares the subsystem for ports. It does not replace the remaining Wasm builtin implementations
or prove their complete UI behavior equivalent. Each port still needs its existing behavior and snapshot tests.
Plugin-specific Rust algorithms and data models must be translated or vendored as local TypeScript modules.
Native macOS execution remains unverified here.

Verification: `cargo x ci check` passed on Linux after the fixes, including format, lint,
shipped TypeScript checks, all 8,301 enabled unit tests (51 skipped), doctests, and all 446 integration tests.
The complete gate took 219 seconds. The new graphics integration test sends Hocus's actual PNG bytes
from a TypeScript plugin through the shared renderer to terminal output. Large-file tests exercise
permissions, confinement, symlinks, EOF, exact offsets, and nonblocking FIFO rejection.
UI tests compare native rendering and editor behavior, including a 10,000-path fuzzy search.
Loader and checker tests cover embedded graphs, missing imports, source maps, and offline authoring.

## Background workers (2026-09-21)

Named `workers` callbacks now run in persistent Deno isolates on dedicated threads.
`postMessageTo` queues jobs in order and `postMessageToPlugin` returns `CustomMessage` to the owner.
Async callbacks can await host replies. Workers share permissions and retain the initiating client.
Parent disposal cancels and joins its workers. Late replies carry the old generation and are discarded.

Linux regressions cover independent state, FIFO order across timers, permission changes, callback errors,
UI responsiveness during a busy loop, reload, unload, disconnect, shutdown, and shared-client ownership transfer.
`deno_background_workers_await_real_commands_and_preserve_state` verifies real commands in a complete session.
The historical worker exclusions below describe the state before this change.

## Parity closure (2026-09-11)

An audit of what Lua and Wasm plugins still got that Deno plugins did not, made against the
host, the guest SDK and the tooling separately, found no missing command, event or permission
and no runtime-specific arm in the dispatcher. What it found instead is closed in this chunk.

Host: a **content-keyed transpile cache** (`loader.rs`; path, source digest and declarations
mode; builtins pinned, the rest LRU-bounded to 256 entries and 64 MiB) so a module is compiled
once rather than once per module per client per reload; **live output and byte replies as
`Uint8Array`** built by the host through the counted allocator (`limits::CountedAllocation`,
`RuntimeInstance::call_event`, `Reply`) instead of a JSON number per byte; the **permission
denial** split, as in Lua, between a request awaiting its answer and no request at all;
`console` routed to the plugin's log from module top level; `CheckPlugin` in `Syntax` mode
permissionless like `CheckLuaPlugin`; the CLI printing lint warnings for TypeScript as well as
Lua (`plugin_authoring::check_plugin_report`); the four data-store lint rules and the
nearest-permission suggestion ported to `analysis.rs`; and `ChangeHostFolder` no longer building
a WASI context for a script plugin.

SDK: `zellij.event(raw)` and `zellij.onEvent(type, handler)` decode the tuple, byte, key, mouse,
filesystem and data-change events into named fields, additively - `update`, `on`, `Event` and
`EventPayloads` are unchanged; `data.REV_ABSENT`, `getFocusedTab`, `getFocusedPane`,
`runCommandInPane`, `StyledText.toString()`, widgets accepting the raw `{Key}`/`{Mouse}` event,
and a named `host.webRequest` form; `console` declared for the checker.

Tooling and docs: Hocus's `plugin_docs` serves both guides through one heading table
(`plugin_docs.rs`, formerly `lua_docs.rs`; TypeScript sections unprefixed, Lua ones `lua_*`)
and `sdk.d.ts` in seven parts; `plugin_check`'s help, the transcript elision notes and a dead
load-failure string no longer assume `.lua`; the TypeScript guide gained Events, Permissions,
the filesystem roots, revision guards, shared namespaces, Hocus data, Work queues, Rendering,
Hooks, Errors, restrictions and a Lua porting table, and every ```` ```ts ```` block in it is
compiled by `guide_code_blocks_typecheck_offline`; `deno/CLAUDE.md` records the runtime's
invariants; the default config names the TypeScript hooks by alias; the vendored compiler's
provenance is a test; and a remote `https://…/plugin.ts` is refused before download.

Tests added: the loader cache and spawn-count tests, byte-exact 64 KiB streaming and reply
round trips, the budget charge of a host-built buffer, the three denial messages, the syntax
check permission, CLI warnings for both runtimes, one analysis test per lint rule plus the
quiet examples, the filesystem roots (each root, climbing, symlinks, `/plugins`,
`FullHdAccess`, `ChangeHostFolder`), watch and scan events across reload, user-plugin-dir
resolution and reload by absolute path, un-normalized configuration, key-driven runs of the
playground, workflow, dashboard, forms and hocus-todos examples, the decoder and `onEvent`
(SDK and types), the small helpers, both guides' coupling tests, the SDK part markers, the
elision of TypeScript names, the provenance hashes and the remote-url refusal. Two Hocus caps
moved for the larger index and tool schema: the `plugin_docs` index bound 4,300 to 5,400 bytes,
the fully loaded tool payload bound 37,500 to 38,200, and the docs-query grain from a quarter to
a third of the cap. The two default-config snapshots and the five `zellij setup` snapshots were
accepted for the three new aliases; the `sdk_rust_fixtures` snapshot for the `Uint8Array`
output fixtures.

Still out of scope, by decision: `config.ts` (stated in `LUA_CONFIG.md`), guest workers, remote
script plugins (refused), async iterators over streams, and native macOS verification.

Verification: the focused suites ran after each step (`cargo x test -p zellij-plugin-host -E
'test(/deno/) | test(/checker/) | test(/analysis/)'`, `cargo x test -p hocus`, the config, CLI
and plugin-manager crates, `zellij-runtime` and `zellij-types` for the url refusal). The first
complete `ZELLIJ_TEST_WAIT_TIMEOUT_MS=20000 cargo x ci check` lost one integration test to
this chunk - `hocus_writes_a_lua_plugin_and_loads_it` waited for the quickstart's old closing
line, and was corrected - and three to load: the hegel health check in
`zellij-session::warped_tab::test::a_tab_with_stacks_keeps_its_shape_at_a_size_it_was_never_arranged_for`,
`hocus_agent_hooks::assistant_messages_reach_the_timeline_and_history_search`, and
`stacked_pane_list::fullscreen_suspends_the_list_and_restores_it`, each of which passed alone.
The second run was red on `format --check` (a long line in the corrected test) and
`stacked_pane_list::splitting_a_new_pane_reforms_the_list_around_it`, the flake this file
already names, which again passed alone. The third complete run **passed** format, lint, all
8,123 enabled unit tests (47 skipped), doctests, and all 395 integration tests at the gate's
16-worker setting, in 123 seconds on Linux. No further snapshots were accepted. The manual
check in the plan (a second client against a multi-module plugin, a live `onOutput` chunk
observed as a `Uint8Array`) was not run in a terminal here; the plugin-thread tests
`deno_two_clients_share_one_transpile_and_a_reload_of_identical_source_spawns_nothing` and
`deno_streaming_output_arrives_as_uint8array_and_a_64_kib_chunk_round_trips_byte_exact` cover
the same two claims against a real plugin thread. Native macOS remains unverified.

## SDK ergonomics parity (2026-09-10)

This chunk gives the Deno SDK the Lua prelude's conveniences. `zellij.host` names every
`PluginCommand` with its first letter lowercased and dispatches through `command()`, so
validation, the permission pre-flight, the `RunCommand` sixth slot, the permission queue,
`Unsubscribe` filtering, and tracing are unchanged. The fifteen tuple commands Lua names in
`tuple_fields` accept one object of camelCase fields; omitted fields become `null`.
`registerDefaultKeybindings` and `zellij.action` emit the same KDL as the Lua prelude, with
1-based `__zellij_default_keybinding:<n>` handler pipes that the SDK consumes before `pipe`.
Root helpers add `keyName`, `paneKey` (the `terminal_3` spelling of `PaneRenderReport`),
`mouse`, `bytes`, `terminalPane`, `pluginPane`, `write`, `writeToPaneId`, `readPluginLog`,
`openTerminal`, `closeSelf`, `setSelectable`, and `text.sanitizeBytes`. `PostMessageTo` is
refused by the SDK before dispatch, as Lua refuses it by name; the host's cache for messages to
a worker that has not started stays for Wasm. Permission inference recognises
`zellij.host.requestPluginPermissions` and outlines `registerDefaultKeybindings`.
Raw command and event shapes are unchanged. No Rust runtime change was needed.

Tests: `host_sdk.rs` (inventory against `CommandType::iter()`, named/positional/array forms,
refusal in every form, helper payloads, Lua-identical KDL parsed back with the host's parser,
reserved-pipe dispatch and cleanup, failed registrations), checker fixtures for `HostCommands`,
named args, keybinding and helper types, the analysis inference test, and three plugin-thread
tests including `deno_default_keybindings_register_after_the_reconfigure_grant_and_dispatch_their_reserved_pipe`,
which captures the server's `RebindKeys` with the rewritten plugin id. `example-plugins/keybindings.ts`
is served as part of the new Hocus `deno_commands` section.

Verification: the focused `cargo x test -p zellij-plugin-host -E 'test(/deno/) | test(/analysis/) | test(/checker/)'`
run passed all 187 selected tests, and the 24 Hocus `lua_docs` tests passed. Two Hocus size
caps moved: the `plugin_docs` index bound (4,100 to 4,300 bytes) and the fully loaded tool
payload bound (37,000 to 37,500 bytes), each by one section name. Three complete
`ZELLIJ_TEST_WAIT_TIMEOUT_MS=20000 cargo x ci check` runs each passed format, lint, doctests,
and (in the third) all 8,077 enabled unit tests (47 skipped). No run was fully green at 16
integration workers: the first and third lost `hocus_agent_picker::the_panel_fills_the_viewport_on_ctrl_p_f_and_comes_back`
to a viewport-restore timeout, and the second lost the hegel health check in
`zellij-session::warped_tab::test::a_stacks_members_arrive_in_the_order_they_were_stacked`.
Both predate this chunk and both pass alone (the session test five times in a row). A separate
`cargo x integration-test -j 4` then passed all 395 integration tests. No snapshots were
accepted or changed. Native macOS remains unverified.

## Live-output parity (2026-09-09)

This chunk adds synchronous `runCommand({onOutput})` callbacks, typed raw
`SubscribePaneOutput`, `PaneOutput`, and `RunCommandOutput`, and the sixth
`RunCommand` field. Both raw command forms preserve existing five-field calls.
Command chunks and completions carry the same Deno generation. Pane chunks carry
the subscribing instance's generation, even when it observes an existing pane.
Disconnect clears only that client's subscriptions. Reload clears registrations
after `BeforeClose` and before the replacement loads. Unload also clears them
after guest cleanup.

The focused regressions cover concurrent commands, binary stdout/stderr, original
clients, unchanged raw events, token stripping, cancellation, callback exceptions,
and listener retention and cleanup. A real shell command must deliver both live
streams before the test releases it to exit. Its final result is capped, while
live delivery contains all bytes and precedes completion. Screen tests cover
client removal, replacement subscriptions, explicit unsubscribe, and Lua/Wasm
routing. Guest tests reject old command and pane chunks after reload. Type checks
compare both raw forms and events against serialized Rust fixtures.

The initial focused run failed only the stale wire snapshot after the three
stale Rust tuple sites were repaired. Snapshot review included the existing
`CheckPlugin` wire-number shift beside the new pane subscription. A new lifecycle
test then exposed missing subscription cleanup in the separate reload path.
That path now clears subscriptions before replacement.

The final focused run passed all 210 selected tests, including Hocus's
`no_section_is_anywhere_near_the_tool_output_cap`. The first full gate passed
format, lint, doctests, and all 391 integration tests. Its unit phase exposed four
stale tests: three missing Deno instruction-context samples, an old `RunCommand`
snapshot, and two Lua tests looking up the removed builtin instead of the retained
Lua fixture. All four failures reproduced alone. The repaired tests passed in the
210-test run. The final `cargo x ci check` passed format, lint, all 8,041 enabled
unit tests (47 skipped), doctests, and all 391 integration tests with 16 workers.
The gate completed in 131 seconds on Linux.

Native macOS,
real SSH/Firecracker sequencing, and dedicated Deno filesystem notifications
remain unverified here. Guest workers, async iterators, awaited output callbacks,
process cancellation, and remote module support remain outside this chunk.

## Builtin and example migration (2026-09-08)

All shipped script builtins and reference examples now use TypeScript. `agent-policy`,
`swamp-bridge`, `swamp-workflow`, and `agent-exit` keep their builtin URLs. The last
two import the registered embedded `zellij:workflow-view` module, which has no
filesystem fallback and participates in semantic TypeScript checking. Hocus's
quickstart and authoring guidance recommend explicit `.ts` filenames.

Lua configuration, user plugins, the Lua checker, and extensionless Lua filenames
remain supported. Bridge delivery records retain their namespace and schema.
Tests cover authenticated and denied callers, duplicate claims, revision conflicts,
agent and terminal completion, and reloading Deno over records written by Lua in
the real SQLite store without dispatching work again. Shared-view tests cover
question/poll races, focus, keyboard and mouse input, previews, small panes,
scrolling, and restart guards. Both whole-session Swamp exit tests use the migrated
TypeScript builtins. Every shipped TypeScript builtin and example passes semantic
checking; every reference example also loads and renders in Deno.

The historical Lua preview failure recorded below was reproduced and corrected in
the test: its question offered only a return choice, but the test tried to submit
free text. The corrected test selects that choice and passes; Lua behavior is
unchanged. Historical red gate results below do not describe the current checkout.

## Capability matrix

Test names are in the linked files. The full repository gate is recorded below.

| Capability | Status and evidence |
| --- | --- |
| Numeric requests and replies | **Verified.** [Transport tests][transport-tests] cover safe integers, i32/u32 boundaries, i64/u64 BigInts, fractions, non-finite optional guards, executable objects and bounded conversion. `deno_integer_commands_keep_values_and_revision_guards_through_the_store` exercises real data writes, replies and stale revisions. |
| Allocation quotas | **Verified on Linux.** [Quota subprocess tests][quota-tests] cover heap, ArrayBuffer, SharedArrayBuffer, encoding, caught exceptions and checker exhaustion. `deno_exhaustion_disposes_the_guest_and_reload_recovers_in_a_live_session` checks host cleanup, another responsive plugin and a healthy replacement. This is quota enforcement, not protection against arbitrary native V8 crashes. |
| Command, reply, event and permission declarations | **Verified inventory and representative shapes.** [Checker tests][checker-tests] compare TypeScript keys with Rust enums and the decoder, independently inspect Wasm shim reply readers, compile serialized Rust fixtures, and snapshot wire definitions. Positive and negative checks exercise both command forms and helper options. Declarations are handwritten. |
| Permissions, identity, errors and absence | **Verified representative paths.** [Guest tests][deno-tests] observe permission grant/denial, protected effects, identities, missing metadata/panes and invalid layout/data errors. The shared dispatcher retains all permission checks. Exhaustive guest testing of every permission/command pair is not claimed. |
| Filesystem and project configuration | **Verified.** `deno_filesystem_helpers_keep_binary_data_confinement_and_config_revision_checks` covers binary and UTF-8 data, read caps, metadata absence, ordering, mutation success, symlinks/traversal at both rename endpoints, and stale project source refusal. |
| Persistence helpers on SQLite and PostgreSQL | **Verified.** `deno_data_helpers_preserve_values_rollback_conflicts_watch_disposal_and_reload_persistence` and the explicit PostgreSQL variant exercise tagged values, ownership fields, two authors' revisions, whole-batch rollback, predicates, catalog, watches, disposal and reload persistence. Different plugin URLs retain separate private namespaces; a Lua guest reads Deno's explicitly shared data. |
| Pending data watches | **Verified on reload.** `deno_reload_disposes_host_watches_and_rejects_their_queued_deliveries` intercepts actual host replies, checks generation ownership and removal of the old watch, then delivers old/current events to the replacement. Shared-store writes from other server processes still do not wake local watches. |
| Buffered commands and HTTP | **Verified.** The mixed session runs a real shell with Unicode stdout, stderr and exit status. `deno_observes_real_buffered_http_and_ordered_stream_bytes` exercises local HTTP through background dispatch, decoding and guest observation. Cancellation rejects the JavaScript wait; started host work may finish. |
| Event-based HTTP streaming | **Verified representative paths.** The HTTP session test uses `webRequest({url, streamId})` and checks the leading response, chunk/end ordering and bytes. `deno_reload_reuses_stream_id_without_delivering_or_cancelling_the_old_generation` checks an in-flight stream and replacement using the same stream ID. The wrapper retains its cancellable response promise; callers subscribe to chunk events. An async-iterator API is excluded. |
| Pipes and hooks | **Verified representative paths.** Existing queue/plugin harnesses cover first headless CLI delivery, reload and reply routing. `deno_headless_hooks_deliver_the_first_payload_after_async_permissions_and_reload` observes real tab changes from first/repeated/reloaded hooks. |
| Queue ownership | **Verified.** [Queue tests][queue-tests] cover a Deno enqueuer reload with running and pending work. Jobs continue, addressed start/finish events remain with the original Deno generation, and the replacement can query status. Lua/Wasm routing and broadcast queue-state events remain unchanged. |
| Pane/tab, layout/session, configuration and recordings | **Verified representative requests.** `deno_host_queries_configuration_layouts_recordings_and_cross_session_warp` checks pane/tab identities and absence, valid/invalid layouts, session lists and layout dump, initial keybindings, reconfiguration via its event, recording queries, and an actual terminal moved between sessions. |
| Async ordering, cancellation, reload and clients | **Verified existing contract.** [SDK and guest tests][deno-tests] preserve ordered callback starts, origin after await, shared-client permissions, reused client IDs, stale timers/commands/renders, disconnect during load, idle cleanup and interruption of runaway execution. |
| Local modules and offline checker | **Verified.** [Loader][loader-tests] and [checker tests][checker-tests] cover local imports, source maps, syntax versus semantic checking, dependency diagnostics, confinement, non-execution and 8 MiB/file plus 64 MiB/graph limits. |
| CLI and Hocus authoring | **Verified.** Shared filename rules support `.lua`, `.js`, `.mjs`, `.ts`, `.mts` and extensionless Lua. [The actual Wasm Hocus test][hocus-test] checks TypeScript syntax/semantics, sibling imports, write/edit refusal, backups, list/read/search/outline, inferred grants and reload. Existing Lua authoring remains supported. |
| Parsed permission inference | **Verified.** [Analysis tests][analysis-tests] cover literal arrays on SDK bindings, imported aliases, namespace imports and raw permission commands. Comments and shadowed names grant nothing. Computed requests warn and use normal runtime approval. Imported source cannot silently add grants. |
| Legacy filesystem scan/watch events | **Raw commands and typed events available.** Existing host tests cover the shared implementation; dedicated Deno change-notification and disconnect coverage is not established by the new data-watch tests. |
| Remaining host command families | **Typed raw access and named helpers available.** `zellij.host.<command>` covers every variant; the representative session test does not exhaust these families. |
| Command sugar and named arguments | **Verified.** `host_sugar_covers_the_rust_command_inventory_and_is_frozen`, `host_sugar_builds_unit_positional_array_and_named_payloads`, and `sdk_host_sugar_types_derive_from_command_args` cover the inventory, all fifteen named forms, passthrough, unknown fields, and the queue and filter paths. |
| Default keybindings | **Verified on Linux.** `default_keybindings_emit_lua_identical_kdl_and_dispatch_reserved_pipes` parses the emitted KDL with the host's parser; the plugin-thread test captures `RebindKeys` with the rewritten plugin id and dispatches the reserved pipe, including a stale index; the denied variant sees the pre-flight refusal. |
| Prelude helper parity | **Verified.** `helpers_encode_writes_spell_pane_keys_and_normalize_keys_mouse_and_bytes` and `sdk_keybinding_and_helper_types_check_modes_actions_and_pane_keys`. Not ported: `snake_case`/`pascal_case` name conversion, `zellij.null`, and event-shape normalization; raw shapes stay as declared. |
| Agent helpers | **Verified on Linux.** spawn details/ID, manifest cache, event subscriptions, bracketed paste, cancellable state/event/turn waits, and prompt ordering. SDK tests cover transitions, cancellation, validation, and dispatch cleanup. Real plugin tests cover agent event delivery and reload. |
| Hocus completion client | **Verified on Linux.** exact daemon discovery, typed transcript/tool-call round trips, reply provenance, request byte limits, errors, deadlines, cancellation, and cleanup. SDK tests and Rust-backed TypeScript fixtures cover the protocol. Real dispatch tests use a deterministic local peer and check permissions and reload isolation. |
| Pane command sequencing | **Verified on Linux.** `pane_sequence_reuses_pane_matches_fast_and_stale_events_and_appends`, `pane_sequence_validation_failure_policies_and_observation_cancellation`, `pane_sequence_cwd_preserves_targets_and_resolves_only_at_dispatch`, `pane_sequence_timeout_cancel_grace_refusal_and_cleanup`, `pane_sequence_types_check_steps_options_results_and_primitives`, `pane_sequence_close_policies_and_exits_cancel_remaining_signals`, and the SDK-backed example regression passed. Whole-session tests verify pane reuse, child cancellation, and reload isolation (see current verification). |
| Live output | **Implemented.** `runCommand({onOutput})` delivers binary stdout/stderr chunks with synchronous callbacks. Typed raw pane observation and generation-safe routing cover reload and client cleanup. Async iterators remain outside this contract. |
| Guest workers | **Implemented.** Named callbacks run in separate persistent isolates and threads. `PostMessageTo` queues jobs in order, including async completion. `PostMessageToPlugin` returns a generation-scoped `CustomMessage`. Workers share permissions, retain the originating client, and stop with their owner. See [Background workers](DENO_PLUGINS.md#background-workers). |
| Tracing | **Implemented and SDK-tested.** Opt-in `zellij.trace()` and merged `trace` configuration record command/event/pipe names and existing async lifecycle markers through the Deno log. Payloads and exception text are excluded. |
| Remote module loading | **Not supported.** Remote URLs and package imports are rejected by the Deno loader. This restriction does not prohibit commands in remote panes. Remote pane sequence execution has not been verified against SSH or Firecracker here. |

RPC now has shared version-1 dispatch through `zellij.rpc.call()` and `zellij.rpc.handle()`.
SDK regressions cover results, errors, provenance, cancellation, deadlines, cleanup, message limits, and ordinary pipes.
Real plugin tests exercise Deno and Lua peers, URL launch/configuration, ID addressing, reverse calls, permission denial, and old replies after reload.
## Public contracts

`command(name, payload)` and `command({Name: payload})` use known Rust command names,
wire fields and corresponding result types. Unit commands accept `command('GetPluginIds')`
and `command({GetPluginIds: null})`. Commands without an immediate reply return
`null`. The declarations identify byte replies; no currently supported command deliberately
returns undecoded protobuf (`OpaqueReplyCommand = never`). An independent shim-reader
inventory prevents a missing decoder from silently becoming an opaque response.

Safe integral inputs become JSON integers. Other finite numbers retain their floating-point
tag so Rust rejects fractions and unsafe numbers in integer destinations. BigInts must fit
i64 or u64; Rust checks narrower ranges. Replies retain the existing decoder: safe integers
are `number`, wider integers are `bigint`. Invalid optional revisions cannot become `null`.
Conversion accepts plain data, arrays and Uint8Array; it rejects proxies, accessors, cycles,
unsupported values and non-finite numbers without running guest getters. It bounds nesting
to 128 and conversion to a 64 MiB message budget.

Each plugin and checker has an on-demand 1 GiB V8 heap budget and a separate 1 GiB live
ArrayBuffer backing-store budget. Exceeding either permanently fails the isolate, including
when JavaScript catches an allocation exception. Heap termination receives temporary
headroom to unwind. Host cleanup removes watches, timers and continuations; exhausted
instances do not run guest cleanup. Resizable/growable buffers and guest WebAssembly memory
are unavailable because they bypass the counted backing-store allocator. Local sources are
bounded before parsing. These limits do not isolate arbitrary native V8 crashes.

Synchronous helpers stay synchronous. Filesystem mutations throw on failure and otherwise
return normally. Project writes require the caller's nullable `expectedSource`. Data helpers
preserve supplied revisions and ownership fields, never re-fetch revisions or retry conflicts.
Fetch/watch snapshots return `{records, raw}`; blob records use Uint8Array. Defaults remain
schema version 1, non-destructive definitions, fetch limit 100, offset 0 and row-mode watches.
Disposing a watch removes its host registration and listener; repeated successful disposal is
harmless. Private namespaces remain URL-based and shared namespaces still require permission.

`CheckPlugin` adds a runtime-aware syntax/semantic report under `ReadFiles` while retaining
`CheckLuaPlugin`. Hocus checks proposed syntax before approval and applies inferred grants
only after a successful write. Explicit JS/TS `plugin_check` semantically checks confined local
imports. Extensionless authoring remains Lua. See [the Deno guide][guide] for the public API.

## Current verification (2026-09-09)

The seven named SDK, TypeScript, and example tests above passed with
`cargo x test -p zellij-plugin-host -E 'test(pane_sequence_)'`.
The broader `cargo x test -p zellij-plugin-host -E 'test(deno)'` run passed all 157 selected tests.
`cargo x integration-test -E 'test(deno_pane_sequence_)'` passed both
`deno_pane_sequence_reuses_one_host_pane_and_cancels_its_child` and
`deno_pane_sequence_reload_does_not_advance_the_old_steps`. These exercise real host
command/event routing with the existing deterministic PTY backend, not an external shell.
Initial fixture failures (invalid KDL and missing replacement permissions) were corrected.
The first full gate passed format, lint, doctests, and all 390 integration tests.
Its unit phase passed 7,984 of 7,985 tests. `lua_docs::tests::no_section_is_anywhere_near_the_tool_output_cap`
failed because the expanded Deno guide exceeded Hocus's response limit. The guide now
exposes a separate `deno_async` section, with shorter index descriptions. All 29 focused
documentation and sequencing checks passed after that change.
The final `ZELLIJ_TEST_WAIT_TIMEOUT_MS=20000 cargo x ci check` **passed** formatting,
lint, all 7,985 enabled unit tests (47 skipped), doctests, and all 390 integration tests
at the gate's 16-worker setting. This was a complete gate rerun, not an isolated retry.
No snapshots were accepted.
Native macOS remains unverified. Exhaustive command/permission combinations, filesystem
notification disconnect behavior and remote SSH/Firecracker sequencing
remain outside the verified coverage. Existing capability evidence above does not establish these paths.

## Arbitrary-pane exit verification (2026-09-09)

`waitForPaneExit` now observes terminal panes opened by the CLI or another plugin.
It subscribes to pane metadata before querying `GetPaneInfo`. An already-exited pane
resolves with its recorded status, and a missing pane resolves as closed.
Later metadata omissions do not imply closure. `runInPane` and `paneSequence` still
match their command contexts and ignore metadata from earlier exits.
The public signature and existing `ReadApplicationState` requirement are unchanged.

All 191 focused Deno, pane-sequencing, and Hocus documentation tests passed,
including the documentation size guard. SDK regressions cover broadcast-only exits,
recorded and missing panes, unrelated and plugin IDs, null status, duplicate events,
cancellation, query errors, originating clients, and observer cleanup.
All three focused pane integration tests passed. The new regression creates a held
command pane through the CLI, starts a separate Deno observer, waits for readiness,
and exits the controlled PTY with status 23. The observer verifies that the pane
remains held. No plugin events are injected. The initial fixture run failed because
keyboard focus remained on the command pane. Explicit observer focus corrected it.

The first `ZELLIJ_TEST_WAIT_TIMEOUT_MS=20000 cargo x ci check` passed format, lint,
all 7,993 enabled unit tests (47 skipped), and doctests. Integration passed 390 of 391.
`stacked_pane_list::a_stack_list_dissolves_into_a_normal_pane_at_one_member` showed
`[0]` instead of `[1]` in the tab count and passed alone at `-j 16`.
The second complete gate **passed** format, lint, all 7,993 enabled unit tests
(47 skipped), doctests, and all 391 integration tests at 16 workers.
No snapshots were accepted. These runs used Linux and the deterministic PTY backend.
Native macOS and real SSH/Firecracker observation remain unverified.

## Tracing verification (2026-09-09)

The focused Deno, authoring-log, and Hocus documentation run passed all 174 tests,
including the documentation size guard. New SDK checks cover configuration precedence,
runtime toggles, fresh runtime state, raw and internal commands, consumed pipes, event
names, originating clients, async success/rejection/cancellation, dispatch failures,
watch disposal, escaped and bounded names, and payload exclusion. The TypeScript checker
accepts the optional boolean API and rejects other argument types.

The required `cargo x ci check` passed formatting, lint, all 7,989 enabled unit tests
(47 skipped), doctests, and all 390 integration tests with local socket access.
No snapshots were changed. Native macOS and real SSH/Firecracker verification remain follow-ups.

## Historical gate runs (2026-09-08)

These results describe earlier checkouts. A passing isolated retry does not make a red
full gate green. The later correction to the Lua preview test is recorded in the migration section.

Migration verification: `ZELLIJ_TEST_WAIT_TIMEOUT_MS=20000 cargo x ci check` passed
formatting, lint, 7,976 enabled unit tests, doctests, and all 388 integration tests.
After adding the pane-sequence cancellation regression, the final gate passed
formatting, lint, all 7,977 enabled unit tests (47 skipped), and doctests. Its
integration phase passed 387 of 388 tests. The untouched
`stacked_pane_list::splitting_a_new_pane_reforms_the_list_around_it` snapshot lacked
the tab's `[2]` count; pane content matched. It passed immediately alone at `-j 16`.
No snapshots were accepted. That complete gate run was red despite
the earlier green gate and the successful isolated recheck.

RPC verification: focused tests passed. `ZELLIJ_TEST_WAIT_TIMEOUT_MS=20000 cargo x ci check` passed format, lint, doctests, and all 386 integration tests.
The unit phase passed 7,936 tests and failed `plugins::lua::unit_tests::swamp_output_preview_names_the_saved_version_and_keeps_return_actionable`.
At that point, the Lua test also failed alone at its submit-focus assertion. That gate was red.

Agent/completion verification (2026-09-08): all new SDK, TypeScript, Rust fixture, and real-dispatch tests passed.
`cargo x ci check` passed format, lint, and doctests. The unit phase passed 7,944 of 7,946 tests.
At that point, the Lua preview test named above still failed alone at its submit-focus assertion.
`plugins::plugin_tests::run_and_pipe_commands::unblock_input_plugin_command` had an extra filesystem event in its snapshot and passed alone.
The integration phase passed 385 of 386 tests at the gate's 16-worker setting.
`stacked_pane_list::floating_the_visible_member_promotes_its_neighbor` showed tab count `[1]` instead of `[2]` and passed alone at `-j 16`.
No snapshots were accepted. That full gate was red. The Lua preview test was the reproducible blocker.

## Historical verification before pane sequencing

All listed runs use the existing Cargo xtask, plugin and whole-session harnesses on Linux.
Quota exhaustion runs only in supervised subprocesses with reduced internal test budgets.
The final gate uses `ZELLIJ_TEST_WAIT_TIMEOUT_MS=20000` so failed unit observations report state before nextest kills the test. Integration concurrency remains 16. No CI configuration was added.

| Check | Result |
| --- | --- |
| Focused transport, quota, SDK, checker, loader, analysis and generation tests | Passed; names and boundaries are listed above. |
| Filesystem, persistence, queues, hooks and headless pipe guest tests | Passed on Linux. |
| Whole-session buffered/streamed HTTP, replacement streams and mixed runtimes | Passed on Linux. |
| Pane/session/configuration/recording/cross-session guest | Passed 20 consecutive runs, including initial keybindings. Subscription and resize metadata no longer lock the guest on the thread answering its synchronous layout query. |
| Actual Wasm Hocus JS/TS authoring | Passed on Linux. |
| `cargo x postgres-test --build` then `cargo x postgres-test --test` | PostgreSQL test service ready; 104 store tests passed. |
| `ZELLIJ_PLUGIN_DATA_TEST_URL=<test-url> cargo x test -p zellij-plugin-host --run-ignored only -E 'test(deno_data_helpers_on_postgres)'` | Passed, including Lua/Deno shared data. |
| `cargo x ci check` | Passed: format check, lint, 7,825 unit tests, doctests and all 376 integration tests. |
| Native macOS runtime and checker | Not run; no macOS host is available in this session. |

## Historical allocation finding

Before quotas, an isolated child using the pinned Deno Core and a 32 MiB old-space cap
terminated with signal 5 and `Fatal JavaScript out of memory: Reached heap limit`.
The old embedding did not return a Rust error. The current subprocess regressions require
controlled failure and a successful supervising process; a fatal exit fails the test.

## Historical transport findings and reproductions (before the boundary repair)

### Integer boundaries

The request path is `JS value → serde_v8 → serde_json::Value → PluginCommand`.
The first conversion uses `deserialize_any` in the locked `serde_v8` 0.314.0 dependency.
It recognizes uint32 and int32 values as integers, treats other numbers as f64, and rejects BigInt.
`serde_json::from_value` then rejects those f64 values for Rust integer fields, even when the value is mathematically integral.

The reverse path uses `serialize_i64`/`serialize_u64`.
Those emit JS numbers inside the safe-integer interval and BigInt outside it.
The same conversion is used for callback arguments, so large integers can appear in events as well as command replies.
This is wider than the SDK's declared `Json` return type.
See [Deno op and callback conversion][runtime], [integer ID definitions][ids], and [data values/revisions][data-types].

These results came from standalone probes linked against this checkout's existing Deno Core and type libraries.
The probes deserialized real `PluginCommand` values without dispatching host operations.
Measured endpoints agree with the interval implied by the converter's source.

| JS input | Intermediate JSON / result for an integer destination |
| --- | --- |
| `-2147483649` | f64. Rejected for i64 despite exact JS representation. |
| `-2147483648`, `2147483647` | Integer. Accepted by i64. Negative values correctly fail u64. |
| `2147483648`, `4294967295` | Integer. Accepted by i64/u64. |
| `4294967296` | f64. Rejected for u64, i64, and `usize`. |
| `9007199254740991` (`2^53−1`) | Exact JS number, but f64 JSON. Rejected for integer fields. |
| `9007199254740992` | f64. Rejected for integer fields. |
| `9007199254740993` | Already rounded to `9007199254740992` by JavaScript, then rejected for integer fields. |
| `-9223372036854775808` | Representable as a number but converted to f64 JSON and rejected for i64. |
| `9223372036854775807`, `18446744073709551615` | Rounded JS numbers, then rejected for integer fields. |
| `1n`, `9007199254740991n`, `-9223372036854775808n`, `18446744073709551615n` | All fail with `serde_v8 error: unsupported type`. |
| `0.25` | Accepted by real-valued fields, rejected by integer fields. |
| `NaN`, `Infinity` | Become JSON `null`. Required numeric fields reject null. Optional fields can silently become absent. |

Reverse conversion was checked with integer JSON values `-2147483649`, `4294967296`, and `2^53−1`.
They arrive as JS numbers, but fail an integer round trip through the request converter.
Integer JSON values `2^53`, i64 minimum/maximum, and u64 maximum arrive as BigInt and cannot be sent back.
An f64 value of `0.25` stays a number and round-trips as f64.

Concrete command results:

| Input payload | Observed deserialization |
| --- | --- |
| `DataUnwatch: {watch_id: 4294967295}` | Valid `DataWatchId`. |
| `DataUnwatch: {watch_id: 4294967296}` | Error: floating point, expected u64. |
| `FsRead: {path: "/host/a", max_bytes: 4294967296}` | Error: floating point, expected usize. |
| A `DataApply` put containing `Int: -2147483649` | Error: floating point, expected i64. |
| A `DataApply` put with `expect_rev: 4294967296` | Error: floating point, expected i64. |
| A `DataApply` put with `expect_rev: NaN` | Decodes with `expect_rev: None`. The guard is lost before the store sees it. No store write was performed in this probe. |
| `FsRead: {path: "/host/a", max_bytes: Infinity}` | Decodes with `max_bytes: None`. |
| `SetTimeout: 0.25`, `SetTimeout: 4294967296`, or a data `Real: 0.25` | Valid real-valued fields. |

The safe-integer validation already in `runCommand.maxBytesReturned` does not solve transport above `u32::MAX`.
It does correctly reject non-finite helper options. Preserve that validation.
Do not fix transport by converting every integral number into an integer without considering real-valued fields and invalid ranges.

This minimal diagnostic can run inside the existing Deno `sdk_tests` module in a disposable checkout.
It reproduces the boundary without permissions, files, watches, or data mutations:

```rust
#[test]
fn numeric_transport_audit() {
    let mut runtime = JsRuntime::new(RuntimeOptions::default());
    for expression in [
        "({DataUnwatch: {watch_id: 4294967295}})",
        "({DataUnwatch: {watch_id: 4294967296}})",
        "({DataUnwatch: {watch_id: 1n}})",
        "({FsRead: {path: '/host/a', max_bytes: Infinity}})",
        "({SetTimeout: 0.25})",
    ] {
        let result = runtime.execute_script("audit.js", expression.to_owned()).unwrap();
        deno_core::scope!(scope, runtime);
        let local = v8::Local::new(scope, result);
        let decoded = serde_v8::from_v8::<Value>(scope, local)
            .map_err(|error| error.to_string())
            .and_then(|value| serde_json::from_value::<PluginCommand>(value)
                .map_err(|error| error.to_string()));
        println!("{expression}: {decoded:?}");
    }
}
```

Run with `cargo x test -p zellij-plugin-host -E 'test(numeric_transport_audit)' --no-capture`.
This is an audit diagnostic, not an acceptance test that should preserve broken behavior.
The transport and real-store regressions now assert exact values and rejection of invalid revision guards. The tables above record the former boundary only.

## Status bar migration (2026-09-22)

`zellij:status-bar` now loads the embedded `status-bar.ts` entry point and its local modules.
The public name, layout files, and `classic` configuration option stay the same.
The Rust crate remains a test reference. Installation excludes its Wasm artifact.

`deno::sdk_tests::status_bar` loads the actual Wasm artifact and the embedded TypeScript plugin.
It supplies the same events and controls time, UTC offsets, cache contents, and Wasm entropy.
The tests compare terminal cells, including colors and attributes, redraw decisions, and host commands.
ANSI encodings can differ when they produce the same cells.

The comparisons cover all input modes, normal and locked base modes, missing and custom bindings,
plugin aliases, Unicode keys, indexed and RGB colors, both font settings, and both layouts.
They also cover width transitions, nested sessions, clipboard messages, swap layouts, fullscreen and floating panes,
agent counts and animation frames, clock rollover, reveal deadlines, pointer hover, ribbon clicks, and reconfiguration.
All ten tips are compared at several widths. Random choice sequences are not required to match.
The adjacent `status-bar.test.ts` suite checks cache failures, version changes, selection thresholds, timers, and state transitions.
`status-bar.integration.test.ts` loads the embedded builtin in a real session and checks mode updates and selectability.
Existing integration tests now wait for the tab bar to reflect the final pane and client state before taking snapshots.
Three snapshots now capture settled output: one includes the loaded status bar, and two omit the old single-pane label after a split.
The full gate also exposed a CLI-pipe test race. That test now waits for the blocked-pipe result and keeps its permission cache outside the fixture's watched folder.

The migration exposed three reference defects:

- The plugin action wire format omitted `Copy`. Action number 122 now carries it without a payload.
- The Rust tip cache requested file creation without write access. The cache now opens with read and write access.
- Ribbon emphasis used terminal columns as character indices. Both implementations now use Unicode scalar indices for emphasis.

The installed test harness also used JSON-only command decoding, which rejected filesystem writes containing `Uint8Array`.
It now uses the same command transport as the live Deno host.

Remaining difference: cache write failures return the `quicknav` fallback in TypeScript.
The Rust reference can panic on write failures during cache initialization or random-tip bookkeeping.
Random tip sequences and redundant cache writes also differ. The selection rules and successful final cache contents remain compatible.

Validation on Linux: `NEXTEST_TEST_THREADS=16 cargo x ci check` passed formatting, lint, all 8,352 enabled workspace tests, doctests, and all 450 integration tests.
The workspace run skipped 51 tests that need separate opt-in suites. Integration tests used 16 workers.
An earlier run at default workspace concurrency timed out in `can_subscribe_to_hd_events`; that test passed in isolation and in the final gate with 16 workers.

[guide]: DENO_PLUGINS.md
[architecture]: ARCHITECTURE.md
[render-path]: PLUGIN_RENDER_PIPELINE.md
[types]: ../zellij-types/src/data.rs
[ids]: ../zellij-types/src/internal_ids.rs
[data-types]: ../zellij-types/src/plugin_data.rs
[shim]: ../zellij-tile/src/shim.rs
[runtime]: ../zellij-plugin-host/src/deno/mod.rs
[sdk]: ../zellij-plugin-host/src/deno/sdk.js
[sdk-types]: ../zellij-plugin-host/src/deno/sdk.d.ts
[events]: ../zellij-plugin-host/src/deno/event.rs
[responses]: ../zellij-plugin-host/src/response.rs
[exports]: ../zellij-plugin-host/src/zellij_exports.rs
[bridge]: ../zellij-plugin-host/src/wasm_bridge.rs
[plugin-thread]: ../zellij-plugin-host/src/lib.rs
[loader]: ../zellij-plugin-host/src/plugin_loader.rs
[loader-tests]: ../zellij-plugin-host/src/deno/unit/loader.rs
[checker-tests]: ../zellij-plugin-host/src/deno/unit/checker.rs
[deno-tests]: ../zellij-plugin-host/src/unit/plugin_tests/deno_plugins.rs
[mixed-test]: ../zellij-integration-tests/tests/deno_plugins.rs
[lua-sdk]: ../zellij-plugin-host/src/lua/prelude.lua
[lua-ui]: ../zellij-plugin-host/src/lua/ui.rs
[lua-tests]: ../zellij-plugin-host/src/lua/unit_tests.rs
[store]: ../zellij-storage/src/plugin_data_store/mod.rs
[background]: ../zellij-bus/src/background_jobs.rs
[queues]: ../zellij-plugin-host/src/queues.rs
[queue-tests]: ../zellij-plugin-host/src/unit/plugin_tests/queues.rs
[limits]: ../zellij-plugin-host/src/plugin_limits.rs
[authoring]: ../zellij-plugin-host/src/plugin_authoring.rs
[cli]: ../src/commands.rs
[hocus-tools]: ../default-plugins/hocus/src/tools.rs
[agent-test]: ../zellij-integration-tests/tests/lua_agent_harness.rs

[transport-tests]: ../zellij-plugin-host/src/deno/unit/transport.rs
[quota-tests]: ../zellij-plugin-host/src/deno/unit/limits.rs
[analysis-tests]: ../zellij-plugin-host/src/deno/unit/analysis.rs
[hocus-test]: ../zellij-integration-tests/tests/hocus_plugin_authoring.rs
