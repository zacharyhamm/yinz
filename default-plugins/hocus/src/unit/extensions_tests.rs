use super::*;

fn discovered() -> BTreeMap<String, String> {
    BTreeMap::from([(
        "weather".to_owned(),
        "/plugins/hocus/weather.lua".to_owned(),
    )])
}

fn instances() -> BTreeMap<zellij_tile::prelude::PluginId, (String, String)> {
    BTreeMap::from([(
        zellij_tile::prelude::PluginId::new(7),
        (
            "weather".to_owned(),
            "/plugins/hocus/weather.lua".to_owned(),
        ),
    )])
}

fn registration(safety: &str, consequence: Option<&str>) -> String {
    let mut tool = json!({
        "name": "forecast",
        "description": "Read the forecast.",
        "parameters": {
            "type": "object",
            "properties": { "city": { "type": "string" } },
            "required": ["city"]
        },
        "safety": safety,
    });
    if let Some(consequence) = consequence {
        tool["consequence"] = json!(consequence);
    }
    json!({
        "version": VERSION,
        "namespace": "weather",
        "path": "/plugins/hocus/weather.lua",
        "tools": [tool],
    })
    .to_string()
}

#[test]
fn discovery_keeps_only_direct_lower_snake_case_extension_files() {
    let files = extension_files(
        "/plugins",
        "/plugins/clock.lua\n/plugins/hocus/weather.lua\n/plugins/hocus/bad-name.lua\n\
         /plugins/hocus/nested/mod.lua\n",
    );
    assert_eq!(files, (discovered(), vec![]));
}

#[test]
fn registration_namespaces_the_tool_and_builds_its_api_spec() {
    let mut registry = Registry::default();
    let names = registry
        .register(
            zellij_tile::prelude::PluginId::new(7),
            &registration("read_only", None),
            &discovered(),
            &instances(),
            &BTreeSet::new(),
        )
        .unwrap();
    assert_eq!(names, vec!["weather_forecast"]);
    let tool = registry.tool("weather_forecast").unwrap();
    assert_eq!(tool.owner, zellij_tile::prelude::PluginId::new(7));
    assert_eq!(tool.safety, Safety::ReadOnly);
    assert_eq!(tool.spec()["function"]["parameters"]["type"], "object");
    assert_eq!(
        registry.owner_for_path(&tool.path),
        Some(zellij_tile::prelude::PluginId::new(7))
    );
}

#[test]
fn safety_is_explicit_and_confirming_tools_explain_their_effect() {
    let mut registry = Registry::default();
    let missing = registration("confirm", None);
    assert!(registry
        .register(
            zellij_tile::prelude::PluginId::new(7),
            &missing,
            &discovered(),
            &instances(),
            &BTreeSet::new()
        )
        .unwrap_err()
        .contains("needs a consequence"));

    registry
        .register(
            zellij_tile::prelude::PluginId::new(7),
            &registration("confirm", Some("posts to the shared channel")),
            &discovered(),
            &instances(),
            &BTreeSet::new(),
        )
        .unwrap();
    assert_eq!(
        registry
            .tool("weather_forecast")
            .unwrap()
            .consequence
            .as_deref(),
        Some("posts to the shared channel")
    );
}

#[test]
fn a_registration_cannot_shadow_hocus_or_another_extension() {
    let mut registry = Registry::default();
    let reserved = BTreeSet::from(["weather_forecast".to_owned()]);
    assert!(registry
        .register(
            zellij_tile::prelude::PluginId::new(7),
            &registration("automatic", None),
            &discovered(),
            &instances(),
            &reserved
        )
        .unwrap_err()
        .contains("collides"));

    registry
        .register(
            zellij_tile::prelude::PluginId::new(7),
            &registration("automatic", None),
            &discovered(),
            &instances(),
            &BTreeSet::new(),
        )
        .unwrap();
    assert!(registry
        .register(
            zellij_tile::prelude::PluginId::new(8),
            &registration("automatic", None),
            &discovered(),
            &instances(),
            &BTreeSet::new(),
        )
        .unwrap_err()
        .contains("does not own"));
}

#[test]
fn reregistering_replaces_an_owners_complete_tool_set() {
    let mut registry = Registry::default();
    registry
        .register(
            zellij_tile::prelude::PluginId::new(7),
            &registration("automatic", None),
            &discovered(),
            &instances(),
            &BTreeSet::new(),
        )
        .unwrap();
    let empty = json!({
        "version": VERSION,
        "namespace": "weather",
        "path": "/plugins/hocus/weather.lua",
        "tools": [],
    })
    .to_string();
    registry
        .register(
            zellij_tile::prelude::PluginId::new(7),
            &empty,
            &discovered(),
            &instances(),
            &BTreeSet::new(),
        )
        .unwrap();
    assert!(registry.names().is_empty());
    assert_eq!(
        registry.owner_for_path("/plugins/hocus/weather.lua"),
        Some(zellij_tile::prelude::PluginId::new(7))
    );
}

#[test]
fn script_suffixes_share_a_namespace_and_conflicts_register_neither() {
    for suffix in ["lua", "js", "mjs", "ts", "mts"] {
        let path = format!("/plugins/hocus/weather.{suffix}");
        assert_eq!(namespace_for_path("/plugins", &path), Some("weather"));
        assert_eq!(extension_files("/plugins", &path).0["weather"], path);
    }
    let (files, errors) = extension_files("/plugins", "/plugins/hocus/weather.lua\n/plugins/hocus/weather.ts\n/plugins/hocus/weather.mts\n/plugins/hocus/weather.bak.ts");
    assert!(files.is_empty());
    assert_eq!(errors.len(), 1);
    for suffix in ["lua", "ts", "mts"] {
        assert!(errors[0].contains(&format!("weather.{suffix}")));
    }
}
