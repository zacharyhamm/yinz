# JavaScript and TypeScript plugins

Zellij embeds Deno Core to run local JavaScript and TypeScript plugins alongside Lua and Wasm plugins.
No Deno or Node installation is needed to run or check a plugin.

Use an explicit `.js`, `.mjs`, `.ts`, or `.mts` filename in a file URL:

```kdl
layout {
    pane {
        plugin location="file:hello.ts"
    }
}
```

File URLs also work in aliases, background plugins, and hooks. Source lookup follows the same rules as Lua plugins.
Set `shared true` in the plugin configuration to use one instance across clients.

## Lifecycle

Import the embedded SDK from `zellij:plugin`. Export a default object with optional `load`, `update`, `pipe`, and `render` methods.

```ts
import { definePlugin, zellij } from "zellij:plugin";

let text = "Starting";

export default definePlugin({
  defaults: { greeting: "Hello from TypeScript" },
  async load(configuration) {
    await zellij.sleep(100);
    text = configuration.greeting;
    zellij.requestRender();
  },
  render(rows, columns) {
    return `${text}\n${columns} columns, ${rows} rows`;
  },
});
```

Configuration values are strings. Optional `defaults` supply values that the caller's configuration can override, including with an empty string.
The SDK validates defaults and passes a new, shallow-merged object to `load`. Neither input object changes.
`update` receives an event, and `pipe` receives a pipe message.
Return `true` from either callback to request a render. `render` returns terminal text, which can include ANSI escape sequences.
Each callback can return a Promise.

Callbacks start in delivery order. While one callback waits, later callbacks can run and change plugin state.
Event waiters do not consume events. The same event still reaches `update` and other listeners.
Each event captures its type, payload, and listeners before `update` starts, then calls those listeners in registration order.
Listeners added during `update` or another listener start with the next event.
A `waitForEvent("Key")` started by a key handler waits for the next matching key, not the key that started the handler.
Render requests coalesce while a render is pending. A state change or resize invalidates an outdated async render result.

Run host commands from lifecycle callbacks. Module initialization cannot use host commands or wait for host events.
An initialization Promise that cannot settle fails with a diagnostic.

## Host commands and permissions

`zellij.command` exposes the existing host command dispatcher. Command names retain their Rust spelling.
Payloads use the command's JSON representation:

```ts
const ids = zellij.command("GetPluginIds");
zellij.command("Subscribe", ["Key", "Timer"]);
```

Commands are validated at runtime and use the same permission checks as Lua and Wasm.
Arbitrary replies have JSON values or binary file bytes. Use the SDK's typed helpers for its core operations.
Do not treat a request for permission as a grant. Wait for the permission answer before calling a protected command.

```ts
await zellij.waitForPermissions(["ReadApplicationState"]);
const key = await zellij.waitForEvent("Key");
zellij.log(key.bare_key);
```

Permission requests use one FIFO queue across `waitForPermissions`, `requestPermission`, and both forms of `command("RequestPluginPermissions", ...)`.
Each request subscribes and registers its result listener before dispatch. Only one request waits for a host answer at a time.
The SDK copies each permission array and retains the originating client when it queues the request.
Later requests include previously confirmed grants because the host replaces its permission set.
`waitForPermissions` rejects if the host denies permission or dispatch fails. Either outcome lets the next queued request proceed.
The other entry points remain fire-and-forget. Deferred dispatch errors use the SDK's lifecycle error reporting.
Use `getPluginIds()` for the typed plugin identity. Async callbacks retain the client that started them across `await`.

Events retain their JSON variant names. For example, a timer is `{ Timer: seconds }` and a unit event is a string.
Optional tuple fields remain `null`, so later fields keep their positions.
`PaneRenderReport` uses string keys such as `terminal_7` and `plugin_7`, as the CLI event stream does.
See [sdk.d.ts](../zellij-server/src/plugins/deno/sdk.d.ts) for the SDK declarations.

## Numbers and allocation limits

Safe integers arrive as `number`. Larger integer replies arrive as `bigint` and can be passed back unchanged.
Integer inputs must be exact and fit the Rust destination type. Use BigInt for integers outside the JS safe range.
Fractions and unsafe integer-shaped numbers remain usable in real-valued fields. Integer fields reject them.
Commands reject `NaN`, infinities, unsupported values, cycles, proxies, and accessors, including in optional fields.
Arrays, `Uint8Array`, and plain data objects are supported. Nesting is limited to 128 and conversion to a 64 MiB message budget.

Each plugin and checker has a 1 GiB V8 heap quota and a separate 1 GiB live buffer quota.
Memory is allocated on demand. Exceeding a quota permanently fails that runtime, even if JavaScript catches the allocation exception.
The host removes its resources without calling guest cleanup code. Reload creates a healthy replacement.
These quotas do not isolate the server from arbitrary native V8 crashes.

Local source files are limited to 8 MiB each and 64 MiB per module graph before parsing.
The offline checker also has a 30-second deadline.
Resizable buffers and guest WebAssembly are unavailable because their allocations bypass V8's buffer allocator.
Fixed `ArrayBuffer` and `SharedArrayBuffer` objects remain available.

## Async helpers

Use `runCommand` and `webRequest` after you receive their permissions:

```ts
await zellij.waitForPermissions(["RunCommands", "WebAccess"]);
const command = zellij.runCommand({ command: ["/usr/bin/printf", "%s", "Hello 🙂"] });
const request = zellij.webRequest({ url: "https://example.com" });
try {
  const [output, response] = await Promise.all([command, request]);
  zellij.log(output.exitCode, zellij.decodeUtf8(output.stdout));
  zellij.log(response.status, zellij.decodeUtf8(response.body));
} finally {
  command.cancel();
  request.cancel();
}
```

`runCommand` accepts `command` as a nonempty argv array, plus optional `envVariables`, `cwd`, `context`, and `maxBytesReturned`.
The maps default to empty objects. The working directory defaults to `/host`. The host supplies the default output limit.
`maxBytesReturned` sets the limit for each output stream and must be a nonnegative safe integer.
The result contains `exitCode`, `stdout`, `stderr`, and `context`. Output streams are `Uint8Array` values. `exitCode` can be `null`.

`webRequest` accepts `url`, plus optional `verb`, `headers`, `body`, `context`, and `proxy`.
The verb defaults to `Get`. The other supported verbs are `Post`, `Put`, and `Delete`.
Headers and context default to empty string maps. The body defaults to an empty string and accepts a string or `Uint8Array`.
This helper buffers the response. Its result contains `status`, `headers`, `body` as `Uint8Array`, and `context`.
Use `encodeUtf8(text)` and `decodeUtf8(bytes)` to convert between strings and bytes. Invalid UTF-8 decodes with replacement characters.

Each operation receives its own result, even when replies arrive out of order. Nonzero command exits and HTTP error statuses resolve normally.
Invalid options, permission failures, and dispatch errors reject the promise. Helpers do not request permissions automatically.
Context values must be strings. The SDK copies the context and reserves `__zellij_async` for correlation. A caller-supplied reserved key rejects the operation.
Helper results omit this key. Normal event handlers still receive the unchanged wire payload, including the correlation key and byte arrays.

### Waits and cancellation

`runCommand`, `webRequest`, `waitForEvent`, `waitForPipe`, and `sleep` return `CancellablePromise<T>`, a native promise with `cancel()`.
Cancel an unsettled operation to reject it with an error named `AbortError`. Catch that rejection as you would any promise failure.
Cancellation is idempotent. Calling it after settlement has no effect. Completion, cancellation, and dispatch failure remove the operation's listeners and timers.
Already-started host actions continue. Late replies still reach normal event handlers.

`Promise.all` and `Promise.race` compose these promises. They do not cancel their inputs. Chained promises have no `cancel()` method.
Keep the original promises when you need cancellation. Permission promises keep their FIFO behavior and are not cancellable.
The [shipped async example](../example-plugins/hello.ts) runs a command and HTTP request concurrently and cancels its waits after five seconds.

`waitForEvent(type, predicate?)` resolves with the next matching event payload.
`waitForPipe(name, predicate?)` resolves with the next matching `PipeMessage`:

```ts
const reply = await zellij.waitForPipe("reply", message => message.args.request === "42");
zellij.log(reply.payload);
```

Both predicates are synchronous and optional. If a predicate throws, the wait rejects.
Waits observe messages without consuming them. Pipe waiters are captured before `pipe` runs, so a wait created inside that callback starts with the next message.
Required event subscriptions remain active while helpers need them, including across raw `Unsubscribe` calls.
Subscriptions stay active after helpers finish. Use `unsubscribe` when you no longer need them.

### Timers

`sleep(milliseconds)` resolves without a value. `after(milliseconds, callback)` and `every(milliseconds, callback)` return a handle with `cancel()`.
All durations must be finite numbers between 0 and 2147483647 milliseconds.

```ts
const refresh = zellij.every(1000, async () => {
  await refreshState(); // Your plugin's function.
  return true;
});
// Stop future invocations when the plugin no longer needs updates.
refresh.cancel();
```

Callbacks retain the client that created the timer. They may return a promise and may return `true` to request rendering.
`every` waits one interval after the previous callback settles, so its callbacks never overlap.
Cancellation stops future invocations. A callback already in progress can finish and request rendering.
A callback failure stops that timer and uses the existing lifecycle error reporting.

## Files and project configuration

These helpers are synchronous and use the existing host permissions and confined paths.
`fs.read(path, {maxBytes?})` returns `Uint8Array`. `fs.list(path)` and `fs.metadata(path)` return typed host results;
missing metadata is `null`. `fs.write(path, contents, {append?, createDirs?})` accepts UTF-8 strings or `Uint8Array`.
`fs.remove(path, {recursive?})`, `fs.mkdir(path, {all?})`, and `fs.rename(from, to)` return normally on success and throw on failure.
All flags default to false. Reads require `ReadFiles`; mutations require `WriteFiles`.
Writing under `/plugins` also requires `AuthorPlugins`. Existing traversal, symlink, and size limits apply.

```ts
await zellij.waitForPermissions(['ReadFiles', 'WriteFiles']);
zellij.fs.write('/host/result.bin', new Uint8Array([0, 128, 255]));
const bytes = zellij.fs.read('/host/result.bin');
const previous = zellij.projectConfig.read('.');
zellij.projectConfig.write('.', 'agent_exit { default_branch "main"; }', previous.source);
```

Project configuration takes a host directory (relative to the plugin cwd, such as `.`).
`projectConfig.write(directory, source, expectedSource)` requires the exact previous source, or `null` if absent.
The host resolves the project control checkout and refuses a stale source. It does not run the configured jobs.

## Persistent data

`zellij.data` uses the existing plugin data store. Private namespaces follow the plugin URL across reloads.
`PluginData` is required; explicit `shared:` namespaces also require `SharedPluginData`.
The helpers do not change ownership columns or revisions, retry conflicts, or migrate storage.

```ts
const d = zellij.data;
await zellij.waitForPermissions(['PluginData']);
d.define('notes', {text: 'Text', score: 'Real', attachment: 'Blob'});
d.apply([d.put('notes', {text: 'hello', score: d.real(1), attachment: new Uint8Array([255])}, {uid: 'first', expectRev: 0})]);
const query = {entity: 'notes', filter: d.eq('uid', 'first')};
const {records, raw} = d.fetch(query);
const revision = records[0].rev;
try {
  d.apply([d.update('notes', query.filter, {text: 'edited'}, {expectRev: revision})]);
} catch (error) {
  if (!d.isConflict(error)) throw error;
  // Show the conflict to the author; keep the revision they originally read.
}
const watch = d.watch(query, snapshot => zellij.log(snapshot.records));
zellij.log(watch.initial.records);
watch.dispose();
```

`define` defaults to schema version 1 and non-destructive changes. Attributes can be kind names or
`{kind, indexed?, fullText?, unique?, required?, default?}`. `catalog(namespace?)` lists visible definitions.
`fetch` and `count` accept `{entity, namespace?, filter?, search?, sort?, select?, limit?, offset?, afterId?}`.
Fetch defaults are limit 100 and offset 0. `apply(mutations, {namespace?})` applies a whole transaction.
`put`, `update`, and `delete` construct mutations; they do not dispatch by themselves.

Predicates are `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `in`, `isNull`, `notNull`, `prefix`, `contains`, `and`, `or`, and `not`.
`toValue`, `fromValue`, `fromRecord`, and `toRecords` convert between tagged values and convenient records.
Plain safe integral numbers become `Int`. Use `int()` for explicit integers, `real()` for integer-shaped real values,
and `json()` for serialized JSON text. Blob values become `Uint8Array`; JSON values remain encoded text.

Fetch and watch snapshots contain `{records, raw}`. The raw rows retain tagged values, truncation, and cursors.
`watch(query, onChange, {mode?})` defaults to row mode and returns `{watchId, initial, dispose()}`.
Registration retains the listener before dispatch. Successful disposal unregisters the host watch and removes its listener;
repeated successful disposal is harmless. Reload and disconnect remove the old runtime's watches.
The shared store still only wakes watches for writes from the same server process.

## Plugin RPC

Register a method with `zellij.rpc.handle(name, handler)`. Registration replaces the handler with the same name.
Handlers can return JSON directly or through a promise. An `undefined` result becomes `null`.
The `hocus_tool:` prefix is reserved for `zellij.hocus.tools()`.

```ts
zellij.rpc.handle("forecast", (payload, call) => ({city: payload, source: call.source}));

// Run inside a lifecycle callback, after permission is granted.
const request = zellij.rpc.call({
  pluginId: 7,
  name: "forecast",
  payload: "Oslo",
  timeoutMs: 10_000,
});
const result = await request;
```

Supply exactly one destination: `pluginId` or `pluginUrl`.
URL calls can include `pluginConfig`, a string map that defaults to `{}`.
The payload defaults to `null`. Without `timeoutMs`, the call has no deadline.
Both peers need `MessageAndLaunchOtherPlugins`. RPC does not request permission automatically.

Calls resolve directly with JSON results. Remote errors, unknown methods, invalid arguments, and local dispatch failures reject the promise.
`request.cancel()` rejects with `AbortError` and stops the local wait. It does not stop remote execution.
Timeouts reject with `TimeoutError`. Calls never retry automatically.

Handler metadata contains `version`, `callId`, `name`, `payload`, and the host-supplied `source: {Plugin: number}`.
Client context survives `await`. Sender-supplied `source` and `reply_to` fields are ignored.
ID-addressed calls accept replies only from that plugin ID. URL-addressed calls match the opaque call ID because the host does not return the destination ID.
Each isolate uses a new UUID prefix, so replies from an old instance cannot settle replacement calls.

RPC uses Lua's version-1 `zellij_rpc_call` and `zellij_rpc_reply` envelopes, with a 64,000-byte limit.
Unsupported JSON values reject instead of losing fields. Malformed, stale, and duplicate replies are ignored.
RPC messages stay internal. Ordinary pipe messages still reach `pipe` callbacks and `waitForPipe()`.
Lua callers retain their existing `{result}` or `{error}` wrapper.

## Hocus extensions

Put an extension directly in the user plugin directory's `hocus/` folder.
Use a lower-snake-case file stem, such as `hocus/weather.ts`.
Hocus accepts `.lua`, `.js`, `.mjs`, `.ts`, and `.mts`. Backups are excluded.
Two files with the same stem conflict, even with different suffixes.
Discovery and `plugin_reload` report both paths and register neither candidate.

In `load`, await `zellij.waitForPermissions(['MessageAndLaunchOtherPlugins'])`, then call `zellij.hocus.tools(definitions)`.
Each key names a tool. Each definition supplies a `handler(args, call)` function.
Handlers can return JSON or a promise of JSON. Exceptions become tool errors.
Hocus calls `forecast` from `weather.ts` as `weather_forecast`.

Definitions accept `description`, JSON Schema `parameters`, `safety`, and `consequence`.
Defaults are the tool name, `{type: 'object'}`, and `read_only`.
Safety can also be `automatic` or `confirm`. A `confirm` tool requires a nonempty consequence.
Handlers must validate their arguments before acting on them.

Calling `tools` again replaces the complete tool set. The SDK repeats registration when Hocus requests discovery.
`zellij.hocus.launch()` returns `{namespace, path}` when Hocus supplied that identity, or `null` otherwise.
Without an identity, the plugin does not register tools.
Hocus verifies ownership independently and keeps its existing 30-second call timeout.
The SDK consumes extension protocol messages. Ordinary pipes still reach listeners and the plugin's `pipe` callback.
Replies use the host-provided source plugin ID. Async handlers retain the client that received the call.

See [weather-extension.ts](../example-plugins/weather-extension.ts), or `plugin_docs section=example_deno_extension`.

## Shared UI and terminal text

`zellij.ui` uses the same Rust rendering toolkit as Lua and Wasm plugins.
Construct components during module initialization if needed. Rendering returns terminal strings.
The plugin controls subscriptions, host commands, and submission behavior.

Use `text`, `ribbon`, `table`, and `nestedList` for themed components.
Styled text supports `selected()`, `opaque()`, `disabled()`, `emphasis(level, from, to)`, `dim()`, `unbold()`, `error()`, and `success()`.
Style ranges use Unicode character indices, with a zero-based start and exclusive end.
They do not use JavaScript UTF-16 offsets: `ui.text('🙂a').emphasis(1, 1, 2)` styles only `a`.

Use `textInput`, `checkbox`, `radioGroup`, `select`, `combo`, and `button` for individual controls.
`form(fields)` groups descriptors with unique, nonempty IDs.
Each descriptor has a camelCase `kind`, an `id`, and the control's options.
`dialog({title, message, help, fields})` adds a frame and supports `setStatus(text)`.
Selections are zero-based. `null` means no selection. Nonempty option lists initially select index zero.

Controls expose `value()`, `values()`, `setValue(value)`, and `focus()`.
For forms and dialogs, use `value(id)`, `setValue(id, value)`, and `focus(id)`.
Choice controls support `setOptions(options)`. Forms use `setOptions(id, options)`.
All widgets support `focusedId()`, `moveFocus(forward)`, `handleKey(key)`, `handleMouse(mouse)`, and `insertPastedText(text)`.
Pass the typed `Key`, `Mouse`, and `PastedText` event payloads directly.
Interaction results contain `kind` and `shouldRender`, plus `id` when applicable.
Kinds are `fieldChanged`, `complete`, `buttonPressed`, `focusMoved`, `cancel`, and `ignored`.
Handle these results explicitly. A button reports intent and does not run a command.

`render({x, y, width, height})` returns a terminal string. `x` and `y` default to zero.
Widget cursors are published only with an accepted plugin render, for that render's client.
Discarded async renders cannot move the cursor. Rendering outside the lifecycle callback does not publish a cursor.

`ui.markdown(source, {compact})` retains parsed blocks and caches wrapping for the current width.
Its `render({x, y, width, height, scroll})` returns `{text, totalRows}`. `scroll` counts rows and defaults to zero.
A width change rebuilds wrapping. A zero-sized viewport produces no visible output.
Native state belongs to one isolate and is released on garbage collection or runtime disposal.
Native allocations share the isolate's backing-store budget. Mutation inputs are conservatively charged until collection, including retained capacity and caches.
Input, coordinates, and output are bounded by transport limits. Excessive native allocations exhaust the isolate's budget.

`zellij.text` provides `width`, `truncate`, `pad`, `padLeft`, `center`, `wrap`, and `sanitize`.
Widths measure terminal columns. `truncate` defaults to `…` and accepts a replacement ellipsis as its third argument.
Padding truncates oversized text without an ellipsis. `wrap` and `sanitize` use the shared Rust implementation.

See [forms.ts](../example-plugins/forms.ts), or `plugin_docs section=example_deno_forms`.

## Modules and checking

Local imports must name an explicit relative file, such as `./state.ts` or `./lib/format.js`.
Each module must remain inside the entrypoint directory after symlink resolution.
The runtime rejects remote URLs, package imports, CommonJS, JSX/TSX, and unsupported file types.
Local `.d.ts` files are available to the checker only.

TypeScript is transpiled when a plugin loads. Transpilation does not check types.
For semantic checking, run:

```sh
zellij plugins check hello.ts
zellij plugins check --file ./hello.ts
```

The checker embeds a pinned TypeScript compiler and ES2022 declarations.
It uses strict checking and ESNext modules. It checks imported local modules without executing plugin code or accessing the network.
There are no declarations for general Deno I/O or browser APIs that this runtime does not provide.

Each instance holds its own source and source maps. Reload reads dependencies again.
Reload discards pending JavaScript callbacks and ignores timer and command replies from the old instance.
Host actions already started can still finish; reload does not undo their side effects.

## Authoring loop

Use an explicit extension when writing JavaScript or TypeScript:

```sh
zellij plugins write hello.ts < ./hello.ts
zellij plugins check hello.ts
zellij plugins reload hello.ts
zellij plugins log hello.ts
```

Writes validate syntax and transpilation. They do not perform semantic checking.
Use `--overwrite` to replace a file. The previous version is preserved as `hello.bak.ts`.
The existing directory and symlink protections also apply to these filenames.
An extensionless authoring name still means a Lua plugin.

The SDK does not include npm, JSR, general Deno I/O, Lua widgets, or agent helpers.
Hocus supports the same names through `plugin_write`, `plugin_edit`, `plugin_list`, `plugin_read`,
`plugin_search`, `plugin_outline`, `plugin_check`, `plugin_reload`, and `plugin_log`.
`plugin_docs section=deno` serves the runtime and authoring guide; `section=deno_data` serves the filesystem and persistence helpers. Writes and edits check syntax before approval;
explicit JS/TS `plugin_check` checks types and local imports. `CheckPlugin` exposes the report as
`{diagnostics, outline, permissions, warnings}` with `Syntax` or `Semantic` mode and requires `ReadFiles`.
`CheckLuaPlugin` remains compatible.

Hocus infers grants only from literal SDK requests in the proposed entrypoint, using parsed JS/TS bindings.
Imported default/named aliases and SDK namespace imports are recognized; comments and shadowed names grant nothing.
Computed requests use normal runtime approval and produce warnings. Imported modules do not add inferred grants.
The approval shows inferred grants; edits show newly requested grants. Grants are applied only after a successful write.

Queued jobs keep running after their enqueuer reloads. Addressed start/finish events belong to the original Deno generation.
The replacement can query `QueueStatus` and receive broadcast `QueueChanged` events.

## Build inputs

The host links `deno_core` 0.405.0, `deno_ast` 0.53.3, and their pinned V8 dependency.
The first build downloads the V8 archive unless it is already cached.
See [compiler provenance](../zellij-server/src/plugins/deno/compiler/provenance.json) for the bundled compiler's version, source, and checksums.
The compiler license and third-party notices are stored beside it.

The pinned V8 release provides prebuilt archives for Linux and for both Intel and Apple Silicon macOS.
The runtime uses the same local-file and executor implementation on these platforms.
This checkout was developed on Linux. A native macOS build still needs a macOS runner.

### Measured Linux build impact

On x86-64 Linux with Rust 1.98.1, `cargo x build --no-plugins` followed by `strip -o` produced these sizes.
Both builds used the development profile; these are not release-profile measurements.

| Binary | Bytes |
| --- | ---: |
| Before this change (`0ae90a1994db`) | 78,906,064 |
| With embedded Deno and the checker | 153,507,864 |
| Increase | 74,601,800 (94.5%) |

The compiler JavaScript is 9,112,572 bytes and the bundled declarations are 484,734 bytes.
The cached V8 149.4.0 static archive is 185,175,872 bytes; this is a build input, not an additional installed file.
The checker initializes its compiler only for an explicit check. Plugin loads use `deno_ast` instead.
Builds used a shared dependency cache, so their elapsed times do not measure cold-build overhead.
