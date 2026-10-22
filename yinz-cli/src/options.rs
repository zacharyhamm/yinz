use clap::{builder::TypedValueParser, Args};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use yinz_types::data::InputMode;
use yinz_types::input::options::*;
use yinz_types::internal_ids::HostId;
use yinz_types::style::ThemeHue;

#[derive(Clone, Default, Debug, PartialEq, Deserialize, Serialize, Args)]
/// Command-line option parsing, converted into the shared `Options` model.
/// CLI fields use strings and numbers where the shared model uses typed IDs and units.
pub struct CliOptions {
    /// Enable tab groups (default: true).
    #[clap(long, value_parser)]
    #[serde(default)]
    pub tab_groups: Option<bool>,
    /// Allow plugins to use a more simplified layout
    /// that is compatible with more fonts (true or false)
    #[clap(long, value_parser)]
    #[serde(default)]
    pub simplified_ui: Option<bool>,
    /// Skip the startup splash and its minimum display time (default: false).
    #[clap(long, value_parser)]
    #[serde(default)]
    pub disable_splash_screen: Option<bool>,
    /// Set the default theme
    #[clap(long, value_parser)]
    pub theme: Option<String>,
    /// Theme name to apply when the host terminal reports a dark color palette
    /// (CSI 2031 / DSR 997). Requires `theme_light` to also be set; if either
    /// is missing the static `theme` remains authoritative.
    #[clap(long, value_parser)]
    pub theme_dark: Option<String>,
    /// Theme name to apply when the host terminal reports a light color palette
    /// (CSI 2031 / DSR 997). Requires `theme_dark` to also be set; if either
    /// is missing the static `theme` remains authoritative.
    #[clap(long, value_parser)]
    pub theme_light: Option<String>,
    /// Pin the session to a dark or light appearance ("dark" or "light"),
    /// resolved before the first render and kept authoritative over ambient
    /// host terminal reports (CSI 2031 / DSR 997). When unset, the session
    /// follows the host terminal.
    #[clap(long, hide_possible_values = true, value_parser = clap::builder::PossibleValuesParser::new(["light", "dark"]).try_map(|value| value.parse::<ThemeHue>()))]
    pub explicit_theme_hue: Option<ThemeHue>,
    /// Set the default mode
    #[clap(long, hide_possible_values = true, value_parser = input_mode_parser())]
    pub default_mode: Option<InputMode>,
    /// Set the default shell
    #[clap(long, value_parser)]
    pub default_shell: Option<PathBuf>,
    /// Set the default cwd
    #[clap(long, value_parser)]
    pub default_cwd: Option<PathBuf>,
    /// Set the default layout
    #[clap(long, value_parser)]
    pub default_layout: Option<PathBuf>,
    /// Set the layout_dir, defaults to
    /// subdirectory of config dir
    #[clap(long, value_parser)]
    pub layout_dir: Option<PathBuf>,
    /// Set the theme_dir, defaults to
    /// subdirectory of config dir
    #[clap(long, value_parser)]
    pub theme_dir: Option<PathBuf>,
    /// Set the folder holding your own plugins, defaults to the
    /// "plugins" subdirectory of the config dir
    #[clap(long, value_parser)]
    pub user_plugin_dir: Option<PathBuf>,
    #[clap(long, value_parser)]
    #[serde(default)]
    /// Set the handling of mouse events (true or false)
    /// Can be temporarily bypassed by the [SHIFT] key
    pub mouse_mode: Option<bool>,
    /// Require two plain Escape presses within one second in agent panes (default: false).
    #[clap(long, value_parser)]
    #[serde(default)]
    pub agent_double_escape: Option<bool>,
    /// Run each pane's process under a supervisor that outlives the session, so
    /// that panes survive the server exiting (true or false).
    ///
    /// Config-file only, deliberately. It is read once when the session is
    /// created and cannot change for a live session, so a per-invocation CLI flag
    /// would be misleading on `yinz attach`. `YINZ_DISABLE_PTY_HOST=1` /
    /// `YINZ_ENABLE_PTY_HOST=1` are the escape hatches, and unlike config they
    /// work without a running session.
    #[clap(skip)]
    #[serde(default)]
    pub detachable_panes: Option<bool>,
    #[clap(long, value_parser)]
    #[serde(default)]
    /// Set display of the pane frames (true or false)
    pub pane_frames: Option<bool>,
    #[clap(long, hide_possible_values = true, value_parser = pane_frame_style_parser())]
    #[serde(default)]
    pub pane_frame_style: Option<PaneFrameStyle>,
    /// The corner notifications appear in
    /// (top-left, top-right, bottom-left or bottom-right)
    #[clap(long, hide_possible_values = true, value_parser = notification_position_parser())]
    #[serde(default)]
    pub notification_position: Option<NotificationPosition>,
    /// How long a notification stays on screen, in seconds
    #[clap(long, value_parser)]
    #[serde(default)]
    pub notification_timeout: Option<u64>,
    #[clap(long, value_parser)]
    #[serde(default)]
    /// Mirror session when multiple users are connected (true or false)
    pub mirror_session: Option<bool>,
    /// Set behaviour on force close (quit or detach)
    #[clap(long, hide_possible_values = true, value_parser = on_force_close_parser())]
    pub on_force_close: Option<OnForceClose>,
    #[clap(long, value_parser)]
    pub scroll_buffer_size: Option<usize>,
    /// Maximum text columns for any agent pane (values below 80 disable the limit).
    #[clap(long, value_parser)]
    #[serde(default)]
    pub max_agent_width: Option<usize>,

    /// Switch to using a user supplied command for clipboard instead of OSC52
    #[clap(long, value_parser)]
    #[serde(default)]
    pub copy_command: Option<String>,

    /// OSC52 destination clipboard
    #[clap(
        long,
        ignore_case = true,
        conflicts_with = "copy_command",
        value_parser = clipboard_parser()
    )]
    #[serde(default)]
    pub copy_clipboard: Option<Clipboard>,

    /// Automatically copy when selecting text (true or false)
    #[clap(long, value_parser)]
    #[serde(default)]
    pub copy_on_select: Option<bool>,

    /// Enable OSC8 hyperlink output (true or false)
    #[clap(long, value_parser)]
    #[serde(default)]
    pub osc8_hyperlinks: Option<bool>,

    /// Explicit full path to open the scrollback editor (default is $EDITOR or $VISUAL)
    #[clap(long, value_parser)]
    pub scrollback_editor: Option<PathBuf>,

    /// The name of the session to create when starting Yinz
    #[clap(long, value_parser)]
    #[serde(default)]
    pub session_name: Option<String>,

    /// Whether to attach to a session specified in "session-name" if it exists
    #[clap(long, value_parser)]
    #[serde(default)]
    pub attach_to_session: Option<bool>,

    /// Whether to lay out panes in a predefined set of layouts whenever possible
    #[clap(long, value_parser)]
    #[serde(default)]
    pub auto_layout: Option<bool>,

    /// Whether sessions should be serialized to the HD so that they can be later resurrected,
    /// default is true
    #[clap(long, value_parser)]
    #[serde(default)]
    pub session_serialization: Option<bool>,

    /// Pins this installation's host id, which is otherwise generated once and kept in
    /// `<data dir>/host-id`. It says which machine wrote a row in the plugin data store, so
    /// rows retain their installation identity across sessions.
    ///
    /// Only worth setting when the generated one would move: a data dir that gets wiped, a
    /// container built fresh each time, or a test that must not depend on the machine running
    /// it. `YINZ_HOST_ID` overrides this in turn. Alphanumerics, `_` and `-`, up to 64
    /// characters.
    ///
    /// Config-file only: changing it mid-session would orphan every row
    /// the session had already written.
    #[clap(skip)]
    #[serde(default)]
    pub host_id: Option<String>,

    /// Whether pane viewports are serialized along with the session, default is false
    #[clap(long, value_parser)]
    #[serde(default)]
    pub serialize_pane_viewport: Option<bool>,

    /// Scrollback lines to serialize along with the pane viewport when serializing sessions, 0
    /// defaults to the scrollback size. If this number is higher than the scrollback size, it will
    /// also default to the scrollback size
    #[clap(long, value_parser)]
    #[serde(default)]
    pub scrollback_lines_to_serialize: Option<usize>,

    /// Whether to use ANSI styled underlines
    #[clap(long, value_parser)]
    #[serde(default)]
    pub styled_underlines: Option<bool>,

    /// The interval at which to serialize sessions for resurrection (in seconds)
    #[clap(long, value_parser)]
    pub serialization_interval: Option<u64>,

    /// If true, will disable writing session metadata to disk
    #[clap(long, value_parser)]
    pub disable_session_metadata: Option<bool>,

    /// Whether to enable support for the Kitty keyboard protocol (must also be supported by the
    /// host terminal), defaults to true if the terminal supports it
    #[clap(long, value_parser)]
    #[serde(default)]
    pub support_kitty_keyboard_protocol: Option<bool>,

    /// Whether to ask the host terminal to report the modifier keys themselves, so the UI can
    /// know that eg. Alt is being held rather than only that it was part of a chord. Defaults
    /// to false: it needs Kitty keyboard flags that also make every printable key arrive as an
    /// escape code, which US-ASCII survives but AltGr glyphs, dead keys and IME commits do not.
    #[clap(long, value_parser)]
    #[serde(default)]
    pub report_modifier_keys: Option<bool>,

    /// Whether to enable support for the Kitty graphics (image) protocol (must also be supported
    /// by the host terminal), defaults to true if the terminal supports it
    #[clap(long, value_parser)]
    #[serde(default)]
    pub support_kitty_graphics_protocol: Option<bool>,

    /// Whether to stack panes when resizing beyond a certain size
    /// default is true
    #[clap(long, value_parser)]
    #[serde(default)]
    pub stacked_resize: Option<bool>,

    /// The smallest tiled pane worth giving a newly spawned coding agent, in columns.
    /// With `agent_pane_min_rows` it names an area: before an agent pane joins a tab, the
    /// tab's tiled panes and the newcomer are imagined sharing the viewport equally, and a
    /// tab whose share would fall below columns x rows sends the agent to the roomiest other
    /// tab that clears it, or to a new tab. 0 in either disables the rule.
    /// default is 80
    #[clap(long, value_parser)]
    #[serde(default)]
    pub agent_pane_min_columns: Option<usize>,

    /// The rows half of `agent_pane_min_columns`
    /// default is 24
    #[clap(long, value_parser)]
    #[serde(default)]
    pub agent_pane_min_rows: Option<usize>,

    #[clap(long, value_parser)]
    #[serde(default)]
    pub stacked_pane_list: Option<bool>,

    /// Whether to enable mouse hover effects and pane grouping functionality
    /// default is true
    #[clap(long, value_parser)]
    #[serde(default)]
    pub advanced_mouse_actions: Option<bool>,

    /// Whether Ctrl+ScrollWheel resizes panes
    /// default is true
    #[clap(long, value_parser)]
    #[serde(default)]
    pub mouse_scroll_resize: Option<bool>,

    /// Whether to enable mouse hover visual effects (frame highlight and help text)
    /// default is true
    #[clap(long, value_parser)]
    #[serde(default)]
    pub mouse_hover_effects: Option<bool>,

    /// Whether to show visual bell indicators (pane/tab frame flash and [!] suffix)
    /// default is true
    #[clap(long, value_parser)]
    #[serde(default)]
    pub visual_bell: Option<bool>,

    /// Whether to focus panes on mouse hover (true or false)
    /// default is false
    #[clap(long, value_parser)]
    #[serde(default)]
    pub focus_follows_mouse: Option<bool>,

    /// Whether clicking a pane to focus it also sends the click into the pane (true or false)
    /// default is false
    #[clap(long, value_parser)]
    #[serde(default)]
    pub mouse_click_through: Option<bool>,

    /// Whether right-clicking a pane opens a context menu; default is true.
    ///
    /// Mouse-tracking programs keep right-click, except recognized agents when
    /// context_menu_override_agent_mouse is enabled.
    #[clap(long, value_parser)]
    #[serde(default)]
    pub context_menu_on_right_click: Option<bool>,

    /// Whether recognized agents use the context menu even while tracking the mouse.
    /// Requires context_menu_on_right_click; default is true.
    #[clap(long, value_parser)]
    #[serde(default)]
    pub context_menu_override_agent_mouse: Option<bool>,

    /// Whether triple-clicking inside shell-marked (OSC 133) command output selects the command
    /// and its output rather than the logical line
    /// default is true
    #[clap(long, value_parser)]
    #[serde(default)]
    pub osc133_command_selection: Option<bool>,

    /// Map managed agent palettes to the Yinz theme (semantic or off).
    #[clap(long)]
    #[serde(default)]
    pub agent_theme_mapping: Option<yinz_types::agent::theme::AgentThemeMapping>,

    /// Characters that terminate a word when double-clicking to select it, in addition to
    /// whitespace (which is always a separator)
    /// default is "[]{}<>()"
    #[clap(long, value_parser)]
    #[serde(default)]
    pub word_separators: Option<String>,

    /// Which escape sequence desktop notifications from panes are forwarded to the host
    /// terminal with (auto, osc9, osc99, bell, off)
    #[clap(long, hide_possible_values = true, value_parser = host_notification_protocol_parser())]
    #[serde(default)]
    pub host_notification_protocol: Option<HostNotificationProtocol>,

    /// A command to run after the discovery of running commands when serializing, for the purpose
    /// of manipulating the command (eg. with a regex) before it gets serialized
    #[clap(long, value_parser)]
    pub post_command_discovery_hook: Option<String>,

    /// How to handle a nested Yinz session detected inside a pane
    /// (ask, fullscreen, descend, never)
    #[clap(long, hide_possible_values = true, value_parser = nested_session_handling_parser())]
    #[serde(default)]
    pub nested_session_handling: Option<NestedSessionHandling>,

    #[clap(long, value_parser)]
    #[serde(default)]
    pub dangerously_enable_paste_buffer_read: Option<bool>,

    /// How long, in milliseconds, a locked-mode Ctrl+g is held as a prefix: type a plain `g`
    /// within the window to send a literal Ctrl+g to the pane and stay locked, otherwise the
    /// window expires and the mode switches to Normal. 0 disables the prefix entirely, so
    /// Ctrl+g unlocks immediately.
    /// default is 250
    #[clap(long, value_parser)]
    #[serde(default)]
    pub locked_escape_timeout_ms: Option<u64>,
}
impl From<CliOptions> for Options {
    fn from(value: CliOptions) -> Self {
        Self {
            tab_groups: value.tab_groups,
            simplified_ui: value.simplified_ui,
            disable_splash_screen: value.disable_splash_screen,
            theme: value.theme,
            theme_dark: value.theme_dark,
            theme_light: value.theme_light,
            explicit_theme_hue: value.explicit_theme_hue,
            default_mode: value.default_mode,
            default_shell: value.default_shell,
            default_cwd: value.default_cwd,
            default_layout: value.default_layout,
            layout_dir: value.layout_dir,
            theme_dir: value.theme_dir,
            user_plugin_dir: value.user_plugin_dir,
            mouse_mode: value.mouse_mode,
            agent_double_escape: value.agent_double_escape,
            detachable_panes: value.detachable_panes,
            pane_frames: value.pane_frames,
            pane_animations: None,
            background_opacity: None,
            pane_frame_style: value.pane_frame_style,
            notification_position: value.notification_position,
            notification_timeout: value
                .notification_timeout
                .map(yinz_types::units::ElapsedSeconds::new),
            mirror_session: value.mirror_session,
            on_force_close: value.on_force_close,
            scroll_buffer_size: value.scroll_buffer_size,
            max_agent_width: value.max_agent_width,
            copy_command: value.copy_command,
            copy_clipboard: value.copy_clipboard,
            copy_on_select: value.copy_on_select,
            osc8_hyperlinks: value.osc8_hyperlinks,
            scrollback_editor: value.scrollback_editor,
            session_name: value.session_name.map(Into::into),
            attach_to_session: value.attach_to_session,
            auto_layout: value.auto_layout,
            session_serialization: value.session_serialization,
            host_id: value.host_id.map(HostId::new),
            serialize_pane_viewport: value.serialize_pane_viewport,
            scrollback_lines_to_serialize: value.scrollback_lines_to_serialize,
            styled_underlines: value.styled_underlines,
            serialization_interval: value
                .serialization_interval
                .map(yinz_types::units::ElapsedSeconds::new),
            disable_session_metadata: value.disable_session_metadata,
            support_kitty_keyboard_protocol: value.support_kitty_keyboard_protocol,
            report_modifier_keys: value.report_modifier_keys,
            support_kitty_graphics_protocol: value.support_kitty_graphics_protocol,
            stacked_resize: value.stacked_resize,
            agent_pane_min_columns: value.agent_pane_min_columns,
            agent_pane_min_rows: value.agent_pane_min_rows,
            stacked_pane_list: value.stacked_pane_list,
            advanced_mouse_actions: value.advanced_mouse_actions,
            mouse_scroll_resize: value.mouse_scroll_resize,
            mouse_hover_effects: value.mouse_hover_effects,
            visual_bell: value.visual_bell,
            focus_follows_mouse: value.focus_follows_mouse,
            mouse_click_through: value.mouse_click_through,
            context_menu_on_right_click: value.context_menu_on_right_click,
            context_menu_override_agent_mouse: value.context_menu_override_agent_mouse,
            osc133_command_selection: value.osc133_command_selection,
            agent_theme_mapping: value.agent_theme_mapping,
            word_separators: value.word_separators,
            host_notification_protocol: value.host_notification_protocol,
            post_command_discovery_hook: value.post_command_discovery_hook,
            nested_session_handling: value.nested_session_handling,
            dangerously_enable_paste_buffer_read: value.dangerously_enable_paste_buffer_read,
            locked_escape_timeout_ms: value
                .locked_escape_timeout_ms
                .map(yinz_types::units::ElapsedMillis::new),
        }
    }
}
impl From<Options> for CliOptions {
    fn from(value: Options) -> Self {
        Self {
            tab_groups: value.tab_groups,
            simplified_ui: value.simplified_ui,
            disable_splash_screen: value.disable_splash_screen,
            theme: value.theme,
            theme_dark: value.theme_dark,
            theme_light: value.theme_light,
            explicit_theme_hue: value.explicit_theme_hue,
            default_mode: value.default_mode,
            default_shell: value.default_shell,
            default_cwd: value.default_cwd,
            default_layout: value.default_layout,
            layout_dir: value.layout_dir,
            theme_dir: value.theme_dir,
            user_plugin_dir: value.user_plugin_dir,
            mouse_mode: value.mouse_mode,
            agent_double_escape: value.agent_double_escape,
            detachable_panes: value.detachable_panes,
            pane_frames: value.pane_frames,
            pane_frame_style: value.pane_frame_style,
            notification_position: value.notification_position,
            notification_timeout: value
                .notification_timeout
                .map(yinz_types::units::ElapsedSeconds::get),
            mirror_session: value.mirror_session,
            on_force_close: value.on_force_close,
            scroll_buffer_size: value.scroll_buffer_size,
            max_agent_width: value.max_agent_width,
            copy_command: value.copy_command,
            copy_clipboard: value.copy_clipboard,
            copy_on_select: value.copy_on_select,
            osc8_hyperlinks: value.osc8_hyperlinks,
            scrollback_editor: value.scrollback_editor,
            session_name: value.session_name.map(|name| name.into_string()),
            attach_to_session: value.attach_to_session,
            auto_layout: value.auto_layout,
            session_serialization: value.session_serialization,
            host_id: value.host_id.map(|id| id.into_string()),
            serialize_pane_viewport: value.serialize_pane_viewport,
            scrollback_lines_to_serialize: value.scrollback_lines_to_serialize,
            styled_underlines: value.styled_underlines,
            serialization_interval: value
                .serialization_interval
                .map(yinz_types::units::ElapsedSeconds::get),
            disable_session_metadata: value.disable_session_metadata,
            support_kitty_keyboard_protocol: value.support_kitty_keyboard_protocol,
            report_modifier_keys: value.report_modifier_keys,
            support_kitty_graphics_protocol: value.support_kitty_graphics_protocol,
            stacked_resize: value.stacked_resize,
            agent_pane_min_columns: value.agent_pane_min_columns,
            agent_pane_min_rows: value.agent_pane_min_rows,
            stacked_pane_list: value.stacked_pane_list,
            advanced_mouse_actions: value.advanced_mouse_actions,
            mouse_scroll_resize: value.mouse_scroll_resize,
            mouse_hover_effects: value.mouse_hover_effects,
            visual_bell: value.visual_bell,
            focus_follows_mouse: value.focus_follows_mouse,
            mouse_click_through: value.mouse_click_through,
            context_menu_on_right_click: value.context_menu_on_right_click,
            context_menu_override_agent_mouse: value.context_menu_override_agent_mouse,
            osc133_command_selection: value.osc133_command_selection,
            agent_theme_mapping: value.agent_theme_mapping,
            word_separators: value.word_separators,
            host_notification_protocol: value.host_notification_protocol,
            post_command_discovery_hook: value.post_command_discovery_hook,
            nested_session_handling: value.nested_session_handling,
            dangerously_enable_paste_buffer_read: value.dangerously_enable_paste_buffer_read,
            locked_escape_timeout_ms: value
                .locked_escape_timeout_ms
                .map(yinz_types::units::ElapsedMillis::get),
        }
    }
}
pub fn merge_from_cli(base: &Options, other: Options) -> Options {
    base.merge(other)
}
pub fn input_mode_parser() -> impl TypedValueParser<Value = InputMode> {
    clap::builder::PossibleValuesParser::new([
        "normal",
        "locked",
        "resize",
        "pane",
        "tab",
        "scroll",
        "enter-search",
        "search",
        "rename-tab",
        "rename-pane",
        "session",
        "move",
        "prompt",
        "tmux",
        "agent",
        "select",
        "notifications",
    ])
    .map(|value| match value.to_ascii_lowercase().as_str() {
        "normal" => InputMode::Normal,
        "locked" => InputMode::Locked,
        "resize" => InputMode::Resize,
        "pane" => InputMode::Pane,
        "tab" => InputMode::Tab,
        "scroll" => InputMode::Scroll,
        "enter-search" => InputMode::EnterSearch,
        "search" => InputMode::Search,
        "rename-tab" => InputMode::RenameTab,
        "rename-pane" => InputMode::RenamePane,
        "session" => InputMode::Session,
        "move" => InputMode::Move,
        "prompt" => InputMode::Prompt,
        "tmux" => InputMode::Tmux,
        "agent" => InputMode::Agent,
        "select" => InputMode::Select,
        "notifications" => InputMode::Notifications,
        _ => unreachable!("possible values parser validated the value"),
    })
}
pub fn on_force_close_parser() -> impl TypedValueParser<Value = OnForceClose> {
    clap::builder::PossibleValuesParser::new(["quit", "detach"]).map(|value| {
        match value.to_ascii_lowercase().as_str() {
            "quit" => OnForceClose::Quit,
            "detach" => OnForceClose::Detach,
            _ => unreachable!("possible values parser validated the value"),
        }
    })
}
pub fn nested_session_handling_parser() -> impl TypedValueParser<Value = NestedSessionHandling> {
    clap::builder::PossibleValuesParser::new(["ask", "fullscreen", "descend", "never"]).map(
        |value| match value.to_ascii_lowercase().as_str() {
            "ask" => NestedSessionHandling::Ask,
            "fullscreen" => NestedSessionHandling::Fullscreen,
            "descend" => NestedSessionHandling::Descend,
            "never" => NestedSessionHandling::Never,
            _ => unreachable!("possible values parser validated the value"),
        },
    )
}
pub fn host_notification_protocol_parser() -> impl TypedValueParser<Value = HostNotificationProtocol>
{
    clap::builder::PossibleValuesParser::new(["auto", "osc9", "osc99", "bell", "off"]).map(
        |value| match value.to_ascii_lowercase().as_str() {
            "auto" => HostNotificationProtocol::Auto,
            "osc9" => HostNotificationProtocol::Osc9,
            "osc99" => HostNotificationProtocol::Osc99,
            "bell" => HostNotificationProtocol::Bell,
            "off" => HostNotificationProtocol::Off,
            _ => unreachable!("possible values parser validated the value"),
        },
    )
}
pub fn pane_frame_style_parser() -> impl TypedValueParser<Value = PaneFrameStyle> {
    clap::builder::PossibleValuesParser::new(["full", "titles", "none"]).map(|value| {
        match value.to_ascii_lowercase().as_str() {
            "full" => PaneFrameStyle::Full,
            "titles" => PaneFrameStyle::Titles,
            "none" => PaneFrameStyle::None,
            _ => unreachable!("possible values parser validated the value"),
        }
    })
}
pub fn notification_position_parser() -> impl TypedValueParser<Value = NotificationPosition> {
    clap::builder::PossibleValuesParser::new([
        "top-left",
        "top-right",
        "bottom-left",
        "bottom-right",
    ])
    .map(|value| match value.to_ascii_lowercase().as_str() {
        "top-left" => NotificationPosition::TopLeft,
        "top-right" => NotificationPosition::TopRight,
        "bottom-left" => NotificationPosition::BottomLeft,
        "bottom-right" => NotificationPosition::BottomRight,
        _ => unreachable!("possible values parser validated the value"),
    })
}
pub fn clipboard_parser() -> impl TypedValueParser<Value = Clipboard> {
    clap::builder::PossibleValuesParser::new(["system", "primary"]).map(|value| {
        match value.to_ascii_lowercase().as_str() {
            "system" => Clipboard::System,
            "primary" => Clipboard::Primary,
            _ => unreachable!("possible values parser validated the value"),
        }
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_agent_width_cli_round_trip_and_precedence() {
        let config = Options {
            max_agent_width: Some(100),
            ..Options::default()
        };
        for value in [None, Some(0), Some(1), Some(79), Some(80), Some(81)] {
            let options = Options {
                max_agent_width: value,
                ..Options::default()
            };
            assert_eq!(Options::from(CliOptions::from(options.clone())), options);
            assert_eq!(
                merge_from_cli(&config, options).max_agent_width,
                value.or(Some(100))
            );
        }
    }

    #[test]
    fn agent_mouse_override_cli_round_trip_and_precedence() {
        let config = Options {
            context_menu_override_agent_mouse: Some(true),
            ..Default::default()
        };
        for value in [None, Some(false), Some(true)] {
            let options = Options {
                context_menu_override_agent_mouse: value,
                ..Default::default()
            };
            assert_eq!(Options::from(CliOptions::from(options.clone())), options);
            assert_eq!(
                merge_from_cli(&config, options).context_menu_override_agent_mouse,
                value.or(Some(true))
            );
        }
    }

    #[test]
    fn agent_double_escape_cli_round_trip() {
        for value in [None, Some(false), Some(true)] {
            let options = Options {
                agent_double_escape: value,
                ..Options::default()
            };
            assert_eq!(Options::from(CliOptions::from(options.clone())), options);
            assert_eq!(
                merge_from_cli(&Options::default(), options).agent_double_escape,
                value
            );
        }
    }
    #[test]
    fn a_host_notification_protocol_unset_everywhere_stays_unset() {
        assert_eq!(
            Options::default()
                .merge(Options::default())
                .host_notification_protocol,
            None
        );
        assert_eq!(
            merge_from_cli(&Options::default(), Options::default()).host_notification_protocol,
            None
        );
    }
    #[test]
    fn a_host_notification_protocol_given_on_the_command_line_wins() {
        let config = Options {
            host_notification_protocol: Some(HostNotificationProtocol::Osc9),
            ..Default::default()
        };
        let cli = Options {
            host_notification_protocol: Some(HostNotificationProtocol::Off),
            ..Default::default()
        };
        assert_eq!(
            merge_from_cli(&config, cli).host_notification_protocol,
            Some(HostNotificationProtocol::Off)
        );
    }
    #[test]
    fn a_host_notification_protocol_absent_from_the_command_line_keeps_the_configured_one() {
        let config = Options {
            host_notification_protocol: Some(HostNotificationProtocol::Osc99),
            ..Default::default()
        };
        assert_eq!(
            merge_from_cli(&config, Options::default()).host_notification_protocol,
            Some(HostNotificationProtocol::Osc99),
        );
    }
}

#[test]
fn tab_groups_cli_value_overrides_instead_of_toggling_config() {
    let base = Options {
        tab_groups: Some(true),
        ..Default::default()
    };
    for enabled in [false, true] {
        let override_options = Options {
            tab_groups: Some(enabled),
            ..Default::default()
        };
        assert_eq!(
            merge_from_cli(&base, override_options).tab_groups,
            Some(enabled)
        );
    }
    assert_eq!(
        merge_from_cli(&base, Options::default()).tab_groups,
        Some(true)
    );
}
