//! Project layouts describe geometry only and never start processes.
use kdl::{KdlDocument, KdlNode};
use yinz_types::input::layout::TiledPaneLayout;

pub fn parse(source: &str) -> Result<TiledPaneLayout, String> {
    let document: KdlDocument = source.parse().map_err(|error| format!("{error}"))?;
    if document.nodes().len() != 1 || document.nodes()[0].name().value() != "layout" {
        return Err("expected a single layout block".into());
    }
    fn validate(node: &KdlNode, root: bool) -> Result<(), String> {
        if !root && node.name().value() != "pane" {
            return Err(format!(
                "{} is not allowed in a project layout",
                node.name().value()
            ));
        }
        for entry in node.entries() {
            let name = entry.name().map(|name| name.value()).unwrap_or("");
            if root || !matches!(name, "split_direction" | "size" | "stacked" | "expanded") {
                return Err(format!("{name:?} is not allowed in a project layout"));
            }
        }
        if let Some(children) = node.children() {
            for child in children.nodes() {
                let single_tab = node.name().value() == "layout"
                    && children.nodes().len() == 1
                    && child.name().value() == "tab";
                validate(child, single_tab)?;
            }
        }
        Ok(())
    }
    validate(&document.nodes()[0], true)?;
    let layout = crate::kdl::layout::from_kdl(source, None, None, None)
        .map_err(|error| error.to_string())?;
    Ok(layout
        .tabs
        .first()
        .map(|(_, tiled, _)| tiled.clone())
        .unwrap_or_else(|| layout.new_tab().0))
}

#[cfg(test)]
#[path = "unit/project_layout_tests.rs"]
mod tests;
