//! Helper functions for querying cargo metadata
use anyhow::Context;
use serde_json::Value;
use xshell::{cmd, Shell};

/// Get cargo metadata for the workspace
pub fn get_cargo_metadata(sh: &Shell) -> anyhow::Result<Value> {
    let cargo = crate::cargo().context("Failed to find cargo executable")?;
    let metadata_json = cmd!(sh, "{cargo} metadata --format-version 1 --no-deps")
        .read()
        .context("Failed to run cargo metadata")?;

    serde_json::from_str(&metadata_json).context("Failed to parse cargo metadata JSON")
}

/// Check every dependency entry Cargo reports, including build, dev, optional and target ones.
pub fn check_dependency_boundaries(sh: &Shell) -> anyhow::Result<()> {
    let _pd = sh.push_dir(crate::project_root());
    validate_dependency_boundaries(&get_cargo_metadata(sh)?)
}

const LIBRARY_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("yinz-types", &[]),
    ("yinz-plugin-location", &["yinz-types"]),
    ("yinz-runtime", &["yinz-types", "yinz-plugin-location"]),
    ("yinz-key-codec", &["yinz-types"]),
    ("yinz-host-protocol", &["yinz-types"]),
    ("yinz-storage", &["yinz-types", "yinz-runtime"]),
    (
        "yinz-project-config",
        &["yinz-types", "yinz-runtime", "yinz-test-support"],
    ),
    (
        "yinz-config",
        &["yinz-types", "yinz-runtime", "yinz-project-config"],
    ),
    (
        "yinz-steel-builtins",
        &[
            "yinz-runtime",
            "yinz-types",
            "yinz-config",
            "yinz-steel",
            "yinz-test-support",
            "yinz-tile-markdown",
        ],
    ),
    (
        "yinz-steel",
        &[
            "yinz-config",
            "yinz-tile",
            "yinz-tile-markdown",
            "yinz-types",
            "agent-proto",
        ],
    ),
    ("yinz-plugin-api", &["yinz-types", "yinz-plugin-location"]),
    (
        "yinz-plugin-authoring",
        &[
            "yinz-config",
            "yinz-types",
            "yinz-runtime",
            "yinz-plugin-api",
            "yinz-test-support",
        ],
    ),
    (
        "yinz-ipc",
        &[
            "yinz-types",
            "yinz-runtime",
            "yinz-plugin-api",
            "yinz-host-protocol",
        ],
    ),
    (
        "yinz-session",
        &["yinz-types", "yinz-runtime", "yinz-config", "yinz-ipc"],
    ),
    (
        "yinz-cli",
        &[
            "yinz-types",
            "yinz-runtime",
            "yinz-key-codec",
            "yinz-storage",
            "yinz-config",
            "yinz-plugin-api",
            "yinz-ipc",
            "yinz-session",
        ],
    ),
    ("yinz-terminal-graphics", &["yinz-types"]),
    (
        "yinz-terminal",
        &[
            "yinz-types",
            "yinz-host-protocol",
            "yinz-runtime",
            "yinz-terminal-graphics",
        ],
    ),
    (
        "yinz-process",
        &[
            "yinz-types",
            "yinz-runtime",
            "yinz-ipc",
            "yinz-test-support",
        ],
    ),
    (
        "yinz-codex",
        &[
            "yinz-types",
            "yinz-runtime",
            "yinz-session",
            "yinz-process",
            "yinz-test-support",
        ],
    ),
    (
        "yinz-bus",
        &[
            "yinz-types",
            "yinz-runtime",
            "yinz-storage",
            "yinz-config",
            "yinz-ipc",
            "yinz-session",
            "yinz-host-protocol",
            "yinz-process",
            "yinz-test-support",
        ],
    ),
    (
        "yinz-server-os",
        &[
            "yinz-types",
            "yinz-runtime",
            // The OS backend runs accepted Bubblewrap environment launch checks.
            "yinz-project-config",
            "yinz-ipc",
            "yinz-process",
            "yinz-bus",
            "yinz-test-support",
        ],
    ),
    (
        "yinz-routing",
        &["yinz-bus", "yinz-types", "yinz-runtime", "yinz-ipc"],
    ),
    (
        "yinz-panes",
        &[
            "yinz-types",
            "yinz-runtime",
            "yinz-key-codec",
            "yinz-config",
            "yinz-ipc",
            "yinz-terminal",
            "yinz-bus",
            "yinz-tile",
            // The native agent palette shares the plugin Markdown parser and wrapping.
            "yinz-tile-markdown",
            // It also shares agent timeline, envelope, and quota presentation.
            "agent-proto",
        ],
    ),
    (
        "yinz-tab",
        &[
            "yinz-types",
            "yinz-runtime",
            "yinz-key-codec",
            "yinz-config",
            "yinz-ipc",
            "yinz-terminal",
            "yinz-bus",
            "yinz-panes",
            "yinz-test-support",
        ],
    ),
    (
        "yinz-plugin-host",
        &[
            // Read-only local account quota RPC through Codex's app-server transport.
            "yinz-codex",
            "yinz-terminal",
            "yinz-types",
            "yinz-runtime",
            "yinz-key-codec",
            "yinz-storage",
            "yinz-config",
            "yinz-steel-builtins",
            "yinz-steel",
            "yinz-plugin-api",
            "yinz-plugin-authoring",
            "yinz-ipc",
            "yinz-session",
            "yinz-process",
            "yinz-bus",
            "yinz-routing",
            "yinz-tile",
            "yinz-tile-markdown",
            "yinz-test-support",
            "project-data",
            // Native palette workers reuse the shared archive readers.
            "agent-proto",
        ],
    ),
    (
        "yinz-screen",
        &[
            "yinz-types",
            "yinz-runtime",
            "yinz-storage",
            "yinz-config",
            "yinz-ipc",
            "yinz-session",
            "yinz-cli",
            "yinz-terminal",
            "yinz-host-protocol",
            "yinz-bus",
            "yinz-routing",
            "yinz-panes",
            "yinz-tab",
            "yinz-tile-utils",
            "yinz-test-support",
        ],
    ),
    (
        "yinz-pty",
        &[
            "yinz-types",
            "yinz-runtime",
            "yinz-storage",
            "yinz-ipc",
            "yinz-session",
            "yinz-bus",
            "yinz-test-support",
        ],
    ),
    ("project-data", &["yinz-tile", "yinz-storage"]),
];

/// Edges `LIBRARY_DEPENDENCIES` allows only as dev-dependencies. The client/server transport and
/// the CLI cross-check their wire formats against the plugin API in tests, but must not wait on its
/// generated code to build. The screen's tests drive it with parsed CLI actions and the pty's tests
/// assert on IPC messages and recorded events and wait with test-support's timeouts, but neither
/// thread names those crates otherwise.
/// The tab's tests parse KDL layouts, stand up client IPC channels, and warm clipboard helpers.
/// Builtin disk lookup uses the runtime name list; only its evaluator and config tests
/// need the interpreter and configuration crates.
const DEV_ONLY_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("yinz-project-config", &["yinz-test-support"]),
    ("yinz-plugin-authoring", &["yinz-test-support"]),
    ("yinz-ipc", &["yinz-plugin-api"]),
    ("yinz-cli", &["yinz-plugin-api"]),
    ("yinz-screen", &["yinz-cli", "yinz-test-support"]),
    (
        "yinz-pty",
        &["yinz-ipc", "yinz-storage", "yinz-test-support"],
    ),
    (
        "yinz-tab",
        &["yinz-config", "yinz-ipc", "yinz-test-support"],
    ),
    ("yinz-panes", &["yinz-config", "yinz-tile"]),
    (
        "yinz-steel-builtins",
        &[
            "yinz-types",
            "yinz-config",
            "yinz-steel",
            "yinz-test-support",
            "yinz-tile-markdown",
        ],
    ),
];

/// The package that composes the concrete backends into the running program.
const COMPOSITION_ROOT: &str = "yinz";

/// Crates only `COMPOSITION_ROOT` may depend on, under any dependency kind or target. They hold
/// concrete backends that every other crate reaches only through a trait, so this rule covers
/// every workspace member, including those `LIBRARY_DEPENDENCIES` does not list.
const ROOT_ONLY_DEPENDENCIES: &[&str] = &["yinz-server-os"];

/// Features that exist only for other crates' tests. Enabled from a normal or build dependency,
/// one is compiled into everything that links the crate, the release binary included, and every
/// build has it on, so a `cfg(any(test, feature = "test-support"))` gate excludes nothing.
const TEST_ONLY_FEATURES: &[&str] = &["test-support"];

/// Crates that must build no guest-language engine and nothing that links one.
/// `yinz-plugin-authoring` reaches the interpreter only through the compilation worker subprocess,
/// and Agent's native tests check plugin sources through it without waiting for the plugin host.
const ENGINE_FREE: &[&str] = &["yinz-plugin-authoring"];
const ENGINES: &[&str] = &[
    "yinz-steel",
    "steel-core",
    "deno_core",
    "deno_ast",
    "v8",
    "wasmi",
    "wasmi_core",
    "wasmi_wasi",
    "yinz-plugin-host",
    "yinz-bus",
    "yinz-process",
    "yinz-terminal",
];

fn validate_dependency_boundaries(metadata: &Value) -> anyhow::Result<()> {
    let packages = metadata["packages"]
        .as_array()
        .context("Missing packages in Cargo metadata")?;
    let members = metadata["workspace_members"]
        .as_array()
        .context("Missing workspace members in Cargo metadata")?;
    let workspace: Vec<_> = packages
        .iter()
        .filter(|p| members.contains(&p["id"]))
        .collect();
    for package in workspace.iter().filter(|p| p["name"] != COMPOSITION_ROOT) {
        for dependency in package["dependencies"]
            .as_array()
            .context("Missing dependencies in Cargo metadata")?
        {
            let target = dependency["name"]
                .as_str()
                .context("Missing dependency name")?;
            if ROOT_ONLY_DEPENDENCIES.contains(&target) {
                anyhow::bail!(
                    "{} may not depend on {target}; only {COMPOSITION_ROOT} may (kind: {}, target: {})",
                    package["name"].as_str().context("Missing package name")?,
                    dependency["kind"],
                    dependency["target"]
                );
            }
        }
    }
    for package in &workspace {
        for dependency in package["dependencies"]
            .as_array()
            .context("Missing dependencies in Cargo metadata")?
        {
            if dependency["kind"] == "dev" {
                continue;
            }
            let features = dependency["features"].as_array().into_iter().flatten();
            if let Some(feature) = features
                .filter_map(Value::as_str)
                .find(|feature| TEST_ONLY_FEATURES.contains(feature))
            {
                anyhow::bail!(
                    "{} may enable {}'s {feature} feature only from a dev-dependency (kind: {}, target: {})",
                    package["name"].as_str().context("Missing package name")?,
                    dependency["name"]
                        .as_str()
                        .context("Missing dependency name")?,
                    dependency["kind"],
                    dependency["target"]
                );
            }
        }
    }
    for (name, allowed) in LIBRARY_DEPENDENCIES {
        let package = workspace
            .iter()
            .find(|p| p["name"] == *name)
            .with_context(|| format!("Required library {name} is missing from the workspace"))?;
        for dependency in package["dependencies"]
            .as_array()
            .context("Missing dependencies in Cargo metadata")?
        {
            let target = dependency["name"]
                .as_str()
                .context("Missing dependency name")?;
            if workspace.iter().any(|p| p["name"] == target) && !allowed.contains(&target) {
                anyhow::bail!(
                    "{name} may not depend on {target} (kind: {}, target: {})",
                    dependency["kind"],
                    dependency["target"]
                );
            }
            let dev_only = DEV_ONLY_DEPENDENCIES
                .iter()
                .any(|(source, targets)| source == name && targets.contains(&target));
            if dev_only && dependency["kind"] != "dev" {
                anyhow::bail!(
                    "{name} may depend on {target} only as a dev-dependency (kind: {}, target: {})",
                    dependency["kind"],
                    dependency["target"]
                );
            }
        }
    }
    anyhow::ensure!(
        !workspace.iter().any(|p| p["name"] == "yinz-utils"),
        "yinz-utils must not be reintroduced"
    );
    validate_engine_free(&workspace)
}

/// Checks each `ENGINE_FREE` crate's dependencies of every kind, then the normal and build
/// dependencies of each workspace crate it builds. Only direct edges from workspace crates are
/// visible in `--no-deps` metadata.
fn validate_engine_free(workspace: &[&Value]) -> anyhow::Result<()> {
    for root in ENGINE_FREE {
        let mut pending = vec![*root];
        let mut seen = Vec::new();
        while let Some(name) = pending.pop() {
            if seen.contains(&name) {
                continue;
            }
            seen.push(name);
            let package = workspace
                .iter()
                .find(|p| p["name"] == name)
                .with_context(|| {
                    format!("Engine-free crate {name} is missing from the workspace")
                })?;
            for dependency in package["dependencies"]
                .as_array()
                .context("Missing dependencies in Cargo metadata")?
            {
                if name != *root && dependency["kind"] == "dev" {
                    continue;
                }
                let target = dependency["name"]
                    .as_str()
                    .context("Missing dependency name")?;
                if ENGINES.contains(&target) {
                    anyhow::bail!(
                        "{root} may not build {target}, a dependency of {name} (kind: {}, target: {})",
                        dependency["kind"],
                        dependency["target"]
                    );
                }
                if let Some(next) = workspace.iter().find(|p| p["name"] == target) {
                    pending.push(next["name"].as_str().context("Missing package name")?);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/metadata.rs"]
mod tests;
