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

`(yinz-open-plugin-in-place-of pane url [options])` loads the plugin at `url` in
place of `pane` and returns a deferred result with the new pane ID, for example
`(hash "Plugin" 12)`. The function requires `OpenTerminalsOrPlugins`. Its `pane`
argument accepts a terminal ID, `(hash "Terminal" id)`, or `(hash "Plugin" id)`.
The `"configuration"` option is a string hash passed to the new plugin, and
`"context"` is reserved. The covered pane stays suppressed behind the new pane.
It returns when the new pane closes, including through an explicit close by the user.
The deferred result rejects when `pane` is suppressed or missing. It also rejects
when an instance with the same URL and configuration exists. Nothing loads in
either case.

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

Return a view hash with `"type"` equal to `"text"`, `"actions"`, or `"form"`.
Action and form views accept an optional `"description"` string. The palette
wraps this text below the title or search field. Omit it for no description.
Display text uses Markdown by default for native and plugin providers. This includes
result labels and descriptions, provider headings, view titles, form labels,
help and errors, submit labels, and secondary action labels. Text views and view
descriptions support block Markdown, including headings, lists, code blocks, and
tables. Result rows keep their two-line limit. Labels use a single logical line.
Links show their destination but do not open it.

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
