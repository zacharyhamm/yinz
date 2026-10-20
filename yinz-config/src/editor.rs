//! Configuration palette catalog and targeted overlays. The normal KDL parser validates edits.
use crate::config::{Config, ConfigError};
use kdl::{KdlDocument, KdlNode, KdlValue};
use serde::Serialize;
use serde_json::Value;
use yinz_types::configuration::{Node, Patch};

pub const EDITS_BLOCK: &str = "configuration_edits";
pub(crate) const KEYBIND_BLOCK_SOURCE: &str = "configuration-editor-source";

pub(crate) fn keybind_block_source(node: &KdlNode) -> Result<Option<KdlNode>, String> {
    use yinz_types::{data::InputMode, keys::KeyWithModifier};

    let Some(entry) = node.get(KEYBIND_BLOCK_SOURCE) else {
        return Ok(None);
    };
    let invalid = || "Invalid configuration-editor-source on keybind block".to_owned();
    let (name, arguments): (String, Vec<String>) =
        serde_json::from_str(entry.value().as_string().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    let valid = if name == "unbind" {
        node.name().value().parse::<InputMode>().is_ok()
            && node.entries().iter().all(|entry| {
                entry
                    .name()
                    .is_some_and(|name| name.value() == KEYBIND_BLOCK_SOURCE)
            })
            && arguments
                .iter()
                .all(|key| key.parse::<KeyWithModifier>().is_ok())
            && node.children().is_some_and(|children| {
                children
                    .nodes()
                    .iter()
                    .all(|child| child.name().value() == "unbind")
            })
    } else {
        matches!(
            node.name().value(),
            "shared" | "shared_except" | "shared_among"
        ) && matches!(name.as_str(), "shared" | "shared_except" | "shared_among")
            && arguments
                .iter()
                .all(|mode| mode.parse::<InputMode>().is_ok())
    };
    if !valid {
        return Err(invalid());
    }
    let mut source = KdlNode::new(name);
    for argument in arguments {
        source.push(argument);
    }
    Ok(Some(source))
}

#[derive(Debug, Clone, Serialize)]
pub struct Setting {
    pub name: String,
    pub label: String,
    pub kind: &'static str,
    pub choices: Vec<&'static str>,
    pub minimum: Option<u64>,
    pub maximum: Option<u64>,
    pub help: &'static str,
    pub timing: &'static str,
}

pub fn catalog() -> Vec<Setting> {
    let options =
        serde_json::to_value(yinz_types::input::options::Options::default()).unwrap_or(Value::Null);
    let names = options
        .as_object()
        .map(|o| o.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    names
        .into_iter()
        .map(|name| {
            let (kind, choices, minimum, maximum, help) = match name.as_str() {
                "background_opacity" => (
                    "number",
                    vec![],
                    Some(0),
                    Some(100),
                    "Set background opacity for overlays that support transparency. 0 is transparent, 100 is opaque.",
                ),
                "serialization_interval" => (
                    "number",
                    vec![],
                    Some(1),
                    None,
                    "Set the interval in seconds between saved session updates.",
                ),
                "notification_timeout" => (
                    "number",
                    vec![],
                    Some(0),
                    None,
                    "Set how many seconds notifications stay visible. Zero disables automatic dismissal.",
                ),
                "locked_escape_timeout_ms" => (
                    "number",
                    vec![],
                    Some(0),
                    None,
                    "Set how long Ctrl+g waits for a plain g in locked mode, in milliseconds. Zero makes Ctrl+g unlock immediately.",
                ),
                "scrollback_lines_to_serialize" => (
                    "number",
                    vec![],
                    Some(0),
                    None,
                    "Set how many scrollback lines to save with pane contents. Zero uses the scrollback buffer size.",
                ),
                "scroll_buffer_size" => (
                    "number",
                    vec![],
                    Some(0),
                    None,
                    "Set how many scrollback lines each pane keeps. Zero keeps no lines.",
                ),
                "max_agent_width" => (
                    "number",
                    vec![],
                    Some(0),
                    None,
                    "Limit the text width of agent panes in columns. Values below 80 disable the limit.",
                ),
                "agent_pane_min_columns" | "agent_pane_min_rows" => (
                    "number",
                    vec![],
                    Some(0),
                    None,
                    "Set the minimum pane area for placing agents, using columns times rows. Zero in either setting disables this rule.",
                ),
                "default_mode" => (
                    "choice",
                    vec![
                        "normal",
                        "locked",
                        "resize",
                        "pane",
                        "tab",
                        "scroll",
                        "search",
                        "entersearch",
                        "renametab",
                        "renamepane",
                        "session",
                        "move",
                        "prompt",
                        "tmux",
                        "agent",
                        "select",
                        "notifications",
                    ],
                    None,
                    None,
                    "Choose the input mode used when a session starts.",
                ),
                "pane_frame_style" => (
                    "choice",
                    vec!["full", "titles", "none"],
                    None,
                    None,
                    "Choose full pane borders, title lines only, or no frames.",
                ),
                "copy_clipboard" => (
                    "choice",
                    vec!["system", "primary"],
                    None,
                    None,
                    "Choose whether OSC 52 copies use the system clipboard or the primary selection.",
                ),
                "on_force_close" => (
                    "choice",
                    vec!["detach", "quit"],
                    None,
                    None,
                    "Choose whether closing the host terminal detaches from or quits the session.",
                ),
                "notification_position" => (
                    "choice",
                    vec!["top-left", "top-right", "bottom-left", "bottom-right"],
                    None,
                    None,
                    "Choose which corner of the screen shows notifications.",
                ),
                "explicit_theme_hue" => (
                    "choice",
                    vec!["dark", "light"],
                    None,
                    None,
                    "Remove this setting to follow the host appearance.",
                ),
                "nested_session_handling" => (
                    "choice",
                    vec!["ask", "fullscreen", "descend", "never"],
                    None,
                    None,
                    "Choose how to handle a Yinz session running inside a pane.",
                ),
                "host_notification_protocol" => (
                    "choice",
                    vec!["auto", "osc9", "osc99", "bell", "off"],
                    None,
                    None,
                    "Choose how notifications from pane programs reach the host terminal.",
                ),
                "agent_theme_mapping" => (
                    "choice",
                    vec!["off", "semantic"],
                    None,
                    None,
                    "Map managed agent colors to the session theme when set to semantic.",
                ),
                "theme" | "theme_dark" | "theme_light" => (
                    "theme",
                    vec![],
                    None,
                    None,
                    "Dark and light themes must both be configured to follow host appearance.",
                ),
                "default_shell" | "scrollback_editor" => (
                    "file",
                    vec![],
                    None,
                    None,
                    if name == "default_shell" {
                        "Choose the shell executable for new panes."
                    } else {
                        "Choose the executable used to edit pane scrollback."
                    },
                ),
                "default_layout" => (
                    "file",
                    vec![],
                    None,
                    None,
                    "Choose the default layout by name or file path.",
                ),
                "default_cwd" | "layout_dir" | "theme_dir" | "user_plugin_dir" => (
                    "directory",
                    vec![],
                    None,
                    None,
                    match name.as_str() {
                        "default_cwd" => "Choose the working directory for new panes.",
                        "layout_dir" => "Choose the directory used to find named layouts.",
                        "theme_dir" => "Choose the directory used to find theme files.",
                        _ => "Choose the directory used to find your plugins.",
                    },
                ),
                "copy_command" | "post_command_discovery_hook" => (
                    "multiline",
                    vec![],
                    None,
                    None,
                    if name == "copy_command" {
                        "Set the command used to copy text instead of OSC 52. Saving does not execute this command."
                    } else {
                        "Set the command that modifies discovered pane commands before saving the session. Saving does not execute this command."
                    },
                ),
                "word_separators" => (
                    "text",
                    vec![],
                    None,
                    None,
                    "Set extra word boundaries for double-click selection. Whitespace always separates words and is preserved here.",
                ),
                "session_name" | "host_id" => (
                    "text",
                    vec![],
                    None,
                    None,
                    if name == "session_name" {
                        "Set the name to use when starting a session."
                    } else {
                        "Set this installation's identity in shared plugin data. Changing it leaves existing records under the old identity."
                    },
                ),
                _ => (
                    "boolean",
                    vec!["Enabled", "Disabled"],
                    None,
                    None,
                    match name.as_str() {
                        "tab_groups" => "Group tabs and show a group bar. Disable to show all tabs in one row.",
                        "simplified_ui" => "Use simpler plugin visuals that work with more fonts.",
                        "mouse_mode" => "Let Yinz handle mouse input for pane selection and resizing. Hold Shift to bypass it.",
                        "agent_double_escape" => "Require two plain Escape presses within one second in agent panes.",
                        "detachable_panes" => "Keep pane processes running when the session server exits.",
                        "pane_frames" => "Show frames around panes.",
                        "pane_animations" => "Animate panes as they open.",
                        "mirror_session" => "Keep the same focused tab and pane for all connected clients.",
                        "copy_on_select" => "Copy selected text to the clipboard automatically.",
                        "osc8_hyperlinks" => "Send clickable hyperlinks to terminals that support OSC 8.",
                        "attach_to_session" => "Attach to the configured session name if that session already exists.",
                        "auto_layout" => "Arrange panes using predefined layouts when possible.",
                        "session_serialization" => "Save sessions to disk so they can be restored later.",
                        "serialize_pane_viewport" => "Include visible pane contents when saving sessions.",
                        "styled_underlines" => "Display styled underlines from pane programs.",
                        "disable_session_metadata" => "Stop writing session metadata to disk.",
                        "support_kitty_keyboard_protocol" => "Use the Kitty keyboard protocol when the host terminal supports it.",
                        "report_modifier_keys" => "Ask the terminal to report modifier keys held alone. This can interfere with AltGr, dead keys, and input methods.",
                        "support_kitty_graphics_protocol" => "Display Kitty graphics from pane programs when the host terminal supports them.",
                        "stacked_resize" => "Stack panes when resizing leaves too little room for them side by side.",
                        "stacked_pane_list" => "Display stacked panes as a list with the expanded pane pinned to the bottom.",
                        "advanced_mouse_actions" => "Enable mouse hover effects and pane grouping.",
                        "mouse_scroll_resize" => "Resize panes with Ctrl and the mouse scroll wheel.",
                        "mouse_hover_effects" => "Highlight pane frames and show help when the pointer hovers over them.",
                        "visual_bell" => "Show pane and tab indicators when a program rings the terminal bell.",
                        "focus_follows_mouse" => "Focus a pane when the pointer moves over it.",
                        "mouse_click_through" => "Send the click used to focus a pane to the program inside it.",
                        "context_menu_on_right_click" => "Open the pane context menu on right-click, unless the program handles the mouse.",
                        "context_menu_override_agent_mouse" => "Let right-click open the context menu for recognized agents even when they handle the mouse. Requires context menu on right click.",
                        "osc133_command_selection" => "Select a command and its output on triple-click when the shell marks command boundaries with OSC 133.",
                        "dangerously_enable_paste_buffer_read" => "Allow any pane program, including remote programs, to read the clipboard through OSC 52 without asking.",
                        _ => "",
                    },
                ),
            };
            let timing = match name.as_str() {
                "session_name"
                | "host_id"
                | "attach_to_session"
                | "theme_dir"
                | "on_force_close"
                | "report_modifier_keys"
                | "scroll_buffer_size"
                | "serialization_interval"
                | "detachable_panes"
                | "disable_session_metadata" => "On next session start",
                "default_shell" | "default_cwd" | "default_layout" | "layout_dir" => {
                    "Applies to new panes"
                },
                "user_plugin_dir" => "Applies to new plugin instances",
                _ => "Applies to this session",
            };
            Setting {
                label: name.replace('_', " "),
                name,
                kind,
                choices,
                minimum,
                maximum,
                help,
                timing,
            }
        })
        .collect()
}

pub fn document(config: &Config) -> Result<Vec<Node>, String> {
    let mut config = config.clone();
    config.overlay_tab_plugins = Some(config.overlay_tab_plugin_declarations());
    config.themes = crate::theme::Themes::from_data(
        config
            .themes
            .inner()
            .iter()
            .map(|(name, theme)| {
                let mut theme = theme.clone();
                theme.sourced_from_external_file = false;
                (name.clone(), theme)
            })
            .collect(),
    );
    let keybinds = std::mem::take(&mut config.keybinds);
    let mut document = config.to_kdl(false).map_err(validation_error)?;
    let expanded = crate::kdl::keybinds::to_editor_kdl(&keybinds).map_err(|e| e.to_string())?;
    if let Some(node) = document.get_mut("keybinds") {
        *node = expanded;
    } else {
        document.nodes_mut().push(expanded);
    }
    Ok(document.nodes().iter().map(from_kdl).collect())
}

/// Discovery carries names only; loading one theme keeps inspection bounded for large libraries.
pub fn inspection_document(
    config: &Config,
    theme: Option<&str>,
    effective: bool,
) -> Result<Vec<Node>, String> {
    let mut selected = config.clone();
    selected.themes = crate::theme::Themes::from_data(
        config
            .themes
            .inner()
            .iter()
            .filter(|(name, _)| theme == Some(name.as_str()))
            .map(|(name, theme)| (name.clone(), theme.clone()))
            .collect(),
    );
    let mut nodes = if effective {
        effective_document(&selected)?
    } else {
        document(&selected)?
    };
    let index = nodes
        .iter()
        .position(|node| node.name == "themes")
        .unwrap_or_else(|| {
            nodes.push(Node {
                name: "themes".into(),
                arguments: vec![],
                properties: Default::default(),
                children: vec![],
            });
            nodes.len() - 1
        });
    for name in config
        .themes
        .inner()
        .keys()
        .filter(|name| theme != Some(name.as_str()))
    {
        nodes[index].children.push(Node {
            name: name.clone(),
            arguments: vec![],
            properties: Default::default(),
            children: vec![],
        });
    }
    nodes[index].children.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(nodes)
}

/// Show resolved screen defaults without storing them in the session override layer.
pub fn effective_document(config: &Config) -> Result<Vec<Node>, String> {
    let screen = crate::screen_config::ScreenConfig::from_config(config);
    let mut resolved = config.clone();
    let options = &mut resolved.options;
    options.tab_groups.get_or_insert(screen.tab_groups);
    options.simplified_ui.get_or_insert(screen.simplified_ui);
    options
        .pane_animations
        .get_or_insert(screen.pane_animations);
    options
        .background_opacity
        .get_or_insert(screen.background_opacity);
    options
        .styled_underlines
        .get_or_insert(screen.styled_underlines);
    options
        .osc8_hyperlinks
        .get_or_insert(screen.osc8_hyperlinks);
    options.auto_layout.get_or_insert(screen.auto_layout);
    options.stacked_resize.get_or_insert(screen.stacked_resize);
    options
        .stacked_pane_list
        .get_or_insert(screen.stacked_pane_list);
    options
        .agent_double_escape
        .get_or_insert(screen.agent_double_escape);
    options
        .advanced_mouse_actions
        .get_or_insert(screen.advanced_mouse_actions);
    options
        .mouse_scroll_resize
        .get_or_insert(screen.mouse_scroll_resize);
    options
        .mouse_hover_effects
        .get_or_insert(screen.mouse_hover_effects);
    options
        .focus_follows_mouse
        .get_or_insert(screen.focus_follows_mouse);
    options
        .mouse_click_through
        .get_or_insert(screen.mouse_click_through);
    options
        .context_menu_on_right_click
        .get_or_insert(screen.context_menu_on_right_click);
    options
        .context_menu_override_agent_mouse
        .get_or_insert(screen.context_menu_override_agent_mouse);
    options
        .osc133_command_selection
        .get_or_insert(screen.osc133_command_selection);
    options.copy_on_select.get_or_insert(screen.copy_on_select);
    options
        .support_kitty_keyboard_protocol
        .get_or_insert(screen.support_kitty_keyboard_protocol);
    options
        .support_kitty_graphics_protocol
        .get_or_insert(screen.support_kitty_graphics_protocol);
    options
        .dangerously_enable_paste_buffer_read
        .get_or_insert(screen.dangerously_enable_paste_buffer_read);
    options.visual_bell.get_or_insert(screen.visual_bell);
    options
        .session_serialization
        .get_or_insert(screen.session_serialization);
    options
        .serialize_pane_viewport
        .get_or_insert(screen.serialize_pane_viewport);
    options
        .notification_position
        .get_or_insert(screen.notification_position);
    options
        .host_notification_protocol
        .get_or_insert(screen.host_notification_protocol);
    options
        .nested_session_handling
        .get_or_insert(screen.nested_session_handling);
    options
        .max_agent_width
        .get_or_insert(screen.max_agent_width);
    options
        .agent_theme_mapping
        .get_or_insert(screen.agent_theme_mapping);
    options
        .pane_frames
        .get_or_insert(screen.pane_frame_style != yinz_types::input::options::PaneFrameStyle::None);
    options
        .pane_frame_style
        .get_or_insert(screen.pane_frame_style);
    options
        .notification_timeout
        .get_or_insert(screen.notification_timeout_secs);
    options
        .agent_pane_min_columns
        .get_or_insert(screen.agent_pane_min_size.cols);
    options
        .agent_pane_min_rows
        .get_or_insert(screen.agent_pane_min_size.rows);
    options
        .word_separators
        .get_or_insert(screen.word_separators);
    options
        .mirror_session
        .get_or_insert(screen.session_is_mirrored);
    options.mouse_mode.get_or_insert(true);
    options.default_mode.get_or_insert_with(Default::default);
    options.report_modifier_keys.get_or_insert(false);
    options
        .scroll_buffer_size
        .get_or_insert(yinz_runtime::consts::DEFAULT_SCROLL_BUFFER_SIZE);
    document(&resolved)
}

fn from_kdl(node: &KdlNode) -> Node {
    fn value(v: &KdlValue) -> Value {
        if let Some(v) = v.as_string() {
            Value::from(v)
        } else if let Some(v) = v.as_bool() {
            Value::from(v)
        } else if let Some(v) = v.as_i64() {
            Value::from(v)
        } else {
            Value::Null
        }
    }
    Node {
        name: node.name().value().into(),
        arguments: node
            .entries()
            .iter()
            .filter(|e| e.name().is_none())
            .map(|e| value(e.value()))
            .collect(),
        properties: node
            .entries()
            .iter()
            .filter_map(|e| e.name().map(|n| (n.value().into(), value(e.value()))))
            .collect(),
        children: node
            .children()
            .map(|c| c.nodes().iter().map(from_kdl).collect())
            .unwrap_or_default(),
    }
}

fn to_kdl(node: &Node, depth: usize, parent: Option<&str>) -> Result<KdlNode, String> {
    if depth > 32 || node.name.is_empty() {
        return Err("Configuration nesting is too deep or a name is empty".into());
    }
    fn value(v: &Value) -> Result<KdlValue, String> {
        match v {
            Value::String(v) => Ok(v.clone().into()),
            Value::Bool(v) => Ok((*v).into()),
            Value::Number(v) => v
                .as_i64()
                .map(Into::into)
                .ok_or_else(|| "Expected a whole number in range".into()),
            _ => Err("Expected text, boolean or whole number".into()),
        }
    }
    let mut result = KdlNode::new(node.name.clone());
    for v in &node.arguments {
        result.push(value(v)?);
    }
    for (k, v) in &node.properties {
        result.insert(k.as_str(), value(v)?);
    }
    if !node.children.is_empty()
        || (parent == Some("keybinds") && node.name != "unbind")
        || matches!(
            node.name.as_str(),
            "env"
                | "ui"
                | "themes"
                | "plugins"
                | "load_plugins"
                | "overlay_tab_plugins"
                | "hooks"
                | "keybinds"
                | "bind"
                | "context_menu"
        )
    {
        let mut children = KdlDocument::new();
        for child in &node.children {
            children
                .nodes_mut()
                .push(to_kdl(child, depth + 1, Some(&node.name))?);
        }
        result.set_children(children);
    }
    Ok(result)
}

pub fn node_key(node: &Node) -> String {
    if node.name == "bind" || node.name == "unbind" {
        format!(
            "{}:{}",
            node.name,
            serde_json::to_string(&node.arguments).unwrap_or_default()
        )
    } else {
        node.name.clone()
    }
}

fn normalize_chords(arguments: &mut [Value]) -> Result<(), String> {
    for argument in arguments {
        let chord = argument.as_str().ok_or("Binding keys must be strings")?;
        let key: yinz_types::keys::KeyWithModifier = chord
            .parse()
            .map_err(|e| format!("Invalid key {chord:?}: {e}"))?;
        *argument = Value::String(key.to_keybind_string());
    }
    Ok(())
}

pub fn normalize_path(path: &mut [String]) -> Result<(), String> {
    if path.first().is_some_and(|root| root == "keybinds") {
        for part in path.iter_mut().skip(2) {
            if let Some((name @ ("bind" | "unbind"), arguments)) = part.split_once(':') {
                let mut arguments: Vec<Value> =
                    serde_json::from_str(arguments).map_err(|e| e.to_string())?;
                normalize_chords(&mut arguments)?;
                *part = format!(
                    "{name}:{}",
                    serde_json::to_string(&arguments).map_err(|e| e.to_string())?
                );
            }
        }
    }
    Ok(())
}

pub fn normalize_patch(patch: &mut Patch) -> Result<(), String> {
    fn normalize_node(node: &mut Node) -> Result<(), String> {
        if matches!(node.name.as_str(), "bind" | "unbind") {
            normalize_chords(&mut node.arguments)?;
        }
        for child in &mut node.children {
            normalize_node(child)?;
        }
        Ok(())
    }
    normalize_path(&mut patch.path)?;
    if patch.path.first().is_some_and(|root| root == "keybinds")
        && let Some(node) = &mut patch.node
    {
        normalize_node(node)?;
    }
    Ok(())
}

pub fn expand_binding_paths(path: &[String]) -> Result<Vec<Vec<String>>, String> {
    let mut path = path.to_vec();
    normalize_path(&mut path)?;
    if path.len() >= 3
        && path[0] == "keybinds"
        && let Some((name @ ("bind" | "unbind"), arguments)) = path[2].split_once(':')
    {
        let arguments: Vec<Value> = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
        if arguments.len() <= 1 {
            return Ok(vec![path]);
        }
        return arguments
            .into_iter()
            .map(|argument| {
                let mut single = path.clone();
                single[2] = format!("{name}:{}", serde_json::json!([argument]));
                Ok(single)
            })
            .collect();
    }
    Ok(vec![path])
}

pub fn normalize_patches(patches: &[Patch]) -> Result<Vec<Patch>, String> {
    let mut expanded = vec![];
    for patch in patches {
        let mut patch = patch.clone();
        normalize_patch(&mut patch)?;
        if patch.path.len() == 3
            && patch.path[0] == "keybinds"
            && let Some(node) = &patch.node
            && node_key(node) != patch.path[2]
        {
            return Err("Replacement name must match its target".into());
        }
        for path in expand_binding_paths(&patch.path)? {
            let mut single = patch.clone();
            if path.len() == 3
                && path != patch.path
                && let Some(node) = &mut single.node
            {
                let (_, arguments) = path[2].split_once(':').ok_or("Invalid binding path")?;
                node.arguments = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
            }
            single.path = path;
            expanded.push(single);
        }
    }
    Ok(expanded)
}

pub fn patch_document(nodes: &mut Vec<Node>, patch: &Patch) -> Result<(), String> {
    if patch.path.is_empty() || patch.path.len() > 32 {
        return Err("An edit must name a configuration entry".into());
    }
    let mut siblings = nodes;
    for part in &patch.path[..patch.path.len() - 1] {
        if siblings.iter().filter(|n| &node_key(n) == part).count() > 1 {
            return Err("Edit the ordered parent collection instead".into());
        }
        let index = match siblings.iter().position(|n| &node_key(n) == part) {
            Some(index) => index,
            None => {
                siblings.push(Node {
                    name: part.clone(),
                    arguments: vec![],
                    properties: Default::default(),
                    children: vec![],
                });
                siblings.len() - 1
            },
        };
        siblings = &mut siblings[index].children;
    }
    let name = &patch.path[patch.path.len() - 1];
    if siblings.iter().filter(|n| &node_key(n) == name).count() > 1 {
        return Err("Edit the ordered parent collection instead".into());
    }
    if patch.node.as_ref().is_some_and(|n| &node_key(n) != name) {
        return Err("Replacement name must match its target".into());
    }
    let position = siblings
        .iter()
        .position(|n| &node_key(n) == name)
        .unwrap_or(siblings.len());
    siblings.retain(|n| &node_key(n) != name);
    if let Some(node) = &patch.node {
        siblings.insert(position.min(siblings.len()), node.clone());
    }
    Ok(())
}

pub fn validation_error(error: ConfigError) -> String {
    match error {
        ConfigError::KdlError(error) => error.error_message,
        other => other.to_string(),
    }
}

pub fn apply(config: &Config, patches: &[Patch]) -> Result<Config, String> {
    let patches = normalize_patches(patches)?;
    let catalog = catalog();
    let mut result = config.clone();
    let roots: std::collections::BTreeSet<_> = patches
        .iter()
        .map(|p| p.path.first().cloned().ok_or("Missing edit target"))
        .collect::<Result<_, _>>()?;
    let mut roots: Vec<_> = roots.into_iter().collect();
    roots.sort_by_key(|root| root != "plugins");
    for root in roots {
        let mut section = Config {
            options: result.options.clone(),
            plugins: result.plugins.clone(),
            ..Default::default()
        };
        match root.as_str() {
            "themes" => section.themes = config.themes.clone(),
            "keybinds" => section.keybinds = config.keybinds.clone(),
            "plugins" => section.plugins = config.plugins.clone(),
            "load_plugins" => {
                section.background_plugins = config.background_plugins.clone();
                section.background_plugins_cleared = config.background_plugins_cleared;
            },
            "overlay_tab_plugins" => {
                section.overlay_tab_plugins = config.overlay_tab_plugins.clone()
            },
            "hooks" => section.hooks = config.hooks.clone(),
            "context_menu" => section.context_menu = config.context_menu.clone(),
            "env" => section.env = config.env.clone(),
            "ui" => section.ui = config.ui,
            _ if catalog.iter().any(|s| s.name == root) => {},
            _ => return Err(format!("Unknown configuration section: {root}")),
        }
        if root == "themes" {
            let all = patches.iter().any(|p| p.path == [root.clone()]);
            if !all {
                let names: std::collections::BTreeSet<_> = patches
                    .iter()
                    .filter(|p| p.path.first() == Some(&root))
                    .filter_map(|p| p.path.get(1))
                    .collect();
                let builtins = crate::assets::get_default_themes();
                section.themes = crate::theme::Themes::from_data(
                    names
                        .into_iter()
                        .filter_map(|name| {
                            config
                                .themes
                                .get_theme(name)
                                .or_else(|| builtins.get_theme(name))
                                .map(|theme| (name.clone(), theme.clone()))
                        })
                        .collect(),
                );
            }
        }
        let original = document(&section)?;
        let mut nodes = original.clone();
        for patch in patches.iter().filter(|p| p.path.first() == Some(&root)) {
            if patch.reset {
                let restored = Patch {
                    path: patch.path.clone(),
                    node: find_node(&original, &patch.path).cloned(),
                    reset: false,
                };
                patch_document(&mut nodes, &restored)?;
            } else {
                patch_document(&mut nodes, patch)?;
            }
        }
        let mut document = KdlDocument::new();
        for node in &nodes {
            if root != "overlay_tab_plugins" && node.name == "overlay_tab_plugins" {
                continue;
            }
            document.nodes_mut().push(to_kdl(node, 0, None)?);
        }
        // Validate overlay aliases against the completed batch, after all sections have applied.
        let base = (root != "overlay_tab_plugins").then(|| Config {
            overlay_tab_plugins: Some(vec![]),
            ..Default::default()
        });
        let parsed = Config::from_kdl(&document.to_string(), base).map_err(validation_error)?;
        match root.as_str() {
            "themes" => {
                if patches.iter().any(|p| p.path == [root.clone()]) {
                    result.themes = parsed.themes;
                } else {
                    let touched: std::collections::BTreeSet<_> = patches
                        .iter()
                        .filter(|p| p.path.first() == Some(&root))
                        .filter_map(|p| p.path.get(1))
                        .collect();
                    let mut themes = result.themes.inner().clone();
                    for name in touched {
                        themes.remove(name);
                    }
                    result.themes = crate::theme::Themes::from_data(themes).merge(parsed.themes);
                }
            },
            "keybinds" => result.keybinds = parsed.keybinds,
            "plugins" => result.plugins = parsed.plugins,
            "load_plugins" => {
                result.background_plugins = parsed.background_plugins;
                result.background_plugins_cleared = parsed.background_plugins_cleared;
            },
            "overlay_tab_plugins" => result.overlay_tab_plugins = parsed.overlay_tab_plugins,
            "hooks" => result.hooks = parsed.hooks,
            "context_menu" => result.context_menu = parsed.context_menu,
            "env" => result.env = parsed.env,
            "ui" => result.ui = parsed.ui,
            _ => result.options = parsed.options,
        }
    }
    result.resolved_overlay_tab_plugins()?;
    Ok(result)
}

pub fn edits_themes(source: &str) -> bool {
    source
        .parse::<KdlDocument>()
        .ok()
        .and_then(|document| {
            document.get(EDITS_BLOCK).map(|block| {
                block.children().is_some_and(|children| {
                    children.nodes().iter().any(|node| {
                        node.get(0).and_then(|entry| entry.value().as_string()) == Some("themes")
                    })
                })
            })
        })
        .unwrap_or(false)
}

pub fn patches_from_kdl(node: &KdlNode) -> Result<Vec<Patch>, ConfigError> {
    let parse = || -> Result<Vec<Patch>, String> {
        node.children()
            .into_iter()
            .flat_map(|d| d.nodes())
            .map(|edit| {
                let path = edit
                    .entries()
                    .iter()
                    .map(|e| {
                        e.value()
                            .as_string()
                            .map(str::to_owned)
                            .ok_or_else(|| "Edit paths must be strings".to_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let children = edit.children().map(|c| c.nodes()).unwrap_or_default();
                let node = match (edit.name().value(), children) {
                    ("set", [node]) => Some(from_kdl(node)),
                    ("remove" | "reset", []) => None,
                    _ => {
                        return Err("Expected set with one child, or remove without children".into())
                    },
                };
                Ok(Patch {
                    path,
                    node,
                    reset: edit.name().value() == "reset",
                })
            })
            .collect()
    };
    parse()
        .and_then(|patches| normalize_patches(&patches))
        .map_err(|e| ConfigError::new_kdl_error(e, node.span().offset(), node.span().len()))
}

pub fn patches_to_kdl(patches: &[Patch]) -> Result<KdlNode, String> {
    let mut block = KdlNode::new(EDITS_BLOCK);
    let mut children = KdlDocument::new();
    for patch in normalize_patches(patches)? {
        let mut edit = KdlNode::new(if patch.reset {
            "reset"
        } else if patch.node.is_some() {
            "set"
        } else {
            "remove"
        });
        for part in &patch.path {
            edit.push(part.clone());
        }
        if let Some(node) = &patch.node {
            let parent = patch.path.iter().rev().nth(1).map(String::as_str);
            edit.ensure_children()
                .nodes_mut()
                .push(to_kdl(node, 0, parent)?);
        }
        children.nodes_mut().push(edit);
    }
    block.set_children(children);
    Ok(block)
}

pub(crate) fn reconcile_legacy_keybinds(
    edits: &KdlNode,
    incoming: &KdlNode,
) -> Result<KdlNode, String> {
    let mut edits = patches_from_kdl(edits).map_err(validation_error)?;
    let root = from_kdl(incoming);
    let mut updates = vec![];
    if root.properties.get("clear-defaults") == Some(&Value::Bool(true)) {
        updates.push(Patch {
            path: vec!["keybinds".into()],
            node: Some(root),
            reset: false,
        });
    } else {
        for block in expanded_keybind_blocks(&root)? {
            let path = vec!["keybinds".into(), block.name.clone()];
            if block.properties.get("clear-defaults") == Some(&Value::Bool(true)) {
                updates.push(Patch {
                    path: path.clone(),
                    node: Some(Node {
                        children: vec![],
                        ..block.clone()
                    }),
                    reset: false,
                });
            }
            for binding in &block.children {
                legacy_binding_updates(&mut updates, &path, binding)?;
            }
        }
    }

    for update in updates {
        edits.retain(|old| !old.path.starts_with(&update.path));
        // A parent edit would mask the legacy fragment, so override only its touched entry.
        if edits.iter().any(|old| update.path.starts_with(&old.path)) {
            edits.push(update);
        }
    }
    patches_to_kdl(&edits)
}

fn expanded_keybind_blocks(root: &Node) -> Result<Vec<Node>, String> {
    use strum::IntoEnumIterator;
    use yinz_types::data::InputMode;

    let mut blocks = vec![];
    // Generated global unbinds retain their precedence over ordinary mode blocks.
    for phase in 0..3 {
        for block in &root.children {
            let is_shared = matches!(
                block.name.as_str(),
                "shared" | "shared_among" | "shared_except"
            );
            let generated_global = keybind_block_source(&to_kdl(block, 0, Some("keybinds"))?)?
                .is_some_and(|source| source.name().value() == "unbind");
            let block_phase = if generated_global {
                2
            } else if is_shared {
                0
            } else {
                1
            };
            if block_phase != phase || block.name == "unbind" {
                continue;
            }
            for mode in InputMode::iter() {
                let listed = block.arguments.iter().any(|value| {
                    value
                        .as_str()
                        .and_then(|name| name.parse::<InputMode>().ok())
                        == Some(mode)
                });
                let applies = match block.name.as_str() {
                    "shared_among" => listed,
                    "shared" | "shared_except" => !listed,
                    _ => block.name.parse::<InputMode>().ok() == Some(mode),
                };
                if !applies {
                    continue;
                }
                let mut expanded = block.clone();
                expanded.name = format!("{mode:?}").to_lowercase();
                expanded.arguments.clear();
                expanded.properties.remove(KEYBIND_BLOCK_SOURCE);
                if is_shared {
                    // The parser ignores clear-defaults on shared blocks.
                    expanded.properties.remove("clear-defaults");
                }
                expanded.children.clear();
                for kind in ["bind", "unbind"] {
                    for binding in block.children.iter().filter(|child| child.name == kind) {
                        expanded.children.extend(single_key_bindings(binding)?);
                    }
                }
                blocks.push(expanded);
            }
        }
    }
    if let Some(unbind) = root.children.iter().find(|child| child.name == "unbind") {
        for mode in InputMode::iter() {
            blocks.push(Node {
                name: format!("{mode:?}").to_lowercase(),
                arguments: vec![],
                properties: Default::default(),
                children: single_key_bindings(unbind)?,
            });
        }
    }
    Ok(blocks)
}

fn single_key_bindings(binding: &Node) -> Result<Vec<Node>, String> {
    let mut arguments = binding.arguments.clone();
    normalize_chords(&mut arguments)?;
    Ok(arguments
        .into_iter()
        .map(|argument| Node {
            arguments: vec![argument],
            ..binding.clone()
        })
        .collect())
}

fn legacy_binding_updates(
    updates: &mut Vec<Patch>,
    parent: &[String],
    binding: &Node,
) -> Result<(), String> {
    for mut node in single_key_bindings(binding)? {
        node.name = "bind".into();
        let mut path = parent.to_vec();
        path.push(node_key(&node));
        updates.push(Patch {
            path,
            node: (binding.name == "bind").then_some(node),
            reset: false,
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/editor.rs"]
mod tests;

#[derive(Clone, Debug, PartialEq)]
pub struct Disk {
    pub overlay: std::path::PathBuf,
    pub files: std::collections::BTreeMap<std::path::PathBuf, Option<String>>,
    primary: Option<std::path::PathBuf>,
    theme_files: std::collections::BTreeMap<std::path::PathBuf, String>,
}

impl Disk {
    pub fn read(
        config_path: Option<&std::path::Path>,
        config_dir: Option<&std::path::Path>,
    ) -> Result<Self, String> {
        let dir = config_dir
            .map(std::path::Path::to_path_buf)
            .or_else(yinz_runtime::home::find_default_config_dir)
            .ok_or("No configuration directory is available")?;
        let overlay = yinz_runtime::home::get_config_overlay_dir(Some(dir.clone()))
            .ok_or("No overlay directory")?;
        let primary = Some(
            config_path
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| dir.join(crate::config::DEFAULT_CONFIG_FILE_NAME)),
        );
        let mut paths = vec![overlay.join(crate::config::MANAGED_CONFIG_FILE_NAME)];
        if let Some(path) = &primary {
            paths.push(path.clone());
        }
        match std::fs::read_dir(&overlay) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry.map_err(|e| e.to_string())?.path();
                    if path.extension().and_then(|v| v.to_str()) == Some("kdl") {
                        paths.push(path);
                    }
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(format!("Cannot read {}: {e}", overlay.display())),
        }
        let mut files = std::collections::BTreeMap::new();
        for path in paths {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
            };
            files.insert(path, text);
        }
        let configured = Config::load(config_path, Some(&dir), false).map_err(validation_error)?;
        let theme_dir = configured
            .options
            .theme_dir
            .unwrap_or_else(|| dir.join("themes"));
        let mut theme_files = std::collections::BTreeMap::new();
        match std::fs::read_dir(&theme_dir) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry
                        .map_err(|e| format!("Cannot read {}: {e}", theme_dir.display()))?
                        .path();
                    if path.extension().and_then(|v| v.to_str()) == Some("kdl") {
                        let source = std::fs::read_to_string(&path)
                            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
                        theme_files.insert(path, source);
                    }
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(format!("Cannot read {}: {e}", theme_dir.display())),
        }
        Ok(Self {
            overlay,
            files,
            primary,
            theme_files,
        })
    }

    pub fn preview(
        &self,
        patches: &[Patch],
        reset: &[Vec<String>],
    ) -> Result<(String, Config, Config), String> {
        let managed_path = self.overlay.join(crate::config::MANAGED_CONFIG_FILE_NAME);
        let mut managed: KdlDocument = self
            .files
            .get(&managed_path)
            .and_then(|s| s.as_deref())
            .unwrap_or("")
            .parse()
            .map_err(|e: kdl::KdlError| e.to_string())?;
        let mut fragment = KdlDocument::new();
        let mut edits = patches_to_kdl(patches)?;
        for path in reset {
            if path.is_empty() {
                return Err("Reset must name a target".into());
            }
            for path in expand_binding_paths(path)? {
                let mut node = KdlNode::new("reset");
                for part in &path {
                    node.push(part.clone());
                }
                edits.ensure_children().nodes_mut().push(node);
            }
        }
        fragment.nodes_mut().push(edits);
        crate::config::merge_kdl_fragment(&mut managed, &fragment);
        let mut config = Config::from_default_assets().map_err(|e| e.to_string())?;
        if let Some(source) = self
            .primary
            .as_ref()
            .and_then(|p| self.files.get(p))
            .and_then(|s| s.as_ref())
        {
            config = Config::from_kdl(source, Some(config)).map_err(validation_error)?;
        }
        crate::config::apply_themes(
            &mut config,
            None,
            self.overlay.parent().map(std::path::Path::to_path_buf),
        )
        .map_err(|e| e.to_string())?;
        let mut underlay = config.clone();
        for (path, source) in &self.files {
            if self.primary.as_ref() == Some(path) {
                continue;
            }
            let replacement;
            let source = if path == &managed_path {
                underlay = config.clone();
                replacement = managed.to_string();
                Some(&replacement)
            } else {
                source.as_ref()
            };
            if let Some(source) = source {
                config = Config::from_kdl(source, Some(config))
                    .map_err(|e| format!("{}: {}", path.display(), validation_error(e)))?;
            }
        }
        let (_, mut config) = crate::config::load_layout_and_config(
            None,
            config,
            self.overlay.parent().map(std::path::Path::to_path_buf),
            self.primary.as_deref(),
            None,
            None,
            std::path::PathBuf::from("."),
        )
        .map_err(validation_error)?;
        crate::config::apply_themes(
            &mut config,
            None,
            self.overlay.parent().map(std::path::Path::to_path_buf),
        )
        .map_err(validation_error)?;
        Ok((fragment.to_string(), config, underlay))
    }
}

/// Action spellings accepted by the configuration parser.
pub const ACTION_NAMES: &[&str] = &[
    "Quit",
    "FocusNextPane",
    "FocusPreviousPane",
    "FocusPaneTabs",
    "PreviousPaneTab",
    "NextPaneTab",
    "FocusLastPane",
    "FocusHostSession",
    "FocusGuestSession",
    "ToggleHostFullscreen",
    "SwitchFocus",
    "ScrollUp",
    "SwapTextSelectionEnds",
    "ActivateNotification",
    "ActivateLastNotification",
    "ToggleNotificationDetail",
    "ScrollDown",
    "ScrollToBottom",
    "ScrollToTop",
    "ScrollToPreviousPrompt",
    "ScrollToNextPrompt",
    "SelectCommandAtScrollPosition",
    "CopyLastCommandOutput",
    "PageScrollUp",
    "PageScrollDown",
    "HalfPageScrollUp",
    "HalfPageScrollDown",
    "ToggleFocusFullscreen",
    "ToggleFocusNoUiFullscreen",
    "TogglePaneFrames",
    "ToggleCommandPalette",
    "ToggleMother",
    "ToggleActiveSyncTab",
    "TogglePaneEmbedOrFloating",
    "ToggleFloatingPanes",
    "CloseFocus",
    "UndoRenamePane",
    "NoOp",
    "GoToNextTab",
    "GoToPreviousTab",
    "NewTabGroup",
    "GoToNextTabGroup",
    "MovePaneToNextTabGroup",
    "MovePaneToPreviousTabGroup",
    "GoToPreviousTabGroup",
    "GoToTabInGroup",
    "SwitchTabGroup",
    "RenameTabGroup",
    "CloseTabGroup",
    "CloseTab",
    "ToggleTab",
    "UndoRenameTab",
    "ToggleMouseMode",
    "Detach",
    "SetDarkTheme",
    "SetLightTheme",
    "ToggleTheme",
    "Copy",
    "Clear",
    "Confirm",
    "Deny",
    "EditScrollback",
    "ShowFloatingPanes",
    "HideFloatingPanes",
    "SwitchSession",
    "Write",
    "WriteChars",
    "SwitchToMode",
    "SetPaneFrameStyle",
    "Search",
    "Resize",
    "MoveFocus",
    "MoveTextSelection",
    "MoveNotificationSelection",
    "MoveTab",
    "MoveFocusOrTab",
    "MovePane",
    "MovePaneBackwards",
    "DumpScreen",
    "DumpLayout",
    "NewPane",
    "PaneNameInput",
    "NewTab",
    "OverrideLayout",
    "GoToTab",
    "TabNameInput",
    "SearchInput",
    "SearchToggleOption",
    "Run",
    "LaunchOrFocusPlugin",
    "LaunchPlugin",
    "PreviousSwapLayout",
    "NextSwapLayout",
    "ArrangePanesByProject",
    "BreakPane",
    "BreakPaneRight",
    "BreakPaneLeft",
    "RenameSession",
    "Notify",
    "MessagePlugin",
    "MessagePluginId",
    "TogglePanePinned",
    "TogglePaneInGroup",
    "ToggleGroupMarking",
];

pub fn action_templates(config: &Config) -> Result<Vec<Node>, String> {
    let mut bindings = config.clone();
    bindings.themes = Default::default();
    let nodes = document(&bindings)?;
    fn find(nodes: &[Node], name: &str) -> Option<Node> {
        for node in nodes {
            if node.name == name {
                return Some(node.clone());
            }
            if let Some(node) = find(&node.children, name) {
                return Some(node);
            }
        }
        None
    }
    Ok(ACTION_NAMES
        .iter()
        .map(|name| {
            if let Some(node) = find(&nodes, name) {
                return node;
            }
            let mut node = Node {
                name: (*name).into(),
                arguments: vec![],
                properties: Default::default(),
                children: vec![],
            };
            let candidates = [
                vec![],
                vec![Value::from("normal")],
                vec![Value::from("left")],
                vec![Value::from("up")],
                vec![Value::from("full")],
                vec![Value::from("CaseSensitivity")],
                vec![Value::from(1)],
                vec![Value::from(1), Value::from("Group")],
                vec![Value::from("")],
            ];
            for args in candidates {
                node.arguments = args;
                if let Ok(kdl) = to_kdl(&node, 0, None)
                    && crate::kdl::action::from_kdl((&kdl, &config.options)).is_ok()
                {
                    return node;
                }
            }
            if *name == "SwitchSession" {
                node.arguments.clear();
                node.properties
                    .insert("name".into(), Value::from("session"));
            }
            node
        })
        .collect())
}

pub const SECTIONS: &[&str] = &[
    "themes",
    "keybinds",
    "env",
    "plugins",
    "load_plugins",
    "overlay_tab_plugins",
    "hooks",
    "context_menu",
    "ui",
];

pub fn templates(config: &Config) -> Result<std::collections::BTreeMap<String, Vec<Node>>, String> {
    fn node(name: &str, arguments: Vec<Value>, children: Vec<Node>) -> Node {
        Node {
            name: name.into(),
            arguments,
            children,
            properties: Default::default(),
        }
    }
    let mut result = std::collections::BTreeMap::new();
    result.insert(
        "env".into(),
        vec![node("NAME", vec![Value::from("")], vec![])],
    );
    result.insert(
        "plugins".into(),
        vec![Node {
            properties: [("location".into(), Value::from("file:/path/to/plugin.scm"))].into(),
            ..node("new-plugin", vec![], vec![])
        }],
    );
    result.insert(
        "load_plugins".into(),
        std::iter::once(node("file:/path/to/plugin.scm", vec![], vec![]))
            .chain(
                config
                    .plugins
                    .aliases
                    .keys()
                    .map(|name| node(name, vec![], vec![])),
            )
            .collect(),
    );
    result.insert(
        "overlay_tab_plugins".into(),
        std::iter::once(node("file:/path/to/plugin.scm", vec![], vec![]))
            .chain(
                config
                    .plugins
                    .aliases
                    .keys()
                    .map(|name| node(name, vec![], vec![])),
            )
            .collect(),
    );
    result.insert(
        "hooks".into(),
        yinz_types::input::hooks::HookPoint::ALL
            .iter()
            .map(|p| node(p.as_str(), vec![Value::from("")], vec![]))
            .collect(),
    );
    for root in ["plugins", "load_plugins", "hooks"] {
        result.insert(
            format!("{root}/*"),
            vec![
                node("cwd", vec![Value::from("")], vec![]),
                node("key", vec![Value::from("")], vec![]),
            ],
        );
    }
    let frame_fields = vec![
        node("rounded_corners", vec![Value::from(false)], vec![]),
        node("hide_session_name", vec![Value::from(false)], vec![]),
    ];
    result.insert(
        "ui".into(),
        vec![node("pane_frames", vec![], frame_fields.clone())],
    );
    result.insert("ui/pane_frames".into(), frame_fields);
    result.insert(
        "keybinds".into(),
        catalog()
            .into_iter()
            .find(|s| s.name == "default_mode")
            .into_iter()
            .flat_map(|s| s.choices)
            .map(|mode| node(mode, vec![], vec![]))
            .collect(),
    );
    result.insert(
        "keybinds/*".into(),
        vec![node(
            "bind",
            vec![Value::from("Ctrl x")],
            vec![node("NoOp", vec![], vec![])],
        )],
    );
    let actions = action_templates(config)?;
    result.insert("keybinds/*/*".into(), actions.clone());
    result.insert(
        "context_menu".into(),
        vec![
            node("New item", vec![], vec![node("Copy", vec![], vec![])]),
            node("separator", vec![], vec![]),
        ],
    );
    result.insert("context_menu/*".into(), actions);
    if let Some(theme) = config.themes.get_theme("default").or_else(|| {
        config
            .themes
            .inner()
            .iter()
            .min_by_key(|(name, _)| *name)
            .map(|(_, theme)| theme)
    }) {
        let mut example = Config::default();
        example.themes.insert("new-theme".into(), theme.clone());
        if let Some(theme) = document(&example)?
            .into_iter()
            .find(|n| n.name == "themes")
            .and_then(|n| n.children.into_iter().next())
        {
            let mut components = theme.children.clone();
            if !components
                .iter()
                .any(|node| node.name == "frame_unselected")
                && let Some(mut frame) = components
                    .iter()
                    .find(|node| node.name == "frame_selected")
                    .cloned()
            {
                frame.name = "frame_unselected".into();
                components.push(frame);
            }
            for component in &components {
                result.insert(
                    format!("themes/*/{}", component.name),
                    component.children.clone(),
                );
            }
            result.insert("themes/*".into(), components);
            result.insert("themes".into(), vec![theme]);
        }
    }
    Ok(result)
}

pub fn find_node<'a>(nodes: &'a [Node], path: &[String]) -> Option<&'a Node> {
    let (first, rest) = path.split_first()?;
    let node = nodes.iter().find(|n| &node_key(n) == first)?;
    if rest.is_empty() {
        Some(node)
    } else {
        find_node(&node.children, rest)
    }
}

// Clone KDL bindings so empty action/configuration blocks survive a reset.
fn split_reset_binding(binding: &KdlNode, key: &str) -> Result<Vec<KdlNode>, String> {
    let bindings = single_key_bindings(&from_kdl(binding))?;
    if !bindings.iter().any(|binding| node_key(binding) == key) {
        return Ok(vec![binding.clone()]);
    }
    Ok(bindings
        .into_iter()
        .map(|single| {
            let mut node = binding.clone();
            node.entries_mut().retain(|entry| entry.name().is_some());
            for argument in single.arguments {
                if let Some(key) = argument.as_str() {
                    node.push(key);
                }
            }
            node
        })
        .collect())
}

fn reset_keybind_blocks(root: &KdlNode, path: &[String]) -> Result<Vec<KdlNode>, String> {
    use strum::IntoEnumIterator;
    use yinz_types::data::InputMode;

    let mode = path[0].parse::<InputMode>().map_err(|e| e.to_string())?;
    let children = root.children().ok_or("Keybinds have no children")?;
    let global_unbind = children.get("unbind");
    let reset_global = match global_unbind {
        Some(unbind) => {
            path.len() == 1
                || single_key_bindings(&from_kdl(unbind))?
                    .iter()
                    .any(|binding| node_key(binding) == path[1])
        },
        None => false,
    };
    let mut blocks = vec![];
    for block in children.nodes() {
        let shared = matches!(
            block.name().value(),
            "shared" | "shared_among" | "shared_except"
        );
        let listed = block
            .entries()
            .iter()
            .filter(|entry| entry.name().is_none())
            .any(|entry| {
                entry
                    .value()
                    .as_string()
                    .and_then(|name| name.parse::<InputMode>().ok())
                    == Some(mode)
            });
        let applies = match block.name().value() {
            "shared_among" => listed,
            "shared" | "shared_except" => !listed,
            _ => block.name().value().parse::<InputMode>().ok() == Some(mode),
        };
        if block.name().value() == "unbind" {
            if !reset_global {
                blocks.push(block.clone());
            }
            continue;
        }
        if !applies {
            blocks.push(block.clone());
            continue;
        }
        let mut target = block.clone();
        if path.len() > 1 {
            let bindings = target
                .ensure_children()
                .nodes()
                .iter()
                .map(|binding| split_reset_binding(binding, &path[1]))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();
            *target.ensure_children().nodes_mut() = bindings;
        }
        let mut rewritten = target.clone();
        if path.len() > 1 {
            reset_managed_path(rewritten.ensure_children(), &path[1..]);
            if rewritten == target {
                blocks.push(block.clone());
                continue;
            }
        }
        if shared {
            // Preserve the original block identity for subsequent legacy writes.
            if target.get(KEYBIND_BLOCK_SOURCE).is_none() {
                let arguments: Vec<_> = target
                    .entries()
                    .iter()
                    .filter(|entry| entry.name().is_none())
                    .filter_map(|entry| entry.value().as_string())
                    .collect();
                let source = serde_json::to_string(&(target.name().value(), arguments))
                    .map_err(|e| e.to_string())?;
                target.insert(KEYBIND_BLOCK_SOURCE, source.clone());
                rewritten.insert(KEYBIND_BLOCK_SOURCE, source);
            }
            let mut remaining = target.clone();
            if path.len() > 1 {
                remaining
                    .ensure_children()
                    .nodes_mut()
                    .retain(|child| node_key(&from_kdl(child)) == path[1]);
            }
            if remaining.name().value() == "shared_among" {
                remaining.entries_mut().retain(|entry| {
                    entry.name().is_some()
                        || entry
                            .value()
                            .as_string()
                            .and_then(|name| name.parse::<InputMode>().ok())
                            != Some(mode)
                });
            } else {
                remaining.push(format!("{mode:?}").to_lowercase());
            }
            target = rewritten;
            if path.len() > 2 {
                let mut changed = target.clone();
                changed
                    .ensure_children()
                    .nodes_mut()
                    .retain(|child| node_key(&from_kdl(child)) == path[1]);
                changed.set_name("shared_among");
                changed.entries_mut().retain(|entry| entry.name().is_some());
                changed.push(format!("{mode:?}").to_lowercase());
                target
                    .ensure_children()
                    .nodes_mut()
                    .retain(|child| node_key(&from_kdl(child)) != path[1]);
                blocks.push(changed);
            }
            // Unbinds must still run after binds from the same original block.
            if path.get(1).is_some_and(|key| key.starts_with("unbind:")) {
                blocks.push(target);
                blocks.push(remaining);
                continue;
            }
            blocks.push(remaining);
        } else {
            target = rewritten;
        }
        if path.len() > 1 {
            blocks.push(target);
        }
    }
    // Only the first global unbind is effective, after all shared and explicit blocks.
    if let Some(unbind) = global_unbind.filter(|_| reset_global) {
        let arguments: Vec<_> = unbind
            .entries()
            .iter()
            .filter(|entry| entry.name().is_none())
            .filter_map(|entry| entry.value().as_string())
            .collect();
        let source = serde_json::to_string(&("unbind", arguments)).map_err(|e| e.to_string())?;
        for unbind_mode in InputMode::iter() {
            if unbind_mode == mode && path.len() == 1 {
                continue;
            }
            let mut block = KdlNode::new(format!("{unbind_mode:?}").to_lowercase());
            block.insert(KEYBIND_BLOCK_SOURCE, source.clone());
            let bindings = if unbind_mode == mode && path.len() > 1 {
                split_reset_binding(unbind, &path[1])?
            } else {
                vec![unbind.clone()]
            };
            *block.ensure_children().nodes_mut() = bindings;
            if unbind_mode == mode {
                reset_managed_path(block.ensure_children(), &path[1..]);
            }
            blocks.push(block);
        }
    }
    Ok(blocks)
}

fn valid_keybind_structure(root: &KdlNode) -> bool {
    use yinz_types::{data::InputMode, keys::KeyWithModifier};

    fn valid_binding(binding: &KdlNode) -> bool {
        matches!(binding.name().value(), "bind" | "unbind")
            && (binding.name().value() != "bind" || binding.children().is_some())
            && binding.entries().iter().all(|entry| {
                entry
                    .value()
                    .as_string()
                    .is_some_and(|key| key.parse::<KeyWithModifier>().is_ok())
            })
    }

    root.children().is_some_and(|children| {
        children.nodes().iter().all(|block| {
            if keybind_block_source(block).is_err() {
                return false;
            }
            if block.name().value() == "unbind" {
                return valid_binding(block);
            }
            let valid_mode = match block.name().value() {
                "shared" | "shared_among" | "shared_except" => {
                    block.entries().iter().all(|entry| {
                        entry
                            .name()
                            .is_some_and(|name| name.value() == KEYBIND_BLOCK_SOURCE)
                            || entry
                                .value()
                                .as_string()
                                .is_some_and(|mode| mode.parse::<InputMode>().is_ok())
                    })
                },
                mode => mode.parse::<InputMode>().is_ok(),
            };
            valid_mode
                && block
                    .children()
                    .is_some_and(|children| children.nodes().iter().all(valid_binding))
        })
    })
}

pub fn reset_managed_path(document: &mut KdlDocument, path: &[String]) {
    if path.len() > 1
        && path[0] == "keybinds"
        && path[1].parse::<yinz_types::data::InputMode>().is_ok()
    {
        for node in document
            .nodes_mut()
            .iter_mut()
            .filter(|node| node.name().value() == "keybinds")
        {
            // Keep invalid fragments intact so the loader rejects them before writing.
            if !valid_keybind_structure(node) {
                continue;
            }
            if let Ok(blocks) = reset_keybind_blocks(node, &path[1..]) {
                *node.ensure_children().nodes_mut() = blocks;
            }
        }
        return;
    }
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        document
            .nodes_mut()
            .retain(|node| node_key(&from_kdl(node)) != *first);
    } else {
        for node in document
            .nodes_mut()
            .iter_mut()
            .filter(|node| node_key(&from_kdl(node)) == *first)
        {
            if let Some(children) = node.children_mut().as_mut() {
                reset_managed_path(children, rest);
            }
        }
    }
}

pub fn action_properties(
) -> std::collections::BTreeMap<&'static str, std::collections::BTreeMap<&'static str, Value>> {
    [
        ("EditScrollback", [("ansi", Value::from(false))].into()),
        (
            "SwitchSession",
            [
                ("name", Value::from("")),
                ("tab_position", Value::from(0)),
                ("pane_id", Value::from(0)),
                ("is_plugin", Value::from(false)),
                ("layout", Value::from("")),
                ("cwd", Value::from("")),
            ]
            .into(),
        ),
        ("LaunchOrFocusPlugin", [("cwd", Value::from(""))].into()),
        (
            "MessagePlugin",
            [("in_place", Value::from(false)), ("cwd", Value::from(""))].into(),
        ),
        ("MessagePluginId", [("in_place", Value::from(false))].into()),
    ]
    .into()
}

pub fn action_children() -> std::collections::BTreeMap<String, Vec<Node>> {
    [
        (
            "NewTab",
            vec![
                ("layout", Value::from("")),
                ("should_change_focus_to_new_tab", Value::from(false)),
                ("cwd", Value::from("")),
                ("name", Value::from("")),
            ],
        ),
        (
            "OverrideLayout",
            vec![
                ("layout", Value::from("")),
                ("cwd", Value::from("")),
                ("name", Value::from("")),
                ("retain_existing_terminal_panes", Value::from(false)),
                ("retain_existing_plugin_panes", Value::from(false)),
                ("apply_only_to_active_tab", Value::from(false)),
            ],
        ),
        (
            "Run",
            vec![
                ("cwd", Value::from("")),
                ("name", Value::from("")),
                ("direction", Value::from("")),
                ("close_on_exit", Value::from(false)),
                ("start_suspended", Value::from(false)),
                ("floating", Value::from(false)),
                ("in_place", Value::from(false)),
                ("close_replaced_pane", Value::from(false)),
                ("stacked", Value::from(false)),
                ("x", Value::from("")),
                ("y", Value::from("")),
                ("width", Value::from("")),
                ("height", Value::from("")),
                ("pinned", Value::from(false)),
                ("borderless", Value::from(false)),
            ],
        ),
        (
            "LaunchOrFocusPlugin",
            vec![
                ("floating", Value::from(false)),
                ("overlay", Value::from(false)),
                ("move_to_focused_tab", Value::from(false)),
                ("in_place", Value::from(false)),
                ("close_replaced_pane", Value::from(false)),
                ("skip_plugin_cache", Value::from(false)),
            ],
        ),
        (
            "LaunchPlugin",
            vec![
                ("floating", Value::from(false)),
                ("in_place", Value::from(false)),
                ("close_replaced_pane", Value::from(false)),
                ("skip_plugin_cache", Value::from(false)),
            ],
        ),
        (
            "MessagePlugin",
            vec![
                ("launch_new", Value::from(false)),
                ("skip_cache", Value::from(false)),
                ("floating", Value::from(false)),
                ("name", Value::from("")),
                ("payload", Value::from("")),
                ("title", Value::from("")),
            ],
        ),
        (
            "MessagePluginId",
            vec![("name", Value::from("")), ("payload", Value::from(""))],
        ),
    ]
    .into_iter()
    .map(|(action, fields)| {
        (
            action.into(),
            fields
                .into_iter()
                .map(|(name, value)| Node {
                    name: name.into(),
                    arguments: vec![value],
                    properties: Default::default(),
                    children: vec![],
                })
                .collect(),
        )
    })
    .collect()
}
