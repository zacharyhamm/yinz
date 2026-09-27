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
    ("zellij-downloader", &[]),
    ("zellij-key-codec", &["zellij-types"]),
    ("zellij-storage", &["zellij-types", "zellij-runtime"]),
    ("zellij-project-config", &["zellij-types", "zellij-runtime"]),
    (
        "zellij-config",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-downloader",
            "zellij-project-config",
        ],
    ),
    ("zellij-deno-builtins", &[]),
    (
        "zellij-plugin-api",
        &["zellij-types", "zellij-plugin-location"],
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
    (
        "zellij-terminal",
        &["zellij-types", "zellij-runtime", "zellij-ipc"],
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
            "zellij-terminal",
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
            "zellij-test-support",
        ],
    ),
    (
        "zellij-plugin-host",
        &[
            "zellij-terminal",
            "zellij-types",
            "zellij-runtime",
            "zellij-downloader",
            "zellij-key-codec",
            "zellij-storage",
            "zellij-config",
            "zellij-deno-builtins",
            "zellij-plugin-api",
            "zellij-ipc",
            "zellij-session",
            "zellij-process",
            "zellij-swamp",
            "zellij-bus",
            "zellij-routing",
            "zellij-tile",
            "zellij-tile-markdown",
            "zellij-test-support",
            "hocus-proto",
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
            "zellij-bus",
            "zellij-routing",
            "zellij-panes",
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
        ],
    ),
];

/// Edges `LIBRARY_DEPENDENCIES` allows only as dev-dependencies. The client/server transport and
/// the CLI cross-check their wire formats against the plugin API in tests, but must not wait on its
/// generated code to build. The screen's tests drive it with parsed CLI actions and the pty's tests
/// assert on IPC messages and recorded events, but neither thread names those crates otherwise.
const DEV_ONLY_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("zellij-ipc", &["zellij-plugin-api"]),
    ("zellij-cli", &["zellij-plugin-api"]),
    ("zellij-screen", &["zellij-cli", "zellij-test-support"]),
    ("zellij-pty", &["zellij-ipc", "zellij-storage"]),
];

/// The package that composes the concrete backends into the running program.
const COMPOSITION_ROOT: &str = "zellij";

/// Crates only `COMPOSITION_ROOT` may depend on, under any dependency kind or target. They hold
/// concrete backends that every other crate reaches only through a trait, so this rule covers
/// every workspace member, including those `LIBRARY_DEPENDENCIES` does not list.
const ROOT_ONLY_DEPENDENCIES: &[&str] = &["zellij-server-os"];

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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn boundaries_cover_all_dependency_kinds_targets_and_aliases() {
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
            ("zellij-bus", "zellij-server-os"),
            ("zellij-panes", "zellij-server-os"),
            ("zellij-routing", "zellij-server-os"),
            ("zellij-plugin-host", "zellij-server-os"),
            ("zellij-server", "zellij-server-os"),
            ("zellij-client", "zellij-server-os"),
            ("zellij-integration-tests", "zellij-server-os"),
            ("zellij-panes", "zellij-routing"),
            ("zellij-routing", "zellij-plugin-host"),
            ("zellij-routing", "zellij-server"),
            ("zellij-routing", "zellij-panes"),
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
}
