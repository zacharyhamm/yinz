# Lua plugins

Plugins come in two kinds: wasm modules (every builtin) and Lua scripts. The Lua runtime lives in
`zellij-plugin-host/src/lua/`; `docs/LUA_PLUGINS.md` is the user-facing guide.

The two runtimes share everything above `RunningPlugin`: event routing, subscriptions, rendering,
and — importantly — the whole command surface. `zellij_exports.rs` has one dispatcher,
`run_plugin_command(&mut PluginEnv, PluginCommand)`, and both runtimes call it, so a capability is
never implemented twice. What differs is confined to the `PluginBackend` enum and the entrypoint
methods on `RunningPlugin`.

Two things to know before changing this:

- **Ergonomics belong in Lua, not in Rust.** `prelude.lua` holds command wrappers and
  `ui.lua` holds shared-widget constructors and event normalization. `ui.rs` binds interpreter-local
  Rust widget state and pure rendering; it never calls shims or writes stdout. The only door
  to host authority is `__dispatch`, mirroring the single `host_run_plugin_command` wasm import. `zellij.on`,
  event normalization and config merging are all Lua. Widening the Rust boundary should be a last
  resort.
- **Lua plugins are not sandboxed by their runtime the way wasm plugins are**, so `lua/loader.rs`
  opens a restricted stdlib with no `io`/`os`/`dofile`/`loadlib`. Do not open those up: the
  permission system is the only thing standing between a plugin and the server's authority, and
  an unrestricted `io` would make it decorative. The two host-mediated exceptions are `require`
  of a file under the plugin's own folder (confined after symlinks, `require_sibling`) and
  `print`/`zellij.log`, which go to zellij's log tagged `[lua:<file>]`.
- **`to_lua_value` serializes `None` as mlua's `null` and then strips it to `nil`** rather than
  serializing `nil` directly: mlua builds a tuple variant's fields with `raw_push`, and pushing
  `nil` is a no-op, so `CommandPaneExited(7, None, ctx)` would otherwise arrive with the context
  in the exit code's slot. Keep that if you touch the serializer options.

`PluginCommand` derives `Deserialize`, and `lua/command.rs` is one `from_value` call, so the whole
command surface is reachable from Lua and no command needs a match arm. Two consequences worth
knowing: serde's variant names are now a second contract alongside the hand-written protobuf one
in `plugin_api/plugin_command.rs` (a test walks `CommandType` to catch drift), and a new command
needs nothing at all done to it to be callable from Lua.

Replies are the exception, and are still per-command: the response types are prost-generated with
no serde representation, so `lua/response.rs` decodes each one by hand into the `zellij_types::data`
type the wasm shim would build, and hands back raw bytes for anything it does not know. A test
lists every reply-returning command and fails when a new one is not decoded.
`zellij-tile/src/shim.rs` is the reference for what each response means.

The prelude also carries a scheduler. `zellij.task` is a coroutine; `zellij.await` yields an
*awaitable* - `{ done, value, events, match, on_settle }`, registered in `waiters[event name]`
at creation, eager - and `__update` runs `deliver(event)` **before** the `zellij.on` handlers:
every waiter for the event gets `match`, the settled ones' tasks are queued, `drain()` resumes
them FIFO, and the event then reaches the handlers and `update` untouched. A task or listener
that raises does not strand the rest of its batch: `each_guarded` runs them all and re-raises
the first error afterwards. `__pipe` does the same through `deliver_pipe`, which also serves
the `zellij.rpc` wire and hocus's extension messages without showing them to the plugin.
Timers are one host `SetTimeout` in flight for the nearest deadline in a sorted list, swept on
every `timer` event; the sweep arms the next deadline *before* settling the due ones, so a
raising callback cannot stop the clock, and `deliver` runs its stages - waiters, sweep, drain,
followers, drain - through `each_guarded` too, so a raising callback cannot leave a task that the
same tick made ready sitting undrained. `os.time` remains the scheduler clock, so deadlines are whole
seconds, and no extra second is added when arming. `zellij.monotonic_ms()` measures elapsed
time at finer resolution without changing scheduler deadlines. `coroutine.resume` is the prelude's
rethrowing wrapper, which is what makes a host cancel unwind a task. `zellij.null` (mlua's
null, installed in `loader.rs`) is how a tuple command's optional middle element is left absent
from a Lua table.

hocus is the main consumer of all this: its `plugin_*` tools write, edit, reload, read back the
pane, and grep the log, and `docs/LUA_PLUGINS.md` plus `example-plugins/` plus `prelude.lua` are
served to its model verbatim by `hocus/src/lua_docs.rs`, cut into sections that each stay under
20 KB. Two of those tests guard the guide against the rules it only *describes*: every
`PermissionType` must be nameable from its permission table, and the `Data` section must state
the `shared:` prefix and every `RESERVED_ATTR_NAMES` entry.
A new `##` heading in the guide needs a section name there, or a test fails.
`zellij_plugin_api::authoring` is the other half of that: the naming rules, the backup name, the
lexical permission parser, and `lint`, which reads a source for the traps a compile cannot see -
a permission spelled wrong (granted to nobody, silently), a data namespace that is not
`shared:`-prefixed, a reserved column declared as an attribute, an extension registering without
the permission its registration needs.
`zellij plugins check`, `plugin_check` and `plugin_write` all report it, and none of them refuse
a write over it: the compiler is the only thing that says no. When hocus's
confirmation of a `plugin_write` is approved, the permissions the source asks for are granted
through the builtin-only `PluginCommand::GrantPluginPermissions`, so an authored plugin never
prompts in a pane the loop cannot see. Authoring is hocus's own job and is never delegated to a
spawned coding agent: an authored plugin goes in the user's plugin folder (`get_user_plugin_dir`),
not into this repo, and `example-plugins/` is reference to read rather than somewhere to write.
