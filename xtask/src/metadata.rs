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
    ("zellij-runtime", &["zellij-types"]),
    ("zellij-storage", &["zellij-types", "zellij-runtime"]),
    ("zellij-config", &["zellij-types", "zellij-runtime"]),
    ("zellij-plugin-api", &["zellij-types", "zellij-runtime"]),
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
            "zellij-config",
            "zellij-ipc",
            "zellij-session",
            "zellij-test-support",
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
        "zellij-routing",
        &["zellij-bus", "zellij-types", "zellij-runtime", "zellij-ipc"],
    ),
    (
        "zellij-panes",
        &[
            "zellij-types",
            "zellij-runtime",
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
            "zellij-storage",
            "zellij-config",
            "zellij-plugin-api",
            "zellij-ipc",
            "zellij-session",
            "zellij-process",
            "zellij-bus",
            "zellij-routing",
            "zellij-tile",
            "zellij-tile-markdown",
            "zellij-test-support",
            "hocus-proto",
        ],
    ),
];

/// Edges `LIBRARY_DEPENDENCIES` allows only as dev-dependencies. The client/server transport and
/// the CLI cross-check their wire formats against the plugin API in tests, but must not wait on its
/// generated code to build.
const DEV_ONLY_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("zellij-ipc", &["zellij-plugin-api"]),
    ("zellij-cli", &["zellij-plugin-api"]),
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
        metadata["packages"].as_array_mut().unwrap().push(json!({
            "id": "zellij-server", "name": "zellij-server", "dependencies": []
        }));
        metadata["workspace_members"]
            .as_array_mut()
            .unwrap()
            .push(json!("zellij-server"));
        for (source, destination) in [
            ("zellij-types", "zellij-cli"),
            ("zellij-bus", "zellij-routing"),
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
