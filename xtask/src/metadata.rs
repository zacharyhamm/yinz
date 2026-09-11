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
        "zellij-panes",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-config",
            "zellij-ipc",
            "zellij-terminal",
            "zellij-bus",
            "zellij-tile-utils",
            "zellij-tile",
            "zellij-test-support",
        ],
    ),
    (
        "zellij-plugin-host",
        &[
            "zellij-types",
            "zellij-runtime",
            "zellij-storage",
            "zellij-config",
            "zellij-plugin-api",
            "zellij-ipc",
            "zellij-session",
            "zellij-process",
            "zellij-bus",
            "zellij-tile",
            "zellij-tile-markdown",
            "zellij-test-support",
            "hocus-proto",
        ],
    ),
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
                "dependencies": allowed.iter().map(|target| json!({"name": target, "kind": null, "target": null})).collect::<Vec<_>>()
            })).collect::<Vec<_>>(),
            "workspace_members": LIBRARY_DEPENDENCIES.iter().map(|(name, _)| name).collect::<Vec<_>>()
        });
        validate_dependency_boundaries(&metadata).unwrap();
        for kind in [json!(null), json!("build"), json!("dev")] {
            for target in [json!(null), json!("cfg(target_family = \"wasm\")")] {
                metadata["packages"][0]["dependencies"] = json!([{
                    "name": "zellij-cli", "rename": "innocent_alias", "optional": true,
                    "kind": kind, "target": target
                }]);
                assert!(validate_dependency_boundaries(&metadata)
                    .unwrap_err()
                    .to_string()
                    .contains("zellij-types may not depend on zellij-cli"));
            }
        }
    }
}
