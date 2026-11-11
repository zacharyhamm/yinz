# Steel plugin SDK

Yinz loads `.scm`, `.ss`, and `.sld` entrypoints in a confined Steel interpreter.
The functions below are available without an import. Use `require` and `provide`
for your own modules inside the entrypoint directory or Git repository root.

## Tab groups

`TabUpdate` continues to contain every tab in the session. Each tab's optional
`group` object contains the group's stable session-local `id` and `name`. Sort
groups by ID for display order and use the active tab's group to filter a tab bar.
Older hosts omit this field; treat those tabs as one Default group.

Use `RunAction` with `NewTabGroup`, `GoToPreviousTabGroup`, `GoToNextTabGroup`,
`SwitchTabGroup {id}`, `RenameTabGroup {id, name}`, or `CloseTabGroup {id}`.
These commands require the existing `RunActionsAsUser` permission. Direct close
actions close the contents immediately; the built-in palette adds confirmation.
`GoToTabInGroup {index}` uses a one-based index within the acting client's group.
Existing tab ID and positional commands keep their session-wide meaning.
`MovePaneToPreviousTabGroup` and `MovePaneToNextTabGroup` move the focused pane
to the adjacent group's last-used loaded tab. Focus follows the pane after a successful move.
These actions wrap around and skip groups without a loaded tab.

## Scheme language and text

The sandbox uses Steel's raw engine without its macro kernel or full standard library.
It includes `assoc` (`equal?`), `assq` (`eq?`), `cdddr`, `vector->list` with optional
start/end indices, `list-index`, and multi-list `map`. `map` stops at the shortest list.
`any` aliases `ormap`. `ormap` and `andmap` short-circuit and preserve predicate result values.

Available forms include `case`, `do`, `letrec`, quasiquote with unquote/splicing,
and `cond` clauses with `=>`. `do` evaluates steps before assigning the next iteration's values.
`values` and `call-with-values` use Steel's list convention for multiple values.
`let-values` evaluates producers before binding their results. `define-values` binds
results once. Both forms reject an incorrect number of results.

`(struct point (x y))` defines `point`, `point?`, `point-x`, and `point-y`.
Fields are immutable. Types with the same fields remain distinct.
Unsupported struct options produce an error. Struct reflection is unavailable.
String literals accept `\f` for form feed.

| Function | Behavior |
|---|---|
| `(char-upper-case? character)` | Unicode uppercase property |
| `(char-alphabetic? character)` | Unicode alphabetic property |
| `(char-foldcase character)` | Unicode simple case folding of one character |
| `(string-contains text needle [start])` | Character index of the first match, or `#f` |
| `(string-search-forward text needle [start])` | Alias of `string-contains` |
| `(string-index text character-or-predicate [start])` | First matching character index, or `#f` |
| `(string-split text [separator])` | Unicode whitespace splitting, or literal splitting with empty fields retained |
| `(string-trim text)` | Alias of `trim` |
| `(string-trim-left text)` | Alias of `trim-start` |
| `(string-trim-right text)` | Alias of `trim-end` |
| `(string-copy text [start [end]])` | Copy a character range with an exclusive end |
| `(string-pad-left text width [fill])` | Pad to a character count. Default fill is a space. Longer text remains intact |
| `(string-builder)` | Create an empty mutable builder |
| `(string-builder-append! builder text)` | Append within the 16 MiB builder limit |
| `(string-builder->string builder)` | Return the accumulated text |

Indices count Unicode scalar values, not UTF-8 bytes or grapheme clusters.
Start indices may equal the string length. Negative indices and indices past the end are errors.
An empty substring matches at the requested start. Literal splitting uses an exact
separator, including adjacent and trailing empty fields.

`(regex pattern [flags])` compiles Rust regex syntax. Flags are a string containing
`i` (Unicode case-insensitive), `m` (multiline), or `s` (dot matches newlines).
Invalid patterns and unsupported flags produce errors. Backreferences and lookaround are unavailable.

| Function | Result |
|---|---|
| `(regex? value)` | Whether the value is a compiled regex |
| `(regex-match? pattern text)` | Whether any match exists |
| `(regex-find pattern text)` | First match hash, or `#f` |
| `(regex-find-all pattern text)` | List of match hashes, including progressing empty matches |
| `(regex-replace-all pattern text replacement)` | Text with all matches replaced |
| `(regex-escape text)` | Escaped literal regex text |

Match hashes contain `"start"`, `"end"`, and `"groups"`.
The end index is exclusive. Group zero is the full match. Unmatched groups are `#f`.
Replacement strings accept `$1`, `${name}`, and `$$` for a literal dollar sign.
Patterns are limited to 64 KiB. Compiled regex size, input, aggregate match data,
and replacement output are bounded to 16 MiB. At most 100,000 matches are returned.

Unicode case-insensitive matching uses simple folding, so indices remain valid.
It matches `s` with `ſ`, `K` with `K`, and the sigma variants.
It does not equate `ß` with `ss` or remove accents.
For smart case that tests the exact uppercase letter category, use `\\p{Lu}`
in a Scheme regex string. The Unicode uppercase property and category `Lu` differ.

## Values and host commands

`(yinz-command command)` executes a permission-checked host command. A command
without arguments is a string. A command with arguments is a hash with one key:

```scheme
(yinz-command "GetPluginIds")
(yinz-command (hash "Subscribe" '("Key" "Timer")))
(yinz-command (hash "SetSelectable" #f))
(yinz-command (hash "OpenTerminal" (hash "path" "/work")))
(yinz-command (hash "SetTimeout" 1.0))
```

The bridge preserves the host's serialized command and event shapes. Use
string-keyed hashes for objects, lists for ordinary arrays and tuples, booleans for JSON
booleans, and `json-null` for null. `json-null?` tests for null. Integers retain
64-bit values without floating-point rounding. Enum variants without fields are
strings; variants with fields are hashes such as `(hash "Terminal" 7)`.

Synchronous queries return decoded values. Host errors raise Scheme errors.
Commands with later replies deliver events; register the waiter before sending
the command. `(yinz-request command event-name predicate)` does both and returns
a deferred result. The predicate receives the event payload.

See [command and event shapes](PLUGIN_PROTOCOL.md), also available through
`yinz plugins docs protocol`, for every command payload and synchronous reply.

## Resolve a directory's project

`(yinz-project-resolve directory)` returns a deferred reply containing a hash with
`"identity"`, `"name"`, and `"root"`, or `json-null` when the directory belongs to no
project. It requires `ReadApplicationState`. Relative directories use the plugin's
working directory; absolute paths refer to the local host filesystem. Missing or
invalid directories and host failures reject the deferred.

```scheme
(yinz-then (yinz-wait-for-permissions '("ReadApplicationState"))
  (lambda (_)
    (yinz-then (yinz-project-resolve "/work/project/.worktrees/topic")
      (lambda (project)
        (unless (json-null? project)
          (yinz-log (hash-ref project "identity")))))))
```

This uses the same resolver as project pane arrangement, including remembered and
discovered projects, subdirectories, linked worktrees and repositories inside a
remembered parent project. Use `"identity"` as your persisted project key, and
`"name"` for display. Registered identities are `project:<registered-root>` and
survive display renames and branch changes. Moving or merging roots can change
them. Unregistered checkouts use repository identities until registered; that
registration can change their key. `"root"` is the configuration root, which is
not sufficient to identify a multi-repository project. Custom providers use the
service's published matches; this query does not invoke a fresh provider scan.
See [the protocol contract](PLUGIN_PROTOCOL.md#directory-project-resolution).

The raw synchronous command is
`(yinz-command (hash "ResolveProject" (hash "directory" directory)))`.

## Per-project configuration

`ProjectConfigRead {directory}` reads the primary Git checkout's
`.yinz/agent-exit.kdl`. It returns a hash with `"root"`, `"source"`, `"config"`,
and `"error"`. Check `"error"` before using the configuration. Reading settings
does not execute configured commands or grant permissions.

```kdl
agent_exit { default_branch "main"; }
merge_queue {
    ci_command "cargo x ci check"
    review_prompt "Review this branch against main"
    merge "ff"
    push false
}
```

The `"config"` hash contains `"default_branch"` and `"sections"`. Each top-level
node other than `agent_exit` is available under its name in `"sections"`:

```scheme
(define snapshot
  (yinz-command (hash "ProjectConfigRead" (hash "directory" "/work/project"))))
(unless (json-null? (hash-ref snapshot "error"))
  (error (hash-ref snapshot "error")))
(define sections (hash-ref (hash-ref snapshot "config") "sections"))
(define queue (hash-ref sections "merge_queue"))
(define settings (hash-ref queue "children"))
```

Each `ConfigurationNode` has `"name"`, `"arguments"` (a list), `"properties"`
(a hash), and `"children"` (an ordered list of nodes). String, number, boolean,
and null values retain their types; null becomes `json-null`. Non-finite numbers
are rejected because the JSON transport cannot represent them. Repeated child
names are preserved. As with the configuration editor's node model, comments,
formatting, and KDL type annotations are not included; `"source"` retains the
original text. Repeated property names use the last value.

Plugins own the meaning and validation of their sections. Yinz does not interpret
`ci_command`, `review_prompt`, `merge`, or `push`; any execution still needs the
normal command permissions. Duplicate top-level names are errors, including
duplicate `agent_exit` blocks. Inside `agent_exit`, only one `default_branch`
string is allowed. A sections-only file is valid. Missing or empty files, an
omitted `agent_exit`, and an empty `agent_exit` retain the existing `main` default.
No plugin sections means an empty `"sections"` hash.

`ProjectConfigWrite {directory, source, expected}` validates and replaces the
whole file, then returns the same snapshot shape. Preserve other plugins'
sections and use the previously read `"source"` as `expected` (`json-null` for
a missing file). Read and write require `ReadFiles` and `WriteFiles`, respectively;
access outside the plugin directory also requires `FullHdAccess`.
The historical filename remains unchanged for compatibility.

## Plugin identity and client geometry

`(yinz-plugin-location)` returns the immutable entrypoint location supplied by the host.
`(yinz-plugin-resolve relative-path)` resolves an existing Scheme target relative to the entrypoint without executing it.
Both are available before `load`, including in named workers and mocked plugins.
File locations are canonical file URLs. Git locations retain their repository and resolved revision.
Builtin locations resolve only to installed sources in `<data-dir>/plugins/builtins/`. Root escapes, symlinks, special files,
and unsupported extensions are rejected. These functions do not grant filesystem access.

`(yinz-key-name bare-key)` returns names such as `"PageDown"`, `"Esc"`, `"F1"`, or a Unicode character.
Pass the `"bare_key"` field of a key event. Malformed keys are rejected.

`(yinz-cell-pixel-size)` queries the acting client's cell geometry through `GetCellPixelSize`.
It returns a hash with `"width"` and `"height"`, or `json-null` when unknown.
Subscribe to `CellPixelSizeChanged` for the current geometry and subsequent changes.
Terminal reports, including reports after resizes, can change the value.
A resize alone retains the last measured cell size. Reconnecting clears previous measurements.
Own-client geometry does not require `ReadApplicationState`.
Shared plugins must keep geometry-dependent state per client and accept unknown geometry.

## Lifecycle

| Callback | Arguments | Result |
|---|---|---|
| `load` | configuration hash with string values | ignored, or deferred initialization |
| `update` | event string or single-key hash | `#t` to render, `#f` otherwise, or deferred result |
| `pipe` | pipe message hash | same as `update` |
| `render` | row count, column count | text, output written with `display`, or deferred text |
| `unload` | none | ignored, or deferred cleanup |

Only a plugin that declares a pane receives `render`, `Resize`, `PluginTick`, and
an initial render. See [Pane declaration](#pane-declaration).

Rendered text has a 16 MiB limit, including printed and returned text together.
Formatted evaluation results have a separate aggregate 16 MiB limit.
Diagnostic output retains its 4 MiB limit. Worker and application transport limits remain separate.

All callbacks are optional. Use `(yinz-request-render)` after a state change that
needs a new render. Outside `render`, output goes to the plugin log. `yinz-log`
always writes to the log, including during rendering. During `render`, `display`, `write`, `newline`, and `displayln`
write the pane's output.

Each client has at most one pending deferred render. During a resize or state
change, the last accepted frame stays visible until a current render completes.
Results for obsolete dimensions or state are discarded and another render is
requested.

During `render` and its deferred continuations, `ShowCursor` commands are held
with the frame. The last cursor command is sent through the permission-checked
bridge when the accepted text is returned, including cached frames. `json-null`
hides the cursor. A frame without a cursor command leaves the cursor unchanged.
Failed or obsolete renders discard their cursor commands. Outside rendering,
cursor commands execute immediately.

`unload` runs once during normal close, reload, or session shutdown. Existing
permissions remain available during cleanup. Cleanup has a two-second limit;
errors are logged and the host then tears down the instance. Pending operations
are cancelled. Cleanup is best effort after process interruption or a crash.
Workers do not run the parent plugin's unload hook.

Enable diagnostic tracing with `(yinz-trace)` or a `trace "true"` plugin
configuration value. `(yinz-trace #f)` disables it. Traces record lifecycle,
command names, acting clients, and asynchronous task outcomes. They omit command
arguments, replies, and error payloads and never enter rendered output.

## Pane declaration

A plugin has a pane only when its entrypoint declares one. Without a declaration
it runs headless: no pane record exists, and it never renders.

| Form | Pane |
|---|---|
| `(yinz-declare-pane)` | Always |
| `(yinz-declare-pane-unless key value)` | Unless the load configuration maps `key` to exactly `value` |

```scheme
(yinz-declare-pane-unless "duplicate_focused" "true")

(define (load configuration) ...)
```

Write the form literally at the top level of the entrypoint, not inside `begin`,
`when`, a function, a macro, or an imported module. The `-unless` arguments must be
string literals. Declare once. The host reads the declaration from the source
before it evaluates the plugin, then checks the evaluated module. A mismatch fails
the load with `(yinz-declare-pane) must appear literally at the top level of the
entry file`. A client that attaches to a running plugin gets an instance read
from the file as it is then; if its declaration changed since the plugin loaded,
that instance fails with `the entry file's pane declaration changed since the
plugin was loaded`, and a reload applies the change. A file that does not parse
declares nothing, and the load reports the parse error. `yinz plugins format`
formats a file whose declaration is malformed and prints the mistake as a
warning. The declaration is sealed before `load`; a later call raises an error.
Workers evaluate the module too, and their declaration is ignored.

A headless plugin still receives subscribed events, pipes, plugin requests,
permission results, and hooks. Events tied to its own pane never arrive, even when
subscribed: `Key`, `Mouse`, and `PastedText` go to a focused pane, and `Visible` to a
pane that is shown or hidden. Use `InterceptKeyPresses` to receive keys as
`InterceptedKeyPress`. It can open other panes and plugins. These commands need its
own pane and fail with `'HideSelf' needs the plugin's own pane, but … declares no
pane`:

- `HideSelf`, `ShowSelf`, `SetSelectable`, `ShowCursor`, and `SetSelfMouseSelectionSupport`.
- `OpenTerminalInPlaceOfPlugin`, `OpenFileInPlaceOfPlugin`, and `OpenCommandPaneInPlaceOfPlugin`.
- `RenamePluginPane`, `FocusPluginPane`, `HidePaneWithId`, `ShowPaneWithId`,
  `SetFloatingPanePinned`, `ChangeFloatingPanesCoordinates`, `ChangeOverlayCoordinates`,
  `BreakPanesToNewTab`, `BreakPanesToTabWithIndex`, `BreakPanesToTabWithId`,
  `SetPaneTitlePrefix`, `SetPaneSubtitle`, and `HighlightAndUnhighlightPanes`, when
  they target the plugin's own ID.

`CloseSelf`, `ClosePluginPane` with its own ID, `InterceptKeyPresses`, and
notifications work headless. `CloseSelf` unloads the plugin.

A `LaunchOrFocusPlugin` that finds a headless plugin running has nothing to focus. It
sends the plugin a private pipe message named `launch_or_focus` from the launching
client instead, so the plugin can bring back what it shows:

```scheme
(define (pipe message)
  (when (and (hash-ref message "is_private") (equal? (hash-ref message "name") "launch_or_focus"))
    (yinz-command (hash "FocusTerminalPane" (list my-terminal #t #f))))
  #f)
```

A headless plugin whose load failed or that crashed is not reused: the next
`LaunchOrFocusPlugin` unloads it and starts a new instance.
`GetPluginInfo` reports `pane`. `yinz plugins check` prints the declaration, warns
about `render` and own-pane commands in an undeclared plugin, and warns about nested
declarations.

## Permissions and asynchronous work

```scheme
(define (load configuration)
  (yinz-then
    (yinz-wait-for-permissions '("RunCommands"))
    (lambda (_)
      (yinz-then
        (yinz-run-command (hash "command" '("/usr/bin/printf" "hello")))
        (lambda (result)
          (yinz-log (yinz-decode-utf8 (hash-ref result "stdout"))))))))
```

`yinz-wait-for-permissions` rejects when permission is denied. Host commands
still enforce their own permissions; requesting a grant does not bypass them.

| Function | Behavior |
|---|---|
| `(yinz-then value callback)` | Run the callback after success; returns a chained deferred value |
| `(yinz-catch value callback)` | Handle failure; the callback receives a message string |
| `(yinz-cancel! deferred)` | Reject an unsettled result and release its waiter |
| `(yinz-deferred)` | Create an unsettled result |
| `(yinz-resolve! deferred value)` | Resolve it; deferred values are adopted |
| `(yinz-reject! deferred message)` | Reject it |
| `(yinz-deferred? value)` | Test whether the value is deferred |
| `(yinz-pending? value)` | Test whether it is unsettled |
| `(yinz-sleep milliseconds)` | Resolve after a monotonic timer |
| `(yinz-wait-for-event type [predicate])` | Resolve on the next matching payload |
| `(yinz-wait-for-pipe name [predicate])` | Resolve on the next matching pipe message |
| `(yinz-on-event type callback)` | Register a payload callback; returns an unsubscribe procedure |

A continuation keeps the acting client that started its operation. Reload and
client disconnection invalidate obsolete work. A callback can return a deferred
value; do not block the interpreter waiting for a host response.

`yinz-run-command` accepts `"command"` (argument list), `"env"` (string hash),
`"cwd"` (default `/host`), `"context"` (string hash), `"max-bytes"`, and an
`"on-output"` callback for streamed output.
Its result has `"exit-code"`, `"stdout"`, `"stderr"`, and `"context"`.
Output is a bytevector. The context key `"__yinz_async"` is reserved.
The host captures at most 8 MiB each for stdout and stderr. `"max-bytes"` can
reduce this limit. Truncated captures include a `\n[truncated]` marker, shortened
if the limit cannot hold it.
The host continues to drain both streams. `"on-output"` receives all output,
including bytes beyond the capture limit.

`(yinz-web-request options)` returns a deferred hash with `"status"`, `"headers"`,
`"body"` (bytevector), and `"context"`. Options are `"url"`, `"verb"` (`"Get"`,
`"Post"`, `"Put"`, `"Delete"`, or `"Patch"`), `"headers"`, `"body"` (text or bytes),
`"context"`, `"stream-id"`, and `"proxy"`. Cancelling a command or web request
releases its SDK listeners; it does not terminate the host operation.

`(yinz-after milliseconds callback)` and `(yinz-every milliseconds callback)`
return cancellable deferred handles. Repeated callbacks run sequentially,
including deferred work. Return `#t` to request a render.

## Plugin requests

`(yinz-rpc-handle name handler)` registers a handler taking `(payload context)`.
Return a JSON-compatible value or a deferred result. The context contains
`"callId"`, `"name"`, `"source"`, and a `"progress"` procedure.

```scheme
(yinz-rpc-handle "echo" (lambda (payload context) payload))
(define request
  (yinz-rpc-call (hash "pluginId" 7 "name" "echo"
                       "payload" (hash "text" "hello") "timeoutMs" 5000)))
```

Specify exactly one destination: `"pluginId"` or `"pluginUrl"`. URL requests can
include `"pluginConfig"`, a string hash. An optional `"onProgress"` callback
receives progress values. Replies correlate by request ID and plugin source;
the SDK releases local waiters on completion, timeout, or cancellation. Each
encoded RPC envelope has a 1 MiB limit. Cancellation does not stop the remote
handler. There is no default RPC timeout.

`(yinz-complete options)` asks the current session's agent service for an LLM
completion. Options are `"messages"`, `"system"`, `"model"`, `"tools"`,
`"toolChoice"`, `"temperature"`, `"maxTokens"`, and `"timeoutMs"` (default 60000).
The request allows 200 messages, 64 tools, and 64000 encoded bytes. The result
contains `"content"`, `"toolCalls"`, `"finishReason"`, `"usage"`, and `"model"`.
Tool arguments remain strings for the caller to validate before execution.

`(yinz-pick options)` opens the builtin Strider file or directory browser and returns a deferred path,
or `json-null` if dismissed. Options are `"kind"` (`"file"` or `"directory"`),
`"roots"`, `"extensions"`, `"hidden"`, and `"timeoutMs"`.
`roots`, when supplied, must contain exactly one starting directory; relative paths
resolve against the caller cwd. Browsing can leave that starting directory.
Multiple roots and the former `recursive` option are rejected. Search filters the
current directory. File extensions are case-insensitive suffixes without leading dots;
directories remain visible for navigation. Enter browses a highlighted directory or
selects a file; Ctrl+Enter selects the current directory in directory mode. Escape
dismisses the browser. Ctrl+E toggles hidden entries.
`(yinz-cancel! request)` asks the host to close the picker. The result stays pending
until the host replies, preserving a selection that wins the cancellation race.
There is no default deadline.

`(yinz-test-plugin name [options])` runs an installed plugin's tests and returns
their report. Cancellation sends a host cancellation request. See
[plugin testing](PLUGIN_TESTING.md) for options and report fields.

## Panes and agents

`(yinz-wait-for-pane-exit pane)` observes a terminal pane and returns a deferred
hash with `"paneId"`, `"exitCode"`, and `"closed"`. Pane arguments accept a terminal
ID or `(hash "Terminal" id)`. `(yinz-run-in-pane pane step)` reruns a command in
that pane and waits for its correlated exit. A step uses command fields such as
`"path"`, `"args"`, `"cwd"`, and `"target"`, plus optional `"stdin"` and
`"timeoutMs"`. These two helpers observe completion; cancellation only stops
observation.

`(yinz-open-plugin-in-place-of pane url [options])` loads the plugin at `url` in
place of `pane` and returns a deferred result with the new pane ID, for example
`(hash "Plugin" 12)`. The function requires `OpenTerminalsOrPlugins`. Its `pane`
argument accepts a terminal ID, `(hash "Terminal" id)`, or `(hash "Plugin" id)`.
The `"configuration"` option is a string hash passed to the new plugin, and
`"context"` is reserved. The covered pane stays suppressed behind the new pane.
It returns when the new pane closes, including through an explicit close by the user.
The deferred result rejects when `pane` is suppressed or missing. It also rejects
when an instance with the same URL and configuration exists. Nothing loads in
either case. A plugin that declares no pane runs headless instead: `pane` stays
in place, and the result is the plugin's ID.

```scheme
;; A background coordinator reopens a merge-queue job's UI over its agent pane.
(yinz-catch
  (yinz-open-plugin-in-place-of 7 "file:/plugins/merge-queue.scm" (hash "configuration" (hash "job" "42")))
  (lambda (message) (displayln message)))
```

Commands that open a pane in place of the calling plugin, or of another pane by
ID, lend that slot too when `close_replaced_pane` is `#f`. If the user presses
Ctrl-C on a held command that a hook opened with `OpenCommandPaneInPlaceOfPlugin`,
the hook returns, and the pane behind the hook stays suppressed. These commands
reject a target that is suppressed or missing. See
[pane slots](PLUGIN_PROTOCOL.md#pane-slots).

`(yinz-pane-sequence options)` runs a nonempty `"steps"` list in one reusable pane.
Options include `"name"`, `"floating"`, `"stopOnFailure"`, `"closeOnSuccess"`,
`"keepScrollback"`, `"killGraceMs"`, `"onCancel"` (`"keep"` or `"close"`),
`"onStep"`, and `"onDone"`. The returned hash contains a `"done"` deferred result,
`"paneId"` and `"status"` getter procedures, a `"push"` procedure to append a
step, and a `"cancel"` procedure. Call the handle's `"cancel"` procedure to stop
the command. Cancellation and step deadlines send SIGINT, then SIGKILL after
the grace period if the command remains active.

| Function | Behavior |
|---|---|
| `(yinz-agent-info pane)` | Cached agent metadata, or `json-null` |
| `(yinz-agent-all)` | Cached agents indexed by terminal ID strings |
| `(yinz-agent-on callback)` | Observe agent events; returns an unsubscribe procedure |
| `(yinz-agent-wait-for-state pane states [options])` | Wait for a matching state; `"after"` requires a preceding state |
| `(yinz-agent-wait-for-event pane [hooks])` | Wait for an agent event, optionally filtered by hook names |
| `(yinz-agent-wait-for-turn pane [options])` | Wait for Stop, StopFailure, or Interrupt; `"fallback"` can be `"idle"` |
| `(yinz-agent-send pane text [options])` | Send bracketed paste and Enter; `"enter"` can be false |
| `(yinz-agent-prompt pane text [options])` | Register a turn waiter, then send text |
| `(yinz-agent-restart pane options)` | Restart an exited, held agent in its own pane; returns a deferred host reply |
| `(yinz-agent-control request)` | Send a host agent-control request and await its correlated response |
| `(yinz-agent-register-provider definition)` | Register a provider |
| `(yinz-agent-providers)` | Query provider definitions |
| `(yinz-agent-spawn options)` | Spawn an agent and return its pane ID |
| `(yinz-agent-spawn-details options)` | Spawn an agent and return the full host result |

Agent waits and prompts have no default deadline. Cancel their deferred result
to release listeners. Agent control uses the host protocol; sending text uses
terminal input.

`yinz-agent-control` also accepts `(hash "CreateProject" (hash "name" name
"destination" path))` with `RunCommands` permission. It creates a new Git
repository on `main`, without a commit, and registers it. Relative destinations
resolve from the plugin's host folder; absolute destinations are host paths.
The parent must exist, and existing destinations or conflicting registrations
are rejected before creation. Success returns `"outcome": "ok"` and a `"project"`
object containing the registered `"name"` and canonical `"root"`. Failure messages
identify directories retained after partial creation. To launch afterwards, send
`Spawn` with that project name, its explicit root as `cwd`, and
`"allow_outside_worktree": true` in the spec.

`yinz-agent-restart` requires `RunCommands` and an explicit `"session"` option:
`"resume"` uses the pane's remembered session id, and `"fresh"` starts a new
conversation. Optional `"prompt"` is a string or `json-null`. `"context"` is a
string hash echoed in `CommandPaneExited` to this plugin. Original launch flags,
cwd and target come from the held command. The deferred rejects on refusal,
including a missing resume id or a provider that cannot resume with a prompt.
Its result has `"pane_id"` (for example `(hash "Terminal" 7)`), `"session_id"` (null for
fresh), and `"command"` (executable and argv). Acceptance does not wait for the
provider to start.

```scheme
(yinz-then
  (yinz-agent-restart 7 (hash "session" "resume" "prompt" "Resolve the rebase conflicts"))
  (lambda (reply) (yinz-command "CloseSelf")))
```

This works while pane 7 is suppressed behind the hook plugin. Closing the
plugin reveals the running agent. Its next exit fires `agent_exit` again.
If the hook plugin remains loaded, subscribe to `Hook` to receive each later exit:

```scheme
(define (load config)
  (yinz-on-event "Hook"
    (lambda (event)
      (when (equal? (hash-ref event "point") "agent_exit")
        (define payload (hash-ref event "payload"))
        ;; The pane ID and exit code are strings, as in the initial config.
        (yinz-log (hash-ref payload "agent_pane_id")))
      #f)))
```

`yinz-on-event` subscribes automatically. A raw `update` handler must subscribe
with `(yinz-command (hash "Subscribe" (list "Hook")))`.
The callback receives a hash with `"point"` and `"payload"`. The payload has the same
keys and string values that a new instance receives from this firing.
Optional keys, such as `"agent_exit_code"`, can be absent. Identical firings also arrive.
The existing instance keeps its initial configuration. Read the event payload for new values.
Delivery needs no extra permission and goes only to the bound hook instance.
Closing the instance lets the next firing open a new hook pane.

The hook configuration includes `"agent_session_id"` when known, and later
`PaneUpdate` observations expose `"last_agent_session_id"` even when `"agent"`
is null after `SessionEnd`. Fresh restart clears the old id until the new
conversation reports itself.
`RunCommandInPane` clears the previous session id even for an identical command.
Use `yinz-agent-restart` with `"session" "resume"` to retain the selected session.
A different provider cannot inherit the previous provider's id. Reusing the pane
for a non-agent command removes the remembered id and disarms `agent_exit`.

## Workers

`(yinz-register-workers (hash name callback ...))` declares named workers. Each
callback takes message name and string payload. Workers have separate persistent
state and process messages in order. A deferred callback completes before the
next message starts.

Use `(yinz-post-worker worker name payload)` to send a message and
`(yinz-post-plugin name payload)` to reply as a `CustomMessage` event.
`(yinz-worker-name)` returns the current worker name, or false in the parent.
`(yinz-register-worker-events callback)` sets the worker's host-event callback.
Reload cancels and joins the old workers before starting replacements.

## Binary data

File reads, pane output, command output, web response bodies, and plugin-data
`Blob` values use Steel bytevectors.
Use `(bytevector? value)`, `(bytes-length value)`, and `(bytes-ref value index)` to
inspect them. `(yinz-decode-utf8 value)` converts bytes to text with replacement
characters for invalid UTF-8. Bytevectors can be passed directly to file writes,
pane writes, and web request bodies; these commands also accept byte lists.

The worker accepts up to 64 MiB of binary data per event, including the combined
stdout and stderr of a command result. Buffered web responses retain their 64 MiB
limit, and file reads retain their 8 MiB limit. Use streamed output or bounded
reads for larger data.

## JSON, text, and time

| Function | Result |
|---|---|
| `(yinz-json-parse text)` | Parsed JSON; input is limited to 64 MiB |
| `(yinz-json-string value)` | JSON with recursively sorted object keys |
| `(yinz-decode-utf8 bytes)` | Text, replacing invalid sequences; bytevector input is limited to 64 MiB |
| `(yinz-text-width text)` | Terminal column width |
| `(yinz-text-truncate text columns)` | Text clipped by column width |
| `(yinz-text-sanitize text)` | Text without escape sequences and unsafe controls |
| `(yinz-now-ms)` | Monotonic milliseconds |
| `(yinz-wall-clock-ms)` | Unix epoch milliseconds |
| `(yinz-utc-date milliseconds)` | Hash of year, month, day, weekday, hour, minute, second |
| `(yinz-random-integer upper)` | Random integer from zero through `upper - 1` |
| `(yinz-instance-id)` | Unique ID for this plugin instance |
| `(yinz-server-binary)` | Host executable path, or false outside a server |
| `(yinz-server-session)` | Host session name, or false outside a server |
| `(yinz-server-incarnation)` | Host session incarnation, or false outside a server |

## Data

The following helpers preserve the ordinary host permissions and transaction
errors: `yinz-data-define`, `yinz-data-define-all`, `yinz-data-fetch`,
`yinz-data-count`, `yinz-data-apply`, `yinz-data-watch`, and `yinz-data-fetch-all`.
Each takes the namespace first, followed by the definition, request, or mutation
list. `yinz-data-watch` also takes the watch mode. `yinz-data-fetch-all` follows
cursors and rejects missing or nonadvancing cursors.

`(yinz-data-watch-callback namespace request callback [mode])` registers a watch
and calls `(callback rows event)` for each update. It returns `"watch-id"`,
`"initial"` rows, and a `"dispose"` procedure that unregisters the watch.

Data watches include commits from other local Yinz servers that use the same
`plugin-data.db`. The host checks for external commits every 250 ms while watches
exist. Query execution and event delivery can add latency. Plugins do not need
their own polling timer.

Each event contains the latest result of the watched query, with its original
filters, selection, ordering, and limit. Writes between refreshes can coalesce
into one event per watch. Intermediate states can be omitted, including changes
that revert before the next refresh. Unchanged results produce no event, and
local commits do not produce duplicate polling events. Events for each watch
follow refresh order, rather than a global transaction order across servers.

Use the revision read by the value's author as `expect_rev` for read-modify-write
operations. A conflict rolls back the batch. Shared session-owned data must use
both the host ID and session name to distinguish owners.

The following builders are pure. Pass their results to the existing permission-checked commands.

| Function | Result |
|---|---|
| `(yinz-data-eq attribute value)` | Equality predicate |
| `(yinz-data-put entity values [options])` | Put mutation. Values may be an attribute hash or a list of name/value pairs |
| `(yinz-data-attr name kind [options])` | Attribute definition. Kind accepts a symbol or string |
| `(yinz-data-query entity [options])` | Fetch request with the ordinary wire defaults |
| `(yinz-data-row columns-or-response values)` | Hash that pairs columns with decoded row values, including `rev` |

Put options include `"uid"` and `"expect_rev"`. Attribute options include `"indexed"`,
`"full_text"`, `"unique"`, `"required"`, and `"default"`. Query options use wire field names.
Plain strings, numbers, booleans, bytevectors, and `json-null` become tagged data values.
Explicit tags such as `(hash "Int" 7)` are preserved. Row decoding preserves binary bytes.
Column/value count mismatches and malformed builder options produce errors.
These helpers do not fetch a fresh revision before writing.

## Terminal UI

`yinz-ui-text`, `yinz-ui-ribbon`, `yinz-ui-table`, and `yinz-ui-nested-list` return
renderable text. Strings are plain text; `(yinz-ui-styled text options)` adds style
options. Optional coordinates come from
`(yinz-ui-coordinates x y width height [scroll])`.

Build forms with `yinz-ui-form` or `yinz-ui-dialog`. Fields come from
`yinz-ui-input`, `yinz-ui-checkbox`, `yinz-ui-radio`, `yinz-ui-select`,
`yinz-ui-combo`, and `yinz-ui-button`. Use `yinz-ui-handle-key`,
`yinz-ui-handle-mouse`, and `yinz-ui-paste!` to deliver input; read values with
`yinz-ui-value` or `yinz-ui-values`. `yinz-ui-render` returns text and issues a
`ShowCursor` command, using the frame timing described above. `yinz-ui-render-buffer`
returns text and cursor metadata without a host call.

Pass `(hash "wrap" #t "message" "…" "help" "…")` as the dialog options to
wrap title, message, status, and help in terminal display columns. Wrapped dialogs
prioritize the focused control in short viewports and shorten field labels to
leave space for editable values. The default remains single-line chrome.
`(yinz-ui-height dialog width)` measures the unscrolled wrapped height;
`(yinz-ui-height dialog)` retains the single-line height estimate.

Add folder/file browsing to any ordinary text field with
`(yinz-ui-path-field form field-id "directory")` (or `"file"`). Route events to
`(yinz-ui-path-handle-event path-field event)` before your normal form handler.
It returns `#f` for unrelated events, otherwise a form-style action with
`"kind"`, `"id"`, and `"shouldRender"`. `"fieldChanged"` means a selected path
was applied; `"error"` includes `"message"` for your status line.

While that field is focused, **Ctrl-f** opens the configured `file-finder` picker
with the matching file/directory filter. The current directory value (or a file
value's parent) supplies its search root; an empty value uses the picker's
default root. An optional fourth argument supplies a base directory for relative
roots and empty values, for example `(yinz-ui-path-field form "path" "directory" initial-cwd)`.
Absolute roots are preserved, and browsing leaves the draft unchanged until a path is selected.
Values beginning with `~/` and the value `~` expand using the session's `HOME`
before resolving the picker root. This requires `ReadSessionEnvironmentVariables`;
a missing or empty `HOME` or a denied lookup returns a field error without opening
the picker. Named-user forms such as `~someone` remain literal.
Cancellation preserves the draft and restores field focus. Picker
errors also preserve the draft. Keep routing `PickResult` events; the helper
subscribes when browsing starts. Subscribe to `Key`, `Mouse`, and `PastedText`
for ordinary editing. Call `(yinz-ui-path-cancel! path-field)` when leaving the
form and from `unload` to retire a pending picker. Include the shortcut in your
form's help text. As with `yinz-pick`, browsing uses the normal host permissions.

`(yinz-ui-text-input label [text])` creates a standalone input.
It uses the same key, mouse, paste, render, and cursor operations as forms.
`(yinz-ui-text-input-op input operation [arguments])` supports `"Text"`, `"SetText"`,
`"CursorPosition"`, `"SetCursorPosition"`, `"ReplaceText"`, `"History"`, `"SetHistory"`,
`"DrainText"`, `"Key"`, `"InsertText"`, `"Undo"`, and `"Redo"`.
Cursor positions count Unicode characters. `"ReplaceText"` takes a list of text and cursor position.
Rendering a buffer is pure. Cursor delivery from `yinz-ui-render` uses normal host permissions.

`yinz-ui-markdown` creates a document. `yinz-ui-lines` renders it at a width.
Text helpers include `yinz-ui-wrap`, `yinz-ui-truncate`, `yinz-ui-truncate-ansi`,
`yinz-ui-pad`, `yinz-ui-pad-left`, `yinz-ui-center`, and `yinz-ui-fuzzy-filter`.

`(yinz-ui-image-png id bytes)`, `(yinz-ui-image-place id columns rows)`, and
`(yinz-ui-image-row id row columns)` produce Kitty image output. Check
`ModeUpdate`'s `capabilities.kitty_graphics` before using images.
PNG input is limited to 8 MiB before encoding. Encoding uses the shared Kitty encoder.
Exported constants are `yinz-ui-image-max-id` (the shared encoder's ID limit),
`yinz-ui-image-max-dimension` (the placeholder dimension limit), and
`yinz-ui-image-max-bytes` (8 MiB). Widget, table, and coordinate limits are unchanged.
Large PNG, base64, transport, and decoded buffers can coexist within the worker's existing memory limit.

## Command palette

`(yinz-palette-register id title actions)` registers a provider and returns an
unregister procedure. `actions` is a list or a procedure taking `(query context)`.
Each action is a hash with `"id"`, `"label"`, and `"run"`; optional fields are
`"description"` and `"aliases"`. Action callbacks take `(values context)`.
Providers, actions, and form submissions can return deferred results. Context
procedures include `"onCancel"` (register a callback and return its removal
procedure), `"close"`, `"suspend"` (boolean argument), and `"cancelled"` (query).
Use cancellation callbacks to release obsolete searches when the query changes.

`GetSessionInventory` takes a request ID and requires `ReadApplicationState`.
Reuse the ID while the picker is open. A new ID starts discovery on the acting
client's machine. The reply contains `current_machine` and `entries`. Each entry
contains `destination`, nullable `name`, `status`, `current`, and `saved`.
Destinations are `"CurrentServer"`, `"Local"`, or `(hash "Remote" host)`.
Only current-server entries include saved sessions. A null name identifies a
host status row. `GetSessionList` keeps its server-local behavior.
When the acting client predates session requests, the reply arrives at once
with `current_server_only` set to `#t`, `current_machine` set to
`"CurrentServer"`, and only current-server entries. Switch to those entries with
`SwitchSession`; `SwitchSessionDestination` preparation fails for such a client
with a message asking the user to reattach with a current build, and
cancellation succeeds.

`SwitchSessionDestination` requires `ChangeApplicationState`. Send
`(hash "Prepare" (hash "id" id "target" (hash "destination" destination
"name" name "saved" #f)))`. Poll with `(hash "Status" id)` until the reply is
`"Ready"`. Send `(hash "Commit" id)` to hand off, or `(hash "Cancel" id)` to stop.
Cancellation also accepts an inventory ID. Preparation returns `"Pending"` and
keeps the source attached. Errors leave the source attached and release the
destination. Remote targets must be remembered hosts or the current connection.
Use `"saved" #t` only for a saved session on `"CurrentServer"`.

Return a view hash with `"type"` equal to `"text"`, `"actions"`, or `"form"`.
Action views with a `"source"` can supply an `"onClose"` procedure. The SDK calls
it with the original acting client when the view is released or dismissed.
Use it to cancel inventory work that belongs to the view.

Action and form views accept an optional `"description"` string. The palette
wraps this text below the title or search field. Omit it for no description.
Display text uses Markdown by default for native and plugin providers. This includes
result labels and descriptions, provider headings, view titles, form labels,
help and errors, submit labels, and secondary action labels. Text views and view
descriptions support block Markdown, including headings, lists, code blocks, and
tables. Result rows keep their two-line limit. Labels use a single logical line.
Links show their destination but do not open it.

Action views also accept optional ordered `"columns"`; actions accept optional
ordered string `"cells"`. For example:

```scheme
(hash "type" "actions" "title" "Issues"
  "columns" (list (hash "header" "Issue number" "width" 12)
                  (hash "header" "Type" "width" 10)
                  (hash "header" "Subject" "maxLines" 3)
                  (hash "header" "Submitter" "width" 14)
                  (hash "header" "Updated (UTC)" "width" 16))
  "actions" (list (hash "id" "42" "label" "Issue 42: Unicode rendering"
    "cells" '("42" "Feature" "A long subject with Unicode 雪" "Zoë" "2026-10-29 12:00")
    "run" (lambda (values context)
      (hash "type" "text" "title" "Issue 42" "text" "Opened issue 42")))))
```

Each column has a nonempty plain-text `"header"`, optional preferred `"width"`
(1–512 terminal cells), and `"maxLines"` (1–3, default 1). At most one column may
omit width; it fills remaining space. Preferred widths shrink in narrow terminals,
and columns may disappear at extreme widths. Up to 16 columns are supported.
Cells and headers are literal text, including Markdown punctuation. Cells wrap
up to maxLines, with an ellipsis on truncation; metadata with maxLines 1 truncates.
Permitted newlines and tabs in cells display as spaces. Headers stay visible
while scrolling whenever space permits a header and an action.

Every action with cells must supply exactly one string per column, including in
`"source"` refresh results. Omit cells for ordinary full-width actions in the same
view. Cells require columns on the containing action view; root provider results
and form actions cannot use them. Omit both fields for the existing appearance
and two-line label limit. Labels, descriptions, aliases, IDs and callbacks keep
their existing roles; search uses labels and aliases, so include searchable cell
content there as needed. A wrapped row remains one keyboard/mouse selection and
executes its original callback. Display data, including cells and headers, shares
the existing 256 KiB reply limit and terminal-control validation.

The display wire API uses the same `columns`/`cells` keys and column object shape
(`header`, optional `width`, `maxLines` defaulting to 1). The SDK replaces `run`
with the private `handle` as usual. Absent cells and empty columns are omitted on
wire serialization. See [the runnable generic table plugin](examples/palette-table.scm).

Search matches displayed labels and aliases. Editable values, choice values,
queries, IDs, and callback handles remain literal. Escape Markdown punctuation
with a backslash when you need literal display text. In Scheme strings, write
`"\\*literal\\*"` to display `*literal*`. Use `(yinz-markdown-escape value)` for dynamic names and paths before
inserting them into display text.

For example, an action can open a form and then show a Markdown text view:

```scheme
(yinz-palette-register "example" "**Example**"
  (list (hash "id" "write" "label" "**Write** a message"
    "description" "Enter a message in the `text` field."
    "run" (lambda (values context)
      (hash "type" "form" "title" "**Message**"
        "description" "## Instructions\n\nEnter the message below."
        "fields" (list (hash "type" "text" "id" "text" "label" "**Text**"))
        "submitLabel" "**Continue**"
        "onSubmit" (lambda (values context)
          (hash "type" "text" "title" "**Done**"
            "text" "# Message received\n\nThe form was submitted.")))))))
```

A form has `"title"`, `"fields"`, and an `"onSubmit"` callback. Set
`"confirmation"` to true for a confirmation form. Directory and file fields add
a picker action. Completion can return
`(hash "type" "complete" "notification" "Saved" "view" view)`.
Errors retain the current form and show its inline error. The SDK scopes callback
handles to the provider, opening, and acting client, and removes them on teardown.

## Default keybindings

`(yinz-register-default-keybindings bindings)` registers a list of hashes with
`"mode"`, `"key"`, and either `"actions"` or `"handler"`. Action entries are names
or hashes with `"name"`, `"args"`, `"props"`, and `"children"`. A handler receives
the keybinding pipe message and can return `#t` or a deferred result. Only messages
from a host keybinding invoke these handlers. Registering again replaces the
previous defaults and handlers.

## Local project identification providers

A shared Scheme plugin can identify projects without Git:

```scheme
(yinz-project-register-provider (hash "id" "marker" "priority" 100)
  (lambda (cwd)
    ;; Return json-null (or #f) for no match, or a deferred result.
    (hash "identity" "stable-work-id" "root" "/work/example" "name" "Example")))
```

The helper installs a `project-identify:<provider-id>` RPC handler before registration.
A plugin can register multiple IDs with separate callbacks; registering an ID again
replaces only that callback. The callback receives a local absolute working directory.
Return an object with `identity`, `root`, `name`, and optionally `branch`. The host checks that the root
is an existing absolute directory and canonically contains the requested directory.
Identity and name must be nonempty and have no control characters. Unknown fields
are rejected; a provider cannot set host identity or another provider's namespace.

Provider IDs use 1–32 lowercase letters, digits, underscores, or hyphens. IDs must
be unique; `git` is reserved. Higher integer priorities run first, with ties sorted
by ID. Custom providers precede Git. A location's complete provider chain has five
seconds; errors, invalid results, and timeouts are logged and fall through to the
next provider or Git. Registration belongs to the plugin generation and is removed
on unload or crash. Reload and registration changes invalidate cached local results.

Choose an identity that stays stable across renames and checkouts. Two roots that
return the same identity from the same provider share one project. Identical IDs
from different providers remain distinct. Do not use a branch as project identity.
Explicit project registrations and tab assignments remain overrides. Agent
checkouts can create saved projects; ordinary shells receive labels without
creating registry entries. Remote panes continue to use Git; SSH paths are never
sent to these providers.

Registration and reading providers (`yinz-project-providers`) require
`RunCommands`. RPC requires `MessageAndLaunchOtherPlugins`. Filesystem commands
and subprocesses require their usual permissions; registration grants no additional
authority. The host commands are `RegisterProjectProvider`, `GetProjectProviders`,
and the subscription event is `ProjectProvidersChanged`. Returned descriptors
include host-owned `plugin_id` and `generation` fields.

The runnable [marker provider](examples/marker-project.scm) requires Python 3.
Create `.yinz-project/identity` and `.yinz-project/name` text files in a project
root, then load the example as a shared background plugin:

```kdl
load_plugins {
    "file:/absolute/path/to/marker-project.scm" { shared true; }
}
```

For example, put `my-workspace` in `identity` and `My workspace` in `name`.
Copy those marker files to another checkout to group both roots together. The
example searches upward for the nearest marker directory and has no Git dependency.
It passes the host path as an argument and runs from `/`; command working directories
otherwise apply the SDK’s special `/tmp`, `/host`, `/cache`, and `/data` mappings.

## Session release updates

`(yinz-command "UpdateYinz")` starts a coordinated stable update of the acting
client's session. `(yinz-command "UpdateYinzPrerelease")` opts that operation into
the highest published SemVer release, including stable releases and prereleases.
Both require `RunCommands`. The session protocol commands are `Start` and
`StartPrerelease`, respectively. `UpdateYinzFromSource` / `StartSource` still
select the registered repository's local main build.

`YinzUpdateStatus` reports the channel, pinned version, and progress.
Preparation waits up to 60 seconds for pending session work to drain; failures
restore the session through the existing recovery process. Retrying an
interrupted update preserves its channel and selected version, including when a
different update action is used. Missing channel fields in older journals mean
stable. Participants without prerelease support fail with instructions to update
manually. This opt-in never changes the installation's future default channel.

## Client timezones

`(yinz-client-timezone [client-id])` returns the selected client's validated
reusable rule string, or `json-null` when unavailable. With no argument it queries
the acting client, including shared callbacks, deferred continuations and workers.
Reports may be IANA names (`"America/Chicago"`), POSIX rules, fixed offsets
(`"+05:30"`), or anonymous historical rules encoded as `tzif:` plus base64 TZif.

`(yinz-client-utc-offset epoch-ms [client-id])` explicitly selects that client and
returns seconds east of UTC at that epoch, including DST, or `json-null`. Raw
`GetLocalUtcOffset` also accepts a `"timezone"` selector for any supported report.
The `"client_id"` and `"timezone"` selectors are mutually exclusive. Unavailable
selected rules and out-of-range instants return null. Omitting both selectors in
the raw command retains its server-local behavior, as does `GetLocalTime`.
Timezone queries and `ClientTimezoneChanged` require no permission.

Subscribe to `ClientTimezoneChanged` for an attachment snapshot and live changes:
`(hash "ClientTimezoneChanged" (hash "client_id" 2 "timezone" "Asia/Kolkata"))`.
Null denotes an unknown zone or disconnect. Events describe every subject client,
including clients other than the instance owner. The subject does not become the
callback actor: pass its ID explicitly to query its offset. Cache presentation
per client, invalidate on events, and accept duplicate subscription snapshots.
Each new attachment publishes even if its report matches an earlier attachment.
Long-running and headless plugins receive the same subscription snapshots.

`ListClients` entries and palette callback contexts contain nullable `timezone`
fields. Listing requires `ReadApplicationState`; palette contexts reflect the
requesting client's current report at discovery, refresh and execution.

The native HTTP-to-Steel transport is available to the configured web plugin via
`yinz-web-dispatch` and the raw `HttpRespond`, `HttpAssets` and `HttpPublish`
commands. See [the protocol](PLUGIN_PROTOCOL.md#authenticated-http-transport)
for request hashes, permission gates, limits and generation ownership. Assets
are frozen at publication; edits require republication or reload. Allowlisted
static files below a relative `static` component are retained with builtin
Scheme generations across installation exchange. The safe HTML/routing SDK and deferred agent-view helper are documented below;
the standard mobile frontend remains separate.

## Safe HTML and web SDK

The embedded SDK is available in every worker. The configured web plugin needs
`RunCommands` for publication, replies and native agent views. Register routes
at initialization; call publication functions from `load` after permission is
granted. Helpers compose the existing host commands and cannot bypass host
identity, source confinement, generation ownership or permissions.

```scheme
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("RunCommands")) (lambda (_)
    (yinz-web-assets "/static/" "static")
    (yinz-web-page "/" "text/html; charset=utf-8"
      (lambda () (yinz-html-document
        (yinz-h "title" (hash) "Status")
        (yinz-h "main" (hash) "Ready")))))))
(yinz-web-route "GET" "/api/greeting/:name"
  (lambda (request) (yinz-web-json
    (hash "hello" (yinz-web-param request "name")))))
(yinz-web-route "GET" "/api/later"
  (lambda (_) (yinz-then (yinz-sleep 10)
    (lambda (_) (yinz-web-text "Ready")))))
```

### Markup and trust

| API | Contract |
| --- | --- |
| `(yinz-h tag attrs child ...)` | Immutable opaque native node; lowercase supported tag and string-keyed attribute hash. |
| `(yinz-html value ...)` | Render text, native nodes or list fragments to a string. Strings/numbers escape `& < > " '`. False and `json-null` disappear. Hashes, true, vectors and arbitrary custom values fail. |
| `(yinz-html-document head body [attrs])` | Trusted document with doctype, head/body and default `lang="en"`; optional attributes replace the default hash. |
| `(yinz-html-escape string)` | Escape text; the returned string remains ordinary text, so inserting it as a child escapes it again. |
| `(yinz-html-raw string)` | Explicit trusted markup, for plugin-authored constants only. Never pass agent output, hashes, request data or Markdown to this escape hatch. |
| `(yinz-html-json-script id value)` | Trusted inert `application/json` script. JSON escapes `< > &` and U+2028/U+2029, including closing-script text. Read with `JSON.parse(element.textContent)`. |
| `(yinz-markdown->html string)` | Trusted safe Markdown HTML; input at most 1 MiB. Raw HTML becomes escaped text, images become alt text, links retain only HTTP/HTTPS/mailto, relative paths and fragments with `rel="noopener noreferrer"`. |
| `(yinz-html-form attrs child ...)` | Fetch-driven semantic form with fixed `method="post"` and required root-relative action. Submission overrides are refused. |

Construction and emission enforce depth 64 (including fragments and reused
native nodes) and 4 MiB UTF-8 output. JSON input has an additional conservative
structural budget before copying. Raw HTML can bypass the element policy and
therefore belongs exclusively to trusted plugin authors.

Supported elements: `html head body title meta link script noscript div span p a
img br hr main header footer nav section article aside h1 h2 h3 h4 h5 h6 ul ol li
dl dt dd pre code kbd samp blockquote strong em b i s small sub sup mark table
caption thead tbody tfoot tr th td button input label select option textarea
fieldset legend details summary time progress meter`. `form` requires the
separate constructor. SVG, MathML, custom tags, namespaces, raw style, iframe,
object, embed and base are outside this safe subset.

All ordinary elements allow `id class title lang dir role tabindex hidden` and
lowercase `data-*`/`aria-*`. Additional attributes are:

| Element | Attributes |
| --- | --- |
| a | href, rel, target |
| img | src, alt, width, height, loading |
| link | href, rel, type; rel must be stylesheet, icon or manifest |
| meta | charset, name, content; never http-equiv |
| script | src, type only; nonempty same-origin root-relative src required; type absent, module or text/javascript; no children |
| form | action, method (post), autocomplete, novalidate |
| input | type, name, value, placeholder, autocomplete, disabled, checked, required, readonly, min, max, step, minlength, maxlength, multiple, autofocus |
| button | type, name, value, disabled |
| label | for |
| select | name, disabled, required, multiple |
| option | value, selected, disabled |
| textarea | name, rows, cols, placeholder, required, readonly, disabled, maxlength |
| th, td | colspan, rowspan, scope, headers |
| ol | start, reversed |
| time | datetime |
| details | open |
| progress, meter | value, min, max |
| fieldset | disabled |

Attribute strings/numbers are quoted and escaped. True is allowed only for
supported boolean attributes; false/null omit an attribute. `on*`, `style`,
`srcdoc`, `srcset`, `ping`, background URLs and submission overrides are refused.
Void elements refuse content. General `a` URLs accept root-relative paths,
fragments, HTTPS and mailto; resources/forms/scripts accept only root-relative
paths. Controls, whitespace, backslashes, authority-relative URLs, encoded
slash/control redirects and entity-looking scheme prefixes fail closed. Scheme
matching ignores case. This conservative API rejects ambiguous URLs instead of
trying to repair them. Markdown additionally allows HTTP and relative links.

The native CSP stays `form-action 'none'` with external same-origin scripts only.
A login form must include labels, an accessible status area and a visible or
`noscript` explanation that JavaScript is required. Its external script must
`preventDefault()` and POST JSON to `/api/login` with `fetch`; update the status
on both success and failure. No-JavaScript submission is blocked by CSP. Never
put passwords in a GET URL or bootstrap data. Inert JSON scripts are a dedicated
constructor; ordinary scripts cannot accept inline code or `application/json`.

### Routes, responses and request lifetime

`(yinz-web-route method pattern handler)` accepts GET, HEAD, POST or `*` and
returns an unregister thunk. Patterns contain literal segments, `:name` and a
final `*rest`. Names must be unique identifiers; rest is slash-joined and can be
empty. Requests already contain decoded segments: the SDK never decodes again.
Trailing slash variants match identically. Specificity compares the first
differing segment: literal > parameter > rest, then exact ending > rest. Method
rank follows path specificity: exact > `*`, or HEAD > GET fallback > `*` for
HEAD requests. A literal GET fallback thus beats a parameter HEAD route. The
handler still sees `method="HEAD"`; the native listener alone strips the body
and sets representation content-length. GET/POST never match HEAD-only routes.
Equivalent ties keep earliest registration; identical method/pattern replaces
in place, and an old unregister thunk cannot remove the replacement.

Handlers receive a fresh hash with `params`, preserving the complete native
request (auth, nullable client/body, query_pairs, server.session and deadline).
`yinz-web-param request name`, `yinz-web-query request key [default]` and
`yinz-web-header request name` return false if absent (or the query default).
Header lookup ignores name case. `yinz-web-body-json request` requires
application/json (parameters permitted) and valid nonempty JSON; valid null is
`json-null`. Missing/wrong media type or bad JSON yields typed 400 `bad_json`.

Responses are hashes with `status`, `headers` and string `body`. Constructors:
`yinz-web-json value [status]`, `yinz-web-html value [status]`,
`yinz-web-text string [status]`, `yinz-web-error status code [message]`,
`yinz-web-redirect root-relative-path` (303) and `yinz-web-not-modified` (304).
HTML responses escape ordinary strings; explicit trusted values insert markup.
Status is an integer 200–599; 304 has an empty body; nonempty bodies need
content-type; body limit is 4 MiB. Only content-type, etag, cache-control, vary,
retry-after, safe 3xx location and nonempty `x-yinz-*` headers are supported.
The native listener still enforces security headers and API no-store policy.

Sync/deferred handlers run through the existing async lifecycle. Unmatched
routes return 404. Raised/rejected or invalid responses return generic 500
`internal`; details stay in local logs. Each dispatch has a private one-response
token, with 64 active slots and 1,024 recent IDs. Old native reply ownership is
the final duplicate fence. Send failures are logged without a second reply.

A callback failure in a handler-created `yinz-after` or `yinz-every` (including
fire-and-forget timers and rejected deferred callback results) fails only its
request. Timer delivery succeeds, and other requests and published pages stay
available. Timers created outside a request keep their existing plugin-wide
failure behavior. Local web diagnostics retain the original failing source file
and line for synchronous and deferred exceptions, including imported helpers.
Plain-string rejections have no invented source location. Diagnostics keep at most
64 records, 2 KiB of message and the primary source span per record, and 16 KiB
per formatted record within the existing 4 MiB log budget. Optional call stacks
are omitted; truncation/dropped records are marked. Logging saturation does not
fail a handled request. These diagnostics never enter HTTP responses or frames.

Handler-created deferreds, chains, event listeners, waits, timers and RPCs belong
to a private request scope, including work created in resumed continuations.
Completion/deadline/unload closes that scope, detaches observations and retires
owned operations even when custom cancellation never acknowledges. Closed-scope
callbacks cannot run late. The native timeout owns 504: SDK deadline cleanup
sends no competing response. Pre-existing operations are borrowed, survive for
other consumers, and lose only the request's observers. Create long-lived work
outside handlers (normally in load). Local cleanup cannot roll back native
commands already sent or stop remote effects through a new cancellation protocol.
Outside request scopes, existing async cancellation and string-error semantics
remain unchanged.

### Agent views, pages and override hooks

`(yinz-agent-view request)` sends the existing 64 KiB native AgentView request
and returns a correlated deferred. A listener and 40-second monotonic timer are
installed before sending. Success resolves `reply.ok`, including false/null.
Errors reject a bounded versioned JSON string; `(yinz-agent-view-error string)`
decodes only this envelope to `{yinz_agent_view_error,code,message,op,origin}`
or false. The origin distinguishes native, timeout, cancel and SDK failures.
Malformed correlated replies reject; unrelated/duplicate/late replies do nothing.
All exits release listener/timer/correlation state. Host timers themselves
cannot be unarmed; later events are harmless.

```scheme
(yinz-web-route "GET" "/api/agents" (lambda (_)
  (yinz-catch
    (yinz-then (yinz-agent-view (hash "op" "list")) yinz-web-json)
    (lambda (failure)
      (define detail (yinz-agent-view-error failure))
      (yinz-web-error (yinz-web-agent-error->status detail)
        (if detail (hash-ref detail "code") "failed"))))))
```

`yinz-web-agent-error->status` accepts the decoded hash or encoded error:
invalid=400, not_found=404, target_changed=409, paused/unavailable=503,
failed/unknown=502. Native unavailable for focus=409; SDK timeout for focus=503.
An uncaught rejection still returns generic 500; explicit catches opt into
expected-error mapping without exposing arbitrary exception text.

`yinz-web-assets prefix directory` freezes plugin-relative or approved builtin
assets through HttpAssets (four prefixes). `yinz-web-page path mime thunk`
retains a render thunk and immediately publishes; replacement uses the same
slot (16 pages, 512 KiB each). Public canonical paths cannot claim `/api`.
HTML MIME `text/html; charset=utf-8` renders safe values; plain text
`text/plain; charset=utf-8` requires a string; `application/manifest+json`
serializes JSON. Native source/path ownership and freezing remain authoritative.

`yinz-web-hook-define name default`, `yinz-web-hook-wrap! name wrapper`,
`yinz-web-hook name arg ...` and `yinz-web-hook-names` provide sorted named
extension points. Latest wrapper is outermost and receives `(next . args)`;
call `(next replacement-arg ...)` to pass explicit arguments. Replacing a
default preserves wrappers. Defining/wrapping republishes every retained page;
all render results/sizes are checked before the first send. Recursive mutation
or publication fails. Publication is per page, so a later host failure can
leave generations mixed; that page keeps its previous native snapshot and the
failure remains observable. There is no multi-page transaction guarantee.

`yinz-web-agent-app configuration` is the future once-per-worker standard-app
installation seam. It currently raises an explicit not-installed diagnostic.
No standard routes, app content or frontend assets load automatically.
