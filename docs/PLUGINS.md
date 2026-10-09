# Plugin lifecycle and permissions

Use session commands to manage a background plugin directly:

```sh
yinz plugins start ./palette.ts
yinz plugins list --running --json
yinz plugins reload ./palette.ts
yinz plugins log ./palette.ts
yinz plugins stop plugin_10
```

Use `--session NAME` before `plugins` to select a session. These commands work without the Agent plugin.
Plain `plugins list` lists source files in the plugin directory.

`start` creates a background instance or returns the existing identity, including an instance that is loading or has failed.
`reload` replaces an existing runtime and rereads its source. It preserves the plugin ID, configuration, sharing, and pane placement.
A missing reload target fails with `not loaded; use plugins start`. `reload --headless` is a compatibility spelling for the same operation.
`stop` removes all client copies, registrations, subscriptions, pending requests, and the hidden pane record.

Lifecycle commands never reveal an existing pane or change focus. Background plugins retain hidden pane records for `ShowSelf` compatibility.
The explicit `action start-or-reload-plugin` command retains its pane behavior.

## Select an identity

Targets accept managed names, configured aliases, paths, URLs, and IDs such as `plugin_10`.
An identity consists of the normalized location and its configuration. File URLs, relative paths, percent encoding, and symlinks resolve to the same location.

Without `--configuration-json`, selection uses a unique existing identity first, then a unique `load_plugins` declaration, then alias defaults or empty configuration.
The CLI includes declarations freshly read from disk. A new declaration can supply `shared true` before reconfiguration.
If more than one configuration matches, the command fails and lists the candidates. Select an exact configuration with:

```sh
yinz plugins reload ./palette.ts --configuration-json '{"shared":"true"}' --json
```

The JSON object uses string values. `{}` selects empty configuration explicitly.
A shared plugin has one session runtime. A per-client plugin has one runtime for each interactive client.
Lifecycle operations apply to the selected identity across the session.

## Read runtime status

Use `plugins list --running --json` to inspect locations, configurations, background placement, sharing, client copies, pending permissions, and errors.
States are `loading`, `awaiting_permission`, `running`, and `failed`. A stop response reports `stopped`.
`running` means the runtime is available. Asynchronous initialization can still be in progress.

Start and reload report scheduling without waiting for a permission decision. Inspect runtime status for later load failures.
A failed identity remains available for inspection and reload, including after its source disappears.

`plugins log TARGET` reports runtime status and labels existing log lines as historical output.
It distinguishes `not loaded` from a loaded plugin with no log lines. Without an available session, runtime status is unknown.

## Answer a permission request

All plugin permission requests use a host-owned modal above the current tab.
The modal does not reveal the plugin pane or change the underlying focus, geometry, or input mode.
It closes an open command palette or context menu. Other clients remain usable.

Deny is selected initially. Up selects Deny, and Down selects Allow, stopping at either end.
Tab and Shift+Tab cycle between the choices. Left and Right do not change the selection.
Selecting a choice scrolls it into view. Enter activates the selected choice only when it is visible.
The Enter hint appears only beside the selected choice. Escape always denies, even when Allow is selected.
Clicking a control also answers the request. Pasted text cannot approve a request.

Use PageUp, PageDown, Home, End, or the mouse wheel to scroll the dialog.

The host prefers the interactive client focused on the initiating CLI pane, then the acting client, then the last active interactive client.
Startup requests wait for an interactive client. If that client disconnects, the host reassigns the prompt. Watchers cannot answer.
Matching requests from client copies share one decision. Each client has a queue of prompts.

Reload and stop cancel obsolete requests. Late answers cannot grant permissions to replacement runtimes.
Grants use the existing permission cache. Denial preserves earlier grants and settles `waitForPermissions` with rejection.
A plugin can request permission again. Reload is also a way to retry.

For TypeScript initialization, await `yinz.waitForPermissions([...])` before calling APIs that need those permissions.
No plugin-side `HideSelf` call is needed.

## Apply background configuration

`load_plugins` is session-wide. Apply changes with the existing reconfiguration command:

```sh
yinz action reconfigure --kdl 'load_plugins { "file:/absolute/path/palette.ts" { shared true; }; }'
yinz action reconfigure --kdl 'load_plugins clear-defaults=true {}'
```

Omitting `load_plugins` preserves the current set. An ordinary block adds entries. `clear-defaults=true` replaces the set.
Added identities start. Unchanged identities keep running. Removed configuration-owned identities stop.
Removing an entry preserves an independently launched or host-required instance.
A different configuration creates a different identity.

An explicitly stopped entry stays stopped across identical reconfiguration.
Explicit start or removal and re-addition of the entry starts it again.
Use `--write` to persist through the managed configuration fragment. Parsing, resolution, and persistence failures return a nonzero status.
Scheduled changes report IDs. Runtime status reports asynchronous load failures.

Source edits do not trigger reload automatically. Run `plugins reload TARGET` after editing.

## Test a background plugin

In an integration test, `session.loadPlugin(path, {headless: true})` uses the session lifecycle request.
Use `session.reload(path)`, `session.stop(path)`, and `session.status(path)` to manage or inspect the identity.
These methods accept an exact `configuration` option. Reload, stop, and status also accept `plugin_N`.
Wait for lifecycle state, logs, or rendered content before asserting asynchronous behavior.

The harness grants the test's declared permissions by default. Use `session.start({autoGrantPermissions: false})` to exercise the real permission modal.
A single load can also opt out with `{headless: true, autoGrantPermissions: false}`.

## Inspect launch provenance

`z.getPluginInfo()` returns `{pluginId, background}` for the calling instance, including during `load()`.
`z.getPluginInfo(id)` returns the same information for another instance, or `null` if it no longer exists.
Inspection of another instance requires `ReadApplicationState`.

The background flag records how the host launched the plugin. Hide, show, reload, and client attachment do not change it.
Finder openers deliver to an existing background destination without a focus command.
Existing pane readers still receive focus, including suppressed readers. A missing reader opens as a floating pane.
Explicit show and focus commands remain available to background plugins.

## Await a file or directory

`z.pick(options)` requires `MessageAndLaunchOtherPlugins` and a connected acting client.
It returns a cancellable promise containing an absolute path, or `null` after Escape, no match, or closing the picker.
The host closes the picker overlay and restores focus before it delivers the result.

Use `kind: "file"` or `kind: "directory"`. Optional fields are `roots`, `extensions`, `hidden`, `recursive`, and `timeoutMs`.
Roots default to the initiating pane's working directory. Relative roots resolve against that directory.
Hidden entries are excluded by default. Traversal is recursive by default and does not follow directory symlinks.
Directory results include the roots themselves. Extensions match case-insensitively and contain no leading dots or glob syntax.
An empty roots array and directory requests with extensions are errors.

There is no default interaction deadline. `.cancel()` rejects with `AbortError`. An explicit deadline rejects with `TimeoutError`.
Invalid options, missing fzf, listing failures, and invalid output reject with an error.
One client can have one active picker. A concurrent request for that client rejects as busy.
Disconnect, caller teardown, and driver failure release picker resources.

Each request uses a fresh builtin finder driver. The `file-finder` alias must resolve to `yinz:file-finder`.
Alias geometry and fzf settings apply. Request listing fields override alias `listing_json` fields, followed by builtin defaults.
Arrays replace arrays. Structured invocations ignore `file_command`. Pick results bypass all opener settings.
Persistent configuration remains unchanged.

See [the folder-to-PDF example](../example-plugins/pick-pdf.ts) for a palette action that closes the palette and awaits two picks.
The example obtains HOME from the session environment and sends the selected path to the configured reader.

For SDK tests, use `harness.pickRequests`, `completePick(requestId, pathOrNull)`, and `failPick(requestId, message)`.
These helpers deliver the normal result event. Use `advance()` to exercise deadlines.
