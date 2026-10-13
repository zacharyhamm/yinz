use std::path::PathBuf;

use yinz_types::colors::PaletteColor;
use yinz_types::data::{ContextMenuItem, InputMode};
use yinz_types::input::actions::Action;
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
use crate::screen_config::ScreenConfig;
use crate::theme::{FrameConfig, Theme, Themes, UiConfig};

/// A styling that differs from every other shade's, and from the default palette.
fn styling(shade: u8) -> Styling {
    let mut styling: Styling = default_palette().into();
    styling.text_unselected.base = PaletteColor::EightBit(shade);
    styling
}

fn theme(shade: u8) -> Theme {
    Theme {
        terminal: None,
        sourced_from_external_file: false,
        palette: styling(shade),
    }
}

/// Every option set to something other than its default, with a theme for each theme name.
fn every_option_set() -> Config {
    let mut themes = Themes::default();
    themes.insert("main".into(), theme(1));
    themes.insert("night".into(), theme(2));
    themes.insert("day".into(), theme(3));
    Config {
        options: Options {
            simplified_ui: Some(true),
            theme: Some("main".into()),
            theme_dark: Some("night".into()),
            theme_light: Some("day".into()),
            explicit_theme_hue: Some(ThemeHue::Light),
            default_mode: Some(InputMode::Locked),
            default_shell: Some(PathBuf::from("/bin/fish")),
            default_cwd: Some(PathBuf::from("/srv")),
            default_layout: Some(PathBuf::from("compact")),
            layout_dir: Some(PathBuf::from("/layouts")),
            agent_double_escape: Some(true),
            pane_animations: Some(false),
            pane_frame_style: Some(PaneFrameStyle::Full),
            max_agent_width: Some(100),
            notification_position: Some(NotificationPosition::BottomLeft),
            notification_timeout: Some(ElapsedSeconds::new(3)),
            mirror_session: Some(true),
            copy_command: Some("wl-copy".into()),
            copy_clipboard: Some(Clipboard::Primary),
            copy_on_select: Some(false),
            osc8_hyperlinks: Some(false),
            scrollback_editor: Some(PathBuf::from("/usr/bin/vi")),
            auto_layout: Some(false),
            session_serialization: Some(false),
            serialize_pane_viewport: Some(true),
            scrollback_lines_to_serialize: Some(500),
            styled_underlines: Some(false),
            background_opacity: Some(40),
            support_kitty_keyboard_protocol: Some(false),
            support_kitty_graphics_protocol: Some(false),
            stacked_resize: Some(false),
            agent_pane_min_columns: Some(10),
            agent_pane_min_rows: Some(5),
            stacked_pane_list: Some(false),
            advanced_mouse_actions: Some(false),
            mouse_scroll_resize: Some(false),
            mouse_hover_effects: Some(false),
            visual_bell: Some(false),
            focus_follows_mouse: Some(true),
            mouse_click_through: Some(true),
            context_menu_on_right_click: Some(true),
            context_menu_override_agent_mouse: Some(true),
            osc133_command_selection: Some(false),
            word_separators: Some(",;".into()),
            host_notification_protocol: Some(HostNotificationProtocol::Osc99),
            nested_session_handling: Some(NestedSessionHandling::Descend),
            dangerously_enable_paste_buffer_read: Some(true),
            ..Default::default()
        },
        themes,
        ui: UiConfig {
            pane_frames: FrameConfig {
                rounded_corners: true,
                hide_session_name: true,
            },
        },
        context_menu: ContextMenu::from_items(vec![ContextMenuItem::new(
            "Only this",
            vec![Action::CloseFocus],
        )]),
        ..Default::default()
    }
}

#[test]
fn defaults_match_the_documented_fallbacks() {
    // No `..`: a field added to `ScreenConfig` has to say what its default is here.
    let ScreenConfig {
        agent_theme_mapping: _,
        theme,
        terminal_theme: _,
        terminal_themes: _,
        themes: _,
        theme_name: _,
        theme_dark_name: _,
        theme_light_name: _,
        host_theme_dark,
        host_theme_light,
        explicit_theme_hue,
        rounded_corners,
        hide_session_name,
        pane_frame_style,
        max_agent_width,
        simplified_ui,
        pane_animations,
        styled_underlines,
        background_opacity,
        osc8_hyperlinks,
        default_mode,
        default_shell,
        default_cwd,
        default_editor,
        default_layout_name,
        layout_dir,
        auto_layout,
        stacked_resize,
        stacked_pane_list,
        agent_pane_min_size,
        agent_double_escape,
        advanced_mouse_actions,
        mouse_scroll_resize,
        mouse_hover_effects,
        focus_follows_mouse,
        mouse_click_through,
        context_menu_on_right_click,
        context_menu_override_agent_mouse,
        context_menu,
        osc133_command_selection,
        word_separators,
        copy_command,
        copy_clipboard,
        copy_on_select,
        support_kitty_keyboard_protocol,
        support_kitty_graphics_protocol,
        host_notification_protocol,
        nested_session_handling,
        dangerously_enable_paste_buffer_read,
        visual_bell,
        session_is_mirrored,
        session_serialization,
        serialize_pane_viewport,
        scrollback_lines_to_serialize,
        notification_position,
        notification_timeout_secs,
    } = ScreenConfig::from_config(&Config::default());

    assert_eq!(theme, default_palette().into());
    assert_eq!(host_theme_dark, None);
    assert_eq!(host_theme_light, None);
    assert_eq!(explicit_theme_hue, None);
    assert!(!rounded_corners);
    assert!(!hide_session_name);
    assert_eq!(pane_frame_style, PaneFrameStyle::Titles);
    assert_eq!(max_agent_width, 0);
    assert!(!simplified_ui);
    assert!(pane_animations);
    assert!(styled_underlines);
    assert_eq!(background_opacity, 85);
    assert!(osc8_hyperlinks);
    assert_eq!(default_mode, None);
    assert_eq!(default_shell, None);
    assert_eq!(default_cwd, None);
    assert_eq!(default_editor, None);
    assert_eq!(default_layout_name, None);
    assert_eq!(layout_dir, None);
    assert!(auto_layout);
    assert!(stacked_resize);
    assert!(stacked_pane_list);
    assert_eq!(
        agent_pane_min_size,
        Size {
            cols: DEFAULT_AGENT_PANE_MIN_COLUMNS,
            rows: DEFAULT_AGENT_PANE_MIN_ROWS,
        }
    );
    assert!(!agent_double_escape);
    assert!(advanced_mouse_actions);
    assert!(mouse_scroll_resize);
    assert!(mouse_hover_effects);
    assert!(!focus_follows_mouse);
    assert!(!mouse_click_through);
    assert!(context_menu_on_right_click);
    assert!(context_menu_override_agent_mouse);
    assert_eq!(context_menu, ContextMenu::default());
    assert!(osc133_command_selection);
    assert_eq!(word_separators, DEFAULT_WORD_SEPARATORS);
    assert_eq!(copy_command, None);
    assert_eq!(copy_clipboard, None);
    assert!(copy_on_select);
    assert!(support_kitty_keyboard_protocol);
    assert!(support_kitty_graphics_protocol);
    assert_eq!(
        host_notification_protocol,
        HostNotificationProtocol::default()
    );
    assert_eq!(nested_session_handling, NestedSessionHandling::default());
    assert!(!dangerously_enable_paste_buffer_read);
    assert!(visual_bell);
    assert!(!session_is_mirrored);
    assert!(session_serialization);
    assert!(!serialize_pane_viewport);
    assert_eq!(scrollback_lines_to_serialize, None);
    assert_eq!(notification_position, NotificationPosition::default());
    assert_eq!(
        notification_timeout_secs,
        ElapsedSeconds::new(DEFAULT_NOTIFICATION_TIMEOUT_SECS)
    );
}

#[test]
fn every_option_reaches_its_field() {
    let config = every_option_set();
    let ScreenConfig {
        agent_theme_mapping: _,
        theme,
        terminal_theme: _,
        terminal_themes: _,
        themes: _,
        theme_name: _,
        theme_dark_name: _,
        theme_light_name: _,
        host_theme_dark,
        host_theme_light,
        explicit_theme_hue,
        rounded_corners,
        hide_session_name,
        pane_frame_style,
        max_agent_width,
        simplified_ui,
        pane_animations,
        styled_underlines,
        background_opacity,
        osc8_hyperlinks,
        default_mode,
        default_shell,
        default_cwd,
        default_editor,
        default_layout_name,
        layout_dir,
        auto_layout,
        stacked_resize,
        stacked_pane_list,
        agent_pane_min_size,
        agent_double_escape,
        advanced_mouse_actions,
        mouse_scroll_resize,
        mouse_hover_effects,
        focus_follows_mouse,
        mouse_click_through,
        context_menu_on_right_click,
        context_menu_override_agent_mouse,
        context_menu,
        osc133_command_selection,
        word_separators,
        copy_command,
        copy_clipboard,
        copy_on_select,
        support_kitty_keyboard_protocol,
        support_kitty_graphics_protocol,
        host_notification_protocol,
        nested_session_handling,
        dangerously_enable_paste_buffer_read,
        visual_bell,
        session_is_mirrored,
        session_serialization,
        serialize_pane_viewport,
        scrollback_lines_to_serialize,
        notification_position,
        notification_timeout_secs,
    } = ScreenConfig::from_config(&config);

    assert_eq!(theme, styling(1));
    assert_eq!(host_theme_dark, Some(styling(2)));
    assert_eq!(host_theme_light, Some(styling(3)));
    assert_eq!(explicit_theme_hue, Some(ThemeHue::Light));
    assert!(rounded_corners);
    assert!(hide_session_name);
    assert_eq!(pane_frame_style, PaneFrameStyle::Full);
    assert_eq!(max_agent_width, 100);
    assert!(simplified_ui);
    assert!(!pane_animations);
    assert!(!styled_underlines);
    assert_eq!(background_opacity, 40);
    assert!(!osc8_hyperlinks);
    assert_eq!(default_mode, Some(InputMode::Locked));
    assert_eq!(default_shell, Some(PathBuf::from("/bin/fish")));
    assert_eq!(default_cwd, Some(PathBuf::from("/srv")));
    assert_eq!(default_editor, Some(PathBuf::from("/usr/bin/vi")));
    assert_eq!(default_layout_name.as_deref(), Some("compact"));
    assert_eq!(layout_dir, Some(PathBuf::from("/layouts")));
    assert!(!auto_layout);
    assert!(!stacked_resize);
    assert!(!stacked_pane_list);
    assert_eq!(agent_pane_min_size, Size { cols: 10, rows: 5 });
    assert!(agent_double_escape);
    assert!(!advanced_mouse_actions);
    assert!(!mouse_scroll_resize);
    assert!(!mouse_hover_effects);
    assert!(focus_follows_mouse);
    assert!(mouse_click_through);
    assert!(context_menu_on_right_click);
    assert!(context_menu_override_agent_mouse);
    assert_eq!(context_menu, config.context_menu);
    assert!(!osc133_command_selection);
    assert_eq!(word_separators, ",;");
    assert_eq!(copy_command.as_deref(), Some("wl-copy"));
    assert_eq!(copy_clipboard, Some(Clipboard::Primary));
    assert!(!copy_on_select);
    assert!(!support_kitty_keyboard_protocol);
    assert!(!support_kitty_graphics_protocol);
    assert_eq!(host_notification_protocol, HostNotificationProtocol::Osc99);
    assert_eq!(nested_session_handling, NestedSessionHandling::Descend);
    assert!(dangerously_enable_paste_buffer_read);
    assert!(!visual_bell);
    assert!(session_is_mirrored);
    assert!(!session_serialization);
    assert!(serialize_pane_viewport);
    assert_eq!(scrollback_lines_to_serialize, Some(500));
    assert_eq!(notification_position, NotificationPosition::BottomLeft);
    assert_eq!(notification_timeout_secs, ElapsedSeconds::new(3));
}

#[test]
fn a_theme_name_the_config_does_not_define_leaves_that_side_of_the_pair_unset() {
    let mut config = every_option_set();
    config.options.theme_dark = Some("nope".into());

    let screen_config = ScreenConfig::from_config(&config);

    assert_eq!(screen_config.host_theme_dark, None);
    assert_eq!(screen_config.host_theme_light, Some(styling(3)));
}

#[test]
fn pane_frames_off_wins_over_the_frame_style() {
    let mut config = every_option_set();
    config.options.pane_frames = Some(false);

    assert_eq!(
        ScreenConfig::from_config(&config).pane_frame_style,
        PaneFrameStyle::None
    );
}

#[test]
fn screen_theme_catalog_contains_all_resolved_themes_in_name_order() {
    let mut config = Config {
        themes: crate::assets::get_default_themes(),
        ..Default::default()
    };
    config.themes.insert(
        "a custom theme".into(),
        crate::theme::Theme {
            terminal: None,
            palette: styling(42),
            sourced_from_external_file: true,
        },
    );
    config.options.theme = Some("a custom theme".into());
    config.options.theme_dark = Some("night".into());
    config.options.theme_light = Some("day".into());
    let screen = ScreenConfig::from_config(&config);
    assert_eq!(screen.themes.len(), config.themes.inner().len());
    assert_eq!(screen.themes["a custom theme"], styling(42));
    assert_eq!(screen.theme_name, config.options.theme);
    assert_eq!(screen.theme_dark_name, config.options.theme_dark);
    assert_eq!(screen.theme_light_name, config.options.theme_light);
    let names: Vec<_> = screen.themes.keys().collect();
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn terminal_theme_resolves_the_selected_name_and_host_switch_palettes() {
    let mut config = Config {
        themes: crate::theme::Themes::from_string(
            include_str!("../../assets/themes/nord.kdl"),
            false,
        )
        .unwrap(),
        ..Default::default()
    };
    config.options.theme = Some("nord".into());
    config.options.theme_dark = Some("nord".into());
    let expected = config.themes.get_theme("nord").unwrap().terminal.unwrap();
    assert_eq!(
        config.terminal_theme_config(config.options.theme.as_ref()),
        Some(expected)
    );
    let resolved = ScreenConfig::from_config(&config);
    assert_eq!(resolved.terminal_theme, Some(expected));
    assert_eq!(resolved.terminal_themes["nord"], expected);
    config.options.theme = Some("missing".into());
    assert_eq!(ScreenConfig::from_config(&config).terminal_theme, None);
}

#[test]
fn right_click_menu_respects_explicit_options() {
    for enabled in [false, true] {
        let mut config = Config::default();
        config.options.context_menu_on_right_click = Some(enabled);
        assert_eq!(
            ScreenConfig::from_config(&config).context_menu_on_right_click,
            enabled
        );
    }
}

#[test]
fn agent_mouse_override_respects_explicit_options() {
    for enabled in [false, true] {
        let mut config = Config::default();
        config.options.context_menu_override_agent_mouse = Some(enabled);
        assert_eq!(
            ScreenConfig::from_config(&config).context_menu_override_agent_mouse,
            enabled
        );
    }
}
