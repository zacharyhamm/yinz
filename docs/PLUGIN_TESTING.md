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

`(test-location)` returns the test file's canonical `file:` URL.
`(test-resolve "./example.scm")` returns a canonical URL for an existing Scheme
target relative to that file. Both work during discovery and in unit tests,
without starting a session. Resolution stays inside the selected source root
and rejects symlinks, special files, and unsupported extensions. Arguments use
URL syntax: encode literal `#`, `?`, and `%` characters in filenames.

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
| `(test-step-wall-clock milliseconds)` | Change only the mock wall clock by a signed integer number of milliseconds, without delivering an event |
| `(test-advance milliseconds)` | Advance the mock monotonic and wall clocks, then deliver a timer event |
| `(test-render [rows columns])` | Render through the terminal grid and return visible text; defaults to 24 by 80 |
| `(render-text text [rows columns])` | Render supplied terminal text through the same grid |
| `(test-renders)` | Return captured visible renders |
| `(test-logs)` | Return captured plugin and test logs |
| `(test-pane-declared?)` | Whether the loaded plugin declares a pane for its test configuration |

Mock replies can also be procedures, either in the load options' `"replies"` hash
or in `test-stub`. A procedure receives the command's payload and returns its
reply. Commands without a payload supply `json-null`. The procedure can inspect
captured commands, update test state, or raise an error to simulate a host failure:

```scheme
(test-stub "GetPluginIds" (lambda (arguments)
  (assert-equal arguments json-null)
  (hash "plugin_id" 7)))
```

The test and mocked plugin share monotonic and wall clocks that start at zero.
`test-advance` takes nonnegative integer milliseconds and delivers one Timer, even
if no timeout is due. It does not wait for real time or schedule pending timeouts.
`test-step-wall-clock` takes signed integer milliseconds. It changes only wall
time and does not deliver an event. To resolve a permission request,
deliver `(hash "PermissionRequestResult" "Granted")` or `"Denied"` through
`test-event`. Requests retain their original client and execute in queue order.

Loading checks the [pane declaration](PLUGIN_SDK.md#pane-declaration) as the host
does. A declaration that is not a literal top-level form fails the load. A plugin
that declares no pane fails commands that need its own pane, such as `HideSelf`
and `SetSelectable`, with the production error.

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

`"load"` launches the plugin like `LaunchPlugin`. It gets a pane only if it declares
one; otherwise it runs headless and its render text never appears. Wait for logs,
data, or pipe output instead.

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
| `"load"` | `"path"`, optional string-valued `"configuration"`, `"headless"`, and `"autoGrantPermissions"`; `"headless" #t` starts the plugin in the background |
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
inside the selected test root or the disposable home used for copied plugins.

`(test-copy-plugin "./example.scm" "example-copy")` copies the entrypoint and its
confined Scheme dependency graph into a fresh `example-copy` directory under
the disposable home. It returns an opaque entrypoint URL for `test-session`
operations or alias KDL. The URL uses a local repository descriptor to retain
the copied source root, including when `src/main.scm` imports `../shared.scm`.
No Git checkout is needed. Keep the URL intact when comparing plugin identities.
Original source text and relative import paths are retained; duplicate imports
are copied once. This integration-only helper rejects
overwrites, destination traversal, symlinks, and special files. The usual source
graph limits apply: 8 MiB per source, 64 MiB total, 1024 modules, and 64 import
levels. Runtime assets outside the Scheme graph require explicit fixtures.
Session operations accept local repository descriptors only for directories
strictly inside the disposable home, without a branch, tag, or commit selector.

The loaded plugin's `(yinz-plugin-location)` and `(yinz-plugin-resolve path)`
use its own source identity, including mocked plugins. They are available
before `load`; configurations cannot replace that identity.

Executable fixtures, agent requests, and agent reports require the declared
`RunCommands` grant. RPC requires `ReadCliPipes` and
`MessageAndLaunchOtherPlugins`. Fixture paths must stay inside the test home.
Executable names must be simple names, without path separators.

Fixture commands inherit `YINZ_SOCKET_DIR`, `YINZ_SESSION_NAME`, and the isolated
runtime directory for the disposable session. Invoke the same Yinz binary from
a fixture to connect to that session; no socket symlink or environment repair is
needed. The store coordinates concurrent initialization within one five-second
deadline. Wait for asynchronous data changes with `test-wait-for`; callers do
not need to retry database-open errors.

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

Timezone operations must be explicitly stubbed in installed Scheme mock suites:
`(test-stub "GetClientTimezone" "America/Chicago")` and
`(test-stub "GetLocalUtcOffset" -21600)`. Stub `GetPluginIds` with a `client_id`
when using `(yinz-client-utc-offset epoch-ms)` without an explicit client.
Use `json-null` stubs to exercise unavailable rules/offsets and send raw
`ClientTimezoneChanged` hashes with `client_id` and nullable `timezone` fields.
Unstubbed timezone queries fail like other host operations. Real session tests
should provide distinct client reports with an unrelated server zone and check
both winter and summer instants; avoid mutating process-global `TZ` during tests.

`test-web` delivers a request hash to a fixture's `yinz-web-dispatch` through
the same worker lifecycle as production. For example, `(test-web (hash "id" 17
"client" 1 "path" "/api/example"))`. Stub `HttpRespond` explicitly to inspect
captured commands. This harness does not establish native authorization or slot
ownership: host `web_transport_` tests exercise real command dispatch and teardown,
and the app `web_transport_` acceptance test uses a bearer-authenticated TCP
request, normal permission handling and native frozen page/asset responses.


## Web SDK coverage and fixtures

The public SDK is tested separately from the raw transport fixtures. Select
`html_tests`/`web_tests` in yinz-steel, `web_sdk_` in yinz-plugin-host, and
`web_sdk_authenticated_routes_pages_assets_and_failures` for the real listener.
Keep the existing raw transport/generation/native-AgentView tests selected too.

An installed test can use public helpers with explicit host stubs:

```scheme
(test "SDK route" (lambda ()
  (test-load-plugin "./plugin.scm" (hash "grants" '("RunCommands")))
  (test-stub "Subscribe" json-null)
  (test-stub "SetTimeout" json-null)
  (test-stub "HttpRespond" json-null)
  (test-web (hash "id" 17 "client" 1 "method" "HEAD"
    "path" "/api/hello" "segments" '("api" "hello")
    "deadline_ms" 9000000000000))
  (assert (hash-contains? (last (test-commands)) "HttpRespond"))))
```

Publication during load needs `HttpAssets`/`HttpPublish` replies in the load
options. Agent-view tests stub `AgentView`, read its correlation ID from the
recorded command and deliver CustomMessage `agent-view-response` with serialized
`{id,reply:{ok:false}}` (or error). Both false and null are successful values.
The installed SDK fixture exercises this path through the supervised worker.

Request-lifetime tests inject clocks, inspect operations, event handlers,
observer links, timers and correlations relative to their pre-request baselines,
and deliver late events/replies/progress. They cover owned chained event/plugin
RPC/agent-view sources, borrowed sources, nested continuations, concurrent actors,
queued notifications, cleanup failures, cancellation without acknowledgement,
unload, send failures and more than 4,096 cumulative deferred creations. The
one-response token is independent of the 1,024 recent-ID FIFO. Ordinary async,
panes, services, consumers, RPC, palette, trace and module callers remain selected.

Timer failure regressions assert both a single sanitized 500 and successful
Timer delivery through Runtime and the supervised worker. They cover chained and
fire-and-forget `yinz-after`/`yinz-every`, rejected callback results, registration
and cleanup failures, concurrent actors, late delivery and healthy follow-up
requests. A separate global timer failure must still escape; request cleanup
must never erase that failure. The real host fixture also holds a concurrent
reply slot and checks the same generation, availability and frozen page after
an isolated timer failure. Named virtual sources and confined imported helpers
assert the actual failing file and line in local logs, with no private messages
or paths in HTTP replies or rendered content. Diagnostic tests exercise UTF-8
clipping, the 64-record cap, the full log port fallback and an empty second drain.

Issue 81 implementation validation (Linux, 2026-11-10): 124 selected Steel tests,
4 final HTML budget tests, 30 Markdown crate tests (plus its doctest pass),
15 focused host/harness/transport tests, 5 native request/service tests, and the
three selected authenticated integration cases passed. The isolated startup
fixture, `cargo x format --check` and warning-denying Clippy for the five changed
crates and their test targets also passed. Full CI, unfiltered workspace tests,
rebase and merge are reserved for the controller.

### Builtin web app

The builtin `web` plugin's adjacent suite, `web.test.scm`, loads `web.scm` (or
the `testing-fixtures/web-custom.scm` wrapper fixture) with `RunCommands` and
explicit stubs for `HttpRespond`, `AgentView`, `HttpAssets` and `HttpPublish`.
It drives routes with `test-web` and answers agent views with correlated
`agent-view-response` messages, so no native agent code runs. It covers
installation, escaped pages and bootstrap, enrichment, strict query and body
validation, error mapping, `405` handling, wrappers and sanitized failures:

```sh
cargo x test -p yinz-plugin-host -E 'test(scheme_builtin_web_)'
yinz plugins test web
```

`migrated_builtins_load_through_the_real_host_dispatcher` loads the installed
builtin through the real host dispatcher and publishes its tree to a site handle.
`cargo x test -p yinz-steel -E 'test(/web/)'` covers the SDK side: definition-only
evaluation, once-per-runtime installation with retry after failure, and
duplicate JSON key detection. `cargo x test -p yinz-steel-builtins -E 'test(/web_tests::/)'`
checks the shipped static tree: whole-tree installation, the served-file
allowlist and limits, page and manifest references, the import graph, vendor
provenance and digests, the single server-HTML sink and CSP-safe templates.
`nested_static_assets_survive_exchange_without_unserved_records` in yinz-runtime
checks that nested `web/static` files are retained across installation exchange.

The frontend's DOM-free modules have Node tests with injected fetch, storage,
clocks and timers. They need no installed packages:

```sh
node --test yinz-steel-builtins/src/unit/web/{api,timeline,submission}.test.mjs
```

`web_builtin_production_http` runs the real listener, password and bearer
authentication, plugin host, worker and default builtin installed from the
checkout, with TestRunner's fake pane OS. `web_builtin_browser_phone_flows`
drives headless Firefox through Playwright against the real frozen pages and
native login, intercepting the agent-data API with fixed fixtures. Playwright is
a pinned, test-only package under `yinz-steel-builtins/src/unit/web/browser`;
its `node_modules/` is ignored, and neither it nor the browser is installed with
Yinz. The test is `#[ignore]`d, so the unfiltered integration suite and
`cargo x ci check` skip it; select it with `--run-ignored only`. Selected without
its tooling, it fails with the provisioning command instead of skipping.

The browser fixture mirrors the default agent-row and agent-header structures
and is paired with the real Scheme route tests for preview boundaries, Unicode, escaping and raw
field preservation. List checks cover compact previews, contrast, density,
wrapping, badges, actions and focus at 320px and 390px in both color schemes,
plus a desktop smoke and successful empty-list recovery. Detail checks use the
same header object to generate short/long escaped default markup, with additional
missing, wrapped and custom hook variants. At exact 320×640 and 390×844 CSS
viewports in both themes, the collapsed header (including tabs) must be ≤120px,
the idle empty-draft composer ≤64px, and the healthy timeline ≥450px/650px.
These budgets exclude actual safe-area insets and warning/submission panels.
The runner verifies media emulation again after native login/navigation.

Disclosure checks use click, Enter and Space, verify labels/ARIA, collapsed tab
order, scroll access, focus across confirmed polls and open state across tabs,
reload, retarget and fallback views. Timeline/Screen checks cover pinned and
scrolled reading positions across expansion, draft reflow and resizing; readers,
prepends and unchanged-poll scroll writes remain covered. Resolved/composited
text colors must reach 4.5:1 and focus outlines 3:1, including readers, sheets and
safety feedback. A 1024×768 detail smoke covers both themes and desktop widths.
Existing freshness, offline/retry, retarget, authentication, prompt persistence
and same-submission retry scenarios also run.

320×480 and 390×480 checks approximate reduced keyboard space: the timeline
retains positive height and composer controls and safety feedback remain
reachable in their scrolling regions. In both themes, Screen also combines
expanded long/custom information, the 80-line terminal, all seven default quick
keys and a four-line draft. Newest-line text ranges must fit inside the visible
output scrollport; Screen regions must stay above the composer. Every quick key,
Refresh and recovery control must fit inside its visible ancestor bounds and
pass a center hit test after scrolling only its intended region. Pointer and
keyboard key delivery, Screen warnings, offline/unknown-submission recovery,
same-id retry, reading positions and draft preservation are checked. Masked
reduced-height Screen screenshots and geometry/hit-test JSON are saved with the
other browser results. These are Firefox emulation checks, not
physical touch, virtual-keyboard opening, Tailscale, iOS Safari or safe-area
hardware coverage. Masked detail screenshots and geometry comparisons are saved
under `target/web-browser-results/`. For pre-change characterization only, set
`YINZ_WEB_HEADER_BASELINE=1` on the same browser command; it records geometry for
the short/long fixtures without asserting the final budgets. Characterization
is not acceptance verification, and baseline JSON is optional context for final
logs, never a substitute for deterministic assertions. For a diagnostic subset,
`YINZ_WEB_BROWSER_FILTER` selects scenario names; leave it unset for acceptance.

Provision both inside this worktree before running it:

```sh
npm ci --prefix yinz-steel-builtins/src/unit/web/browser --ignore-scripts --no-audit --no-fund
env PLAYWRIGHT_BROWSERS_PATH="$PWD/target/web-browser" \
  node yinz-steel-builtins/src/unit/web/browser/node_modules/playwright/cli.js install firefox
env PLAYWRIGHT_BROWSERS_PATH="$PWD/target/web-browser" \
  cargo x integration-test --run-ignored only -E 'test(web_builtin_browser_phone_flows)' -j 16
cargo x integration-test -E 'test(web_builtin_production_http)' -j 16
```

[WEB.md](WEB.md#testing-the-builtin-app) summarizes which parts each layer
mocks. Results are recorded in the implementation evidence. No layer uses a
phone, Tailscale Serve or a real agent.

### Browser interaction smoke

Firefox 159.0.1 ran headlessly with its built-in Marionette protocol and a
throwaway profile. The SDK integration fixture publishes a temporary `/login`
form and same-origin external script under the unchanged native CSP. It verified
escaped title/labels, `JSON.parse(textContent)` with closing-script text and both
Unicode separators, failed and successful JSON-fetch login against native
`/api/login`, accessible status feedback, and a broken-script submission blocked
by `form-action 'none'` without credential-bearing navigation. A visible and
noscript JavaScript-required explanation remains. The standard app is not shipped
by this fixture. No phone/Tailscale or general frontend validation is claimed.

The integration case optionally runs a supplied local Python browser driver via
`YINZ_WEB_BROWSER_SMOKE=/absolute/path/to/driver.py`; it receives the listener's
address as its only argument and must join/stop its browser before returning.
The implementation smoke used a temporary driver outside the repository; normal
automated tests do not require a browser installation. This is recorded browser
evidence, distinct from the TCP assertions.

### Startup measurements

Fixture: `steel::tests::web_sdk_startup_phase_timings`, run alone with
`cargo x test -p yinz-plugin-host -E 'test(web_sdk_startup_phase_timings)' --no-capture -j 1`.
Every batch loads/renders the identical minimal declared-pane plugin through
SteelPlugin: two warmups, then ten fresh worker processes. It asserts successful
render content and complete existing phase records; no wall-time threshold is an
automated assertion. All binaries/artifacts came from this worktree's real
`target/`, Cargo dev/test profiles (unoptimized, debuginfo), on Linux x86_64,
32 logical CPUs, AMD Ryzen AI MAX+ 395. No other Cargo run was observed when the
batches began, and this task ran them serially with no concurrent task-owned jobs.
The machine is shared: baseline numeric load was not captured, so host load is
not controlled and these timings must not be treated as a precise causal estimate.

Before: `d4661c01797a5becbd0c8c2de292a7d6053d2ed3` plus the measurement-only fixture,
prior to SDK evaluation changes. Final: the implementation source in the commit
containing this table (the lifecycle evidence supplies its full commit hash).
The source fingerprint below hashes sorted path, NUL, bytes, NUL for lib.rs,
html.rs, web.rs, async.scm, rpc.scm, agents.scm, web.scm and web_app.scm under
yinz-steel/src, yinz-tile-markdown/src/html.rs and the host timing fixture file.

Implementation fingerprint: `fcde9e33ae280410e1bf3bc3ae5ed6bb856ae5ce29ff698f3dd4f167bd21a420`.

Medians, milliseconds (phase durations are measured independently):

| Batch | Worker ready | Module eval | Load | First render | Total to first render |
| --- | ---: | ---: | ---: | ---: | ---: |
| before | 73.666325 | 0.297341 | 0.150504 | 0.099292 | 74.537642 |
| after | 86.911476 | 0.367783 | 0.206906 | 0.123198 | 87.973774 |
| after-repeat | 86.732793 | 0.314883 | 0.159565 | 0.103711 | 87.558405 |
| batched | 132.390421 | 0.405114 | 0.173473 | 0.082831 | 133.772677 |
| startup-final | 192.772046 | 0.672964 | 0.292086 | 0.169705 | 194.167063 |
| startup-repeat-final | 93.052412 | 0.380733 | 0.173507 | 0.094428 | 94.028238 |

The initial SDK batches exceeded both the 10% and 5 ms investigation threshold
(about 13 ms / 18% total growth); the repeat confirmed it. Combining all SDK
sources into one compilation pass worsened the median to about 134 ms and was
reverted. The final first batch varied substantially; its observed load averages
were 44.91 (one minute), versus 94.15 at the later repeat.
The unchanged final binary then returned to the lower timing range below.
The repeat invocation's process-tree CPU/wall accounting is recorded here:
`elapsed_s=1.758612 user_s=1.504956 system_s=0.259956`.
Residual SDK startup cost is material and is disclosed, not waived or hidden.

When these measurements were taken, the private standard-app source was
registered with `register_steel_module`, never evaluated for ordinary plugins,
and the callable API raised a not-installed diagnostic. This pinned Steel engine
permits `require` only at top level, so a callable could not lazily require it.
Issue 82 replaced that seam: every worker now evaluates `web_app.scm` as
definitions only, which sends no host commands, and `yinz-web-agent-app`
installs the app on an explicit call. The private module name and relative
imports of the source stay unresolvable for guests
(`web_sdk_private_app_source_does_not_expand_the_guest_import_boundary`). The
startup tables above predate this change and were not re-recorded here.

Raw before and final samples, all in milliseconds:

| Batch | Sample | Worker ready | Module eval | Load | First render | Total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| before | 0 | 76.059608 | 0.262054 | 0.130496 | 0.097504 | 76.739471 |
| before | 1 | 71.963482 | 0.275801 | 0.275560 | 0.128722 | 72.871846 |
| before | 2 | 77.853772 | 0.294525 | 0.140825 | 0.070783 | 78.585923 |
| before | 3 | 75.970410 | 0.339831 | 0.272374 | 0.173297 | 77.044767 |
| before | 4 | 72.573964 | 0.276551 | 0.133712 | 0.125447 | 73.321083 |
| before | 5 | 73.948968 | 0.416105 | 0.206780 | 0.086483 | 74.916201 |
| before | 6 | 76.750111 | 0.322057 | 0.168658 | 0.101080 | 77.621043 |
| before | 7 | 73.383681 | 0.302972 | 0.149432 | 0.093597 | 74.159083 |
| before | 8 | 71.574919 | 0.300156 | 0.151576 | 0.080111 | 72.365510 |
| before | 9 | 73.256752 | 0.273405 | 0.132971 | 0.125978 | 73.980637 |
| startup-final | 0 | 182.127306 | 1.199843 | 0.305707 | 0.183937 | 184.188072 |
| startup-final | 1 | 210.150137 | 0.644816 | 0.295808 | 0.156715 | 211.610611 |
| startup-final | 2 | 192.281386 | 0.687947 | 0.255993 | 0.142398 | 193.710912 |
| startup-final | 3 | 207.796539 | 0.567390 | 0.232598 | 0.128722 | 209.043962 |
| startup-final | 4 | 189.132217 | 0.743713 | 0.288364 | 0.247387 | 190.949204 |
| startup-final | 5 | 201.073609 | 0.704929 | 0.296479 | 0.178547 | 202.626508 |
| startup-final | 6 | 169.188924 | 0.617455 | 0.264078 | 0.155553 | 170.580508 |
| startup-final | 7 | 193.262707 | 0.569313 | 0.314644 | 0.177986 | 194.623214 |
| startup-final | 8 | 180.522559 | 0.761626 | 0.287362 | 0.163649 | 182.092390 |
| startup-final | 9 | 198.436956 | 0.657981 | 0.299886 | 0.175761 | 199.927757 |
| startup-repeat-final | 0 | 91.062329 | 0.428999 | 0.189277 | 0.082806 | 92.045754 |
| startup-repeat-final | 1 | 89.804325 | 0.382170 | 0.151767 | 0.073218 | 90.699514 |
| startup-repeat-final | 2 | 97.545706 | 0.382602 | 0.201921 | 0.110278 | 98.514884 |
| startup-repeat-final | 3 | 92.220342 | 0.379296 | 0.170792 | 0.085271 | 93.125911 |
| startup-repeat-final | 4 | 95.058375 | 0.353787 | 0.159871 | 0.074030 | 95.892809 |
| startup-repeat-final | 5 | 88.288748 | 0.390897 | 0.161715 | 0.135064 | 90.016145 |
| startup-repeat-final | 6 | 93.927692 | 0.358486 | 0.291770 | 0.135636 | 94.965881 |
| startup-repeat-final | 7 | 96.866876 | 0.361612 | 0.153710 | 0.073499 | 97.703424 |
| startup-repeat-final | 8 | 93.884481 | 0.439679 | 0.207101 | 0.115889 | 94.930564 |
| startup-repeat-final | 9 | 91.753833 | 0.379145 | 0.176222 | 0.103585 | 92.670751 |
