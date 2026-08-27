# Lua plugins

Plugins come in two kinds: wasm modules (every builtin) and Lua scripts. The Lua runtime lives in
`zellij-server/src/plugins/lua/`; `docs/LUA_PLUGINS.md` is the user-facing guide.

The two runtimes share everything above `RunningPlugin`: event routing, subscriptions, rendering,
and — importantly — the whole command surface. `zellij_exports.rs` has one dispatcher,
`run_plugin_command(&mut PluginEnv, PluginCommand)`, and both runtimes call it, so a capability is
never implemented twice. What differs is confined to the `PluginBackend` enum and the entrypoint
methods on `RunningPlugin`.

Two things to know before changing this:

- **Ergonomics belong in `lua/prelude.lua`, not in Rust.** Rust exposes exactly one function to
  Lua (`__dispatch`), mirroring the single `host_run_plugin_command` wasm import. `zellij.on`,
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
no serde representation, so `lua/response.rs` decodes each one by hand into the `zellij_utils::data`
type the wasm shim would build, and hands back raw bytes for anything it does not know. A test
lists every reply-returning command and fails when a new one is not decoded.
`zellij-tile/src/shim.rs` is the reference for what each response means.

hocus is the main consumer of all this: its `plugin_*` tools write, reload, read back the pane,
and grep the log, and `docs/LUA_PLUGINS.md` plus `example-plugins/` plus `prelude.lua` are served
to its model verbatim by `hocus/src/lua_docs.rs`, cut into sections that each stay under 20 KB.
A new `##` heading in the guide needs a section name there, or a test fails. When hocus's
confirmation of a `plugin_write` is approved, the permissions the source asks for are granted
through the builtin-only `PluginCommand::GrantPluginPermissions`, so an authored plugin never
prompts in a pane the loop cannot see.
