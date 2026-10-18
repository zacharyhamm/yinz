use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use yinz_types::command::PaneTarget;

use yinz_types::{
    geometry::{PercentOrFixed, SplitDirection, SplitSize},
    pane_size::Constraint,
};
use {
    yinz_types::input::layout::FloatingPaneLayout, yinz_types::input::layout::LayoutConstraint,
    yinz_types::input::layout::PluginUserConfiguration, yinz_types::input::layout::Run,
    yinz_types::input::layout::RunPluginOrAlias, yinz_types::input::layout::SwapFloatingLayout,
    yinz_types::input::layout::SwapTiledLayout, yinz_types::input::layout::TiledPaneLayout,
};

use yinz_types::session_manifest::{GlobalLayoutManifest, PaneLayoutManifest, TabLayoutManifest};

pub fn serialize_session_layout(
    global_layout_manifest: GlobalLayoutManifest,
) -> Result<(String, BTreeMap<String, String>), String> {
    // BTreeMap is the pane contents and their file names
    let mut document = KdlDocument::new();
    let mut pane_contents = BTreeMap::new();
    let mut layout_node = KdlNode::new("layout");
    let mut layout_node_children = KdlDocument::new();
    if let Some(global_cwd) = serialize_global_cwd(&global_layout_manifest.global_cwd) {
        layout_node_children.nodes_mut().push(global_cwd);
    }
    match serialize_multiple_tabs(global_layout_manifest.tabs, &mut pane_contents) {
        Ok(mut serialized_tabs) => {
            layout_node_children
                .nodes_mut()
                .append(&mut serialized_tabs);
        },
        Err(e) => {
            return Err(e.to_string());
        },
    }
    serialize_new_tab_template(
        global_layout_manifest.default_layout.template,
        &mut pane_contents,
        &mut layout_node_children,
    );
    serialize_swap_tiled_layouts(
        global_layout_manifest.default_layout.swap_tiled_layouts,
        &mut pane_contents,
        &mut layout_node_children,
    );
    serialize_swap_floating_layouts(
        global_layout_manifest.default_layout.swap_floating_layouts,
        &mut pane_contents,
        &mut layout_node_children,
    );

    layout_node.set_children(layout_node_children);
    document.nodes_mut().push(layout_node);
    Ok((document.to_string(), pane_contents))
}

fn serialize_tab(
    tab_name: String,
    is_focused: bool,
    hide_floating_panes: bool,
    user_named: bool,
    tiled_panes: &Vec<PaneLayoutManifest>,
    floating_panes: &[PaneLayoutManifest],
    pane_contents: &mut BTreeMap<String, String>,
) -> KdlNode {
    let mut serialized_tab = KdlNode::new("tab");
    let mut serialized_tab_children = KdlDocument::new();
    // Lossy: a tab whose arrangement is not a tree of splits still comes back
    // with every one of its panes, arranged as near as a layout gets. The
    // alternative is dropping the whole tab out of the serialized session.
    let tiled_panes_layout = tiled_layout_from_panegeoms_lossy(tiled_panes);
    let floating_panes_layout = get_floating_panes_layout_from_panegeoms(floating_panes);
    let tiled_panes = tiled_panes_to_serialize(tiled_panes_layout);
    serialized_tab
        .entries_mut()
        .push(KdlEntry::new_prop("name", tab_name));
    if is_focused {
        serialized_tab
            .entries_mut()
            .push(KdlEntry::new_prop("focus", KdlValue::Bool(true)));
    }
    if hide_floating_panes {
        serialized_tab.entries_mut().push(KdlEntry::new_prop(
            "hide_floating_panes",
            KdlValue::Bool(true),
        ));
    }
    if user_named {
        serialized_tab
            .entries_mut()
            .push(KdlEntry::new_prop("user_named", KdlValue::Bool(true)));
    }

    serialize_tiled_and_floating_panes(
        &tiled_panes,
        floating_panes_layout,
        pane_contents,
        &mut serialized_tab_children,
    );

    serialized_tab.set_children(serialized_tab_children);
    serialized_tab
}

/// The `pane` nodes a layout root is written as.
///
/// A root with children is written as those children, unless the split
/// direction or stacking lives on the root itself, in which case the root has
/// to be kept to carry them. A root that is itself a single pane has no
/// children to write, so it is the pane: dropping it wrote `tab { }` for any
/// tab with exactly one tiled pane, and the pane was gone on resurrection. A
/// root that is a bare default pane is left out, which is what a bare `tab`
/// already means.
fn tiled_panes_to_serialize(root: TiledPaneLayout) -> Vec<TiledPaneLayout> {
    let root_is_leaf = root.children.is_empty() && root.external_children_index.is_none();
    if root_is_leaf {
        if root == TiledPaneLayout::default() {
            vec![]
        } else {
            vec![root]
        }
    } else if root.children_split_direction != SplitDirection::default()
        || root.children_are_stacked
    {
        vec![root]
    } else {
        root.children
    }
}

fn serialize_tiled_and_floating_panes(
    tiled_panes: &Vec<TiledPaneLayout>,
    floating_panes_layout: Vec<FloatingPaneLayout>,
    pane_contents: &mut BTreeMap<String, String>,
    serialized_tab_children: &mut KdlDocument,
) {
    for tiled_pane_layout in tiled_panes {
        let ignore_size = false;
        let tiled_pane_node = serialize_tiled_pane(tiled_pane_layout, ignore_size, pane_contents);
        serialized_tab_children.nodes_mut().push(tiled_pane_node);
    }
    if !floating_panes_layout.is_empty() {
        let mut floating_panes_node = KdlNode::new("floating_panes");
        let mut floating_panes_node_children = KdlDocument::new();
        for floating_pane in floating_panes_layout {
            let pane_node = serialize_floating_pane(&floating_pane, pane_contents);
            floating_panes_node_children.nodes_mut().push(pane_node);
        }
        floating_panes_node.set_children(floating_panes_node_children);
        serialized_tab_children
            .nodes_mut()
            .push(floating_panes_node);
    }
}

fn serialize_tiled_pane(
    layout: &TiledPaneLayout,
    ignore_size: bool,
    pane_contents: &mut BTreeMap<String, String>,
) -> KdlNode {
    let (command, args) = extract_command_and_args(&layout.run);
    let (plugin, plugin_config) = extract_plugin_and_config(&layout.run);
    let (edit, line_number) = extract_edit_and_line_number(&layout.run);
    let cwd = layout.run.as_ref().and_then(|r| r.get_cwd());
    let has_children = layout.external_children_index.is_some() || !layout.children.is_empty();

    let mut tiled_pane_node = KdlNode::new("pane");
    serialize_pane_title_and_attributes(
        &command,
        &edit,
        line_number,
        &layout.name,
        cwd,
        layout.focus,
        &layout.pane_initial_contents,
        pane_contents,
        has_children,
        &mut tiled_pane_node,
    );

    serialize_target_attributes(&layout.run, &mut tiled_pane_node);
    serialize_tiled_layout_attributes(layout, ignore_size, &mut tiled_pane_node);
    if let Some(ref fg) = layout.default_fg {
        tiled_pane_node
            .entries_mut()
            .push(KdlEntry::new_prop("default_fg", fg.to_owned()));
    }
    if let Some(ref bg) = layout.default_bg {
        tiled_pane_node
            .entries_mut()
            .push(KdlEntry::new_prop("default_bg", bg.to_owned()));
    }
    let has_child_attributes = !layout.children.is_empty()
        || layout.external_children_index.is_some()
        || !args.is_empty()
        || plugin.is_some()
        || command.is_some();
    if has_child_attributes {
        let mut tiled_pane_node_children = KdlDocument::new();
        serialize_args(args, &mut tiled_pane_node_children);
        serialize_start_suspended(&command, &mut tiled_pane_node_children);
        serialize_plugin(plugin, plugin_config, &mut tiled_pane_node_children);
        if layout.children.is_empty() && layout.external_children_index.is_some() {
            tiled_pane_node_children
                .nodes_mut()
                .push(KdlNode::new("children"));
        }
        for (i, pane) in layout.children.iter().enumerate() {
            if Some(i) == layout.external_children_index {
                tiled_pane_node_children
                    .nodes_mut()
                    .push(KdlNode::new("children"));
            } else {
                let ignore_size = layout.children_are_stacked;
                let child_pane_node = serialize_tiled_pane(pane, ignore_size, pane_contents);
                tiled_pane_node_children.nodes_mut().push(child_pane_node);
            }
        }
        tiled_pane_node.set_children(tiled_pane_node_children);
    }
    if let Some(tabs) = &layout.pane_tabs {
        serialize_pane_tabs(tabs, &mut tiled_pane_node);
    }
    tiled_pane_node
}

/// Preserve the Bubblewrap environment when dumping a command pane.
fn serialize_target_attributes(layout_run: &Option<Run>, kdl_node: &mut KdlNode) {
    let Some(Run::Command(run_command)) = layout_run else {
        return;
    };
    if let PaneTarget::Bubblewrap { root } = &run_command.target {
        kdl_node
            .entries_mut()
            .push(KdlEntry::new_prop("bubblewrap", root.display().to_string()));
    }
}

pub fn extract_command_and_args(layout_run: &Option<Run>) -> (Option<String>, Vec<String>) {
    match layout_run {
        // A remote default shell has no explicit command to serialize.
        Some(Run::Command(run_command)) if run_command.is_remote_default_shell() => (None, vec![]),
        Some(Run::Command(run_command)) => (
            Some(run_command.command.display().to_string()),
            run_command.args.clone(),
        ),
        _ => (None, vec![]),
    }
}
pub fn extract_plugin_and_config(
    layout_run: &Option<Run>,
) -> (Option<String>, Option<PluginUserConfiguration>) {
    match &layout_run {
        Some(Run::Plugin(run_plugin_or_alias)) => match run_plugin_or_alias {
            RunPluginOrAlias::RunPlugin(run_plugin) => (
                Some(run_plugin.location_for_serialization()),
                Some(run_plugin.configuration.clone()),
            ),
            RunPluginOrAlias::Alias(plugin_alias) => {
                // in this case, the aliases should already be populated by the RunPlugins they
                // translate to - if they are not, the alias either does not exist or this is some
                // sort of bug
                let name = plugin_alias
                    .run_plugin
                    .as_ref()
                    .map(|run_plugin| run_plugin.location_for_serialization())
                    .unwrap_or_else(|| plugin_alias.name.clone());
                let configuration = plugin_alias
                    .run_plugin
                    .as_ref()
                    .map(|run_plugin| run_plugin.configuration.clone());
                (Some(name), configuration)
            },
        },
        _ => (None, None),
    }
}
pub fn extract_edit_and_line_number(layout_run: &Option<Run>) -> (Option<String>, Option<usize>) {
    match &layout_run {
        Some(Run::EditFile(path, line_number, _cwd)) => {
            (Some(path.display().to_string()), *line_number)
        },
        _ => (None, None),
    }
}

#[allow(clippy::too_many_arguments)]
fn serialize_pane_title_and_attributes(
    command: &Option<String>,
    edit: &Option<String>,
    line_number: Option<usize>,
    name: &Option<String>,
    cwd: Option<PathBuf>,
    focus: Option<bool>,
    initial_pane_contents: &Option<String>,
    pane_contents: &mut BTreeMap<String, String>,
    has_children: bool,
    kdl_node: &mut KdlNode,
) {
    match (&command, &edit) {
        (Some(command), _) => kdl_node
            .entries_mut()
            .push(KdlEntry::new_prop("command", command.to_owned())),
        (None, Some(edit)) => {
            kdl_node
                .entries_mut()
                .push(KdlEntry::new_prop("edit", edit.to_owned()));
            // KDL integers are signed. An out-of-range editor position must not make the
            // entire session layout unreadable.
            if let Some(line_number) = line_number.and_then(|line| i64::try_from(line).ok()) {
                kdl_node
                    .entries_mut()
                    .push(KdlEntry::new_prop("line_number", line_number));
            }
        },
        _ => {},
    };
    if let Some(name) = name {
        kdl_node
            .entries_mut()
            .push(KdlEntry::new_prop("name", name.to_owned()));
    }
    if let Some(cwd) = cwd {
        let path = cwd.display().to_string();
        if !path.is_empty() && !has_children {
            kdl_node
                .entries_mut()
                .push(KdlEntry::new_prop("cwd", path.to_owned()));
        }
    }
    if focus.unwrap_or(false) {
        kdl_node
            .entries_mut()
            .push(KdlEntry::new_prop("focus", KdlValue::Bool(true)));
    }
    if let Some(initial_pane_contents) = initial_pane_contents.as_ref() {
        if command.is_none() && edit.is_none() {
            let file_name = format!("initial_contents_{}", pane_contents.keys().len() + 1);
            kdl_node
                .entries_mut()
                .push(KdlEntry::new_prop("contents_file", file_name.clone()));

            pane_contents.insert(file_name, initial_pane_contents.clone());
        }
    }
}

fn serialize_args(args: Vec<String>, pane_node_children: &mut KdlDocument) {
    if !args.is_empty() {
        let mut args_node = KdlNode::new("args");
        for arg in &args {
            args_node.entries_mut().push(KdlEntry::new(arg.to_owned()));
        }
        pane_node_children.nodes_mut().push(args_node);
    }
}

fn serialize_plugin(
    plugin: Option<String>,
    plugin_config: Option<PluginUserConfiguration>,
    pane_node_children: &mut KdlDocument,
) {
    if let Some(plugin) = plugin {
        let mut plugin_node = KdlNode::new("plugin");
        plugin_node
            .entries_mut()
            .push(KdlEntry::new_prop("location", plugin.to_owned()));
        if let Some(plugin_config) = plugin_config.filter(|p| !p.inner().is_empty()) {
            let mut plugin_node_children = KdlDocument::new();
            for (config_key, config_value) in plugin_config.inner() {
                let mut config_node = KdlNode::new(config_key.to_owned());
                config_node
                    .entries_mut()
                    .push(KdlEntry::new(config_value.to_owned()));
                plugin_node_children.nodes_mut().push(config_node);
            }
            plugin_node.set_children(plugin_node_children);
        }
        pane_node_children.nodes_mut().push(plugin_node);
    }
}

fn serialize_tiled_layout_attributes(
    layout: &TiledPaneLayout,
    ignore_size: bool,
    kdl_node: &mut KdlNode,
) {
    if !ignore_size {
        match layout.split_size {
            Some(SplitSize::Fixed(size)) => kdl_node
                .entries_mut()
                .push(KdlEntry::new_prop("size", KdlValue::Base10(size as i64))),
            Some(SplitSize::Percent(size)) => kdl_node
                .entries_mut()
                .push(KdlEntry::new_prop("size", format!("{size}%"))),
            None => (),
        };
    }
    if layout.borderless.unwrap_or(false) {
        kdl_node
            .entries_mut()
            .push(KdlEntry::new_prop("borderless", KdlValue::Bool(true)));
    }
    if layout.children_are_stacked {
        kdl_node
            .entries_mut()
            .push(KdlEntry::new_prop("stacked", KdlValue::Bool(true)));
    }
    if layout.is_expanded_in_stack {
        kdl_node
            .entries_mut()
            .push(KdlEntry::new_prop("expanded", KdlValue::Bool(true)));
    }
    if layout.children_split_direction != SplitDirection::default() {
        let direction = match layout.children_split_direction {
            SplitDirection::Horizontal => "horizontal",
            SplitDirection::Vertical => "vertical",
        };
        kdl_node
            .entries_mut()
            .push(KdlEntry::new_prop("split_direction", direction));
    }
}

fn serialize_floating_layout_attributes(
    layout: &FloatingPaneLayout,
    pane_node_children: &mut KdlDocument,
) {
    match layout.height {
        Some(PercentOrFixed::Fixed(fixed_height)) => {
            let mut node = KdlNode::new("height");
            node.entries_mut()
                .push(KdlEntry::new(KdlValue::Base10(fixed_height as i64)));
            pane_node_children.nodes_mut().push(node);
        },
        Some(PercentOrFixed::Percent(percent)) => {
            let mut node = KdlNode::new("height");
            node.entries_mut()
                .push(KdlEntry::new(format!("{}%", percent)));
            pane_node_children.nodes_mut().push(node);
        },
        None => {},
    }
    match layout.width {
        Some(PercentOrFixed::Fixed(fixed_width)) => {
            let mut node = KdlNode::new("width");
            node.entries_mut()
                .push(KdlEntry::new(KdlValue::Base10(fixed_width as i64)));
            pane_node_children.nodes_mut().push(node);
        },
        Some(PercentOrFixed::Percent(percent)) => {
            let mut node = KdlNode::new("width");
            node.entries_mut()
                .push(KdlEntry::new(format!("{}%", percent)));
            pane_node_children.nodes_mut().push(node);
        },
        None => {},
    }
    match layout.x {
        Some(PercentOrFixed::Fixed(fixed_x)) => {
            let mut node = KdlNode::new("x");
            node.entries_mut()
                .push(KdlEntry::new(KdlValue::Base10(fixed_x as i64)));
            pane_node_children.nodes_mut().push(node);
        },
        Some(PercentOrFixed::Percent(percent)) => {
            let mut node = KdlNode::new("x");
            node.entries_mut()
                .push(KdlEntry::new(format!("{}%", percent)));
            pane_node_children.nodes_mut().push(node);
        },
        None => {},
    }
    match layout.y {
        Some(PercentOrFixed::Fixed(fixed_y)) => {
            let mut node = KdlNode::new("y");
            node.entries_mut()
                .push(KdlEntry::new(KdlValue::Base10(fixed_y as i64)));
            pane_node_children.nodes_mut().push(node);
        },
        Some(PercentOrFixed::Percent(percent)) => {
            let mut node = KdlNode::new("y");
            node.entries_mut()
                .push(KdlEntry::new(format!("{}%", percent)));
            pane_node_children.nodes_mut().push(node);
        },
        None => {},
    }
    if let Some(true) = layout.pinned {
        let mut node = KdlNode::new("pinned");
        node.entries_mut().push(KdlEntry::new(KdlValue::Bool(true)));
        pane_node_children.nodes_mut().push(node);
    }
}

fn serialize_start_suspended(command: &Option<String>, pane_node_children: &mut KdlDocument) {
    if command.is_some() {
        let mut start_suspended_node = KdlNode::new("start_suspended");
        start_suspended_node
            .entries_mut()
            .push(KdlEntry::new(KdlValue::Bool(true)));
        pane_node_children.nodes_mut().push(start_suspended_node);
    }
}

fn serialize_global_cwd(global_cwd: &Option<PathBuf>) -> Option<KdlNode> {
    global_cwd.as_ref().map(|cwd| {
        let mut node = KdlNode::new("cwd");
        node.push(cwd.display().to_string());
        node
    })
}

fn serialize_new_tab_template(
    new_tab_template: Option<(TiledPaneLayout, Vec<FloatingPaneLayout>)>,
    pane_contents: &mut BTreeMap<String, String>,
    layout_children_node: &mut KdlDocument,
) {
    if let Some((tiled_panes, floating_panes)) = new_tab_template {
        let tiled_panes = tiled_panes_to_serialize(tiled_panes);
        let mut new_tab_template_node = KdlNode::new("new_tab_template");
        let mut new_tab_template_children = KdlDocument::new();

        serialize_tiled_and_floating_panes(
            &tiled_panes,
            floating_panes,
            pane_contents,
            &mut new_tab_template_children,
        );
        new_tab_template_node.set_children(new_tab_template_children);
        layout_children_node.nodes_mut().push(new_tab_template_node);
    }
}

fn serialize_swap_tiled_layouts(
    swap_tiled_layouts: Vec<SwapTiledLayout>,
    pane_contents: &mut BTreeMap<String, String>,
    layout_node_children: &mut KdlDocument,
) {
    for swap_tiled_layout in swap_tiled_layouts {
        let mut swap_tiled_layout_node = KdlNode::new("swap_tiled_layout");
        let mut swap_tiled_layout_node_children = KdlDocument::new();
        let swap_tiled_layout_name = swap_tiled_layout.1;
        if let Some(name) = swap_tiled_layout_name {
            swap_tiled_layout_node
                .entries_mut()
                .push(KdlEntry::new_prop("name", name.to_owned()));
        }

        for (layout_constraint, tiled_panes_layout) in swap_tiled_layout.0 {
            let tiled_panes_layout = tiled_panes_to_serialize(tiled_panes_layout);
            let mut layout_step_node = KdlNode::new("tab");
            let mut layout_step_node_children = KdlDocument::new();
            if let Some(layout_constraint_entry) = serialize_layout_constraint(layout_constraint) {
                layout_step_node.entries_mut().push(layout_constraint_entry);
            }

            serialize_tiled_and_floating_panes(
                &tiled_panes_layout,
                vec![],
                pane_contents,
                &mut layout_step_node_children,
            );
            layout_step_node.set_children(layout_step_node_children);
            swap_tiled_layout_node_children
                .nodes_mut()
                .push(layout_step_node);
        }
        swap_tiled_layout_node.set_children(swap_tiled_layout_node_children);
        layout_node_children
            .nodes_mut()
            .push(swap_tiled_layout_node);
    }
}

fn serialize_layout_constraint(layout_constraint: LayoutConstraint) -> Option<KdlEntry> {
    match layout_constraint {
        LayoutConstraint::MaxPanes(max_panes) => Some(KdlEntry::new_prop(
            "max_panes",
            KdlValue::Base10(max_panes as i64),
        )),
        LayoutConstraint::MinPanes(min_panes) => Some(KdlEntry::new_prop(
            "min_panes",
            KdlValue::Base10(min_panes as i64),
        )),
        LayoutConstraint::ExactPanes(exact_panes) => Some(KdlEntry::new_prop(
            "exact_panes",
            KdlValue::Base10(exact_panes as i64),
        )),
        LayoutConstraint::NoConstraint => None,
    }
}

fn serialize_swap_floating_layouts(
    swap_floating_layouts: Vec<SwapFloatingLayout>,
    pane_contents: &mut BTreeMap<String, String>,
    layout_children_node: &mut KdlDocument,
) {
    for swap_floating_layout in swap_floating_layouts {
        let mut swap_floating_layout_node = KdlNode::new("swap_floating_layout");
        let mut swap_floating_layout_node_children = KdlDocument::new();
        let swap_floating_layout_name = swap_floating_layout.1;
        if let Some(name) = swap_floating_layout_name {
            swap_floating_layout_node
                .entries_mut()
                .push(KdlEntry::new_prop("name", name.to_owned()));
        }

        for (layout_constraint, floating_panes_layout) in swap_floating_layout.0 {
            let mut layout_step_node = KdlNode::new("floating_panes");
            let mut layout_step_node_children = KdlDocument::new();
            if let Some(layout_constraint_entry) = serialize_layout_constraint(layout_constraint) {
                layout_step_node.entries_mut().push(layout_constraint_entry);
            }

            for floating_pane_layout in floating_panes_layout {
                let floating_pane_node =
                    serialize_floating_pane(&floating_pane_layout, pane_contents);
                layout_step_node_children
                    .nodes_mut()
                    .push(floating_pane_node);
            }
            layout_step_node.set_children(layout_step_node_children);
            swap_floating_layout_node_children
                .nodes_mut()
                .push(layout_step_node);
        }
        swap_floating_layout_node.set_children(swap_floating_layout_node_children);
        layout_children_node
            .nodes_mut()
            .push(swap_floating_layout_node);
    }
}

fn serialize_multiple_tabs(
    tabs: Vec<(String, TabLayoutManifest)>,
    pane_contents: &mut BTreeMap<String, String>,
) -> Result<Vec<KdlNode>, &'static str> {
    let mut serialized_tabs: Vec<KdlNode> = vec![];
    for (tab_name, tab_layout_manifest) in tabs {
        let tiled_panes = tab_layout_manifest.tiled_panes;
        let floating_panes = tab_layout_manifest.floating_panes;
        let hide_floating_panes = tab_layout_manifest.hide_floating_panes;
        // Every tab serializes: one whose arrangement is not a tree of splits is
        // approximated rather than dropped, so there is no longer a way for a tab
        // to fall out of the session it belongs to.
        let mut serialized_tab = serialize_tab(
            tab_name.clone(),
            tab_layout_manifest.is_focused,
            hide_floating_panes,
            tab_layout_manifest.user_named,
            &tiled_panes,
            &floating_panes,
            pane_contents,
        );
        if let Some(group) = tab_layout_manifest.tab_group {
            serialized_tab
                .entries_mut()
                .push(KdlEntry::new_prop("group_id", i64::from(group.id)));
            serialized_tab
                .entries_mut()
                .push(KdlEntry::new_prop("group_name", group.name));
            if tab_layout_manifest.group_focused {
                serialized_tab
                    .entries_mut()
                    .push(KdlEntry::new_prop("group_focused", true));
            }
        }
        serialized_tabs.push(serialized_tab);
    }
    Ok(serialized_tabs)
}

fn serialize_floating_pane(
    layout: &FloatingPaneLayout,
    pane_contents: &mut BTreeMap<String, String>,
) -> KdlNode {
    let mut floating_pane_node = KdlNode::new("pane");
    let mut floating_pane_node_children = KdlDocument::new();
    let (command, args) = extract_command_and_args(&layout.run);
    let (plugin, plugin_config) = extract_plugin_and_config(&layout.run);
    let (edit, line_number) = extract_edit_and_line_number(&layout.run);
    let cwd = layout.run.as_ref().and_then(|r| r.get_cwd());
    let has_children = false;
    serialize_pane_title_and_attributes(
        &command,
        &edit,
        line_number,
        &layout.name,
        cwd,
        layout.focus,
        &layout.pane_initial_contents,
        pane_contents,
        has_children,
        &mut floating_pane_node,
    );
    serialize_target_attributes(&layout.run, &mut floating_pane_node);
    if let Some(ref fg) = layout.default_fg {
        floating_pane_node
            .entries_mut()
            .push(KdlEntry::new_prop("default_fg", fg.to_owned()));
    }
    if let Some(ref bg) = layout.default_bg {
        floating_pane_node
            .entries_mut()
            .push(KdlEntry::new_prop("default_bg", bg.to_owned()));
    }
    serialize_start_suspended(&command, &mut floating_pane_node_children);
    serialize_floating_layout_attributes(layout, &mut floating_pane_node_children);
    serialize_args(args, &mut floating_pane_node_children);
    serialize_plugin(plugin, plugin_config, &mut floating_pane_node_children);
    floating_pane_node.set_children(floating_pane_node_children);
    if let Some(tabs) = &layout.pane_tabs {
        serialize_pane_tabs(tabs, &mut floating_pane_node);
    }
    floating_pane_node
}

fn stack_layout_from_manifest(
    geoms: &Vec<PaneLayoutManifest>,
    split_size: Option<SplitSize>,
) -> Option<TiledPaneLayout> {
    let mut children_stacks: HashMap<usize, Vec<PaneLayoutManifest>> = HashMap::new();
    for p in geoms {
        // A pane that is not in a stack has no place in a stack layout, and
        // dropping it here would produce a layout describing fewer panes than
        // there are - which is silent: whoever gets the layout has no way to tell
        // a pane was left out of it. This is only reached when no split was found
        // anywhere, so the arrangement is not one this can express at all.
        let stack_id = p.geom.stacked?;
        children_stacks.entry(stack_id).or_default().push(p.clone());
    }
    if children_stacks.is_empty() {
        return None;
    }
    let mut stack_nodes = vec![];
    for (_stack_id, mut stacked_panes) in children_stacks.into_iter() {
        stacked_panes.sort_by_key(|p| p.geom.y);
        stack_nodes.push(TiledPaneLayout {
            split_size,
            children: stacked_panes
                .iter()
                .map(|p| tiled_pane_layout_from_manifest(Some(p), None))
                .collect(),
            children_are_stacked: true,
            ..Default::default()
        })
    }
    if stack_nodes.len() == 1 {
        // if there's only one stack, we return it without a wrapper
        stack_nodes.first().cloned()
    } else {
        // here there is more than one stack, so we wrap it in a logical container node
        Some(TiledPaneLayout {
            split_size,
            children: stack_nodes,
            ..Default::default()
        })
    }
}

fn tiled_pane_layout_from_manifest(
    manifest: Option<&PaneLayoutManifest>,
    split_size: Option<SplitSize>,
) -> TiledPaneLayout {
    let (
        run,
        borderless,
        is_expanded_in_stack,
        name,
        focus,
        pane_initial_contents,
        default_fg,
        default_bg,
    ) = manifest
        .map(|g| {
            let mut run = g.run.clone();
            if let Some(cwd) = &g.cwd {
                if let Some(run) = run.as_mut() {
                    run.add_cwd(cwd);
                } else {
                    run = Some(Run::Cwd(cwd.clone()));
                }
            }
            (
                run,
                Some(g.is_borderless),
                g.geom.is_stacked() && g.geom.rows.as_usize() > 1,
                g.title.clone(),
                Some(g.is_focused),
                g.pane_contents.clone(),
                g.default_fg.clone(),
                g.default_bg.clone(),
            )
        })
        .unwrap_or((None, None, false, None, None, None, None, None));
    TiledPaneLayout {
        pane_tabs: manifest.and_then(|m| m.pane_tabs.clone()),
        split_size,
        run,
        borderless,
        is_expanded_in_stack,
        name,
        focus,
        pane_initial_contents,
        default_fg,
        default_bg,
        ..Default::default()
    }
}

/// Tab-level parsing
pub(crate) fn get_tiled_panes_layout_from_panegeoms(
    geoms: &Vec<PaneLayoutManifest>,
    split_size: Option<SplitSize>,
) -> Option<TiledPaneLayout> {
    let (children_split_direction, splits) = match get_splits(geoms) {
        Some(x) => x,
        None => {
            if geoms.len() > 1 {
                // usually this means all geoms belong to one or more stacks,
                // since stack splits are discounted in the get_splits method -
                // but it also happens for an arrangement no layout can describe,
                // which `stack_layout_from_manifest` says `None` to rather than
                // describing something else
                return stack_layout_from_manifest(geoms, split_size);
            } else {
                return Some(tiled_pane_layout_from_manifest(
                    geoms.iter().next(),
                    split_size,
                ));
            }
        },
    };
    let mut children = Vec::new();
    let mut remaining_geoms = geoms.clone();
    let mut new_geoms = Vec::new();
    let mut new_constraints = Vec::new();
    for i in 1..splits.len() {
        let (v_min, v_max) = (splits[i - 1], splits[i]);
        let subgeoms: Vec<PaneLayoutManifest>;
        (subgeoms, remaining_geoms) = match children_split_direction {
            SplitDirection::Horizontal => remaining_geoms
                .clone()
                .into_iter()
                .partition(|g| g.geom.y + g.geom.rows.as_usize() <= v_max),
            SplitDirection::Vertical => remaining_geoms
                .clone()
                .into_iter()
                .partition(|g| g.geom.x + g.geom.cols.as_usize() <= v_max),
        };
        // An empty band would become a leaf built out of no manifest at all - a
        // pane in the layout that no pane in the tab corresponds to
        if subgeoms.is_empty() {
            return None;
        }
        match get_domain_constraint(&subgeoms, &children_split_direction, (v_min, v_max)) {
            Some(constraint) => {
                new_geoms.push(subgeoms);
                new_constraints.push(constraint);
            },
            None => {
                return None;
            },
        }
    }

    let new_split_sizes = get_split_sizes(&new_constraints);

    for (subgeoms, subsplit_size) in new_geoms.iter().zip(new_split_sizes) {
        match get_tiled_panes_layout_from_panegeoms(subgeoms, subsplit_size) {
            Some(child) => {
                children.push(child);
            },
            None => {
                return None;
            },
        }
    }
    let children_are_stacked = children_split_direction == SplitDirection::Horizontal
        && all_geoms_are_from_the_same_stack(&new_geoms);
    Some(TiledPaneLayout {
        children_split_direction,
        split_size,
        children,
        children_are_stacked,
        ..Default::default()
    })
}

/// A tab's arrangement as a layout: exactly when it can be one, and as close as
/// a layout gets when it cannot.
///
/// Not every arrangement a tab can be in is a layout. A layout is a tree of
/// splits, while `TiledPanes` keeps a flat map of panes to rectangles, and
/// nothing holds those rectangles to a shape a tree could have produced - a
/// even a rectangle. `get_tiled_panes_layout_from_panegeoms` says `None` to
/// those, which is the honest answer to "what layout is this tab", and a useless
/// one to anybody who has to carry the tab somewhere: refusing loses every pane
/// rather than the arrangement.
///
/// So the arrangement is approximated instead - see [`approximate_tiled_layout`].
/// There is always an answer: no panes at all comes back as the smallest layout
/// there is, which describes one empty pane.
pub(crate) fn tiled_layout_from_panegeoms_lossy(
    geoms: &Vec<PaneLayoutManifest>,
) -> TiledPaneLayout {
    get_tiled_panes_layout_from_panegeoms(geoms, None)
        .unwrap_or_else(|| approximate_tiled_layout(geoms, None))
}

/// The panes of one arrangement, grouped so that a stack stays one thing.
///
/// A stack is a single slot in the grid that happens to hold several panes, and
/// cutting between two of its members would arrive as two stacks. So the cuts
/// below are made between units, never inside one.
#[derive(Clone)]
struct ArrangementUnit {
    members: Vec<PaneLayoutManifest>,
    is_stack: bool,
    x: usize,
    y: usize,
    cols: usize,
    rows: usize,
}

impl ArrangementUnit {
    fn start(&self, direction: SplitDirection) -> usize {
        match direction {
            SplitDirection::Horizontal => self.y,
            SplitDirection::Vertical => self.x,
        }
    }
    fn end(&self, direction: SplitDirection) -> usize {
        match direction {
            SplitDirection::Horizontal => self.y + self.rows,
            SplitDirection::Vertical => self.x + self.cols,
        }
    }
    /// Doubled, so a unit spanning an odd number of cells still compares exactly.
    fn twice_center(&self, direction: SplitDirection) -> usize {
        self.start(direction) + self.end(direction)
    }
    fn into_layout(mut self, split_size: Option<SplitSize>) -> TiledPaneLayout {
        if !self.is_stack {
            return tiled_pane_layout_from_manifest(self.members.first(), split_size);
        }
        self.members.sort_by_key(|member| member.geom.y);
        TiledPaneLayout {
            split_size,
            children_split_direction: SplitDirection::Horizontal,
            children: self
                .members
                .iter()
                .map(|member| tiled_pane_layout_from_manifest(Some(member), None))
                .collect(),
            children_are_stacked: true,
            ..Default::default()
        }
    }
}

fn arrangement_units(geoms: &[PaneLayoutManifest]) -> Vec<ArrangementUnit> {
    let mut units: Vec<ArrangementUnit> = Vec::new();
    let mut stacks: HashMap<usize, usize> = HashMap::new(); // stack id -> unit index
    for manifest in geoms {
        let index = match manifest.geom.stacked {
            Some(stack_id) => *stacks.entry(stack_id).or_insert(units.len()),
            None => units.len(),
        };
        if index == units.len() {
            units.push(ArrangementUnit {
                members: vec![],
                is_stack: manifest.geom.stacked.is_some(),
                x: manifest.geom.x,
                y: manifest.geom.y,
                cols: 0,
                rows: 0,
            });
        }
        let unit = &mut units[index];
        // the union of the members' rectangles: a stack's members are meant to sit
        // one under another, but nothing here depends on their doing so
        let right = (unit.x + unit.cols).max(manifest.geom.x + manifest.geom.cols.as_usize());
        let bottom = (unit.y + unit.rows).max(manifest.geom.y + manifest.geom.rows.as_usize());
        unit.x = unit.x.min(manifest.geom.x);
        unit.y = unit.y.min(manifest.geom.y);
        unit.cols = right - unit.x;
        unit.rows = bottom - unit.y;
        unit.members.push(manifest.clone());
    }
    units
}

/// Describe an arrangement no layout can describe exactly, without losing a pane.
///
/// Total by construction: every manifest becomes exactly one leaf, whatever the
/// rectangles look like - overlapping, leaving gaps, or not adding up to a
/// rectangle at all. That is the property that matters, because the caller is
/// carrying live panes and a missing leaf is a stranded process.
///
/// The shape is the best guillotine cut of what is there: the split line, on
/// either axis, that the fewest panes straddle - and among those, the one that
/// divides them most evenly. A cut nothing straddles is a real split, so an
/// arrangement that *is* a tree apart from one wrinkle comes back with the tree
/// it had and the wrinkle straightened out. Panes that do straddle the line go
/// to the side their middle is on, which is the least wrong place for them.
fn approximate_tiled_layout(
    geoms: &[PaneLayoutManifest],
    split_size: Option<SplitSize>,
) -> TiledPaneLayout {
    layout_from_units(arrangement_units(geoms), split_size)
}

fn layout_from_units(
    mut units: Vec<ArrangementUnit>,
    split_size: Option<SplitSize>,
) -> TiledPaneLayout {
    if units.len() == 1 {
        return units.remove(0).into_layout(split_size);
    }
    if units.is_empty() {
        // no caller can reach this: `tiled_layout_from_panegeoms_lossy` refuses an
        // empty tab, and every cut below leaves both sides non-empty
        return TiledPaneLayout {
            split_size,
            ..Default::default()
        };
    }

    let (direction, mut near, mut far) = best_cut(&units);
    let extent = |group: &Vec<ArrangementUnit>| -> usize {
        let start = group
            .iter()
            .map(|unit| unit.start(direction))
            .min()
            .unwrap_or(0);
        let end = group
            .iter()
            .map(|unit| unit.end(direction))
            .max()
            .unwrap_or(0);
        end.saturating_sub(start)
    };
    let (near_extent, far_extent) = (extent(&near), extent(&far));
    // A side that was asked for in cells is asked for in cells again: a one-row
    // status strip is 2% of a 46-row tab, and 2% of a smaller one rounds away to
    // nothing. `get_split_sizes` makes the same distinction from the constraints.
    let percent = |part: usize| -> Option<SplitSize> {
        let whole = near_extent + far_extent;
        if whole == 0 {
            return Some(SplitSize::Percent(50));
        }
        Some(SplitSize::Percent((part * 100 / whole).clamp(1, 99)))
    };
    let (near_fixed, far_fixed) = (is_fixed(&near, direction), is_fixed(&far, direction));
    let (near_size, far_size) = match (near_fixed, far_fixed) {
        (true, true) => (
            Some(SplitSize::Fixed(near_extent)),
            Some(SplitSize::Fixed(far_extent)),
        ),
        // the side that is not fixed is left implicit, and takes whatever the
        // fixed one does not
        (true, false) => (Some(SplitSize::Fixed(near_extent)), None),
        (false, true) => (None, Some(SplitSize::Fixed(far_extent))),
        // neither is fixed: the larger one is left implicit, the way
        // `get_split_sizes` leaves the largest constraint unsized
        (false, false) if near_extent > far_extent => (None, percent(far_extent)),
        (false, false) => (percent(near_extent), None),
    };

    // sorted so that the panes arrive in the order they were in, which is what
    // the leaves' order in the tree means to whoever positions them
    near.sort_by_key(|unit| (unit.y, unit.x));
    far.sort_by_key(|unit| (unit.y, unit.x));
    TiledPaneLayout {
        split_size,
        children_split_direction: direction,
        children: vec![
            layout_from_units(near, near_size),
            layout_from_units(far, far_size),
        ],
        ..Default::default()
    }
}

/// Whether a whole side was measured in cells rather than as a share of the tab.
///
/// Asked of every pane in it, not of the side as a whole: one percentage
/// anywhere and the side has to grow with the tab, or that pane's share of it
/// would be measured against a fixed number of rows.
fn is_fixed(units: &[ArrangementUnit], direction: SplitDirection) -> bool {
    units.iter().flat_map(|unit| unit.members.iter()).all(|m| {
        match direction {
            SplitDirection::Horizontal => m.geom.rows,
            SplitDirection::Vertical => m.geom.cols,
        }
        .as_percent()
        .is_none()
    })
}

/// Split the units in two: the least bad guillotine cut, on either axis.
///
/// Always returns two non-empty groups - with two or more units there is always
/// a cut with one of them on each side, if only the one that halves them by
/// position - so the recursion above always gets smaller and always terminates.
fn best_cut(
    units: &[ArrangementUnit],
) -> (SplitDirection, Vec<ArrangementUnit>, Vec<ArrangementUnit>) {
    let mut best: Option<(usize, usize, SplitDirection, usize)> = None; // straddlers, imbalance, direction, cut
    for direction in [SplitDirection::Horizontal, SplitDirection::Vertical] {
        let mut candidates: Vec<usize> = units
            .iter()
            .flat_map(|unit| [unit.start(direction), unit.end(direction)])
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        for cut in candidates {
            let straddlers = units
                .iter()
                .filter(|unit| unit.start(direction) < cut && unit.end(direction) > cut)
                .count();
            let (near, far) = split_units_at(units, direction, cut);
            if near.is_empty() || far.is_empty() {
                continue;
            }
            let imbalance = near.len().abs_diff(far.len());
            if best
                .map(|(best_straddlers, best_imbalance, _, _)| {
                    (straddlers, imbalance) < (best_straddlers, best_imbalance)
                })
                .unwrap_or(true)
            {
                best = Some((straddlers, imbalance, direction, cut));
            }
        }
    }

    if let Some((_, _, direction, cut)) = best {
        let (near, far) = split_units_at(units, direction, cut);
        return (direction, near, far);
    }

    // Every unit begins and ends in the same place on both axes - panes lying
    // exactly on top of each other, which is not an arrangement a tab is supposed
    // to be in but is one this still has to carry. Halved by position instead.
    let mut ordered = units.to_vec();
    ordered.sort_by_key(|unit| (unit.y, unit.x));
    let far = ordered.split_off(ordered.len() / 2);
    (SplitDirection::Horizontal, ordered, far)
}

/// Which side of `cut` each unit belongs on, straddlers going with their middle.
fn split_units_at(
    units: &[ArrangementUnit],
    direction: SplitDirection,
    cut: usize,
) -> (Vec<ArrangementUnit>, Vec<ArrangementUnit>) {
    let mut near = vec![];
    let mut far = vec![];
    for unit in units {
        let goes_near = if unit.end(direction) <= cut {
            true
        } else if unit.start(direction) >= cut {
            false
        } else {
            unit.twice_center(direction) <= cut * 2
        };
        if goes_near {
            near.push(unit.clone());
        } else {
            far.push(unit.clone());
        }
    }
    (near, far)
}

fn all_geoms_are_from_the_same_stack(manifests: &Vec<Vec<PaneLayoutManifest>>) -> bool {
    let mut stack_ids = HashSet::new();
    for manifest_group in manifests {
        for pane_layout_manifest in manifest_group {
            stack_ids.insert(pane_layout_manifest.geom.stacked);
        }
    }
    stack_ids.len() == 1 && !stack_ids.contains(&None)
}

pub(crate) fn get_floating_panes_layout_from_panegeoms(
    manifests: &[PaneLayoutManifest],
) -> Vec<FloatingPaneLayout> {
    manifests
        .iter()
        .map(|m| {
            let mut run = m.run.clone();
            if let Some(cwd) = &m.cwd {
                // the same merge the tiled path does: a pane with nothing but a
                // cwd is a pane that should come back in that directory
                if let Some(r) = run.as_mut() {
                    r.add_cwd(cwd)
                } else {
                    run = Some(Run::Cwd(cwd.clone()));
                }
            }
            FloatingPaneLayout {
                pane_tabs: m.pane_tabs.clone(),
                name: m.title.clone(),
                height: Some(m.geom.rows.into()),
                width: Some(m.geom.cols.into()),
                x: Some(PercentOrFixed::Fixed(m.geom.x)),
                y: Some(PercentOrFixed::Fixed(m.geom.y)),
                pinned: Some(m.geom.is_pinned),
                run,
                focus: Some(m.is_focused),
                already_running: false,
                pane_initial_contents: m.pane_contents.clone(),
                logical_position: None,
                borderless: Some(m.is_borderless),
                default_fg: m.default_fg.clone(),
                default_bg: m.default_bg.clone(),
            }
        })
        .collect()
}

fn get_x_lims(geoms: &[PaneLayoutManifest]) -> Option<(usize, usize)> {
    match (
        geoms.iter().map(|g| g.geom.x).min(),
        geoms
            .iter()
            .map(|g| g.geom.x + g.geom.cols.as_usize())
            .max(),
    ) {
        (Some(x_min), Some(x_max)) => Some((x_min, x_max)),
        _ => None,
    }
}

fn get_y_lims(geoms: &[PaneLayoutManifest]) -> Option<(usize, usize)> {
    match (
        geoms.iter().map(|g| g.geom.y).min(),
        geoms
            .iter()
            .map(|g| g.geom.y + g.geom.rows.as_usize())
            .max(),
    ) {
        (Some(y_min), Some(y_max)) => Some((y_min, y_max)),
        _ => None,
    }
}

/// Returns the `SplitDirection` as well as the values, on the axis
/// perpendicular the `SplitDirection`, for which there is a split spanning
/// the max_cols or max_rows of the domain. The values are ordered
/// increasingly and contains the boundaries of the domain.
fn get_splits(geoms: &[PaneLayoutManifest]) -> Option<(SplitDirection, Vec<usize>)> {
    if geoms.len() == 1 {
        return None;
    }
    let (x_lims, y_lims) = match (get_x_lims(geoms), get_y_lims(geoms)) {
        (Some(x_lims), Some(y_lims)) => (x_lims, y_lims),
        _ => return None,
    };
    let mut direction = SplitDirection::default();
    let mut splits = match direction {
        SplitDirection::Vertical => get_col_splits(geoms, &x_lims, &y_lims),
        SplitDirection::Horizontal => get_row_splits(geoms, &x_lims, &y_lims),
    };
    if splits.len() <= 2 {
        // ie only the boundaries are present and no real split has been found
        direction = !direction;
        splits = match direction {
            SplitDirection::Vertical => get_col_splits(geoms, &x_lims, &y_lims),
            SplitDirection::Horizontal => get_row_splits(geoms, &x_lims, &y_lims),
        };
    }
    if splits.len() <= 2 {
        // ie no real split has been found in both directions
        None
    } else {
        Some((direction, splits))
    }
}

/// Returns a vector containing the abscisse (x) of the cols that split the
/// domain including the boundaries, ie the min and max abscisse values.
fn get_col_splits(
    geoms: &[PaneLayoutManifest],
    (_, x_max): &(usize, usize),
    (y_min, y_max): &(usize, usize),
) -> Vec<usize> {
    let max_rows = y_max - y_min;
    let mut splits = Vec::new();
    let mut sorted_geoms = geoms.to_vec();
    sorted_geoms.sort_by_key(|g| g.geom.x);
    for x in sorted_geoms.iter().map(|g| g.geom.x) {
        if splits.contains(&x) {
            continue;
        }
        if sorted_geoms
            .iter()
            .filter(|g| g.geom.x == x)
            .map(|g| g.geom.rows.as_usize())
            .sum::<usize>()
            == max_rows
        {
            splits.push(x);
        };
    }
    splits.push(*x_max); // Necessary as `g.x` is from the upper-left corner
    splits
}

/// Returns a vector containing the coordinate (y) of the rows that split the
/// domain including the boundaries, ie the min and max coordinate values.
fn get_row_splits(
    geoms: &[PaneLayoutManifest],
    (x_min, x_max): &(usize, usize),
    (_, y_max): &(usize, usize),
) -> Vec<usize> {
    let max_cols = x_max - x_min;
    let mut splits = Vec::new();
    let mut sorted_geoms = geoms.to_vec();
    sorted_geoms.sort_by_key(|g| g.geom.y);

    //  here we make sure the various panes in all the stacks aren't counted as splits, since
    //  stacked panes must always stay togethyer - we group them into one "geom" for the purposes
    //  of figuring out their splits
    let mut stack_geoms: HashMap<usize, Vec<PaneLayoutManifest>> = HashMap::new();
    let mut all_geoms = vec![];
    for pane_layout_manifest in sorted_geoms.drain(..) {
        if let Some(stack_id) = pane_layout_manifest.geom.stacked {
            stack_geoms
                .entry(stack_id)
                .or_default()
                .push(pane_layout_manifest)
        } else {
            all_geoms.push(pane_layout_manifest);
        }
    }
    for (_stack_id, mut geoms_in_stack) in stack_geoms.into_iter() {
        let mut geom_of_whole_stack = geoms_in_stack.remove(0);
        if let Some(last_geom) = geoms_in_stack.last() {
            geom_of_whole_stack
                .geom
                .rows
                .set_inner(last_geom.geom.y + last_geom.geom.rows.as_usize())
        }
        all_geoms.push(geom_of_whole_stack);
    }

    all_geoms.sort_by_key(|g| g.geom.y);

    for y in all_geoms.iter().map(|g| g.geom.y) {
        if splits.contains(&y) {
            continue;
        }
        if all_geoms
            .iter()
            .filter(|g| g.geom.y == y)
            .map(|g| g.geom.cols.as_usize())
            .sum::<usize>()
            == max_cols
        {
            splits.push(y);
        };
    }
    splits.push(*y_max); // Necessary as `g.y` is from the upper-left corner
    splits
}

/// Get the constraint of the domain considered, base on the rows or columns,
/// depending on the split direction provided.
fn get_domain_constraint(
    geoms: &[PaneLayoutManifest],
    split_direction: &SplitDirection,
    (v_min, v_max): (usize, usize),
) -> Option<Constraint> {
    match split_direction {
        SplitDirection::Horizontal => get_domain_row_constraint(geoms, (v_min, v_max)),
        SplitDirection::Vertical => get_domain_col_constraint(geoms, (v_min, v_max)),
    }
}

fn get_domain_col_constraint(
    geoms: &[PaneLayoutManifest],
    (x_min, x_max): (usize, usize),
) -> Option<Constraint> {
    let mut percent = 0.0;
    let mut x = x_min;
    while x != x_max {
        // we only look at one (ie the last) geom that has value `x` for `g.x`
        let geom = geoms.iter().rfind(|g| g.geom.x == x);
        match geom {
            Some(geom) => {
                // a zero-width pane would leave `x` where it is and walk this
                // loop forever
                if geom.geom.cols.as_usize() == 0 {
                    return None;
                }
                if let Some(size) = geom.geom.cols.as_percent() {
                    percent += size;
                }
                x += geom.geom.cols.as_usize();
            },
            None => {
                return None;
            },
        }
    }
    if percent == 0.0 {
        Some(Constraint::Fixed(x_max - x_min))
    } else {
        Some(Constraint::Percent(percent))
    }
}

fn get_domain_row_constraint(
    geoms: &[PaneLayoutManifest],
    (y_min, y_max): (usize, usize),
) -> Option<Constraint> {
    let mut percent = 0.0;
    let mut fixed = 0;
    let mut y = y_min;
    while y != y_max {
        // we only look at one (ie the last) geom that has value `y` for `g.y`
        let geom = geoms.iter().rfind(|g| g.geom.y == y);
        match geom {
            Some(geom) => {
                // a zero-height pane would leave `y` where it is and walk this
                // loop forever
                if geom.geom.rows.as_usize() == 0 {
                    return None;
                }
                if let Some(size) = geom.geom.rows.as_percent() {
                    percent += size;
                    fixed += geom.geom.rows.as_usize();
                } else if let Constraint::Fixed(size) = geom.geom.rows.constraint {
                    // Collapsed chrome keeps its configured height in the constraint.
                    fixed += size;
                }
                y += geom.geom.rows.as_usize();
            },
            None => {
                return None;
            },
        }
    }
    if percent == 0.0 {
        Some(Constraint::Fixed(fixed))
    } else {
        Some(Constraint::Percent(percent))
    }
}

/// Returns split sizes for all the children of a `TiledPaneLayout` based on
/// their constraints.
fn get_split_sizes(constraints: &Vec<Constraint>) -> Vec<Option<SplitSize>> {
    let mut split_sizes = Vec::new();
    let max_percent = constraints
        .iter()
        .filter_map(|c| match c {
            Constraint::Percent(size) => Some(size),
            _ => None,
        })
        .sum::<f64>();
    for constraint in constraints {
        let split_size = match constraint {
            Constraint::Fixed(size) => Some(SplitSize::Fixed(*size)),
            Constraint::Percent(size) => {
                if size == &max_percent {
                    None
                } else {
                    Some(SplitSize::Percent((100.0 * size / max_percent) as usize))
                }
            },
        };
        split_sizes.push(split_size);
    }
    split_sizes
}

fn serialize_pane_tabs(tabs: &yinz_types::pane_tabs::PaneTabsLayout, node: &mut KdlNode) {
    node.entries_mut().push(KdlEntry::new_prop("tabbed", true));
    node.entries_mut()
        .push(KdlEntry::new_prop("selected", tabs.active as i64));
    let mut children = node.children().cloned().unwrap_or_default();
    children
        .nodes_mut()
        .retain(|node| node.name().value() != "plugin");
    for tab in &tabs.tabs {
        let mut child = KdlNode::new("pane");
        child
            .entries_mut()
            .push(KdlEntry::new_prop("name", tab.label.clone()));
        let mut plugin = KdlNode::new("plugin");
        plugin
            .entries_mut()
            .push(KdlEntry::new_prop("location", tab.location.clone()));
        if let Some(cwd) = &tab.initial_cwd {
            plugin.entries_mut().push(KdlEntry::new_prop(
                "cwd",
                cwd.to_string_lossy().into_owned(),
            ));
        }
        if !tab.configuration.is_empty() {
            let mut config = KdlDocument::new();
            for (key, value) in &tab.configuration {
                let mut entry = KdlNode::new(key.clone());
                entry.entries_mut().push(KdlEntry::new(value.clone()));
                config.nodes_mut().push(entry);
            }
            plugin.set_children(config);
        }
        let mut content = KdlDocument::new();
        content.nodes_mut().push(plugin);
        child.set_children(content);
        children.nodes_mut().push(child);
    }
    node.set_children(children);
}

#[cfg(test)]
mod tests {

    use super::*;
    use expect_test::expect;
    use insta::assert_snapshot;
    use serde_json::Value;
    use std::collections::HashMap;
    use yinz_types::{
        input::layout::Layout,
        pane_size::{Dimension, PaneGeom},
    };
    const PANEGEOMS_JSON: &[&[&str]] = &[
        &[
            r#"{ "x": 0, "y": 1, "rows": { "constraint": "Percent(100.0)", "inner": 43 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
            r#"{ "x": 0, "y": 0, "rows": { "constraint": "Fixed(1)", "inner": 1 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
            r#"{ "x": 0, "y": 44, "rows": { "constraint": "Fixed(2)", "inner": 2 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
        ],
        &[
            r#"{ "x": 0, "y": 0, "rows": { "constraint": "Percent(100.0)", "inner": 26 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
            r#"{ "x": 0, "y": 26, "rows": { "constraint": "Fixed(20)", "inner": 20 }, "cols": { "constraint": "Fixed(50)", "inner": 50 }, "is_stacked": false }"#,
            r#"{ "x": 50, "y": 26, "rows": { "constraint": "Fixed(20)", "inner": 20 }, "cols": { "constraint": "Percent(100.0)", "inner": 161 }, "is_stacked": false }"#,
        ],
        &[
            r#"{ "x": 0, "y": 0, "rows": { "constraint": "Fixed(10)", "inner": 10 }, "cols": { "constraint": "Percent(50.0)", "inner": 106 }, "is_stacked": false }"#,
            r#"{ "x": 106, "y": 0, "rows": { "constraint": "Fixed(10)", "inner": 10 }, "cols": { "constraint": "Percent(50.0)", "inner": 105 }, "is_stacked": false }"#,
            r#"{ "x": 0, "y": 10, "rows": { "constraint": "Percent(100.0)", "inner": 26 }, "cols": { "constraint": "Fixed(40)", "inner": 40 }, "is_stacked": false }"#,
            r#"{ "x": 40, "y": 10, "rows": { "constraint": "Percent(100.0)", "inner": 26 }, "cols": { "constraint": "Percent(100.0)", "inner": 131 }, "is_stacked": false }"#,
            r#"{ "x": 171, "y": 10, "rows": { "constraint": "Percent(100.0)", "inner": 26 }, "cols": { "constraint": "Fixed(40)", "inner": 40 }, "is_stacked": false }"#,
            r#"{ "x": 0, "y": 36, "rows": { "constraint": "Fixed(10)", "inner": 10 }, "cols": { "constraint": "Percent(50.0)", "inner": 106 }, "is_stacked": false }"#,
            r#"{ "x": 106, "y": 36, "rows": { "constraint": "Fixed(10)", "inner": 10 }, "cols": { "constraint": "Percent(50.0)", "inner": 105 }, "is_stacked": false }"#,
        ],
        &[
            r#"{ "x": 0, "y": 0, "rows": { "constraint": "Percent(30.0)", "inner": 11 }, "cols": { "constraint": "Percent(35.0)", "inner": 74 }, "is_stacked": false }"#,
            r#"{ "x": 0, "y": 11, "rows": { "constraint": "Percent(30.0)", "inner": 11 }, "cols": { "constraint": "Percent(35.0)", "inner": 74 }, "is_stacked": false }"#,
            r#"{ "x": 0, "y": 22, "rows": { "constraint": "Percent(40.0)", "inner": 14 }, "cols": { "constraint": "Percent(35.0)", "inner": 74 }, "is_stacked": false }"#,
            r#"{ "x": 74, "y": 0, "rows": { "constraint": "Percent(100.0)", "inner": 36 }, "cols": { "constraint": "Percent(35.0)", "inner": 74 }, "is_stacked": false }"#,
            r#"{ "x": 0, "y": 36, "rows": { "constraint": "Fixed(10)", "inner": 10 }, "cols": { "constraint": "Percent(70.0)", "inner": 148 }, "is_stacked": false }"#,
            r#"{ "x": 148, "y": 0, "rows": { "constraint": "Percent(100.0)", "inner": 46 }, "cols": { "constraint": "Percent(30.0)", "inner": 63 }, "is_stacked": false }"#,
        ],
        &[
            r#"{ "x": 0, "y": 0, "rows": { "constraint": "Fixed(5)", "inner": 5 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
            r#"{ "x": 0, "y": 5, "rows": { "constraint": "Percent(100.0)", "inner": 36 }, "cols": { "constraint": "Fixed(20)", "inner": 20 }, "is_stacked": false }"#,
            r#"{ "x": 20, "y": 5, "rows": { "constraint": "Percent(100.0)", "inner": 36 }, "cols": { "constraint": "Percent(50.0)", "inner": 86 }, "is_stacked": false }"#,
            r#"{ "x": 106, "y": 5, "rows": { "constraint": "Percent(100.0)", "inner": 36 }, "cols": { "constraint": "Percent(50.0)", "inner": 85 }, "is_stacked": false }"#,
            r#"{ "x": 191, "y": 5, "rows": { "constraint": "Percent(100.0)", "inner": 36 }, "cols": { "constraint": "Fixed(20)", "inner": 20 }, "is_stacked": false }"#,
            r#"{ "x": 0, "y": 41, "rows": { "constraint": "Fixed(5)", "inner": 5 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
        ],
    ];

    #[test]
    fn editor_line_numbers_survive_tiled_and_floating_session_serialization() {
        for line_number in [Some(42), Some(0), Some(usize::MAX), None] {
            let run = Some(Run::EditFile("/tmp/notes.md".into(), line_number, None));
            let manifest = GlobalLayoutManifest {
                tabs: vec![(
                    "editors".to_owned(),
                    TabLayoutManifest {
                        tiled_panes: vec![PaneLayoutManifest {
                            run: run.clone(),
                            geom: PaneGeom {
                                rows: Dimension::fixed(20),
                                cols: Dimension::fixed(80),
                                ..Default::default()
                            },
                            ..Default::default()
                        }],
                        floating_panes: vec![PaneLayoutManifest {
                            run: run.clone(),
                            geom: PaneGeom {
                                rows: Dimension::fixed(10),
                                cols: Dimension::fixed(40),
                                ..Default::default()
                            },
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                )],
                ..Default::default()
            };
            let (raw, _) = serialize_session_layout(manifest).unwrap();
            let layout = yinz_config::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
            let (_, tiled, floating) = &layout.tabs()[0];
            let expected = Some(Run::EditFile(
                "/tmp/notes.md".into(),
                line_number.filter(|line| i64::try_from(*line).is_ok()),
                None,
            ));
            assert_eq!(tiled.children[0].run, expected);
            assert_eq!(floating[0].run, expected);
        }
    }

    #[test]
    fn geoms() {
        let geoms = PANEGEOMS_JSON[0]
            .iter()
            .map(|pg| parse_panegeom_from_json(pg))
            .map(|geom| PaneLayoutManifest {
                geom,
                ..Default::default()
            })
            .collect();
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: geoms,
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane size=1
                    pane
                    pane size=2
                }
            }
        "#]]
        .assert_eq(&kdl.0);

        let geoms = PANEGEOMS_JSON[1]
            .iter()
            .map(|pg| parse_panegeom_from_json(pg))
            .map(|geom| PaneLayoutManifest {
                geom,
                ..Default::default()
            })
            .collect();
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: geoms,
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane
                    pane size=20 split_direction="vertical" {
                        pane size=50
                        pane
                    }
                }
            }
        "#]]
        .assert_eq(&kdl.0);

        let geoms = PANEGEOMS_JSON[2]
            .iter()
            .map(|pg| parse_panegeom_from_json(pg))
            .map(|geom| PaneLayoutManifest {
                geom,
                ..Default::default()
            })
            .collect();
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: geoms,
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane size=10 split_direction="vertical" {
                        pane size="50%"
                        pane size="50%"
                    }
                    pane split_direction="vertical" {
                        pane size=40
                        pane
                        pane size=40
                    }
                    pane size=10 split_direction="vertical" {
                        pane size="50%"
                        pane size="50%"
                    }
                }
            }
        "#]]
        .assert_eq(&kdl.0);

        let geoms = PANEGEOMS_JSON[3]
            .iter()
            .map(|pg| parse_panegeom_from_json(pg))
            .map(|geom| PaneLayoutManifest {
                geom,
                ..Default::default()
            })
            .collect();
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: geoms,
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane split_direction="vertical" {
                        pane size="70%" {
                            pane split_direction="vertical" {
                                pane size="50%" {
                                    pane size="30%"
                                    pane size="30%"
                                    pane size="40%"
                                }
                                pane size="50%"
                            }
                            pane size=10
                        }
                        pane size="30%"
                    }
                }
            }
        "#]]
        .assert_eq(&kdl.0);

        let geoms = PANEGEOMS_JSON[4]
            .iter()
            .map(|pg| parse_panegeom_from_json(pg))
            .map(|geom| PaneLayoutManifest {
                geom,
                ..Default::default()
            })
            .collect();
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: geoms,
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane size=5
                    pane split_direction="vertical" {
                        pane size=20
                        pane size="50%"
                        pane size="50%"
                        pane size=20
                    }
                    pane size=5
                }
            }
        "#]]
        .assert_eq(&kdl.0);
    }

    #[test]
    fn global_cwd() {
        let global_layout_manifest = GlobalLayoutManifest {
            global_cwd: Some(PathBuf::from("/path/to/m\"y/global cwd")),
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }

    #[test]
    fn can_serialize_tab_name() {
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("my \"tab \\name".to_owned(), TabLayoutManifest::default())],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_tab_focus() {
        let tab_layout_manifest = TabLayoutManifest {
            is_focused: true,
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_tab_hide_floating_panes() {
        let tab_layout_manifest = TabLayoutManifest {
            hide_floating_panes: true,
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_tab_user_named() {
        let tab_layout_manifest = TabLayoutManifest {
            user_named: true,
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("notes".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_tab_with_tiled_panes() {
        use yinz_types::input::command::RunCommand;

        let mut plugin_configuration = BTreeMap::new();
        plugin_configuration.insert("key 1\"\\".to_owned(), "val 1\"\\".to_owned());
        plugin_configuration.insert("key 2\"\\".to_owned(), "val 2\"\\".to_owned());
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: vec![
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 0,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::Cwd(PathBuf::from("/tmp/\"my/cool cwd"))),
                    geom: PaneGeom {
                        x: 0,
                        y: 10,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::EditFile(
                        PathBuf::from("/tmp/\"my/cool cwd/my-file"),
                        None,
                        None,
                    )),
                    geom: PaneGeom {
                        x: 0,
                        y: 20,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::Command(RunCommand {
                        command: PathBuf::from("/tmp/\"my/cool cwd/command.sh"),
                        ..Default::default()
                    })),
                    geom: PaneGeom {
                        x: 0,
                        y: 30,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::Command(RunCommand {
                        command: PathBuf::from("/tmp/\"my/cool cwd/command.sh"),
                        args: vec![
                            "--arg1".to_owned(),
                            "arg\"2".to_owned(),
                            "arg > \\3".to_owned(),
                        ],
                        ..Default::default()
                    })),
                    geom: PaneGeom {
                        x: 0,
                        y: 40,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(
                        yinz_runtime::plugin_location::run_plugin_from_url(
                            "file:/tmp/\"my/cool cwd/plugin.scm",
                        )
                        .unwrap(),
                    ))),
                    geom: PaneGeom {
                        x: 0,
                        y: 50,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(
                        yinz_runtime::plugin_location::run_plugin_from_url(
                            "file:/tmp/\"my/cool cwd/plugin.scm",
                        )
                        .unwrap()
                        .with_configuration(plugin_configuration),
                    ))),
                    geom: PaneGeom {
                        x: 0,
                        y: 60,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    is_borderless: true,
                    geom: PaneGeom {
                        x: 0,
                        y: 70,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    title: Some("my cool \\ \"pane_title\"".to_owned()),
                    is_focused: true,
                    pane_contents: Some("can has pane contents".to_owned()),
                    geom: PaneGeom {
                        x: 0,
                        y: 80,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab with \"tiled panes\"".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_tab_with_floating_panes() {
        use yinz_types::input::command::RunCommand;

        let mut plugin_configuration = BTreeMap::new();
        plugin_configuration.insert("key 1\"\\".to_owned(), "val 1\"\\".to_owned());
        plugin_configuration.insert("key 2\"\\".to_owned(), "val 2\"\\".to_owned());
        let tab_layout_manifest = TabLayoutManifest {
            floating_panes: vec![
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 0,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::Cwd(PathBuf::from("/tmp/\"my/cool cwd"))),
                    geom: PaneGeom {
                        x: 0,
                        y: 10,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::EditFile(
                        PathBuf::from("/tmp/\"my/cool cwd/my-file"),
                        None,
                        None,
                    )),
                    geom: PaneGeom {
                        x: 0,
                        y: 20,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::Command(RunCommand {
                        command: PathBuf::from("/tmp/\"my/cool cwd/command.sh"),
                        ..Default::default()
                    })),
                    geom: PaneGeom {
                        x: 0,
                        y: 30,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::Command(RunCommand {
                        command: PathBuf::from("/tmp/\"my/cool cwd/command.sh"),
                        args: vec![
                            "--arg1".to_owned(),
                            "arg\"2".to_owned(),
                            "arg > \\3".to_owned(),
                        ],
                        ..Default::default()
                    })),
                    geom: PaneGeom {
                        x: 0,
                        y: 40,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(
                        yinz_runtime::plugin_location::run_plugin_from_url(
                            "file:/tmp/\"my/cool cwd/plugin.scm",
                        )
                        .unwrap(),
                    ))),
                    geom: PaneGeom {
                        x: 0,
                        y: 50,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(
                        yinz_runtime::plugin_location::run_plugin_from_url(
                            "file:/tmp/\"my/cool cwd/plugin.scm",
                        )
                        .unwrap()
                        .with_configuration(plugin_configuration),
                    ))),
                    geom: PaneGeom {
                        x: 0,
                        y: 60,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    // note that in this case, `is_borderless` should be ignored because this is a
                    // floating pane
                    is_borderless: true,
                    geom: PaneGeom {
                        x: 0,
                        y: 70,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    title: Some("my cool \\ \"pane_title\"".to_owned()),
                    is_focused: true,
                    pane_contents: Some("can has pane contents".to_owned()),
                    geom: PaneGeom {
                        x: 0,
                        y: 80,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![(
                "Tab with \"floating panes\"".to_owned(),
                tab_layout_manifest,
            )],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_tab_with_stacked_panes() {
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: vec![
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 0,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(0),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 1,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: Some(0),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 11,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(0),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab with \"stacked panes\"".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_tab_with_multiple_stacked_panes_in_the_same_node() {
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: vec![
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 0,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(0),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 1,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: Some(0),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 11,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(0),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 12,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 22,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(1),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 23,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: Some(1),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 33,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(1),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab with \"stacked panes\"".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_tab_with_multiple_stacks_next_to_eachother() {
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: vec![
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 0,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(0),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 1,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: Some(0),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 11,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(0),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 12,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 22,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(1),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 23,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: Some(1),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 33,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(1),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 10,
                        y: 0,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(2),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 10,
                        y: 1,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: Some(2),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 10,
                        y: 11,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(2),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 10,
                        y: 12,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 10,
                        y: 22,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(3),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 10,
                        y: 23,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: Some(3),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 10,
                        y: 33,
                        rows: Dimension::fixed(1),
                        cols: Dimension::fixed(10),
                        stacked: Some(3),
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab with \"stacked panes\"".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_multiple_tabs() {
        let tab_1_layout_manifest = TabLayoutManifest {
            tiled_panes: vec![PaneLayoutManifest {
                geom: PaneGeom {
                    x: 0,
                    y: 0,
                    rows: Dimension::percent(100.0),
                    cols: Dimension::percent(100.0),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let tab_2_layout_manifest = TabLayoutManifest {
            tiled_panes: vec![
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 0,
                        y: 0,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
                PaneLayoutManifest {
                    geom: PaneGeom {
                        x: 10,
                        y: 0,
                        rows: Dimension::fixed(10),
                        cols: Dimension::fixed(10),
                        stacked: None,
                        is_pinned: false,
                        logical_position: None,
                    },
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![
                ("First tab".to_owned(), tab_1_layout_manifest),
                ("Second tab".to_owned(), tab_2_layout_manifest),
            ],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn pane_tabs_serialize_descriptors_configuration_and_selected_index() {
        use yinz_types::pane_tabs::*;
        let legacy: PaneTabDescriptor = serde_json::from_str(
            r#"{"label":"Files","location":"yinz:strider","configuration":{}}"#,
        )
        .unwrap();
        assert_eq!(legacy.initial_cwd, None);
        let tabs = PaneTabsLayout {
            active: 1,
            tabs: vec![
                PaneTabDescriptor {
                    label: "Files".into(),
                    location: "yinz:strider".into(),
                    initial_cwd: Some(PathBuf::from("/tmp/files")),
                    configuration: BTreeMap::new(),
                },
                PaneTabDescriptor {
                    label: "Todos!".into(),
                    location: "yinz:todos".into(),
                    initial_cwd: None,
                    configuration: BTreeMap::from([("custom".into(), "value".into())]),
                },
            ],
        };
        let pane = PaneLayoutManifest {
            pane_tabs: Some(tabs.clone()),
            geom: PaneGeom {
                rows: Dimension::fixed(24),
                cols: Dimension::fixed(80),
                ..Default::default()
            },
            ..Default::default()
        };
        for floating in [false, true] {
            let mut tab = TabLayoutManifest::default();
            if floating {
                tab.floating_panes.push(pane.clone());
            } else {
                tab.tiled_panes.push(pane.clone());
            }
            let (raw, _) = serialize_session_layout(GlobalLayoutManifest {
                tabs: vec![("tabs".into(), tab)],
                ..Default::default()
            })
            .unwrap();
            let restored = yinz_config::kdl::layout::from_kdl(&raw, None, None, None).unwrap();
            let (_, tiled, floats) = &restored.tabs[0];
            let actual = if floating {
                floats[0].pane_tabs.as_ref()
            } else {
                tiled.children[0].pane_tabs.as_ref()
            };
            let expected = tabs.clone();
            assert_eq!(actual, Some(&expected), "{raw}");
        }
    }

    #[test]
    fn can_serialize_tab_with_a_single_tiled_pane() {
        use yinz_types::input::command::RunCommand;
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: vec![PaneLayoutManifest {
                run: Some(Run::Command(RunCommand {
                    command: PathBuf::from("/bin/sleep"),
                    args: vec!["10000".to_owned()],
                    ..Default::default()
                })),
                title: Some("my-only-pane".to_owned()),
                geom: PaneGeom {
                    x: 0,
                    y: 0,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane command="/bin/sleep" name="my-only-pane" {
                        args "10000"
                        start_suspended true
                    }
                }
            }
        "#]]
        .assert_eq(&kdl.0);
    }
    #[test]
    fn can_serialize_tab_with_a_single_tiled_pane_and_a_floating_pane() {
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: vec![PaneLayoutManifest {
                title: Some("my-only-tiled-pane".to_owned()),
                geom: PaneGeom {
                    x: 0,
                    y: 0,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            }],
            floating_panes: vec![PaneLayoutManifest {
                title: Some("my-floating-pane".to_owned()),
                geom: PaneGeom {
                    x: 1,
                    y: 1,
                    rows: Dimension::fixed(5),
                    cols: Dimension::fixed(5),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        expect![[r#"
            layout {
                tab name="Tab #1" {
                    pane name="my-only-tiled-pane"
                    floating_panes {
                        pane name="my-floating-pane" {
                            height 5
                            width 5
                            x 1
                            y 1
                        }
                    }
                }
            }
        "#]]
        .assert_eq(&kdl.0);
    }

    /// A sandboxed pane resurrects into its sandbox: its command is the agent's own, so
    /// without the attribute it would come back running on the host.
    #[test]
    fn can_serialize_bubblewrap_panes() {
        use yinz_types::input::command::RunCommand;
        let sandboxed = RunCommand {
            command: PathBuf::from("claude"),
            args: vec!["--resume".to_owned()],
            cwd: Some(PathBuf::from("/repo/.worktrees/task")),
            hold_on_close: true,
            target: PaneTarget::Bubblewrap {
                root: PathBuf::from("/repo"),
            },
            ..Default::default()
        };
        let tab_layout_manifest = TabLayoutManifest {
            tiled_panes: vec![PaneLayoutManifest {
                run: Some(Run::Command(sandboxed.clone())),
                geom: PaneGeom {
                    x: 0,
                    y: 0,
                    rows: Dimension::fixed(10),
                    cols: Dimension::fixed(10),
                    stacked: None,
                    is_pinned: false,
                    logical_position: None,
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            tabs: vec![("Tab #1".to_owned(), tab_layout_manifest)],
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert!(kdl.0.contains(r#"bubblewrap="/repo""#), "{}", kdl.0);

        let layout =
            yinz_config::kdl::layout::from_kdl(&kdl.0, Some("dump.kdl".into()), None, None)
                .unwrap();
        let (_, tiled, _) = &layout.tabs[0];
        let mut panes = vec![tiled];
        let mut resurrected = None;
        while let Some(pane) = panes.pop() {
            if let Some(Run::Command(command)) = &pane.run {
                resurrected = Some(command.clone());
            }
            panes.extend(&pane.children);
        }
        assert_eq!(
            resurrected,
            Some(RunCommand {
                hold_on_start: true,
                ..sandboxed
            })
        );
    }
    #[test]
    fn can_serialize_new_tab_template_with_a_single_pane() {
        let tiled_panes_layout = TiledPaneLayout {
            name: Some("my-only-template-pane".to_owned()),
            ..Default::default()
        };
        let default_layout = Layout {
            template: Some((tiled_panes_layout, vec![])),
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            default_layout: Box::new(default_layout),
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        expect![[r#"
            layout {
                new_tab_template {
                    pane name="my-only-template-pane"
                }
            }
        "#]]
        .assert_eq(&kdl.0);
    }
    #[test]
    fn can_serialize_swap_tiled_layout_with_a_single_pane() {
        let tiled_panes_layout = TiledPaneLayout {
            name: Some("my-only-swap-pane".to_owned()),
            ..Default::default()
        };
        let mut swap_tiled_layout = BTreeMap::new();
        swap_tiled_layout.insert(LayoutConstraint::NoConstraint, tiled_panes_layout);
        let default_layout = Layout {
            swap_tiled_layouts: vec![(swap_tiled_layout, Some("my-swap".to_owned()))],
            ..Default::default()
        };
        let global_layout_manifest = GlobalLayoutManifest {
            default_layout: Box::new(default_layout),
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        expect![[r#"
            layout {
                swap_tiled_layout name="my-swap" {
                    tab {
                        pane name="my-only-swap-pane"
                    }
                }
            }
        "#]]
        .assert_eq(&kdl.0);
    }
    #[test]
    fn can_serialize_new_tab_template() {
        let tiled_panes_layout = TiledPaneLayout {
            children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
            ..Default::default()
        };

        let floating_panes_layout = vec![
            FloatingPaneLayout::default(),
            FloatingPaneLayout::default(),
            FloatingPaneLayout::default(),
        ];
        let default_layout = Box::new(Layout {
            template: Some((tiled_panes_layout, floating_panes_layout)),
            ..Default::default()
        });
        let global_layout_manifest = GlobalLayoutManifest {
            default_layout,
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_swap_tiled_panes() {
        let tiled_panes_layout = TiledPaneLayout {
            children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
            ..Default::default()
        };
        let mut default_layout = Layout::default();
        let mut swap_tiled_layout_1 = BTreeMap::new();
        let mut swap_tiled_layout_2 = BTreeMap::new();
        swap_tiled_layout_1.insert(LayoutConstraint::MaxPanes(1), tiled_panes_layout.clone());
        swap_tiled_layout_1.insert(LayoutConstraint::MinPanes(1), tiled_panes_layout.clone());
        swap_tiled_layout_1.insert(LayoutConstraint::ExactPanes(1), tiled_panes_layout.clone());
        swap_tiled_layout_1.insert(LayoutConstraint::NoConstraint, tiled_panes_layout.clone());
        swap_tiled_layout_2.insert(LayoutConstraint::MaxPanes(2), tiled_panes_layout.clone());
        swap_tiled_layout_2.insert(LayoutConstraint::MinPanes(2), tiled_panes_layout.clone());
        swap_tiled_layout_2.insert(LayoutConstraint::ExactPanes(2), tiled_panes_layout.clone());
        swap_tiled_layout_2.insert(LayoutConstraint::NoConstraint, tiled_panes_layout.clone());

        let swap_tiled_layouts = vec![
            (swap_tiled_layout_1, None),
            (swap_tiled_layout_2, Some("swap_tiled_layout_2".to_owned())),
        ];
        default_layout.swap_tiled_layouts = swap_tiled_layouts;
        let default_layout = Box::new(default_layout);
        let global_layout_manifest = GlobalLayoutManifest {
            default_layout,
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }
    #[test]
    fn can_serialize_swap_floating_panes() {
        let floating_panes_layout = vec![
            FloatingPaneLayout::default(),
            FloatingPaneLayout::default(),
            FloatingPaneLayout::default(),
        ];
        let mut default_layout = Layout::default();
        let mut swap_floating_layout_1 = BTreeMap::new();
        let mut swap_floating_layout_2 = BTreeMap::new();
        swap_floating_layout_1.insert(LayoutConstraint::MaxPanes(1), floating_panes_layout.clone());
        swap_floating_layout_1.insert(LayoutConstraint::MinPanes(1), floating_panes_layout.clone());
        swap_floating_layout_1.insert(
            LayoutConstraint::ExactPanes(1),
            floating_panes_layout.clone(),
        );
        swap_floating_layout_1.insert(
            LayoutConstraint::NoConstraint,
            floating_panes_layout.clone(),
        );
        swap_floating_layout_2.insert(LayoutConstraint::MaxPanes(2), floating_panes_layout.clone());
        swap_floating_layout_2.insert(LayoutConstraint::MinPanes(2), floating_panes_layout.clone());
        swap_floating_layout_2.insert(
            LayoutConstraint::ExactPanes(2),
            floating_panes_layout.clone(),
        );
        swap_floating_layout_2.insert(
            LayoutConstraint::NoConstraint,
            floating_panes_layout.clone(),
        );

        let swap_floating_layouts = vec![
            (swap_floating_layout_1, None),
            (
                swap_floating_layout_2,
                Some("swap_floating_layout_2".to_owned()),
            ),
        ];
        default_layout.swap_floating_layouts = swap_floating_layouts;
        let default_layout = Box::new(default_layout);
        let global_layout_manifest = GlobalLayoutManifest {
            default_layout,
            ..Default::default()
        };
        let kdl = serialize_session_layout(global_layout_manifest).unwrap();
        assert_snapshot!(kdl.0);
    }

    // ---------------------------------------------------------------------
    // Arrangements that are not a tree of splits
    // ---------------------------------------------------------------------

    fn manifest_at(x: usize, y: usize, cols: usize, rows: usize) -> PaneLayoutManifest {
        // `cols`/`rows` are `Dimension`s and are set through `set_inner` rather than assigned, so the
        // struct literal covers x and y and the two dimensions follow.
        let mut geom = PaneGeom {
            x,
            y,
            ..Default::default()
        };
        geom.cols.set_inner(cols);
        geom.rows.set_inner(rows);
        PaneLayoutManifest {
            geom,
            ..Default::default()
        }
    }

    fn stacked(mut manifest: PaneLayoutManifest, stack_id: usize) -> PaneLayoutManifest {
        manifest.geom.stacked = Some(stack_id);
        manifest
    }

    /// The ui strips span the screen, the panes between them only part of it. No
    /// split line spans this in either direction, and none of it is stacked.
    fn a_tab_that_is_not_a_rectangle() -> Vec<PaneLayoutManifest> {
        vec![
            manifest_at(0, 0, 100, 1),
            manifest_at(20, 1, 40, 19),
            manifest_at(60, 1, 40, 19),
            manifest_at(20, 20, 40, 19),
            manifest_at(60, 20, 40, 19),
            manifest_at(0, 39, 100, 1),
        ]
    }

    #[test]
    fn an_arrangement_that_is_not_a_layout_is_refused_rather_than_shrunk() {
        // It used to come back as a single empty pane standing in for all six -
        // the shape of bug that loses five panes without saying anything.
        let arrangement = a_tab_that_is_not_a_rectangle();
        assert!(get_tiled_panes_layout_from_panegeoms(&arrangement, None).is_none());
    }

    #[test]
    fn a_stack_layout_is_refused_when_something_in_it_is_not_stacked() {
        // `stack_layout_from_manifest` is only reached on the assumption that
        // everything left is in a stack. When that is wrong, the panes that are
        // not have nowhere to go, and saying so beats leaving them out.
        let mixed = vec![
            stacked(manifest_at(0, 0, 50, 1), 1),
            stacked(manifest_at(0, 1, 50, 19), 1),
            manifest_at(50, 0, 50, 20),
        ];
        assert!(stack_layout_from_manifest(&mixed, None).is_none());
        assert!(stack_layout_from_manifest(&vec![manifest_at(0, 0, 50, 20)], None).is_none());
    }

    #[test]
    fn an_approximated_arrangement_keeps_every_pane() {
        let arrangement = a_tab_that_is_not_a_rectangle();
        let layout = tiled_layout_from_panegeoms_lossy(&arrangement);
        assert_eq!(layout.pane_count(), arrangement.len());
    }

    #[test]
    fn an_approximation_keeps_a_stack_together() {
        // A stack cut in half arrives as two stacks, so the cuts go between them.
        let arrangement = vec![
            manifest_at(0, 0, 100, 1),
            stacked(manifest_at(20, 1, 80, 1), 1),
            stacked(manifest_at(20, 2, 80, 1), 1),
            stacked(manifest_at(20, 3, 80, 36), 1),
        ];
        let layout = tiled_layout_from_panegeoms_lossy(&arrangement);
        assert_eq!(layout.pane_count(), 4);

        fn stack_sizes(layout: &TiledPaneLayout, sizes: &mut Vec<usize>) {
            if layout.children_are_stacked {
                sizes.push(layout.children.len());
            }
            for child in &layout.children {
                stack_sizes(child, sizes);
            }
        }
        let mut sizes = vec![];
        stack_sizes(&layout, &mut sizes);
        assert_eq!(
            sizes,
            vec![3],
            "the stack should arrive as one stack of three"
        );
    }

    #[test]
    fn an_approximation_preserves_positions_after_rounding() {
        // The approximation is only reached for arrangements no layout describes,
        // but it should not be a *different* arrangement when handed one that a
        // layout does describe - a cut nothing straddles is a real split. Checked
        // against the arrangements these tests already have: each pane must come
        // back nearer its own rectangle than any other pane's, which is the
        // strongest thing that can be asked of something allowed to round.
        for panegeoms in PANEGEOMS_JSON.iter() {
            // each pane named after its index, so the leaves can be paired with
            // the panes they came from rather than with wherever they turn up
            let arrangement: Vec<PaneLayoutManifest> = panegeoms
                .iter()
                .map(|pg| parse_panegeom_from_json(pg))
                .enumerate()
                .map(|(index, geom)| PaneLayoutManifest {
                    geom,
                    run: Some(Run::Cwd(PathBuf::from(index.to_string()))),
                    ..Default::default()
                })
                .collect();
            let approximated = approximate_tiled_layout(&arrangement, None);
            assert_eq!(approximated.pane_count(), arrangement.len());

            let mut space = PaneGeom::default();
            space.cols.set_inner(
                arrangement
                    .iter()
                    .map(|pane| pane.geom.x + pane.geom.cols.as_usize())
                    .max()
                    .unwrap(),
            );
            space.rows.set_inner(
                arrangement
                    .iter()
                    .map(|pane| pane.geom.y + pane.geom.rows.as_usize())
                    .max()
                    .unwrap(),
            );
            let positioned = approximated.position_panes_in_space(&space, None, false, false);
            let positioned = positioned.unwrap();
            let landed: Vec<(usize, PaneGeom)> = positioned
                .into_iter()
                .map(|(layout, geom)| match layout.run {
                    Some(Run::Cwd(name)) => {
                        (name.to_str().unwrap().parse::<usize>().unwrap(), geom)
                    },
                    other => panic!("a pane came back without its name: {other:?}"),
                })
                .collect();
            assert_eq!(landed.len(), arrangement.len());
            let moved_by =
                |from: &PaneGeom, to: &PaneGeom| from.x.abs_diff(to.x) + from.y.abs_diff(to.y);
            for (a, landed_a) in &landed {
                for (b, landed_b) in &landed {
                    let (was_a, was_b) = (&arrangement[*a].geom, &arrangement[*b].geom);
                    assert!(
                        moved_by(was_a, landed_a) + moved_by(was_b, landed_b)
                            <= moved_by(was_a, landed_b) + moved_by(was_b, landed_a),
                        "panes {a} and {b} came back in each other's places",
                    );
                }
            }
        }
    }

    // utility functions
    fn parse_panegeom_from_json(data_str: &str) -> PaneGeom {
        //
        // Expects this input
        //
        //  r#"{ "x": 0, "y": 1, "rows": { "constraint": "Percent(100.0)", "inner": 43 }, "cols": { "constraint": "Percent(100.0)", "inner": 211 }, "is_stacked": false }"#,
        //
        let data: HashMap<String, Value> = serde_json::from_str(data_str).unwrap();
        PaneGeom {
            x: data["x"].to_string().parse().unwrap(),
            y: data["y"].to_string().parse().unwrap(),
            rows: get_dim(&data["rows"]),
            cols: get_dim(&data["cols"]),
            stacked: None,
            is_pinned: false,
            logical_position: None,
        }
    }

    fn get_dim(dim_hm: &Value) -> Dimension {
        let constr_str = dim_hm["constraint"].to_string();
        let dim = if constr_str.contains("Fixed") {
            let value = &constr_str[7..constr_str.len() - 2];
            Dimension::fixed(value.parse().unwrap())
        } else if constr_str.contains("Percent") {
            let value = &constr_str[9..constr_str.len() - 2];
            let mut dim = Dimension::percent(value.parse().unwrap());
            dim.set_inner(dim_hm["inner"].to_string().parse().unwrap());
            dim
        } else {
            panic!("Constraint is nor a percent nor fixed");
        };
        dim
    }
}
