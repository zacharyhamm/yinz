# Steel plugins

Yinz plugins are Scheme modules executed by `yinz-steel`. The user guides are
`docs/PLUGINS.md`, `docs/PLUGIN_SDK.md`, and `docs/PLUGIN_TESTING.md`.

This directory adapts the interpreter to the plugin host. The engine and guest SDK
live in `yinz-steel/src`; embedded builtin sources live in
`yinz-steel-builtins/assets/plugins`.

## Host authority

All guest host commands must pass through `yinz_exports::run_plugin_command` and
its permission checks. SDK helpers compose those commands in Scheme. Native
bindings can implement pure model transformations, text processing, or local UI
state; they must not provide another route to files, processes, or the network.
Keep command and response coverage when adding a command.

`dispatch` rejects a command before execution when its permission is absent.
An unanswered permission request must produce a different diagnostic from a
permission that was never requested. Parent and worker instances share one
permission coordinator. Only its queue head reaches the host, and later requests
include earlier grants because the host replaces the permission set. A private
completion resolves the requesting deferred exactly once. Ordinary
`PermissionRequestResult` observers cannot consume that completion.

The plugin environment supplies the acting client. Shared plugins have one
instance for the session, but commands and deferred continuations retain the
client that caused them. Never use the instance's map owner as the acting client.
Server metadata is supplied by the host before initialization and cannot be
replaced through guest configuration.

## Process and source confinement

Live plugins and named workers run in separate supervised processes, using
`yinz_steel::worker::WorkerRuntime`. Do not replace this with the in-process
`Runtime`: native parsing, macro expansion, or allocation can outlive a bytecode
interrupt. The process boundary provides memory limits, deadlines, interruption,
and reaping. A failed or interrupted worker cannot be reused. Replies, source
files, logs, and rendered output have separate size limits.

The sandbox exposes only approved Steel modules and functions. Do not restore
filesystem, dynamic library, environment, subprocess, or network primitives.
Inherited descriptors are closed in the worker before guest evaluation. Host
callbacks remain on the plugin's pinned executor thread.

A `SourceRoot` resolves explicit relative Scheme imports inside the canonical
entrypoint directory or pinned repository root. Symlink escapes, special files,
and unbounded dependency graphs are refused. Embedded imports resolve only in
the compiled builtin tree, with no filesystem fallback. Loading, checking, and
provider inspection must use the same source resolver. Preserve source filenames
and line numbers through deferred failures.

Module initialization has no host-command authority. Provider inspection runs in
a bounded compilation worker, reads the `agent-provider` descriptor, validates
it, and never calls `load`.

## Lifetime and delivery

An instance belongs to one executor thread. `RUNTIMES` is thread-local and keyed
by generation; `SteelPlugin::with_runtime` rejects execution on another thread.

A generation identifies one incarnation, including its workers. Replies, ticks,
data watches, pane output, and queued jobs must keep that identity so stale work
cannot reach a replacement. A registered instance ID additionally identifies pipe
ownership across map rekeys. Retiring one instance must release only its own pipe
registrations.

The reply mailbox exists before initialization and survives until final disposal.
The owning executor claims replies after subscription and permission checks.
Cleanup stops ordinary delivery, drains unclaimed replies in routing order, and
preserves each acting client. Failed initialization and final disposal remove the
mailbox.

Reload calls `BeforeClose`, runs the optional `unload` hook once, clears old
registrations, and then loads the replacement. An in-flight call gets the unload
grace before interruption. Failed or interrupted instances skip guest cleanup.
Cleanup has a bounded deadline, and shutdown keeps background jobs available
until plugins finish so cleanup can await command results. No hook is guaranteed
after a crash or after the session shutdown deadline.

Each named worker has its own persistent process and state. Its queue waits for a
deferred job to finish before starting the next job. One rejected job is logged
without dropping later jobs. Parent disposal cancels and joins every worker before
replacement. Worker replies and timers carry the worker's generation.

Initial load must schedule a render even when permission prompts cache a resize.
Keep this host behavior separate from the standalone runtime's lifecycle return
values. Batch delivery must retain event order and render once at the end of the
batch when requested.

## Events, rendering, and diagnostics

Events preserve the Rust serde wire shapes: string-keyed hashes for objects,
lists for ordinary arrays and tuples, bytevectors for typed binary fields, and
`json-null` for absent values. Decode compact binary transport markers only at
typed byte positions; ordinary JSON objects with the same key remain unchanged.
Pane render reports
use canonical pane IDs such as `terminal_3`. SDK helpers may add convenience APIs
without changing the raw event contract. Reserved RPC and SDK pipes are consumed
before the user's pipe callback.

`render` can return text or a deferred text result. Plugin output and diagnostic
logs use separate bounded ports. Drain both after execution, including errors,
workers, and REPL evaluations. `yinz-log` must never become visible frame content.
Every diagnostic line retains the plugin's source identity so CLI and host log
queries isolate aliases and repository plugins correctly.

## Authoring and tests

`yinz-plugin-authoring` must remain engine-free. It calls the supervised
compilation-worker protocol for checks, formatting, analysis, and provider
inspection. A syntax check does not execute guest initialization. Semantic and prepare checks use the same confined source graph as loading.
Formatting parses only the named file and never loads or evaluates its imports.

Use `cargo x test` and `cargo x integration-test -j 16`. The canonical commands
build the production worker binary first. Do not rebuild it while session tests
are running. Direct host tests must set `YINZ_COMPILATION_WORKER` to this
worktree's freshly built binary and use sandbox escalation when they create
sockets or the host cache.

Engine tests cover the SDK and confined worker transport. Host tests cover real
permission dispatch, generations, cancellation, workers, and plugin-thread
routing. Installed `.test.scm` suites use explicit mock replies; unstubbed host
operations fail. Installed `.integration.test.scm` cases own disposable real
sessions and enforce declared grants. Keep timeout, cancellation, cleanup, and
module-boundary tests when changing these paths. Retained reference traces compare
builtin commands and rendered cells against the original implementations.

Wait for observable content or state with `Watched::wait_until`, not render counts
or arbitrary sleeps. Count renders only when a test controls a fixed batch and
explains why scheduling cannot change that count. Treat timeouts and flaky results
as defects that need a cause and a fix.
