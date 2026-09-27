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
    ("zellij-types", &[]),
    ("zellij-plugin-location", &["zellij-types"]),
    (
        "zellij-runtime",
        &["zellij-types", "zellij-plugin-location"],
    ),
    ("zellij-key-codec", &["zellij-types"]),
    ("zellij-host-protocol", &["zellij-types"]),
    ("zellij-storage", &["zellij-types", "zellij-runtime"]),
    ("zellij-project-config", &["zellij-types", "zellij-runtime"]),
    (
        "zellij-config",
        &["zellij-types", "zellij-runtime", "zellij-project-config"],
    ),
    ("zellij-deno-builtins", &["zellij-runtime"]),
    (
        "zellij-plugin-api",
        &["zellij-types", "zellij-plugin-location"],
    ),
    (
        "zellij-plugin-authoring",
        &["zellij-types", "zellij-runtime", "zellij-plugin-api"],
    ),
    (
        "zellij-ipc",
        &["zellij-types", "zellij-runtime", "zellij-plugin-api"],
    ),
    (
        "zellij-session",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-config",
            "zellij-ipc",
        ],
    ),
    (
        "zellij-cli",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-key-codec",
            "zellij-storage",
            "zellij-config",
            "zellij-plugin-api",
            "zellij-ipc",
            "zellij-session",
        ],
    ),
    ("zellij-terminal-graphics", &["zellij-types"]),
    (
        "zellij-terminal",
        &[
            "zellij-types",
            "zellij-host-protocol",
            "zellij-runtime",
            "zellij-ipc",
            "zellij-terminal-graphics",
        ],
    ),
    (
        "zellij-process",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-ipc",
            "zellij-test-support",
        ],
    ),
    (
        "zellij-swamp",
        &[
            "zellij-runtime",
            "zellij-project-config",
            "zellij-test-support",
        ],
    ),
    (
        "zellij-codex",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-session",
            "zellij-process",
            "zellij-test-support",
        ],
    ),
    (
        "zellij-firecracker-provider",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-config",
            "zellij-session",
            "zellij-process",
        ],
    ),
    (
        "zellij-bus",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-storage",
            "zellij-config",
            "zellij-ipc",
            "zellij-session",
            "zellij-host-protocol",
            "zellij-process",
            "zellij-test-support",
        ],
    ),
    (
        "zellij-server-os",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-ipc",
            "zellij-process",
            "zellij-bus",
            "zellij-test-support",
        ],
    ),
    (
        "zellij-routing",
        &["zellij-bus", "zellij-types", "zellij-runtime", "zellij-ipc"],
    ),
    (
        "zellij-panes",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-key-codec",
            "zellij-config",
            "zellij-ipc",
            "zellij-terminal",
            "zellij-bus",
            "zellij-tile",
        ],
    ),
    (
        "zellij-tab",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-key-codec",
            "zellij-config",
            "zellij-ipc",
            "zellij-terminal",
            "zellij-bus",
            "zellij-panes",
        ],
    ),
    (
        "zellij-plugin-host",
        &[
            "zellij-terminal",
            "zellij-types",
            "zellij-runtime",
            "zellij-key-codec",
            "zellij-storage",
            "zellij-config",
            "zellij-deno-builtins",
            "zellij-plugin-api",
            "zellij-plugin-authoring",
            "zellij-ipc",
            "zellij-session",
            "zellij-process",
            "zellij-swamp",
            "zellij-bus",
            "zellij-routing",
            "zellij-tile",
            "zellij-tile-markdown",
            "zellij-test-support",
            "hocus-data",
        ],
    ),
    (
        "zellij-screen",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-storage",
            "zellij-config",
            "zellij-ipc",
            "zellij-session",
            "zellij-cli",
            "zellij-terminal",
            "zellij-host-protocol",
            "zellij-bus",
            "zellij-routing",
            "zellij-panes",
            "zellij-tab",
            "zellij-tile-utils",
            "zellij-test-support",
        ],
    ),
    (
        "zellij-pty",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-storage",
            "zellij-ipc",
            "zellij-session",
            "zellij-bus",
            "zellij-test-support",
        ],
    ),
    ("hocus-data", &["zellij-tile", "zellij-storage"]),
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
    ("zellij-ipc", &["zellij-plugin-api"]),
    ("zellij-cli", &["zellij-plugin-api"]),
    ("zellij-screen", &["zellij-cli", "zellij-test-support"]),
    (
        "zellij-pty",
        &["zellij-ipc", "zellij-storage", "zellij-test-support"],
    ),
    ("zellij-tab", &["zellij-config", "zellij-ipc"]),
    ("zellij-panes", &["zellij-config", "zellij-tile"]),
    ("zellij-deno-builtins", &["zellij-runtime"]),
];

/// The package that composes the concrete backends into the running program.
const COMPOSITION_ROOT: &str = "zellij";

/// Crates only `COMPOSITION_ROOT` may depend on, under any dependency kind or target. They hold
/// concrete backends that every other crate reaches only through a trait, so this rule covers
/// every workspace member, including those `LIBRARY_DEPENDENCIES` does not list.
const ROOT_ONLY_DEPENDENCIES: &[&str] = &["zellij-server-os"];

/// Features that exist only for other crates' tests. Enabled from a normal or build dependency,
/// one is compiled into everything that links the crate, the release binary included, and every
/// build has it on, so a `cfg(any(test, feature = "test-support"))` gate excludes nothing.
const TEST_ONLY_FEATURES: &[&str] = &["test-support"];

/// Crates that must build no JavaScript or Wasm engine and nothing that links one.
/// `zellij-plugin-authoring` reaches V8 and SWC only through the compilation worker subprocess,
/// and Hocus's native tests check plugin sources through it without waiting for the plugin host.
const ENGINE_FREE: &[&str] = &["zellij-plugin-authoring", "hocus"];
const ENGINES: &[&str] = &[
    "deno_core",
    "deno_ast",
    "v8",
    "wasmi",
    "wasmi_core",
    "wasmi_wasi",
    "zellij-plugin-host",
    "zellij-bus",
    "zellij-process",
    "zellij-terminal",
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
        !workspace.iter().any(|p| p["name"] == "zellij-utils"),
        "zellij-utils must not be reintroduced"
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

    /// Every library with all its allowed edges, and Hocus with its authoring dev-dependency.
    fn allowed_workspace() -> Value {
        let mut metadata = json!({
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
        });
        metadata["packages"].as_array_mut().unwrap().push(json!({
            "id": "hocus", "name": "hocus",
            "dependencies": [{"name": "zellij-plugin-authoring", "kind": "dev", "target": null}]
        }));
        metadata["workspace_members"]
            .as_array_mut()
            .unwrap()
            .push(json!("hocus"));
        metadata
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
            ("zellij-server", vec![]),
            ("zellij-client", vec![]),
            ("zellij-integration-tests", vec![]),
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
            ("zellij-types", "zellij-cli"),
            ("zellij-bus", "zellij-routing"),
            ("zellij-bus", "zellij-terminal"),
            ("zellij-bus", "zellij-server-os"),
            ("zellij-panes", "zellij-server-os"),
            ("zellij-routing", "zellij-server-os"),
            ("zellij-plugin-host", "zellij-server-os"),
            ("zellij-server", "zellij-server-os"),
            ("zellij-client", "zellij-server-os"),
            ("zellij-integration-tests", "zellij-server-os"),
            ("zellij-panes", "zellij-routing"),
            ("zellij-panes", "zellij-tab"),
            ("zellij-tab", "zellij-routing"),
            ("zellij-tab", "zellij-server-os"),
            ("zellij-routing", "zellij-plugin-host"),
            ("zellij-routing", "zellij-server"),
            ("zellij-routing", "zellij-panes"),
            // Authoring reads its two constants from the runtime so Hocus's native tests do not
            // build configuration.
            ("zellij-plugin-authoring", "zellij-config"),
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
            ("zellij-plugin-host", "zellij-plugin-authoring"),
            ("zellij-tab", "zellij-panes"),
            ("zellij-screen", "zellij-panes"),
            ("zellij-screen", "zellij-tab"),
            (COMPOSITION_ROOT, "zellij-plugin-api"),
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
        const AUTHORING: &str = "zellij-plugin-authoring";
        for (source, destination, kind, rejected_for) in [
            ("hocus", "zellij-plugin-host", "dev", Some("hocus")),
            ("hocus", "deno_core", "build", Some("hocus")),
            (AUTHORING, "wasmi", "dev", Some(AUTHORING)),
            (AUTHORING, "v8", "normal", Some(AUTHORING)),
            (AUTHORING, "wasmi_wasi", "normal", Some(AUTHORING)),
            ("zellij-runtime", "deno_ast", "build", Some(AUTHORING)),
            ("zellij-types", "zellij-bus", "normal", Some(AUTHORING)),
            // A dependency's dev-dependencies are not built.
            ("zellij-runtime", "deno_core", "dev", None),
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
