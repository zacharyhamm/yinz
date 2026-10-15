//! Compact transport for fields whose host API type is a byte sequence.
//!
//! The marker is interpreted only at those typed positions. Ordinary JSON, including objects
//! with the same key, always crosses the general bridge unchanged.

#[path = "binary_command.rs"]
mod command;
pub use command::decode_command;

use anyhow::Result;
use serde_json::Value;
use steel::rvals::SteelVal;

pub use yinz_types::compact_bytes::{MARKER, MAX_BYTES};

pub fn encode(bytes: &[u8]) -> Result<Value> {
    Ok(yinz_types::compact_bytes::encode(bytes)?)
}

pub fn decode(value: &Value) -> Result<Vec<u8>> {
    Ok(yinz_types::compact_bytes::decode(value)?)
}

pub(crate) fn reply(command: &Value, value: &Value) -> Result<SteelVal> {
    let command = command
        .get("__yinz_runtime")
        .and_then(|value| value.get("command"))
        .unwrap_or(command);
    let name = command
        .as_object()
        .and_then(|command| command.keys().next())
        .map(String::as_str);
    match name {
        Some("FsRead") => crate::bridge::to_typed(value, |path| path.is_empty()),
        Some("GetPaneGitProbe") => crate::bridge::to_typed(
            value,
            |path| matches!(path, [field] if field == "metadata" || field == "config"),
        ),
        Some("DataFetch") => crate::bridge::to_typed(value, rows_byte_path),
        Some("DataWatch") => crate::bridge::to_typed(value, |path| {
            path.first().is_some_and(|key| key == "rows") && rows_byte_path(&path[1..])
        }),
        Some("DataCatalog") => crate::bridge::to_typed(value, catalog_byte_path),
        _ => crate::to_scheme(value),
    }
}

pub(crate) fn event(value: &Value) -> Result<SteelVal> {
    crate::bridge::to_typed(value, |path| match path {
        [event, field] => match event.as_str() {
            "PaneOutput" | "RunCommandOutput" => field == "data",
            "WebRequestStreamChunk" => field == "chunk",
            "RunCommandResult" => field == "1" || field == "2",
            "WebRequestResult" => field == "2",
            _ => false,
        },
        [event, rows, rest @ ..] if event == "PluginDataChanged" && rows == "rows" => {
            rows_byte_path(rest)
        },
        _ => false,
    })
}

fn rows_byte_path(path: &[String]) -> bool {
    matches!(path, [rows, row, column, blob] if rows == "rows" && index(row) && index(column) && blob == "Blob")
}
fn index(value: &str) -> bool {
    value.parse::<usize>().is_ok()
}
fn catalog_byte_path(path: &[String]) -> bool {
    matches!(path, [entity, def, attrs, attr, default, blob] if index(entity) && def == "def" && attrs == "attrs" && index(attr) && default == "default" && blob == "Blob")
}
fn filter_byte_path(path: &[String]) -> bool {
    match path {
        [operation, rest @ ..] if operation == "Not" => filter_byte_path(rest),
        [operation, item, rest @ ..]
            if (operation == "And" || operation == "Or") && index(item) =>
        {
            filter_byte_path(rest)
        },
        [operation, value, blob]
            if matches!(
                operation.as_str(),
                "Eq" | "Ne" | "Gt" | "Gte" | "Lt" | "Lte"
            ) =>
        {
            value == "1" && blob == "Blob"
        },
        [operation, values, item, blob] if operation == "In" => {
            values == "1" && index(item) && blob == "Blob"
        },
        _ => false,
    }
}
fn data_command_byte_path(path: &[String]) -> bool {
    match path {
        [command, def, attrs, attr, default, blob] if command == "DataDefineEntity" => {
            def == "def"
                && attrs == "attrs"
                && index(attr)
                && default == "default"
                && blob == "Blob"
        },
        [command, request, filter, rest @ ..]
            if matches!(command.as_str(), "DataFetch" | "DataCount" | "DataWatch")
                && request == "request"
                && filter == "filter" =>
        {
            filter_byte_path(rest)
        },
        [command, mutations, mutation, kind, rest @ ..]
            if command == "DataApply" && mutations == "mutations" && index(mutation) =>
        {
            match rest {
                [values, item, value, blob] if kind == "Put" || kind == "Update" => {
                    values == "values" && index(item) && value == "1" && blob == "Blob"
                },
                [filter, rest @ ..]
                    if matches!(kind.as_str(), "Update" | "Delete" | "AssertRows")
                        && filter == "filter" =>
                {
                    filter_byte_path(rest)
                },
                _ => false,
            }
        },
        _ => false,
    }
}

pub(crate) fn decode_utf8(value: SteelVal) -> Result<String> {
    match value {
        SteelVal::ByteVector(bytes) => bytes.with_bytes(|bytes| {
            anyhow::ensure!(bytes.len() <= MAX_BYTES, "UTF-8 input exceeds 64 MiB");
            Ok(String::from_utf8_lossy(bytes).into_owned())
        }),
        _ => Ok(String::from_utf8_lossy(&decode(&crate::from_scheme(&value)?)?).into_owned()),
    }
}

// Paths are validated by the host's typed PluginCommand/Action decoder as well.
pub(crate) fn command_byte_path(path: &[String]) -> bool {
    let path = if path.first().is_some_and(|part| part == "__yinz_runtime") {
        if path.get(1).is_some_and(|part| part == "command") {
            &path[2..]
        } else {
            return false;
        }
    } else {
        path
    };
    if data_command_byte_path(path) {
        return true;
    }
    match path {
        [command] => command == "Write",
        [command, field] => match command.as_str() {
            "FsWrite" => field == "contents",
            "WriteToPaneId" => field == "0",
            "WebRequest" => field == "3",
            _ => false,
        },
        [command, index, rest @ ..] if command == "RunAction" && index == "0" => {
            action_byte_path(rest)
        },
        _ => false,
    }
}
fn action_byte_path(path: &[String]) -> bool {
    match path {
        [action, field, rest @ ..] if action == "SkipConfirm" && field == "action" => {
            action_byte_path(rest)
        },
        [action, field] => match action.as_str() {
            "Write" | "WriteToPaneId" => field == "bytes",
            "PaneNameInput" | "TabNameInput" | "SearchInput" => field == "input",
            "RenameTerminalPane" | "RenamePluginPane" | "RenameTab" | "RenamePaneByPaneId" => {
                field == "name"
            },
            _ => false,
        },
        _ => false,
    }
}

/// Expand typed binary fields for harnesses that record ordinary JSON commands.
/// Other fields, including subscription ordering and marker-shaped user data, stay unchanged.
pub fn expand_command_bytes(mut command: Value) -> Result<Value> {
    fn expand(value: &mut Value, path: &mut Vec<String>) -> Result<()> {
        anyhow::ensure!(path.len() <= 64, "command nesting exceeds 64");
        if command_byte_path(path) && value.is_object() {
            *value = serde_json::to_value(decode(value)?)?;
            return Ok(());
        }
        match value {
            Value::Array(values) => {
                for (index, value) in values.iter_mut().enumerate() {
                    path.push(index.to_string());
                    expand(value, path)?;
                    path.pop();
                }
            },
            Value::Object(values) => {
                for (key, value) in values.iter_mut() {
                    path.push(key.clone());
                    expand(value, path)?;
                    path.pop();
                }
            },
            _ => {},
        }
        Ok(())
    }
    expand(&mut command, &mut Vec::new())?;
    Ok(command)
}
