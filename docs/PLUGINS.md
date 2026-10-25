# Plugin lifecycle and permissions

## Overlay plugin tabs

The overlay contains Agents by default. Set `overlay_tab_plugins`
in `config.kdl` to replace the list:

```kdl
overlay_tab_plugins {
    agents label="Agents"
    "file:/path/to/plugin.scm" label="Custom"
}
```

Entries follow the configured order. Each entry names a plugin alias or location.
Aliases provide plugin configuration and the working directory. An omitted label
uses the entry name. Duplicate resolved locations and invalid entries are configuration errors.
Use `overlay_tab_plugins {}` to disable grouping.

`LaunchPlugin` and `LaunchOrFocusPlugin` open the group and select the requested
plugin. These actions also work when sent by another plugin. Plugin commands such
as `OpenPluginPaneFloating` use the same launch handling. Unloaded tabs load when
selected and use the normal permission checks. No separate overlay command is required.
Explicit layout panes, explicit destination tabs, background loads, and pane
replacements keep their requested placement. Plugins outside the list launch normally.
A plugin that declares no pane runs headless wherever it is launched; see
[Panes are declared](#panes-are-declared).

The first launch merges explicit plugin configuration into the requested tab. Later
launches select tabs without changing the group's configuration. Caller cwd and
terminal metadata are captured when the group is created and retained for lazy tabs.

Reloading configuration leaves an existing group's descriptors and running plugins
unchanged. Hiding and revealing the group preserves it. Close the group and launch
it again, or restart the session, to apply a changed list.

## Steel Scheme

Scheme sources use `.scm`, `.ss`, or `.sld`. All builtin plugins use Steel.
Read [the SDK reference](PLUGIN_SDK.md) for values, host commands, asynchronous
work, workers, data, and terminal UI. Read [the test API](PLUGIN_TESTING.md) for
unit tests and disposable-session tests.

Save this source as `hello.scm`:

```scheme
(yinz-declare-pane)

(define (load configuration)
  (yinz-command (hash "SetSelectable" #f)))

(define (render rows columns)
  "Hello from Steel")
```

Open the plugin with `yinz plugin -- file:/absolute/path/hello.scm`.
Scheme plugins use the host command dispatcher and permission checks.
Use `yinz-command` with the serialized command name or a hash that maps the command name to its arguments.
Objects use string-keyed hashes. Ordinary arrays use lists; typed binary fields use
bytevectors. JSON null uses the symbol `'json-null`.

The optional callbacks are `load`, `update`, `render`, `pipe`, and `unload`.
`update` and `pipe` return a boolean that requests a render.
`render` receives the pane dimensions and returns text, or writes text with `display`.
Only a plugin that declares a pane is rendered.
The SDK supports Unicode regex search, common Scheme forms, immutable structs,
and bounded string builders. See the SDK reference for signatures and limits.
Use `yinz-plugin-location` and `yinz-plugin-resolve` for confined source identity.
Use `yinz-cell-pixel-size` and `CellPixelSizeChanged` for the acting client's reported geometry.
PNG input accepts 8 MiB and render output accepts 16 MiB, including printed and returned text.
Raw filesystem, process, and network APIs are unavailable. Access host resources through permission-checked commands.
Use `require` and `provide` for modules. Imports must name a relative `.scm`, `.ss`, or `.sld`
file inside the entrypoint directory. Repository plugins can import from their repository root.
Symlinks cannot escape that root. Builtins import only from the installed `plugins/builtins/` tree in the selected Yinz data directory.
`only-in` and `prefix-in` select and rename imports.

Named workers have separate state and process messages in order:

```scheme
(yinz-register-workers
  (hash "counter"
    (lambda (name payload)
      (yinz-post-plugin "result" payload))))

(define (load configuration)
  (yinz-command (hash "Subscribe" '("CustomMessage")))
  (yinz-post-worker "counter" "count" "one"))
```

The parent receives worker replies as `CustomMessage` events. Workers can receive host replies
through an optional `worker-update` callback. Reload cancels and joins the old workers before
the replacement starts.

Use `yinz-then` and `yinz-catch` to handle deferred results. `yinz-run-command`,
`yinz-wait-for-permissions`, `yinz-wait-for-event`, and `yinz-sleep` return deferred values.
Sleep durations use milliseconds. A callback can return a deferred value; its continuation
keeps the client that started the operation. A worker finishes that value before it starts
the next message. Reload cancels pending operations from the old instance.

Check Scheme source with `yinz plugins check ./hello.scm`. Checks compile the entrypoint and its
confined imports without running plugin callbacks or module initializers. Literal permission
requests appear in the report. Computed requests require a runtime permission answer.
The report prints the pane declaration: `always`, `unless KEY=VALUE`, or `none`.
It warns when an undeclared plugin defines `render` or sends a command that needs its own
pane, and when a declaration is not a top-level form of the entrypoint.
A successful check does not prove that every callback executes successfully.
Run unit tests and disposable-session tests to exercise callback behavior.
Use an explicit Scheme extension with `plugins write`. Checked writes keep a backup before
replacing a file. Formatting parses only the named file, without loading its imports.
It adjusts indentation and preserves comments, strings, and line breaks.

Start a Scheme REPL connected to a running Yinz session:

```sh
yinz repl
yinz repl --eval '(+ 20 22)'
printf '(define answer 42)\nanswer\n' | yinz repl
```

The interactive REPL keeps definitions between expressions and accepts multiline input.
Use `:quit` or Ctrl-D to exit. Interactive errors leave the REPL open.
With `--eval` or piped input, an evaluation error produces a nonzero exit status.
All input modes require a running session and support Yinz host commands.
Inside Yinz, the REPL selects the current session. Outside Yinz, it selects the sole running session.
If no sessions are running, or several are running outside Yinz, the command fails.
Use `--session NAME` to override automatic selection:

```sh
yinz --session work repl
yinz --session work repl --eval '(yinz-command "GetPluginIds")'
```

The REPL keeps its own Scheme definitions and uses the session's host command dispatcher.
Each REPL has a separate headless plugin instance. Exiting with `:quit` or Ctrl-D closes that instance.

Put unit tests in an adjacent `hello.test.scm` file:

```scheme
(test "renders a greeting"
  (lambda ()
    (test-load-plugin "./hello.scm")
    (assert (string-contains? (test-render) "Hello from Steel"))))
```

Run them with `yinz plugins test --file ./hello.test.scm`. Test files can import confined
Scheme modules. `test-load-plugin` creates a separate mocked plugin instance. Supply a hash
with `"configuration"` and `"replies"` as its optional second argument. Unstubbed host
operations fail, except basic subscription, timer, cursor, selection, and picker commands.
Use `test-stub` to set a reply, `test-event` or `test-pipe` to deliver input, and
`test-advance` to advance the plugin's frozen clock in milliseconds. `test-commands`,
`test-logs`, and `test-renders` return captured history.

Name real session tests `hello.integration.test.scm`. `test-session` sends an operation
and an optional argument to a disposable session. For example, call `(test-session "start")`,
then `(test-session "load" (hash "path" "./hello.scm"))`, then
`(test-wait-for-text "Hello from Steel")`. The load follows the plugin's pane declaration.
Declare required grants in the test's options:
`(test "name" (hash "grants" '("RunCommands")) (lambda () ...))`.
Each test has a fresh worker and a deadline. The runner closes its disposable session after
the test, including when the test fails.

## Panes are declared

A plugin gets a pane only when its entrypoint declares one with a literal top-level form:

```scheme
(yinz-declare-pane)
(yinz-declare-pane-unless "duplicate_focused" "true")
```

The second form declares a pane unless the load configuration maps the key to exactly that value.
Use one form, once. See the [SDK reference](PLUGIN_SDK.md#pane-declaration) for the rules.

A plugin without a declaration runs headless. It never has a pane record: it is absent
from `ListPanes`, `PaneUpdate`, the floating layer, and saved layouts. It never receives
`render`, `Resize`, or `PluginTick`. It still receives subscribed events, pipes, plugin
requests, permission prompts, and hooks, but not the events tied to a pane: `Key`, `Mouse`,
`PastedText`, and `Visible` never arrive, even when subscribed. `InterceptKeyPresses`
delivers keys as `InterceptedKeyPress` instead. Commands that need its own pane fail.

| Launch | Declared | Undeclared |
|---|---|---|
| `load_plugins`, `plugins start`, background `LoadNewPlugin`, headless hooks, pickers | Hidden pane record | No pane; events arrive by subscription |
| `LaunchPlugin`, `yinz plugin --`, `New*PluginPane`, foreground `LoadNewPlugin`, `start-or-reload-plugin` | Pane as requested | Headless; succeeds and prints `<location> runs headless: it declares no pane`, then `plugin_N` on the last line |
| `LaunchOrFocusPlugin` | Focus or launch | Reuses a running headless instance and sends it a private `launch_or_focus` pipe message; replaces one that failed or crashed; else starts headless |
| `yinz pipe --plugin`, keybind pipes, plugin messages to a missing destination | Floating, unfocused | Headless; the message is delivered |
| `ReplacePane` hooks, in-place opens | In place | Headless; the target pane stays, even with `--close-replaced-pane` |
| Layouts, new tabs, resurrection, pane tabs, multipane children | The layout slot | Slot removed, one notification per location, plugin unloaded |
| `close-pane --pane-id plugin_N`, `ClosePluginPane` | Close, then unload | Unload |
| `focus-pane-id plugin_N` | Focus | Error: the plugin runs headless |
| Other actions and plugin commands naming `plugin_N` (rename, pin, float, move, resize, scroll, stack, break to a tab, hide, show) | Act on the pane | Error: the plugin runs headless. A plugin command with no reply logs it |

A script that reads the ID a launch prints should take the last line: a headless launch
prints its note first.

A load failure before the declaration is known creates no pane. This covers unreadable
source, an unresolved alias, a failed clone, and an invalid declaration. A foreground
launch fails its command and notifies the acting client. A layout slot shows the error.
A multipane child's slot is removed instead, as for an undeclared child: the other child
stays in the container's place as its only pane, and the multipane ends.
A background start records it in runtime status and the log. A declared plugin shows later
failures in its pane. A headless plugin launched in the foreground notifies once per
load; one started in the background reports only through runtime status and the log.

Reload rereads the declaration. A plugin that drops it loses its pane quietly and keeps
running headless; a layout slot is removed and the plugin unloaded instead. A plugin that
gains a declaration gets a hidden record if it started in the background, otherwise an
unfocused floating pane. The original in-place placement is not replayed. A foreground
launch that failed before its declaration was known never placed anything, so a reload that
finds a declaration opens its pane, unfocused, where the launch asked for it; one that asked
to open in place of another pane opens floating instead.

## Manage a plugin

Use session commands to manage a background plugin directly:

```sh
yinz plugins start ./palette.scm
yinz plugins list --running --json
yinz plugins reload ./palette.scm
yinz plugins log ./palette.scm
yinz plugins stop plugin_10
```

Use `--session NAME` before `plugins` to select a session. These commands work without the Agent plugin.
Plain `plugins list` lists source files in the plugin directory.

`start` creates a background instance or returns the existing identity, including an instance that is loading or has failed.
`reload` replaces an existing runtime and rereads its source. It preserves the plugin ID, configuration, sharing, and pane placement.
A missing reload target fails with `not loaded; use plugins start`. `reload --headless` is a compatibility spelling for the same operation.
`stop` removes all client copies, registrations, subscriptions, pending requests, and any hidden pane record.

Lifecycle commands never reveal an existing pane or change focus. Background plugins that declare a pane retain hidden pane records for `ShowSelf`.
Undeclared plugins have no pane record.
The explicit `action start-or-reload-plugin` command retains its pane behavior for declared plugins.

## Select an identity

Targets accept managed names, configured aliases, paths, URLs, and IDs such as `plugin_10`.
An identity consists of the normalized location and its configuration. File URLs, relative paths, percent encoding, and symlinks resolve to the same location.

Without `--configuration-json`, selection uses a unique existing identity first, then a unique `load_plugins` declaration, then alias defaults or empty configuration.
The CLI includes declarations freshly read from disk. A new declaration can supply `shared true` before reconfiguration.
If more than one configuration matches, the command fails and lists the candidates. Select an exact configuration with:

```sh
yinz plugins reload ./palette.scm --configuration-json '{"shared":"true"}' --json
```

The JSON object uses string values. `{}` selects empty configuration explicitly.
A shared plugin has one session runtime. A per-client plugin has one runtime for each interactive client.
Lifecycle operations apply to the selected identity across the session.

## Read runtime status

Use `plugins list --running --json` to inspect locations, configurations, background placement, panes, sharing, client copies, pending permissions, and errors.
`pane` is `true`, `false` for a headless plugin, or `null` while the declaration is unknown, such as during a clone.
Plain output prints it as `pane: yes`, `pane: no (headless)`, or `pane: pending`.
States are `loading`, `awaiting_permission`, `running`, and `failed`. A stop response reports `stopped`.
`running` means the runtime is available. Asynchronous initialization can still be in progress.

Start and reload report scheduling without waiting for a permission decision. Inspect runtime status for later load failures.
A failed identity remains available for inspection and reload, including after its source disappears.

`plugins log TARGET` reports runtime status and labels existing log lines as historical output.
It distinguishes `not loaded` from a loaded plugin with no log lines. Without an available session, runtime status is unknown.

## Answer a permission request

All plugin permission requests use a host-owned modal above the current tab, including requests from headless plugins.
The modal does not reveal a plugin pane or change the underlying focus, geometry, or input mode.
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
Grants use the existing permission cache. Denial preserves earlier grants and rejects `yinz-wait-for-permissions`.
A plugin can request permission again. Reload is also a way to retry.

During initialization, chain protected operations with `yinz-then` after
`(yinz-wait-for-permissions permissions)`.
No plugin-side `HideSelf` call is needed.

## Apply background configuration

`load_plugins` is session-wide. Apply changes with the existing reconfiguration command:

```sh
yinz action reconfigure --kdl 'load_plugins { "file:/absolute/path/palette.scm" { shared true; }; }'
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

In an integration test, use `(test-session "load" (hash "path" "./palette.scm"
"headless" #t))` to create a background plugin. `"headless"` selects a background
start; whether the plugin has a pane follows its declaration. The `"reload"`, `"stop"`, and
`"status"` operations manage its identity. Wait for lifecycle state, logs, or
rendered content before asserting asynchronous behavior.

Tests declare grants in their options. Start a session with
`(test-session "start" (hash "autoGrantPermissions" #f))` to exercise the real
permission modal. A load can also set `"autoGrantPermissions"` to false.

## Inspect launch provenance

`(yinz-command (hash "GetPluginInfo" json-null))` returns `{pluginId, background, pane}` for the calling instance, including during `load()`.
`pane` is `true` or `false`, or `null` while a repository plugin is still resolving.
`(yinz-command (hash "GetPluginInfo" id))` returns the same information for another instance, or `null` if it no longer exists.
Inspection of another instance requires `ReadApplicationState`.

The background flag records how the host launched the plugin. Hide, show, reload, and client attachment do not change it.
Finder openers deliver to an existing background destination without a focus command.
Existing pane readers still receive focus, including suppressed readers. A missing reader opens as a
floating pane only if it declares one; otherwise it receives the message headless.
Explicit show and focus commands remain available to background plugins that declare a pane.

## Await a file or directory

`yinz-pick` requires `MessageAndLaunchOtherPlugins` and a connected acting client:

```scheme
(define result
  (yinz-pick (hash "kind" "directory")))
(yinz-then result (lambda (path) (unless (json-null? path) (yinz-log path))))
```

The deferred result resolves to a path, or `json-null` after dismissal, and rejects
on failure. The host closes the overlay and restores focus before delivering it.

Options use `"kind"` (`"file"` or `"directory"`), `"roots"`, `"extensions"`,
`"hidden"`, `"recursive"`, and `"timeoutMs"`. Roots default to the initiating
pane's working directory. Relative roots resolve there. Hidden entries are
excluded by default. Traversal is recursive and does not follow directory
symlinks. Directory results include the roots themselves. Extensions match
case-insensitively without leading dots or glob syntax. Empty roots and directory
requests with extensions are errors.

There is no default interaction deadline. `(yinz-cancel! result)` asks the host
to close the picker. The deferred result settles when the host acknowledges the
request; a selection that wins the race still resolves normally.
Missing fzf, listing failures, and invalid output produce errors. One client can
have one active picker; a concurrent request fails as busy. Disconnect, caller
teardown, and driver failure release the resources.

Each request uses a fresh builtin finder driver. The `file-finder` alias must
resolve to `yinz:file-finder`. Alias geometry and fzf settings apply. Request
listing fields override alias `listing_json` fields, followed by builtin defaults.
Arrays replace arrays. Structured requests ignore `file_command`. Results bypass
opener settings. Persistent configuration remains unchanged.

## Palette completion feedback

Palette actions and form submissions can return completion feedback, directly
or from a deferred callback:

```scheme
(hash "type" "complete" "notification" "Report saved"
      "view" (hash "type" "text" "title" "Report" "text" "Saved report contents"))
```

Omit `view` to close the palette. Return an ordinary view hash to navigate
without a notification. A callback that returns nothing closes the palette with
“Completed: <action label>”. Forms use their title. Incidental return values keep
the same closing behavior. Failed callbacks show a notification and retain the
inline error so the user can retry.

A `CommandPalette` close request closes immediately. Feedback waits for the callback to settle.
After close, the host ignores a returned view but still shows a completion message
or failure. Cancellation, stale replies, provider reload, and client disconnect
invalidate pending feedback. Notification text must be nonblank, contain no
terminal controls, and fit within the existing 256 KiB display-reply limit.
The host supplies plugin attribution. Notification policy can suppress or replace
messages, and the normal history, placement, and lifetime settings apply.

## Management palette and forms

The shared `yinz:session-palette`, `yinz:layout-palette`, and `yinz:plugin-palette`
background plugins supply management commands by default.
`load_plugins clear-defaults=true` removes them unless the block explicitly loads them.
The Session Manager, Layout Manager, and Plugin Manager windows have been removed.
Their default shortcuts (`Ctrl o`, then `w`, `l`, or `p`) now open the command palette.
Replace custom bindings that launch these managers with `ToggleCommandPalette`.
Remove their entries from custom plugin aliases and layouts.
The `welcome` layout now opens a normal terminal layout with access to the palette.

Use **Switch session…** for running or saved local sessions. Selecting a saved session resurrects it.
**Find tab…** and **Find pane…** search all running local sessions.
**New session…** accepts an optional name. **Configure…** adds layout selection and a directory field.
Kill, deletion, and disconnection commands require confirmation. Cancel has initial focus.
Use Page Up and Page Down to read all confirmation details.

Use **Layouts…** to search layouts, inspect metadata or diagnostics, open tabs,
apply a layout, or edit, rename, and delete custom layouts.
The apply form selects the target tabs and which existing panes to retain.
**Save session as layout…** captures all tabs or the current tab.
**Import layout…** accepts KDL and shows a preview before saving.

Use **Plugins…** to inspect each plugin's location, tab, and configuration.
You can reload or close a plugin from its actions or the corresponding root commands, and focus one that has a pane.
**Load plugin…** accepts a URL, alias, or file and supports foreground or background placement.
Foreground opens a pane only for plugins that declare one.
Use **Edit configuration…** to add, change, or remove configuration entries before loading.

SDK forms accept optional `actions`. Each action has `"id"`, `"label"`, and a `"run"` procedure taking `(values context)`.
The callback receives current field values keyed by field ID, like `"onSubmit"`.
Return a view to continue, or raise an error to keep the form open with an inline error.
Set `"confirmation"` to `#t` to put Cancel before Submit and show scrollable confirmation details.

All choice fields support typing to search their options. Left and Right select adjacent options.
A `directory` field accepts text and adds a **Browse…** action using the existing directory picker.
Browsing requires `MessageAndLaunchOtherPlugins`. The palette releases input while the picker is open.
A selected path updates the field. Cancellation preserves all field values.

Dynamic nested action sources refresh once per second while visible.
Refresh preserves the query and selected action ID. Navigation and dismissal invalidate pending refreshes.
Each client and opening has separate state. Sources must search the complete inventory before pagination.
Use an action view’s `initialQuery` to retain a search when a page link opens the next view.
Each reply still has a limit of 100 actions and 256 KiB.
The session provider reserves two action slots for page navigation.

Only report an operation as completed after a host acknowledgment.
For commands without replies, the session provider reports that it requested the operation.

### Disconnect target query

`(yinz-command "GetDisconnectTargets")` return the IDs of ordinary clients that `DisconnectOtherClients` will
remove, excluding the initiating client. Watchers remain connected. Requires
`ReadApplicationState`. Each synchronous query has a separate response channel
and a 100 ms timeout; send failures, timeout, and closed channels return errors.
Recheck the IDs before disconnecting and request confirmation again if they
changed. Revalidation and disconnection are separate operations, so connections
can still change between them.
