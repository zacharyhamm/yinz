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
`PaneRenderReport` uses string keys such as `terminal_7` and `plugin_7`, as the CLI event stream does; `zellij.paneKey(paneId)` spells them.
See [sdk.d.ts](../zellij-server/src/plugins/deno/sdk.d.ts) for the SDK declarations.

## Host command helpers

`zellij.host` names every host command with its first letter lowercased and dispatches it through `zellij.command`,
so validation, permission checks, and tracing are unchanged:

```ts
const ids = zellij.host.getPluginIds();
zellij.host.switchTabTo(2);
zellij.host.renameTab(0, "logs");            // positional tuple fields
zellij.host.renameTab([0, "logs"]);          // or the raw array
zellij.host.renameTab({ tabIndex: 0, name: "logs" });
```

A unit command takes no argument. A tuple command takes its fields as positional arguments or one array.
Any other command takes its payload as one argument. These tuple commands also accept one object of camelCase named fields;
an omitted field becomes `null`, which an optional slot reads as absent, and an unknown field throws:

| Helper | Named fields |
| --- | --- |
| `renameTab`, `renameTabWithId`, `renameTerminalPane`, `renamePluginPane` | `tabIndex` / `tabId` / `paneId`, `name` |
| `focusTerminalPane`, `focusPluginPane` | `paneId`, `shouldFloatIfHidden`, `shouldBeInPlaceIfHidden` |
| `showPaneWithId` | `paneId`, `shouldFloatIfHidden`, `shouldFocusPane` |
| `setFloatingPanePinned`, `setPaneBorderless` | `paneId`, `shouldBePinned` / `borderless` |
| `revealPaneWithId` | `paneId`, `label` |
| `notify` | `message`, `paneId`, `click` |
| `cliPipeOutput` | `pipeName`, `output` |
| `reconfigure` | `configuration`, `writeToDisk` |
| `runCommand` | `command`, `envVariables`, `cwd`, `context`, `maxBytesReturned`, `streamOutput` |
| `openCommandPaneInGitCheckoutNearPlugin` | `command`, `sourceDirectory`, `checkout` |

`zellij.host.runCommand` is the raw command; `zellij.runCommand` remains the promise-returning helper.
`PostMessageTo` throws before dispatch: Deno plugins have no workers. Use timers, the async helpers, or a work queue.

The root object also carries the Lua prelude's conveniences:

- `keyName(bareKey)`: `"t"`, `"Esc"`, `"F2"`. `mouse(event)` returns `{kind, line, col}` or `{kind, lines}` with the Rust variant name as `kind`.
- `paneKey(paneId)`: `"terminal_3"`, the `PaneRenderReport` key. `terminalPane(id)` and `pluginPane(id)` build `PaneId` values.
- `bytes(value)`: a `Uint8Array` from raw event bytes, text, or an existing array. `text.sanitizeBytes(bytes)` decodes and sanitizes.
- `write(text)` and `writeToPaneId(text, paneId)` encode text as UTF-8 (`WriteToStdin`).
- `readPluginLog(name, lines = 50)` (`AuthorPlugins`), `openTerminal(cwd?)` (`OpenTerminalsOrPlugins`), `closeSelf()`, `setSelectable(boolean)`.

## Default keybindings

A plugin can register fallback keybindings. User configuration always wins; a default is active only while its
mode and key are otherwise unbound. Registrations are owned by the plugin, replaced by its next call, and removed
when it unloads. If plugins collide, the lower plugin id wins until it unloads. Registration needs `Reconfigure`:

```ts
let open = false;
export default {
  async load() {
    await zellij.waitForPermissions(["Reconfigure"]);
    zellij.registerDefaultKeybindings([
      { mode: "normal", key: "Ctrl g", handler(message) { open = !open; return true; } },
      { mode: "normal", key: "Ctrl n", actions: [zellij.action("NewPane")] },
    ]);
  },
};
```

Each entry has `mode`, `key`, and exactly one of `handler` or `actions`. Handlers receive the keybinding pipe message,
run with the client that pressed the key, and re-render when they return `true`. Their pipe names start with
`__zellij_default_keybinding:`; the SDK consumes those messages before `pipe` and pipe waits see them.
Actions are ordinary keybinding actions: a bare name, `zellij.action(name, ...args)` for positional arguments,
`zellij.action(name, {prop: value})` for properties, or `zellij.action(name, {args, props, children})`.
Values are strings, finite numbers, or booleans. A refused registration keeps the previous handlers.
See [keybindings.ts](../example-plugins/keybindings.ts).

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

`runCommand` accepts `command` as a nonempty argv array, plus optional `envVariables`, `cwd`, `context`, `maxBytesReturned`, and `onOutput`.
The maps default to empty objects. The working directory defaults to `/host`. The host supplies the default output limit.
`maxBytesReturned` sets the limit for each output stream and must be a nonnegative safe integer.
The result contains `exitCode`, `stdout`, `stderr`, and `context`. Output streams are `Uint8Array` values. `exitCode` can be `null`.

Supply `onOutput({data, stderr, context})` to receive live command output. `data` is a `Uint8Array`.
Callbacks run synchronously in delivery order under the client that started the command. Return values are ignored, including promises.
A synchronous exception rejects the command promise. Completion waits for stdout and stderr to drain.
`maxBytesReturned` limits buffered results only. Live chunks continue beyond that limit.
Completion, cancellation, and failures remove helper listeners. Cancellation stops observation without killing the host process.
Callback context omits `__zellij_async`. Ordinary event handlers still receive unchanged `RunCommandOutput` events.

```ts
const result = await zellij.runCommand({
  command: ["/usr/bin/printf", "hello"],
  onOutput({data, stderr}) { zellij.log(`${stderr ? "stderr" : "stdout"}: ${data.length} bytes`); },
});
```

Both raw command forms accept `RunCommand: [argv, env, cwd, context, maxBytesReturned, streamOutput]`.
Use `true` for live events, or `false`/`null` for buffered output. Existing five-field calls remain supported.

To observe command panes, request `ReadPaneContents`, subscribe to `PaneOutput`, then register a nonempty context filter:

```ts
zellij.subscribe(["PaneOutput"]);
const off = zellij.on("PaneOutput", ({pane_id, data, context}) => {
  zellij.log(`pane ${pane_id}: ${data.length} bytes`);
});
zellij.command("SubscribePaneOutput", {work: "one"});
// Later, stop host observation and remove this handler:
zellij.command("SubscribePaneOutput", null);
off();
```

Every filter entry must match the command pane's launch context. Raw `PaneOutput.data` is a JSON byte array.
A new registration replaces that plugin/client pair's filter. Disconnect removes that client's registration. Unload or reload removes the plugin's registrations.
A replacement plugin can subscribe to existing panes. Queued chunks from its predecessor cannot reach the replacement.

`webRequest` accepts `url`, plus optional `verb`, `headers`, `body`, `context`, `proxy`, and `streamId`.
The verb defaults to `Get`. The other supported verbs are `Post`, `Put`, and `Delete`.
Headers and context default to empty string maps. The body defaults to an empty string and accepts a string or `Uint8Array`.
Without `streamId`, this helper buffers the response. Its result contains `status`, `headers`, `body` as `Uint8Array`, and `context`.
Use `encodeUtf8(text)` and `decodeUtf8(bytes)` to convert between strings and bytes. Invalid UTF-8 decodes with replacement characters.

Each operation receives its own result, even when replies arrive out of order. Nonzero command exits and HTTP error statuses resolve normally.
Invalid options, permission failures, and dispatch errors reject the promise. Helpers do not request permissions automatically.
Context values must be strings. The SDK copies the context and reserves `__zellij_async` for correlation. A caller-supplied reserved key rejects the operation.
Helper results omit this key. Normal event handlers still receive the unchanged wire payload, including the correlation key and byte arrays.

For streaming, supply a string `streamId`, distinct for each concurrent stream.
The promise resolves with the leading response: empty body for 2xx, buffered body otherwise.
Subscribe and register listeners before dispatch. Filter by `stream_id` and check terminal `done`/`error`.
After receiving `WebAccess`:

```ts
const streamId = "download-1";
zellij.subscribe(["WebRequestStreamChunk"]);
const off = zellij.on("WebRequestStreamChunk", event => {
  if (event.stream_id !== streamId) return;
  // Process event.chunk bytes in order.
  if (event.done) {
    if (event.error !== null) zellij.log(event.error);
    off();
  }
});
const response = await zellij.webRequest({ url: "https://example.com", streamId });
```

The wrapper does not subscribe to or buffer chunks.
`.cancel()` cancels only the response wait. Stop host streaming with `zellij.command("CancelWebRequest", streamId)`.

### Commands in one pane

`paneSequence` runs commands in order and reuses one terminal pane. Project automation
still uses Swamp workflows. See [the pane example](../example-plugins/workflow.ts).

```ts
const sequence = zellij.paneSequence({
  name: 'Checks',
  steps: [
    {path: 'sh', args: ['-c', 'echo first']},
    {path: 'cat', stdin: 'input\n\x04', timeoutMs: 30_000},
  ],
  onStep(number, step, exitCode) {
    zellij.log(`step ${number}: ${exitCode}`);
    // sequence.push({path: 'true'}) can append work here.
  },
});
const result = await sequence.done;
// sequence.cancel() signals the child. Repeated cancellation is harmless.
```

Steps accept all `CommandToRun` fields, including SSH and Firecracker targets, plus
`name`, `stdin`, and `timeoutMs`. The helper writes `stdin` once, after the matching
process starts. It does not add a newline or EOF. The timeout starts at that same event.
All helper durations use **milliseconds**. An omitted timeout has no deadline.

`cwd: 'inherit'` reads the latest pane metadata when dispatching the step. It uses the
reported directory only if the execution target matches. Otherwise it uses the host's
default directory. The first step has no preceding pane directory.

| Option | Default and behavior |
| --- | --- |
| `name` | Optional pane title. Step names label callbacks. |
| `floating` | `false`: tiled pane. Accepts `true` or `FloatingPaneCoordinates`. |
| `stopOnFailure` | `true`: stop at the first nonzero or unavailable exit code. With `false`, continue, but report `failed` if any step failed. |
| `keepScrollback` | `true`: preserve output between commands. |
| `closeOnSuccess` | `false`: leave the pane open. |
| `onCancel` | `'keep'`: leave the pane open. `'close'` closes it after cancellation settles. |
| `killGraceMs` | `5000`: delay between SIGINT and SIGKILL, and the final wait after SIGKILL. |
| `onStep` | Optional `(number, step, exitCode)` callback. Step numbers start at one. |
| `onDone` | Optional `(result)` callback, once for a resolved outcome. |

The handle exposes `paneId`, `status`, `done`, `push(step)`, and `cancel()`.
`paneId` is a `PaneId` known after a successful open. It remains `null` if opening fails.
`status` starts as `running`, changes to `cancelling` after accepted cancellation,
and ends with `success`, `failed`, `cancelled`, or `pane_closed`.

`done` resolves with `{status, paneId, exitCodes, failedStep}`. Codes are numbers or
`null` when unavailable. `failedStep` is the first failed step number, or `null`.
Closing the pane settles immediately without reporting an exit for the unfinished step.
A timeout uses the cancellation escalation but ends with `failed`, even with
`stopOnFailure: false`. After both grace periods expire, the current exit code is `null`.
An initial cancellation signal refusal throws and leaves the sequence running.

Initial options and all steps are validated synchronously before any pane opens.
Host failures reject `done` after cleanup. Callback exceptions are logged. Callback
promises do not delay progression, so append synchronously within `onStep` to extend
the sequence before it completes. `push` validates and copies appended steps while
running. Additions after cancellation or completion are ignored.

Permissions: `RunCommands` opens and replaces commands. `ReadApplicationState` allows
startup, exit, closure, and pane metadata observation. `ChangeApplicationState` is needed
for naming, timeout signals, cancellation, and closing. `WriteToStdin` is needed for `stdin`.
Obtain permissions before starting the helper.

`waitForPaneExit(pane)` observes any terminal pane, including panes opened by the CLI or another plugin.
It requires `ReadApplicationState`. An already-exited pane resolves immediately with its recorded exit status.
A missing pane resolves immediately with `exitCode: null` and `closed: true`.
`runInPane(pane, step)` registers observation, then replaces the command in an existing
pane with scrollback preserved. Both accept a terminal number or `{Terminal: number}`
and return cancellable promises of `{paneId, exitCode, closed}`. They reject with
`AbortError` when cancelled. Their cancellation stops **observation only** and does not
signal the child. For `runInPane`, `timeoutMs` also stops observation only, with a timeout
error. Use `paneSequence` when a timeout must signal the child.

Listeners and helper timers are removed on completion or rejection. For `runInPane` and
`paneSequence`, unique instance and step contexts exclude exits from earlier commands.
Reload or disposal discards pending sequences and their timers. It does not stop an
already started child, resume the sequence, or run `onDone` in the replacement instance.
A disconnected callback cannot signal work through a newly attached client's identity.

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

## Embedded builtins and server identity

The script builtins `context-menu`, `agent-policy`, `swamp-bridge`, `swamp-workflow`, and
`agent-exit` use embedded TypeScript. Their `zellij:` URLs remain unchanged.
The bridge retains its delivery namespace, schema, correlation keys, and revision guards.

Embedded plugins can import `zellij:plugin` and the registered `zellij:workflow-view` module.
The shared view resolves from embedded source during execution and semantic checking.
There is no filesystem fallback. Filesystem plugins retain their directory confinement.

New plugins and reference examples use explicit `.ts` filenames. Extensionless authoring
names still select Lua. Lua plugins, `require('zellij.workflow_view')`, `CheckLuaPlugin`,
and `config.lua` remain supported.

The SDK exposes four readonly strings during module initialization:

- `serverBinary`: the resolved server executable path.
- `serverSession`: the session identity also supplied to Lua.
- `serverIncarnation`: one UUID per server process, shared with Lua and retained across plugin reloads.
- `instanceId`: one UUID per isolate, replaced on reload.

Metadata access does not enable host commands during module initialization.
The internal synchronous `workflowRemotePanes()` function is available only to the trusted `zellij:swamp-bridge` builtin.
It queries PTY ownership with a five-second timeout. Filesystem plugins cannot obtain this capability through their name or configuration.

## Agent helpers

Use `zellij.agent` to spawn, inspect, and prompt terminal agents.
Agent methods accept a numeric terminal ID or `{Terminal: id}`. Plugin IDs and invalid numbers are rejected.

```ts
await zellij.waitForPermissions(['RunCommands', 'ReadApplicationState', 'WriteToStdin']);
const launched = zellij.agent.spawnDetails({
  kind: 'codex', prompt: 'Inspect the parser', branch: 'parser-review',
  sourceDirectory: '/host', startSuppressed: true,
});
const reply = await zellij.agent.prompt(launched.pane_id as {Terminal: number}, 'Explain the result');
zellij.log(reply.message ?? 'Turn ended without a message');
```

`spawnDetails` returns `SpawnedAgent`: `{pane_id, branch, cwd}`. `spawn` returns only its `PaneId`.
Both require `RunCommands` and preserve the host launch defaults and worktree checks.
Options include all `AgentSpawnSpec` fields with their existing spelling, such as `permission_mode` and `allow_outside_worktree`.
Host options are `sourceDirectory`, `cwd`, `name`, `context`, `automaticFirecrackerVm`, and `startSuppressed`.

`info(pane)` returns the latest `AgentInfo` or `null`. `all()` returns a snapshot keyed by terminal pane ID.
Both subscribe to `PaneUpdate`. The cache updates before user callbacks and removes closed panes.
`on(handler)` subscribes to `AgentEvent` and returns an unsubscribe function.
These observations require `ReadApplicationState`.

`send(pane, text, {enter: true})` sends bracketed paste followed by Enter. Set `enter: false` to omit Enter.
It requires `WriteToStdin`.

| Wait | Result |
| --- | --- |
| `waitForState(pane, states, {after})` | The `PaneInfo` from a future matching update. Optional `after` requires a preceding matching update. |
| `waitForEvent(pane, hooks)` | The next matching `AgentEvent`. Omit `hooks` for any hook, or supply a name or array. |
| `waitForTurn(pane, {fallback})` | `{message, hook, state, sessionId}`, with nullable fields. |
| `prompt(pane, text, options)` | The same turn result. Registers the wait before sending and disposes it if sending fails. |

State filters accept canonical or lowercase names, individually or in arrays: `Busy`, `Waiting`, `Idle`, and `Exited`.
Turns end on `Stop`, `StopFailure`, or `Interrupt`. `SessionStart` does not end a turn.
With `fallback: 'idle'`, a turn can also finish after observing Busy or Waiting followed by Idle.
All waits return `CancellablePromise`. Calling `cancel()` removes listeners and competing waits, then rejects with `AbortError`.
Cancellation does not stop the agent. Waits have no implicit deadline.

## Hocus completions

`zellij.hocus.complete(options)` requests one model turn from the current session's `zellij:hocus-agentd`.
It requires `ReadApplicationState` for discovery and `MessageAndLaunchOtherPlugins` for dispatch.
Discovery must find exactly one daemon. The SDK addresses its plugin ID and checks that ID on replies.

```ts
await zellij.waitForPermissions(['ReadApplicationState', 'MessageAndLaunchOtherPlugins']);
const pending = zellij.hocus.complete({
  messages: [{role: 'user', content: 'Explain this error: unexpected EOF'}],
  system: 'Answer briefly.', timeoutMs: 30_000,
});
const result = await pending;
zellij.log(result.content);
```

Options are `messages`, `system`, `model`, `tools`, `toolChoice`, `temperature`, `maxTokens`, and `timeoutMs`.
The default deadline is 60,000 milliseconds. Requests allow at most 200 messages, 64 tools, and 64,000 encoded UTF-8 bytes.
Guest accessors, proxies, and unsupported JSON values are rejected without executing them.

The result is `{content, toolCalls, finishReason, usage, model}`.
`usage` is `null` when absent, or `{prompt_tokens, completion_tokens, total_tokens}` with nonnegative safe integers.
Each tool call contains `{id, name, arguments}`. `arguments` remains the model's raw string, even if it is invalid JSON.
Nested message fields retain the wire spelling so results can be supplied in subsequent requests:

```ts
const next = await zellij.hocus.complete({messages: [
  {role: 'assistant', content: result.content, tool_calls: result.toolCalls},
  {role: 'tool', tool_call_id: result.toolCalls[0].id, content: 'The tool result'},
]});
```

Only append tool results when the response contains calls. The caller executes and validates tools.
Daemon, discovery, and permission errors reject the promise. Deadlines reject with `TimeoutError`.
`cancel()` rejects with `AbortError` and removes the local pending request and timer. The daemon may finish an already-started request.
Malformed, forged, stale, and duplicate replies are ignored. Validated successful replies resolve once.
Completion protocol pipes stay internal. Ordinary pipes still reach callbacks and `waitForPipe()`.
Async continuations retain the originating client, and reload creates new request IDs.

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
Use `zellij.ui.markdown(source, {dim: true})` for subdued history output.
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

`zellij.text` provides `width`, `truncate`, `pad`, `padLeft`, `center`, `wrap`, `sanitize`, and `sanitizeBytes`.
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

### Tracing

Call `zellij.trace()` or `zellij.trace(true)` to enable a transcript. Call `zellij.trace(false)` to disable it.
Other argument types throw. Each runtime starts with tracing disabled.
The merged configuration applies `trace: "true"` or `trace: "false"` before `load`, with configuration overriding plugin defaults.
Absent or other values preserve the current setting. Runtime calls can change it afterward.

Read the existing `[deno:<plugin>]` log output with `zellij plugins log hello.ts` or Hocus `plugin_log`.
Trace lines contain the instance ID, originating client ID, command names, event types, and pipe names, including SDK-consumed pipes.
`zellij.host` helpers trace under the underlying command name; a refused `PostMessageTo` gets its own `refused` marker.
Existing SDK promise observers emit numbered `task` markers. Cancellable helpers emit numbered `operation` markers.
Both use `started` and `finished`, `failed`, or `cancelled` outcomes.
Tracing does not observe arbitrary guest promises.

Arguments, payloads, results, and exception text are excluded. Invalid command values receive a generic marker.
Names are limited to 200 characters, with control characters escaped.
Disabling tracing suppresses subsequent lines. Reload starts fresh and does not manufacture completion lines for discarded callbacks.

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

The SDK does not include npm, JSR, general Deno I/O, or workers.
Hocus supports the same names through `plugin_write`, `plugin_edit`, `plugin_list`, `plugin_read`,
`plugin_search`, `plugin_outline`, `plugin_check`, `plugin_reload`, and `plugin_log`.
`plugin_docs section=deno` serves the runtime and authoring guide; `section=deno_commands` serves the command helpers and default keybindings; `section=deno_data` serves the filesystem and persistence helpers. Writes and edits check syntax before approval;
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
