use super::*;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;
use yinz_types::position::{ColumnCount, RowCount};

pub fn decode_from_kdl(kdl_document: &KdlDocument) -> Result<TabInfo, String> {
    macro_rules! int_node {
        ($name:expr, $type:ident) => {{
            kdl_document
                .get($name)
                .and_then(|n| n.entries().iter().next())
                .and_then(|e| e.value().as_i64())
                .map(|e| e as $type)
                .ok_or(format!("Failed to parse tab {}", $name))?
        }};
    }
    macro_rules! string_node {
        ($name:expr) => {{
            kdl_document
                .get($name)
                .and_then(|n| n.entries().iter().next())
                .and_then(|e| e.value().as_string())
                .map(|s| s.to_owned())
                .ok_or(format!("Failed to parse tab {}", $name))?
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
                .ok_or(format!("Failed to parse tab {}", $name))?
        }};
    }

    let position = TabPosition::try_from(int_node!("position", i64))
        .map_err(|_| "Tab position is out of range")?;
    let name = string_node!("name");
    let active = bool_node!("active");
    let panes_to_hide = int_node!("panes_to_hide", usize);
    let is_fullscreen_active = bool_node!("is_fullscreen_active");
    let is_sync_panes_active = bool_node!("is_sync_panes_active");
    let are_floating_panes_visible = bool_node!("are_floating_panes_visible");
    let mut other_focused_clients = vec![];
    if let Some(tab_other_focused_clients) = kdl_document
        .get("other_focused_clients")
        .map(|n| n.entries())
    {
        for entry in tab_other_focused_clients {
            if let Some(entry_parsed) = entry.value().as_i64() {
                other_focused_clients.push(
                    ClientId::try_from(entry_parsed)
                        .map_err(|_| "client_id is outside u32 range")?,
                );
            }
        }
    }
    let active_swap_layout_name = optional_string_node!("active_swap_layout_name");
    let viewport_rows = RowCount::try_from(optional_int_node!("viewport_rows", i64).unwrap_or(0))
        .map_err(|_| "viewport_rows is out of range")?;
    let viewport_columns =
        ColumnCount::try_from(optional_int_node!("viewport_columns", i64).unwrap_or(0))
            .map_err(|_| "viewport_columns is out of range")?;
    let display_area_rows =
        RowCount::try_from(optional_int_node!("display_area_rows", i64).unwrap_or(0))
            .map_err(|_| "display_area_rows is out of range")?;
    let display_area_columns =
        ColumnCount::try_from(optional_int_node!("display_area_columns", i64).unwrap_or(0))
            .map_err(|_| "display_area_columns is out of range")?;
    let is_swap_layout_dirty = bool_node!("is_swap_layout_dirty");
    let selectable_tiled_panes_count =
        optional_int_node!("selectable_tiled_panes_count", usize).unwrap_or(0);
    let selectable_floating_panes_count =
        optional_int_node!("selectable_floating_panes_count", usize).unwrap_or(0);
    let tab_id = TabId::try_from(optional_int_node!("tab_id", i64).unwrap_or(0))
        .map_err(|_| "tab_id is outside usize range")?;
    let group = optional_int_node!("group_id", i64)
        .map(|id| {
            Ok::<_, String>(yinz_types::tab_bar::TabGroupInfo {
                id: u32::try_from(id).map_err(|_| "Group ID is out of range")?,
                name: optional_string_node!("group_name").unwrap_or_else(|| "Default".into()),
            })
        })
        .transpose()?;
    Ok(TabInfo {
        group,
        position,
        name,
        active,
        panes_to_hide,
        is_fullscreen_active,
        is_sync_panes_active,
        are_floating_panes_visible,
        other_focused_clients,
        other_focused_client_slots: vec![],
        active_swap_layout_name,
        is_swap_layout_dirty,
        viewport_rows,
        viewport_columns,
        display_area_rows,
        display_area_columns,
        selectable_tiled_panes_count,
        selectable_floating_panes_count,
        tab_id,
        has_bell_notification: false,
        is_flashing_bell: false,
    })
}
pub fn encode_to_kdl(value: &TabInfo) -> anyhow::Result<KdlDocument> {
    let mut kdl_doucment = KdlDocument::new();

    if let Some(group) = &value.group {
        let mut id = KdlNode::new("group_id");
        id.push(i64::from(group.id));
        let mut name = KdlNode::new("group_name");
        name.push(group.name.clone());
        kdl_doucment.nodes_mut().extend([id, name]);
    }
    let mut position = KdlNode::new("position");
    position.push(i64::try_from(value.position.get())?);
    kdl_doucment.nodes_mut().push(position);

    let mut name = KdlNode::new("name");
    name.push(value.name.clone());
    kdl_doucment.nodes_mut().push(name);

    let mut active = KdlNode::new("active");
    active.push(value.active);
    kdl_doucment.nodes_mut().push(active);

    let mut panes_to_hide = KdlNode::new("panes_to_hide");
    panes_to_hide.push(value.panes_to_hide as i64);
    kdl_doucment.nodes_mut().push(panes_to_hide);

    let mut is_fullscreen_active = KdlNode::new("is_fullscreen_active");
    is_fullscreen_active.push(value.is_fullscreen_active);
    kdl_doucment.nodes_mut().push(is_fullscreen_active);

    let mut is_sync_panes_active = KdlNode::new("is_sync_panes_active");
    is_sync_panes_active.push(value.is_sync_panes_active);
    kdl_doucment.nodes_mut().push(is_sync_panes_active);

    let mut are_floating_panes_visible = KdlNode::new("are_floating_panes_visible");
    are_floating_panes_visible.push(value.are_floating_panes_visible);
    kdl_doucment.nodes_mut().push(are_floating_panes_visible);

    if !value.other_focused_clients.is_empty() {
        let mut other_focused_clients = KdlNode::new("other_focused_clients");
        for client_id in &value.other_focused_clients {
            other_focused_clients.push(i64::from(client_id.get()));
        }
        kdl_doucment.nodes_mut().push(other_focused_clients);
    }

    if let Some(active_swap_layout_name) = value.active_swap_layout_name.as_ref() {
        let mut active_swap_layout = KdlNode::new("active_swap_layout_name");
        active_swap_layout.push(active_swap_layout_name.to_string());
        kdl_doucment.nodes_mut().push(active_swap_layout);
    }

    let mut viewport_rows = KdlNode::new("viewport_rows");
    viewport_rows.push(i64::try_from(value.viewport_rows)?);
    kdl_doucment.nodes_mut().push(viewport_rows);

    let mut viewport_columns = KdlNode::new("viewport_columns");
    viewport_columns.push(i64::try_from(value.viewport_columns)?);
    kdl_doucment.nodes_mut().push(viewport_columns);

    let mut display_area_columns = KdlNode::new("display_area_columns");
    display_area_columns.push(i64::try_from(value.display_area_columns)?);
    kdl_doucment.nodes_mut().push(display_area_columns);

    let mut display_area_rows = KdlNode::new("display_area_rows");
    display_area_rows.push(i64::try_from(value.display_area_rows)?);
    kdl_doucment.nodes_mut().push(display_area_rows);

    let mut is_swap_layout_dirty = KdlNode::new("is_swap_layout_dirty");
    is_swap_layout_dirty.push(value.is_swap_layout_dirty);
    kdl_doucment.nodes_mut().push(is_swap_layout_dirty);

    let mut selectable_tiled_panes_count = KdlNode::new("selectable_tiled_panes_count");
    selectable_tiled_panes_count.push(value.selectable_tiled_panes_count as i64);
    kdl_doucment.nodes_mut().push(selectable_tiled_panes_count);

    let mut selectable_floating_panes_count = KdlNode::new("selectable_floating_panes_count");
    selectable_floating_panes_count.push(value.selectable_floating_panes_count as i64);
    kdl_doucment
        .nodes_mut()
        .push(selectable_floating_panes_count);

    let mut tab_id = KdlNode::new("tab_id");
    tab_id.push(i64::try_from(value.tab_id.get())?);
    kdl_doucment.nodes_mut().push(tab_id);

    Ok(kdl_doucment)
}
