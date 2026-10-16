use super::*;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TabId;

pub fn from_string(
    raw_session_info: &str,
    current_session_name: &SessionName,
) -> Result<SessionInfo, String> {
    let kdl_document: KdlDocument = raw_session_info
        .parse()
        .map_err(|e| format!("Failed to parse kdl document: {}", e))?;
    let name = kdl_document
        .get("name")
        .and_then(|n| n.entries().iter().next())
        .and_then(|e| e.value().as_string())
        .map(|s| s.to_owned())
        .ok_or("Failed to parse session name")?;
    let connected_clients = kdl_document
        .get("connected_clients")
        .and_then(|n| n.entries().iter().next())
        .and_then(|e| e.value().as_i64())
        .map(|c| c as usize)
        .ok_or("Failed to parse connected_clients")?;
    let tabs: Vec<TabInfo> = kdl_document
        .get("tabs")
        .and_then(|t| t.children())
        .and_then(|c| {
            let mut tab_nodes = vec![];
            for tab_node in c.nodes() {
                if let Some(tab) = tab_node.children() {
                    tab_nodes.push(crate::metadata::tab_info::decode_from_kdl(tab).ok()?);
                }
            }
            Some(tab_nodes)
        })
        .ok_or("Failed to parse tabs")?;
    let panes: PaneManifest = kdl_document
        .get("panes")
        .and_then(|p| p.children())
        .map(crate::metadata::pane_manifest::decode_from_kdl)
        .ok_or("Failed to parse panes")?;
    let available_layouts: Vec<LayoutInfo> = kdl_document
        .get("available_layouts")
        .and_then(|p| p.children())
        .map(|e| {
            e.nodes()
                .iter()
                .filter_map(|n| {
                    let layout_name = n.name().value().to_owned();
                    let layout_source = n
                        .entries()
                        .iter()
                        .find(|e| e.name().map(|n| n.value()) == Some("source"))
                        .and_then(|e| e.value().as_string());
                    match layout_source {
                        Some(layout_source) => match layout_source {
                            "built-in" => Some(LayoutInfo::BuiltIn(layout_name)),
                            "file" => {
                                Some(LayoutInfo::File(layout_name, LayoutMetadata::default()))
                            },
                            _ => None,
                        },
                        None => None,
                    }
                })
                .collect()
        })
        .ok_or("Failed to parse available_layouts")?;
    let is_current_session = name == current_session_name.as_str();
    let mut tab_history = BTreeMap::new();
    if let Some(kdl_tab_history) = kdl_document.get("tab_history").and_then(|p| p.children()) {
        for client_node in kdl_tab_history.nodes() {
            if let Some(client_id) = client_node.children().and_then(|c| {
                c.get("id")
                    .and_then(|c| c.entries().iter().next().and_then(|e| e.value().as_i64()))
            }) {
                let mut history = vec![];
                if let Some(history_entries) = client_node
                    .children()
                    .and_then(|c| c.get("history"))
                    .map(|h| h.entries())
                {
                    for entry in history_entries {
                        if let Some(entry) = entry.value().as_i64() {
                            history.push(
                                TabId::try_from(entry)
                                    .map_err(|_| "tab_id is outside usize range")?,
                            );
                        }
                    }
                }
                tab_history.insert(
                    ClientId::try_from(client_id).map_err(|_| "client_id is outside u32 range")?,
                    history,
                );
            }
        }
    }
    let mut pane_history = BTreeMap::new();
    if let Some(kdl_pane_history) = kdl_document.get("pane_history").and_then(|p| p.children()) {
        for client_node in kdl_pane_history.nodes() {
            if let Some(client_id) = client_node.children().and_then(|c| {
                c.get("id")
                    .and_then(|c| c.entries().iter().next().and_then(|e| e.value().as_i64()))
            }) {
                let mut history = vec![];
                if let Some(history_node) = client_node.children().and_then(|c| c.get("history")) {
                    if let Some(history_children) = history_node.children() {
                        for pane_id_node in history_children.nodes() {
                            if pane_id_node.name().value() == "pane_id" {
                                let pane_type = pane_id_node
                                    .entries()
                                    .iter()
                                    .find(|e| e.name().map(|n| n.value()) == Some("type"))
                                    .and_then(|e| e.value().as_string());
                                let id = pane_id_node
                                    .entries()
                                    .iter()
                                    .find(|e| e.name().is_none())
                                    .and_then(|e| e.value().as_i64())
                                    .map(|i| i as u32);
                                if let (Some(pane_type), Some(id)) = (pane_type, id) {
                                    let pane_id = match pane_type {
                                        "terminal" => Some(PaneId::Terminal(
                                            yinz_types::internal_ids::TerminalId::new(id),
                                        )),
                                        "plugin" => Some(PaneId::Plugin(
                                            yinz_types::internal_ids::PluginId::new(id),
                                        )),
                                        _ => None,
                                    };
                                    if let Some(pane_id) = pane_id {
                                        history.push(pane_id);
                                    }
                                }
                            }
                        }
                    }
                }
                pane_history.insert(
                    ClientId::try_from(client_id).map_err(|_| "client_id is outside u32 range")?,
                    history,
                );
            }
        }
    }
    let creation_time = kdl_document
        .get("creation_time")
        .and_then(|n| n.entries().iter().next())
        .and_then(|e| e.value().as_i64())
        .map(|c| Duration::from_secs(c as u64))
        .unwrap_or_default();
    Ok(SessionInfo {
        name: name.into(),
        tabs,
        panes,
        connected_clients,
        is_current_session,
        available_layouts,
        plugins: Default::default(), // we do not serialize plugin information
        tab_history,
        pane_history,
        creation_time,
    })
}
#[allow(clippy::inherent_to_string)]
pub fn to_string(value: &SessionInfo) -> anyhow::Result<String> {
    let mut kdl_document = KdlDocument::new();

    let mut name = KdlNode::new("name");
    name.push(value.name.as_str());

    let mut connected_clients = KdlNode::new("connected_clients");
    connected_clients.push(value.connected_clients as i64);

    let mut tabs = KdlNode::new("tabs");
    let mut tab_children = KdlDocument::new();
    for tab_info in &value.tabs {
        let mut tab = KdlNode::new("tab");
        let kdl_tab_info = tab_info::encode_to_kdl(tab_info)?;
        tab.set_children(kdl_tab_info);
        tab_children.nodes_mut().push(tab);
    }
    tabs.set_children(tab_children);

    let mut panes = KdlNode::new("panes");
    panes.set_children(pane_manifest::encode_to_kdl(&value.panes)?);

    let mut available_layouts = KdlNode::new("available_layouts");
    let mut available_layouts_children = KdlDocument::new();
    for layout_info in &value.available_layouts {
        let (layout_name, layout_source) = match layout_info {
            LayoutInfo::File(name, _layout_metadata) => (name.clone(), "file"),
            LayoutInfo::BuiltIn(name) => (name.clone(), "built-in"),
            LayoutInfo::Stringified(_stringified) => ("stringified-layout".to_owned(), "N/A"),
        };
        let mut layout_node = KdlNode::new(layout_name.to_string());
        let layout_source = KdlEntry::new_prop("source", layout_source);
        layout_node.entries_mut().push(layout_source);
        available_layouts_children.nodes_mut().push(layout_node);
    }
    available_layouts.set_children(available_layouts_children);

    let mut tab_history = KdlNode::new("tab_history");
    let mut tab_history_children = KdlDocument::new();
    for (client_id, client_tab_history) in &value.tab_history {
        let mut client_document = KdlDocument::new();
        let mut client_node = KdlNode::new("client");
        let mut id = KdlNode::new("id");
        id.push(i64::from(client_id.get()));
        client_document.nodes_mut().push(id);
        let mut history = KdlNode::new("history");
        for entry in client_tab_history {
            history.push(i64::try_from(entry.get())?);
        }
        client_document.nodes_mut().push(history);
        client_node.set_children(client_document);
        tab_history_children.nodes_mut().push(client_node);
    }
    tab_history.set_children(tab_history_children);

    let mut pane_history = KdlNode::new("pane_history");
    let mut pane_history_children = KdlDocument::new();
    for (client_id, client_pane_history) in &value.pane_history {
        let mut client_document = KdlDocument::new();
        let mut client_node = KdlNode::new("client");
        let mut id = KdlNode::new("id");
        id.push(i64::from(client_id.get()));
        client_document.nodes_mut().push(id);
        let mut history = KdlNode::new("history");
        for pane_id in client_pane_history {
            let mut pane_id_node = KdlNode::new("pane_id");
            match pane_id {
                PaneId::Terminal(id) => {
                    pane_id_node.push(KdlEntry::new_prop("type", "terminal"));
                    pane_id_node.push(i64::from(id.get()));
                },
                PaneId::Plugin(id) => {
                    pane_id_node.push(KdlEntry::new_prop("type", "plugin"));
                    pane_id_node.push(i64::from(id.get()));
                },
            }
            history.ensure_children().nodes_mut().push(pane_id_node);
        }
        client_document.nodes_mut().push(history);
        client_node.set_children(client_document);
        pane_history_children.nodes_mut().push(client_node);
    }
    pane_history.set_children(pane_history_children);

    kdl_document.nodes_mut().push(name);
    kdl_document.nodes_mut().push(tabs);
    kdl_document.nodes_mut().push(panes);
    kdl_document.nodes_mut().push(connected_clients);
    kdl_document.nodes_mut().push(available_layouts);
    kdl_document.nodes_mut().push(tab_history);
    kdl_document.nodes_mut().push(pane_history);

    let mut creation_time_node = KdlNode::new("creation_time");
    creation_time_node.push(value.creation_time.as_secs() as i64);
    kdl_document.nodes_mut().push(creation_time_node);

    kdl_document.fmt();
    Ok(kdl_document.to_string())
}
