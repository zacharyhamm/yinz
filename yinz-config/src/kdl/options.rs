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
    let tab_groups =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "tab_groups").map(|(v, _)| v);
    let simplified_ui =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "simplified_ui").map(|(v, _)| v);
    let disable_splash_screen =
        kdl_property_first_arg_as_bool_or_error!(kdl_options, "disable_splash_screen")
            .map(|(v, _)| v);
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
        tab_groups,
        simplified_ui,
        disable_splash_screen,
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

pub fn to_kdl(value: &Options, add_comments: bool) -> Vec<KdlNode> {
    use yinz_types::input::options::{HostNotificationProtocol, NestedSessionHandling};

    let mut nodes = vec![];
    let mut add_option =
        |name: &str, value: Option<KdlValue>, example: Option<KdlValue>, comment: &str| {
            let is_example = value.is_none();
            let Some(node_value) = value.or_else(|| example.filter(|_| add_comments)) else {
                return;
            };
            let mut node = KdlNode::new(name);
            node.push(node_value);
            if add_comments && (!comment.is_empty() || is_example) {
                let prefix = if is_example { "// " } else { "" };
                node.set_leading(format!("{comment}{prefix}"));
            }
            nodes.push(node);
        };
    add_option(
        "tab_groups",
        value.tab_groups.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            "\n",
            "// Enable tab groups. Disable to show all tabs in one bar. Default: true.\n",
        ),
    );
    add_option(
        "simplified_ui",
        value.simplified_ui.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            " \n",
            "// Use a simplified UI without special fonts (arrow glyphs)\n",
            "// Options:\n",
            "//   - true\n",
            "//   - false (Default)\n",
            "// \n",
        ),
    );
    add_option(
        "disable_splash_screen",
        value.disable_splash_screen.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            "\n",
            "// Skip the startup splash and its minimum display time.\n",
            "// Default: false\n",
            "//\n",
        ),
    );
    add_option(
        "osc8_hyperlinks",
        value.osc8_hyperlinks.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            " \n",
            "// Enable OSC8 hyperlink output\n",
            "// Options:\n",
            "//   - true (Default)\n",
            "//   - false\n",
            "// \n",
        ),
    );
    add_option(
        "theme",
        value.theme.as_deref().map(Into::into),
        Some("dracula".into()),
        concat!(
            " \n",
            "// Choose the theme that is specified in the themes section.\n",
            "// Default: default\n",
            "// \n",
        ),
    );
    add_option(
        "theme_dark",
        value.theme_dark.as_deref().map(Into::into),
        Some("dracula".into()),
        concat!(
            " \n",
            "// Theme to use when the host terminal reports a dark color palette.\n",
            "// Requires `theme_light` to also be set; otherwise `theme` is used.\n",
            "// \n",
        ),
    );
    add_option(
        "theme_light",
        value.theme_light.as_deref().map(Into::into),
        Some("solarized-light".into()),
        concat!(
            " \n",
            "// Theme to use when the host terminal reports a light color palette.\n",
            "// Requires `theme_dark` to also be set; otherwise `theme` is used.\n",
            "// \n",
        ),
    );
    add_option(
        "explicit_theme_hue",
        value
            .explicit_theme_hue
            .as_ref()
            .map(|hue| hue.to_string().into()),
        Some(ThemeHue::Dark.to_string().into()),
        concat!(
            " \n",
            "// Pin the session to a dark or light appearance, ignoring what the\n",
            "// host terminal reports. When unset, the host terminal decides.\n",
            "// Options: dark, light\n",
            "// \n",
        ),
    );
    add_option(
        "default_mode",
        value
            .default_mode
            .as_ref()
            .map(|mode| format!("{mode:?}").to_lowercase().into()),
        Some(format!("{:?}", InputMode::Locked).to_lowercase().into()),
        concat!(
            " \n",
            "// Choose the base input mode of yinz.\n",
            "// Default: normal\n",
            "// \n",
        ),
    );
    add_option(
        "default_shell",
        value
            .default_shell
            .as_ref()
            .map(|path| path.display().to_string().into()),
        Some("fish".into()),
        concat!(
            " \n",
            "// Choose the path to the default shell that yinz will use for opening new panes\n",
            "// Default: $SHELL\n",
            "// \n",
        ),
    );
    add_option(
        "default_cwd",
        value
            .default_cwd
            .as_ref()
            .map(|path| path.display().to_string().into()),
        Some("/tmp".into()),
        concat!(
            " \n",
            "// Choose the path to override cwd that yinz will use for opening new panes\n",
            "// \n",
        ),
    );
    add_option(
        "default_layout",
        value
            .default_layout
            .as_ref()
            .map(|path| path.display().to_string().into()),
        Some("compact".into()),
        concat!(
            " \n",
            "// The name of the default layout to load on startup\n",
            "// Default: \"default\"\n",
            "// \n",
        ),
    );
    add_option(
        "layout_dir",
        value
            .layout_dir
            .as_ref()
            .map(|path| path.display().to_string().into()),
        Some("/tmp".into()),
        concat!(
            " \n",
            "// The folder in which Yinz will look for layouts\n",
            "// (Requires restart)\n",
            "// \n",
        ),
    );
    add_option(
        "theme_dir",
        value
            .theme_dir
            .as_ref()
            .map(|path| path.display().to_string().into()),
        Some("/tmp".into()),
        concat!(
            " \n",
            "// The folder in which Yinz will look for themes\n",
            "// (Requires restart)\n",
            "// \n",
        ),
    );
    add_option(
        "user_plugin_dir",
        value
            .user_plugin_dir
            .as_ref()
            .map(|path| path.display().to_string().into()),
        Some("/tmp".into()),
        concat!(
            " \n",
            "// The folder in which Yinz will look for plugins you wrote yourself\n",
            "// (Requires restart)\n",
            "// \n",
        ),
    );
    add_option(
        "detachable_panes",
        value.detachable_panes.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            " \n",
            "// Run each pane's process under a small supervisor process, so that\n",
            "// panes keep running when the session's server exits (for example if\n",
            "// it is killed or crashes).\n",
            "// Options:\n",
            "//   - true (default)\n",
            "//   - false\n",
            "// \n",
        ),
    );
    add_option(
        "agent_double_escape",
        value.agent_double_escape.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            "// Require two plain Escape presses within one second in panes with an agent record.\n",
            "// The first press is discarded; the second sends one Escape. Disabled by default.\n",
        ),
    );
    add_option(
        "mouse_mode",
        value.mouse_mode.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Toggle enabling the mouse mode.\n",
            "// On certain configurations, or terminals this could\n",
            "// potentially interfere with copying text.\n",
            "// Options:\n",
            "//   - true (default)\n",
            "//   - false\n",
            "// \n",
        ),
    );
    add_option(
        "pane_animations",
        value.pane_animations.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether to animate pane opening effects\n",
            "// default is true\n",
        ),
    );
    add_option(
        "pane_frames",
        value.pane_frames.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Toggle having pane frames around the panes\n",
            "// Options:\n",
            "//   - true (default, enabled)\n",
            "//   - false\n",
            "// \n",
        ),
    );
    add_option(
        "pane_frame_style",
        value.pane_frame_style.map(|style| {
            match style {
                PaneFrameStyle::Full => "full",
                PaneFrameStyle::Titles => "titles",
                PaneFrameStyle::None => "none",
            }
            .into()
        }),
        Some("titles".into()),
        concat!(
            " \n",
            "// Set the pane frame style when pane_frames is enabled\n",
            "// Options:\n",
            "//   - full\n",
            "//   - titles (default)\n",
            "// \n",
        ),
    );
    add_option(
        "notification_position",
        value
            .notification_position
            .as_ref()
            .map(|position| position.as_str().into()),
        Some(NotificationPosition::default().as_str().into()),
        concat!(
            " \n",
            "// The corner notifications appear in - they stack downwards from a top corner\n",
            "// and upwards from a bottom one\n",
            "// Options:\n",
            "//   - top-left\n",
            "//   - top-right (default)\n",
            "//   - bottom-left\n",
            "//   - bottom-right\n",
        ),
    );
    add_option(
        "notification_timeout",
        value
            .notification_timeout
            .map(|timeout| i64::from_ne_bytes(timeout.get().to_ne_bytes()).into()),
        Some(i64::from_ne_bytes(DEFAULT_NOTIFICATION_TIMEOUT_SECS.to_ne_bytes()).into()),
        concat!(
            " \n",
            "// How long a notification stays on screen, in seconds\n",
            "// Default: 4\n",
        ),
    );
    add_option(
        "mirror_session",
        value.mirror_session.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            " \n",
            "// When attaching to an existing session with other users,\n",
            "// should the session be mirrored (true)\n",
            "// or should each user have their own cursor (false)\n",
            "// (Requires restart)\n",
            "// Default: false\n",
            "// \n",
        ),
    );
    add_option(
        "on_force_close",
        value.on_force_close.as_ref().map(|action| {
            match action {
                OnForceClose::Detach => "detach",
                OnForceClose::Quit => "quit",
            }
            .into()
        }),
        Some("quit".into()),
        concat!(
            " \n",
            "// Choose what to do when yinz receives SIGTERM, SIGINT, SIGQUIT or SIGHUP\n",
            "// eg. when terminal window with an active yinz session is closed\n",
            "// (Requires restart)\n",
            "// Options:\n",
            "//   - detach (Default)\n",
            "//   - quit\n",
            "// \n",
        ),
    );
    add_option(
        "max_agent_width",
        value.max_agent_width.map(|width| (width as i64).into()),
        Some(0.into()),
        concat!(
            "\n",
            "// Limit agent panes in all layouts to this many text columns.\n",
            "// Center the grid when it cannot fill the screen. Frames are outside the text width.\n",
            "// Values below 80 disable the limit. Default: 0 (unrestricted).\n",
        ),
    );
    add_option(
        "scroll_buffer_size",
        value.scroll_buffer_size.map(|size| (size as i64).into()),
        Some(10000.into()),
        concat!(
            " \n",
            "// Configure the scroll back buffer size\n",
            "// This is the number of lines yinz stores for each pane in the scroll back\n",
            "// buffer. Excess number of lines are discarded in a FIFO fashion.\n",
            "// (Requires restart)\n",
            "// Valid values: positive integers\n",
            "// Default value: 10000\n",
            "// \n",
        ),
    );
    add_option(
        "locked_escape_timeout_ms",
        value
            .locked_escape_timeout_ms
            .map(|timeout| (timeout.get() as i64).into()),
        Some(250.into()),
        concat!(
            " \n",
            "// How long a locked-mode Ctrl+g waits for a plain g that would quote it into\n",
            "// the pane, in milliseconds. 0 unlocks immediately and never quotes Ctrl+g.\n",
        ),
    );
    add_option(
        "copy_command",
        value.copy_command.as_deref().map(Into::into),
        Some("pbcopy".into()),
        concat!(
            " \n",
            "// Provide a command to execute when copying text. The text will be piped to\n",
            "// the stdin of the program to perform the copy. This can be used with\n",
            "// terminal emulators which do not support the OSC 52 ANSI control sequence\n",
            "// that will be used by default if this option is not set.\n",
            "// Examples:\n",
            "//\n",
            "// copy_command \"xclip -selection clipboard\" // x11\n",
            "// copy_command \"wl-copy\"                    // wayland\n",
            "// copy_command \"pbcopy\"                     // osx\n",
            "// \n",
        ),
    );
    add_option(
        "copy_clipboard",
        value.copy_clipboard.as_ref().map(|clipboard| match clipboard { Clipboard::Primary => "primary", Clipboard::System => "system" }.into()),
        Some("primary".into()),
        concat!(
            " \n",
            "// Choose the destination for copied text\n",
            "// Allows using the primary selection buffer (on x11/wayland) instead of the system clipboard.\n",
            "// Does not apply when using copy_command.\n",
            "// Options:\n",
            "//   - system (default)\n",
            "//   - primary\n",
            "// \n",
        ),
    );
    add_option(
        "copy_on_select",
        value.copy_on_select.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            " \n",
            "// Enable automatic copying (and clearing) of selection when releasing mouse\n",
            "// Default: true\n",
            "// \n",
        ),
    );
    add_option(
        "scrollback_editor",
        value
            .scrollback_editor
            .as_ref()
            .map(|path| path.display().to_string().into()),
        Some("/usr/bin/vim".into()),
        concat!(
            " \n",
            "// Path to the default editor to use to edit pane scrollbuffer\n",
            "// Default: $EDITOR or $VISUAL\n",
        ),
    );
    add_option(
        "session_name",
        value.session_name.as_ref().map(|name| name.as_str().into()),
        Some("My singleton session".into()),
        concat!(
            " \n",
            "// A fixed name to always give the Yinz session.\n",
            "// Consider also setting `attach_to_session true,`\n",
            "// otherwise this will error if such a session exists.\n",
            "// Default: <RANDOM>\n",
            "// \n",
        ),
    );
    add_option(
        "attach_to_session",
        value.attach_to_session.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            " \n",
            "// When `session_name` is provided, attaches to that session\n",
            "// if it is already running or creates it otherwise.\n",
            "// Default: false\n",
            "// \n",
        ),
    );
    add_option(
        "auto_layout",
        value.auto_layout.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Toggle between having Yinz lay out panes according to a predefined set of layouts whenever possible\n",
            "// Options:\n",
            "//   - true (default)\n",
            "//   - false\n",
            "// \n",
        ),
    );
    add_option(
        "session_serialization",
        value.session_serialization.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether sessions should be serialized to the cache folder (including their tabs/panes, cwds and running commands) so that they can later be resurrected\n",
            "// Options:\n",
            "//   - true (default)\n",
            "//   - false\n",
            "// \n",
        ),
    );
    add_option(
        "host_id",
        value.host_id.as_ref().map(|id| id.as_str().into()),
        None,
        "",
    );
    add_option(
        "serialize_pane_viewport",
        value.serialize_pane_viewport.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether pane viewports are serialized along with the session, default is false\n",
            "// Options:\n",
            "//   - true\n",
            "//   - false (default)\n",
            "// \n",
        ),
    );
    add_option(
        "scrollback_lines_to_serialize",
        value.scrollback_lines_to_serialize.map(|size| (size as i64).into()),
        Some(10000.into()),
        concat!(
            " \n",
            "// Scrollback lines to serialize along with the pane viewport when serializing sessions, 0\n",
            "// defaults to the scrollback size. If this number is higher than the scrollback size, it will\n",
            "// also default to the scrollback size. This does nothing if `serialize_pane_viewport` is not true.\n",
            "// \n",
        ),
    );
    add_option(
        "background_opacity",
        value.background_opacity.map(|opacity| format!("{opacity}%").into()),
        None,
        concat!(
            "\n",
            "// Background opacity for Agent, the command palette and translucent overlays (default: 85%).\n",
            "// Text and controls remain opaque. 0 is clear; 100 is solid.\n",
        ),
    );
    add_option(
        "styled_underlines",
        value.styled_underlines.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Enable or disable the rendering of styled and colored underlines (undercurl).\n",
            "// May need to be disabled for certain unsupported terminals\n",
            "// (Requires restart)\n",
            "// Default: true\n",
            "// \n",
        ),
    );
    add_option(
        "serialization_interval",
        value
            .serialization_interval
            .map(|interval| i64::from_ne_bytes(interval.get().to_ne_bytes()).into()),
        Some(10000.into()),
        concat!(
            " \n",
            "// How often in seconds sessions are serialized\n",
            "// \n",
        ),
    );
    add_option(
        "disable_session_metadata",
        value.disable_session_metadata.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Enable or disable writing of session metadata to disk (if disabled, other sessions might not know\n",
            "// metadata info on this session)\n",
            "// (Requires restart)\n",
            "// Default: false\n",
            "// \n",
        ),
    );
    add_option(
        "support_kitty_keyboard_protocol",
        value.support_kitty_keyboard_protocol.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Enable or disable support for the enhanced Kitty Keyboard Protocol (the host terminal must also support it)\n",
            "// (Requires restart)\n",
            "// Default: true (if the host terminal supports it)\n",
            "// \n",
        ),
    );
    add_option(
        "report_modifier_keys",
        value.report_modifier_keys.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Ask the host terminal to report the modifier keys themselves, so the UI can show\n",
            "// something for as long as eg. Alt is held. Needs Kitty Keyboard flags that also make\n",
            "// every printable key arrive as an escape code: AltGr glyphs, dead keys and IME input\n",
            "// will stop working. (Requires restart) Default: false\n",
            "// \n",
        ),
    );
    add_option(
        "support_kitty_graphics_protocol",
        value.support_kitty_graphics_protocol.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Enable or disable support for the Kitty Graphics Protocol, used to display images (the host terminal must also support it)\n",
            "// (Requires restart)\n",
            "// Default: true (if the host terminal supports it)\n",
            "// \n",
        ),
    );
    add_option(
        "stacked_resize",
        value.stacked_resize.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether to stack panes when resizing beyond a certain size\n",
            "// Default: true\n",
            "// \n",
        ),
    );
    add_option(
        "agent_pane_min_columns",
        value
            .agent_pane_min_columns
            .map(|columns| (columns as i64).into()),
        Some((DEFAULT_AGENT_PANE_MIN_COLUMNS as i64).into()),
        if value.agent_pane_min_columns.is_none() {
            concat!(
            " \n",
            "// The smallest tiled pane worth giving a newly spawned coding agent, as a terminal\n",
            "// size. Before an agent pane joins a tab, yinz projects the area each tiled pane\n",
            "// would have if the tab's tiled panes (floating ones aside) shared the viewport\n",
            "// equally, and compares it with columns x rows. A tab that would fall below it\n",
            "// sends the agent to the roomiest other tab that stays above it, or to a new tab.\n",
            "// Set either to 0 to always open agents where they were asked for.\n",
            "// Default: 80 and 24\n",
            "// \n",
        )
        } else {
            ""
        },
    );
    add_option(
        "agent_pane_min_rows",
        value.agent_pane_min_rows.map(|rows| (rows as i64).into()),
        Some((DEFAULT_AGENT_PANE_MIN_ROWS as i64).into()),
        "",
    );
    add_option(
        "stacked_pane_list",
        value.stacked_pane_list.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether stacked panes display as a list with the expanded pane pinned to the bottom\n",
            "// Default: true\n",
            "// \n",
        ),
    );
    add_option(
        "advanced_mouse_actions",
        value.advanced_mouse_actions.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether to enable mouse hover effects and pane grouping functionality\n",
            "// default is true\n",
        ),
    );
    add_option(
        "mouse_scroll_resize",
        value.mouse_scroll_resize.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether Ctrl+ScrollWheel resizes panes\n",
            "// default is true\n",
        ),
    );
    add_option(
        "mouse_hover_effects",
        value.mouse_hover_effects.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether to enable mouse hover visual effects (frame highlight and help text)\n",
            "// default is true\n",
        ),
    );
    add_option(
        "visual_bell",
        value.visual_bell.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            " \n",
            "// Whether to show visual bell indicators (pane/tab frame flash and [!] suffix)\n",
            "// default is true\n",
        ),
    );
    add_option(
        "focus_follows_mouse",
        value.focus_follows_mouse.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether to focus panes on mouse hover\n",
            "// default is false\n",
        ),
    );
    add_option(
        "mouse_click_through",
        value.mouse_click_through.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether clicking a pane to focus it also sends the click into the pane\n",
            "// default is false\n",
        ),
    );
    add_option(
        "context_menu_on_right_click",
        value.context_menu_on_right_click.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            " \n",
            "// Whether right-clicking a pane opens a context menu. A program that is tracking the\n",
            "// mouse keeps right-click, except agents with context_menu_override_agent_mouse true.\n",
            "// default is true\n",
        ),
    );
    add_option(
        "context_menu_override_agent_mouse",
        value.context_menu_override_agent_mouse.map(KdlValue::Bool),
        Some(true.into()),
        concat!(
            "\n",
            "// Whether recognized agents open the context menu even while tracking the mouse.\n",
            "// Requires context_menu_on_right_click. Set false to let agents handle right-click.\n",
            "// default is true\n",
        ),
    );
    add_option(
        "agent_theme_mapping",
        value
            .agent_theme_mapping
            .map(|mapping| mapping.to_string().into()),
        None,
        "",
    );
    add_option(
        "osc133_command_selection",
        value.osc133_command_selection.map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            " \n",
            "// Whether triple-clicking inside command output marked by the shell (OSC 133) selects\n",
            "// the command and its output instead of the logical line\n",
            "// default is true\n",
        ),
    );
    add_option(
        "word_separators",
        value.word_separators.as_deref().map(Into::into),
        Some(DEFAULT_WORD_SEPARATORS.into()),
        concat!(
            " \n",
            "// Characters that terminate a word when double-clicking to select it\n",
            "// whitespace is always a separator and need not be listed here\n",
            "// default is \"[]{}<>()\"\n",
        ),
    );
    add_option(
        "post_command_discovery_hook",
        value.post_command_discovery_hook.as_deref().map(Into::into),
        Some("echo $RESURRECT_COMMAND | sed <your_regex_here>".into()),
        concat!(
            " \n",
            "// A command to run (will be wrapped with sh -c and provided the RESURRECT_COMMAND env variable) \n",
            "// after Yinz attempts to discover a command inside a pane when resurrecting sessions, the STDOUT\n",
            "// of this command will be used instead of the discovered RESURRECT_COMMAND\n",
            "// can be useful for removing wrappers around commands\n",
            "// Note: be sure to escape backslashes and similar characters properly\n",
        ),
    );
    add_option(
        "dangerously_enable_paste_buffer_read",
        value
            .dangerously_enable_paste_buffer_read
            .map(KdlValue::Bool),
        Some(false.into()),
        concat!(
            "\n",
            "// Whether to let programs running inside panes read the paste buffer\n",
            "// (clipboard) with the OSC 52 escape sequence. When enabled, any program\n",
            "// in any pane - including one running on a remote machine over SSH - can\n",
            "// read the clipboard without the user being asked.\n",
            "// Default: false\n",
        ),
    );
    add_option(
        "nested_session_handling",
        value.nested_session_handling.map(|handling| {
            match handling {
                NestedSessionHandling::Ask => "ask",
                NestedSessionHandling::Fullscreen => "fullscreen",
                NestedSessionHandling::Descend => "descend",
                NestedSessionHandling::Never => "never",
            }
            .into()
        }),
        Some("ask".into()),
        concat!(
            "\n",
            "// How to handle a nested Yinz session detected inside a pane.\n",
            "// Options:\n",
            "//   - \"ask\" (Default — prompt with a modal)\n",
            "//   - \"fullscreen\" (always zoom into the nested session)\n",
            "//   - \"descend\" (always control the nested session on focus)\n",
            "//   - \"never\" (never prompt or descend; do it manually)\n",
            "// \n",
        ),
    );
    add_option(
        "host_notification_protocol",
        value
            .host_notification_protocol
            .map(|protocol| protocol.as_str().into()),
        Some(HostNotificationProtocol::Auto.as_str().into()),
        concat!(
            "\n",
            "// Which escape sequence desktop notifications coming from panes are\n",
            "// forwarded to the host terminal with.\n",
            "// Options:\n",
            "//   - \"auto\" (Default — detect from the host terminal environment)\n",
            "//   - \"osc9\" (the legacy iTerm2 protocol, understood by most terminals)\n",
            "//   - \"osc99\" (kitty's notification protocol)\n",
            "//   - \"bell\" (ring the terminal bell instead)\n",
            "//   - \"off\" (do not forward notifications to the host terminal)\n",
            "// \n",
        ),
    );
    nodes
}
