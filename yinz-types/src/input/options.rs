//! Handles cli and configuration options
use crate::data::InputMode;
use crate::internal_ids::HostId;
use crate::internal_ids::SessionName;
use crate::style::ThemeHue;
use crate::units::{ElapsedMillis, ElapsedSeconds};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;

pub const DEFAULT_WORD_SEPARATORS: &str = "[]{}<>()";

#[derive(Copy, Clone, Debug, PartialEq, Deserialize, Serialize, Default)]
pub enum OnForceClose {
    #[serde(alias = "quit")]
    Quit,
    #[default]
    #[serde(alias = "detach")]
    Detach,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub enum NestedSessionHandling {
    #[default]
    #[serde(alias = "ask")]
    Ask,
    #[serde(alias = "fullscreen")]
    Fullscreen,
    #[serde(alias = "descend")]
    Descend,
    #[serde(alias = "never")]
    Never,
}

impl FromStr for NestedSessionHandling {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Ask" | "ask" => Ok(Self::Ask),
            "Fullscreen" | "fullscreen" => Ok(Self::Fullscreen),
            "Descend" | "descend" => Ok(Self::Descend),
            "Never" | "never" => Ok(Self::Never),
            _ => Err(format!("No such nested_session_handling: {}", s)),
        }
    }
}

/// Which escape sequence desktop notifications coming from panes are forwarded to the host
/// terminal with.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub enum HostNotificationProtocol {
    /// Detect from the host terminal environment.
    #[default]
    #[serde(alias = "auto")]
    Auto,
    /// The legacy iTerm2 protocol, understood by most terminals.
    #[serde(alias = "osc9")]
    Osc9,
    /// kitty's notification protocol.
    #[serde(alias = "osc99")]
    Osc99,
    /// Ring the terminal bell instead.
    #[serde(alias = "bell")]
    Bell,
    /// Do not forward notifications to the host terminal.
    #[serde(alias = "off")]
    Off,
}

impl HostNotificationProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Osc9 => "osc9",
            Self::Osc99 => "osc99",
            Self::Bell => "bell",
            Self::Off => "off",
        }
    }
}

impl FromStr for HostNotificationProtocol {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Auto" | "auto" => Ok(Self::Auto),
            "Osc9" | "osc9" => Ok(Self::Osc9),
            "Osc99" | "osc99" => Ok(Self::Osc99),
            "Bell" | "bell" => Ok(Self::Bell),
            "Off" | "off" => Ok(Self::Off),
            _ => Err(format!("No such host_notification_protocol: {}", s)),
        }
    }
}

impl FromStr for OnForceClose {
    type Err = Box<dyn std::error::Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "quit" => Ok(Self::Quit),
            "detach" => Ok(Self::Detach),
            e => Err(e.to_string().into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PaneFrameStyle {
    Full,
    #[default]
    Titles,
    None,
}

impl PaneFrameStyle {
    pub fn draws_full_frames(&self) -> bool {
        matches!(self, PaneFrameStyle::Full)
    }

    pub fn draws_titles(&self) -> bool {
        matches!(self, PaneFrameStyle::Titles)
    }

    pub fn from_options(options: &Options) -> Self {
        if options.pane_frames == Some(false) {
            return PaneFrameStyle::None;
        }
        match options.pane_frame_style {
            Some(PaneFrameStyle::Full) => PaneFrameStyle::Full,
            _ => PaneFrameStyle::Titles,
        }
    }
}

impl FromStr for PaneFrameStyle {
    type Err = Box<dyn std::error::Error>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "full" => Ok(PaneFrameStyle::Full),
            "titles" => Ok(PaneFrameStyle::Titles),
            "none" => Ok(PaneFrameStyle::None),
            e => Err(format!(
                "Unknown pane frame style: '{}' (expected 'full', 'titles' or 'none')",
                e
            )
            .into()),
        }
    }
}

/// The corner notifications appear in. Stacks grow away from the edge they're anchored to: down
/// from a top corner, up from a bottom one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationPosition {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

impl NotificationPosition {
    pub fn as_str(&self) -> &'static str {
        match self {
            NotificationPosition::TopLeft => "top-left",
            NotificationPosition::TopRight => "top-right",
            NotificationPosition::BottomLeft => "bottom-left",
            NotificationPosition::BottomRight => "bottom-right",
        }
    }

    pub fn is_top(&self) -> bool {
        matches!(
            self,
            NotificationPosition::TopLeft | NotificationPosition::TopRight
        )
    }

    pub fn is_left(&self) -> bool {
        matches!(
            self,
            NotificationPosition::TopLeft | NotificationPosition::BottomLeft
        )
    }
}

impl FromStr for NotificationPosition {
    type Err = Box<dyn std::error::Error>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().replace('_', "-").as_str() {
            "top-left" => Ok(NotificationPosition::TopLeft),
            "top-right" => Ok(NotificationPosition::TopRight),
            "bottom-left" => Ok(NotificationPosition::BottomLeft),
            "bottom-right" => Ok(NotificationPosition::BottomRight),
            e => Err(format!(
                "Unknown notification position: '{}' (expected 'top-left', 'top-right', 'bottom-left' or 'bottom-right')",
                e
            )
            .into()),
        }
    }
}

pub const DEFAULT_BACKGROUND_OPACITY: u8 = 85;

/// How long a notification stays on screen when `notification_timeout` is unset.
pub const DEFAULT_NOTIFICATION_TIMEOUT_SECS: u64 = 10;
/// The floor a spawned agent pane's equal share of a tab must clear, as a terminal size:
/// `agent_pane_min_columns` by `agent_pane_min_rows`, compared by area. See
/// `Screen::agent_pane_target` in yinz-screen for the rule these feed.
pub const DEFAULT_AGENT_PANE_MIN_COLUMNS: usize = 80;
pub const DEFAULT_AGENT_PANE_MIN_ROWS: usize = 24;

#[derive(Clone, Default, Debug, PartialEq, Deserialize, Serialize)]
/// Shared configuration options. Command-line parsing and conversion live in
/// `yinz-cli`, keeping this model independent of clap.
pub struct Options {
    /// Map managed agent palettes to semantic theme colors.
    #[serde(default)]
    pub agent_theme_mapping: Option<crate::agent::theme::AgentThemeMapping>,
    /// Allow plugins to use a more simplified layout
    /// that is compatible with more fonts (true or false)
    #[serde(default)]
    pub simplified_ui: Option<bool>,
    /// Set the default theme
    pub theme: Option<String>,
    /// Theme name to apply when the host terminal reports a dark color palette
    /// (CSI 2031 / DSR 997). Requires `theme_light` to also be set; if either
    /// is missing the static `theme` remains authoritative.
    pub theme_dark: Option<String>,
    /// Theme name to apply when the host terminal reports a light color palette
    /// (CSI 2031 / DSR 997). Requires `theme_dark` to also be set; if either
    /// is missing the static `theme` remains authoritative.
    pub theme_light: Option<String>,
    /// Pin the session to a dark or light appearance ("dark" or "light"),
    /// resolved before the first render and kept authoritative over ambient
    /// host terminal reports (CSI 2031 / DSR 997). When unset, the session
    /// follows the host terminal.
    pub explicit_theme_hue: Option<ThemeHue>,
    /// Set the default mode
    pub default_mode: Option<InputMode>,
    /// Set the default shell
    pub default_shell: Option<PathBuf>,
    /// Set the default cwd
    pub default_cwd: Option<PathBuf>,
    /// Set the default layout
    pub default_layout: Option<PathBuf>,
    /// Set the layout_dir, defaults to
    /// subdirectory of config dir
    pub layout_dir: Option<PathBuf>,
    /// Set the theme_dir, defaults to
    /// subdirectory of config dir
    pub theme_dir: Option<PathBuf>,
    /// Set the folder holding your own plugins, defaults to the
    /// "plugins" subdirectory of the config dir
    pub user_plugin_dir: Option<PathBuf>,
    #[serde(default)]
    /// Set the handling of mouse events (true or false)
    /// Can be temporarily bypassed by the [SHIFT] key
    pub mouse_mode: Option<bool>,
    /// Require two plain Escape presses within one second in agent panes (default: false).
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
    #[serde(default)]
    pub detachable_panes: Option<bool>,
    #[serde(default)]
    /// Set display of the pane frames (true or false)
    pub pane_frames: Option<bool>,
    /// Animate pane opening effects (default: true).
    #[serde(default)]
    pub pane_animations: Option<bool>,
    /// Background blending percentage for overlays that opt into transparency.
    #[serde(default)]
    pub background_opacity: Option<u8>,
    #[serde(default)]
    pub pane_frame_style: Option<PaneFrameStyle>,
    /// The corner notifications appear in
    /// (top-left, top-right, bottom-left or bottom-right)
    #[serde(default)]
    pub notification_position: Option<NotificationPosition>,
    /// How long a notification stays on screen, in seconds
    #[serde(default)]
    pub notification_timeout: Option<ElapsedSeconds>,
    #[serde(default)]
    /// Mirror session when multiple users are connected (true or false)
    pub mirror_session: Option<bool>,
    /// Set behaviour on force close (quit or detach)
    pub on_force_close: Option<OnForceClose>,
    pub scroll_buffer_size: Option<usize>,
    /// Maximum text columns for any agent pane (values below 80 disable the limit).
    #[serde(default)]
    pub max_agent_width: Option<usize>,

    /// Switch to using a user supplied command for clipboard instead of OSC52
    #[serde(default)]
    pub copy_command: Option<String>,

    /// OSC52 destination clipboard
    #[serde(default)]
    pub copy_clipboard: Option<Clipboard>,

    /// Automatically copy when selecting text (true or false)
    #[serde(default)]
    pub copy_on_select: Option<bool>,

    /// Enable OSC8 hyperlink output (true or false)
    #[serde(default)]
    pub osc8_hyperlinks: Option<bool>,

    /// Explicit full path to open the scrollback editor (default is $EDITOR or $VISUAL)
    pub scrollback_editor: Option<PathBuf>,

    /// The name of the session to create when starting Yinz
    #[serde(default)]
    pub session_name: Option<SessionName>,

    /// Whether to attach to a session specified in "session-name" if it exists
    #[serde(default)]
    pub attach_to_session: Option<bool>,

    /// Whether to lay out panes in a predefined set of layouts whenever possible
    #[serde(default)]
    pub auto_layout: Option<bool>,

    /// Whether sessions should be serialized to the HD so that they can be later resurrected,
    /// default is true
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
    #[serde(default)]
    pub host_id: Option<HostId>,

    /// Whether pane viewports are serialized along with the session, default is false
    #[serde(default)]
    pub serialize_pane_viewport: Option<bool>,

    /// Scrollback lines to serialize along with the pane viewport when serializing sessions, 0
    /// defaults to the scrollback size. If this number is higher than the scrollback size, it will
    /// also default to the scrollback size
    #[serde(default)]
    pub scrollback_lines_to_serialize: Option<usize>,

    /// Whether to use ANSI styled underlines
    #[serde(default)]
    pub styled_underlines: Option<bool>,

    /// The interval at which to serialize sessions for resurrection (in seconds)
    pub serialization_interval: Option<ElapsedSeconds>,

    /// If true, will disable writing session metadata to disk
    pub disable_session_metadata: Option<bool>,

    /// Whether to enable support for the Kitty keyboard protocol (must also be supported by the
    /// host terminal), defaults to true if the terminal supports it
    #[serde(default)]
    pub support_kitty_keyboard_protocol: Option<bool>,

    /// Whether to ask the host terminal to report the modifier keys themselves, so the UI can
    /// know that eg. Alt is being held rather than only that it was part of a chord. Defaults
    /// to false: it needs Kitty keyboard flags that also make every printable key arrive as an
    /// escape code, which US-ASCII survives but AltGr glyphs, dead keys and IME commits do not.
    #[serde(default)]
    pub report_modifier_keys: Option<bool>,

    /// Whether to enable support for the Kitty graphics (image) protocol (must also be supported
    /// by the host terminal), defaults to true if the terminal supports it
    #[serde(default)]
    pub support_kitty_graphics_protocol: Option<bool>,

    /// Whether to stack panes when resizing beyond a certain size
    /// default is true
    #[serde(default)]
    pub stacked_resize: Option<bool>,

    /// The smallest tiled pane worth giving a newly spawned coding agent, in columns.
    /// With `agent_pane_min_rows` it names an area: before an agent pane joins a tab, the
    /// tab's tiled panes and the newcomer are imagined sharing the viewport equally, and a
    /// tab whose share would fall below columns x rows sends the agent to the roomiest other
    /// tab that clears it, or to a new tab. 0 in either disables the rule.
    /// default is 80
    #[serde(default)]
    pub agent_pane_min_columns: Option<usize>,

    /// The rows half of `agent_pane_min_columns`
    /// default is 24
    #[serde(default)]
    pub agent_pane_min_rows: Option<usize>,
    #[serde(default)]
    pub stacked_pane_list: Option<bool>,

    /// Whether to enable mouse hover effects and pane grouping functionality
    /// default is true
    #[serde(default)]
    pub advanced_mouse_actions: Option<bool>,

    /// Whether Ctrl+ScrollWheel resizes panes
    /// default is true
    #[serde(default)]
    pub mouse_scroll_resize: Option<bool>,

    /// Whether to enable mouse hover visual effects (frame highlight and help text)
    /// default is true
    #[serde(default)]
    pub mouse_hover_effects: Option<bool>,

    /// Whether to show visual bell indicators (pane/tab frame flash and [!] suffix)
    /// default is true
    #[serde(default)]
    pub visual_bell: Option<bool>,

    /// Whether to focus panes on mouse hover (true or false)
    /// default is false
    #[serde(default)]
    pub focus_follows_mouse: Option<bool>,

    /// Whether clicking a pane to focus it also sends the click into the pane (true or false)
    /// default is false
    #[serde(default)]
    pub mouse_click_through: Option<bool>,

    /// Whether right-clicking a pane opens a context menu; default is true.
    /// Mouse-tracking programs keep right-click, except recognized agents when
    /// context_menu_override_agent_mouse is enabled.
    #[serde(default)]
    pub context_menu_on_right_click: Option<bool>,

    /// Whether recognized agents use the context menu even while tracking the mouse.
    /// Requires context_menu_on_right_click; default is true.
    #[serde(default)]
    pub context_menu_override_agent_mouse: Option<bool>,

    /// Whether triple-clicking inside shell-marked (OSC 133) command output selects the command
    /// and its output rather than the logical line
    /// default is true
    #[serde(default)]
    pub osc133_command_selection: Option<bool>,

    /// Characters that terminate a word when double-clicking to select it, in addition to
    /// whitespace (which is always a separator)
    /// default is "[]{}<>()"
    #[serde(default)]
    pub word_separators: Option<String>,

    /// Which escape sequence desktop notifications from panes are forwarded to the host
    /// terminal with (auto, osc9, osc99, bell, off)
    #[serde(default)]
    pub host_notification_protocol: Option<HostNotificationProtocol>,

    /// A command to run after the discovery of running commands when serializing, for the purpose
    /// of manipulating the command (eg. with a regex) before it gets serialized
    pub post_command_discovery_hook: Option<String>,

    /// How to handle a nested Yinz session detected inside a pane
    /// (ask, fullscreen, descend, never)
    #[serde(default)]
    pub nested_session_handling: Option<NestedSessionHandling>,
    #[serde(default)]
    pub dangerously_enable_paste_buffer_read: Option<bool>,

    /// How long, in milliseconds, a locked-mode Ctrl+g is held as a prefix: type a plain `g`
    /// within the window to send a literal Ctrl+g to the pane and stay locked, otherwise the
    /// window expires and the mode switches to Normal. 0 disables the prefix entirely, so
    /// Ctrl+g unlocks immediately.
    /// default is 250
    #[serde(default)]
    pub locked_escape_timeout_ms: Option<ElapsedMillis>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Default)]
pub enum Clipboard {
    #[default]
    #[serde(alias = "system")]
    System,
    #[serde(alias = "primary")]
    Primary,
}

impl FromStr for Clipboard {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "System" | "system" => Ok(Self::System),
            "Primary" | "primary" => Ok(Self::Primary),
            _ => Err(format!("No such clipboard: {}", s)),
        }
    }
}

impl Options {
    pub fn from_yaml(from_yaml: Option<Options>) -> Options {
        from_yaml.unwrap_or_default()
    }
    /// Merges two [`Options`] structs, a `Some` in `other`
    /// will supersede a `Some` in `self`
    pub fn merge(&self, other: Options) -> Options {
        let agent_double_escape = other.agent_double_escape.or(self.agent_double_escape);
        let mouse_mode = other.mouse_mode.or(self.mouse_mode);
        let detachable_panes = other.detachable_panes.or(self.detachable_panes);

        let background_opacity = other.background_opacity.or(self.background_opacity);
        let pane_animations = other.pane_animations.or(self.pane_animations);
        let pane_frames = other.pane_frames.or(self.pane_frames);
        let pane_frame_style = other.pane_frame_style.or(self.pane_frame_style);
        let notification_position = other.notification_position.or(self.notification_position);
        let notification_timeout = other.notification_timeout.or(self.notification_timeout);
        let auto_layout = other.auto_layout.or(self.auto_layout);
        let mirror_session = other.mirror_session.or(self.mirror_session);
        let simplified_ui = other.simplified_ui.or(self.simplified_ui);
        let default_mode = other.default_mode.or(self.default_mode);
        let default_shell = other.default_shell.or_else(|| self.default_shell.clone());
        let default_cwd = other.default_cwd.or_else(|| self.default_cwd.clone());
        let default_layout = other.default_layout.or_else(|| self.default_layout.clone());
        let layout_dir = other.layout_dir.or_else(|| self.layout_dir.clone());
        let theme_dir = other.theme_dir.or_else(|| self.theme_dir.clone());
        let user_plugin_dir = other
            .user_plugin_dir
            .or_else(|| self.user_plugin_dir.clone());
        let theme = other.theme.or_else(|| self.theme.clone());
        let theme_dark = other.theme_dark.or_else(|| self.theme_dark.clone());
        let theme_light = other.theme_light.or_else(|| self.theme_light.clone());
        let explicit_theme_hue = other.explicit_theme_hue.or(self.explicit_theme_hue);
        let on_force_close = other.on_force_close.or(self.on_force_close);
        let scroll_buffer_size = other.scroll_buffer_size.or(self.scroll_buffer_size);
        let max_agent_width = other.max_agent_width.or(self.max_agent_width);
        let locked_escape_timeout_ms = other
            .locked_escape_timeout_ms
            .or(self.locked_escape_timeout_ms);
        let copy_command = other.copy_command.or_else(|| self.copy_command.clone());
        let copy_clipboard = other.copy_clipboard.or(self.copy_clipboard);
        let copy_on_select = other.copy_on_select.or(self.copy_on_select);
        let osc8_hyperlinks = other.osc8_hyperlinks.or(self.osc8_hyperlinks);
        let scrollback_editor = other
            .scrollback_editor
            .or_else(|| self.scrollback_editor.clone());
        let session_name = other.session_name.or_else(|| self.session_name.clone());
        let attach_to_session = other.attach_to_session.or(self.attach_to_session);
        let session_serialization = other.session_serialization.or(self.session_serialization);
        let host_id = other.host_id.clone().or_else(|| self.host_id.clone());
        let serialize_pane_viewport = other
            .serialize_pane_viewport
            .or(self.serialize_pane_viewport);
        let scrollback_lines_to_serialize = other
            .scrollback_lines_to_serialize
            .or(self.scrollback_lines_to_serialize);
        let styled_underlines = other.styled_underlines.or(self.styled_underlines);
        let serialization_interval = other.serialization_interval.or(self.serialization_interval);
        let disable_session_metadata = other
            .disable_session_metadata
            .or(self.disable_session_metadata);
        let support_kitty_keyboard_protocol = other
            .support_kitty_keyboard_protocol
            .or(self.support_kitty_keyboard_protocol);
        let report_modifier_keys = other.report_modifier_keys.or(self.report_modifier_keys);
        let support_kitty_graphics_protocol = other
            .support_kitty_graphics_protocol
            .or(self.support_kitty_graphics_protocol);
        let stacked_resize = other.stacked_resize.or(self.stacked_resize);
        let agent_pane_min_columns = other.agent_pane_min_columns.or(self.agent_pane_min_columns);
        let agent_pane_min_rows = other.agent_pane_min_rows.or(self.agent_pane_min_rows);
        let stacked_pane_list = other.stacked_pane_list.or(self.stacked_pane_list);
        let advanced_mouse_actions = other.advanced_mouse_actions.or(self.advanced_mouse_actions);
        let mouse_scroll_resize = other.mouse_scroll_resize.or(self.mouse_scroll_resize);
        let mouse_hover_effects = other.mouse_hover_effects.or(self.mouse_hover_effects);
        let visual_bell = other.visual_bell.or(self.visual_bell);
        let focus_follows_mouse = other.focus_follows_mouse.or(self.focus_follows_mouse);
        let mouse_click_through = other.mouse_click_through.or(self.mouse_click_through);
        let context_menu_on_right_click = other
            .context_menu_on_right_click
            .or(self.context_menu_on_right_click);
        let context_menu_override_agent_mouse = other
            .context_menu_override_agent_mouse
            .or(self.context_menu_override_agent_mouse);
        let osc133_command_selection = other
            .osc133_command_selection
            .or(self.osc133_command_selection);
        let word_separators = other
            .word_separators
            .or_else(|| self.word_separators.clone());
        let post_command_discovery_hook = other
            .post_command_discovery_hook
            .or(self.post_command_discovery_hook.clone());
        let nested_session_handling = other
            .nested_session_handling
            .or(self.nested_session_handling);
        let host_notification_protocol = other
            .host_notification_protocol
            .or(self.host_notification_protocol);
        let dangerously_enable_paste_buffer_read = other
            .dangerously_enable_paste_buffer_read
            .or(self.dangerously_enable_paste_buffer_read);

        Options {
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
            agent_double_escape,
            mouse_mode,
            detachable_panes,

            pane_frames,
            pane_animations,
            background_opacity,
            pane_frame_style,
            notification_position,
            notification_timeout,
            mirror_session,
            on_force_close,
            scroll_buffer_size,
            max_agent_width,
            locked_escape_timeout_ms,
            copy_command,
            copy_clipboard,
            copy_on_select,
            osc8_hyperlinks,
            scrollback_editor,
            session_name,
            attach_to_session,
            auto_layout,
            session_serialization,
            host_id,
            serialize_pane_viewport,
            scrollback_lines_to_serialize,
            styled_underlines,
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
            agent_theme_mapping: other.agent_theme_mapping.or(self.agent_theme_mapping),
            osc133_command_selection,
            word_separators,
            post_command_discovery_hook,
            nested_session_handling,
            host_notification_protocol,
            dangerously_enable_paste_buffer_read,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_frame_style_from_str_accepts_all_variants() {
        assert_eq!(
            "full".parse::<PaneFrameStyle>().unwrap(),
            PaneFrameStyle::Full
        );
        assert_eq!(
            "titles".parse::<PaneFrameStyle>().unwrap(),
            PaneFrameStyle::Titles
        );
        assert_eq!(
            "none".parse::<PaneFrameStyle>().unwrap(),
            PaneFrameStyle::None
        );
        assert_eq!(
            "NONE".parse::<PaneFrameStyle>().unwrap(),
            PaneFrameStyle::None
        );
        assert!("bogus".parse::<PaneFrameStyle>().is_err());
    }

    #[test]
    fn host_notification_protocol_from_str_accepts_all_variants() {
        assert_eq!(
            "auto".parse::<HostNotificationProtocol>().unwrap(),
            HostNotificationProtocol::Auto
        );
        assert_eq!(
            "osc9".parse::<HostNotificationProtocol>().unwrap(),
            HostNotificationProtocol::Osc9
        );
        assert_eq!(
            "osc99".parse::<HostNotificationProtocol>().unwrap(),
            HostNotificationProtocol::Osc99
        );
        assert_eq!(
            "bell".parse::<HostNotificationProtocol>().unwrap(),
            HostNotificationProtocol::Bell
        );
        assert_eq!(
            "off".parse::<HostNotificationProtocol>().unwrap(),
            HostNotificationProtocol::Off
        );
        assert!("bogus".parse::<HostNotificationProtocol>().is_err());
    }

    #[test]
    fn every_host_notification_protocol_variant_stringifies_back_to_itself() {
        for variant in [
            HostNotificationProtocol::Auto,
            HostNotificationProtocol::Osc9,
            HostNotificationProtocol::Osc99,
            HostNotificationProtocol::Bell,
            HostNotificationProtocol::Off,
        ] {
            assert_eq!(
                variant
                    .as_str()
                    .parse::<HostNotificationProtocol>()
                    .unwrap(),
                variant
            );
        }
    }

    #[test]
    fn the_host_notification_protocol_defaults_to_auto() {
        assert_eq!(
            HostNotificationProtocol::default(),
            HostNotificationProtocol::Auto
        );
    }

    #[test]
    fn a_configured_host_notification_protocol_is_overridden_by_the_merged_in_one() {
        let config = Options {
            host_notification_protocol: Some(HostNotificationProtocol::Osc9),
            ..Default::default()
        };
        let layout = Options {
            host_notification_protocol: Some(HostNotificationProtocol::Bell),
            ..Default::default()
        };
        assert_eq!(
            config.merge(layout).host_notification_protocol,
            Some(HostNotificationProtocol::Bell)
        );
    }

    #[test]
    fn an_unset_host_notification_protocol_does_not_clobber_the_configured_one() {
        let config = Options {
            host_notification_protocol: Some(HostNotificationProtocol::Osc9),
            ..Default::default()
        };
        assert_eq!(
            config.merge(Options::default()).host_notification_protocol,
            Some(HostNotificationProtocol::Osc9)
        );
    }
}
