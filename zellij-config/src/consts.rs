use include_dir::{include_dir, Dir};
pub static ZELLIJ_DEFAULT_THEMES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/themes");

pub const FEATURES: &[&str] = &[];

pub const BUILTIN_PLUGIN_NAMES: &[&str] = &[
    "compact-bar",
    "status-bar",
    "tab-bar",
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
    "hocus-new-agent",
    "hocus-store",
];

/// TypeScript builtins are compiled into the server and transpiled by its worker.
pub const BUILTIN_DENO_PLUGINS: &[(&str, &str)] = &[
    (
        "context-menu",
        include_str!("../assets/plugins/context-menu.ts"),
    ),
    (
        "agent-policy",
        include_str!("../assets/plugins/agent-policy.ts"),
    ),
    (
        "swamp-bridge",
        include_str!("../assets/plugins/swamp-bridge.ts"),
    ),
    (
        "swamp-workflow",
        include_str!("../assets/plugins/swamp-workflow.ts"),
    ),
    (
        "agent-exit",
        include_str!("../assets/plugins/agent-exit.ts"),
    ),
];

pub fn builtin_deno_plugin(name: &str) -> Option<&'static str> {
    BUILTIN_DENO_PLUGINS
        .iter()
        .find(|(tag, _)| *tag == name)
        .map(|(_, source)| *source)
}

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

/// Sources and adjacent tests shipped for installed TypeScript plugin testing.
pub static BUILTIN_DENO_TEST_FILES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/plugins");
