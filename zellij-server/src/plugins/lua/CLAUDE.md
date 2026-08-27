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
  an unrestricted `io` would make it decorative.

`PluginCommand` derives `Deserialize`, and `lua/command.rs` is one `from_value` call, so the whole
command surface is reachable from Lua and no command needs a match arm. Two consequences worth
knowing: serde's variant names are now a second contract alongside the hand-written protobuf one
in `plugin_api/plugin_command.rs` (a test walks `CommandType` to catch drift), and a new command
needs nothing at all done to it to be callable from Lua.

Replies are the exception, and are still per-command: the response types are prost-generated with
no serde representation, so `lua/response.rs` decodes the ones it knows and hands back raw bytes
for the rest. `zellij-tile/src/shim.rs` is the reference for what each response means.
