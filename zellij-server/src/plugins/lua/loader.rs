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
/// So the parts of the stdlib that reach outside the interpreter stay shut, and the capabilities
/// worth having arrive as host commands instead - the `Fs*` family, which goes through the same
/// four roots the wasm side sees and is gated on `ReadFiles`/`WriteFiles` like every other
/// capability.
///
/// What is opened here is what cannot reach outside the VM on its own: `string`, `table`, `math`,
/// `utf8`, `coroutine` and the base library, plus the harmless part of `os`, which
/// [`restrict_table`] cuts down below. `io` and `package` are never opened at all, which is what
/// keeps `package.loadlib` - and `package.loaded`, through which the discarded original `os`
/// could otherwise be recovered - out of reach.
///
/// `debug` is absent because mlua will not open it in a safe interpreter at all: the library can
/// reach upvalues and userdata metatables, which is memory-unsafety rather than merely too much
/// authority. Having `debug.traceback` would mean building the whole interpreter unsafely, which
/// is a poor trade for a nicer error message.
fn plugin_stdlib() -> StdLib {
    StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE | StdLib::OS
}

/// The `os` functions a plugin keeps: the clock, and nothing else.
///
/// A wasm plugin reads the clock through WASI with no permission at all, so time is not authority
/// and asking the user to approve it would be noise. Everything else `os` offers is: `execute`
/// and `exit` are the process, `getenv` is the server's environment, `remove`/`rename` are the
/// filesystem without the confinement the `Fs*` commands apply, `tmpname` hands out a path
/// outside every root, and `setlocale` mutates process-global C locale for the whole server -
/// a plugin changing how the *application* formats numbers.
const ALLOWED_OS: &[&str] = &["time", "date", "difftime", "clock"];

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

    restrict_table(&lua, &globals, "os", ALLOWED_OS)?;

    let zellij: Table = lua
        .load(PRELUDE)
        .set_name("zellij:prelude")
        .eval()
        .map_err(|e| anyhow!("failed to load zellij's Lua prelude: {}", e))?;

    install_json(&lua, &zellij)?;

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

/// Replace the global named `name` with a fresh table holding only `allowed`, taken from it.
///
/// A whitelist rather than a deny-list on purpose. Nilling out the names known to be dangerous
/// leaves the next Lua release free to add one this file has never heard of, and the cost of
/// missing a single name here is the whole sandbox - `os.setlocale` alone would let a plugin
/// change how the entire server formats numbers. Building the replacement from the names that
/// were vetted means anything unlisted is gone whether or not it was thought of.
fn restrict_table(lua: &Lua, globals: &Table, name: &str, allowed: &[&str]) -> Result<()> {
    let original: Table = globals
        .get(name)
        .map_err(|e| anyhow!("failed to read the '{}' library: {}", name, e))?;
    let restricted = lua
        .create_table()
        .map_err(|e| anyhow!("failed to restrict '{}': {}", name, e))?;
    for entry in allowed {
        let value: LuaValue = original
            .get(*entry)
            .map_err(|e| anyhow!("failed to read '{}.{}': {}", name, entry, e))?;
        restricted
            .set(*entry, value)
            .map_err(|e| anyhow!("failed to restrict '{}.{}': {}", name, entry, e))?;
    }
    globals
        .set(name, restricted)
        .map_err(|e| anyhow!("failed to restrict '{}': {}", name, e))?;
    Ok(())
}

/// Install `zellij.json.encode` / `zellij.json.decode`.
///
/// In Rust rather than the prelude, unusually for this runtime: a JSON parser written in Lua
/// would be slower and larger than the one already linked in, and this widens no authority -
/// it converts values that are already in the plugin's hands.
///
/// Both directions go through mlua's serde bridge with the same options [`to_lua_value`] uses,
/// so a JSON `null` decodes to a real `nil` rather than mlua's null userdata, and a plugin
/// checking `if value then` gets the answer Lua would give. The one thing a caller has to know
/// is that Lua cannot tell an empty array from an empty table, so `encode {}` is `[]`.
fn install_json(lua: &Lua, zellij: &Table) -> Result<()> {
    use mlua::LuaSerdeExt;

    let json = lua
        .create_table()
        .map_err(|e| anyhow!("failed to define zellij.json: {}", e))?;

    let decode = lua
        .create_function(|lua, text: mlua::LuaString| {
            let text = text.to_str()?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| mlua::Error::runtime(format!("could not decode JSON: {}", e)))?;
            super::to_lua_value(lua, &value).map_err(|e| mlua::Error::runtime(format!("{:#}", e)))
        })
        .map_err(|e| anyhow!("failed to define zellij.json.decode: {}", e))?;

    let encode = lua
        .create_function(|lua, value: LuaValue| {
            let value: serde_json::Value = lua.from_value(value).map_err(|e| {
                mlua::Error::runtime(format!("could not encode this as JSON: {}", e))
            })?;
            serde_json::to_string(&value)
                .map_err(|e| mlua::Error::runtime(format!("could not encode JSON: {}", e)))
        })
        .map_err(|e| anyhow!("failed to define zellij.json.encode: {}", e))?;

    json.set("decode", decode)
        .and_then(|_| json.set("encode", encode))
        .map_err(|e| anyhow!("failed to define zellij.json: {}", e))?;
    zellij
        .set("json", json)
        .map_err(|e| anyhow!("failed to define zellij.json: {}", e))?;
    Ok(())
}
