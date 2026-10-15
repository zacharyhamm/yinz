use super::*;
use yinz_types::internal_ids::HostId;

fn unsigned_option<T: TryFrom<i64>>(
    document: &KdlDocument,
    name: &str,
    minimum: i64,
) -> Result<Option<T>, ConfigError> {
    kdl_property_first_arg_as_i64_or_error!(document, name)
        .map(|(value, entry)| {
            T::try_from(value)
                .ok()
                .filter(|_| value >= minimum)
                .ok_or_else(|| {
                    kdl_parsing_error!(
                        format!("{name} must be at least {minimum} and in range, got {value}"),
                        entry
                    )
                })
        })
        .transpose()
}

pub fn from_kdl(kdl_options: &KdlDocument) -> Result<Options, ConfigError> {
    let background_opacity = kdl_options
        .get("background_opacity")
        .map(|node| {
            let entry = node.get(0).ok_or_else(|| {
                kdl_parsing_error!("background_opacity requires a percentage".into(), node)
            })?;
            let opacity = entry
                .value()
                .as_i64()
                .and_then(|v| u8::try_from(v).ok())
                .or_else(|| {
                    let value = entry.value().as_string()?;
                    let digits = value.strip_suffix('%').unwrap_or(value);
                    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
                        .then(|| digits.parse::<u8>().ok())
                        .flatten()
                })
                .filter(|v| *v <= 100);
            opacity.ok_or_else(|| {
                kdl_parsing_error!(
                    "background_opacity must be a whole percentage from 0 to 100".into(),
                    entry
                )
            })
        })
        .transpose()?;
    let on_force_close =
        match kdl_property_first_arg_as_string_or_error!(kdl_options, "on_force_close") {
            Some((string, entry)) => Some(OnForceClose::from_str(string).map_err(|_| {
                kdl_parsing_error!(
                    format!("Invalid value for on_force_close: '{}'", string),
                    entry
                )
            })?),
            None => None,
        };
    let simplified_ui =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "simplified_ui").map(|(v, _)| v);
    let default_shell = kdl_property_first_arg_as_string_or_error!(kdl_options, "default_shell")
        .map(|(string, _entry)| PathBuf::from(string));
    let default_cwd = kdl_property_first_arg_as_string_or_error!(kdl_options, "default_cwd")
        .map(|(string, _entry)| PathBuf::from(string));
    let pane_animations =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "pane_animations").map(|(v, _)| v);
    let pane_frames =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "pane_frames").map(|(v, _)| v);
    let pane_frame_style =
        match kdl_property_first_arg_as_string_or_error!(kdl_options, "pane_frame_style") {
            Some((string, entry)) => Some(PaneFrameStyle::from_str(string).map_err(|_| {
                kdl_parsing_error!(
                    format!("Invalid value for pane_frame_style: '{}'", string),
                    entry
                )
            })?),
            None => None,
        };
    let notification_position =
        match kdl_property_first_arg_as_string_or_error!(kdl_options, "notification_position") {
            Some((string, entry)) => {
                Some(NotificationPosition::from_str(string).map_err(|_| {
                    kdl_parsing_error!(
                        format!("Invalid value for notification_position: '{}'", string),
                        entry
                    )
                })?)
            },
            None => None,
        };
    let notification_timeout = unsigned_option(kdl_options, "notification_timeout", 0)?
        .map(yinz_types::units::ElapsedSeconds::new);
    let auto_layout =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "auto_layout").map(|(v, _)| v);
    let theme = kdl_property_first_arg_as_string_or_error!(kdl_options, "theme")
        .map(|(theme, _entry)| theme.to_string());
    let theme_dark = kdl_property_first_arg_as_string_or_error!(kdl_options, "theme_dark")
        .map(|(theme, _entry)| theme.to_string());
    let theme_light = kdl_property_first_arg_as_string_or_error!(kdl_options, "theme_light")
        .map(|(theme, _entry)| theme.to_string());
    let explicit_theme_hue =
        match kdl_property_first_arg_as_string_or_error!(kdl_options, "explicit_theme_hue") {
            Some((string, entry)) => Some(ThemeHue::from_str(string).map_err(|_| {
                kdl_parsing_error!(
                    format!("Invalid value for explicit_theme_hue: '{}'", string),
                    entry
                )
            })?),
            None => None,
        };
    let default_mode =
        match kdl_property_first_arg_as_string_or_error!(kdl_options, "default_mode") {
            Some((string, entry)) => Some(InputMode::from_str(string).map_err(|_| {
                kdl_parsing_error!(format!("Invalid input mode: '{}'", string), entry)
            })?),
            None => None,
        };
    let default_layout = kdl_property_first_arg_as_string_or_error!(kdl_options, "default_layout")
        .map(|(string, _entry)| PathBuf::from(string));
    let layout_dir = kdl_property_first_arg_as_string_or_error!(kdl_options, "layout_dir")
        .map(|(string, _entry)| PathBuf::from(string));
    let theme_dir = kdl_property_first_arg_as_string_or_error!(kdl_options, "theme_dir")
        .map(|(string, _entry)| PathBuf::from(string));
    let user_plugin_dir =
        kdl_property_first_arg_as_string_or_error!(kdl_options, "user_plugin_dir")
            .map(|(string, _entry)| PathBuf::from(string));
    let agent_double_escape =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "agent_double_escape")
            .map(|(v, _)| v);
    let mouse_mode =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "mouse_mode").map(|(v, _)| v);
    let detachable_panes =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "detachable_panes").map(|(v, _)| v);
    let scroll_buffer_size = unsigned_option(kdl_options, "scroll_buffer_size", 0)?;
    let max_agent_width = unsigned_option(kdl_options, "max_agent_width", 0)?;
    let locked_escape_timeout_ms = unsigned_option(kdl_options, "locked_escape_timeout_ms", 0)?;
    let copy_command = kdl_property_first_arg_as_string_or_error!(kdl_options, "copy_command")
        .map(|(copy_command, _entry)| copy_command.to_string());
    let copy_clipboard =
        match kdl_property_first_arg_as_string_or_error!(kdl_options, "copy_clipboard") {
            Some((string, entry)) => Some(Clipboard::from_str(string).map_err(|_| {
                kdl_parsing_error!(
                    format!("Invalid value for copy_clipboard: '{}'", string),
                    entry
                )
            })?),
            None => None,
        };
    let copy_on_select =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "copy_on_select").map(|(v, _)| v);
    let osc8_hyperlinks =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "osc8_hyperlinks").map(|(v, _)| v);
    let scrollback_editor =
        kdl_property_first_arg_as_string_or_error!(kdl_options, "scrollback_editor")
            .map(|(string, _entry)| PathBuf::from(string));
    let mirror_session =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "mirror_session").map(|(v, _)| v);
    let session_name = kdl_property_first_arg_as_string_or_error!(kdl_options, "session_name")
        .map(|(session_name, _entry)| session_name.to_string());
    let attach_to_session =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "attach_to_session").map(|(v, _)| v);
    let session_serialization =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "session_serialization")
            .map(|(v, _)| v);
    let host_id = match kdl_property_first_arg_as_string_or_error!(kdl_options, "host_id") {
        Some((value, entry)) => {
            if !yinz_runtime::host_identity::is_valid_host_id(value) {
                return Err(kdl_parsing_error!(
                    format!(
                        "host_id must be 1-{} characters of letters, digits, `_` or `-`",
                        yinz_runtime::host_identity::MAX_HOST_ID_LEN
                    ),
                    entry
                ));
            }
            Some(HostId::new(value))
        },
        None => None,
    };
    let serialize_pane_viewport =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "serialize_pane_viewport")
            .map(|(v, _)| v);
    let scrollback_lines_to_serialize =
        unsigned_option(kdl_options, "scrollback_lines_to_serialize", 0)?;
    let styled_underlines =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "styled_underlines").map(|(v, _)| v);
    let serialization_interval = unsigned_option(kdl_options, "serialization_interval", 1)?
        .map(yinz_types::units::ElapsedSeconds::new);
    let disable_session_metadata =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "disable_session_metadata")
            .map(|(v, _)| v);
    let support_kitty_keyboard_protocol =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "support_kitty_keyboard_protocol")
            .map(|(v, _)| v);
    let report_modifier_keys =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "report_modifier_keys")
            .map(|(v, _)| v);
    let support_kitty_graphics_protocol =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "support_kitty_graphics_protocol")
            .map(|(v, _)| v);
    let stacked_resize =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "stacked_resize").map(|(v, _)| v);
    let agent_pane_min_columns = unsigned_option(kdl_options, "agent_pane_min_columns", 0)?;
    let agent_pane_min_rows = unsigned_option(kdl_options, "agent_pane_min_rows", 0)?;
    let stacked_pane_list =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "stacked_pane_list").map(|(v, _)| v);
    let advanced_mouse_actions =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "advanced_mouse_actions")
            .map(|(v, _)| v);
    let mouse_scroll_resize =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "mouse_scroll_resize")
            .map(|(v, _)| v);
    let mouse_hover_effects =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "mouse_hover_effects")
            .map(|(v, _)| v);
    let post_command_discovery_hook =
        kdl_property_first_arg_as_string_or_error!(kdl_options, "post_command_discovery_hook")
            .map(|(hook, _entry)| hook.to_string());
    let visual_bell =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "visual_bell").map(|(v, _)| v);
    let focus_follows_mouse =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "focus_follows_mouse")
            .map(|(v, _)| v);
    let context_menu_on_right_click =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "context_menu_on_right_click")
            .map(|(v, _)| v);
    let context_menu_override_agent_mouse =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "context_menu_override_agent_mouse")
            .map(|(v, _)| v);
    let mouse_click_through =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "mouse_click_through")
            .map(|(v, _)| v);
    let agent_theme_mapping =
        match kdl_property_first_arg_as_string_or_error!(kdl_options, "agent_theme_mapping") {
            Some((value, entry)) => Some(value.parse().map_err(|e| kdl_parsing_error!(e, entry))?),
            None => None,
        };
    let osc133_command_selection =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "osc133_command_selection")
            .map(|(v, _)| v);
    let word_separators =
        kdl_property_first_arg_as_string_or_error!(kdl_options, "word_separators")
            .map(|(separators, _entry)| separators.to_string());
    let nested_session_handling =
        match kdl_property_first_arg_as_string_or_error!(kdl_options, "nested_session_handling") {
            Some((value, entry)) => {
                use yinz_types::input::options::NestedSessionHandling;
                match value.parse::<NestedSessionHandling>() {
                    Ok(v) => Some(v),
                    Err(e) => return Err(kdl_parsing_error!(e, entry)),
                }
            },
            None => None,
        };
    let host_notification_protocol =
        match kdl_property_first_arg_as_string_or_error!(kdl_options, "host_notification_protocol")
        {
            Some((value, entry)) => {
                use yinz_types::input::options::HostNotificationProtocol;
                match value.parse::<HostNotificationProtocol>() {
                    Ok(v) => Some(v),
                    Err(e) => return Err(kdl_parsing_error!(e, entry)),
                }
            },
            None => None,
        };
    let dangerously_enable_paste_buffer_read = kdl_property_first_arg_as_bool_or_error!(
        kdl_options,
        "dangerously_enable_paste_buffer_read"
    )
    .map(|(v, _)| v);

    Ok(Options {
        simplified_ui,
        theme,
        theme_dark,
        theme_light,
        explicit_theme_hue,
        default_mode,
        default_shell,
        default_cwd,
        default_layout,
        layout_dir,
        theme_dir,
        user_plugin_dir,
        mouse_mode,
        agent_double_escape,
        detachable_panes,

        pane_frames,
        pane_animations,
        pane_frame_style,
        notification_position,
        notification_timeout,
        mirror_session,
        on_force_close,
        scroll_buffer_size,
        max_agent_width,
        locked_escape_timeout_ms: locked_escape_timeout_ms
            .map(yinz_types::units::ElapsedMillis::new),
        copy_command,
        copy_clipboard,
        copy_on_select,
        osc8_hyperlinks,
        scrollback_editor,
        session_name: session_name.map(Into::into),
        attach_to_session,
        auto_layout,
        session_serialization,
        host_id,
        serialize_pane_viewport,
        scrollback_lines_to_serialize,
        styled_underlines,
        background_opacity,
        serialization_interval,
        disable_session_metadata,
        support_kitty_keyboard_protocol,
        report_modifier_keys,
        support_kitty_graphics_protocol,
        stacked_resize,
        agent_pane_min_columns,
        agent_pane_min_rows,
        stacked_pane_list,
        advanced_mouse_actions,
        mouse_scroll_resize,
        mouse_hover_effects,
        visual_bell,
        focus_follows_mouse,
        mouse_click_through,
        context_menu_on_right_click,
        context_menu_override_agent_mouse,
        osc133_command_selection,
        agent_theme_mapping,
        word_separators,
        post_command_discovery_hook,
        nested_session_handling,
        host_notification_protocol,
        dangerously_enable_paste_buffer_read,
    })
}
pub fn from_string(stringified_keybindings: &str) -> Result<Options, ConfigError> {
    let document: KdlDocument = stringified_keybindings.parse()?;
    crate::kdl::options::from_kdl(&document)
}
fn simplified_ui_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Use a simplified UI without special fonts (arrow glyphs)",
        "// Options:",
        "//   - true",
        "//   - false (Default)",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("simplified_ui");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(simplified_ui) = value.simplified_ui {
        let mut node = create_node(simplified_ui);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(true);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn osc8_hyperlinks_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Enable OSC8 hyperlink output",
        "// Options:",
        "//   - true (Default)",
        "//   - false",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("osc8_hyperlinks");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(osc8_hyperlinks) = value.osc8_hyperlinks {
        let mut node = create_node(osc8_hyperlinks);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(true);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn theme_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// Choose the theme that is specified in the themes section.",
        "// Default: default",
        "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("theme");
        node.push(node_value.to_owned());
        node
    };
    if let Some(theme) = &value.theme {
        let mut node = create_node(theme);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("dracula");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn theme_dark_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// Theme to use when the host terminal reports a dark color palette.",
        "// Requires `theme_light` to also be set; otherwise `theme` is used.",
        "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("theme_dark");
        node.push(node_value.to_owned());
        node
    };
    if let Some(theme) = &value.theme_dark {
        let mut node = create_node(theme);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("dracula");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn theme_light_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// Theme to use when the host terminal reports a light color palette.",
        "// Requires `theme_dark` to also be set; otherwise `theme` is used.",
        "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("theme_light");
        node.push(node_value.to_owned());
        node
    };
    if let Some(theme) = &value.theme_light {
        let mut node = create_node(theme);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("solarized-light");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn explicit_theme_hue_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}",
        " ",
        "// Pin the session to a dark or light appearance, ignoring what the",
        "// host terminal reports. When unset, the host terminal decides.",
        "// Options: dark, light",
        "// ",
    );

    let create_node = |hue: &ThemeHue| -> KdlNode {
        let mut node = KdlNode::new("explicit_theme_hue");
        node.push(format!("{}", hue));
        node
    };
    if let Some(explicit_theme_hue) = &value.explicit_theme_hue {
        let mut node = create_node(explicit_theme_hue);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(&ThemeHue::Dark);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn default_mode_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ", "// Choose the base input mode of yinz.", "// Default: normal", "// "
    );

    let create_node = |default_mode: &InputMode| -> KdlNode {
        let mut node = KdlNode::new("default_mode");
        node.push(format!("{:?}", default_mode).to_lowercase());
        node
    };
    if let Some(default_mode) = &value.default_mode {
        let mut node = create_node(default_mode);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(&InputMode::Locked);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn default_shell_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// Choose the path to the default shell that yinz will use for opening new panes",
        "// Default: $SHELL",
        "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("default_shell");
        node.push(node_value.to_owned());
        node
    };
    if let Some(default_shell) = &value.default_shell {
        let mut node = create_node(&default_shell.display().to_string());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("fish");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn default_cwd_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ", "// Choose the path to override cwd that yinz will use for opening new panes", "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("default_cwd");
        node.push(node_value.to_owned());
        node
    };
    if let Some(default_cwd) = &value.default_cwd {
        let mut node = create_node(&default_cwd.display().to_string());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("/tmp");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn default_layout_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// The name of the default layout to load on startup",
        "// Default: \"default\"",
        "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("default_layout");
        node.push(node_value.to_owned());
        node
    };
    if let Some(default_layout) = &value.default_layout {
        let mut node = create_node(&default_layout.display().to_string());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("compact");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn layout_dir_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ", "// The folder in which Yinz will look for layouts", "// (Requires restart)", "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("layout_dir");
        node.push(node_value.to_owned());
        node
    };
    if let Some(layout_dir) = &value.layout_dir {
        let mut node = create_node(&layout_dir.display().to_string());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("/tmp");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn theme_dir_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ", "// The folder in which Yinz will look for themes", "// (Requires restart)", "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("theme_dir");
        node.push(node_value.to_owned());
        node
    };
    if let Some(theme_dir) = &value.theme_dir {
        let mut node = create_node(&theme_dir.display().to_string());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("/tmp");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn user_plugin_dir_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// The folder in which Yinz will look for plugins you wrote yourself",
        "// (Requires restart)",
        "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("user_plugin_dir");
        node.push(node_value.to_owned());
        node
    };
    if let Some(user_plugin_dir) = &value.user_plugin_dir {
        let mut node = create_node(&user_plugin_dir.display().to_string());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("/tmp");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn detachable_panes_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Run each pane's process under a small supervisor process, so that",
        "// panes keep running when the session's server exits (for example if",
        "// it is killed or crashes).",
        "// Options:",
        "//   - true (default)",
        "//   - false",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("detachable_panes");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(detachable_panes) = value.detachable_panes {
        let mut node = create_node(detachable_panes);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(true);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn locked_escape_timeout_ms_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ",
        "// How long a locked-mode Ctrl+g waits for a plain g that would quote it into",
        "// the pane, in milliseconds. 0 unlocks immediately and never quotes Ctrl+g.",
    );
    let create_node = |value: u64| {
        let mut node = KdlNode::new("locked_escape_timeout_ms");
        node.push(value as i64);
        node
    };
    match value.locked_escape_timeout_ms {
        Some(timeout) => {
            let mut node = create_node((timeout).get());
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        },
        None if add_comments => {
            let mut node = create_node(250);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        },
        None => None,
    }
}

fn agent_double_escape_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text =
        "// Require two plain Escape presses within one second in panes with an agent record.
// The first press is discarded; the second sends one Escape. Disabled by default.";

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("agent_double_escape");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(agent_double_escape) = value.agent_double_escape {
        let mut node = create_node(agent_double_escape);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn mouse_mode_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Toggle enabling the mouse mode.",
        "// On certain configurations, or terminals this could",
        "// potentially interfere with copying text.",
        "// Options:",
        "//   - true (default)",
        "//   - false",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("mouse_mode");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(mouse_mode) = value.mouse_mode {
        let mut node = create_node(mouse_mode);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn pane_animations_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ", "// Whether to animate pane opening effects", "// default is true",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("pane_animations");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(pane_animations) = value.pane_animations {
        let mut node = create_node(pane_animations);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}

fn pane_frames_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Toggle having pane frames around the panes",
        "// Options:",
        "//   - true (default, enabled)",
        "//   - false",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("pane_frames");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(pane_frames) = value.pane_frames {
        let mut node = create_node(pane_frames);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn pane_frame_style_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Set the pane frame style when pane_frames is enabled",
        "// Options:",
        "//   - full",
        "//   - titles (default)",
        "// ",
    );

    let style_as_str = |style: &PaneFrameStyle| -> &'static str {
        match style {
            PaneFrameStyle::Full => "full",
            PaneFrameStyle::Titles => "titles",
            PaneFrameStyle::None => "none",
        }
    };

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("pane_frame_style");
        node.push(node_value.to_owned());
        node
    };
    if let Some(pane_frame_style) = &value.pane_frame_style {
        let mut node = create_node(style_as_str(pane_frame_style));
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("titles");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn notification_position_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// The corner notifications appear in - they stack downwards from a top corner",
        "// and upwards from a bottom one",
        "// Options:",
        "//   - top-left",
        "//   - top-right (default)",
        "//   - bottom-left",
        "//   - bottom-right",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("notification_position");
        node.push(node_value.to_owned());
        node
    };
    if let Some(notification_position) = &value.notification_position {
        let mut node = create_node(notification_position.as_str());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(NotificationPosition::default().as_str());
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn notification_timeout_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ", "// How long a notification stays on screen, in seconds", "// Default: 4",
    );

    let create_node = |node_value: u64| -> KdlNode {
        let mut node = KdlNode::new("notification_timeout");
        node.push(i64::from_ne_bytes(node_value.to_ne_bytes()));
        node
    };
    if let Some(notification_timeout) = value.notification_timeout {
        let mut node = create_node((notification_timeout).get());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(DEFAULT_NOTIFICATION_TIMEOUT_SECS);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn mirror_session_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// When attaching to an existing session with other users,",
        "// should the session be mirrored (true)",
        "// or should each user have their own cursor (false)",
        "// (Requires restart)",
        "// Default: false",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("mirror_session");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(mirror_session) = value.mirror_session {
        let mut node = create_node(mirror_session);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(true);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn on_force_close_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Choose what to do when yinz receives SIGTERM, SIGINT, SIGQUIT or SIGHUP",
        "// eg. when terminal window with an active yinz session is closed",
        "// (Requires restart)",
        "// Options:",
        "//   - detach (Default)",
        "//   - quit",
        "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("on_force_close");
        node.push(node_value.to_owned());
        node
    };
    if let Some(on_force_close) = &value.on_force_close {
        let mut node = match on_force_close {
            OnForceClose::Detach => create_node("detach"),
            OnForceClose::Quit => create_node("quit"),
        };
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("quit");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn scroll_buffer_size_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Configure the scroll back buffer size",
        "// This is the number of lines yinz stores for each pane in the scroll back",
        "// buffer. Excess number of lines are discarded in a FIFO fashion.",
        "// (Requires restart)",
        "// Valid values: positive integers",
        "// Default value: 10000",
        "// ",
    );

    let create_node = |node_value: usize| -> KdlNode {
        let mut node = KdlNode::new("scroll_buffer_size");
        node.push(KdlValue::Base10(node_value as i64));
        node
    };
    if let Some(scroll_buffer_size) = value.scroll_buffer_size {
        let mut node = create_node(scroll_buffer_size);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(10000);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn max_agent_width_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    if value.max_agent_width.is_none() && !add_comments {
        return None;
    }
    let mut node = KdlNode::new("max_agent_width");
    node.push(KdlValue::Base10(value.max_agent_width.unwrap_or(0) as i64));
    if add_comments {
        let prefix = if value.max_agent_width.is_some() {
            ""
        } else {
            "// "
        };
        node.set_leading(format!("\n// Limit agent panes in all layouts to this many text columns.\n// Center the grid when it cannot fill the screen. Frames are outside the text width.\n// Values below 80 disable the limit. Default: 0 (unrestricted).\n{prefix}"));
    }
    Some(node)
}
fn copy_command_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Provide a command to execute when copying text. The text will be piped to",
        "// the stdin of the program to perform the copy. This can be used with",
        "// terminal emulators which do not support the OSC 52 ANSI control sequence",
        "// that will be used by default if this option is not set.",
        "// Examples:",
        "//",
        "// copy_command \"xclip -selection clipboard\" // x11",
        "// copy_command \"wl-copy\"                    // wayland",
        "// copy_command \"pbcopy\"                     // osx",
        "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("copy_command");
        node.push(node_value.to_owned());
        node
    };
    if let Some(copy_command) = &value.copy_command {
        let mut node = create_node(copy_command);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("pbcopy");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn copy_clipboard_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!("{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Choose the destination for copied text",
            "// Allows using the primary selection buffer (on x11/wayland) instead of the system clipboard.",
            "// Does not apply when using copy_command.",
            "// Options:",
            "//   - system (default)",
            "//   - primary",
            "// ",
        );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("copy_clipboard");
        node.push(node_value.to_owned());
        node
    };
    if let Some(copy_clipboard) = &value.copy_clipboard {
        let mut node = match copy_clipboard {
            Clipboard::Primary => create_node("primary"),
            Clipboard::System => create_node("system"),
        };
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("primary");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn copy_on_select_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// Enable automatic copying (and clearing) of selection when releasing mouse",
        "// Default: true",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("copy_on_select");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(copy_on_select) = value.copy_on_select {
        let mut node = create_node(copy_on_select);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(true);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn scrollback_editor_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ",
        "// Path to the default editor to use to edit pane scrollbuffer",
        "// Default: $EDITOR or $VISUAL",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("scrollback_editor");
        node.push(node_value.to_owned());
        node
    };
    if let Some(scrollback_editor) = &value.scrollback_editor {
        let mut node = create_node(&scrollback_editor.display().to_string());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("/usr/bin/vim");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn session_name_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// A fixed name to always give the Yinz session.",
        "// Consider also setting `attach_to_session true,`",
        "// otherwise this will error if such a session exists.",
        "// Default: <RANDOM>",
        "// ",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("session_name");
        node.push(node_value.to_owned());
        node
    };
    if let Some(session_name) = &value.session_name {
        let mut node = create_node(session_name.as_str());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("My singleton session");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn attach_to_session_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}",
        " ",
        "// When `session_name` is provided, attaches to that session",
        "// if it is already running or creates it otherwise.",
        "// Default: false",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("attach_to_session");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(attach_to_session) = value.attach_to_session {
        let mut node = create_node(attach_to_session);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(true);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn auto_layout_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!("{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Toggle between having Yinz lay out panes according to a predefined set of layouts whenever possible",
            "// Options:",
            "//   - true (default)",
            "//   - false",
            "// ",
        );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("auto_layout");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(auto_layout) = value.auto_layout {
        let mut node = create_node(auto_layout);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn session_serialization_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!("{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Whether sessions should be serialized to the cache folder (including their tabs/panes, cwds and running commands) so that they can later be resurrected",
            "// Options:",
            "//   - true (default)",
            "//   - false",
            "// ",
        );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("session_serialization");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(session_serialization) = value.session_serialization {
        let mut node = create_node(session_serialization);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn host_id_to_kdl(value: &Options) -> Option<KdlNode> {
    value.host_id.as_ref().map(|host_id| {
        let mut node = KdlNode::new("host_id");
        node.push(host_id.as_str());
        node
    })
}
fn serialize_pane_viewport_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Whether pane viewports are serialized along with the session, default is false",
        "// Options:",
        "//   - true",
        "//   - false (default)",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("serialize_pane_viewport");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(serialize_pane_viewport) = value.serialize_pane_viewport {
        let mut node = create_node(serialize_pane_viewport);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn scrollback_lines_to_serialize_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!("{}\n{}\n{}\n{}\n{}",
            " ",
            "// Scrollback lines to serialize along with the pane viewport when serializing sessions, 0",
            "// defaults to the scrollback size. If this number is higher than the scrollback size, it will",
            "// also default to the scrollback size. This does nothing if `serialize_pane_viewport` is not true.",
            "// ",
        );

    let create_node = |node_value: usize| -> KdlNode {
        let mut node = KdlNode::new("scrollback_lines_to_serialize");
        node.push(KdlValue::Base10(node_value as i64));
        node
    };
    if let Some(scrollback_lines_to_serialize) = value.scrollback_lines_to_serialize {
        let mut node = create_node(scrollback_lines_to_serialize);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(10000);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn styled_underlines_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Enable or disable the rendering of styled and colored underlines (undercurl).",
        "// May need to be disabled for certain unsupported terminals",
        "// (Requires restart)",
        "// Default: true",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("styled_underlines");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(styled_underlines) = value.styled_underlines {
        let mut node = create_node(styled_underlines);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn serialization_interval_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ", "// How often in seconds sessions are serialized", "// ",
    );

    let create_node = |node_value: u64| -> KdlNode {
        let mut node = KdlNode::new("serialization_interval");
        node.push(KdlValue::Base10(i64::from_ne_bytes(
            node_value.to_ne_bytes(),
        )));
        node
    };
    if let Some(serialization_interval) = value.serialization_interval {
        let mut node = create_node((serialization_interval).get());
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(10000);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn disable_session_metadata_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!("{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Enable or disable writing of session metadata to disk (if disabled, other sessions might not know",
            "// metadata info on this session)",
            "// (Requires restart)",
            "// Default: false",
            "// ",
        );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("disable_session_metadata");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(disable_session_metadata) = value.disable_session_metadata {
        let mut node = create_node(disable_session_metadata);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn support_kitty_keyboard_protocol_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!("{}\n{}\n{}\n{}\n{}",
            " ",
            "// Enable or disable support for the enhanced Kitty Keyboard Protocol (the host terminal must also support it)",
            "// (Requires restart)",
            "// Default: true (if the host terminal supports it)",
            "// ",
        );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("support_kitty_keyboard_protocol");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(support_kitty_keyboard_protocol) = value.support_kitty_keyboard_protocol {
        let mut node = create_node(support_kitty_keyboard_protocol);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn report_modifier_keys_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// Ask the host terminal to report the modifier keys themselves, so the UI can show",
        "// something for as long as eg. Alt is held. Needs Kitty Keyboard flags that also make",
        "// every printable key arrive as an escape code: AltGr glyphs, dead keys and IME input",
        "// will stop working. (Requires restart) Default: false",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("report_modifier_keys");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(report_modifier_keys) = value.report_modifier_keys {
        let mut node = create_node(report_modifier_keys);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn support_kitty_graphics_protocol_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!("{}\n{}\n{}\n{}\n{}",
            " ",
            "// Enable or disable support for the Kitty Graphics Protocol, used to display images (the host terminal must also support it)",
            "// (Requires restart)",
            "// Default: true (if the host terminal supports it)",
            "// ",
        );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("support_kitty_graphics_protocol");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(support_kitty_graphics_protocol) = value.support_kitty_graphics_protocol {
        let mut node = create_node(support_kitty_graphics_protocol);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn agent_pane_min_columns_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        " ",
        "// The smallest tiled pane worth giving a newly spawned coding agent, as a terminal",
        "// size. Before an agent pane joins a tab, yinz projects the area each tiled pane",
        "// would have if the tab's tiled panes (floating ones aside) shared the viewport",
        "// equally, and compares it with columns x rows. A tab that would fall below it",
        "// sends the agent to the roomiest other tab that stays above it, or to a new tab.",
        "// Set either to 0 to always open agents where they were asked for.",
        "// Default: 80 and 24",
        "// ",
        "// "
    );
    let create_node = |value: usize| {
        let mut node = KdlNode::new("agent_pane_min_columns");
        node.push(value as i64);
        node
    };
    match value.agent_pane_min_columns {
        Some(value) => Some(create_node(value)),
        None if add_comments => {
            let mut node = create_node(DEFAULT_AGENT_PANE_MIN_COLUMNS);
            node.set_leading(comment_text);
            Some(node)
        },
        None => None,
    }
}
fn agent_pane_min_rows_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let create_node = |value: usize| {
        let mut node = KdlNode::new("agent_pane_min_rows");
        node.push(value as i64);
        node
    };
    match value.agent_pane_min_rows {
        Some(value) => Some(create_node(value)),
        None if add_comments => {
            let mut node = create_node(DEFAULT_AGENT_PANE_MIN_ROWS);
            node.set_leading("// ".to_owned());
            Some(node)
        },
        None => None,
    }
}
fn stacked_resize_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// Whether to stack panes when resizing beyond a certain size",
        "// Default: true",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("stacked_resize");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(stacked_resize) = value.stacked_resize {
        let mut node = create_node(stacked_resize);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn stacked_pane_list_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// Whether stacked panes display as a list with the expanded pane pinned to the bottom",
        "// Default: true",
        "// ",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("stacked_pane_list");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(stacked_pane_list) = value.stacked_pane_list {
        let mut node = create_node(stacked_pane_list);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn advanced_mouse_actions_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ",
        "// Whether to enable mouse hover effects and pane grouping functionality",
        "// default is true",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("advanced_mouse_actions");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(advanced_mouse_actions) = value.advanced_mouse_actions {
        let mut node = create_node(advanced_mouse_actions);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn mouse_scroll_resize_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ", "// Whether Ctrl+ScrollWheel resizes panes", "// default is true",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("mouse_scroll_resize");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(mouse_scroll_resize) = value.mouse_scroll_resize {
        let mut node = create_node(mouse_scroll_resize);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn mouse_hover_effects_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ",
        "// Whether to enable mouse hover visual effects (frame highlight and help text)",
        "// default is true",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("mouse_hover_effects");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(mouse_hover_effects) = value.mouse_hover_effects {
        let mut node = create_node(mouse_hover_effects);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn visual_bell_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ",
        "// Whether to show visual bell indicators (pane/tab frame flash and [!] suffix)",
        "// default is true",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("visual_bell");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(visual_bell) = value.visual_bell {
        let mut node = create_node(visual_bell);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(true);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn focus_follows_mouse_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ", "// Whether to focus panes on mouse hover", "// default is false",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("focus_follows_mouse");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(focus_follows_mouse) = value.focus_follows_mouse {
        let mut node = create_node(focus_follows_mouse);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn mouse_click_through_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}",
        " ",
        "// Whether clicking a pane to focus it also sends the click into the pane",
        "// default is false",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("mouse_click_through");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(mouse_click_through) = value.mouse_click_through {
        let mut node = create_node(mouse_click_through);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn context_menu_on_right_click_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// Whether right-clicking a pane opens a context menu. A program that is tracking the",
        "// mouse keeps right-click, except agents with context_menu_override_agent_mouse true.",
        "// default is true",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("context_menu_on_right_click");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(context_menu_on_right_click) = value.context_menu_on_right_click {
        let mut node = create_node(context_menu_on_right_click);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(true);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn context_menu_override_agent_mouse_to_kdl(
    value: &Options,
    add_comments: bool,
) -> Option<KdlNode> {
    let comment_text = "\n// Whether recognized agents open the context menu even while tracking the mouse.\n// Requires context_menu_on_right_click. Set false to let agents handle right-click.\n// default is true";

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("context_menu_override_agent_mouse");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(context_menu_override_agent_mouse) = value.context_menu_override_agent_mouse {
        let mut node = create_node(context_menu_override_agent_mouse);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(true);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn osc133_command_selection_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// Whether triple-clicking inside command output marked by the shell (OSC 133) selects",
        "// the command and its output instead of the logical line",
        "// default is true",
    );

    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("osc133_command_selection");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(osc133_command_selection) = value.osc133_command_selection {
        let mut node = create_node(osc133_command_selection);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn word_separators_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}",
        " ",
        "// Characters that terminate a word when double-clicking to select it",
        "// whitespace is always a separator and need not be listed here",
        "// default is \"[]{}<>()\"",
    );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("word_separators");
        node.push(node_value.to_owned());
        node
    };
    if let Some(word_separators) = &value.word_separators {
        let mut node = create_node(word_separators);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(DEFAULT_WORD_SEPARATORS);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn post_command_discovery_hook_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// A command to run (will be wrapped with sh -c and provided the RESURRECT_COMMAND env variable) ",
            "// after Yinz attempts to discover a command inside a pane when resurrecting sessions, the STDOUT",
            "// of this command will be used instead of the discovered RESURRECT_COMMAND",
            "// can be useful for removing wrappers around commands",
            "// Note: be sure to escape backslashes and similar characters properly",
        );

    let create_node = |node_value: &str| -> KdlNode {
        let mut node = KdlNode::new("post_command_discovery_hook");
        node.push(node_value.to_owned());
        node
    };
    if let Some(post_command_discovery_hook) = &value.post_command_discovery_hook {
        let mut node = create_node(post_command_discovery_hook);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node("echo $RESURRECT_COMMAND | sed <your_regex_here>");
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn nested_session_handling_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    use yinz_types::input::options::NestedSessionHandling;
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        "",
        "// How to handle a nested Yinz session detected inside a pane.",
        "// Options:",
        "//   - \"ask\" (Default — prompt with a modal)",
        "//   - \"fullscreen\" (always zoom into the nested session)",
        "//   - \"descend\" (always control the nested session on focus)",
        "//   - \"never\" (never prompt or descend; do it manually)",
        "// ",
    );
    let create_node = |value: NestedSessionHandling| -> KdlNode {
        let mut node = KdlNode::new("nested_session_handling");
        let s = match value {
            NestedSessionHandling::Ask => "ask",
            NestedSessionHandling::Fullscreen => "fullscreen",
            NestedSessionHandling::Descend => "descend",
            NestedSessionHandling::Never => "never",
        };
        node.push(KdlValue::String(s.to_string()));
        node
    };
    if let Some(value) = value.nested_session_handling {
        let mut node = create_node(value);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(NestedSessionHandling::Ask);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn host_notification_protocol_to_kdl(value: &Options, add_comments: bool) -> Option<KdlNode> {
    use yinz_types::input::options::HostNotificationProtocol;
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        "",
        "// Which escape sequence desktop notifications coming from panes are",
        "// forwarded to the host terminal with.",
        "// Options:",
        "//   - \"auto\" (Default — detect from the host terminal environment)",
        "//   - \"osc9\" (the legacy iTerm2 protocol, understood by most terminals)",
        "//   - \"osc99\" (kitty's notification protocol)",
        "//   - \"bell\" (ring the terminal bell instead)",
        "//   - \"off\" (do not forward notifications to the host terminal)",
        "// ",
    );
    let create_node = |value: HostNotificationProtocol| -> KdlNode {
        let mut node = KdlNode::new("host_notification_protocol");
        node.push(KdlValue::String(value.as_str().to_string()));
        node
    };
    if let Some(value) = value.host_notification_protocol {
        let mut node = create_node(value);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(HostNotificationProtocol::Auto);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
fn dangerously_enable_paste_buffer_read_to_kdl(
    value: &Options,
    add_comments: bool,
) -> Option<KdlNode> {
    let comment_text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        "",
        "// Whether to let programs running inside panes read the paste buffer",
        "// (clipboard) with the OSC 52 escape sequence. When enabled, any program",
        "// in any pane - including one running on a remote machine over SSH - can",
        "// read the clipboard without the user being asked.",
        "// Default: false",
    );
    let create_node = |node_value: bool| -> KdlNode {
        let mut node = KdlNode::new("dangerously_enable_paste_buffer_read");
        node.push(KdlValue::Bool(node_value));
        node
    };
    if let Some(value) = value.dangerously_enable_paste_buffer_read {
        let mut node = create_node(value);
        if add_comments {
            node.set_leading(format!("{}\n", comment_text));
        }
        Some(node)
    } else if add_comments {
        let mut node = create_node(false);
        node.set_leading(format!("{}\n// ", comment_text));
        Some(node)
    } else {
        None
    }
}
pub fn to_kdl(value: &Options, add_comments: bool) -> Vec<KdlNode> {
    let mut nodes = vec![];
    if let Some(simplified_ui_node) = simplified_ui_to_kdl(value, add_comments) {
        nodes.push(simplified_ui_node);
    }
    if let Some(osc8_hyperlinks_node) = osc8_hyperlinks_to_kdl(value, add_comments) {
        nodes.push(osc8_hyperlinks_node);
    }
    if let Some(theme_node) = theme_to_kdl(value, add_comments) {
        nodes.push(theme_node);
    }
    if let Some(theme_dark_node) = theme_dark_to_kdl(value, add_comments) {
        nodes.push(theme_dark_node);
    }
    if let Some(theme_light_node) = theme_light_to_kdl(value, add_comments) {
        nodes.push(theme_light_node);
    }
    if let Some(explicit_theme_hue_node) = explicit_theme_hue_to_kdl(value, add_comments) {
        nodes.push(explicit_theme_hue_node);
    }
    if let Some(default_mode) = default_mode_to_kdl(value, add_comments) {
        nodes.push(default_mode);
    }
    if let Some(default_shell) = default_shell_to_kdl(value, add_comments) {
        nodes.push(default_shell);
    }
    if let Some(default_cwd) = default_cwd_to_kdl(value, add_comments) {
        nodes.push(default_cwd);
    }
    if let Some(default_layout) = default_layout_to_kdl(value, add_comments) {
        nodes.push(default_layout);
    }
    if let Some(layout_dir) = layout_dir_to_kdl(value, add_comments) {
        nodes.push(layout_dir);
    }
    if let Some(theme_dir) = theme_dir_to_kdl(value, add_comments) {
        nodes.push(theme_dir);
    }
    if let Some(user_plugin_dir) = user_plugin_dir_to_kdl(value, add_comments) {
        nodes.push(user_plugin_dir);
    }
    if let Some(detachable_panes) = detachable_panes_to_kdl(value, add_comments) {
        nodes.push(detachable_panes);
    }
    if let Some(node) = agent_double_escape_to_kdl(value, add_comments) {
        nodes.push(node);
    }
    if let Some(mouse_mode) = mouse_mode_to_kdl(value, add_comments) {
        nodes.push(mouse_mode);
    }
    if let Some(node) = pane_animations_to_kdl(value, add_comments) {
        nodes.push(node);
    }
    if let Some(pane_frames) = pane_frames_to_kdl(value, add_comments) {
        nodes.push(pane_frames);
    }
    if let Some(pane_frame_style) = pane_frame_style_to_kdl(value, add_comments) {
        nodes.push(pane_frame_style);
    }
    if let Some(notification_position) = notification_position_to_kdl(value, add_comments) {
        nodes.push(notification_position);
    }
    if let Some(notification_timeout) = notification_timeout_to_kdl(value, add_comments) {
        nodes.push(notification_timeout);
    }
    if let Some(mirror_session) = mirror_session_to_kdl(value, add_comments) {
        nodes.push(mirror_session);
    }
    if let Some(on_force_close) = on_force_close_to_kdl(value, add_comments) {
        nodes.push(on_force_close);
    }
    if let Some(width) = max_agent_width_to_kdl(value, add_comments) {
        nodes.push(width);
    }
    if let Some(scroll_buffer_size) = scroll_buffer_size_to_kdl(value, add_comments) {
        nodes.push(scroll_buffer_size);
    }
    if let Some(locked_escape_timeout_ms) = locked_escape_timeout_ms_to_kdl(value, add_comments) {
        nodes.push(locked_escape_timeout_ms);
    }
    if let Some(copy_command) = copy_command_to_kdl(value, add_comments) {
        nodes.push(copy_command);
    }
    if let Some(copy_clipboard) = copy_clipboard_to_kdl(value, add_comments) {
        nodes.push(copy_clipboard);
    }
    if let Some(copy_on_select) = copy_on_select_to_kdl(value, add_comments) {
        nodes.push(copy_on_select);
    }
    if let Some(scrollback_editor) = scrollback_editor_to_kdl(value, add_comments) {
        nodes.push(scrollback_editor);
    }
    if let Some(session_name) = session_name_to_kdl(value, add_comments) {
        nodes.push(session_name);
    }
    if let Some(attach_to_session) = attach_to_session_to_kdl(value, add_comments) {
        nodes.push(attach_to_session);
    }
    if let Some(auto_layout) = auto_layout_to_kdl(value, add_comments) {
        nodes.push(auto_layout);
    }
    if let Some(session_serialization) = session_serialization_to_kdl(value, add_comments) {
        nodes.push(session_serialization);
    }
    if let Some(host_id) = host_id_to_kdl(value) {
        nodes.push(host_id);
    }
    if let Some(serialize_pane_viewport) = serialize_pane_viewport_to_kdl(value, add_comments) {
        nodes.push(serialize_pane_viewport);
    }
    if let Some(scrollback_lines_to_serialize) =
        scrollback_lines_to_serialize_to_kdl(value, add_comments)
    {
        nodes.push(scrollback_lines_to_serialize);
    }
    if let Some(opacity) = value.background_opacity {
        let mut node = KdlNode::new("background_opacity");
        node.push(format!("{opacity}%"));
        if add_comments {
            node.set_leading("\n// Background opacity for Agent, the command palette and translucent overlays (default: 85%).\n// Text and controls remain opaque. 0 is clear; 100 is solid.\n");
        }
        nodes.push(node);
    }
    if let Some(styled_underlines) = styled_underlines_to_kdl(value, add_comments) {
        nodes.push(styled_underlines);
    }
    if let Some(serialization_interval) = serialization_interval_to_kdl(value, add_comments) {
        nodes.push(serialization_interval);
    }
    if let Some(disable_session_metadata) = disable_session_metadata_to_kdl(value, add_comments) {
        nodes.push(disable_session_metadata);
    }
    if let Some(support_kitty_keyboard_protocol) =
        support_kitty_keyboard_protocol_to_kdl(value, add_comments)
    {
        nodes.push(support_kitty_keyboard_protocol);
    }
    if let Some(report_modifier_keys) = report_modifier_keys_to_kdl(value, add_comments) {
        nodes.push(report_modifier_keys);
    }
    if let Some(support_kitty_graphics_protocol) =
        support_kitty_graphics_protocol_to_kdl(value, add_comments)
    {
        nodes.push(support_kitty_graphics_protocol);
    }
    if let Some(stacked_resize) = stacked_resize_to_kdl(value, add_comments) {
        nodes.push(stacked_resize);
    }
    if let Some(agent_pane_min_columns) = agent_pane_min_columns_to_kdl(value, add_comments) {
        nodes.push(agent_pane_min_columns);
    }
    if let Some(agent_pane_min_rows) = agent_pane_min_rows_to_kdl(value, add_comments) {
        nodes.push(agent_pane_min_rows);
    }
    if let Some(stacked_pane_list) = stacked_pane_list_to_kdl(value, add_comments) {
        nodes.push(stacked_pane_list);
    }
    if let Some(advanced_mouse_actions) = advanced_mouse_actions_to_kdl(value, add_comments) {
        nodes.push(advanced_mouse_actions);
    }
    if let Some(mouse_scroll_resize) = mouse_scroll_resize_to_kdl(value, add_comments) {
        nodes.push(mouse_scroll_resize);
    }
    if let Some(mouse_hover_effects) = mouse_hover_effects_to_kdl(value, add_comments) {
        nodes.push(mouse_hover_effects);
    }
    if let Some(visual_bell) = visual_bell_to_kdl(value, add_comments) {
        nodes.push(visual_bell);
    }
    if let Some(focus_follows_mouse) = focus_follows_mouse_to_kdl(value, add_comments) {
        nodes.push(focus_follows_mouse);
    }
    if let Some(mouse_click_through) = mouse_click_through_to_kdl(value, add_comments) {
        nodes.push(mouse_click_through);
    }
    if let Some(context_menu_on_right_click) =
        context_menu_on_right_click_to_kdl(value, add_comments)
    {
        nodes.push(context_menu_on_right_click);
    }
    if let Some(context_menu_override_agent_mouse) =
        context_menu_override_agent_mouse_to_kdl(value, add_comments)
    {
        nodes.push(context_menu_override_agent_mouse);
    }
    if let Some(mapping) = value.agent_theme_mapping {
        let mut node = KdlNode::new("agent_theme_mapping");
        node.push(mapping.to_string());
        nodes.push(node);
    }
    if let Some(osc133_command_selection) = osc133_command_selection_to_kdl(value, add_comments) {
        nodes.push(osc133_command_selection);
    }
    if let Some(word_separators) = word_separators_to_kdl(value, add_comments) {
        nodes.push(word_separators);
    }
    if let Some(post_command_discovery_hook) =
        post_command_discovery_hook_to_kdl(value, add_comments)
    {
        nodes.push(post_command_discovery_hook);
    }
    if let Some(dangerously_enable_paste_buffer_read) =
        dangerously_enable_paste_buffer_read_to_kdl(value, add_comments)
    {
        nodes.push(dangerously_enable_paste_buffer_read);
    }
    if let Some(nested_session_handling) = nested_session_handling_to_kdl(value, add_comments) {
        nodes.push(nested_session_handling);
    }
    if let Some(host_notification_protocol) = host_notification_protocol_to_kdl(value, add_comments)
    {
        nodes.push(host_notification_protocol);
    }
    nodes
}
