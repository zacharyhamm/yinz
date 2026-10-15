# Steel plugin SDK

Yinz loads `.scm`, `.ss`, and `.sld` entrypoints in a confined Steel interpreter.
The functions below are available without an import. Use `require` and `provide`
for your own modules inside the entrypoint directory or Git repository root.

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

## Lifecycle

| Callback | Arguments | Result |
|---|---|---|
| `load` | configuration hash with string values | ignored, or deferred initialization |
| `update` | event string or single-key hash | `#t` to render, `#f` otherwise, or deferred result |
| `pipe` | pipe message hash | same as `update` |
| `render` | row count, column count | text, output written with `display`, or deferred text |
| `unload` | none | ignored, or deferred cleanup |

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

`(yinz-pick options)` opens a file or directory picker and returns a deferred path,
or `json-null` if dismissed. Options are `"kind"` (`"file"` or `"directory"`),
`"roots"`, `"extensions"`, `"hidden"`, `"recursive"`, and `"timeoutMs"`.
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
| `(yinz-agent-control request)` | Send a host agent-control request and await its correlated response |
| `(yinz-agent-register-provider definition)` | Register a provider |
| `(yinz-agent-providers)` | Query provider definitions |
| `(yinz-agent-spawn options)` | Spawn an agent and return its pane ID |
| `(yinz-agent-spawn-details options)` | Spawn an agent and return the full host result |

Agent waits and prompts have no default deadline. Cancel their deferred result
to release listeners. Agent control uses the host protocol; sending text uses
terminal input.

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

Use the revision read by the value's author as `expect_rev` for read-modify-write
operations. A conflict rolls back the batch. Shared session-owned data must use
both the host ID and session name to distinguish owners.

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

`yinz-ui-markdown` creates a document. `yinz-ui-lines` renders it at a width.
Text helpers include `yinz-ui-wrap`, `yinz-ui-truncate`, `yinz-ui-truncate-ansi`,
`yinz-ui-pad`, `yinz-ui-pad-left`, `yinz-ui-center`, and `yinz-ui-fuzzy-filter`.

`(yinz-ui-image-png id bytes)`, `(yinz-ui-image-place id columns rows)`, and
`(yinz-ui-image-row id row columns)` produce Kitty image output. Check
`ModeUpdate`'s `capabilities.kitty_graphics` before using images.

## Command palette

`(yinz-palette-register id title actions)` registers a provider and returns an
unregister procedure. `actions` is a list or a procedure taking `(query context)`.
Each action is a hash with `"id"`, `"label"`, and `"run"`; optional fields are
`"description"` and `"aliases"`. Action callbacks take `(values context)`.
Providers, actions, and form submissions can return deferred results. Context
procedures include `"onCancel"` (register a callback and return its removal
procedure), `"close"`, `"suspend"` (boolean argument), and `"cancelled"` (query).
Use cancellation callbacks to release obsolete searches when the query changes.

Return a view hash with `"type"` equal to `"text"`, `"actions"`, or `"form"`.
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
