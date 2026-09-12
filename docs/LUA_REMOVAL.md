# Migrate from Lua

Lua plugins and Lua configuration are no longer supported. Rust/Wasm plugins remain supported.
Deno is the only script runtime. All shipped script builtins retain their `zellij:` names.

Rewrite custom Lua plugins as TypeScript with the [Deno SDK](DENO_PLUGINS.md). Change plugin URLs
and hook bindings to the new files. Renaming a file does not convert its source. Use `CheckPlugin`
instead of the retired `CheckLuaPlugin` API. Extensionless authoring names now select `.ts`.
Explicit `.js`, `.mjs`, `.ts`, and `.mts` names keep their extensions. Explicit `.lua` names fail.

Move configuration settings manually into `config.kdl`. Automatic discovery ignores `config.lua`,
even when it is the only configuration file. An explicit `--config config.lua` fails with an
unsupported-format error. KDL overlays, reloads, CLI overrides, and plugin configuration blocks
still apply. There is no `config.ts` or configuration evaluator. Existing user files are untouched.
