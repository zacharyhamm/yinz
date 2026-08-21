//! Turning a Lua call into a [`PluginCommand`].
//!
//! This is the one place where the Lua runtime has per-command knowledge, and it is deliberately
//! small. `PluginCommand` does not derive `Deserialize` today - protobuf is its only
//! serialization path - so the commands the first Lua plugins need are built by hand here.
//!
//! The intended end state is that `PluginCommand` gains `#[derive(Deserialize)]` and the body of
//! [`plugin_command_from_lua`] becomes a single `lua.from_value` call, converting all ~200
//! commands at once. The signature is written for that: callers pass a name and a value and get
//! a command back, and none of them will need to change.

use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;

use mlua::Value as LuaValue;
use zellij_utils::data::{EventType, FileToOpen, PermissionType, PluginCommand};
use zellij_utils::errors::prelude::*;

pub fn plugin_command_from_lua(name: &str, args: LuaValue) -> Result<PluginCommand> {
    match name {
        "subscribe" => Ok(PluginCommand::Subscribe(event_types(args)?)),
        "unsubscribe" => Ok(PluginCommand::Unsubscribe(event_types(args)?)),
        "request_permission" => Ok(PluginCommand::RequestPluginPermissions(permissions(args)?)),
        "set_selectable" => Ok(PluginCommand::SetSelectable(matches!(
            args,
            LuaValue::Boolean(true)
        ))),
        // `OpenTerminal` carries a `FileToOpen` but only reads its `path`, treating it as the
        // cwd - so a bare string is accepted here and put where the handler looks.
        "open_terminal" => Ok(PluginCommand::OpenTerminal(cwd_to_open(args)?)),
        "close_self" => Ok(PluginCommand::CloseSelf),
        "set_timeout" => Ok(PluginCommand::SetTimeout(number(args, "set_timeout")?)),
        other => Err(anyhow!("unknown zellij command '{}'", other)),
    }
}

/// Parse a Lua list of event names. They arrive already in `EventType`'s own spelling, the
/// prelude having converted whatever the plugin wrote.
fn event_types(args: LuaValue) -> Result<HashSet<EventType>> {
    let mut event_types = HashSet::new();
    for name in string_list(args, "event types")? {
        let event_type = EventType::from_str(&name)
            .map_err(|_| anyhow!("'{}' is not an event zellij can send", name))?;
        event_types.insert(event_type);
    }
    Ok(event_types)
}

fn permissions(args: LuaValue) -> Result<Vec<PermissionType>> {
    let mut permissions = vec![];
    for name in string_list(args, "permissions")? {
        let permission = PermissionType::from_str(&name)
            .map_err(|_| anyhow!("'{}' is not a permission zellij grants", name))?;
        permissions.push(permission);
    }
    Ok(permissions)
}

/// Build the `FileToOpen` that `OpenTerminal` expects, which reads the cwd out of `path`.
fn cwd_to_open(args: LuaValue) -> Result<FileToOpen> {
    let mut file = FileToOpen::default();
    match args {
        LuaValue::String(cwd) => {
            file.path = PathBuf::from(cwd.to_string_lossy().to_string());
        },
        LuaValue::Table(_) => {
            let from_table = file_to_open(args)?;
            file.path = from_table
                .cwd
                .clone()
                .unwrap_or_else(|| from_table.path.clone());
        },
        _ => {},
    }
    Ok(file)
}

fn file_to_open(args: LuaValue) -> Result<FileToOpen> {
    let mut file = FileToOpen::default();
    let LuaValue::Table(table) = args else {
        return Ok(file);
    };
    if let Ok(LuaValue::String(cwd)) = table.get::<LuaValue>("cwd") {
        file.cwd = Some(PathBuf::from(cwd.to_string_lossy().to_string()));
    }
    if let Ok(LuaValue::String(path)) = table.get::<LuaValue>("path") {
        file.path = PathBuf::from(path.to_string_lossy().to_string());
    }
    Ok(file)
}

fn number(args: LuaValue, what: &str) -> Result<f64> {
    match args {
        LuaValue::Number(n) => Ok(n),
        LuaValue::Integer(n) => Ok(n as f64),
        _ => Err(anyhow!("{} expects a number", what)),
    }
}

fn string_list(args: LuaValue, what: &str) -> Result<Vec<String>> {
    let LuaValue::Table(table) = args else {
        return Err(anyhow!("expected a list of {}", what));
    };
    let mut out = vec![];
    for entry in table.sequence_values::<String>() {
        out.push(entry.map_err(|e| anyhow!("expected a list of {}: {}", what, e))?);
    }
    Ok(out)
}
