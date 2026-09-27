use include_dir::{include_dir, Dir};
pub static ZELLIJ_DEFAULT_THEMES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/themes");

pub const FEATURES: &[&str] = &[];

pub const BUILTIN_PLUGIN_NAMES: &[&str] = &[
    "compact-bar",
    "strider",
    "session-manager",
    "configuration",
    "plugin-manager",
    "about",
    "multiple-select",
    "layout-manager",
    "link",
    "file-finder",
    "quit-confirmation",
    "shared-test",
    "hocus",
    "hocus-agentd",
    "hocus-agents",
    "hocus-todos",
    "hocus-new-agent",
    "hocus-store",
];

/// The TypeScript builtins. Only their tags live here: the sources are compiled into the plugin
/// host from `zellij-deno-builtins`, so editing one does not rebuild this crate or anything that
/// depends on it. The plugin host's tests hold the two lists equal.
pub const BUILTIN_DENO_PLUGIN_NAMES: &[&str] = &[
    "tab-bar",
    "status-bar",
    "context-menu",
    "agent-policy",
    "swamp-bridge",
    "swamp-workflow",
    "agent-exit",
];

/// The checkout's freshly built debug plugins, baked in at compile time from this crate's
/// manifest dir. Searched *before* any installed copy: a `cargo x run` or `cargo test` on the
/// same `VERSION` as an installed zellij must see the plugins it just built, not last week's
/// install. Only debug binaries look here; on an installed system the folder does not exist
/// and is skipped.
#[cfg(all(not(target_family = "wasm"), debug_assertions))]
pub const BUILTIN_PLUGIN_DEBUG_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../target/wasm32-wasip1/debug");

/// The checkout's release plugins, the last resort for a binary run straight out of `target/`
/// without a `cargo x install`. Baked in at compile time like the debug dir, and skipped in
/// the same way where it does not exist. Note that a same-`VERSION` install is preferred over
/// this folder - see `PluginConfig::resolve_wasm_bytes` for the whole order.
#[cfg(not(target_family = "wasm"))]
pub const BUILTIN_PLUGIN_RELEASE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../target/wasm32-wasip1/release"
);
