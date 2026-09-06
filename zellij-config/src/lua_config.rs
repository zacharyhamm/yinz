use std::{collections::BTreeMap, fs, path::PathBuf};

use mlua::{Function, Lua, Table, Value};

use {super::config::Config, super::config::ConfigError, zellij_types::input::hooks::Hooks};

const PRELUDE: &str = include_str!("lua_config.lua");

#[allow(clippy::result_large_err)]
pub(super) fn from_path(path: &PathBuf, base: Option<Config>) -> Result<Config, ConfigError> {
    let source = fs::read_to_string(path).map_err(|e| ConfigError::IoPath(e, path.clone()))?;
    let lua = Lua::new();
    let config: Table = lua
        .load(PRELUDE)
        .set_name("zellij:config-prelude")
        .eval()
        .map_err(lua_error)?;

    let package: Table = lua.globals().get("package").map_err(lua_error)?;
    let preload: Table = package.get("preload").map_err(lua_error)?;
    let module = config.clone();
    preload
        .set(
            "zellij.config",
            lua.create_function(move |_, ()| Ok(module.clone()))
                .map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    if let Some(parent) = path.parent() {
        let current: String = package.get("path").map_err(lua_error)?;
        let parent = parent.to_string_lossy();
        package
            .set(
                "path",
                format!("{parent}/?.lua;{parent}/?/init.lua;{current}"),
            )
            .map_err(lua_error)?;
    }

    let returned: Value = lua
        .load(&source)
        .set_name(path.display().to_string())
        .eval()
        .map_err(lua_error)?;
    let Value::Table(returned) = returned else {
        return Err(ConfigError::Lua(
            "config.lua must return require(\"zellij.config\")".to_owned(),
        ));
    };
    let render: Function = returned.get("__render").map_err(lua_error)?;
    let kdl: String = render.call(returned.clone()).map_err(lua_error)?;

    let mut base = base.unwrap_or_default();
    let cleared: Table = returned.get("__cleared").map_err(lua_error)?;
    if cleared.get::<bool>("options").unwrap_or(false) {
        base.options = Default::default();
    }
    if cleared.get::<bool>("keybinds").unwrap_or(false) {
        base.keybinds = Default::default();
    }
    if cleared.get::<bool>("themes").unwrap_or(false) {
        base.themes = Default::default();
    }
    if cleared.get::<bool>("plugins").unwrap_or(false) {
        base.plugins = Default::default();
    }
    if cleared.get::<bool>("hooks").unwrap_or(false) {
        base.hooks = Hooks::from_data(BTreeMap::new());
    }
    if cleared.get::<bool>("ui").unwrap_or(false) {
        base.ui = Default::default();
    }
    if cleared.get::<bool>("env").unwrap_or(false) {
        base.env = Default::default();
    }
    Config::from_kdl(&kdl, Some(base)).map_err(|e| {
        ConfigError::Lua(format!(
            "{} generated invalid configuration: {e}",
            path.display()
        ))
    })
}

fn lua_error(error: mlua::Error) -> ConfigError {
    ConfigError::Lua(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use {
        zellij_types::data::InputMode, zellij_types::data::KeyWithModifier,
        zellij_types::input::actions::Action,
    };

    use super::*;

    #[test]
    fn lua_config_uses_the_existing_config_and_keybinding_parser() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.lua");
        fs::write(
            &path,
            r#"
              local config = require("zellij.config")
              config.options { theme = "dracula", simplified_ui = true }
              config.bind("normal", "Ctrl g", {
                config.action("SwitchToMode", "Pane")
              })
              config.plugin("mine", "file:mine.lua", { greeting = "hello" })
              return config
            "#,
        )
        .unwrap();

        let config = from_path(&path, Some(Config::default())).unwrap();
        assert_eq!(config.options.theme.as_deref(), Some("dracula"));
        assert_eq!(config.options.simplified_ui, Some(true));
        let key = KeyWithModifier::from_str("Ctrl g").unwrap();
        assert_eq!(
            config
                .keybinds
                .get_actions_for_key_in_mode(&InputMode::Normal, &key),
            Some(&vec![Action::SwitchToMode {
                input_mode: InputMode::Pane,
            }])
        );
        assert_eq!(
            config.plugins.aliases["mine"]
                .configuration
                .inner()
                .get("greeting")
                .map(String::as_str),
            Some("hello")
        );
    }

    #[test]
    fn lua_config_can_require_a_sibling_and_clear_inherited_keybindings() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.lua"), "return { theme = 'nord' }").unwrap();
        let path = dir.path().join("config.lua");
        fs::write(
            &path,
            r#"
              local config = require("zellij.config")
              local settings = require("settings")
              config.clear_keybinds()
              config.options(settings)
              return config
            "#,
        )
        .unwrap();
        let mut base = Config::default();
        base.keybinds
            .0
            .entry(InputMode::Normal)
            .or_default()
            .insert(KeyWithModifier::from_str("a").unwrap(), vec![Action::Quit]);

        let config = from_path(&path, Some(base)).unwrap();
        assert!(config.keybinds.0.is_empty());
        assert_eq!(config.options.theme.as_deref(), Some("nord"));
    }

    #[test]
    fn automatic_discovery_prefers_lua_but_an_explicit_kdl_path_wins() {
        let dir = tempfile::tempdir().unwrap();
        let lua_path = dir.path().join("config.lua");
        let kdl_path = dir.path().join("config.kdl");
        fs::write(
            &lua_path,
            r#"local config = require("zellij.config"); config.options { theme = "lua" }; return config"#,
        )
        .unwrap();
        fs::write(&kdl_path, "theme \"kdl\"").unwrap();

        let discovered = Config::load(None, Some(dir.path()), false).unwrap();
        assert_eq!(discovered.options.theme.as_deref(), Some("lua"));

        let explicit = Config::load(Some(&kdl_path), Some(dir.path()), false).unwrap();
        assert_eq!(explicit.options.theme.as_deref(), Some("kdl"));
    }
}
