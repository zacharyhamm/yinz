use super::*;
use crate::kdl_parsing_error;
use yinz_types::pane_tabs::PaneTabDescriptor;

pub(super) fn from_kdl(block: &KdlNode) -> Result<Vec<PaneTabDescriptor>, ConfigError> {
    if !block.entries().is_empty() {
        return Err(kdl_parsing_error!(
            "overlay_tab_plugins accepts only child entries".to_owned(),
            block
        ));
    }
    let children = block.children().ok_or_else(|| {
        kdl_parsing_error!("overlay_tab_plugins requires a block".to_owned(), block)
    })?;
    children
        .nodes()
        .iter()
        .map(|node| {
            if node.children().is_some()
                || node.entries().iter().any(|entry| {
                    entry.name().map(|name| name.value()) != Some("label")
                        || entry.value().as_string().is_none()
                })
            {
                return Err(kdl_parsing_error!(
                    "overlay entries accept only an optional string label".to_owned(),
                    node
                ));
            }
            let location = node.name().value().to_owned();
            let label = node
                .get("label")
                .and_then(|entry| entry.value().as_string())
                .unwrap_or(&location)
                .to_owned();
            if label.trim().is_empty() || location.trim().is_empty() {
                return Err(kdl_parsing_error!(
                    "overlay entries require a nonempty name and label".to_owned(),
                    node
                ));
            }
            Ok(PaneTabDescriptor {
                location,
                label,
                configuration: BTreeMap::new(),
                initial_cwd: None,
            })
        })
        .collect()
}

pub(super) fn to_kdl(tabs: &[PaneTabDescriptor]) -> KdlNode {
    let mut block = KdlNode::new("overlay_tab_plugins");
    let mut children = KdlDocument::new();
    for tab in tabs {
        let mut node = KdlNode::new(tab.location.clone());
        node.insert("label", tab.label.clone());
        children.nodes_mut().push(node);
    }
    block.set_children(children);
    block
}

impl Config {
    pub fn overlay_tab_plugin_declarations(&self) -> Vec<PaneTabDescriptor> {
        self.overlay_tab_plugins.clone().unwrap_or_else(|| {
            [("agents", "Agents")]
                .into_iter()
                .map(|(name, label)| PaneTabDescriptor {
                    location: if self.plugins.aliases.contains_key(name) {
                        name.into()
                    } else {
                        format!("yinz:{name}")
                    },
                    label: label.into(),
                    configuration: BTreeMap::new(),
                    initial_cwd: None,
                })
                .collect()
        })
    }

    pub fn resolved_overlay_tab_plugins(&self) -> Result<Vec<PaneTabDescriptor>, String> {
        let mut tabs = self.overlay_tab_plugin_declarations();
        let mut seen = BTreeSet::new();
        for tab in &mut tabs {
            let name = tab.location.clone();
            tab.populate_plugin_alias(&self.plugins);
            let run = yinz_runtime::plugin_location::run_plugin_from_url(&tab.location)
                .map_err(|error| format!("Invalid overlay_tab_plugins entry {name:?}: {error}"))?;
            if crate::plugins::PluginConfig::from_run_plugin(&run).is_none() {
                return Err(format!("Invalid overlay_tab_plugins plugin: {name}"));
            }
            if !seen.insert(run.location.clone()) {
                return Err(format!(
                    "Duplicate resolved plugin in overlay_tab_plugins: {name}"
                ));
            }
            tab.location = run.location_for_serialization();
        }
        Ok(tabs)
    }
}

#[cfg(test)]
#[path = "unit/overlay_tab_plugins.rs"]
mod tests;
