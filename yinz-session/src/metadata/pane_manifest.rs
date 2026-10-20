use super::*;
use yinz_types::internal_ids::TabPosition;

pub fn decode_from_kdl(kdl_doucment: &KdlDocument) -> PaneManifest {
    let mut panes: HashMap<TabPosition, Vec<PaneInfo>> = HashMap::new();
    for node in kdl_doucment.nodes() {
        if node.name().to_string() == "pane"
            && let Some(pane_document) = node.children()
            && let Ok((Some(tab_position), pane_info)) =
                crate::metadata::pane_info::decode_from_kdl(pane_document)
        {
            let panes_in_tab_position = panes.entry(tab_position).or_default();
            panes_in_tab_position.push(pane_info);
        }
    }
    PaneManifest { panes }
}
pub fn encode_to_kdl(value: &PaneManifest) -> anyhow::Result<KdlDocument> {
    let mut kdl_doucment = KdlDocument::new();
    for (tab_position, panes) in &value.panes {
        for pane in panes {
            let mut pane_node = KdlNode::new("pane");
            let mut pane = pane_info::encode_to_kdl(pane)?;

            let mut position_node = KdlNode::new("tab_position");
            position_node.push(i64::try_from(tab_position.get())?);
            pane.nodes_mut().push(position_node);

            pane_node.set_children(pane);
            kdl_doucment.nodes_mut().push(pane_node);
        }
    }
    Ok(kdl_doucment)
}
