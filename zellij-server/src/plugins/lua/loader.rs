//! Creating a Lua interpreter for one plugin and running its source.

use mlua::{Lua, StdLib, Table, Value as LuaValue};
use zellij_utils::errors::prelude::*;

use super::{LuaPlugin, PRELUDE};
use crate::plugins::plugin_map::PluginEnv;

/// The standard library a plugin gets.
///
/// A wasm plugin reaches the filesystem only through the four directories zellij preopens for it,
/// and only through paths the host has rewritten - so the permission system is the whole of its
/// authority. Lua has no such boundary, and stock `io`, `os`, and `package.loadlib` would hand a
/// plugin the server process's full authority and make the permission prompts meaningless.
///
/// So the interpreter is opened closed: string/table/math/utf8 and the base library, nothing that
/// touches the outside world. File access is meant to be reintroduced deliberately, as host
/// commands that go through the same path translation the wasm side uses, rather than by opening
/// `io` and hoping.
fn plugin_stdlib() -> StdLib {
    StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8
}

/// Base-library functions that load code from outside the plugin, and so would sidestep the
/// restricted stdlib above.
const DISALLOWED_GLOBALS: &[&str] = &["dofile", "loadfile", "load", "loadstring", "require"];

/// Load a Lua plugin: build its interpreter, run the prelude, then run the plugin's own source
/// and keep the table it returns.
pub fn load_lua_plugin(source: &str, chunk_name: &str, env: PluginEnv) -> Result<LuaPlugin> {
    let lua = Lua::new_with(plugin_stdlib(), mlua::LuaOptions::default())
        .map_err(|e| anyhow!("failed to create a Lua interpreter: {}", e))?;

    let globals = lua.globals();
    for global in DISALLOWED_GLOBALS {
        globals
            .set(*global, LuaValue::Nil)
            .map_err(|e| anyhow!("failed to restrict '{}': {}", global, e))?;
    }

    let zellij: Table = lua
        .load(PRELUDE)
        .set_name("zellij:prelude")
        .eval()
        .map_err(|e| anyhow!("failed to load zellij's Lua prelude: {}", e))?;

    // Plugins say `require("zellij")`, which is the convention Lua modules are written against,
    // even though `require` itself is not available for anything else.
    let require_zellij = {
        let zellij = zellij.clone();
        lua.create_function(move |_, name: String| {
            if name == "zellij" {
                Ok(zellij.clone())
            } else {
                Err(mlua::Error::runtime(format!(
                    "plugins may only require(\"zellij\"), not \"{}\"",
                    name
                )))
            }
        })
        .map_err(|e| anyhow!("failed to define require: {}", e))?
    };
    globals
        .set("require", require_zellij)
        .map_err(|e| anyhow!("failed to define require: {}", e))?;
    globals
        .set("zellij", zellij.clone())
        .map_err(|e| anyhow!("failed to define the zellij global: {}", e))?;

    let plugin: Table = lua
        .load(source)
        .set_name(chunk_name)
        .eval()
        .map_err(|e| anyhow!("{}", e))
        .with_context(|| {
            format!(
                "failed to load the Lua plugin at '{}'. A plugin must return a table, eg. \
                 `local M = {{}} ... return M`",
                chunk_name
            )
        })?;

    Ok(LuaPlugin {
        lua,
        plugin,
        zellij,
        env,
    })
}
