//! Turning a Lua call into a [`PluginCommand`].
//!
//! There is no per-command knowledge here, and there is not meant to be. `PluginCommand` derives
//! `Deserialize`, so the whole ~230-command surface converts through one `from_value` call: what
//! Lua passes is a table shaped like the variant's payload, and serde does the rest.
//!
//! The one thing this file knows is serde's *externally tagged* convention, which is how the
//! command's name and its payload are put together:
//!
//! | variant | Rust | Lua |
//! |---|---|---|
//! | unit | `CloseSelf` | `zellij.close_self()` |
//! | newtype | `SetTimeout(f64)` | `zellij.set_timeout(1.0)` |
//! | tuple | `RenameTab(u32, String)` | `zellij.rename_tab { 0, "logs" }` |
//! | struct | `NewTab { name, cwd }` | `zellij.new_tab { name = "logs" }` |
//!
//! Names arrive in `PluginCommand`'s own spelling - the prelude pascal-cases whatever the plugin
//! wrote - so no case conversion happens on this side, and there is no second case rule to drift
//! from serde's.

use std::str::FromStr;

use mlua::{Lua, LuaSerdeExt, Value as LuaValue};
use zellij_runtime::errors::prelude::*;
use zellij_utils::data::{CommandType, PluginCommand};

/// `name` is the command in `PluginCommand`'s own spelling; `spelling` is how the plugin wrote
/// it, and appears in errors so that a typo is reported back as typed.
pub fn plugin_command_from_lua(
    lua: &Lua,
    name: &str,
    spelling: &str,
    args: LuaValue,
) -> Result<PluginCommand> {
    // Checked up front so that a misspelled command says so, rather than producing serde's
    // "unknown variant, expected one of ..." with all ~230 names inlined.
    CommandType::from_str(name).map_err(|_| anyhow!("unknown zellij command '{}'", spelling))?;

    match args {
        // A unit variant is a bare string rather than a table. Nil is the only signal we have -
        // and `zellij.close_self()` does pass nothing - but a struct variant whose fields are all
        // optional is just as reasonably called with no arguments, so that is tried second.
        LuaValue::Nil => from_lua(lua, spelling, LuaValue::String(lua.create_string(name)?))
            .or_else(|_| {
                let empty = LuaValue::Table(lua.create_table()?);
                from_lua(lua, spelling, wrapped(lua, name, spelling, empty)?)
            }),
        args => from_lua(lua, spelling, wrapped(lua, name, spelling, args)?),
    }
}

/// `{ CommandName = payload }` - serde's externally tagged representation of one variant.
fn wrapped(lua: &Lua, name: &str, spelling: &str, payload: LuaValue) -> Result<LuaValue> {
    let table = lua
        .create_table()
        .map_err(|e| anyhow!("failed to build the arguments for '{}': {}", spelling, e))?;
    table
        .set(name, payload)
        .map_err(|e| anyhow!("failed to build the arguments for '{}': {}", spelling, e))?;
    Ok(LuaValue::Table(table))
}

fn from_lua(lua: &Lua, spelling: &str, value: LuaValue) -> Result<PluginCommand> {
    lua.from_value(value)
        .map_err(|e| anyhow!("could not read the arguments for '{}': {}", spelling, e))
}
