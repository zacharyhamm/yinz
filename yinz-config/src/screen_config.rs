//! What the screen thread takes from the configuration, as one value.

use std::path::PathBuf;

use yinz_types::data::InputMode;
use yinz_types::input::context_menu::ContextMenu;
use yinz_types::input::options::{
    Clipboard, HostNotificationProtocol, NestedSessionHandling, NotificationPosition, Options,
    PaneFrameStyle, DEFAULT_AGENT_PANE_MIN_COLUMNS, DEFAULT_AGENT_PANE_MIN_ROWS,
    DEFAULT_NOTIFICATION_TIMEOUT_SECS, DEFAULT_WORD_SEPARATORS,
};
use yinz_types::pane_size::Size;
use yinz_types::style::{default_palette, Styling, ThemeHue};
use yinz_types::units::ElapsedSeconds;

use crate::config::Config;

/// Everything `Screen` takes from the configuration, resolved once by
/// [`ScreenConfig::from_config`].
///
/// Handed whole to `Screen::new` and carried whole by `ScreenInstruction::Reconfigure`, and
/// `Screen::reconfigure` destructures it without `..`: an option added here cannot be read at
/// startup and forgotten on a reload, which is how a dozen of them came to take effect only
/// after a restart. The defaults for options the config leaves unset live here and nowhere
/// else.
///
/// A few fields stay `Option` on purpose, each for a reason at its declaration; the rest are
/// resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenConfig {
    pub agent_theme_mapping: yinz_types::agent::theme::AgentThemeMapping,
    // theme and chrome
    /// Applied on a reload only. At startup `Screen` paints with the first client's
    /// `ClientAttributes.style`, which that client resolved against its command line as well as
    /// the config file, and the same goes for `rounded_corners` and `hide_session_name`.
    pub theme: Styling,
    pub terminal_theme: Option<yinz_types::style::TerminalTheme>,
    pub terminal_themes: std::collections::BTreeMap<String, yinz_types::style::TerminalTheme>,
    pub themes: std::collections::BTreeMap<String, Styling>,
    pub theme_name: Option<String>,
    pub theme_dark_name: Option<String>,
    pub theme_light_name: Option<String>,
    pub host_theme_dark: Option<Styling>,
    pub host_theme_light: Option<Styling>,
    pub explicit_theme_hue: Option<ThemeHue>,
    /// See `theme`.
    pub rounded_corners: bool,
    /// See `theme`.
    pub hide_session_name: bool,
    pub pane_frame_style: PaneFrameStyle,
    pub max_fullscreen_agent_width: usize,
    pub simplified_ui: bool,
    pub pane_animations: bool,
    pub background_opacity: u8,
    pub styled_underlines: bool,
    pub osc8_hyperlinks: bool,
    // modes, shells and paths
    /// `None` is meaningful: a `ModeInfo` built without one keeps `base_mode` unset.
    pub default_mode: Option<InputMode>,
    /// `None` is the server's own default shell, found from the environment on the server side.
    pub default_shell: Option<PathBuf>,
    pub default_cwd: Option<PathBuf>,
    /// `None` is `$EDITOR`, read on the server side.
    pub default_editor: Option<PathBuf>,
    pub default_layout_name: Option<String>,
    pub layout_dir: Option<PathBuf>,
    // layout behaviour
    pub auto_layout: bool,
    pub stacked_resize: bool,
    pub stacked_pane_list: bool,
    /// The floor a spawned agent pane's equal share of a tab must clear, columns by rows; zero
    /// in either turns the rule off.
    pub agent_pane_min_size: Size,
    pub agent_double_escape: bool,
    // mouse, selection and the context menu
    pub advanced_mouse_actions: bool,
    pub mouse_scroll_resize: bool,
    pub mouse_hover_effects: bool,
    pub focus_follows_mouse: bool,
    pub mouse_click_through: bool,
    pub context_menu_on_right_click: bool,
    /// The items a right-click offers. Held by `Screen` rather than re-read per click, so a
    /// reload has to hand the new list over or the session keeps the one it started with.
    pub context_menu: ContextMenu,
    pub osc133_command_selection: bool,
    pub word_separators: String,
    // the clipboard
    pub copy_command: Option<String>,
    /// `None` on a reload keeps the clipboard the session already uses; at startup it is the
    /// default clipboard.
    pub copy_clipboard: Option<Clipboard>,
    pub copy_on_select: bool,
    // host protocols and safety
    pub support_kitty_keyboard_protocol: bool,
    pub support_kitty_graphics_protocol: bool,
    pub host_notification_protocol: HostNotificationProtocol,
    pub nested_session_handling: NestedSessionHandling,
    pub dangerously_enable_paste_buffer_read: bool,
    pub visual_bell: bool,
    // the session
    pub session_is_mirrored: bool,
    pub session_serialization: bool,
    pub serialize_pane_viewport: bool,
    pub scrollback_lines_to_serialize: Option<usize>,
    // notifications
    pub notification_position: NotificationPosition,
    pub notification_timeout_secs: ElapsedSeconds,
}

impl ScreenConfig {
    /// Resolve `config` for the screen. The one place an unset option gets its default.
    pub fn from_config(config: &Config) -> Self {
        let options: &Options = &config.options;
        let host_theme = |name: &Option<String>, which: &str| -> Option<Styling> {
            let styling = name
                .as_ref()
                .and_then(|name| config.theme_config(Some(name)));
            if name.is_some() && styling.is_none() {
                log::warn!(
                    "{which}='{}' not found in themes; auto-theme switch disabled for {which}.",
                    name.as_deref().unwrap_or("?")
                );
            }
            styling
        };
        Self {
            terminal_theme: config.terminal_theme_config(options.theme.as_ref()),
            terminal_themes: config
                .themes
                .inner()
                .iter()
                .filter_map(|(name, theme)| theme.terminal.map(|terminal| (name.clone(), terminal)))
                .collect(),
            themes: config
                .themes
                .inner()
                .iter()
                .map(|(name, theme)| (name.clone(), theme.palette))
                .collect(),
            agent_theme_mapping: options.agent_theme_mapping.unwrap_or_default(),
            theme_name: options.theme.clone(),
            theme_dark_name: options.theme_dark.clone(),
            theme_light_name: options.theme_light.clone(),
            theme: config
                .theme_config(options.theme.as_ref())
                .unwrap_or_else(|| default_palette().into()),
            host_theme_dark: host_theme(&options.theme_dark, "theme_dark"),
            host_theme_light: host_theme(&options.theme_light, "theme_light"),
            explicit_theme_hue: options.explicit_theme_hue,
            rounded_corners: config.ui.pane_frames.rounded_corners,
            hide_session_name: config.ui.pane_frames.hide_session_name,
            pane_frame_style: PaneFrameStyle::from_options(options),
            max_fullscreen_agent_width: options.max_fullscreen_agent_width.unwrap_or(0),
            simplified_ui: options.simplified_ui.unwrap_or(false),
            pane_animations: options.pane_animations.unwrap_or(true),
            background_opacity: options
                .background_opacity
                .unwrap_or(yinz_types::input::options::DEFAULT_BACKGROUND_OPACITY),
            styled_underlines: options.styled_underlines.unwrap_or(true),
            osc8_hyperlinks: options.osc8_hyperlinks.unwrap_or(true),
            default_mode: options.default_mode,
            default_shell: options.default_shell.clone(),
            default_cwd: options.default_cwd.clone(),
            default_editor: options.scrollback_editor.clone(),
            default_layout_name: options
                .default_layout
                .as_ref()
                .map(|layout| layout.display().to_string()),
            layout_dir: options.layout_dir.clone(),
            auto_layout: options.auto_layout.unwrap_or(true),
            stacked_resize: options.stacked_resize.unwrap_or(true),
            stacked_pane_list: options.stacked_pane_list.unwrap_or(true),
            agent_pane_min_size: Size {
                cols: options
                    .agent_pane_min_columns
                    .unwrap_or(DEFAULT_AGENT_PANE_MIN_COLUMNS),
                rows: options
                    .agent_pane_min_rows
                    .unwrap_or(DEFAULT_AGENT_PANE_MIN_ROWS),
            },
            agent_double_escape: options.agent_double_escape.unwrap_or(false),
            advanced_mouse_actions: options.advanced_mouse_actions.unwrap_or(true),
            mouse_scroll_resize: options.mouse_scroll_resize.unwrap_or(true),
            mouse_hover_effects: options.mouse_hover_effects.unwrap_or(true),
            focus_follows_mouse: options.focus_follows_mouse.unwrap_or(false),
            mouse_click_through: options.mouse_click_through.unwrap_or(false),
            context_menu_on_right_click: options.context_menu_on_right_click.unwrap_or(true),
            context_menu: config.context_menu.clone(),
            osc133_command_selection: options.osc133_command_selection.unwrap_or(true),
            word_separators: options
                .word_separators
                .clone()
                .unwrap_or_else(|| DEFAULT_WORD_SEPARATORS.to_owned()),
            copy_command: options.copy_command.clone(),
            copy_clipboard: options.copy_clipboard,
            copy_on_select: options.copy_on_select.unwrap_or(true),
            support_kitty_keyboard_protocol: options
                .support_kitty_keyboard_protocol
                .unwrap_or(true),
            support_kitty_graphics_protocol: options
                .support_kitty_graphics_protocol
                .unwrap_or(true),
            host_notification_protocol: options.host_notification_protocol.unwrap_or_default(),
            nested_session_handling: options.nested_session_handling.unwrap_or_default(),
            dangerously_enable_paste_buffer_read: options
                .dangerously_enable_paste_buffer_read
                .unwrap_or(false),
            visual_bell: options.visual_bell.unwrap_or(true),
            session_is_mirrored: options.mirror_session.unwrap_or(false),
            session_serialization: options.session_serialization.unwrap_or(true),
            serialize_pane_viewport: options.serialize_pane_viewport.unwrap_or(false),
            scrollback_lines_to_serialize: options.scrollback_lines_to_serialize,
            notification_position: options.notification_position.unwrap_or_default(),
            notification_timeout_secs: options
                .notification_timeout
                .unwrap_or(ElapsedSeconds::new(DEFAULT_NOTIFICATION_TIMEOUT_SECS)),
        }
    }
}

#[cfg(test)]
#[path = "./unit/screen_config_test.rs"]
mod screen_config_test;
