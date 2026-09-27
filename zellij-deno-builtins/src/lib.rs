//! The TypeScript builtin plugins, compiled into the server and transpiled by its Deno worker.
//!
//! Only the plugin host depends on this crate (the whole-app tests also use it as a
//! dev-dependency), so an edit to a builtin's source rebuilds the host and what links it, not the
//! configuration crate or the wasm plugins. `zellij-config` keeps just the tags, in
//! `BUILTIN_DENO_PLUGIN_NAMES`; the plugin host's tests hold the two lists equal.
use include_dir::{include_dir, Dir};

/// TypeScript builtins are compiled into the server and transpiled by its worker.
pub const BUILTIN_DENO_PLUGINS: &[(&str, &str)] = &[
    ("tab-bar", include_str!("../assets/plugins/tab-bar.ts")),
    (
        "status-bar",
        include_str!("../assets/plugins/status-bar.ts"),
    ),
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

/// Module sources and adjacent tests shipped with TypeScript builtins.
pub static BUILTIN_DENO_FILES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/plugins");
