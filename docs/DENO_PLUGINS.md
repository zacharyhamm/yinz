# JavaScript and TypeScript plugins

Zellij embeds Deno Core to run local JavaScript and TypeScript plugins alongside Wasm plugins.
No Deno or Node installation is needed to run or check a plugin.

Use an explicit `.js`, `.mjs`, `.ts`, or `.mts` filename in a file URL:

```kdl
layout {
    pane {
        plugin location="file:hello.ts"
    }
}
```

File URLs also work in aliases, background plugins, and hooks. Source lookup uses these rules:
a bare `file:hello.ts` is looked for in the user's plugin folder. Set `shared true` in the plugin configuration
to use one instance across clients. A plugin is a local file: `https://` URLs load Wasm plugins only, and a remote
`.ts`, `.mts`, `.js` or `.mjs` URL is refused with a message to download the file and load it with `file:`.

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

## Events

Subscribe by the Rust variant name:

```ts
zellij.subscribe(["Key", "TabUpdate", "PaneUpdate"]);
```

An event arrives as its JSON variant: a unit event is a string (`"InputReceived"`), and every other event is one
`{Type: payload}` object. `update(event)` receives that object; `zellij.on(type, handler)` hands a handler the payload
and the whole event. Tuple payloads are positional arrays with `null` in an absent slot, bytes in an event are `number[]`
except live output (`RunCommandOutput` and `PaneOutput` carry a `Uint8Array`), and `PaneRenderReport` is keyed
`terminal_7` / `plugin_7` as the CLI spells it.

`zellij.event(raw)` decodes an event into a discriminated union - `type`, the untouched payload as `value`, the whole
event as `raw` - with named camelCase fields for the events whose raw shape is positional or bytes, and
`zellij.onEvent(type, handler)` is `on` with the decoded event:

```ts
zellij.onEvent("RunCommandResult", (e) => {
  if (e.exitCode === 0) zellij.log(zellij.decodeUtf8(e.stdout)); // stdout is a Uint8Array
  return true;
});
zellij.onEvent("Key", (e) => { if (e.key === "Esc") zellij.closeSelf(); }); // "t", "Esc", "F2"
zellij.onEvent("Mouse", (e) => { if (e.kind === "LeftClick") zellij.log(e.line, e.col); });
export default definePlugin({
  update(event) {
    const e = zellij.event(event);
    if (e.type === "TabUpdate") return e.value.some((tab) => tab.active); // a list, as delivered
    if (e.type === "Timer") zellij.log(e.value); // seconds
    return false;
  },
});
```

The named fields: `RunCommandResult` has `exitCode`, `stdout`, `stderr`, `context`; `WebRequestResult` has `status`,
`headers`, `body`, `context`; `RunCommandOutput` and `PaneOutput` have `data` (and `paneId`); `WebRequestStreamChunk`
has `streamId`, `chunk`, `done`, `error`; `CommandPaneExited` and `EditPaneExited` have `paneId`, `exitCode`,
`context`; `CommandPaneOpened`, `EditPaneOpened` and `CommandPaneReRun` have `paneId`, `context`; `SessionUpdate` has
`sessions`, `resurrectableSessions`; `CustomMessage` has `message`, `payload`; `UserAction` has `action`, `clientId`,
`terminalId`, `cliClientId`; `ActionComplete` has `action`, `paneId`, `context`; `CwdChanged` has `paneId`, `cwd`,
`focusedClientIds`; `CommandChanged` has `paneId`, `command`, `isForeground`, `focusedClientIds`; `AvailableLayoutInfo`
has `layouts`, `errors`; `Key` and `InterceptedKeyPress` have `key`; `Mouse` has `kind` and `line`/`col` or `lines`;
the `FileSystem*` events have `files`, a list of `{path, metadata}`; and `PluginDataChanged` has `records`. Every byte
field is a `Uint8Array`. Struct payloads such as `Hook` and `AgentEvent` are not spread: read them from `value`.
`DecodedEvent` and `DecodedEventOf<K>` are the types.

`AgentEvent` is one event per hook a coding agent fired in a pane: `pane_id`, `kind`, `hook` (`UserPromptSubmit`,
`PreToolUse`, `Stop`, ...), `state`, `detail`, `prompt`, `last_message`, `tool_name`, `cwd`, `session_id`, and `raw`,
the agent's own JSON. It needs `ReadApplicationState`. See [Agent helpers](#agent-helpers).

**Order.** For one event, `update` runs first, then the pipe waiters (for a pipe), then the `on` and `onEvent`
handlers in registration order. `waitForEvent` waiters settle
before any of them and do not consume the event. Either `update` or a handler returning `true` requests a render.
The reserved pipes the SDK consumes - RPC, completions, Hocus extension messages and default-keybinding handlers -
never reach `pipe` or `waitForPipe`.

## Host commands and permissions

`zellij.command` exposes the existing host command dispatcher. Command names retain their Rust spelling.
Payloads use the command's JSON representation:

```ts
const ids = zellij.command("GetPluginIds");
zellij.command("Subscribe", ["Key", "Timer"]);
```

Commands are validated at runtime and use the same permission checks as Wasm.
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
See [sdk.d.ts](../zellij-plugin-host/src/deno/sdk.d.ts) for the SDK declarations.

### Multipane overlays

A multipane owns two panes inside one overlay frame. Each child keeps its pane ID, input, output, and scrollback.
Move, resize, hide, reveal, fullscreen, and explicit close commands addressed to either child affect the container.
Drag the divider to change the split. The host keeps that ratio across terminal resizing and fullscreen transitions.

Use `zellij.host.createMultipane(request)`, `updateMultipane(request)`, and `replaceMultipaneChild(request)`.
The Wasm equivalents are `create_multipane`, `update_multipane`, and `replace_multipane_child` in `zellij_tile::prelude`.
These commands require `ChangeApplicationState`. Launching a terminal also requires `RunCommands`, and launching a plugin requires `OpenTerminalsOrPlugins`.

```ts
const panel = zellij.host.createMultipane({
  children: [
    { Existing: { Plugin: zellij.host.getPluginIds().plugin_id } },
    { Terminal: { path: "sh" } },
  ],
  placement: { kind: "Dismissible", coordinates: null },
  split: { direction: "LeftRight", ratio: 5000 },
  focus: 0,
});
```

Children are ordered. Each child can be an `Existing` pane ID, a `Terminal` command, or a `Plugin` with `url` and `configuration`.
The direction is `LeftRight` or `TopBottom`. The ratio sets the first child's share in basis points, from 1 through 9999.
Set `focus` to slot 0 or 1, or omit it to preserve focus.

Creation returns container metadata after both slots exist. Plugin loading continues inside its reserved slot.
Use `PaneUpdate` and `PaneInfo.multipane` for the current child IDs after replacement or loading.
Permission prompts, loading errors, and exited terminals remain in their slots.

`updateMultipane` accepts `id`, optional `coordinates` and `split`, and `override_user_placement`.
Omit `split` to preserve divider changes. `replaceMultipaneChild` accepts `id`, `slot`, and `child`.
Invalid requests return an error before adoption. A coordinate batch with conflicting placements for one container is rejected.

Containers move intact between tabs. Stacking, embedding, nesting, and independent child warps are rejected.
Generic multipane persistence, CLI composition, and KDL composition are not available.

### Permissions

Privileged commands need permission, which the user is prompted for once:

```ts
export default {
  async load() {
    await zellij.waitForPermissions(["ChangeApplicationState", "OpenTerminalsOrPlugins"]);
  },
};
```

A plugin written through Hocus is granted whatever its source asks for when you approve the write - the confirmation
names them - so it never prompts in its own pane.

Calling a command you have not been granted throws, naming the permission you need - the same rule Wasm plugins live
under, except that a Wasm plugin cannot be told about it and you can:

```ts
try { zellij.command("CopyToClipboard", "hi"); }
catch (error) { zellij.log(error); } // 'CopyToClipboard' requires the 'WriteToClipboard' permission, ...
```

**A permission is not in force while `load` runs.** Every plugin starts with its permissions unanswered - even one
granted long ago, and even one Hocus pre-granted when you approved the write - because the grant arrives as a
`PermissionRequestResult` event, and `load` runs before any event does. So `load` is where you ask, and
`await zellij.waitForPermissions([...])` is what makes the rest of `load` run after the answer; a gated call made
before it is refused, and the refusal says so. `requestPermission` asks without waiting, for a plugin that does the
work in a handler:

```ts
export default {
  load() { zellij.requestPermission(["PluginData"]); }, // ask here
};
zellij.on("PermissionRequestResult", () => {
  zellij.data.define("note", {text: "Text"}); // act here, on every run
});
```

The handler fires on every load, not only the first: an already-granted permission is answered immediately rather
than prompting.

These are the names, as a plugin spells them - the `Permission` type, so the checker refuses a misspelling:

| Permission | What it covers |
|---|---|
| `ReadApplicationState` | the session's panes, tabs and modes - most `*Update` events |
| `ChangeApplicationState` | moving, closing, renaming and focusing panes and tabs |
| `OpenFiles` | opening a file in the user's editor |
| `RunCommands` | `zellij.runCommand`, commands in panes, and `zellij.agent.spawn` |
| `OpenTerminalsOrPlugins` | new terminal panes, and starting or reloading plugins |
| `WriteToStdin` | typing into another pane |
| `WebAccess` | `zellij.webRequest` |
| `ReadCliPipes` | pipes from the command line |
| `MessageAndLaunchOtherPlugins` | `MessageToPlugin` and `zellij.rpc` - what a Hocus extension registers over |
| `Reconfigure` | changing the session's configuration and keybindings |
| `FullHdAccess` | files outside `/host`, `/data`, `/cache`, `/tmp` and `/plugins` |
| `InterceptInput` | taking keys and mouse before the rest of the session |
| `ReadPaneContents` | another pane's viewport and selection, and live `PaneOutput` |
| `RunActionsAsUser` | `RunAction`, which is anything a keybinding can do |
| `WriteToClipboard` | the system clipboard |
| `ReadSessionEnvironmentVariables` | the environment the session started in |
| `PluginData` | the plugin data store, in the plugin's own namespace |
| `SharedPluginData` | a `shared:` namespace, on top of `PluginData` |
| `ReadFiles` | reading in the plugin's own folders |
| `WriteFiles` | writing in them |
| `AuthorPlugins` | writing into the plugin folder and reloading plugins by url |

A name that is not on this list grants **nothing** and raises nothing: Hocus reads the list out of your source to
pre-grant it, and a name it does not recognise is simply not there, so the plugin is denied at run time in a pane
nobody is watching. `plugin_check` and `zellij plugins check` warn about that, suggest the nearest name, and warn
about the other traps a compile cannot see.

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

The root object also provides these helpers:

- `keyName(bareKey)`: `"t"`, `"Esc"`, `"F2"`. `mouse(event)` returns `{kind, line, col}` or `{kind, lines}` with the Rust variant name as `kind`.
- `paneKey(paneId)`: `"terminal_3"`, the `PaneRenderReport` key. `terminalPane(id)` and `pluginPane(id)` build `PaneId` values.
- `bytes(value)`: a `Uint8Array` from raw event bytes, text, or an existing array. `text.sanitizeBytes(bytes)` decodes and sanitizes.
- `write(text)` and `writeToPaneId(text, paneId)` encode text as UTF-8 (`WriteToStdin`).
- `readPluginLog(name, lines = 50)` (`AuthorPlugins`), `openTerminal(cwd?)` (`OpenTerminalsOrPlugins`), `closeSelf()`, `setSelectable(boolean)`.

### Default keybindings

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
async function refreshState() {} // Your plugin's function.
const refresh = zellij.every(1000, async () => {
  await refreshState();
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


A plugin can read and write inside the folders zellij gives it, which are the same ones a Wasm plugin has preopened:

| path | what |
|---|---|
| `/host` | the plugin's own folder - normally where the session was started. A relative path is read against this one. |
| `/data` | scratch for this instance, thrown away with the session |
| `/cache` | this plugin's own folder, kept across sessions |
| `/tmp` | zellij's temp folder, **shared with every other plugin and session** |
| `/plugins` | the user's plugin folder - `~/.config/zellij/plugins/` - when the session has one |

A path is confined *after* symlinks are followed, so a link inside `/host` pointing elsewhere is refused rather than
followed out. `/tmp` is a real prefix here, so an absolute path that begins `/tmp` is read as the shared root.
Reaching outside all five folders needs `FullHdAccess`. `zellij.host.changeHostFolder(path)` (`FullHdAccess`) moves
`/host`, and `HostFolderChanged` confirms it. Zellij's own log is outside every root: `zellij.readPluginLog(name,
lines)` answers with a plugin's last lines of it and needs `AuthorPlugins`. A missing file is an error, not an empty
answer; `fs.metadata` answers `null` for something that is not there.

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


### Writing without losing somebody else's write

`rev` is the row's revision. It starts at 1 and the store advances it on every write that touches the row, which is
what lets two writers tell a row they read from a row somebody has since changed. That matters more than it used to:
the store is shared by every session on the machine, and with `plugin_data_url` pointing at Postgres it is shared by
every machine pointed at that database.

Pass the `rev` you read as `expectRev` to `put` or `update` and the write becomes a compare-and-set - it applies only
if the row is still at that revision, and the whole `apply` fails otherwise, rolling back everything else in the
batch. `zellij.data.REV_ABSENT` (0) is the other direction: the row must *not* exist yet, so a `put` guarded on it
claims a uid or fails.

```ts
const d = zellij.data;
const {records} = d.fetch({entity: "note", filter: d.eq("uid", "n1")});
const note = records[0];
try {
  d.apply([d.put("note", {title: `${note.title}!`}, {uid: "n1", expectRev: note.rev})]);
} catch (error) {
  if (d.isConflict(error)) {
    // Somebody wrote it first. Read it again and decide what to do with their version.
  } else throw error; // not a race: this request fails the same way however often it is sent.
}
```

**Ask `zellij.data.isConflict(error)` before you retry.** A failed `apply` throws for two different reasons, and they
have opposite answers: a conflict is a race, and reading the row again is what resolves it, while an entity that was
never declared, a malformed filter or a permission that was not granted will fail identically every time. A retry
loop that cannot tell them apart spins forever on the second kind.

A guarded `put` never inserts: if the row has been deleted since you read it, the write conflicts rather than
bringing it back, because resurrecting it would lose the delete just as surely as an unguarded write loses an edit.

**Pass the `rev` the *value* was composed from, not the one you just read.** Fetch the row, build the new value,
guard on the row's `rev`, write: the two reads are microseconds apart, so the guard spans a window nothing happens
in. The revision that matters is the one whoever composed the value was looking at, which for a plugin taking an
edit from a person or a model is turns or minutes back. Carry it with the value: put it in what you hand out, take
it back as an argument, and guard on *that*.

**A guard belongs to the batch, not to one row in it.** A batch that replaces something without touching the row
that names it - a note's tags, which are their own rows - is still a replacement composed from a reading, so give it
a guarded `update` of the owning row in the same batch; since `apply` is all or nothing, a refused guard rolls the
rest back. Guard the row *once*: two guarded writes of one row in a batch is the second finding the revision the
first just advanced. `apply` answers with `per_mutation` as well as `rows_written` and `rows_deleted`, for guarding
by hand with a `rev` filter instead of failing the batch.

A watch is only re-run by writes made in the same zellij server. On a shared Postgres, another machine's write does
not wake your watcher. Re-fetch if you need to see what somebody else did.

### Sharing a namespace

Everything in a plugin's own namespace is private to it, and a plugin cannot name another plugin's. Rows two plugins
share live in a **shared namespace**, which is a namespace whose name begins with `shared:`:

```ts
export default {
  async load() {
    // Both permissions, in every plugin that touches the namespace - the reader too.
    await zellij.waitForPermissions(["PluginData", "SharedPluginData"]);
    const watch = zellij.data.watch({entity: "todo", namespace: "shared:todo", sort: [["id", "Desc"]]}, () => true);
    zellij.log(watch.initial.records.length);
  },
};
```

Three rules, and every one of them is a load error or a denial rather than something that degrades quietly:

- **The prefix is not optional.** `namespace: "todo"` is refused; `"shared:todo"` is the same store. The segment
  after the colon is lower-case letters, digits, `-` and `_`.
- **`SharedPluginData` as well as `PluginData`**, in every plugin that reads or writes it.
- **One plugin defines; the rest only use.** `define` is a migration: two plugins racing to define the same entity
  is a bug waiting for the day their two spellings differ. Imports are confined to the plugin file's own folder, so a
  `hocus/` extension and a top-level pane cannot share a schema module - which makes "the owner defines, the reader
  fetches" the pattern rather than a preference. If a reader must be defensive, define exactly what the owner defines
  and let the error out; never swallow it into an empty list.

`namespace` is an option of `define` (its third argument), of the query given to `fetch`, `count` and `watch`, and of
`apply`'s second argument; `catalog(namespace)` takes it positionally.

### Try it on twenty lines first

The namespace, the permissions and the column names are all settled by one reload of a plugin that does nothing
else. Write this before the real thing, not after:

```ts
import zellij, {definePlugin} from "zellij:plugin";
export default definePlugin({
  async load() {
    await zellij.waitForPermissions(["PluginData", "SharedPluginData"]);
    const d = zellij.data;
    const defined = d.define("todo", {title: {kind: "Text", required: true}, done: {kind: "Bool", default: false}}, {namespace: "shared:todo"});
    zellij.log(JSON.stringify(defined));
    d.apply([d.put("todo", {title: "probe"})], {namespace: "shared:todo"});
    zellij.log(`rows: ${d.fetch({entity: "todo", namespace: "shared:todo"}).records.length}`);
  },
});
```

`plugin_reload` it, `plugin_log` it. A cycle spent here is a cycle not spent rewriting a finished plugin around a
constraint it was built against.

### Finding what is already there

**You do not declare an entity you only read** - `define` is for entities you own, and a fetch works against
anything already in the store. `data.catalog()` says what that is:

```ts
for (const entity of zellij.data.catalog()) { // every namespace this plugin may see
  zellij.log(`${entity.namespace} ${entity.def.name} v${entity.def.version}`);
  for (const attr of entity.def.attrs) zellij.log(`  ${attr.name} ${attr.kind}${attr.indexed ? " indexed" : ""}`);
}
zellij.data.catalog("shared:hocus"); // or just one namespace
```

With no argument it lists the plugin's own namespace plus every `shared:` one, once `SharedPluginData` is granted.
Another plugin's private namespace is never listed.

### Hocus data

Hocus keeps its projects, todos, notes, scheduled prompts and coding-agent records in the shared namespace
`shared:hocus`, declared at session start whether or not a Hocus pane is open. Any plugin holding `PluginData` and
`SharedPluginData` can read them. `plugin_docs section=hocus_data` (the "Hocus data" section, whose tables are
language-neutral) lists every entity and attribute, and `hocus-todos.ts` and `hocus-notes.ts` in `example-plugins/`
are working read-only plugins over them:

```ts
const d = zellij.data;
await zellij.waitForPermissions(["PluginData", "SharedPluginData"]);
const todos = d.fetch({
  namespace: "shared:hocus", entity: "todo",
  filter: d.and(d.eq("root", "/home/me/project"), d.eq("done", false)),
  sort: [["id", "Asc"]],
});
zellij.log(todos.records.length);
```

**Read these; do not write them.** Nothing blocks a write, but Hocus maintains invariants the rows do not carry, so a
row added by hand can produce a list Hocus will not accept. Mutate through Hocus's tools (`/todo_add`, `/todo_done`,
`/note_add`) or `zellij pipe` to `hocus-store` instead. `id`, `uid`, `created_at`, `updated_at`, `rev` are on
every row without being declared, here as anywhere else, and the storage engines' own column names (`rowid`, `oid`, `xmin`, `xmax`, `cmin`, `cmax`, `ctid`, `tableoid`) are
reserved with them; a `define` that names any of these fails whole.

## Hocus data

Hocus keeps its projects, todos, notes, scheduled prompts and coding-agent records in the shared
namespace `shared:hocus`, and `hocus-store` declares every entity at session start - so the
tables are there whether or not a hocus pane is open. Any plugin holding `PluginData` and
`SharedPluginData` can read them:

```ts
import zellij from "zellij:plugin";
const projectRoot = "/work";
const d = zellij.data;
const todos = d.fetch({
  namespace: "shared:hocus", entity: "todo",
  filter: d.and(d.eq("root", projectRoot), d.eq("done", false)),
  sort: [["id", "Asc"]],
});
```

`example-plugins/hocus-todos.ts` is a working read-only plugin over exactly this, and
`example-plugins/hocus-notes.ts` is the same over the notes and their side tables.

**Read these; do not write them.** Nothing blocks a write, but hocus maintains invariants the
rows do not carry - list caps, tree shape, project discovery, a note's tags and links - so a
plugin that adds a row by hand can produce a list hocus will not accept. Mutate through hocus's
tools (`/todo_add`, `/todo_done`, `/note_add`) or `zellij pipe` to `hocus-store` instead.

`id`, `uid`, `created_at`, `updated_at` and `rev` are on every row without being declared, here
as anywhere else.

**`project`** - the project registry. `uid` is the name.

| Attribute | Kind | |
|---|---|---|
| `name` | text | required; also the uid |
| `root` | text | required, indexed; the folder. Not unique - resolve a cwd by *longest matching root* |
| `firecracker_image` | text | the default Firecracker image for agents in this project |
| `worktree_dir` | text | optional default parent directory for new worktrees |
| `remote` | text | |
| `worktree_status` | text | snake case, as serde spells it |
| `common_dir` | text | the main checkout, for a worktree |
| `discovered` | bool | required; true for one found by a running agent rather than named by hand |
| `replaced_ids` | json | optional array of project row IDs replaced by explicit renames, merges, or adoption, including earlier replacements |

Set a project's `worktree_dir` through `project_open` or `project_assign`. Omit the argument to keep the saved setting.
Use an empty string to clear it. `project_list` returns the setting. NUL bytes are rejected.

For new worktrees, an explicit launch directory takes precedence over the project setting.
If neither is set, Zellij uses `.worktrees/`. The branch name is appended to that directory.
Relative paths, including sibling paths such as `../agent trees`, resolve against the repository's primary checkout.
Absolute paths are also accepted. Zellij does not expand shell variables or `~`.

The setting applies to Rust and TypeScript agent launches, Hocus, and the New Agent dialog.
The dialog displays the default and allows a launch override. Existing worktrees stay in place.
Forks continue to use the source agent's checkout.

**`todo`** - one item. The tree is edges, not nesting.

Todo lists have no total item cap. Fetch limits are page sizes. Read every page before building a tree from `parent_id` links.

| Attribute | Kind | |
|---|---|---|
| `root` | text | required, indexed; the project's `root`, which is how a list is scoped |
| `parent_id` | int | the store `id` of the item above; null at top level |
| `text` | text | required, full-text searchable; Markdown |
| `done` | bool | required, indexed |

There is no `position` and no `depth`: siblings are only ever appended, so `id` ascending *is*
sibling order, and depth follows from the parent chain. One project's whole list fits in a single
fetch.

**`note`** - one note: a title and a Markdown body, kept with a project the way a todo list is,
or with none.

| Attribute | Kind | |
|---|---|---|
| `root` | text | indexed; the project's `root`, or **null for a global note** - filter with `isNull` |
| `title` | text | required, full-text searchable |
| `body` | text | required, full-text searchable; Markdown, up to 16 KB |
| `author` | text | required; `user`, `hocus` for one the model wrote, or an agent kind |
| `archived` | bool | required, indexed; hidden from hocus's listings until un-archived |

The note's timestamps are the store's own `created_at` and `updated_at`, in milliseconds. Both
text columns are in one full-text index, so `search = "word"` finds it in either and answers one
row per note. A listing should `select` everything *but* `body` - a thousand notes at the cap
would exceed one fetch's byte limit, and a listing does not show one anyway.

**`note_link`** - a note referencing one todo item, many to many. `uid` is `<note id>:<todo id>`.
**`note_tag`** - one tag on one note. `uid` is `<note id>:<tag>`.

| Attribute | Kind | |
|---|---|---|
| `note_id` | int | required, indexed; both entities |
| `todo_id` | int | required, indexed; `note_link` only - the store `id` of a `todo` row |
| `tag` | text | required, indexed; `note_tag` only - lowercase, `[a-z0-9_-]` |

A note's tags and links are the rows naming its id; a todo item's notes are the `note_link` rows
naming its `todo_id`; and a list's notes are the notes with its `root` plus the ones linked to
any item in it. Fetch a side table with `in("note_id", ids)` for the notes on screen rather
than the whole table - and not at all when there are none, since an empty `in` is refused.

**`job`** - a scheduled prompt.

| Attribute | Kind | |
|---|---|---|
| `host_id` | text | required, indexed; the machine that armed it. `uid` is `<host id>:<job id>` |
| `zellij_session_name` | text | required, indexed; the session that armed it, as attribution — not part of the key |
| `job_id` | int | required; a per-machine counter, so it is *not* unique across machines |
| `chat_id` | int | the chat that armed it; null for one never saved |
| `every` | real | seconds |
| `next_due_at` | int | wall-clock seconds; written when armed and when it fires, never per tick |
| `prompt` | text | required |
| `show` | bool | required |
| `auto_confirm` | bool | required |

**`agent_latch`** - a kind set on a pane by hand with `agent_register`. **`agent_summary`** - what
the daemon's model last said about an idle agent's pane. Both are keyed
`<host id>:<session>:<pane id>`:

| Attribute | Kind | |
|---|---|---|
| `pane_id` | int | required, indexed |
| `zellij_session_name` | text | required, indexed |
| `host_id` | text | required, indexed; the machine the pane is on |
| `kind` | text | `agent_latch` only - `claude`, `codex`, ... |
| `text`, `title` | text | `agent_summary` only |
| `awaiting` | bool | `agent_summary` only |
| `created_at_ms` | int | `agent_summary` only - epoch milliseconds it was made; absent on a summary stored before this attribute existed |

The session and the host are part of the key because the store is neither per-session nor, with
Postgres, per-machine: a bare pane id would make two sessions' pane 5 into one row, and a
session name alone would do the same to two machines that both run a session called `main`. A
pane id means nothing outside the server that handed it out, so filter on **both**
`host_id` and `zellij_session_name` before believing a `pane_id`. Rows written before this
existed have an empty `host_id`; do **not** treat those as yours - they name panes in a server
that is gone, and hocus deletes them on its next restore.

**`agent_run`** - one hook-backed agent session, live or finished. `uid` is `<kind>:<session
id>`, or `<kind>:pane:<pane id>:<started at>:<host id>` for a hook that reports no session id of
its own. The machine is in the second shape because a pane id and a millisecond are a name two
servers sharing the store can reach for at once; the first needs no help, since the agent's own
session id is a uuid.

| Attribute | Kind | |
|---|---|---|
| `pane_id` | int | required, indexed |
| `session_id` | text | required, indexed; the agent's own |
| `zellij_session_name` | text | indexed |
| `host_id` | text | required, indexed; the machine whose server owns the run. Empty on runs recorded before this attribute existed, which reads as *this* machine - unlike `agent_latch` above, a run is history and is claimed rather than dropped |
| `kind` | text | required |
| `active` | bool | required, indexed; false once the run ended |
| `started_at_ms` | int | required |
| `header` | json | the cheap summary an archive listing reads |
| `payload` | json | required; the run's metadata |

`payload` is the larger of the two columns, so `select: ["header"]` a listing and fetch
`payload` only for the row you are showing. Decode either with `JSON.parse`.

**`agent_run_entry`** - one retained event on a run's timeline: a prompt, a plan, a question, a
result, or a summary somebody asked for. `uid` is `<run uid>#<revision>`.

| Attribute | Kind | |
|---|---|---|
| `run_id` | text | required, indexed; the `agent_run` uid this belongs to |
| `host_id` | text | required, indexed; where the entry was recorded. Attribution only — a run id does not collide between machines either way, carrying the agent's own session uuid or, in the fallback shape, the machine — so entries do not collide |
| `revision` | int | required; the run's own counter, and what orders the timeline |
| `event_id` | text | the provider's id for the event behind it, where it has one |
| `kind` | text | required; `prompt`, `plan`, `question`, `step`, `result`, `summary` |
| `text` | text | required; the whole of what the agent said, uncut |
| `created_at_ms` | int | epoch milliseconds this entry was recorded; absent on an entry recorded before this attribute existed |

A row per event rather than a list inside `payload`, so a long-running agent costs one small
insert per event instead of a rewrite of its whole history - and so nothing has to be cut down
to keep that rewrite cheap. Filter by `run_id` and sort by `revision`; a run recorded by an
older hocus has no rows here and keeps its timeline inside `payload` instead.

Pane-level agent state - busy, waiting, idle - does not need this table at all: it is on
`PaneInfo.agent` in the pane manifest, and the `agent_start` / `agent_waiting` hook points below
fire on the transitions.

## Work queues

A plugin can put *other plugin code* on a named **work queue**, to run later as its own headless plugin instance
when the queue has a free slot. Each queue runs at most `concurrency` jobs at once (one, until someone says
otherwise), so a queue is how you build a merge queue, a deploy pipeline, or anything else that must not run
everything at the same time.

The enqueuer:

```ts
zellij.subscribe(["QueueJobStarted", "QueueJobFinished", "QueueChanged"]);
zellij.host.configureQueue({queue: "merge", concurrency: 2});

const jobId = zellij.host.enqueueQueueJob({
  queue: "merge",
  plugin_url: "file:merge_one.ts",
  config: {branch: "feature/x"}, // the job's configuration
  context: {pr: "42"}, // yours, echoed back; the job never sees it
});

zellij.on("QueueJobStarted", (e) => { zellij.log(e.queue, e.job_id, e.plugin_id, e.context.pr); });
zellij.on("QueueJobFinished", (e) => {
  // e.outcome is "Succeeded", "Failed", "Cancelled" or "Aborted"; e.payload is what the job
  // reported (or the error, for a job that failed to load or crashed)
  zellij.log(e.outcome, e.payload);
});

const queues = zellij.host.queueStatus({queue: "merge"}); // {queue: null} for every queue
zellij.host.cancelQueueJob({job_id: jobId});
```

The job is an ordinary plugin with no `render`. It is loaded `shared`, so it is one instance for the whole session
however many clients are attached (its configuration carries `shared: "true"`). It learns what it is from its
configuration and ends itself by reporting:

```ts
import zellij, {definePlugin} from "zellij:plugin";
export default definePlugin({
  async load(config) {
    // config.zellij_queue, config.zellij_queue_job_id, config.zellij_queue_enqueuer_plugin_id,
    // plus whatever the enqueuer put in `config` (here, config.branch) and `caller_cwd`.
    await zellij.waitForPermissions(["RunCommands"]);
    const result = await zellij.runCommand({command: ["git", "merge", config.branch]});
    zellij.host.finishQueueJob({success: result.exitCode === 0, payload: `merged ${config.branch}`});
  },
});
```

`finishQueueJob` records the outcome and unloads the job; the enqueuer's slot frees and the next pending job starts.
A job that closes itself, or whose pane is closed, ends as `Aborted`; one that fails to load or throws ends as
`Failed`, with the error as its payload. `QueueChanged` goes to every subscriber whenever a queue's contents or limit
change, and carries the same `pending` and `running` lists `queueStatus` does.

Permissions: `enqueueQueueJob` needs `MessageAndLaunchOtherPlugins`, `configureQueue` and `cancelQueueJob` need
`ChangeApplicationState`, and `queueStatus` (and the `QueueChanged` event) need `ReadApplicationState`.
`finishQueueJob` needs nothing: a job runs headless, and a permission prompt it raised would have no pane to be
answered in - so a job requests what it needs in `load`, as a headless hook does, and the first grant is remembered.

Queued jobs keep running after their enqueuer reloads. Addressed start/finish events belong to the original
instance; the replacement can query `QueueStatus` and receives broadcast `QueueChanged` events. Two things a queue
does not do: remember jobs across sessions, and tell an enqueuer that has since unloaded anything - its events are
dropped, and the job runs to completion regardless.

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

Handlers can send JSON progress with `call.progress(payload)`. Callers receive it through the optional `onProgress(payload)` callback.
Progress does not settle the call or change its deadline. A callback error rejects the local wait.
Completion, cancellation, and errors remove the local progress callback and timer. Progress sent after a handler completes is ignored.

Handler metadata contains `version`, `callId`, `name`, `payload`, `progress`, and the host-supplied `source: {Plugin: number}`.
Client context survives `await`. Sender-supplied `source` and `reply_to` fields are ignored.
ID-addressed calls accept replies only from that plugin ID. URL-addressed calls match the opaque call ID because the host does not return the destination ID.
Each isolate uses a new UUID prefix, so replies from an old instance cannot settle replacement calls.

RPC uses version-1 `zellij_rpc_call`, `zellij_rpc_reply`, and `zellij_rpc_progress` envelopes, with a 1 MiB (1,048,576-byte) limit.
Progress carries `{version: 1, call_id, payload}` and uses the same source validation as replies.
Hocus tool calls, replies, and progress retain their 64,000-byte limit.
Unsupported JSON values reject instead of losing fields. Malformed, stale, and duplicate replies are ignored.
RPC messages stay internal. Ordinary pipe messages still reach `pipe` callbacks and `waitForPipe()`.

## Embedded builtins and server identity

The script builtins `context-menu`, `agent-policy`, `swamp-bridge`, `swamp-workflow`, and
`agent-exit` use embedded TypeScript. Their `zellij:` URLs remain unchanged.
The bridge retains its delivery namespace, schema, correlation keys, and revision guards.

Embedded plugins can import `zellij:plugin` and the registered `zellij:workflow-view` module.
The shared view resolves from embedded source during execution and semantic checking.
There is no filesystem fallback. Filesystem plugins retain their directory confinement.

New plugins and reference examples use TypeScript. Extensionless authoring names select `.ts`.
Configuration uses `config.kdl`. See [Migrate from Lua](LUA_REMOVAL.md) for the breaking removal.

The SDK exposes four readonly strings during module initialization:

- `serverBinary`: the resolved server executable path.
- `serverSession`: the server session identity.
- `serverIncarnation`: one UUID per server process, retained across plugin reloads.
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
const result = await zellij.hocus.complete({messages: [{role: 'user', content: 'What time is it?'}], tools: []});
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
Hocus accepts `.js`, `.mjs`, `.ts`, and `.mts`. Backups are excluded.
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

See [weather-extension.ts](../example-plugins/weather-extension.ts), or `plugin_docs section=example_weather_extension`.

## Rendering

`render(rows, columns)` returns the frame as a string, and ANSI escapes work in it. A render is asked for when
`update`, `pipe`, an `on` handler or a timer callback returns `true`, or when `zellij.requestRender()` is called;
requests coalesce while one is pending, and a state change or resize invalidates an outdated async render result
before it is drawn. `render` may be `async`. A plugin that wants to look like part of zellij uses the same components
the builtin plugins do, drawn by zellij in the user's theme:

```ts
const ui = zellij.ui;
let panes: {id: number; command: string}[] = [];
export default {
  render(rows: number, cols: number) {
    const table = ui.table([["pane", "command"]]); // the first row is the title row
    for (const pane of panes) table.row(String(pane.id), ui.text(pane.command).dim());
    return [
      ui.text("panes").selected().render(),
      table.render(),
      ui.nestedList(["one", {text: "two", indent: 1}]).render(),
    ].join("\n");
  },
};
```

`render(coords)` gives the string to put in the frame. Without an argument the component is drawn where the cursor
is, so components join with `"\n"` like any other lines; with `{x: 0, y: 2, width: 40, height: 5}` it is drawn there,
and clipped to the width and height if they are given. A `StyledText` also renders as a string where one is expected
(`` `${ui.text("x")}` ``). Text is measured in columns, which is neither bytes nor code points: `zellij.text.width(s)`
is the answer, and `pad`, `padLeft`, `center` and `truncate` are built on it, so a box a plugin draws itself lines up
whatever is in it.

### Shared UI and terminal text

`zellij.ui` uses the same Rust rendering toolkit as Wasm plugins.
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

See [forms.ts](../example-plugins/forms.ts), or `plugin_docs section=example_forms`.

## Hooks

A **hook point** is a moment in the session's life that a plugin can own. Bind one in the config:

```kdl
hooks {
    agent_exit "file:my-agent-exit.ts"
}
```

A `file:` path that is not there is looked for in the plugin folder, so a bare name is a hook written with
`plugin_write name=my-agent-exit.ts`. An absolute path works too, and means exactly what it says.

The hook points:

| Point | When | Shape |
|---|---|---|
| `agent_exit` | an agent pane's command exited | opened **in place of** that pane |
| `agent_start` | a pane gained an agent record: spawned as an agent, or its agent reported for itself | headless, `Hook` event |
| `agent_waiting` | an agent's record moved to `waiting` - a permission prompt, a question | headless, `Hook` event |
| `agent_idle` | an agent's record moved to `idle` from `busy` or `waiting`: its turn ended | headless, `Hook` event |
| `pane_exit` | any command pane's command exited, held or not | headless, `Hook` event |
| `session_start` | the session's first client connected | headless, `Hook` event |
| `context_menu` | a right-click asked for a menu | headless, `ContextMenuRequested` event |

A point has one of two shapes, and the shape is a fact about the point. `agent_exit` opens its plugin **in place
of** the pane, once per firing, with the payload as the plugin's configuration. Every other point runs its plugin
**headless**: one long-lived instance, loaded without a pane the first time the point fires, receives every later
firing as an event. A hook point takes **one** plugin; bind the empty string to turn a hook off. Zellij does not
rewrite your own config file, so the `hooks { }` block is yours to add - or, from a shell,
`zellij action set-hook agent_exit file:my-agent-exit.ts --write` binds the point at once and persists the binding
in `config.d/zellij-managed.kdl` beside your config. Hocus can write the plugin but not bind it.

### The `agent_exit` hook

The exited pane is *suppressed* behind the hook, not closed: its scrollback survives, and closing the hook pane is
what brings it back. So a hook that wants to leave the pane alone just closes itself.

The payload arrives as the plugin's configuration, so `load(config)` receives it. Every key is prefixed, because an
unprefixed `cwd` would collide with a reserved configuration key and arrive empty:

| Key | |
|---|---|
| `agent_pane_id` | the pane the hook stands in for, as a string |
| `agent_cwd` | where the agent was working |
| `agent_exit_token` | host-generated correlation token, retained when the hook reloads |
| `agent_exit_code` | absent if the command was killed by a signal |
| `agent_command` | the command that exited, for display |
| `agent_kind` | the last recognized agent kind |
| `agent_target` | `local`, `ssh`, or `firecracker` |
| `agent_workflow_owner`, `agent_workflow_run`, `agent_workflow_role` | helper handoff context, when present |
| `agent_firecracker_vm`, `agent_firecracker_worktree`, `agent_session` | for a worktree-backed Firecracker pane only |
| `agent_zellij_executable`, `agent_zellij_config_file`, `agent_zellij_config_dir` | the host binary and configuration, for Firecracker management commands |

The shipped `agent_exit` hook is `zellij-config/assets/plugins/agent-exit.ts` in the source tree, and
`example-plugins/agent-commit.ts` is a small commit-only example (`plugin_docs section=example_agent_commit`). Read
them; do not copy them. Your own hook is a plugin you write: `plugin_write name=my-agent-exit.ts`, then bind it. A
file-loaded copy needs permissions for commands, project config reads outside its directory, pane state and changes,
and messages to its workflow owner; the builtin has them automatically. Wait for them before initializing, and
nothing initializes after a denial:

```ts
import zellij, {definePlugin} from "zellij:plugin";
export default definePlugin({
  async load(config) {
    await zellij.waitForPermissions([
      "ChangeApplicationState", "ReadApplicationState", "RunCommands",
      "ReadFiles", "FullHdAccess", "MessageAndLaunchOtherPlugins",
    ]);
    zellij.log(`standing in for pane ${config.agent_pane_id ?? "(none: opened by hand)"}`);
  },
});
```

Three notes on writing one:

- **A reload gives it no payload.** `plugin_reload` - and any other way of opening the plugin by hand - starts it
  outside the hook, so `load` gets no `agent_pane_id` at all. Write the hook so that it still renders, with whatever
  it does to the pane simply skipped. It is also the only way to see the hook render without exiting an agent.
- **Restarting does not need the command.** `zellij.host.rerunCommandPane(paneId)` makes the pane replay what it
  already holds, which reaches it even while it is suppressed.
- **Closing has an order.** Close the agent pane *first*, then yourself - closing yourself first restores it instead.

Because a hook is an ordinary plugin, anything a plugin can do is reachable at agent exit - including starting the
next agent: `zellij.host.openCommandPaneInGitCheckoutNearPlugin` opens one on a branch or in a worktree of its own,
and the pane it opens is itself an agent pane, so this hook runs again when *that* command exits.

### Project-owned Swamp workflows

The builtin `agent-exit` hook launches the workflow bound in the control checkout.
Escape closes both the held agent pane and the exit view without deleting files, with or without a configured workflow.
When the answer field has focus, Escape and Delete edit the answer instead of closing or deleting the pane.
During deletion confirmation, Escape dismisses the confirmation. During deletion, the exit view ignores keys.
Without a configured workflow, Enter restarts the agent.
A verified local or Firecracker secondary worktree always offers **Delete worktree & branch and close pane**.
This option is available while the workflow starts, runs, waits for input, or has completed or failed.
It is also available without a workflow. Configuration errors remain visible and do not prevent checkout inspection.

Press Delete to refresh the checkout identity and show its worktree, branch, revision, associated VM, and cancellation warning.
Deletion can discard uncommitted work and unmerged commits.
After the full confirmation is visible, press Delete again to confirm, or Escape to dismiss it.
If the pane cannot display the confirmation, enlarge it before confirming.
Confirmation cancels the workflow and waits for verified termination of its owned operations before deletion.
The exit pane stays open during cancellation and shows failures for retry. New operations cannot start during cleanup.
Deletion force-removes the worktree and compare-deletes its branch at the confirmed HEAD with `git update-ref -d`.
It then closes both panes.
Primary checkouts, SSH panes, non-repositories, and unverifiable checkouts do not offer deletion.
A detached secondary checkout removes only the worktree.

Local deletion revalidates the worktree immediately before removing it. Firecracker deletion validates and removes
the worktree under the provider's lease lock. Failures remain visible in the exit view and can be retried, including
a branch deletion that failed after the worktree was already removed.
Failure actions describe the remaining cleanup. A removed worktree cannot restart an agent.

Configure `.zellij/swamp-workflows.kdl` in the control checkout:

```kdl
agent_exit {
    workflow "@hammz/zellij-agent-exit"
    ci "cargo" "x" "ci" "check"
    default_branch "main"
}
```

The supplied workflow spawns a work agent, observes explicit completion, syncs its VM, asks a separate agent for
structured review findings, commits changes, rebases onto the local target, and runs the configured gate. Two
automatic repair rounds are written in the definition. Unresolved work then suspends for instructions. Integration
and cleanup each require an answer to a separate question. No remote Git operations are included. See
[SWAMP_WORKFLOWS.md](SWAMP_WORKFLOWS.md) for setup, custom definitions, questions and recovery.

`zellij.projectConfig.read(hostDirectory)` returns `{root, source, config, error}` and needs `ReadFiles`. An invalid
source is returned with an error so setup can correct it. Operational callers must stop when `error` is present.
`zellij.projectConfig.write(hostDirectory, source, expected)` validates before writing and needs `WriteFiles`. Pass
the exact previous source as `expected`, or `null` for an absent file. Stale writes fail. Writes use an OS lock and
atomic replacement; they refuse file and directory symlinks. Reads do not create `.zellij`.

Linked worktrees resolve through the primary checkout. An optional `control_checkout "path"` inside `agent_exit`
redirects to another control directory; relative paths resolve against the primary checkout. Both directories
require `FullHdAccess` when outside the plugin directory. Configuration, definitions, models, and Swamp state all
resolve through that control checkout.

`zellij.agent.spawnDetails(options)` returns the actual `{pane_id, branch, cwd}`; `zellij.agent.spawn(options)`
returns the pane ID. The builtin `swamp-bridge` routes typed `@hammz/zellij` operations through host
implementations. It loads headless on first use, with no pane, and survives closing the tab it was first addressed
from. Its durable records contain host, session, server incarnation, agent ownership, correlation keys,
dispatch claims and attachments. Pane IDs alone never prove ownership. Managed helpers report their exits to their
operation and do not start another exit workflow.

A workflow runs in a separate supervised command pane. Closing its view detaches; `zellij workflow show <run-id>`
reopens it. Cancellation is a separate action and preserves resources. The view reads Swamp history, questions,
reports and recorded command scrollback. Terminal history has a retention limit, which the output marks. Idle
transitions and pane closure never count as success.

Showing the same directory and run or launch preserves output, cached reports, scroll position, form state, and any pending submission.
An omitted launch reuses the known launch. An explicitly different run or launch resets the view.
Each show requests a refresh and restores the subtitle.

`zellij.swampRequest()` accepts an optional `onProgress(payload)` callback for correlated `managed.progress` frames.
These frames do not complete the request. The bridge forwards answer-admission progress to the view.
The view suspends its 90-second answer deadline on `waiting` or `reloading` and starts a fresh deadline on `ready`.
Submission stays disabled until completion or error. Switching attachments cancels the local wait and removes its timer.

### Headless hooks

`agent_start`, `agent_waiting`, `agent_idle`, `pane_exit` and `session_start` share one plugin shape. The plugin is
loaded the first time its point fires and stays loaded; each firing is a `Hook` event carrying the point's name and a
payload typed per point. A firing that arrives while that first load is still under way is held for the loading
instance and delivered once it is up, in order - two points firing within a few milliseconds of each other reach
one plugin, not two.

**Quickstart.** A headless hook ships with zellij, `zellij:agent-policy`
(`zellij-config/assets/plugins/agent-policy.ts` in the source tree). It logs every firing it receives and raises a
notification when an agent stops for a question. Nothing binds it by default. To see the shape work, from a shell in
the session:

```sh
zellij action set-hook agent_waiting zellij:agent-policy --write
zellij action set-hook agent_start zellij:agent-policy --write
zellij run -- claude          # or type `claude` in a pane whose hooks are installed
zellij plugins log agent-policy
```

The first `agent_start` loads the plugin and is delivered to it; each line it logs reads
`[deno:agent-policy] hook agent_start {"pane_id":7,"agent":{"kind":"claude",...},...}`, and the next permission
prompt in that pane raises a toast that focuses the pane when clicked. To make it yours, copy the file into your
plugin folder as a plugin of its own - `plugin_write name=my-policy.ts` through Hocus, or
`zellij plugins write my-policy.ts < agent-policy.ts` - request `ChangeApplicationState` in its `load` (the shipped
copy is a builtin and needs no grant; yours is not), and bind `file:my-policy.ts` instead. The whole plugin is:

```ts
import zellij, {definePlugin} from "zellij:plugin";

zellij.on("Hook", ({point, payload}) => {
  zellij.log(`hook ${point} ${JSON.stringify(payload)}`);
  if (point === "agent_waiting" && "agent" in payload) {
    zellij.host.notify({
      message: `${payload.agent.kind || "agent"} in pane ${payload.pane_id}: ${payload.agent.detail || "is waiting for you"}`,
      paneId: zellij.terminalPane(payload.pane_id),
    });
  }
});

export default definePlugin({
  load() {
    zellij.subscribe(["Hook"]);
    zellij.requestPermission(["ChangeApplicationState"]); // for a file: copy
  },
});
```

`paneId` makes the toast clickable: a left-click on it dismisses the toast and focuses that pane. Add
`click: {message, payload}` and the click also comes back to *this* plugin as a `CustomMessage` event carrying that
pair - which is how a plugin does something more particular than "be looked at" when its toast is clicked. It needs
a `CustomMessage` subscription, and rides on `paneId`, so a click without a pane is dropped.

One plugin may be bound to several points; `point` tells them apart. The plugin has no pane, so `render` is never
asked for and a permission prompt has nowhere to appear: request what you need in `load`, and the first grant is
remembered.

The payloads, typed (`HookPayload`) - a number is a number and the agent's record is the record:

| Point | Fields |
|---|---|
| `agent_start`, `agent_waiting`, `agent_idle` | `pane_id`; `agent`, the record as it now stands (`kind`, `state`, `source`, `detail`, `cwd`, `session_id`, `run_id`); `previous_state` (`Busy`/`Waiting`/`Idle`/`Exited`, `null` for a record that just appeared); `command`, if the pane is a command pane |
| `pane_exit` | `pane_id`, `exit_code` (`null` if killed by a signal), `cwd`, `command`, `held` (`true` if the pane holds and can be re-run, `false` if it is closing) |
| `session_start` | `session_name`, `client_id` |

What fires `agent_start`: a pane created with an agent record (a plugin's `zellij.agent.spawn`), an agent record
appearing on a pane that had none (`claude` typed at a shell, reporting through its hooks), a record whose last state
was `exited` coming back (`RerunCommandPane`, the shipped `agent_exit` hook's Enter), and a new `session_id` in the
same pane. `agent_waiting` fires on the edge into `waiting` only - a second `waiting` report with a new detail is the
same wait. `agent_idle` fires on the edge into `idle` from `busy` or `waiting` - the turn ended - and not for a record
that appears idle, which is `agent_start`'s. Clearing a record and moving to `exited` fire nothing; the exit is
`agent_exit`'s and `pane_exit`'s. A pane that has ever held a record is an agent pane for as long as it lives; only a
pane re-run with a *different* command forgets it. For a pane that is both, `pane_exit` fires before `agent_exit`,
so an observer hears of the exit before the pane is covered.

`session_start` fires when the first client connects. The first tab may not exist yet - a hook that wants to open
panes should subscribe to `TabUpdate` and act on the first one; the point is for registering state, timers and
subscriptions.

### The `context_menu` hook

The odd one out: it is given no pane at all. A right-click menu is a thing zellij draws over the panes and
hit-tests itself, so the plugin's whole job is to say what is on it. It gets no `render`, and it stays loaded between
clicks rather than being opened per click.

```kdl
context_menu_on_right_click true   // in the options; the menu is opt-in

hooks {
    context_menu "file:menu.ts"
}
```

```ts
import zellij, {definePlugin, type ContextMenuItem} from "zellij:plugin";

zellij.on("ContextMenuRequested", (event) => {
  const items: ContextMenuItem[] = [];
  if (event.link?.matched) {
    items.push({label: `Open ${event.link.matched}`, context: {url: event.link.matched}});
  }
  for (const item of event.items) { // what the config declared
    if (item.label !== "Copy" || event.selected_text !== null) items.push(item);
  }
  zellij.command("SetContextMenu", {request_id: event.request_id, items});
});

zellij.on("ContextMenuItemSelected", ({item_context}) => {
  if (item_context.url) zellij.log(`open ${item_context.url}`);
});

export default definePlugin({
  load() { zellij.subscribe(["ContextMenuRequested", "ContextMenuItemSelected"]); },
});
```

The event (`ContextMenuContext`) carries the click: `pane_id`, `click` and `click_in_pane`, `on_frame`,
`pane_title`, `pane_command`, `selected_text`, `word_under_cursor`, `line_under_cursor`, `link` (a `link`-plugin
regex match under the cursor), and `items` - what the `context_menu { }` config block declared, in order.

Two things it does *not* carry, both because getting them would mean blocking the thread every mouse movement goes
through: the pane's **cwd** (ask for it with `zellij.host.getPaneCwd`) and its **scrollback**
(`zellij.host.getPaneScrollback`). Both are ordinary calls; re-render when the answer arrives.

**Items you were given have an `id` and no actions.** Hand one back with its `id` and its configured actions come
back with it - so you can relabel, reorder and drop freely. An item you build yourself has no `id`, runs no actions,
and reaches you as `ContextMenuItemSelected` carrying its `context` as `item_context`, from where `RunAction` can do
anything an action could.

Answering is not urgent and cannot be too late: `request_id` says which menu you are answering, and an answer for
one that has been dismissed or replaced is discarded. If you do not answer within a tenth of a second the menu shows
the configured items on its own, so a plugin that is slow, wedged or absent never makes right-click do nothing.
Answer with an empty list to say "no menu here". **`ContextMenuItemSelected` goes to the plugin that offered the
item, and to nobody else.**

The shipped hook, `zellij:context-menu`, is `zellij-config/assets/plugins/context-menu.ts`. It probes `xdg-open`
and `open` once at load to select the platform link opener.

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

## Errors

Host commands throw ordinary exceptions, and an exception a lifecycle callback lets out - or a rejection of a
promise the SDK is tracking - is reported against your plugin rather than taken quietly: the error text replaces the
pane's contents, with the TypeScript file and line it came from (each instance keeps its source maps). Catch what
you want to handle yourself.

To see what a plugin is doing short of an error, write to zellij's log:

```ts
const key = "t";
zellij.log("pressed", key); // console.log(...) goes to the same place
console.log("also", key);
```

Each line lands in `$TMPDIR/zellij-<uid>/zellij-log/zellij.log` tagged `[deno:<your file>]`, and
`zellij plugins log hello.ts` or Hocus `plugin_log` reads one plugin's lines back. `console.log`, `info`, `warn`,
`error`, `debug` and `trace` go there too, from module top level onward, because a plugin has no terminal of its
own. Both join their arguments with tabs. `zellij.readPluginLog(name, lines)` reads another plugin's lines
(`AuthorPlugins`). See [Tracing](#tracing) for a transcript of every command and event.

## What TypeScript plugins may not do

A Wasm plugin is sandboxed by its runtime. A TypeScript plugin runs in a V8 isolate with nothing in it but the SDK,
so the same boundary is drawn by what the isolate is given, and the capabilities worth having come back as host
commands, where the permission system can see them. Gone, and staying gone:

- **`Deno.*` and Core's internals**: no process, filesystem, network or raw ops. Files go through `zellij.fs`,
  commands through `zellij.runCommand`, HTTP through `zellij.webRequest` - each confined and permissioned.
- **Remote, npm, JSR and `node:` imports**: only explicit relative imports of `.js`, `.mjs`, `.ts` and `.mts` files
  inside the plugin's own folder, plus `zellij:plugin` (a builtin may also import `zellij:workflow-view`). Nor is
  `https://…/plugin.ts` a plugin URL: download the file and load it with `file:`.
- **Workers**: `PostMessageTo` is refused before dispatch. Timers, the async helpers and work queues are the answer.
- **Top-level `await`**: module initialization must complete without a pending promise, and host commands and event
  waits are unavailable until `load`.
- **Resizable `ArrayBuffer`s and guest WebAssembly**: their allocations bypass the isolate's counted allocator. Fixed
  `ArrayBuffer` and `SharedArrayBuffer` remain.

What you do get: the whole of ECMAScript, `Date` and `JSON` included, and a `console` that writes to the plugin's
log. Each isolate has the [allocation limits](#numbers-and-allocation-limits) above.

## Porting a Lua plugin

Lua plugins and configuration have been removed. Follow [Migrate from Lua](LUA_REMOVAL.md) for file and configuration changes.
Rewrite plugins with `export default`, explicit imports, and the SDK documented here.
Use `async` functions and `await` for tasks. Timer arguments use milliseconds, and UI selections start at zero.

## Authoring loop

Names without an extension select TypeScript. Use an explicit extension to select another supported source format:

```sh
zellij plugins write hello.ts < ./hello.ts
zellij plugins check hello.ts
zellij plugins test hello.ts
zellij plugins reload hello.ts
zellij plugins log hello.ts
```

Writes validate imports, format the proposed file with Oxfmt, then check strict TypeScript and Oxlint rules.
A failed check leaves the source, its backup, and permission grants unchanged.
Create dependencies before writing a new file that imports them. Only the requested file is formatted.
`zellij plugins write --no-check` is an explicit CLI escape hatch. Hocus has no equivalent.
Use `--overwrite` to replace a file. The previous version is preserved as `hello.bak.ts`.
The existing directory and symlink protections also apply to these filenames.
An extensionless authoring name selects `.ts`.

The SDK does not include npm, JSR, general Deno I/O, or workers.
Hocus supports the same names through `plugin_write`, `plugin_edit`, `plugin_list`, `plugin_read`,
`plugin_search`, `plugin_outline`, `plugin_check`, `plugin_format`, `plugin_test`, `plugin_reload`, and `plugin_log`.
`plugin_docs` serves this guide a `##` section at a time (`section=lifecycle`, `commands`, `helpers`, `async`, `files`, `data`, and the rest the index lists), the SDK declarations in parts (`section=sdk_core` onward), and the worked examples. Writes and edits format and check the result before approval.
Use `plugin_check` with a name to check a saved file, or with `source` to check a draft without writing.
Use `plugin_format` with `source` to return formatted draft text. Without `source`, it confirms and formats the saved file with a backup.

```sh
zellij plugins format hello.ts
zellij plugins format --file ./hello.ts --check
```

`--check` verifies formatting without writing. Format and check do not execute plugin code.
Fix the reported diagnostics, check again, then use `plugin_reload` and `plugin_log` to verify runtime behavior.
Existing plugins can still load without authoring checks.

`CheckPlugin` returns `{diagnostics, outline, permissions, warnings, formatted_source?}`.
`Syntax` provides lightweight analysis and needs no file permission.
`Semantic` checks the dependency graph and strict lint rules.
`Format` returns formatted text. `Prepare` formats and performs all checks before returning the proposed source.
These three modes require `ReadFiles`.

Hocus infers grants only from literal SDK requests in the proposed entrypoint, using parsed JS/TS bindings.
Imported default/named aliases and SDK namespace imports are recognized; comments and shadowed names grant nothing.
Computed requests use normal runtime approval and produce warnings. Imported modules do not add inferred grants.
The approval shows inferred grants; edits show newly requested grants. Grants are applied only after a successful write.


### Shared authoring policy

Compiler checks include strict mode, unchecked indexed access, exact optional properties, unused declarations,
implicit returns, switch fallthrough, implicit overrides, and index-signature access.
Oxlint enables correctness, suspicious, pedantic, restriction, and performance categories for its core,
TypeScript, Oxc, import, and Unicorn plugins. Framework plugins and nursery rules are excluded.
The shared preset permits SDK lifecycle exports, console logging, explicit import extensions, async/await,
optional chaining, object spread, and nullish guards. It uses ES2022-compatible array sorting.
The file-length limit is 10,000 code lines so embedded builtins can remain self-contained.
The exact shared options are in the [lint preset](../zellij-plugin-host/src/deno/authoring/oxlint.json).

Explicit `any`, non-null assertions, unsafe assignments/calls/returns/member access, unsafe assertions,
unhandled or misused promises, and incomplete switches are errors.
Use `await` or a rejection handler for SDK promises. A `void` prefix does not handle rejection.
Lint suppressions must name a rule and explain the exception, for example:
`// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper updates shared state.`
For a TypeScript exception, use `// @ts-expect-error -- reason` or `/// @ts-expect-error -- reason` with a nonempty explanation.
This suppresses TypeScript errors on the following line. It does not suppress lint findings or errors on other lines.
Semantic checks and checked writes reject an unused `@ts-expect-error`. Syntax analysis and format-only operations do not check whether it is used.
These rules also apply to imported files. `@ts-ignore`, file-wide `@ts-nocheck`, block-comment suppressions, and blanket, unexplained, or unused lint suppressions are rejected.
`@ts-check` is permitted.
Plugins cannot replace the preset or load configuration code.

Formatting uses two spaces, double quotes, semicolons, trailing commas, LF endings, and a 100-column width.
Import sorting and semantic autofixes are disabled during writes.
`cargo x lint` checks shipped plugins and reference examples with this policy.
`cargo x format --check` verifies their formatting. Vendor compiler assets, invalid fixtures, and Swamp extensions use their own policies.


## Plugin tests

The installed binary runs TypeScript tests with its embedded runtime, checker, and SDK.
You do not need a Rust checkout, Node, or Deno. Tests run only when you request them.
Test before you reload a changed plugin.

```sh
zellij plugins test hello
zellij plugins test --file ./plugins --kind unit --filter timer --json
zellij plugins test --file ./hello.integration.test.ts --timeout-ms 60000
zellij plugins test agent-exit
```

A plugin name selects adjacent `hello.test.ts` and `hello.integration.test.ts` files.
An explicit directory selects matching files recursively. Hidden directories and symlinks are excluded.
Local imports must stay inside the selected directory, or the selected file's parent directory.
Builtin names select the test sources packaged in the binary. `workflow-view` selects shared unit tests;
its integration coverage runs through `agent-exit` and `swamp-workflow`.

`--kind` accepts `unit`, `integration`, or `all` (the default). `--filter` matches a substring in the test name.
`--timeout-ms` sets the deadline for each discovery process and each case; the default is 30000.
A failed assertion, type error, crash, timeout, cancellation, or empty selection fails the command.
`--json` prints `{tests: [...]}`. Each result has `name`, `kind`, `status`, `duration_ms`, `error`, and `output`.
Errors include source locations. Diagnostic output is limited to 32 KiB per case.

### Unit cases

Import `test`, `assert`, `assertEquals`, `assertThrows`, and `assertRejects` from `zellij:testing`.
This module is available only in test workers and their checker. Register tests at module initialization.
Each case gets a fresh isolate; async callbacks are awaited.

```ts
import {test, assertEquals, assertRejects} from 'zellij:testing';
test('async result', async () => {
  assertEquals(await Promise.resolve({answer: 42}), {answer: 42});
  await assertRejects(async () => { throw Error('expected failure'); }, 'expected failure');
});
```

`loadPlugin('./hello.ts', {configuration, replies})` loads one plugin per case through the production
SDK lifecycle bridge. `replies` maps host command names to replies or synchronous functions of their arguments.
Unstubbed host commands fail. Subscription and SDK timer bookkeeping have inert default replies.
No host operation from a unit plugin runs against your session.

Use `await harness.event(event)` and `await harness.pipe(message)` to deliver input in order.
Use `await harness.advance(milliseconds)` to advance SDK timers and `Date.now()`.
`await harness.render(rows, columns)` returns screen text parsed by Zellij's terminal grid.
`harness.commands`, `.logs`, and `.renders` expose captured effects; `.stub(name, reply)` changes a reply.
For imported render functions, `renderText(raw, rows, columns)` decodes their UI output.

### Integration cases

Name these files `*.integration.test.ts`. Each case owns a real server process and a noninteractive IPC client.
The controller uses normal input routing, plugin loading, hooks, permission checks, and SQLite storage.

```ts
import {test, session, assert} from 'zellij:testing';
test('fallback closes', async () => {
  session.start();
  session.loadPlugin('zellij:agent-exit');
  await session.waitForText('restart agent');
  session.resize(30, 100);
  session.key('Esc');
  await session.waitFor('fallback closed', () => !session.screen.includes('restart agent'));
  assert(!session.screen.includes('restart agent'));
});
```

Use `session.start({hooks: {session_start: './hook.ts'}, backgroundPlugins: ['./service.ts']})`
for headless plugins. Trigger hooks with real session actions. `session.loadPlugin(path, {headless: true})`
loads another background plugin. Visible loads accept `{configuration: {key: 'value'}}`.
`session.text`, `.key`, `.mouse`, `.resize`, `.pipe`, and `.reload` drive the session.
`await session.rpc(plugin, name, payload, timeoutMs)` calls a real SDK RPC handler through an ordinary
file plugin. Declare `ReadCliPipes` and `MessageAndLaunchOtherPlugins` for this control. The helper has
its own authenticated identity; it cannot impersonate a builtin. RPC replies are limited to 32 KiB.
`session.action(kdl)` accepts local pane, tab, focus, resize, and mode actions.
Inspect `.screen`, `.panes`, `.events`, `.logs`, `.catalog()`, or `.data(plugin, entity)`.
Use `await session.waitFor(description, predicate, timeoutMs)` for an observable effect; use
`await session.waitForText(text, timeoutMs)` for rendered content. Waits default to 10 seconds and
remain subject to the case deadline. Failure output includes recent screen content, logs, and the pending wait.

Each case gets temporary home, configuration, data, cache, socket, and storage directories.
It does not inherit your hooks, background plugins, Postgres configuration, or active-session identity.
Declare plugin grants with `test('name', {grants: ['ReadApplicationState']}, async () => {...})`.
Grants have their normal host-access meaning. Temporary directories do not sandbox a granted external command
or network request. Use deterministic fixtures for external tools and services.
Before `session.start()`, `session.fixture(files, commands)` writes relative files under the test home
and returns its absolute path. Each `commands` entry installs a shell script at the front of the
session's `PATH`; executable fixtures require `RunCommands`. Files and scripts are limited to 1 MiB.
Use this to replace Swamp or agent commands at the external-process boundary.
The runner shuts down and reaps its server after success, failure, timeout, or cancellation.

### SDK and Hocus

`await zellij.testPlugin('hello', {kind: 'all', filter: 'timer', timeout_ms: 30000})` returns the same report.
The returned promise has `.cancel()`. The `TestPlugin` command starts the run asynchronously and returns its
result in a correlated `CustomMessage`; `CancelPluginTest` cancels a request. Both require `AuthorPlugins`.
API callers can grant only permissions they hold. Integration controls additionally require `RunActionsAsUser`.
The production plugin dispatcher remains active while the test subprocess runs.

In Hocus, call `plugin_test name=hello`, optionally with `kind`, `filter`, or `timeout_ms`.
Hocus returns the same per-case JSON and can cancel an active run. Read `plugin_docs section=testing`
for this guide and `plugin_docs section=sdk_testing` for the test declarations.

## Build inputs

The host links `deno_core` 0.405.0, `deno_ast` 0.53.3, and their pinned V8 dependency.
The first build downloads the V8 archive unless it is already cached.
`cargo x build` also acquires pinned standalone Oxlint, Oxfmt, and tsgolint binaries with verified hashes.
`cargo x install` installs them under `authoring-tools/<zellij-version>/<tool-versions>` in application data,
independently of Wasm builtins. Installed checks need no Node, npm, Deno CLI, network, or tools on PATH.
Missing or mismatched binaries fail authoring checks with a repair instruction.
See [authoring provenance](../zellij-plugin-host/src/deno/authoring/provenance.json) for versions, hashes, sources, and licenses.
TypeScript uses its bundled ES2022 declarations. tsgolint uses its own embedded ES2022 library for intrinsic type recognition.
See [compiler provenance](../zellij-plugin-host/src/deno/compiler/provenance.json) for the bundled compiler's version, source, and checksums.
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
