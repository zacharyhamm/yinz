use super::*;
use yinz_types::input::actions::ActionKind;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;
pub fn new_from_bytes(
    action_name: &str,
    bytes: Vec<u8>,
    action_node: &KdlNode,
) -> Result<Action, ConfigError> {
    match action_name {
        "Write" => Ok(Action::Write {
            key_with_modifier: None,
            bytes,
            is_kitty_keyboard_protocol: false,
        }),
        "PaneNameInput" => Ok(Action::PaneNameInput { input: bytes }),
        "TabNameInput" => Ok(Action::TabNameInput { input: bytes }),
        "SearchInput" => Ok(Action::SearchInput { input: bytes }),
        _ => Err(ConfigError::new_kdl_error(
            "Failed to parse action".into(),
            action_node.span().offset(),
            action_node.span().len(),
        )),
    }
}
pub fn new_from_string(
    action_name: &str,
    string: String,
    action_node: &KdlNode,
) -> Result<Action, ConfigError> {
    match action_name {
        "WriteChars" => Ok(Action::WriteChars { chars: string }),
        "SetPaneFrameStyle" => {
            let style = PaneFrameStyle::from_str(string.as_str()).map_err(|e| {
                ConfigError::new_kdl_error(
                    format!("{}", e),
                    action_node.span().offset(),
                    action_node.span().len(),
                )
            })?;
            Ok(Action::SetPaneFrameStyle(style))
        },
        "SwitchToMode" => match InputMode::from_str(string.as_str()) {
            Ok(input_mode) => Ok(Action::SwitchToMode { input_mode }),
            Err(_e) => Err(ConfigError::new_kdl_error(
                format!("Unknown InputMode '{}'", string),
                action_node.span().offset(),
                action_node.span().len(),
            )),
        },
        "Resize" => {
            let mut resize: Option<Resize> = None;
            let mut direction: Option<Direction> = None;
            for word in string.to_ascii_lowercase().split_whitespace() {
                match Resize::from_str(word) {
                    Ok(value) => resize = Some(value),
                    Err(_) => match Direction::from_str(word) {
                        Ok(value) => direction = Some(value),
                        Err(_) => {
                            return Err(ConfigError::new_kdl_error(
                                format!(
                                    "failed to read either of resize type or direction from '{}'",
                                    word
                                ),
                                action_node.span().offset(),
                                action_node.span().len(),
                            ))
                        },
                    },
                }
            }
            let resize = resize.unwrap_or(Resize::Increase);
            Ok(Action::Resize { resize, direction })
        },
        "MoveFocus" => {
            let direction = Direction::from_str(string.as_str()).map_err(|_| {
                ConfigError::new_kdl_error(
                    format!("Invalid direction: '{}'", string),
                    action_node.span().offset(),
                    action_node.span().len(),
                )
            })?;
            Ok(Action::MoveFocus { direction })
        },
        "MoveTextSelection" => {
            let direction = Direction::from_str(string.as_str()).map_err(|_| {
                ConfigError::new_kdl_error(
                    format!("Invalid direction: '{}'", string),
                    action_node.span().offset(),
                    action_node.span().len(),
                )
            })?;
            Ok(Action::MoveTextSelection { direction })
        },
        "MoveNotificationSelection" => {
            let direction = Direction::from_str(string.as_str()).map_err(|_| {
                ConfigError::new_kdl_error(
                    format!("Invalid direction: '{}'", string),
                    action_node.span().offset(),
                    action_node.span().len(),
                )
            })?;
            Ok(Action::MoveNotificationSelection { direction })
        },
        "MoveFocusOrTab" => {
            let direction = Direction::from_str(string.as_str()).map_err(|_| {
                ConfigError::new_kdl_error(
                    format!("Invalid direction: '{}'", string),
                    action_node.span().offset(),
                    action_node.span().len(),
                )
            })?;
            Ok(Action::MoveFocusOrTab { direction })
        },
        "MoveTab" => {
            let direction = Direction::from_str(string.as_str()).map_err(|_| {
                ConfigError::new_kdl_error(
                    format!("Invalid direction: '{}'", string),
                    action_node.span().offset(),
                    action_node.span().len(),
                )
            })?;
            if direction.is_vertical() {
                Err(ConfigError::new_kdl_error(
                    format!("Invalid horizontal direction: '{}'", string),
                    action_node.span().offset(),
                    action_node.span().len(),
                ))
            } else {
                Ok(Action::MoveTab { direction })
            }
        },
        "MovePane" => {
            if string.is_empty() {
                Ok(Action::MovePane { direction: None })
            } else {
                let direction = Direction::from_str(string.as_str()).map_err(|_| {
                    ConfigError::new_kdl_error(
                        format!("Invalid direction: '{}'", string),
                        action_node.span().offset(),
                        action_node.span().len(),
                    )
                })?;
                Ok(Action::MovePane {
                    direction: Some(direction),
                })
            }
        },
        "MovePaneBackwards" => Ok(Action::MovePaneBackwards),
        "DumpScreen" => Ok(Action::DumpScreen {
            file_path: Some(string),
            include_scrollback: false,
            pane_id: None,
            ansi: false,
        }),
        "DumpLayout" => Ok(Action::DumpLayout),
        "NewPane" => {
            if string.is_empty() {
                Ok(Action::NewPane {
                    direction: None,
                    pane_name: None,
                    start_suppressed: false,
                })
            } else if string == "stacked" {
                Ok(Action::NewStackedPane {
                    command: None,
                    pane_name: None,
                    near_current_pane: false,
                    no_focus: false,
                    tab_id: None,
                })
            } else {
                let direction = Direction::from_str(string.as_str()).map_err(|_| {
                    ConfigError::new_kdl_error(
                        format!("Invalid direction: '{}'", string),
                        action_node.span().offset(),
                        action_node.span().len(),
                    )
                })?;
                Ok(Action::NewPane {
                    direction: Some(direction),
                    pane_name: None,
                    start_suppressed: false,
                })
            }
        },
        "SearchToggleOption" => {
            let toggle_option = SearchOption::from_str(string.as_str()).map_err(|_| {
                ConfigError::new_kdl_error(
                    format!("Invalid direction: '{}'", string),
                    action_node.span().offset(),
                    action_node.span().len(),
                )
            })?;
            Ok(Action::SearchToggleOption {
                option: toggle_option,
            })
        },
        "Search" => {
            let search_direction = SearchDirection::from_str(string.as_str()).map_err(|_| {
                ConfigError::new_kdl_error(
                    format!("Invalid direction: '{}'", string),
                    action_node.span().offset(),
                    action_node.span().len(),
                )
            })?;
            Ok(Action::Search {
                direction: search_direction,
            })
        },
        "RenameSession" => Ok(Action::RenameSession {
            name: string.into(),
        }),
        "Notify" => Ok(Action::Notify(string)),
        _ => Err(ConfigError::new_kdl_error(
            format!("Unsupported action: {}", action_name),
            action_node.span().offset(),
            action_node.span().len(),
        )),
    }
}
/// Why an `Action` has no KDL form.
///
/// `WireOnly` is a decision about the variant: the CLI and the plugin API can send it, and no
/// config line means it. `Shape` is a decision about one value of a variant that does have a
/// spelling: what the parser would read back from the written line is not this value, so the
/// line is refused rather than written wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotWritable {
    #[error("`{0:?}` can only be sent by the CLI or a plugin; it has no config spelling")]
    WireOnly(ActionKind),
    #[error("`{kind:?}` cannot be written here: {why}")]
    Shape { kind: ActionKind, why: &'static str },
}

fn shape(value: &Action, why: &'static str) -> NotWritable {
    NotWritable::Shape {
        kind: ActionKind::from(value),
        why,
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Left => "left",
        Direction::Right => "right",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}

/// A child node with one argument: `name value`.
fn child(name: &str, value: impl Into<KdlEntry>) -> KdlNode {
    let mut node = KdlNode::new(name);
    node.push(value);
    node
}

fn push_name(children: &mut KdlDocument, name: &Option<String>) {
    if let Some(name) = name {
        children.nodes_mut().push(child("name", name.clone()));
    }
}

fn with_children(mut node: KdlNode, children: KdlDocument) -> KdlNode {
    if !children.nodes().is_empty() {
        node.set_children(children);
    }
    node
}

/// The parts of a `RunCommandAction` the `Run` parser cannot read back, as the reason to refuse
/// it; `Ok` when every field survives the trip.
fn run_command_is_writable(run_command_action: &RunCommandAction) -> Result<(), &'static str> {
    if run_command_action.originating_plugin.is_some() {
        return Err(
            "a command a plugin launched carries that plugin's identity, which no config line does",
        );
    }
    if run_command_action.use_terminal_title {
        return Err("`use_terminal_title` has no config spelling");
    }
    if run_command_action.remote_default_shell {
        return Err("`remote_default_shell` has no config spelling");
    }
    match &run_command_action.target {
        PaneTarget::Local => Ok(()),
        PaneTarget::Ssh { .. } => Err("a `Run` line cannot name an ssh host"),
        PaneTarget::Bubblewrap { .. } => Err("a `Run` line cannot name a Bubblewrap environment"),
    }
}

/// The `Run` node every command form shares: the command and its arguments, then the children
/// the parser reads back - `cwd`, `start_suspended`, `close_on_exit`
/// and `direction`. One writer for the four forms so that they cannot drift apart again: they
/// did, and wrote `hold_on_start`/`hold_on_close`, two names the parser has never read.
fn run_command_to_kdl(
    value: &Action,
    run_command_action: &RunCommandAction,
) -> Result<(KdlNode, KdlDocument), NotWritable> {
    run_command_is_writable(run_command_action).map_err(|why| shape(value, why))?;
    let mut node = KdlNode::new("Run");
    node.push(run_command_action.command.display().to_string());
    for arg in &run_command_action.args {
        node.push(arg.clone());
    }
    let mut children = KdlDocument::new();
    if let Some(cwd) = &run_command_action.cwd {
        children
            .nodes_mut()
            .push(child("cwd", cwd.display().to_string()));
    }
    if run_command_action.hold_on_start {
        children.nodes_mut().push(child("start_suspended", true));
    }
    if !run_command_action.hold_on_close {
        children.nodes_mut().push(child("close_on_exit", true));
    }
    if let Some(direction) = run_command_action.direction {
        children
            .nodes_mut()
            .push(child("direction", direction_name(direction)));
    }
    Ok((node, children))
}

/// The `x`/`y`/`width`/`height`/`pinned`/`borderless` children of a floating `Run`, written the
/// way `FloatingPaneCoordinates::new` reads them: sizes as strings, a zero extent refused
/// because it is read back as unset.
fn push_floating_coordinates(
    value: &Action,
    coordinates: &FloatingPaneCoordinates,
    children: &mut KdlDocument,
) -> Result<(), NotWritable> {
    if *coordinates == FloatingPaneCoordinates::default() {
        return Err(shape(
            value,
            "empty floating coordinates are read back as no coordinates",
        ));
    }
    let sizes = [
        ("x", coordinates.x, false),
        ("y", coordinates.y, false),
        ("width", coordinates.width, true),
        ("height", coordinates.height, true),
    ];
    for (name, size, is_extent) in sizes {
        let Some(size) = size else {
            continue;
        };
        let written = match size {
            PercentOrFixed::Percent(percent) => {
                if percent > 100 {
                    return Err(shape(value, "a percentage above 100 is not read back"));
                }
                format!("{percent}%")
            },
            PercentOrFixed::Fixed(fixed) => fixed.to_string(),
        };
        if is_extent && matches!(size, PercentOrFixed::Percent(0) | PercentOrFixed::Fixed(0)) {
            return Err(shape(value, "a zero width or height is read back as unset"));
        }
        children.nodes_mut().push(child(name, written));
    }
    if let Some(pinned) = coordinates.pinned {
        children.nodes_mut().push(child("pinned", pinned));
    }
    if let Some(borderless) = coordinates.borderless {
        children.nodes_mut().push(child("borderless", borderless));
    }
    Ok(())
}

/// A plugin's own initial cwd - the `cwd` child `LaunchOrFocusPlugin` reads into it.
fn plugin_initial_cwd(plugin: &RunPluginOrAlias) -> Option<&PathBuf> {
    match plugin {
        RunPluginOrAlias::RunPlugin(run_plugin) => run_plugin.initial_cwd.as_ref(),
        RunPluginOrAlias::Alias(alias) => alias.initial_cwd.as_ref(),
    }
}

fn push_plugin_flag(children: &mut KdlDocument, name: &str, set: bool) {
    if set {
        children.nodes_mut().push(child(name, true));
    }
}

/// The plugin's configuration as the children a launch action reads it back from: an alias's own
/// configuration, which is all an alias holds until it is resolved, else the plugin's.
fn push_plugin_configuration(children: &mut KdlDocument, run_plugin_or_alias: &RunPluginOrAlias) {
    let configuration = match run_plugin_or_alias {
        RunPluginOrAlias::RunPlugin(plugin) => Some(&plugin.configuration),
        RunPluginOrAlias::Alias(alias) => alias.configuration.as_ref(),
    };
    for (config_key, config_value) in configuration.into_iter().flat_map(|c| c.inner().iter()) {
        let mut node = KdlNode::new(config_key.clone());
        if matches!(
            config_key.as_str(),
            "overlay" | "modal" | "awaits_placement"
        ) && config_value == "true"
        {
            node.push(KdlValue::Bool(true));
        } else {
            node.push(config_value.clone());
        }
        children.nodes_mut().push(node);
    }
}

/// The reason `round_trips` gives. Named so that a test can tell the backstop's refusal from an
/// arm's: a value refused for this reason is a shape nobody has described yet.
pub const NOT_READ_BACK: &str = "its written form does not load back as the same action";

/// Whether reading `node`'s text back gives `value`: the contract behind every `Ok` this module
/// returns. The explicit arms above name the shapes that are known not to survive the trip;
/// this is the backstop for the ones nobody has named yet, so that they are refused rather
/// than written wrong. Not applied to the actions whose parser loads a layout from disk.
fn round_trips(value: &Action, node: KdlNode) -> Result<KdlNode, NotWritable> {
    let reread = node
        .to_string()
        .parse::<KdlDocument>()
        .ok()
        .and_then(|document| document.nodes().first().cloned())
        .and_then(|node| from_kdl((&node, &Options::default())).ok());
    if reread.as_ref() == Some(value) {
        Ok(node)
    } else {
        Err(shape(value, NOT_READ_BACK))
    }
}

/// The KDL line for `value`, or why there is none. `Ok` means the line reads back as `value`.
pub fn to_kdl(value: &Action) -> Result<KdlNode, NotWritable> {
    let node = spell(value)?;
    match value {
        // Reading these loads a layout - the default one, or the one they name - so what they
        // read back is a property of the disk, checked against the source when the config is
        // written (`Config::write_config_to_disk`) rather than here.
        Action::NewTab { .. } | Action::SwitchSession { .. } => Ok(node),
        _ => round_trips(value, node),
    }
}

/// The spelling of each variant. No wildcard arm: a new `Action` variant does not compile until
/// it has been given a line here or listed as wire-only, which is what keeps a binding from
/// silently vanishing when the config is saved.
#[deny(clippy::wildcard_enum_match_arm)]
fn spell(value: &Action) -> Result<KdlNode, NotWritable> {
    match value {
        Action::Quit => Ok(KdlNode::new("Quit")),
        Action::Write {
            key_with_modifier: None,
            bytes,
            is_kitty_keyboard_protocol: false,
        } => {
            let mut node = KdlNode::new("Write");
            for byte in bytes {
                node.push(KdlValue::Base10(*byte as i64));
            }
            Ok(node)
        },
        Action::Write { .. } => Err(shape(
            value,
            "the key it came from and the kitty keyboard flag have no config spelling",
        )),
        Action::WriteChars { chars: string } => {
            let mut node = KdlNode::new("WriteChars");
            node.push(string.clone());
            Ok(node)
        },
        Action::Notify(text) => {
            let mut node = KdlNode::new("Notify");
            node.push(text.clone());
            Ok(node)
        },
        Action::SwitchToMode { input_mode } => {
            let mut node = KdlNode::new("SwitchToMode");
            node.push(format!("{:?}", input_mode).to_lowercase());
            Ok(node)
        },
        Action::Resize {
            resize,
            direction: resize_direction,
        } => {
            let mut node = KdlNode::new("Resize");
            let resize = match resize {
                Resize::Increase => "Increase",
                Resize::Decrease => "Decrease",
            };
            if let Some(resize_direction) = resize_direction {
                node.push(format!("{} {}", resize, direction_name(*resize_direction)));
            } else {
                node.push(resize.to_string());
            }
            Ok(node)
        },
        Action::FocusNextPane => Ok(KdlNode::new("FocusNextPane")),
        Action::FocusPreviousPane => Ok(KdlNode::new("FocusPreviousPane")),
        Action::FocusPaneTabs => Ok(KdlNode::new("FocusPaneTabs")),
        Action::PreviousPaneTab => Ok(KdlNode::new("PreviousPaneTab")),
        Action::NextPaneTab => Ok(KdlNode::new("NextPaneTab")),
        Action::FocusLastPane => Ok(KdlNode::new("FocusLastPane")),
        Action::SwitchFocus => Ok(KdlNode::new("SwitchFocus")),
        Action::MoveFocus { direction } => {
            Ok(child("MoveFocus", direction_name(*direction)))
        },
        Action::MoveFocusOrTab { direction } => {
            Ok(child("MoveFocusOrTab", direction_name(*direction)))
        },
        Action::MovePane { direction } => {
            let mut node = KdlNode::new("MovePane");
            if let Some(direction) = direction {
                node.push(direction_name(*direction));
            }
            Ok(node)
        },
        Action::MovePaneBackwards => Ok(KdlNode::new("MovePaneBackwards")),
        Action::ClearScreen => Ok(KdlNode::new("Clear")),
        Action::DumpScreen {
            file_path: Some(file),
            include_scrollback: false,
            pane_id: None,
            ansi: false,
        } => Ok(child("DumpScreen", file.clone())),
        Action::DumpScreen {
            file_path: None, ..
        } => Err(shape(
            value,
            "`DumpScreen` without a file writes to the terminal, which a binding cannot",
        )),
        Action::DumpScreen { .. } => Err(shape(
            value,
            "`include_scrollback`, `ansi` and a pane id have no config spelling",
        )),
        Action::DumpLayout => Ok(KdlNode::new("DumpLayout")),
        Action::EditScrollback { ansi } => {
            let mut node = KdlNode::new("EditScrollback");
            if *ansi {
                let mut children = KdlDocument::new();
                children.nodes_mut().push(child("ansi", true));
                node.set_children(children);
            }
            Ok(node)
        },
        Action::ScrollUp => Ok(KdlNode::new("ScrollUp")),
        Action::ScrollDown => Ok(KdlNode::new("ScrollDown")),
        Action::MoveTextSelection { direction } => {
            Ok(child("MoveTextSelection", direction_name(*direction)))
        },
        Action::SwapTextSelectionEnds => Ok(KdlNode::new("SwapTextSelectionEnds")),
        Action::MoveNotificationSelection { direction } => Ok(child(
            "MoveNotificationSelection",
            direction_name(*direction),
        )),
        Action::ActivateNotification => Ok(KdlNode::new("ActivateNotification")),
        Action::ActivateLastNotification => Ok(KdlNode::new("ActivateLastNotification")),
        Action::ToggleNotificationDetail => Ok(KdlNode::new("ToggleNotificationDetail")),
        Action::ScrollToBottom => Ok(KdlNode::new("ScrollToBottom")),
        Action::ScrollToTop => Ok(KdlNode::new("ScrollToTop")),
        Action::ScrollToPreviousPrompt => Ok(KdlNode::new("ScrollToPreviousPrompt")),
        Action::ScrollToNextPrompt => Ok(KdlNode::new("ScrollToNextPrompt")),
        Action::SelectCommandAtScrollPosition => {
            Ok(KdlNode::new("SelectCommandAtScrollPosition"))
        },
        Action::CopyLastCommandOutput => Ok(KdlNode::new("CopyLastCommandOutput")),
        Action::PageScrollUp => Ok(KdlNode::new("PageScrollUp")),
        Action::PageScrollDown => Ok(KdlNode::new("PageScrollDown")),
        Action::HalfPageScrollUp => Ok(KdlNode::new("HalfPageScrollUp")),
        Action::HalfPageScrollDown => Ok(KdlNode::new("HalfPageScrollDown")),
        Action::ToggleFocusFullscreen => Ok(KdlNode::new("ToggleFocusFullscreen")),
        Action::ToggleFocusNoUiFullscreen => Ok(KdlNode::new("ToggleFocusNoUiFullscreen")),
        Action::TogglePaneFrames => Ok(KdlNode::new("TogglePaneFrames")),
        Action::ToggleCommandPalette => Ok(KdlNode::new("ToggleCommandPalette")),
        Action::SetPaneFrameStyle(style) => {
            let style = match style {
                PaneFrameStyle::Full => "full",
                PaneFrameStyle::Titles => "titles",
                PaneFrameStyle::None => "none",
            };
            Ok(child("SetPaneFrameStyle", style))
        },
        Action::ToggleActiveSyncTab => Ok(KdlNode::new("ToggleActiveSyncTab")),
        Action::NewPane {
            direction,
            pane_name: None,
            start_suppressed: false,
        } => {
            let mut node = KdlNode::new("NewPane");
            if let Some(direction) = direction {
                node.push(direction_name(*direction));
            }
            Ok(node)
        },
        Action::NewPane { .. } => Err(shape(
            value,
            "a pane name and `start_suppressed` have no config spelling on `NewPane`",
        )),
        Action::TogglePaneEmbedOrFloating => Ok(KdlNode::new("TogglePaneEmbedOrFloating")),
        Action::ToggleFloatingPanes => Ok(KdlNode::new("ToggleFloatingPanes")),
        Action::ShowFloatingPanes { tab_id } => {
            let mut node = KdlNode::new("ShowFloatingPanes");
            if let Some(id) = tab_id {
                let id = i64::try_from(id.get())
                    .map_err(|_| shape(value, "the tab id is wider than a config line holds"))?;
                node.push(KdlValue::Base10(id));
            }
            Ok(node)
        },
        Action::HideFloatingPanes { tab_id } => {
            let mut node = KdlNode::new("HideFloatingPanes");
            if let Some(id) = tab_id {
                let id = i64::try_from(id.get())
                    .map_err(|_| shape(value, "the tab id is wider than a config line holds"))?;
                node.push(KdlValue::Base10(id));
            }
            Ok(node)
        },
        Action::CloseFocus => Ok(KdlNode::new("CloseFocus")),
        Action::PaneNameInput { input: bytes } => {
            let mut node = KdlNode::new("PaneNameInput");
            for byte in bytes {
                node.push(KdlValue::Base10(*byte as i64));
            }
            Ok(node)
        },
        Action::UndoRenamePane => Ok(KdlNode::new("UndoRenamePane")),
        Action::NewTab {
            initial_panes: Some(_),
            ..
        } => Err(shape(
            value,
            "a tab opened with its own panes has no config spelling",
        )),
        Action::NewTab {
            first_pane_unblock_condition: Some(_),
            ..
        } => Err(shape(
            value,
            "a tab that waits on its first pane has no config spelling",
        )),
        Action::NewTab {
            layout_source,
            tiled_layout,
            floating_layouts: _,
            swap_tiled_layouts: _,
            swap_floating_layouts: _,
            tab_name: name,
            should_change_focus_to_new_tab,
            cwd,
            initial_panes: None,
            first_pane_unblock_condition: None,
        } => {
            let mut node = KdlNode::new("NewTab");
            let mut children = KdlDocument::new();
            if let Some(layout_source) = layout_source {
                children
                    .nodes_mut()
                    .push(child("layout", layout_source.display().to_string()));
            }
            if !should_change_focus_to_new_tab
                || tiled_layout.as_ref().and_then(|layout| layout.focus) == Some(false)
            {
                children.nodes_mut().push(child(
                    "should_change_focus_to_new_tab",
                    *should_change_focus_to_new_tab,
                ));
            }
            push_name(&mut children, name);
            if let Some(cwd) = cwd {
                children
                    .nodes_mut()
                    .push(child("cwd", cwd.display().to_string()));
            }
            // An empty metadata block still loads the configured default layout;
            // a bare NewTab defers layout selection until execution.
            if !children.nodes().is_empty() || tiled_layout.is_some() {
                node.set_children(children);
            }
            Ok(node)
        },
        Action::NoOp => Ok(KdlNode::new("NoOp")),
        Action::NewTabGroup => Ok(KdlNode::new("NewTabGroup")),
        Action::GoToNextTabGroup => Ok(KdlNode::new("GoToNextTabGroup")),
        Action::MovePaneToNextTabGroup => Ok(KdlNode::new("MovePaneToNextTabGroup")),
        Action::MovePaneToPreviousTabGroup => Ok(KdlNode::new("MovePaneToPreviousTabGroup")),
        Action::GoToPreviousTabGroup => Ok(KdlNode::new("GoToPreviousTabGroup")),
        Action::GoToTabInGroup { index } => Ok(child("GoToTabInGroup", i64::from(index.get()))),
        Action::SwitchTabGroup { id } => Ok(child("SwitchTabGroup", i64::from(*id))),
        Action::CloseTabGroup { id } => Ok(child("CloseTabGroup", i64::from(*id))),
        Action::RenameTabGroup { id, name } => {
            let mut node = child("RenameTabGroup", i64::from(*id));
            node.push(name.clone());
            Ok(node)
        },
        Action::GoToNextTab => Ok(KdlNode::new("GoToNextTab")),
        Action::GoToPreviousTab => Ok(KdlNode::new("GoToPreviousTab")),
        Action::CloseTab => Ok(KdlNode::new("CloseTab")),
        Action::GoToTab { index } => Ok(child(
            "GoToTab",
            KdlValue::Base10(i64::from(index.get())),
        )),
        Action::ToggleTab => Ok(KdlNode::new("ToggleTab")),
        Action::TabNameInput { input: bytes } => {
            let mut node = KdlNode::new("TabNameInput");
            for byte in bytes {
                node.push(KdlValue::Base10(*byte as i64));
            }
            Ok(node)
        },
        Action::UndoRenameTab => Ok(KdlNode::new("UndoRenameTab")),
        Action::MoveTab { direction } => {
            if direction.is_vertical() {
                return Err(shape(value, "a tab only moves left or right"));
            }
            Ok(child("MoveTab", direction_name(*direction)))
        },
        Action::NewTiledPane {
            direction,
            command: Some(run_command_action),
            pane_name,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        } => {
            if run_command_action.direction != *direction {
                return Err(shape(
                    value,
                    "the pane's direction and its command's disagree; a `Run` line has one `direction` for both",
                ));
            }
            let (node, mut children) = run_command_to_kdl(value, run_command_action)?;
            push_name(&mut children, pane_name);
            Ok(with_children(node, children))
        },
        Action::NewTiledPane { command: None, .. } => {
            Err(shape(value, "a `Run` line needs a command"))
        },
        Action::NewTiledPane { .. } => Err(shape(
            value,
            "`near_current_pane`, `no_focus`, `borderless` and a tab id have no config spelling",
        )),
        Action::NewFloatingPane {
            command: Some(run_command_action),
            pane_name,
            placement: NewPanePlacement::Floating(coordinates),
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        } => {
            let (node, mut children) = run_command_to_kdl(value, run_command_action)?;
            children.nodes_mut().insert(0, child("floating", true));
            if let Some(coordinates) = coordinates {
                push_floating_coordinates(value, coordinates, &mut children)?;
            }
            push_name(&mut children, pane_name);
            Ok(with_children(node, children))
        },
        Action::NewFloatingPane { command: None, .. } => {
            Err(shape(value, "a `Run` line needs a command"))
        },
        Action::NewFloatingPane {
            placement:
                NewPanePlacement::NoPreference { .. }
                | NewPanePlacement::Tiled { .. }
                | NewPanePlacement::InPlace { .. }
                | NewPanePlacement::Stacked { .. }
                | NewPanePlacement::Overlay(_),
            ..
        } => Err(shape(
            value,
            "only a floating placement has a config spelling; an overlay or a tiled placement does not",
        )),
        Action::NewFloatingPane { .. } => Err(shape(
            value,
            "`near_current_pane`, `no_focus` and a tab id have no config spelling",
        )),
        Action::NewInPlacePane {
            command: Some(run_command_action),
            pane_name,
            near_current_pane: false,
            no_focus: false,
            pane_id_to_replace: None,
            close_replaced_pane,
            tab_id: None,
        } => {
            let (node, mut children) = run_command_to_kdl(value, run_command_action)?;
            children.nodes_mut().insert(0, child("in_place", true));
            if *close_replaced_pane {
                children
                    .nodes_mut()
                    .push(child("close_replaced_pane", true));
            }
            push_name(&mut children, pane_name);
            Ok(with_children(node, children))
        },
        Action::NewInPlacePane { command: None, .. } => {
            Err(shape(value, "a `Run` line needs a command"))
        },
        Action::NewInPlacePane { .. } => Err(shape(
            value,
            "`near_current_pane`, `no_focus`, the pane to replace and a tab id have no config spelling",
        )),
        Action::NewStackedPane {
            command: Some(run_command_action),
            pane_name,
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        } => {
            let (node, mut children) = run_command_to_kdl(value, run_command_action)?;
            children.nodes_mut().insert(0, child("stacked", true));
            push_name(&mut children, pane_name);
            Ok(with_children(node, children))
        },
        Action::NewStackedPane {
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            tab_id: None,
        } => Ok(child("NewPane", "stacked")),
        Action::NewStackedPane { command: None, .. } => Err(shape(
            value,
            "`NewPane \"stacked\"` is read back with no name, `near_current_pane`, `no_focus` or tab id",
        )),
        Action::NewStackedPane { .. } => Err(shape(
            value,
            "`near_current_pane`, `no_focus` and a tab id have no config spelling",
        )),
        Action::Detach => Ok(KdlNode::new("Detach")),
        Action::SwitchSession {
            name,
            tab_position,
            pane_id,
            layout,
            cwd,
        } => {
            let mut node = KdlNode::new("SwitchSession");
            node.push(KdlEntry::new_prop("name", name.to_string()));
            if let Some(pos) = tab_position {
                node.push(KdlEntry::new_prop("tab_position", i64::from(pos.get())));
            }
            if let Some(pane_id) = pane_id {
                let (id, is_plugin) = pane_id.legacy_parts();
                node.push(KdlEntry::new_prop("pane_id", i64::from(id)));
                if is_plugin {
                    node.push(KdlEntry::new_prop("is_plugin", true));
                }
            }
            if let Some(layout_info) = layout {
                node.push(KdlEntry::new_prop("layout", layout_info.name()));
            }
            if let Some(cwd_path) = cwd {
                node.push(KdlEntry::new_prop(
                    "cwd",
                    cwd_path.to_string_lossy().to_string(),
                ));
            }
            Ok(node)
        },
        Action::LaunchOrFocusPlugin { tab_id: Some(_), .. } => {
            Err(shape(value, "a tab id has no config spelling"))
        },
        Action::LaunchOrFocusPlugin {
            plugin: run_plugin_or_alias,
            should_float,
            move_to_focused_tab,
            should_open_in_place,
            close_replaced_pane,
            skip_cache: skip_plugin_cache,
            tab_id: None,
        } => {
            let mut node = KdlNode::new("LaunchOrFocusPlugin");
            let mut children = KdlDocument::new();
            node.push(run_plugin_or_alias.location_for_serialization());
            push_plugin_flag(&mut children, "floating", *should_float);
            push_plugin_flag(&mut children, "move_to_focused_tab", *move_to_focused_tab);
            push_plugin_flag(&mut children, "in_place", *should_open_in_place);
            push_plugin_flag(&mut children, "close_replaced_pane", *close_replaced_pane);
            push_plugin_flag(&mut children, "skip_plugin_cache", *skip_plugin_cache);
            if let Some(cwd) = plugin_initial_cwd(run_plugin_or_alias) {
                children
                    .nodes_mut()
                    .push(child("cwd", cwd.display().to_string()));
            }
            push_plugin_configuration(&mut children, run_plugin_or_alias);
            Ok(with_children(node, children))
        },
        Action::LaunchPlugin { cwd: Some(_), .. } => Err(shape(
            value,
            "`LaunchPlugin` is read back with no cwd of its own; the focused pane's is used",
        )),
        Action::LaunchPlugin { no_focus: true, .. } => {
            Err(shape(value, "`no_focus` has no config spelling"))
        },
        Action::LaunchPlugin { tab_id: Some(_), .. } => {
            Err(shape(value, "a tab id has no config spelling"))
        },
        Action::LaunchPlugin {
            plugin: run_plugin_or_alias,
            should_float,
            should_open_in_place,
            close_replaced_pane,
            skip_cache: skip_plugin_cache,
            cwd: None,
            no_focus: false,
            tab_id: None,
        } => {
            if plugin_initial_cwd(run_plugin_or_alias).is_some() {
                return Err(shape(
                    value,
                    "`LaunchPlugin` does not read a `cwd` back into the plugin",
                ));
            }
            let mut node = KdlNode::new("LaunchPlugin");
            let mut children = KdlDocument::new();
            node.push(run_plugin_or_alias.location_for_serialization());
            push_plugin_flag(&mut children, "floating", *should_float);
            push_plugin_flag(&mut children, "in_place", *should_open_in_place);
            push_plugin_flag(&mut children, "close_replaced_pane", *close_replaced_pane);
            push_plugin_flag(&mut children, "skip_plugin_cache", *skip_plugin_cache);
            push_plugin_configuration(&mut children, run_plugin_or_alias);
            Ok(with_children(node, children))
        },
        Action::Copy => Ok(KdlNode::new("Copy")),
        Action::Confirm => Ok(KdlNode::new("Confirm")),
        Action::Deny => Ok(KdlNode::new("Deny")),
        Action::SearchInput { input: bytes } => {
            let mut node = KdlNode::new("SearchInput");
            for byte in bytes {
                node.push(KdlValue::Base10(*byte as i64));
            }
            Ok(node)
        },
        Action::Search {
            direction: search_direction,
        } => {
            let direction = match search_direction {
                SearchDirection::Down => "down",
                SearchDirection::Up => "up",
            };
            Ok(child("Search", direction))
        },
        Action::SearchToggleOption {
            option: search_toggle_option,
        } => Ok(child(
            "SearchToggleOption",
            format!("{:?}", search_toggle_option),
        )),
        Action::ToggleMouseMode => Ok(KdlNode::new("ToggleMouseMode")),
        Action::PreviousSwapLayout => Ok(KdlNode::new("PreviousSwapLayout")),
        Action::NextSwapLayout => Ok(KdlNode::new("NextSwapLayout")),
        Action::OverrideLayout {
            tabs,
            retain_existing_terminal_panes: false,
            retain_existing_plugin_panes: false,
            apply_only_to_active_tab: false,
        } if tabs.is_empty() => Ok(KdlNode::new("OverrideLayout")),
        Action::OverrideLayout { .. } => Err(shape(
            value,
            "the action keeps no reference to the layout it loaded, so only a bare `OverrideLayout` can be written",
        )),
        Action::ArrangePanesByProject => Ok(KdlNode::new("ArrangePanesByProject")),
        Action::BreakPane => Ok(KdlNode::new("BreakPane")),
        Action::BreakPaneRight => Ok(KdlNode::new("BreakPaneRight")),
        Action::BreakPaneLeft => Ok(KdlNode::new("BreakPaneLeft")),
        Action::RenameSession { name } => Ok(child("RenameSession", name.to_string())),
        Action::KeybindPipe { name: None, .. } => Err(shape(
            value,
            "a message needs a name; a line without one is read back with a made-up name",
        )),
        Action::KeybindPipe { args: Some(_), .. } => {
            Err(shape(value, "message `args` have no config spelling"))
        },
        Action::KeybindPipe {
            name: Some(name),
            payload,
            args: None,
            plugin: None,
            plugin_id: Some(plugin_id),
            configuration: None,
            launch_new: false,
            skip_cache: false,
            floating: None,
            in_place,
            cwd: None,
            pane_title: None,
        } => {
            let mut node = KdlNode::new("MessagePluginId");
            node.push(KdlValue::Base10(i64::from(plugin_id.get())));
            let mut children = KdlDocument::new();
            children.nodes_mut().push(child("name", name.clone()));
            if let Some(payload) = payload {
                children.nodes_mut().push(child("payload", payload.clone()));
            }
            if let Some(in_place) = in_place {
                children.nodes_mut().push(child("in_place", *in_place));
            }
            Ok(with_children(node, children))
        },
        Action::KeybindPipe {
            plugin_id: Some(_), ..
        } => Err(shape(
            value,
            "`MessagePluginId` carries only a name, payload and in_place flag",
        )),
        Action::KeybindPipe {
            plugin_id: None,
            floating: None,
            ..
        } => Err(shape(
            value,
            "`MessagePlugin` is read back with `floating` decided one way or the other",
        )),
        Action::KeybindPipe {
            name: Some(name),
            payload,
            args: None,
            plugin,
            plugin_id: None,
            configuration,
            launch_new,
            skip_cache,
            floating: Some(floating),
            in_place,
            cwd,
            pane_title,
        } => {
            let mut node = KdlNode::new("MessagePlugin");
            let mut children = KdlDocument::new();
            if let Some(plugin) = plugin {
                node.push(plugin.clone());
            }
            children.nodes_mut().push(child("name", name.clone()));
            if let Some(cwd) = cwd {
                children
                    .nodes_mut()
                    .push(child("cwd", cwd.display().to_string()));
            }
            if let Some(payload) = payload {
                children.nodes_mut().push(child("payload", payload.clone()));
            }
            push_plugin_flag(&mut children, "launch_new", *launch_new);
            push_plugin_flag(&mut children, "skip_cache", *skip_cache);
            children.nodes_mut().push(child("floating", *floating));
            if let Some(in_place) = in_place {
                children.nodes_mut().push(child("in_place", *in_place));
            }
            if let Some(title) = pane_title {
                children.nodes_mut().push(child("title", title.clone()));
            }
            if let Some(configuration) = configuration {
                if configuration.is_empty() {
                    return Err(shape(
                        value,
                        "an empty configuration is read back as no configuration",
                    ));
                }
                // The parser strips the message's own properties out of the configuration it
                // reads, so a configuration that names one of them cannot be written whole.
                let writable = PluginUserConfiguration::new(configuration.clone());
                if writable.inner() != configuration {
                    return Err(shape(
                        value,
                        "a configuration key that is also a `MessagePlugin` property (name, payload, cwd, title, ...) cannot be written",
                    ));
                }
                for (config_key, config_value) in writable.inner().iter() {
                    children
                        .nodes_mut()
                        .push(child(config_key, config_value.clone()));
                }
            }
            Ok(with_children(node, children))
        },
        Action::TogglePanePinned => Ok(KdlNode::new("TogglePanePinned")),
        Action::TogglePaneInGroup => Ok(KdlNode::new("TogglePaneInGroup")),
        Action::ToggleGroupMarking => Ok(KdlNode::new("ToggleGroupMarking")),
        Action::SetDarkTheme => Ok(KdlNode::new("SetDarkTheme")),
        Action::SetLightTheme => Ok(KdlNode::new("SetLightTheme")),
        Action::ToggleTheme => Ok(KdlNode::new("ToggleTheme")),
        Action::FocusHostSession => Ok(KdlNode::new("FocusHostSession")),
        Action::FocusGuestSession => Ok(KdlNode::new("FocusGuestSession")),
        Action::ToggleHostFullscreen => Ok(KdlNode::new("ToggleHostFullscreen")),
        // Wire-only. Each of these is sent by the CLI or a plugin with a payload a config line
        // has no way to name: a pane or tab id, a session, a plugin reference, a mouse event, a
        // query that expects an answer. `from_kdl` accepts none of their names.
        Action::WriteToPaneId { .. }
        | Action::WriteCharsToPaneId { .. }
        | Action::Paste { .. }
        | Action::SwitchModeForAllClients { .. }
        | Action::SaveSession
        | Action::ScrollUpAt { .. }
        | Action::ScrollDownAt { .. }
        | Action::NewBlockingPane { .. }
        | Action::EditFile { .. }
        | Action::AreFloatingPanesVisible { .. }
        | Action::GoToTabName { .. }
        | Action::Run { .. }
        | Action::SetPaneColor { .. }
        | Action::SetPaneAgent { .. }
        | Action::AgentEvent(_)
        | Action::WarpPane { .. }
        | Action::AdoptPane { .. }
        | Action::WarpTab { .. }
        | Action::AdoptTab { .. }
        | Action::WarpTabFrom { .. }
        | Action::MouseEvent { .. }
        | Action::SkipConfirm { .. }
        | Action::QueryTabNames
        | Action::NewTiledPluginPane { .. }
        | Action::NewFloatingPluginPane { .. }
        | Action::NewInPlacePluginPane { .. }
        | Action::StartOrReloadPlugin { .. }
        | Action::CloseTerminalPane { .. }
        | Action::ClosePluginPane { .. }
        | Action::FocusTerminalPaneWithId { .. }
        | Action::FocusPluginPaneWithId { .. }
        | Action::RenameTerminalPane { .. }
        | Action::RenamePluginPane { .. }
        | Action::RenameTab { .. }
        | Action::GoToTabById { .. }
        | Action::CloseTabById { .. }
        | Action::RenameTabById { .. }
        | Action::Reconfigure { .. }
        | Action::CliPipe { .. }
        | Action::ListClients
        | Action::ListPanes { .. }
        | Action::NewNamedTabGroup { .. }
        | Action::GoToTabGroupName { .. }
        | Action::RenameCurrentTabGroup { .. }
        | Action::CloseCurrentTabGroup
        | Action::ListTabGroups { .. }
        | Action::CurrentTabGroupInfo { .. }
        | Action::ListTabs { .. }
        | Action::CurrentTabInfo { .. }
        | Action::StackPanes { .. }
        | Action::ChangeFloatingPaneCoordinates { .. }
        | Action::TogglePaneBorderless { .. }
        | Action::SetPaneBorderless { .. }
        | Action::ScrollUpByPaneId { .. }
        | Action::ScrollDownByPaneId { .. }
        | Action::ScrollToTopByPaneId { .. }
        | Action::ScrollToBottomByPaneId { .. }
        | Action::PageScrollUpByPaneId { .. }
        | Action::PageScrollDownByPaneId { .. }
        | Action::HalfPageScrollUpByPaneId { .. }
        | Action::HalfPageScrollDownByPaneId { .. }
        | Action::ResizeByPaneId { .. }
        | Action::MovePaneByPaneId { .. }
        | Action::MovePaneBackwardsByPaneId { .. }
        | Action::ClearScreenByPaneId { .. }
        | Action::EditScrollbackByPaneId { .. }
        | Action::ToggleFocusFullscreenByPaneId { .. }
        | Action::ToggleFocusNoUiFullscreenByPaneId { .. }
        | Action::TogglePaneEmbedOrFloatingByPaneId { .. }
        | Action::CloseFocusByPaneId { .. }
        | Action::RenamePaneByPaneId { .. }
        | Action::UndoRenamePaneByPaneId { .. }
        | Action::TogglePanePinnedByPaneId { .. }
        | Action::FocusPaneByPaneId { .. }
        | Action::UndoRenameTabByTabId { .. }
        | Action::ToggleActiveSyncTabByTabId { .. }
        | Action::ToggleFloatingPanesByTabId { .. }
        | Action::PreviousSwapLayoutByTabId { .. }
        | Action::NextSwapLayoutByTabId { .. }
        | Action::MoveTabByTabId { .. } => Err(NotWritable::WireOnly(ActionKind::from(value))),
    }
}
pub fn from_kdl((kdl_action, config_options): (&KdlNode, &Options)) -> Result<Action, ConfigError> {
    let action_name = kdl_name!(kdl_action);
    let action_arguments: Vec<&KdlEntry> = kdl_argument_values!(kdl_action);
    let action_children: Vec<&KdlDocument> = kdl_children!(kdl_action);
    match action_name {
        // Expand the argument parser once; repeated expansions exhaust debug-build stacks.
        "Quit"
        | "FocusNextPane"
        | "FocusPreviousPane"
        | "FocusPaneTabs"
        | "PreviousPaneTab"
        | "NextPaneTab"
        | "FocusLastPane"
        | "FocusHostSession"
        | "FocusGuestSession"
        | "ToggleHostFullscreen"
        | "SwitchFocus"
        | "ScrollUp"
        | "SwapTextSelectionEnds"
        | "ActivateNotification"
        | "ActivateLastNotification"
        | "ToggleNotificationDetail"
        | "ScrollDown"
        | "ScrollToBottom"
        | "ScrollToTop"
        | "ScrollToPreviousPrompt"
        | "ScrollToNextPrompt"
        | "SelectCommandAtScrollPosition"
        | "CopyLastCommandOutput"
        | "PageScrollUp"
        | "PageScrollDown"
        | "HalfPageScrollUp"
        | "HalfPageScrollDown"
        | "ToggleFocusFullscreen"
        | "ToggleFocusNoUiFullscreen"
        | "TogglePaneFrames"
        | "ToggleCommandPalette"
        | "ToggleMother"
        | "ToggleActiveSyncTab"
        | "TogglePaneEmbedOrFloating"
        | "ToggleFloatingPanes"
        | "CloseFocus"
        | "UndoRenamePane"
        | "NoOp"
        | "NewTabGroup"
        | "GoToNextTabGroup"
        | "MovePaneToNextTabGroup"
        | "MovePaneToPreviousTabGroup"
        | "GoToPreviousTabGroup"
        | "GoToNextTab"
        | "GoToPreviousTab"
        | "CloseTab"
        | "ToggleTab"
        | "UndoRenameTab"
        | "ToggleMouseMode"
        | "Detach"
        | "SetDarkTheme"
        | "SetLightTheme"
        | "ToggleTheme"
        | "Copy"
        | "Clear"
        | "Confirm"
        | "Deny" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "EditScrollback" => {
            let ansi =
                crate::kdl_get_bool_property_or_child_value!(kdl_action, "ansi").unwrap_or(false);
            Ok(Action::EditScrollback { ansi })
        },
        "ShowFloatingPanes" => {
            let tab_id = action_arguments
                .first()
                .and_then(|v| v.value().as_i64())
                .map(TabId::try_from)
                .transpose()
                .map_err(|_| {
                    ConfigError::new_kdl_error(
                        "Tab id is out of range".to_owned(),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    )
                })?;
            Ok(Action::ShowFloatingPanes { tab_id })
        },
        "HideFloatingPanes" => {
            let tab_id = action_arguments
                .first()
                .and_then(|v| v.value().as_i64())
                .map(TabId::try_from)
                .transpose()
                .map_err(|_| {
                    ConfigError::new_kdl_error(
                        "Tab id is out of range".to_owned(),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    )
                })?;
            Ok(Action::HideFloatingPanes { tab_id })
        },
        "SwitchSession" => {
            let name = kdl_get_string_property_or_child_value!(kdl_action, "name")
                .map(|s| s.to_string())
                .ok_or(ConfigError::new_kdl_error(
                    "SwitchSession action requires a 'name' property".into(),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                ))?;
            let tab_position =
                crate::kdl_get_int_property_or_child_value!(kdl_action, "tab_position")
                    .map(TabNumber::try_from)
                    .transpose()
                    .map_err(|_| {
                        ConfigError::new_kdl_error(
                            "Tab number is out of range".to_owned(),
                            kdl_action.span().offset(),
                            kdl_action.span().len(),
                        )
                    })?;
            let pane_id = crate::kdl_get_int_property_or_child_value!(kdl_action, "pane_id")
                .map(u32::try_from)
                .transpose()
                .map_err(|_| {
                    ConfigError::new_kdl_error(
                        "Pane id is out of range".to_owned(),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    )
                })?;
            let is_plugin = crate::kdl_get_bool_property_or_child_value!(kdl_action, "is_plugin")
                .unwrap_or(false);
            let pane_id = pane_id.map(|id| yinz_types::pane_id::PaneId::from_legacy(id, is_plugin));

            // Parse layout
            let layout = if let Some(layout_str) =
                kdl_get_string_property_or_child_value!(kdl_action, "layout")
            {
                let layout_path = PathBuf::from(layout_str);
                let layout_dir = config_options
                    .layout_dir
                    .clone()
                    .or_else(|| get_layout_dir(find_default_config_dir()));
                crate::layout::layout_info_from_config(&layout_dir, &Some(layout_path))
            } else {
                None
            };

            // Parse cwd
            let cwd = kdl_get_string_property_or_child_value!(kdl_action, "cwd").map(PathBuf::from);

            Ok(Action::SwitchSession {
                name: name.into(),
                tab_position,
                pane_id,
                layout,
                cwd,
            })
        },
        "Write" => parse_kdl_action_u8_arguments!(action_name, action_arguments, kdl_action),
        "WriteChars" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "SwitchToMode" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "SetPaneFrameStyle" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "Search" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "Resize" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "MoveFocus" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "MoveTextSelection" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "MoveNotificationSelection" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "MoveTab" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "MoveFocusOrTab" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "MovePane" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "MovePaneBackwards" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "DumpScreen" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "DumpLayout" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "NewPane" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "PaneNameInput" => {
            parse_kdl_action_u8_arguments!(action_name, action_arguments, kdl_action)
        },
        "NewTab" => {
            let command_metadata = action_children.first();
            if command_metadata.is_none() {
                return Ok(Action::NewTab {
                    layout_source: None,
                    tiled_layout: None,
                    floating_layouts: vec![],
                    swap_tiled_layouts: None,
                    swap_floating_layouts: None,
                    tab_name: None,
                    should_change_focus_to_new_tab: true,
                    cwd: None,
                    initial_panes: None,
                    first_pane_unblock_condition: None,
                });
            }

            let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

            // The reference is kept as written so that saving the configuration
            // writes it back unchanged; only loading resolves a relative path.
            let layout_source = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "layout"))
                .map(PathBuf::from);
            let layout = layout_source
                .clone()
                .map(|path| {
                    if path.extension().is_some() || path.components().count() > 1 {
                        current_dir.join(path)
                    } else {
                        path
                    }
                })
                .or_else(|| config_options.default_layout.clone());
            let focus_override = command_metadata.and_then(|c_m| {
                kdl_child_bool_value_for_entry(c_m, "should_change_focus_to_new_tab")
            });
            let cwd = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "cwd"))
                .map(PathBuf::from)
                .map(|cwd| current_dir.join(cwd));
            let name = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "name"))
                .map(|name_string| name_string.to_string());

            let layout_dir = config_options
                .layout_dir
                .clone()
                .or_else(|| get_layout_dir(find_default_config_dir()));
            let (path_to_raw_layout, raw_layout, swap_layouts) =
                crate::layout::stringified_from_path_or_default(layout.as_ref(), layout_dir)
                    .map_err(|e| {
                        ConfigError::new_kdl_error(
                            format!("Failed to load layout: {}", e),
                            kdl_action.span().offset(),
                            kdl_action.span().len(),
                        )
                    })?;

            let layout = crate::layout::from_str(
                &raw_layout,
                path_to_raw_layout,
                swap_layouts.as_ref().map(|(f, p)| (f.as_str(), p.as_str())),
                cwd.clone(),
            )
            .map_err(|e| {
                ConfigError::new_kdl_error(
                    format!("Failed to load layout: {}", e),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                )
            })?;

            let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
            let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());

            let mut tabs = layout.tabs();
            if tabs.len() > 1 {
                Err(ConfigError::new_kdl_error(
                    "Tab layout cannot itself have tabs".to_string(),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                ))
            } else if !tabs.is_empty() {
                let (tab_name, mut layout, floating_panes_layout) = tabs.drain(..).next().unwrap();
                // A name given with the action, filling in for a layout that gave none, is the
                // user's.
                layout.user_named |= tab_name.is_none() && name.is_some();
                let name = tab_name.or(name);
                let should_change_focus_to_new_tab =
                    focus_override.or(layout.focus).unwrap_or(true);

                Ok(Action::NewTab {
                    layout_source,
                    tiled_layout: Some(layout),
                    floating_layouts: floating_panes_layout,
                    swap_tiled_layouts,
                    swap_floating_layouts,
                    tab_name: name,
                    should_change_focus_to_new_tab,
                    cwd,
                    initial_panes: None,
                    first_pane_unblock_condition: None,
                })
            } else {
                let (mut layout, floating_panes_layout) = layout.new_tab();
                layout.user_named |= name.is_some();
                let should_change_focus_to_new_tab =
                    focus_override.or(layout.focus).unwrap_or(true);

                Ok(Action::NewTab {
                    layout_source,
                    tiled_layout: Some(layout),
                    floating_layouts: floating_panes_layout,
                    swap_tiled_layouts,
                    swap_floating_layouts,
                    tab_name: name,
                    should_change_focus_to_new_tab,
                    cwd,
                    initial_panes: None,
                    first_pane_unblock_condition: None,
                })
            }
        },
        "OverrideLayout" => {
            let command_metadata = action_children.first();
            if command_metadata.is_none() {
                return Ok(Action::OverrideLayout {
                    tabs: vec![],
                    retain_existing_terminal_panes: false,
                    retain_existing_plugin_panes: false,
                    apply_only_to_active_tab: false,
                });
            }

            let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

            let layout = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "layout"))
                .map(PathBuf::from)
                .or_else(|| config_options.default_layout.clone());
            let cwd = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "cwd"))
                .map(PathBuf::from)
                .map(|cwd| current_dir.join(cwd));
            let name = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "name"))
                .map(|name_string| name_string.to_string());
            let retain_existing_terminal_panes = command_metadata
                .and_then(|c_m| {
                    kdl_child_bool_value_for_entry(c_m, "retain_existing_terminal_panes")
                })
                .unwrap_or(false);
            let retain_existing_plugin_panes = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "retain_existing_plugin_panes"))
                .unwrap_or(false);
            let apply_only_to_active_tab = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "apply_only_to_active_tab"))
                .unwrap_or(false);

            let layout_dir = config_options
                .layout_dir
                .clone()
                .or_else(|| get_layout_dir(find_default_config_dir()));
            let (path_to_raw_layout, raw_layout, swap_layouts) =
                crate::layout::stringified_from_path_or_default(layout.as_ref(), layout_dir)
                    .map_err(|e| {
                        ConfigError::new_kdl_error(
                            format!("Failed to load layout: {}", e),
                            kdl_action.span().offset(),
                            kdl_action.span().len(),
                        )
                    })?;

            let layout = crate::layout::from_str(
                &raw_layout,
                path_to_raw_layout,
                swap_layouts.as_ref().map(|(f, p)| (f.as_str(), p.as_str())),
                cwd.clone(),
            )
            .map_err(|e| {
                ConfigError::new_kdl_error(
                    format!("Failed to load layout: {}", e),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                )
            })?;

            let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
            let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());

            let mut tabs = layout.tabs();
            if tabs.len() > 1 {
                Err(ConfigError::new_kdl_error(
                    "Tab layout cannot itself have tabs".to_string(),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                ))
            } else if !tabs.is_empty() {
                let (tab_name, layout, floating_panes_layout) = tabs.drain(..).next().unwrap();
                let name = tab_name.or(name);

                let tab_layout_info = TabLayoutInfo {
                    tab_index: TabPosition::new(0),
                    tab_name: name,
                    tiled_layout: layout,
                    floating_layouts: floating_panes_layout,
                    swap_tiled_layouts,
                    swap_floating_layouts,
                };

                Ok(Action::OverrideLayout {
                    tabs: vec![tab_layout_info],
                    retain_existing_terminal_panes,
                    retain_existing_plugin_panes,
                    apply_only_to_active_tab,
                })
            } else {
                let (layout, floating_panes_layout) = layout.new_tab();

                let tab_layout_info = TabLayoutInfo {
                    tab_index: TabPosition::new(0),
                    tab_name: name,
                    tiled_layout: layout,
                    floating_layouts: floating_panes_layout,
                    swap_tiled_layouts,
                    swap_floating_layouts,
                };

                Ok(Action::OverrideLayout {
                    tabs: vec![tab_layout_info],
                    retain_existing_terminal_panes,
                    retain_existing_plugin_panes,
                    apply_only_to_active_tab,
                })
            }
        },
        "GoToTab" | "GoToTabInGroup" => {
            // Read as a whole number rather than through the byte-list parser the input
            // actions share, which silently folded `GoToTab 300` into tab 44.
            let index = action_arguments
                .first()
                .and_then(|entry| entry.value().as_i64())
                .ok_or_else(|| {
                    ConfigError::new_kdl_error(
                        format!("{action_name} needs a tab number"),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    )
                })?;
            let index = TabNumber::try_from(index).map_err(|_| {
                ConfigError::new_kdl_error(
                    "Tab number is out of range".to_owned(),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                )
            })?;
            Ok(if action_name == "GoToTabInGroup" {
                Action::GoToTabInGroup { index }
            } else {
                Action::GoToTab { index }
            })
        },
        "SwitchTabGroup" | "CloseTabGroup" | "RenameTabGroup" => {
            let error = || {
                ConfigError::new_kdl_error(
                    "Expected a group ID, and a name for RenameTabGroup".into(),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                )
            };
            let id = action_arguments
                .first()
                .and_then(|e| e.value().as_i64())
                .and_then(|id| u32::try_from(id).ok())
                .ok_or_else(error)?;
            Ok(match action_name {
                "SwitchTabGroup" => Action::SwitchTabGroup { id },
                "CloseTabGroup" => Action::CloseTabGroup { id },
                _ => Action::RenameTabGroup {
                    id,
                    name: action_arguments
                        .get(1)
                        .and_then(|e| e.value().as_string())
                        .ok_or_else(error)?
                        .to_owned(),
                },
            })
        },
        "TabNameInput" => {
            parse_kdl_action_u8_arguments!(action_name, action_arguments, kdl_action)
        },
        "SearchInput" => {
            parse_kdl_action_u8_arguments!(action_name, action_arguments, kdl_action)
        },
        "SearchToggleOption" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "Run" => {
            for children in &action_children {
                for child in children.nodes() {
                    let field = child.name().value();
                    if matches!(
                        field,
                        "firecracker_vm" | "firecracker_image" | "firecracker_worktree"
                    ) {
                        return Err(ConfigError::new_kdl_error(
                            format!("Removed field '{field}': Firecracker execution is no longer supported."),
                            child.span().offset(),
                            child.span().len(),
                        ));
                    }
                }
            }
            let arguments = action_arguments.iter().copied();
            let mut args = kdl_arguments_that_are_strings(arguments)?;
            if args.is_empty() {
                return Err(ConfigError::new_kdl_error(
                    "No command found in Run action".into(),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                ));
            }
            let command = args.remove(0);
            let command_metadata = action_children.first();
            let cwd = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "cwd"))
                .map(PathBuf::from);
            let name = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "name"))
                .map(|name_string| name_string.to_string());
            let direction = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "direction"))
                .and_then(|direction_string| Direction::from_str(direction_string).ok());
            let hold_on_close = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "close_on_exit"))
                .map(|close_on_exit| !close_on_exit)
                .unwrap_or(true);
            let hold_on_start = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "start_suspended"))
                .unwrap_or(false);
            let floating = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "floating"))
                .unwrap_or(false);
            let in_place = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "in_place"))
                .unwrap_or(false);
            let close_replaced_pane = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "close_replaced_pane"))
                .unwrap_or(false);
            let stacked = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "stacked"))
                .unwrap_or(false);
            let run_command_action = RunCommandAction {
                command: PathBuf::from(command),
                args,
                cwd,
                direction,
                hold_on_close,
                hold_on_start,
                ..Default::default()
            };
            let x = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "x"))
                .map(|s| s.to_owned());
            let y = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "y"))
                .map(|s| s.to_owned());
            let width = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "width"))
                .map(|s| s.to_owned());
            let height = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "height"))
                .map(|s| s.to_owned());
            let pinned =
                command_metadata.and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "pinned"));
            let borderless =
                command_metadata.and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "borderless"));
            if floating {
                Ok(Action::NewFloatingPane {
                    command: Some(run_command_action),
                    pane_name: name,
                    placement: NewPanePlacement::Floating(FloatingPaneCoordinates::new(
                        x, y, width, height, pinned, borderless,
                    )),
                    near_current_pane: false,
                    no_focus: false,
                    tab_id: None,
                })
            } else if in_place {
                Ok(Action::NewInPlacePane {
                    command: Some(run_command_action),
                    pane_name: name,
                    near_current_pane: false,
                    no_focus: false,
                    pane_id_to_replace: None,
                    close_replaced_pane,
                    tab_id: None,
                })
            } else if stacked {
                Ok(Action::NewStackedPane {
                    command: Some(run_command_action),
                    pane_name: name,
                    near_current_pane: false,
                    no_focus: false,
                    tab_id: None,
                })
            } else {
                Ok(Action::NewTiledPane {
                    direction,
                    command: Some(run_command_action),
                    pane_name: name,
                    near_current_pane: false,
                    no_focus: false,
                    borderless: None,
                    tab_id: None,
                })
            }
        },
        "LaunchOrFocusPlugin" => {
            let arguments = action_arguments.iter().copied();
            let mut args = kdl_arguments_that_are_strings(arguments)?;
            if args.is_empty() {
                return Err(ConfigError::new_kdl_error(
                    "No plugin found to launch in LaunchOrFocusPlugin".into(),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                ));
            }
            let plugin_path = args.remove(0);

            let command_metadata = action_children.first();
            let should_float = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "floating"))
                .unwrap_or(false);
            let is_overlay = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "overlay"))
                .unwrap_or(false);
            let move_to_focused_tab = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "move_to_focused_tab"))
                .unwrap_or(false);
            let should_open_in_place = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "in_place"))
                .unwrap_or(false);
            let close_replaced_pane = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "close_replaced_pane"))
                .unwrap_or(false);
            let skip_plugin_cache = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "skip_plugin_cache"))
                .unwrap_or(false);
            let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut configuration = KdlLayoutParser::parse_plugin_user_configuration(kdl_action)?;
            if is_overlay {
                configuration.insert("overlay", "true");
            }
            let initial_cwd =
                kdl_get_string_property_or_child_value!(kdl_action, "cwd").map(PathBuf::from);
            let run_plugin_or_alias = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                &plugin_path,
                &Some(configuration.inner().clone()),
                None,
                Some(current_dir),
            )
            .map_err(|e| {
                ConfigError::new_kdl_error(
                    format!("Failed to parse plugin: {}", e),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                )
            })?
            .with_initial_cwd(initial_cwd);
            Ok(Action::LaunchOrFocusPlugin {
                plugin: run_plugin_or_alias,
                should_float,
                move_to_focused_tab,
                should_open_in_place,
                close_replaced_pane,
                skip_cache: skip_plugin_cache,
                tab_id: None,
            })
        },
        "LaunchPlugin" => {
            let arguments = action_arguments.iter().copied();
            let mut args = kdl_arguments_that_are_strings(arguments)?;
            if args.is_empty() {
                return Err(ConfigError::new_kdl_error(
                    "No plugin found to launch in LaunchPlugin".into(),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                ));
            }
            let plugin_path = args.remove(0);

            let command_metadata = action_children.first();
            let should_float = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "floating"))
                .unwrap_or(false);
            let should_open_in_place = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "in_place"))
                .unwrap_or(false);
            let close_replaced_pane = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "close_replaced_pane"))
                .unwrap_or(false);
            let skip_plugin_cache = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "skip_plugin_cache"))
                .unwrap_or(false);
            let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let configuration = KdlLayoutParser::parse_plugin_user_configuration(kdl_action)?;
            let run_plugin_or_alias = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                &plugin_path,
                &Some(configuration.inner().clone()),
                None,
                Some(current_dir),
            )
            .map_err(|e| {
                ConfigError::new_kdl_error(
                    format!("Failed to parse plugin: {}", e),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                )
            })?;
            Ok(Action::LaunchPlugin {
                plugin: run_plugin_or_alias,
                should_float,
                should_open_in_place,
                close_replaced_pane,
                skip_cache: skip_plugin_cache,
                cwd: None, // we explicitly do not send the current dir here so that it will be
                // filled from the active pane == better UX
                no_focus: false,
                tab_id: None,
            })
        },
        "PreviousSwapLayout" => Ok(Action::PreviousSwapLayout),
        "NextSwapLayout" => Ok(Action::NextSwapLayout),
        "ArrangePanesByProject" => Ok(Action::ArrangePanesByProject),
        "BreakPane" => Ok(Action::BreakPane),
        "BreakPaneRight" => Ok(Action::BreakPaneRight),
        "BreakPaneLeft" => Ok(Action::BreakPaneLeft),
        "RenameSession" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "Notify" => {
            parse_kdl_action_char_or_string_arguments!(action_name, action_arguments, kdl_action)
        },
        "MessagePlugin" => {
            let arguments = action_arguments.iter().copied();
            let mut args = kdl_arguments_that_are_strings(arguments)?;
            let plugin_path = if args.is_empty() {
                None
            } else {
                Some(args.remove(0))
            };

            let command_metadata = action_children.first();
            let launch_new = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "launch_new"))
                .unwrap_or(false);
            let skip_cache = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "skip_cache"))
                .unwrap_or(false);
            let should_float = command_metadata
                .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "floating"))
                .unwrap_or(false);
            let name = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "name"))
                .map(|n| n.to_owned());
            let payload = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "payload"))
                .map(|p| p.to_owned());
            let title = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "title"))
                .map(|t| t.to_owned());
            let in_place = kdl_get_bool_property_or_child_value!(kdl_action, "in_place");
            let configuration = KdlLayoutParser::parse_plugin_user_configuration(kdl_action)?;
            let configuration = if configuration.inner().is_empty() {
                None
            } else {
                Some(configuration.inner().clone())
            };
            let cwd = kdl_get_string_property_or_child_value!(kdl_action, "cwd").map(PathBuf::from);

            let name = name
                // first we try to take the explicitly supplied message name
                // then we use the plugin, to facilitate using aliases
                .or_else(|| plugin_path.clone())
                // then we use a uuid to at least have some sort of identifier for this message
                .or_else(|| Some(Uuid::new_v4().to_string()));

            Ok(Action::KeybindPipe {
                name,
                payload,
                // `args` has no config spelling yet: the metadata block's children
                // already flow into `configuration`, so a spelling has to be chosen
                // that keeps the two apart.
                args: None,
                plugin: plugin_path,
                configuration,
                launch_new,
                skip_cache,
                floating: Some(should_float),
                in_place,
                cwd,
                pane_title: title,
                plugin_id: None,
            })
        },
        "MessagePluginId" => {
            let arguments = action_arguments.iter().copied();
            let mut args = kdl_arguments_that_are_digits(arguments)?;
            let plugin_id = if args.is_empty() {
                None
            } else {
                Some(
                    yinz_types::internal_ids::PluginId::try_from(args.remove(0)).map_err(|_| {
                        ConfigError::new_kdl_error(
                            "Plugin id is out of range".to_owned(),
                            kdl_action.span().offset(),
                            kdl_action.span().len(),
                        )
                    })?,
                )
            };

            let command_metadata = action_children.first();
            let launch_new = false;
            let skip_cache = false;
            let name = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "name"))
                .map(|n| n.to_owned());
            let payload = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "payload"))
                .map(|p| p.to_owned());
            let configuration = None;

            let name = name
                // if no name is provided, we use a uuid to at least have some sort of identifier for this message
                .or_else(|| Some(Uuid::new_v4().to_string()));

            Ok(Action::KeybindPipe {
                name,
                payload,
                // See `MessagePlugin` for `args`. A message to a running plugin never
                // launches one, so `floating`/`in_place` do not apply.
                args: None,
                plugin: None,
                configuration,
                launch_new,
                skip_cache,
                floating: None,
                in_place: kdl_get_bool_property_or_child_value!(kdl_action, "in_place"),
                cwd: None,
                pane_title: None,
                plugin_id,
            })
        },
        "TogglePanePinned" => Ok(Action::TogglePanePinned),
        "TogglePaneInGroup" => Ok(Action::TogglePaneInGroup),
        "ToggleGroupMarking" => Ok(Action::ToggleGroupMarking),
        _ => Err(ConfigError::new_kdl_error(
            format!("Unsupported action: {}", action_name),
            kdl_action.span().offset(),
            kdl_action.span().len(),
        )),
    }
}
