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
    ("yinz-deno-builtins", &["yinz-runtime"]),
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
        &["yinz-types", "yinz-runtime", "yinz-plugin-api"],
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
            "yinz-ipc",
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
        ],
    ),
    (
        "yinz-plugin-host",
        &[
            "yinz-terminal",
            "yinz-types",
            "yinz-runtime",
            "yinz-key-codec",
            "yinz-storage",
            "yinz-config",
            "yinz-deno-builtins",
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
/// The tab's tests parse KDL layouts and stand up client IPC channels, which the tab never does.
/// The TypeScript builtins' tests hold their files to the runtime's name list, but the builtins
/// name no crate, so the sources the plugin host embeds never wait on or rebuild with the runtime.
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
    ("yinz-tab", &["yinz-config", "yinz-ipc"]),
    ("yinz-panes", &["yinz-config", "yinz-tile"]),
    ("yinz-deno-builtins", &["yinz-runtime"]),
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

/// Crates that must build no JavaScript or Wasm engine and nothing that links one.
/// `yinz-plugin-authoring` reaches V8 and SWC only through the compilation worker subprocess,
/// and Agent's native tests check plugin sources through it without waiting for the plugin host.
const ENGINE_FREE: &[&str] = &["yinz-plugin-authoring"];
const ENGINES: &[&str] = &[
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
mod tests {
    use super::*;
    use serde_json::json;

    /// Every library with all its allowed edges, and Agent with its authoring dev-dependency.
    fn allowed_workspace() -> Value {
        json!({
            "packages": LIBRARY_DEPENDENCIES.iter().map(|(name, allowed)| json!({
                "id": name, "name": name,
                "dependencies": allowed.iter().map(|target| {
                    let dev_only = DEV_ONLY_DEPENDENCIES
                        .iter()
                        .any(|(source, targets)| source == name && targets.contains(target));
                    json!({"name": target, "kind": if dev_only { json!("dev") } else { json!(null) }, "target": null})
                }).collect::<Vec<_>>()
            })).collect::<Vec<_>>(),
            "workspace_members": LIBRARY_DEPENDENCIES.iter().map(|(name, _)| name).collect::<Vec<_>>()
        })
    }

    #[test]
    fn boundaries_cover_all_dependency_kinds_targets_and_aliases() {
        let mut metadata = allowed_workspace();
        validate_dependency_boundaries(&metadata).unwrap();
        // Members `LIBRARY_DEPENDENCIES` does not list, including the composition root, which may
        // depend on the root-only crates under every kind and target.
        let root_dependencies: Vec<_> = ROOT_ONLY_DEPENDENCIES
            .iter()
            .flat_map(|target| {
                [json!(null), json!("build"), json!("dev")]
                    .into_iter()
                    .flat_map(move |kind| {
                        [json!(null), json!("cfg(unix)")].into_iter().map(
                            move |platform| json!({"name": target, "kind": kind, "target": platform}),
                        )
                    })
            })
            .collect();
        for (name, dependencies) in [
            (COMPOSITION_ROOT, root_dependencies),
            ("yinz-server", vec![]),
            ("yinz-client", vec![]),
            ("yinz-integration-tests", vec![]),
        ] {
            metadata["packages"].as_array_mut().unwrap().push(json!({
                "id": name, "name": name, "dependencies": dependencies
            }));
            metadata["workspace_members"]
                .as_array_mut()
                .unwrap()
                .push(json!(name));
        }
        validate_dependency_boundaries(&metadata).unwrap();
        for (source, destination) in [
            ("yinz-types", "yinz-cli"),
            ("yinz-bus", "yinz-routing"),
            ("yinz-bus", "yinz-terminal"),
            ("yinz-bus", "yinz-server-os"),
            ("yinz-panes", "yinz-server-os"),
            ("yinz-routing", "yinz-server-os"),
            ("yinz-plugin-host", "yinz-server-os"),
            ("yinz-server", "yinz-server-os"),
            ("yinz-client", "yinz-server-os"),
            ("yinz-integration-tests", "yinz-server-os"),
            ("yinz-panes", "yinz-routing"),
            ("yinz-panes", "yinz-tab"),
            ("yinz-tab", "yinz-routing"),
            ("yinz-tab", "yinz-server-os"),
            ("yinz-routing", "yinz-plugin-host"),
            ("yinz-routing", "yinz-server"),
            ("yinz-routing", "yinz-panes"),
        ] {
            let index = metadata["packages"]
                .as_array()
                .unwrap()
                .iter()
                .position(|p| p["name"] == source)
                .unwrap();
            let original = metadata["packages"][index]["dependencies"].clone();
            for kind in [json!(null), json!("build"), json!("dev")] {
                for target in [json!(null), json!("cfg(target_family = \"wasm\")")] {
                    metadata["packages"][index]["dependencies"] = json!([{
                        "name": destination, "rename": "innocent_alias", "optional": true,
                        "kind": kind, "target": target
                    }]);
                    assert!(validate_dependency_boundaries(&metadata)
                        .unwrap_err()
                        .to_string()
                        .contains(&format!("{source} may not depend on {destination}")));
                }
            }
            metadata["packages"][index]["dependencies"] = original;
        }

        for (source, targets) in DEV_ONLY_DEPENDENCIES {
            let index = metadata["packages"]
                .as_array()
                .unwrap()
                .iter()
                .position(|p| p["name"] == *source)
                .unwrap();
            let original = metadata["packages"][index]["dependencies"].clone();
            for destination in *targets {
                metadata["packages"][index]["dependencies"] =
                    json!([{"name": destination, "kind": "dev", "target": null}]);
                validate_dependency_boundaries(&metadata).unwrap();
                for kind in [json!(null), json!("build")] {
                    metadata["packages"][index]["dependencies"] =
                        json!([{"name": destination, "kind": kind, "target": null}]);
                    assert!(validate_dependency_boundaries(&metadata)
                        .unwrap_err()
                        .to_string()
                        .contains(&format!(
                            "{source} may depend on {destination} only as a dev-dependency"
                        )));
                }
            }
            metadata["packages"][index]["dependencies"] = original;
        }
    }

    /// The first four edges are the ones that once shipped test helpers in the release binary.
    #[test]
    fn test_only_features_are_enabled_only_from_dev_dependencies() {
        let mut metadata = allowed_workspace();
        metadata["packages"].as_array_mut().unwrap().push(json!({
            "id": COMPOSITION_ROOT, "name": COMPOSITION_ROOT, "dependencies": []
        }));
        metadata["workspace_members"]
            .as_array_mut()
            .unwrap()
            .push(json!(COMPOSITION_ROOT));
        for (source, destination) in [
            ("yinz-plugin-host", "yinz-plugin-authoring"),
            ("yinz-tab", "yinz-panes"),
            ("yinz-screen", "yinz-panes"),
            ("yinz-screen", "yinz-tab"),
            (COMPOSITION_ROOT, "yinz-plugin-api"),
        ] {
            let index = metadata["packages"]
                .as_array()
                .unwrap()
                .iter()
                .position(|p| p["name"] == source)
                .unwrap();
            let original = metadata["packages"][index]["dependencies"].clone();
            for (kind, feature) in [
                (json!("dev"), "test-support"),
                (json!(null), "unrelated"),
                (json!("build"), "unrelated"),
            ] {
                metadata["packages"][index]["dependencies"] = json!([{
                    "name": destination, "kind": kind, "target": null, "features": [feature]
                }]);
                validate_dependency_boundaries(&metadata).unwrap();
            }
            for kind in [json!(null), json!("build")] {
                for target in [json!(null), json!("cfg(unix)")] {
                    metadata["packages"][index]["dependencies"] = json!([{
                        "name": destination, "rename": "innocent_alias", "optional": true,
                        "kind": kind, "target": target, "features": ["unrelated", "test-support"]
                    }]);
                    assert!(validate_dependency_boundaries(&metadata)
                        .unwrap_err()
                        .to_string()
                        .contains(&format!(
                            "{source} may enable {destination}'s test-support feature only from \
                             a dev-dependency"
                        )));
                }
            }
            metadata["packages"][index]["dependencies"] = original;
        }
    }

    #[test]
    fn engine_free_crates_reject_engines_directly_and_through_workspace_crates() {
        let allowed = allowed_workspace();
        const AUTHORING: &str = "yinz-plugin-authoring";
        for (source, destination, kind, rejected_for) in [
            (AUTHORING, "wasmi", "dev", Some(AUTHORING)),
            (AUTHORING, "v8", "normal", Some(AUTHORING)),
            (AUTHORING, "wasmi_wasi", "normal", Some(AUTHORING)),
            ("yinz-runtime", "deno_ast", "build", Some(AUTHORING)),
            ("yinz-types", "yinz-bus", "normal", Some(AUTHORING)),
            // A dependency's dev-dependencies are not built.
            ("yinz-runtime", "deno_core", "dev", None),
        ] {
            let mut metadata = allowed.clone();
            let package = metadata["packages"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|p| p["name"] == source)
                .unwrap();
            package["dependencies"].as_array_mut().unwrap().push(json!({
                "name": destination, "rename": "innocent_alias", "optional": true,
                "kind": if kind == "normal" { json!(null) } else { json!(kind) },
                "target": "cfg(unix)"
            }));
            let workspace: Vec<_> = metadata["packages"].as_array().unwrap().iter().collect();
            let result = validate_engine_free(&workspace);
            match rejected_for {
                Some(root) => assert!(
                    result.unwrap_err().to_string().contains(&format!(
                        "{root} may not build {destination}, a dependency of {source}"
                    )),
                    "{source} -> {destination} ({kind})"
                ),
                None => result.unwrap(),
            }
        }
    }
}
