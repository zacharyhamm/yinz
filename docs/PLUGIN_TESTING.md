# Scheme plugin tests

Run adjacent tests with the installed Yinz binary:

```sh
yinz plugins test example
yinz plugins test --file ./example.scm
yinz plugins test --file ./plugins --kind unit
yinz plugins test session-palette --kind integration
yinz plugins test --file ./example.test.scm --filter "timer" --json
```

A plugin name resolves inside `user_plugin_dir`. Builtin names use their bundled
test sources. A file selects its adjacent suites. A directory discovers suites
recursively and skips hidden entries.

Use `example.test.scm` for unit tests and `example.integration.test.scm` for
integration tests. Tests can use relative `require` modules within the selected
root. Each case runs in a fresh worker process. Discovery also runs in a worker.

## Register cases and assertions

```scheme
(test "addition" (lambda ()
  (assert-equal (+ 2 3) 5)
  (assert (> 5 2) "five must exceed two")
  (assert-throws (lambda () (error "expected failure")) "expected failure")))
```

`test` takes a name, an optional options hash, and a procedure. Names must be
unique within a file. Register all cases during module initialization.

The options hash accepts `"grants"`, a list of permission names. These grants
apply to plugins in the disposable integration session:

```scheme
(test "command fixture" (hash "grants" '("RunCommands")) (lambda ()
  ;; Create fixtures, start a session, then exercise the plugin.
  (void)))
```

`assert` requires a true value. `assert-equal` compares Scheme values, including
lists and hashes. `assert-throws` requires an error and can check a message
substring. `assert-rejects` accepts a procedure that throws or returns a rejected
deferred, with the same optional message substring. It returns a deferred that
resolves to the error text. Return it from the test or chain it with `yinz-then`:

```scheme
(test "deferred failure" (lambda ()
  (assert-rejects
    (lambda () (yinz-then json-null (lambda (_) (error "expected failure"))))
    "expected failure")))
```

## Unit tests

Load one plugin per case. Supply replies for host operations that it uses:

```scheme
(test "key changes the rendered value" (lambda ()
  (test-load-plugin "./example.scm"
    (hash "configuration" (hash "label" "Counter")
          "replies" (hash "GetPluginIds" (hash "plugin_id" 7))))
  (test-event (hash "Key" (hash "bare_key" (hash "Char" "a")
                               "key_modifiers" '())))
  (assert (string-contains? (test-render 24 80) "Counter: 1"))))
```

The loaded plugin uses the real Steel interpreter and SDK. Host commands use
mock replies. Commands must match the production command schema. An unstubbed
operation fails the case, except routine subscriptions, timers, selection,
cursor control, and `Pick`, which default to a null reply.

| Helper | Behavior |
|---|---|
| `(test-load-plugin path [options])` | Load a relative Scheme file with optional `"configuration"` and `"replies"` hashes |
| `(test-stub name reply-or-procedure)` | Set or replace an operation's reply and clear its previous injected failure |
| `(test-stub-error name message)` | Make the operation raise an error |
| `(test-commands)` | Return captured host commands in order |
| `(test-event event)` | Deliver an event and process deferred callbacks |
| `(test-pipe message)` | Deliver a typed pipe message |
| `(test-palette message client-id)` | Deliver a raw command-palette request for the specified client |
| `(test-advance milliseconds)` | Advance the mock monotonic and wall clocks, then deliver a timer event |
| `(test-render [rows columns])` | Render through the terminal grid and return visible text; defaults to 24 by 80 |
| `(render-text text [rows columns])` | Render supplied terminal text through the same grid |
| `(test-renders)` | Return captured visible renders |
| `(test-logs)` | Return captured plugin and test logs |

Mock replies can also be procedures, either in the load options' `"replies"` hash
or in `test-stub`. A procedure receives the command's payload and returns its
reply. Commands without a payload supply `json-null`. The procedure can inspect
captured commands, update test state, or raise an error to simulate a host failure:

```scheme
(test-stub "GetPluginIds" (lambda (arguments)
  (assert-equal arguments json-null)
  (hash "plugin_id" 7)))
```

The test and mocked plugin share a clock that starts at zero.
`test-advance` takes nonnegative integer
milliseconds. It does not wait for real time. To resolve a permission request,
deliver `(hash "PermissionRequestResult" "Granted")` or `"Denied"` through
`test-event`. Requests retain their original client and execute in queue order.

Use the JSON wire shapes from the plugin SDK: string-keyed hashes for objects,
lists for arrays, and `json-null` for null. Unit replies can be partial fixtures,
but commands emitted by the plugin must remain valid.

## Integration tests

An integration case controls its own disposable Yinz session. It does not
attach to an existing session or use your normal configuration and data.

```scheme
(test "plugin appears in a real pane" (lambda ()
  (test-session "start" (hash "rows" 30 "columns" 100))
  (test-session "load" (hash "path" "./example.scm"))
  (test-wait-for-text "Counter: 0")
  (test-session "key" "a")
  (test-wait-for-text "Counter: 1")))
```

Use `test-wait-for-text` for rendered content. Use `test-wait-for` for other
observable conditions. Both poll the session and default to a ten-second wait.
These waits use real deadlines and do not advance the test's mock clock:

```scheme
(test-wait-for "two terminal panes"
  (lambda ()
    (= 2 (length (filter (lambda (pane) (not (hash-ref pane "is_plugin")))
                        (test-session "panes")))))
```

An optional final argument overrides the wait timeout in milliseconds. Wait for
content or state instead of a render count. Scheduling can change render counts.
A predicate can return a value or a deferred. The wait checks the resolved value,
propagates rejection, and applies its deadline while the deferred is pending.

`test-session` takes an operation name and an optional argument:

| Operation | Argument and result |
|---|---|
| `"fixture"` | `{"files": {...}, "commands": {...}}`; create files and executable shell stubs before startup, return the fixture home path |
| `"start"` | Optional `"rows"`, `"columns"`, `"hooks"`, `"backgroundPlugins"`, and `"autoGrantPermissions"` |
| `"load"` | `"path"`, optional string-valued `"configuration"`, `"headless"`, and `"autoGrantPermissions"` |
| `"reload"`, `"stop"`, `"status"` | Plugin `"path"` and optional `"configuration"`; return lifecycle results |
| `"key"` | A key string such as `"Enter"`, `"Esc"`, or `"Ctrl Alt p"` |
| `"text"` | Text to type |
| `"mouse"` | A serialized mouse event |
| `"resize"` | `"rows"` and `"columns"` |
| `"action"` | KDL action text |
| `"screen"` | Return the current visible terminal text |
| `"panes"` | Return the most recent pane list |
| `"events"` | Return captured session events |
| `"logs"` | Return bounded session logs |
| `"pipe"` | `"name"`, `"payload"`, and optional `"plugin"`; return pipe output |
| `"rpc"` | `"plugin"`, `"name"`, `"payload"`, and `"timeoutMs"`; return the correlated RPC result |
| `"catalog"` | Return the disposable session's data catalog |
| `"data"` | `"plugin"` and `"entity"`; read that plugin's private entity |
| `"agent"` | Serialized agent request; return a request ID |
| `"agent-poll"` | Request ID; return null while pending, then an `Ok` or `Err` result |
| `"report"` | `"pane"`, `"kind"`, and serialized agent `"hook"` |
| `"poll"` | Receive pending session messages |

`backgroundPlugins` accepts paths or hashes with `"path"` and
`"configuration"`. Hooks map hook names to plugin paths. Builtin URLs such as
`"yinz:tab-bar"` work in session operations. Local test targets must remain
inside the selected test root.

Executable fixtures, agent requests, and agent reports require the declared
`RunCommands` grant. RPC requires `ReadCliPipes` and
`MessageAndLaunchOtherPlugins`. Fixture paths must stay inside the test home.
Executable names must be simple names, without path separators.

## Failures, limits, and cleanup

The default worker deadline is 30 seconds per discovery operation or case.
Set `--timeout-ms` to an integer from 1 through 3600000. A timeout or cancellation
terminates and reaps the worker. Integration cleanup also shuts down the owned
server and its processes. A cleanup failure fails the case.

The worker confines plugin execution and denies direct filesystem, process,
and network authority. Integration effects use explicit session controls and
declared grants. When a plugin invokes the test API, the requested grants must
fit its existing permissions. Integration controls also require
`RunActionsAsUser` on that caller.

Discovery rejects symlinks and limits a selection to 1000 files and 1000 cases.
Command capture permits at most 10000 commands and 1 MiB. Fixture content is
limited to 1 MiB. Render dimensions range from 1 through 500 rows and 1 through
1000 columns.

A report contains each case's name, kind, status, duration, error, and output.
Failed integration cases include recent screen content and session diagnostics.
Captured output is bounded to 32 KiB and survives worker timeouts. An empty
selection fails with `no matching tests`. The CLI exits unsuccessfully if any
case fails.

For repository development, run the builtin harness regression tests with:

```sh
cargo x test -p yinz-plugin-host -E 'test(scheme_builtin_) | test(scheme_installed_)'
```
