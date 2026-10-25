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
        (AUTHORING, "yinz-steel", "dev", Some(AUTHORING)),
        (AUTHORING, "steel-core", "normal", Some(AUTHORING)),
        ("yinz-types", "yinz-steel", "normal", Some(AUTHORING)),
        ("yinz-runtime", "steel-core", "build", Some(AUTHORING)),
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
