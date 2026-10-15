//! Decode compact byte fields without constructing a JSON value for every byte.

use anyhow::{Context, Result};
use serde_json::Value;
use yinz_types::{data::PluginCommand, input::actions::Action};

pub fn decode_command(mut value: Value) -> Result<PluginCommand> {
    let bytes = if let Some(field) = command_field(&mut value) {
        if field.is_object() {
            let bytes = crate::binary::decode(field).context("decode command bytes")?;
            *field = Value::Array(Vec::new());
            Some(bytes)
        } else {
            None
        }
    } else {
        None
    };
    let mut command: PluginCommand =
        serde_json::from_value(value).context("decode plugin command")?;
    if let Some(bytes) = bytes {
        *command_bytes(&mut command).context("missing decoded command byte field")? = bytes;
    }
    Ok(command)
}

fn command_field(value: &mut Value) -> Option<&mut Value> {
    let object = value.as_object_mut()?;
    if object.len() != 1 {
        return None;
    }
    let (name, value) = object.iter_mut().next()?;
    match name.as_str() {
        "Write" => Some(value),
        "FsWrite" => value.get_mut("contents"),
        "WebRequest" => value.get_mut(3),
        "WriteToPaneId" => value.get_mut(0),
        "RunAction" => action_field(value.get_mut(0)?),
        _ => None,
    }
}

fn action_field(mut value: &mut Value) -> Option<&mut Value> {
    loop {
        let object = value.as_object_mut()?;
        if object.len() != 1 {
            return None;
        }
        let (name, arguments) = object.iter_mut().next()?;
        match name.as_str() {
            "Write" | "WriteToPaneId" => return arguments.get_mut("bytes"),
            "PaneNameInput" | "TabNameInput" | "SearchInput" => {
                return arguments.get_mut("input");
            },
            "RenameTerminalPane" | "RenamePluginPane" | "RenameTab" | "RenamePaneByPaneId" => {
                return arguments.get_mut("name");
            },
            "SkipConfirm" => value = arguments.get_mut("action")?,
            _ => return None,
        }
    }
}

fn command_bytes(command: &mut PluginCommand) -> Option<&mut Vec<u8>> {
    match command {
        PluginCommand::Write(bytes)
        | PluginCommand::WriteToPaneId(bytes, _)
        | PluginCommand::WebRequest(_, _, _, bytes, _, _, _)
        | PluginCommand::FsWrite {
            contents: bytes, ..
        } => Some(bytes),
        PluginCommand::RunAction(action, _) => action_bytes(action),
        _ => None,
    }
}

fn action_bytes(mut action: &mut Action) -> Option<&mut Vec<u8>> {
    loop {
        match action {
            Action::Write { bytes, .. } | Action::WriteToPaneId { bytes, .. } => {
                return Some(bytes)
            },
            Action::PaneNameInput { input }
            | Action::TabNameInput { input }
            | Action::SearchInput { input } => return Some(input),
            Action::RenameTerminalPane { name, .. }
            | Action::RenamePluginPane { name, .. }
            | Action::RenameTab { name, .. }
            | Action::RenamePaneByPaneId { name, .. } => return Some(name),
            Action::SkipConfirm { action: inner } => action = inner,
            _ => return None,
        }
    }
}

#[cfg(test)]
#[path = "unit/binary_command.rs"]
mod tests;
