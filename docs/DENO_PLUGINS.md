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
  async load(configuration) {
    await zellij.sleep(100);
    text = configuration.greeting ?? "Hello from TypeScript";
    zellij.requestRender();
  },
  render(rows, columns) {
    return `${text}\n${columns} columns, ${rows} rows`;
  },
});
```

Configuration values are strings. `update` receives an event, and `pipe` receives a pipe message.
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

The initial SDK does not include npm, JSR, general Deno I/O, Lua widgets, or agent helpers.
The hocus authoring tools and `CheckLuaPlugin` protocol retain their Lua interface.

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
