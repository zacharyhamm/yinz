use super::*;
use zellij_types::internal_ids::TabId;
use zellij_types::internal_ids::TabNumber;
use zellij_types::internal_ids::TabPosition;

#[allow(clippy::result_large_err)]
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
        "GoToTab" => {
            let tab_index = *bytes.first().ok_or_else(|| {
                ConfigError::new_kdl_error(
                    "Missing tab index".to_string(),
                    action_node.span().offset(),
                    action_node.span().len(),
                )
            })? as u32;
            Ok(Action::GoToTab {
                index: TabNumber::new(tab_index),
            })
        },
        _ => Err(ConfigError::new_kdl_error(
            "Failed to parse action".into(),
            action_node.span().offset(),
            action_node.span().len(),
        )),
    }
}
#[allow(clippy::result_large_err)]
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
pub fn to_kdl(value: &Action) -> Option<KdlNode> {
    match value {
        Action::Quit => Some(KdlNode::new("Quit")),
        Action::Write {
            key_with_modifier: _key,
            bytes,
            is_kitty_keyboard_protocol: _is_kitty,
        } => {
            let mut node = KdlNode::new("Write");
            for byte in bytes {
                node.push(KdlValue::Base10(*byte as i64));
            }
            Some(node)
        },
        Action::WriteChars { chars: string } => {
            let mut node = KdlNode::new("WriteChars");
            node.push(string.clone());
            Some(node)
        },
        Action::Notify(text) => {
            let mut node = KdlNode::new("Notify");
            node.push(text.clone());
            Some(node)
        },
        Action::SwitchToMode { input_mode } => {
            let mut node = KdlNode::new("SwitchToMode");
            node.push(format!("{:?}", input_mode).to_lowercase());
            Some(node)
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
                let resize_direction = match resize_direction {
                    Direction::Left => "left",
                    Direction::Right => "right",
                    Direction::Up => "up",
                    Direction::Down => "down",
                };
                node.push(format!("{} {}", resize, resize_direction));
            } else {
                node.push(resize.to_string());
            }
            Some(node)
        },
        Action::FocusNextPane => Some(KdlNode::new("FocusNextPane")),
        Action::FocusPreviousPane => Some(KdlNode::new("FocusPreviousPane")),
        Action::FocusLastPane => Some(KdlNode::new("FocusLastPane")),
        Action::SwitchFocus => Some(KdlNode::new("SwitchFocus")),
        Action::MoveFocus { direction } => {
            let mut node = KdlNode::new("MoveFocus");
            let direction = match direction {
                Direction::Left => "left",
                Direction::Right => "right",
                Direction::Up => "up",
                Direction::Down => "down",
            };
            node.push(direction);
            Some(node)
        },
        Action::MoveFocusOrTab { direction } => {
            let mut node = KdlNode::new("MoveFocusOrTab");
            let direction = match direction {
                Direction::Left => "left",
                Direction::Right => "right",
                Direction::Up => "up",
                Direction::Down => "down",
            };
            node.push(direction);
            Some(node)
        },
        Action::MovePane { direction } => {
            let mut node = KdlNode::new("MovePane");
            if let Some(direction) = direction {
                let direction = match direction {
                    Direction::Left => "left",
                    Direction::Right => "right",
                    Direction::Up => "up",
                    Direction::Down => "down",
                };
                node.push(direction);
            }
            Some(node)
        },
        Action::MovePaneBackwards => Some(KdlNode::new("MovePaneBackwards")),
        Action::DumpScreen {
            file_path: Some(file),
            include_scrollback: _,
            pane_id: _,
            ansi: _,
        } => {
            let mut node = KdlNode::new("DumpScreen");
            node.push(file.clone());
            Some(node)
        },
        Action::DumpScreen {
            file_path: None, ..
        } => None,
        Action::DumpLayout => Some(KdlNode::new("DumpLayout")),
        Action::EditScrollback { ansi } => {
            let mut node = KdlNode::new("EditScrollback");
            if *ansi {
                let mut children = KdlDocument::new();
                let mut ansi_node = KdlNode::new("ansi");
                ansi_node.push(KdlValue::Bool(true));
                children.nodes_mut().push(ansi_node);
                node.set_children(children);
            }
            Some(node)
        },
        Action::ScrollUp => Some(KdlNode::new("ScrollUp")),
        Action::ScrollDown => Some(KdlNode::new("ScrollDown")),
        Action::MoveTextSelection { direction } => {
            let mut node = KdlNode::new("MoveTextSelection");
            node.push(match direction {
                Direction::Left => "left",
                Direction::Right => "right",
                Direction::Up => "up",
                Direction::Down => "down",
            });
            Some(node)
        },
        Action::SwapTextSelectionEnds => Some(KdlNode::new("SwapTextSelectionEnds")),
        Action::MoveNotificationSelection { direction } => {
            let mut node = KdlNode::new("MoveNotificationSelection");
            node.push(match direction {
                Direction::Left => "left",
                Direction::Right => "right",
                Direction::Up => "up",
                Direction::Down => "down",
            });
            Some(node)
        },
        Action::ActivateNotification => Some(KdlNode::new("ActivateNotification")),
        Action::ToggleNotificationDetail => Some(KdlNode::new("ToggleNotificationDetail")),
        Action::ScrollToBottom => Some(KdlNode::new("ScrollToBottom")),
        Action::ScrollToTop => Some(KdlNode::new("ScrollToTop")),
        Action::ScrollToPreviousPrompt => Some(KdlNode::new("ScrollToPreviousPrompt")),
        Action::ScrollToNextPrompt => Some(KdlNode::new("ScrollToNextPrompt")),
        Action::SelectCommandAtScrollPosition => {
            Some(KdlNode::new("SelectCommandAtScrollPosition"))
        },
        Action::CopyLastCommandOutput => Some(KdlNode::new("CopyLastCommandOutput")),
        Action::PageScrollUp => Some(KdlNode::new("PageScrollUp")),
        Action::PageScrollDown => Some(KdlNode::new("PageScrollDown")),
        Action::HalfPageScrollUp => Some(KdlNode::new("HalfPageScrollUp")),
        Action::HalfPageScrollDown => Some(KdlNode::new("HalfPageScrollDown")),
        Action::ToggleFocusFullscreen => Some(KdlNode::new("ToggleFocusFullscreen")),
        Action::ToggleFocusNoUiFullscreen => Some(KdlNode::new("ToggleFocusNoUiFullscreen")),
        Action::TogglePaneFrames => Some(KdlNode::new("TogglePaneFrames")),
        Action::SetPaneFrameStyle(style) => {
            let mut node = KdlNode::new("SetPaneFrameStyle");
            let style = match style {
                PaneFrameStyle::Full => "full",
                PaneFrameStyle::Titles => "titles",
                PaneFrameStyle::None => "none",
            };
            node.push(style);
            Some(node)
        },
        Action::ToggleActiveSyncTab => Some(KdlNode::new("ToggleActiveSyncTab")),
        Action::NewPane {
            direction,
            pane_name: _,
            start_suppressed: _,
        } => {
            let mut node = KdlNode::new("NewPane");
            if let Some(direction) = direction {
                let direction = match direction {
                    Direction::Left => "left",
                    Direction::Right => "right",
                    Direction::Up => "up",
                    Direction::Down => "down",
                };
                node.push(direction);
            }
            let children = KdlDocument::new();
            if !children.nodes().is_empty() {
                node.set_children(children);
            }
            Some(node)
        },
        Action::TogglePaneEmbedOrFloating => Some(KdlNode::new("TogglePaneEmbedOrFloating")),
        Action::ToggleFloatingPanes => Some(KdlNode::new("ToggleFloatingPanes")),
        Action::ShowFloatingPanes { tab_id } => {
            let mut node = KdlNode::new("ShowFloatingPanes");
            if let Some(id) = tab_id {
                node.push(KdlValue::Base10(i64::try_from(id.get()).ok()?));
            }
            Some(node)
        },
        Action::HideFloatingPanes { tab_id } => {
            let mut node = KdlNode::new("HideFloatingPanes");
            if let Some(id) = tab_id {
                node.push(KdlValue::Base10(i64::try_from(id.get()).ok()?));
            }
            Some(node)
        },
        Action::CloseFocus => Some(KdlNode::new("CloseFocus")),
        Action::PaneNameInput { input: bytes } => {
            let mut node = KdlNode::new("PaneNameInput");
            for byte in bytes {
                node.push(KdlValue::Base10(*byte as i64));
            }
            Some(node)
        },
        Action::UndoRenamePane => Some(KdlNode::new("UndoRenamePane")),
        Action::NewTab {
            layout_source,
            tiled_layout,
            floating_layouts: _,
            swap_tiled_layouts: _,
            swap_floating_layouts: _,
            tab_name: name,
            should_change_focus_to_new_tab,
            cwd,
            initial_panes: _,
            first_pane_unblock_condition: _,
        } => {
            let mut node = KdlNode::new("NewTab");
            let mut children = KdlDocument::new();
            if let Some(layout_source) = layout_source {
                let mut layout_node = KdlNode::new("layout");
                layout_node.push(layout_source.display().to_string());
                children.nodes_mut().push(layout_node);
            }
            if !should_change_focus_to_new_tab
                || tiled_layout.as_ref().and_then(|layout| layout.focus) == Some(false)
            {
                let mut focus_node = KdlNode::new("should_change_focus_to_new_tab");
                focus_node.push(KdlValue::Bool(*should_change_focus_to_new_tab));
                children.nodes_mut().push(focus_node);
            }
            if let Some(name) = name {
                let mut name_node = KdlNode::new("name");
                name_node.push(name.clone());
                children.nodes_mut().push(name_node);
            }
            if let Some(cwd) = cwd {
                let mut cwd_node = KdlNode::new("cwd");
                cwd_node.push(cwd.display().to_string());
                children.nodes_mut().push(cwd_node);
            }
            // An empty metadata block still loads the configured default layout;
            // a bare NewTab defers layout selection until execution.
            if !children.nodes().is_empty() || tiled_layout.is_some() {
                node.set_children(children);
            }
            Some(node)
        },
        Action::GoToNextTab => Some(KdlNode::new("GoToNextTab")),
        Action::GoToPreviousTab => Some(KdlNode::new("GoToPreviousTab")),
        Action::CloseTab => Some(KdlNode::new("CloseTab")),
        Action::GoToTab { index } => {
            let mut node = KdlNode::new("GoToTab");
            node.push(KdlValue::Base10(i64::from(index.get())));
            Some(node)
        },
        Action::ToggleTab => Some(KdlNode::new("ToggleTab")),
        Action::TabNameInput { input: bytes } => {
            let mut node = KdlNode::new("TabNameInput");
            for byte in bytes {
                node.push(KdlValue::Base10(*byte as i64));
            }
            Some(node)
        },
        Action::UndoRenameTab => Some(KdlNode::new("UndoRenameTab")),
        Action::MoveTab { direction } => {
            let mut node = KdlNode::new("MoveTab");
            let direction = match direction {
                Direction::Left => "left",
                Direction::Right => "right",
                Direction::Up => "up",
                Direction::Down => "down",
            };
            node.push(direction);
            Some(node)
        },
        Action::NewTiledPane {
            direction,
            command: run_command_action,
            pane_name: name,
            near_current_pane: false,
            ..
        } => {
            let mut node = KdlNode::new("Run");
            let mut node_children = KdlDocument::new();
            if let Some(run_command_action) = run_command_action {
                node.push(run_command_action.command.display().to_string());
                for arg in &run_command_action.args {
                    node.push(arg.clone());
                }
                if let Some(cwd) = &run_command_action.cwd {
                    let mut cwd_node = KdlNode::new("cwd");
                    cwd_node.push(cwd.display().to_string());
                    node_children.nodes_mut().push(cwd_node);
                }
                push_firecracker_nodes(run_command_action, &mut node_children);
                if run_command_action.hold_on_start {
                    let mut hos_node = KdlNode::new("hold_on_start");
                    hos_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(hos_node);
                }
                if !run_command_action.hold_on_close {
                    let mut hoc_node = KdlNode::new("hold_on_close");
                    hoc_node.push(KdlValue::Bool(false));
                    node_children.nodes_mut().push(hoc_node);
                }
            }
            if let Some(name) = name {
                let mut name_node = KdlNode::new("name");
                name_node.push(name.clone());
                node_children.nodes_mut().push(name_node);
            }
            if let Some(direction) = direction {
                let mut direction_node = KdlNode::new("direction");
                let direction = match direction {
                    Direction::Left => "left",
                    Direction::Right => "right",
                    Direction::Up => "up",
                    Direction::Down => "down",
                };
                direction_node.push(direction);
                node_children.nodes_mut().push(direction_node);
            }
            if !node_children.nodes().is_empty() {
                node.set_children(node_children);
            }
            Some(node)
        },
        Action::NewFloatingPane {
            command: run_command_action,
            pane_name: name,
            placement,
            near_current_pane: false,
            ..
        } => {
            let floating_pane_coordinates = placement.floating_pane_coordinates();
            let mut node = KdlNode::new("Run");
            let mut node_children = KdlDocument::new();
            let mut floating_pane = KdlNode::new("floating");
            floating_pane.push(KdlValue::Bool(true));
            node_children.nodes_mut().push(floating_pane);
            if let Some(run_command_action) = run_command_action {
                node.push(run_command_action.command.display().to_string());
                for arg in &run_command_action.args {
                    node.push(arg.clone());
                }
                if let Some(cwd) = &run_command_action.cwd {
                    let mut cwd_node = KdlNode::new("cwd");
                    cwd_node.push(cwd.display().to_string());
                    node_children.nodes_mut().push(cwd_node);
                }
                push_firecracker_nodes(run_command_action, &mut node_children);
                if run_command_action.hold_on_start {
                    let mut hos_node = KdlNode::new("hold_on_start");
                    hos_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(hos_node);
                }
                if !run_command_action.hold_on_close {
                    let mut hoc_node = KdlNode::new("hold_on_close");
                    hoc_node.push(KdlValue::Bool(false));
                    node_children.nodes_mut().push(hoc_node);
                }
            }
            if let Some(floating_pane_coordinates) = floating_pane_coordinates {
                if let Some(x) = floating_pane_coordinates.x {
                    let mut x_node = KdlNode::new("x");
                    match x {
                        PercentOrFixed::Percent(x) => {
                            x_node.push(format!("{}%", x));
                        },
                        PercentOrFixed::Fixed(x) => {
                            x_node.push(KdlValue::Base10(x as i64));
                        },
                    };
                    node_children.nodes_mut().push(x_node);
                }
                if let Some(y) = floating_pane_coordinates.y {
                    let mut y_node = KdlNode::new("y");
                    match y {
                        PercentOrFixed::Percent(y) => {
                            y_node.push(format!("{}%", y));
                        },
                        PercentOrFixed::Fixed(y) => {
                            y_node.push(KdlValue::Base10(y as i64));
                        },
                    };
                    node_children.nodes_mut().push(y_node);
                }
                if let Some(width) = floating_pane_coordinates.width {
                    let mut width_node = KdlNode::new("width");
                    match width {
                        PercentOrFixed::Percent(width) => {
                            width_node.push(format!("{}%", width));
                        },
                        PercentOrFixed::Fixed(width) => {
                            width_node.push(KdlValue::Base10(width as i64));
                        },
                    };
                    node_children.nodes_mut().push(width_node);
                }
                if let Some(height) = floating_pane_coordinates.height {
                    let mut height_node = KdlNode::new("height");
                    match height {
                        PercentOrFixed::Percent(height) => {
                            height_node.push(format!("{}%", height));
                        },
                        PercentOrFixed::Fixed(height) => {
                            height_node.push(KdlValue::Base10(height as i64));
                        },
                    };
                    node_children.nodes_mut().push(height_node);
                }
            }
            if let Some(name) = name {
                let mut name_node = KdlNode::new("name");
                name_node.push(name.clone());
                node_children.nodes_mut().push(name_node);
            }
            if !node_children.nodes().is_empty() {
                node.set_children(node_children);
            }
            Some(node)
        },
        Action::NewInPlacePane {
            command: run_command_action,
            pane_name: name,
            near_current_pane: false,
            pane_id_to_replace: None,
            close_replaced_pane,
            ..
        } => {
            let mut node = KdlNode::new("Run");
            let mut node_children = KdlDocument::new();
            if let Some(run_command_action) = run_command_action {
                node.push(run_command_action.command.display().to_string());
                for arg in &run_command_action.args {
                    node.push(arg.clone());
                }
                let mut in_place_node = KdlNode::new("in_place");
                in_place_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(in_place_node);
                if let Some(cwd) = &run_command_action.cwd {
                    let mut cwd_node = KdlNode::new("cwd");
                    cwd_node.push(cwd.display().to_string());
                    node_children.nodes_mut().push(cwd_node);
                }
                push_firecracker_nodes(run_command_action, &mut node_children);
                if run_command_action.hold_on_start {
                    let mut hos_node = KdlNode::new("hold_on_start");
                    hos_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(hos_node);
                }
                if !run_command_action.hold_on_close {
                    let mut hoc_node = KdlNode::new("hold_on_close");
                    hoc_node.push(KdlValue::Bool(false));
                    node_children.nodes_mut().push(hoc_node);
                }
            }
            if *close_replaced_pane {
                let mut crp_node = KdlNode::new("close_replaced_pane");
                crp_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(crp_node);
            }
            if let Some(name) = name {
                let mut name_node = KdlNode::new("name");
                name_node.push(name.clone());
                node_children.nodes_mut().push(name_node);
            }
            if !node_children.nodes().is_empty() {
                node.set_children(node_children);
            }
            Some(node)
        },
        Action::NewStackedPane {
            command: run_command_action,
            pane_name: name,
            ..
        } => match run_command_action {
            Some(run_command_action) => {
                let mut node = KdlNode::new("Run");
                let mut node_children = KdlDocument::new();
                node.push(run_command_action.command.display().to_string());
                for arg in &run_command_action.args {
                    node.push(arg.clone());
                }
                let mut stacked_node = KdlNode::new("stacked");
                stacked_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(stacked_node);
                if let Some(cwd) = &run_command_action.cwd {
                    let mut cwd_node = KdlNode::new("cwd");
                    cwd_node.push(cwd.display().to_string());
                    node_children.nodes_mut().push(cwd_node);
                }
                push_firecracker_nodes(run_command_action, &mut node_children);
                if run_command_action.hold_on_start {
                    let mut hos_node = KdlNode::new("hold_on_start");
                    hos_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(hos_node);
                }
                if !run_command_action.hold_on_close {
                    let mut hoc_node = KdlNode::new("hold_on_close");
                    hoc_node.push(KdlValue::Bool(false));
                    node_children.nodes_mut().push(hoc_node);
                }
                if let Some(name) = name {
                    let mut name_node = KdlNode::new("name");
                    name_node.push(name.clone());
                    node_children.nodes_mut().push(name_node);
                }
                if !node_children.nodes().is_empty() {
                    node.set_children(node_children);
                }
                Some(node)
            },
            None => {
                let mut node = KdlNode::new("NewPane");
                node.push("stacked");
                Some(node)
            },
        },
        Action::Detach => Some(KdlNode::new("Detach")),
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
            Some(node)
        },
        Action::LaunchOrFocusPlugin {
            plugin: run_plugin_or_alias,
            should_float,
            move_to_focused_tab,
            should_open_in_place,
            close_replaced_pane,
            skip_cache: skip_plugin_cache,
            ..
        } => {
            let mut node = KdlNode::new("LaunchOrFocusPlugin");
            let mut node_children = KdlDocument::new();
            let location = run_plugin_or_alias.location_string();
            node.push(location);
            if *should_float {
                let mut should_float_node = KdlNode::new("floating");
                should_float_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(should_float_node);
            }
            if *move_to_focused_tab {
                let mut move_to_focused_tab_node = KdlNode::new("move_to_focused_tab");
                move_to_focused_tab_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(move_to_focused_tab_node);
            }
            if *should_open_in_place {
                let mut should_open_in_place_node = KdlNode::new("in_place");
                should_open_in_place_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(should_open_in_place_node);
            }
            if *close_replaced_pane {
                let mut crp_node = KdlNode::new("close_replaced_pane");
                crp_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(crp_node);
            }
            if *skip_plugin_cache {
                let mut skip_plugin_cache_node = KdlNode::new("skip_plugin_cache");
                skip_plugin_cache_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(skip_plugin_cache_node);
            }
            let configuration = match run_plugin_or_alias {
                RunPluginOrAlias::RunPlugin(plugin) => Some(&plugin.configuration),
                RunPluginOrAlias::Alias(alias) => alias.configuration.as_ref(),
            };
            if let Some(configuration) = configuration {
                for (config_key, config_value) in configuration.inner().iter() {
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
                    node_children.nodes_mut().push(node);
                }
            }
            if !node_children.nodes().is_empty() {
                node.set_children(node_children);
            }
            Some(node)
        },
        Action::LaunchPlugin {
            plugin: run_plugin_or_alias,
            should_float,
            should_open_in_place,
            close_replaced_pane,
            skip_cache: skip_plugin_cache,
            cwd,
            ..
        } => {
            let mut node = KdlNode::new("LaunchPlugin");
            let mut node_children = KdlDocument::new();
            let location = run_plugin_or_alias.location_string();
            node.push(location);
            if *should_float {
                let mut should_float_node = KdlNode::new("floating");
                should_float_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(should_float_node);
            }
            if *should_open_in_place {
                let mut should_open_in_place_node = KdlNode::new("in_place");
                should_open_in_place_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(should_open_in_place_node);
            }
            if *close_replaced_pane {
                let mut crp_node = KdlNode::new("close_replaced_pane");
                crp_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(crp_node);
            }
            if *skip_plugin_cache {
                let mut skip_plugin_cache_node = KdlNode::new("skip_plugin_cache");
                skip_plugin_cache_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(skip_plugin_cache_node);
            }
            if let Some(cwd) = &cwd {
                let mut cwd_node = KdlNode::new("cwd");
                cwd_node.push(cwd.display().to_string());
                node_children.nodes_mut().push(cwd_node);
            } else if let Some(cwd) = run_plugin_or_alias.get_initial_cwd() {
                let mut cwd_node = KdlNode::new("cwd");
                cwd_node.push(cwd.display().to_string());
                node_children.nodes_mut().push(cwd_node);
            }
            if let Some(configuration) = run_plugin_or_alias.get_configuration() {
                for (config_key, config_value) in configuration.inner().iter() {
                    let mut node = KdlNode::new(config_key.clone());
                    node.push(config_value.clone());
                    node_children.nodes_mut().push(node);
                }
            }
            if !node_children.nodes().is_empty() {
                node.set_children(node_children);
            }
            Some(node)
        },
        Action::Copy => Some(KdlNode::new("Copy")),
        Action::SearchInput { input: bytes } => {
            let mut node = KdlNode::new("SearchInput");
            for byte in bytes {
                node.push(KdlValue::Base10(*byte as i64));
            }
            Some(node)
        },
        Action::Search {
            direction: search_direction,
        } => {
            let mut node = KdlNode::new("Search");
            let direction = match search_direction {
                SearchDirection::Down => "down",
                SearchDirection::Up => "up",
            };
            node.push(direction);
            Some(node)
        },
        Action::SearchToggleOption {
            option: search_toggle_option,
        } => {
            let mut node = KdlNode::new("SearchToggleOption");
            node.push(format!("{:?}", search_toggle_option));
            Some(node)
        },
        Action::ToggleMouseMode => Some(KdlNode::new("ToggleMouseMode")),
        Action::PreviousSwapLayout => Some(KdlNode::new("PreviousSwapLayout")),
        Action::NextSwapLayout => Some(KdlNode::new("NextSwapLayout")),
        Action::ArrangePanesByProject => Some(KdlNode::new("ArrangePanesByProject")),
        Action::BreakPane => Some(KdlNode::new("BreakPane")),
        Action::BreakPaneRight => Some(KdlNode::new("BreakPaneRight")),
        Action::BreakPaneLeft => Some(KdlNode::new("BreakPaneLeft")),
        Action::KeybindPipe {
            name,
            payload,
            args: _, // currently unsupported
            plugin,
            configuration,
            launch_new,
            skip_cache,
            floating,
            in_place: _, // currently unsupported
            cwd,
            pane_title,
            plugin_id,
        } => {
            if plugin_id.is_some() {
                log::warn!("Not serializing temporary keybinding MessagePluginId");
                return None;
            }
            let mut node = KdlNode::new("MessagePlugin");
            let mut node_children = KdlDocument::new();
            if let Some(plugin) = plugin {
                node.push(plugin.clone());
            }
            if let Some(name) = name {
                let mut name_node = KdlNode::new("name");
                name_node.push(name.clone());
                node_children.nodes_mut().push(name_node);
            }
            if let Some(cwd) = cwd {
                let mut cwd_node = KdlNode::new("cwd");
                cwd_node.push(cwd.display().to_string());
                node_children.nodes_mut().push(cwd_node);
            }
            if let Some(payload) = payload {
                let mut payload_node = KdlNode::new("payload");
                payload_node.push(payload.clone());
                node_children.nodes_mut().push(payload_node);
            }
            if *launch_new {
                let mut launch_new_node = KdlNode::new("launch_new");
                launch_new_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(launch_new_node);
            }
            if *skip_cache {
                let mut skip_cache_node = KdlNode::new("skip_cache");
                skip_cache_node.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(skip_cache_node);
            }
            if let Some(floating) = floating {
                let mut floating_node = KdlNode::new("floating");
                floating_node.push(KdlValue::Bool(*floating));
                node_children.nodes_mut().push(floating_node);
            }
            if let Some(title) = pane_title {
                let mut title_node = KdlNode::new("title");
                title_node.push(title.clone());
                node_children.nodes_mut().push(title_node);
            }
            if let Some(configuration) = configuration {
                // we do this because the constructor removes the relevant config fields from
                // above, otherwise we would have duplicates
                let configuration = PluginUserConfiguration::new(configuration.clone());
                let configuration = configuration.inner();
                for (config_key, config_value) in configuration.iter() {
                    let mut node = KdlNode::new(config_key.clone());
                    node.push(config_value.clone());
                    node_children.nodes_mut().push(node);
                }
            }
            if !node_children.nodes().is_empty() {
                node.set_children(node_children);
            }
            Some(node)
        },
        Action::TogglePanePinned => Some(KdlNode::new("TogglePanePinned")),
        Action::TogglePaneInGroup => Some(KdlNode::new("TogglePaneInGroup")),
        Action::ToggleGroupMarking => Some(KdlNode::new("ToggleGroupMarking")),
        Action::SetDarkTheme => Some(KdlNode::new("SetDarkTheme")),
        Action::SetLightTheme => Some(KdlNode::new("SetLightTheme")),
        Action::ToggleTheme => Some(KdlNode::new("ToggleTheme")),
        Action::FocusHostSession => Some(KdlNode::new("FocusHostSession")),
        Action::FocusGuestSession => Some(KdlNode::new("FocusGuestSession")),
        Action::ToggleHostFullscreen => Some(KdlNode::new("ToggleHostFullscreen")),
        _ => None,
    }
}

#[allow(clippy::result_large_err)]
pub fn from_kdl((kdl_action, config_options): (&KdlNode, &Options)) -> Result<Action, ConfigError> {
    let action_name = kdl_name!(kdl_action);
    let action_arguments: Vec<&KdlEntry> = kdl_argument_values!(kdl_action);
    let action_children: Vec<&KdlDocument> = kdl_children!(kdl_action);
    match action_name {
        "Quit" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "FocusNextPane" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "FocusPreviousPane" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "FocusLastPane" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "FocusHostSession" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "FocusGuestSession" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ToggleHostFullscreen" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "SwitchFocus" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "EditScrollback" => {
            let ansi =
                crate::kdl_get_bool_property_or_child_value!(kdl_action, "ansi").unwrap_or(false);
            Ok(Action::EditScrollback { ansi })
        },
        "ScrollUp" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "SwapTextSelectionEnds" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ActivateNotification" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ToggleNotificationDetail" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ScrollDown" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "ScrollToBottom" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ScrollToTop" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ScrollToPreviousPrompt" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ScrollToNextPrompt" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "SelectCommandAtScrollPosition" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "CopyLastCommandOutput" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "PageScrollUp" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "PageScrollDown" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "HalfPageScrollUp" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "HalfPageScrollDown" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ToggleFocusFullscreen" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ToggleFocusNoUiFullscreen" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "TogglePaneFrames" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ToggleActiveSyncTab" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "TogglePaneEmbedOrFloating" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ToggleFloatingPanes" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
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
        "CloseFocus" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "UndoRenamePane" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "NoOp" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "GoToNextTab" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "GoToPreviousTab" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "CloseTab" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "ToggleTab" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "UndoRenameTab" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ToggleMouseMode" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "Detach" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "SetDarkTheme" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "SetLightTheme" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
        },
        "ToggleTheme" => {
            parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
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
            let pane_id =
                pane_id.map(|id| zellij_types::pane_id::PaneId::from_legacy(id, is_plugin));

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
        "Copy" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "Clear" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "Confirm" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
        "Deny" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
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
        "ResizeNew" => {
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
            #[allow(clippy::needless_return)]
            if tabs.len() > 1 {
                return Err(ConfigError::new_kdl_error(
                    "Tab layout cannot itself have tabs".to_string(),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                ));
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
            #[allow(clippy::needless_return)]
            if tabs.len() > 1 {
                return Err(ConfigError::new_kdl_error(
                    "Tab layout cannot itself have tabs".to_string(),
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                ));
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
        "GoToTab" => parse_kdl_action_u8_arguments!(action_name, action_arguments, kdl_action),
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
            let firecracker_vm = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "firecracker_vm"));
            let firecracker_image = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "firecracker_image"));
            let firecracker_worktree = command_metadata
                .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "firecracker_worktree"))
                .map(zellij_runtime::shared::expand_path)
                .transpose()
                .map_err(|error| {
                    ConfigError::new_kdl_error(
                        format!("firecracker_worktree: {error}"),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    )
                })?;
            let target = PaneTarget::from_parts(
                None,
                firecracker_vm,
                firecracker_image,
                firecracker_worktree.as_deref(),
            )
            .map_err(|error| {
                ConfigError::new_kdl_error(
                    error,
                    kdl_action.span().offset(),
                    kdl_action.span().len(),
                )
            })?;
            let run_command_action = RunCommandAction {
                command: PathBuf::from(command),
                args,
                cwd,
                direction,
                hold_on_close,
                hold_on_start,
                target,
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
            let run_plugin_or_alias =
                zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
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
            let run_plugin_or_alias =
                zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
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
                args: None, // TODO: consider supporting this if there's a need
                plugin: plugin_path,
                configuration,
                launch_new,
                skip_cache,
                floating: Some(should_float),
                in_place: None, // TODO: support this
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
                    zellij_types::internal_ids::PluginId::try_from(args.remove(0)).map_err(
                        |_| {
                            ConfigError::new_kdl_error(
                                "Plugin id is out of range".to_owned(),
                                kdl_action.span().offset(),
                                kdl_action.span().len(),
                            )
                        },
                    )?,
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
                args: None, // TODO: consider supporting this if there's a need
                plugin: None,
                configuration,
                launch_new,
                skip_cache,
                floating: None,
                in_place: None, // TODO: support this
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
