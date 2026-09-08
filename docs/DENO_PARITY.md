# Deno capability and verification status

Updated 2026-09-07. The seven requested changes are implemented in this checkout:
exact numeric inputs, runtime/checker quotas, host-path tests and queue ownership,
complete raw SDK declarations, filesystem/configuration helpers, persistence helpers,
and runtime-aware Hocus authoring. The existing asynchronous SDK remains in place.

This is a capability record, not a claim of full Wasm/Lua/Deno parity.
**Verified** below means the stated outcome has passed a named Linux test. Raw access
and compiler inventory coverage do not establish every command's host behavior.
Native macOS verification requires a macOS host and remains outstanding.

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
| Raw HTTP streaming | **Verified representative paths.** The HTTP session test checks response/chunk/end ordering and bytes. `deno_reload_reuses_stream_id_without_delivering_or_cancelling_the_old_generation` checks an in-flight stream and replacement using the same stream ID. A streaming convenience API is excluded. |
| Pipes and hooks | **Verified representative paths.** Existing queue/plugin harnesses cover first headless CLI delivery, reload and reply routing. `deno_headless_hooks_deliver_the_first_payload_after_async_permissions_and_reload` observes real tab changes from first/repeated/reloaded hooks. |
| Queue ownership | **Verified.** [Queue tests][queue-tests] cover a Deno enqueuer reload with running and pending work. Jobs continue, addressed start/finish events remain with the original Deno generation, and the replacement can query status. Lua/Wasm routing and broadcast queue-state events remain unchanged. |
| Pane/tab, layout/session, configuration and recordings | **Verified representative requests.** `deno_host_queries_configuration_layouts_recordings_and_cross_session_warp` checks pane/tab identities and absence, valid/invalid layouts, session lists and layout dump, initial keybindings, reconfiguration via its event, recording queries, and an actual terminal moved between sessions. |
| Async ordering, cancellation, reload and clients | **Verified existing contract.** [SDK and guest tests][deno-tests] preserve ordered callback starts, origin after await, shared-client permissions, reused client IDs, stale timers/commands/renders, disconnect during load, idle cleanup and interruption of runaway execution. |
| Local modules and offline checker | **Verified.** [Loader][loader-tests] and [checker tests][checker-tests] cover local imports, source maps, syntax versus semantic checking, dependency diagnostics, confinement, non-execution and 8 MiB/file plus 64 MiB/graph limits. |
| CLI and Hocus authoring | **Verified.** Shared filename rules support `.lua`, `.js`, `.mjs`, `.ts`, `.mts` and extensionless Lua. [The actual Wasm Hocus test][hocus-test] checks TypeScript syntax/semantics, sibling imports, write/edit refusal, backups, list/read/search/outline, inferred grants and reload. Existing Lua authoring remains supported. |
| Parsed permission inference | **Verified.** [Analysis tests][analysis-tests] cover literal arrays on SDK bindings, imported aliases, namespace imports and raw permission commands. Comments and shadowed names grant nothing. Computed requests warn and use normal runtime approval. Imported source cannot silently add grants. |
| Legacy filesystem scan/watch events | **Raw commands and typed events available.** Existing host tests cover the shared implementation; dedicated Deno change-notification and disconnect coverage is not established by the new data-watch tests. |
| Remaining host command families | **Typed raw access available.** Common dispatch covers keybindings, clipboard, pane placement, actions and other host operations. The representative session test does not exhaust these families. |
| Agent helpers | **Verified on Linux.** spawn details/ID, manifest cache, event subscriptions, bracketed paste, cancellable state/event/turn waits, and prompt ordering. SDK tests cover transitions, cancellation, validation, and dispatch cleanup. Real plugin tests cover agent event delivery and reload. |
| Hocus completion client | **Verified on Linux.** exact daemon discovery, typed transcript/tool-call round trips, reply provenance, request byte limits, errors, deadlines, cancellation, and cleanup. SDK tests and Rust-backed TypeScript fixtures cover the protocol. Real dispatch tests use a deterministic local peer and check permissions and reload isolation. |
| Streaming helpers, pane command sequencing and workers | **Excluded from this work.** Raw commands/events expose applicable host operations, but do not provide these SDK conveniences or CPU parallelism. |

RPC now has shared version-1 dispatch through `zellij.rpc.call()` and `zellij.rpc.handle()`.
SDK regressions cover results, errors, provenance, cancellation, deadlines, cleanup, message limits, and ordinary pipes.
Real plugin tests exercise Deno and Lua peers, URL launch/configuration, ID addressing, reverse calls, permission denial, and old replies after reload.
RPC verification: focused tests passed. `ZELLIJ_TEST_WAIT_TIMEOUT_MS=20000 cargo x ci check` passed format, lint, doctests, and all 386 integration tests.
The unit phase passed 7,936 tests and failed `plugins::lua::unit_tests::swamp_output_preview_names_the_saved_version_and_keeps_return_actionable`.
That unchanged Lua test also fails alone at its submit-focus assertion. The gate remains red.
The historical gate results below predate this addition.

Agent/completion verification (2026-09-08): all new SDK, TypeScript, Rust fixture, and real-dispatch tests passed.
`cargo x ci check` passed format, lint, and doctests. The unit phase passed 7,944 of 7,946 tests.
The existing Lua preview test named above still fails alone at its submit-focus assertion.
`plugins::plugin_tests::run_and_pipe_commands::unblock_input_plugin_command` had an extra filesystem event in its snapshot and passed alone.
The integration phase passed 385 of 386 tests at the gate's 16-worker setting.
`stacked_pane_list::floating_the_visible_member_promotes_its_neighbor` showed tab count `[1]` instead of `[2]` and passed alone at `-j 16`.
No snapshots were accepted. The full gate remains red; the reproducible blocker is the unchanged Lua preview test.

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

## Verification

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
| `ZELLIJ_PLUGIN_DATA_TEST_URL=<test-url> cargo x test -p zellij-server --run-ignored only -E 'test(deno_data_helpers_on_postgres)'` | Passed, including Lua/Deno shared data. |
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

Run with `cargo x test -p zellij-server -E 'test(numeric_transport_audit)' --no-capture`.
This is an audit diagnostic, not an acceptance test that should preserve broken behavior.
The transport and real-store regressions now assert exact values and rejection of invalid revision guards. The tables above record the former boundary only.

[guide]: DENO_PLUGINS.md
[architecture]: ARCHITECTURE.md
[render-path]: PLUGIN_RENDER_PIPELINE.md
[types]: ../zellij-types/src/data.rs
[ids]: ../zellij-types/src/internal_ids.rs
[data-types]: ../zellij-types/src/plugin_data.rs
[shim]: ../zellij-tile/src/shim.rs
[runtime]: ../zellij-server/src/plugins/deno/mod.rs
[sdk]: ../zellij-server/src/plugins/deno/sdk.js
[sdk-types]: ../zellij-server/src/plugins/deno/sdk.d.ts
[events]: ../zellij-server/src/plugins/deno/event.rs
[responses]: ../zellij-server/src/plugins/response.rs
[exports]: ../zellij-server/src/plugins/zellij_exports.rs
[bridge]: ../zellij-server/src/plugins/wasm_bridge.rs
[plugin-thread]: ../zellij-server/src/plugins/mod.rs
[loader]: ../zellij-server/src/plugins/plugin_loader.rs
[loader-tests]: ../zellij-server/src/plugins/deno/unit/loader.rs
[checker-tests]: ../zellij-server/src/plugins/deno/unit/checker.rs
[deno-tests]: ../zellij-server/src/plugins/unit/plugin_tests/deno_plugins.rs
[mixed-test]: ../zellij-integration-tests/tests/deno_plugins.rs
[lua-sdk]: ../zellij-server/src/plugins/lua/prelude.lua
[lua-ui]: ../zellij-server/src/plugins/lua/ui.rs
[lua-tests]: ../zellij-server/src/plugins/lua/unit_tests.rs
[store]: ../zellij-storage/src/plugin_data_store/mod.rs
[background]: ../zellij-server/src/background_jobs.rs
[queues]: ../zellij-server/src/plugins/queues.rs
[queue-tests]: ../zellij-server/src/plugins/unit/plugin_tests/queues.rs
[limits]: ../zellij-server/src/plugins/plugin_limits.rs
[authoring]: ../zellij-server/src/plugin_authoring.rs
[cli]: ../src/commands.rs
[hocus-tools]: ../default-plugins/hocus/src/tools.rs
[agent-test]: ../zellij-integration-tests/tests/lua_agent_harness.rs

[transport-tests]: ../zellij-server/src/plugins/deno/unit/transport.rs
[quota-tests]: ../zellij-server/src/plugins/deno/unit/limits.rs
[analysis-tests]: ../zellij-server/src/plugins/deno/unit/analysis.rs
[hocus-test]: ../zellij-integration-tests/tests/hocus_plugin_authoring.rs
