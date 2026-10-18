use super::*;
use yinz_types::internal_ids::TabPosition;
use yinz_types::position::{CellCoordinates, Column, ColumnCount, Row, RowCount};

pub fn decode_from_kdl(
    kdl_document: &KdlDocument,
) -> Result<(Option<TabPosition>, PaneInfo), String> {
    macro_rules! int_node {
        ($name:expr, $type:ident) => {{
            kdl_document
                .get($name)
                .and_then(|n| n.entries().iter().next())
                .and_then(|e| e.value().as_i64())
                .map(|e| e as $type)
                .ok_or(format!("Failed to parse pane {}", $name))?
        }};
    }
    macro_rules! optional_int_node {
        ($name:expr, $type:ident) => {{
            kdl_document
                .get($name)
                .and_then(|n| n.entries().iter().next())
                .and_then(|e| e.value().as_i64())
                .map(|e| e as $type)
        }};
    }
    macro_rules! bool_node {
        ($name:expr) => {{
            kdl_document
                .get($name)
                .and_then(|n| n.entries().iter().next())
                .and_then(|e| e.value().as_bool())
                .ok_or(format!("Failed to parse pane {}", $name))?
        }};
    }
    macro_rules! string_node {
        ($name:expr) => {{
            kdl_document
                .get($name)
                .and_then(|n| n.entries().iter().next())
                .and_then(|e| e.value().as_string())
                .map(|s| s.to_owned())
                .ok_or(format!("Failed to parse pane {}", $name))?
        }};
    }
    macro_rules! optional_string_node {
        ($name:expr) => {{
            kdl_document
                .get($name)
                .and_then(|n| n.entries().iter().next())
                .and_then(|e| e.value().as_string())
                .map(|s| s.to_owned())
        }};
    }
    let tab_position = optional_int_node!("tab_position", i64)
        .map(TabPosition::try_from)
        .transpose()
        .map_err(|_| "Tab position is out of range")?;
    let id = u32::try_from(int_node!("id", i64)).map_err(|_| "Pane id is out of range")?;

    let is_plugin = bool_node!("is_plugin");
    let is_focused = bool_node!("is_focused");
    let is_fullscreen = bool_node!("is_fullscreen");
    let is_floating = bool_node!("is_floating");
    let is_suppressed = bool_node!("is_suppressed");
    let title = string_node!("title");
    let exited = bool_node!("exited");
    let exit_status = optional_int_node!("exit_status", i32);
    let is_held = bool_node!("is_held");
    let pane_x =
        Column::try_from(int_node!("pane_x", i64)).map_err(|_| "pane_x is out of range")?;
    let pane_content_x = Column::try_from(int_node!("pane_content_x", i64))
        .map_err(|_| "pane_content_x is out of range")?;
    let pane_y = Row::try_from(int_node!("pane_y", i64)).map_err(|_| "pane_y is out of range")?;
    let pane_content_y = Row::try_from(int_node!("pane_content_y", i64))
        .map_err(|_| "pane_content_y is out of range")?;
    let pane_rows =
        RowCount::try_from(int_node!("pane_rows", i64)).map_err(|_| "pane_rows is out of range")?;
    let pane_content_rows = RowCount::try_from(int_node!("pane_content_rows", i64))
        .map_err(|_| "pane_content_rows is out of range")?;
    let pane_columns = ColumnCount::try_from(int_node!("pane_columns", i64))
        .map_err(|_| "pane_columns is out of range")?;
    let pane_content_columns = ColumnCount::try_from(int_node!("pane_content_columns", i64))
        .map_err(|_| "pane_content_columns is out of range")?;
    let cursor_coordinates_in_pane = kdl_document
        .get("cursor_coordinates_in_pane")
        .map(|n| {
            let mut entries = n.entries().iter();
            (entries.next(), entries.next())
        })
        .and_then(|(x, y)| {
            let x = x.and_then(|x| x.value().as_i64());
            let y = y.and_then(|y| y.value().as_i64());
            match (x, y) {
                (Some(x), Some(y)) => Some((x, y)),
                _ => None,
            }
        })
        .map(|(x, y)| {
            Ok::<_, String>(CellCoordinates::new(
                Column::try_from(x).map_err(|_| "Cursor column is out of range")?,
                Row::try_from(y).map_err(|_| "Cursor row is out of range")?,
            ))
        })
        .transpose()?;
    let terminal_command = optional_string_node!("terminal_command");
    let cwd = optional_string_node!("cwd");
    let plugin_url = optional_string_node!("plugin_url");
    let is_selectable = bool_node!("is_selectable");
    let pane_info = PaneInfo {
        pane_tabs: None,
        multipane: None,
        host: None,
        target: Default::default(),
        agent: None,
        id: yinz_types::pane_id::PaneId::from_legacy(id, is_plugin),
        is_focused,
        is_fullscreen,
        is_floating,
        is_suppressed,
        is_stack_list_member: kdl_document
            .get("is_stack_list_member")
            .and_then(|node| node.entries().first())
            .and_then(|entry| entry.value().as_bool())
            .unwrap_or(false),
        title,
        display_title: optional_string_node!("display_title"),
        exited,
        exit_status,
        is_held,
        restart_error: None,
        last_agent_session_id: None,
        pane_x,
        pane_content_x,
        pane_y,
        pane_content_y,
        pane_rows,
        pane_content_rows,
        pane_columns,
        pane_content_columns,
        cursor_coordinates_in_pane,
        terminal_command,
        plugin_url,
        is_selectable,
        index_in_pane_group: Default::default(), // we don't serialize this
        default_fg: None,
        default_bg: None,
        cwd,
    };
    Ok((tab_position, pane_info))
}
pub fn encode_to_kdl(value: &PaneInfo) -> anyhow::Result<KdlDocument> {
    let mut kdl_doucment = KdlDocument::new();
    macro_rules! int_node {
        ($name:expr, $val:expr) => {{
            let mut att = KdlNode::new($name);
            att.push(i64::try_from($val)?);
            kdl_doucment.nodes_mut().push(att);
        }};
    }
    macro_rules! bool_node {
        ($name:expr, $val:expr) => {{
            let mut att = KdlNode::new($name);
            att.push($val);
            kdl_doucment.nodes_mut().push(att);
        }};
    }
    macro_rules! string_node {
        ($name:expr, $val:expr) => {{
            let mut att = KdlNode::new($name);
            att.push($val);
            kdl_doucment.nodes_mut().push(att);
        }};
    }

    int_node!("id", value.id.legacy_parts().0);
    bool_node!("is_plugin", value.is_plugin());
    bool_node!("is_focused", value.is_focused);
    bool_node!("is_fullscreen", value.is_fullscreen);
    bool_node!("is_floating", value.is_floating);
    bool_node!("is_suppressed", value.is_suppressed);
    if value.is_stack_list_member {
        bool_node!("is_stack_list_member", true);
    }
    string_node!("title", value.title.to_string());
    if let Some(display_title) = &value.display_title {
        string_node!("display_title", display_title.to_string());
    }
    bool_node!("exited", value.exited);
    if let Some(exit_status) = value.exit_status {
        int_node!("exit_status", exit_status);
    }
    bool_node!("is_held", value.is_held);
    int_node!("pane_x", value.pane_x);
    int_node!("pane_content_x", value.pane_content_x);
    int_node!("pane_y", value.pane_y);
    int_node!("pane_content_y", value.pane_content_y);
    int_node!("pane_rows", value.pane_rows);
    int_node!("pane_content_rows", value.pane_content_rows);
    int_node!("pane_columns", value.pane_columns);
    int_node!("pane_content_columns", value.pane_content_columns);
    if let Some(cursor) = value.cursor_coordinates_in_pane {
        let mut cursor_coordinates_in_pane = KdlNode::new("cursor_coordinates_in_pane");
        cursor_coordinates_in_pane.push(i64::try_from(cursor.x)?);
        cursor_coordinates_in_pane.push(i64::try_from(cursor.y)?);
        kdl_doucment.nodes_mut().push(cursor_coordinates_in_pane);
    }
    if let Some(terminal_command) = &value.terminal_command {
        string_node!("terminal_command", terminal_command.to_string());
    }
    if let Some(cwd) = &value.cwd {
        string_node!("cwd", cwd.to_string());
    }
    if let Some(plugin_url) = &value.plugin_url {
        string_node!("plugin_url", plugin_url.to_string());
    }
    bool_node!("is_selectable", value.is_selectable);
    Ok(kdl_doucment)
}
