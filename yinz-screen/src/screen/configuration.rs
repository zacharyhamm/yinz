//! Screen reconfiguration and theme propagation.
use super::get_default_editor;
use super::Screen;
use super::ThemePolicy;
use crate::plugins::PluginInstruction;
use crate::pty::get_default_shell;
use crate::route::NotificationEnd;
use crate::ClientId;
use yinz_config::screen_config::ScreenConfig;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::Event;
use yinz_types::input::command::RunCommand;
use yinz_types::input::command::TerminalAction;
use yinz_types::input::keybinds::Keybinds;
use yinz_types::style::HostTerminalThemeMode;
use yinz_types::style::Styling;
use yinz_types::style::ThemeHue;

impl Screen {
    /// Apply a reloaded configuration. `config` is destructured without `..`, so a field added
    /// to `ScreenConfig` does not compile until this says what a reload does with it.
    pub fn reconfigure(
        &mut self,
        config: ScreenConfig,
        keybinds: Keybinds,
        client_id: ClientId,
    ) -> Result<()> {
        if config.session_is_mirrored && !self.core.session_is_mirrored {
            // Move clients independently before mirrored tab switches assume they all share
            // one tab. Prefer the caller's focus, then the lowest connected client ID.
            let mut clients: Vec<ClientId> = self
                .core
                .connected_clients
                .borrow()
                .iter()
                .copied()
                .collect();
            clients.sort_unstable_by_key(|id| (*id != client_id, *id));
            let shared_focus = clients.iter().find_map(|id| {
                self.get_active_tab(*id)
                    .ok()
                    .map(|tab| (*id, tab.position, tab.get_active_pane_id(*id)))
            });
            if let Some((reference_client_id, tab_position, pane_id)) = shared_focus {
                for client_id in clients {
                    self.switch_active_tab(tab_position, None, true, client_id)
                        .context("failed to reconcile client tabs before enabling mirroring")?;
                    let old_pane_id = self.get_active_pane_id(&client_id);
                    if let Some(pane_id) = pane_id.filter(|id| Some(*id) != old_pane_id) {
                        self.get_active_tab_mut(client_id)?
                            .focus_pane_with_id(pane_id, false, false, client_id)
                            .context("failed to reconcile pane focus before enabling mirroring")?;
                        if let Some(old_pane_id) = old_pane_id {
                            self.report_key_passthrough_state(client_id, old_pane_id, pane_id);
                        }
                        self.clear_bell_for_focused_pane(client_id);
                        self.sync_scroll_mode_on_focus(client_id).non_fatal();
                    }
                }
                self.get_active_tab_mut(reference_client_id)?
                    .reconcile_tiled_focus(reference_client_id);
                self.log_and_report_session_state()
                    .context("failed to report reconciled client focus")?;
            }
        }
        let ScreenConfig {
            overlay_tab_plugins,
            plugin_aliases,
            agent_theme_mapping,
            // Per-client configs must not change what shared applications are told
            // or the pane palette they are painted on. The session baseline arrives
            // through `ScreenInstruction::SetApplicationThemeConfig` instead.
            terminal_theme: _,
            application_theme: _,
            terminal_themes,
            theme,
            themes,
            theme_name,
            theme_dark_name,
            theme_light_name,
            host_theme_dark,
            host_theme_light,
            explicit_theme_hue,
            rounded_corners,
            hide_session_name,
            pane_frame_style,
            max_agent_width,
            tab_groups,
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
        } = config;
        let new_default_mode = default_mode.unwrap_or_default();
        let should_support_arrow_fonts = !simplified_ui;
        let disable_kitty_keyboard_protocol = !support_kitty_keyboard_protocol;

        if let Some(client) = self
            .palette
            .theme_preview
            .as_ref()
            .map(|preview| preview.client)
        {
            self.close_command_palette(client);
        }
        self.core.themes = themes;
        self.core.terminal_themes = terminal_themes;
        self.core.style.agent_theme_mapping = agent_theme_mapping;
        self.core.theme_name = theme_name;
        self.core.theme_dark_name = theme_dark_name;
        self.core.theme_light_name = theme_light_name;
        self.set_pane_animations(pane_animations);
        self.set_agent_double_escape(agent_double_escape);
        self.core.host_theme_dark_styling = host_theme_dark;
        self.core.host_theme_light_styling = host_theme_light;
        self.core.arrow_fonts = should_support_arrow_fonts;

        self.core.overlay_tab_plugins = overlay_tab_plugins;
        self.core.plugin_aliases = plugin_aliases;
        // global configuration
        self.core.style.colors = theme;
        self.core.default_mode_info.max_agent_width = Some(max_agent_width);
        self.core.default_mode_info.tab_groups = Some(tab_groups);
        self.core.default_mode_info.update_theme(theme);
        self.core
            .default_mode_info
            .update_rounded_corners(rounded_corners);
        // `default_mode_info` is the fallback used by `change_mode` for
        // clients that don't yet have a per-client `mode_info` entry, so its
        // keybinds and base mode must be kept in sync with reconfigures.
        self.core
            .default_mode_info
            .update_keybinds(keybinds.clone());
        self.core
            .default_mode_info
            .update_default_mode(new_default_mode);
        self.core.default_shell = default_shell.clone().unwrap_or_else(get_default_shell);
        self.core.default_shell_action = default_shell.clone().map(|shell| {
            TerminalAction::RunCommand(RunCommand {
                command: shell,
                cwd: default_cwd.clone(),
                use_terminal_title: true,
                ..Default::default()
            })
        });
        self.core.default_editor = default_editor.clone().or_else(get_default_editor);
        self.core.auto_layout = auto_layout;
        self.core.copy_options.command = copy_command.clone();
        self.core.copy_options.copy_on_select = copy_on_select;
        self.core.pane_frame_style = pane_frame_style;
        self.core.max_agent_width = max_agent_width;
        self.core.advanced_mouse_actions = advanced_mouse_actions;
        self.core.mouse_scroll_resize = mouse_scroll_resize;
        self.core.mouse_hover_effects = mouse_hover_effects;
        self.core.visual_bell = visual_bell;
        self.core.focus_follows_mouse = focus_follows_mouse;
        self.core.mouse_click_through = mouse_click_through;
        // Menus already open keep the list they opened with - `set_context_menu_items` says why -
        // so nothing here has to repaint. The next right-click picks these up.
        self.menus.on_reconfigure(
            context_menu,
            context_menu_on_right_click,
            context_menu_override_agent_mouse,
        );
        self.core.osc133_command_selection = osc133_command_selection;
        self.core.word_separators = word_separators;
        self.nested.nested_session_handling = nested_session_handling;
        self.core.paste_buffer_read_enabled = dangerously_enable_paste_buffer_read;
        self.set_host_notification_protocol(host_notification_protocol);
        self.core
            .default_mode_info
            .update_arrow_fonts(should_support_arrow_fonts);
        self.core
            .default_mode_info
            .update_hide_session_name(hide_session_name);
        {
            *self.core.stacked_resize.borrow_mut() = stacked_resize;
        }
        {
            *self.core.stacked_pane_list.borrow_mut() = stacked_pane_list;
        }
        self.core.agent_pane_min_size = agent_pane_min_size;
        if let Some(copy_clipboard) = copy_clipboard {
            self.core.copy_options.clipboard = copy_clipboard;
        }
        // read when they are next needed: a layout dump, a toast, the next focus change
        self.core.session_serialization = session_serialization;
        self.core.serialize_pane_viewport = serialize_pane_viewport;
        self.core.scrollback_lines_to_serialize = scrollback_lines_to_serialize;
        self.core.session_is_mirrored = session_is_mirrored;
        self.notices
            .on_reconfigure(notification_position, notification_timeout_secs);
        if self.core.default_layout_name != default_layout_name
            || self.core.layout_dir != layout_dir
        {
            self.core.default_layout_name = default_layout_name;
            self.core.layout_dir = layout_dir;
            self.restart_layout_watcher();
        }
        // read every frame by the output
        self.core.styled_underlines = styled_underlines;
        self.core.background_opacity = background_opacity;
        self.core.osc8_hyperlinks = osc8_hyperlinks;
        self.core.explicitly_disable_kitty_keyboard_protocol = disable_kitty_keyboard_protocol;
        let kitty_graphics_support_changed =
            self.core.support_kitty_graphics_protocol != support_kitty_graphics_protocol;
        self.core.support_kitty_graphics_protocol = support_kitty_graphics_protocol;
        for mode in self.core.mode_info.values_mut() {
            mode.max_agent_width = Some(max_agent_width);
            mode.tab_groups = Some(tab_groups);
        }
        // Closing a preview above already returned `style.terminal` to the baseline block.
        for tab in self.core.tabs.values_mut() {
            tab.update_tab_groups(tab_groups)?;
            tab.update_terminal_theme(self.core.style.terminal);
            tab.update_agent_theme_mapping(self.core.style.agent_theme_mapping);
            tab.update_theme(theme);
            tab.update_rounded_corners(rounded_corners);
            tab.update_default_shell(default_shell.clone());
            tab.update_default_editor(self.core.default_editor.clone());
            tab.update_auto_layout(auto_layout);
            tab.update_copy_options(&self.core.copy_options);
            tab.set_pane_frames(pane_frame_style);
            tab.max_agent_width = max_agent_width;
            tab.update_arrow_fonts(should_support_arrow_fonts);
            tab.update_advanced_mouse_actions(advanced_mouse_actions);
            tab.update_mouse_scroll_resize(mouse_scroll_resize);
            tab.update_mouse_hover_effects(mouse_hover_effects);
            tab.update_focus_follows_mouse(focus_follows_mouse);
            tab.update_mouse_click_through(mouse_click_through);
            tab.update_context_menu_on_right_click(context_menu_on_right_click);
            tab.update_context_menu_override_agent_mouse(context_menu_override_agent_mouse);
            tab.update_selection_options(
                osc133_command_selection,
                self.core.word_separators.clone(),
            );
            tab.update_session_is_mirrored(session_is_mirrored);
            tab.update_render_options(styled_underlines, osc8_hyperlinks);
            tab.update_background_opacity(background_opacity);
            tab.update_kitty_keyboard_protocol_disabled(disable_kitty_keyboard_protocol);
            tab.sync_stacked_pane_list_mode();
        }
        if kitty_graphics_support_changed {
            self.reapply_kitty_graphics_support();
        }
        // Clear hover state when disabled
        if !mouse_hover_effects {
            for tab in self.core.tabs.values_mut() {
                tab.clear_mouse_hover_state();
            }
        }

        // client specific configuration
        if self.connected_clients_contains(&client_id) {
            let mode_info = self
                .core
                .mode_info
                .entry(client_id)
                .or_insert_with(|| self.core.default_mode_info.clone());
            mode_info.update_keybinds(keybinds);
            mode_info.update_default_mode(new_default_mode);
            mode_info.update_theme(theme);
            mode_info.update_arrow_fonts(should_support_arrow_fonts);
            mode_info.update_hide_session_name(hide_session_name);
            for tab in self.core.tabs.values_mut() {
                tab.change_mode_info(mode_info.clone(), client_id);
                tab.mark_active_pane_for_rerender(client_id);
            }
        }

        // this needs to be done separately at the end because it applies some of the above changes
        // and propagates them to plugins
        for tab in self.core.tabs.values_mut() {
            tab.update_input_modes()?;
        }
        self.broadcast_nested_shortcuts();
        self.reapply_theme_after_reconfigure(explicit_theme_hue);
        // Retained cells must repaint even when no pane output follows the reload.
        self.render()
            .context("failed to render after reconfiguration")
    }

    /// Apply a host-reported color-palette theme mode (CSI 2031 / DSR 997).
    ///
    /// The report is always remembered (so that unpinning can restore it), but
    /// it only repaints the session while the theme policy is `Auto`: an
    /// `explicit_theme_hue` or a manual `SetDarkTheme` / `SetLightTheme`
    /// outranks the host terminal.
    pub fn update_host_terminal_theme_mode(&mut self, mode: HostTerminalThemeMode) -> Result<()> {
        self.core.last_host_reported_theme_mode = Some(mode);
        if matches!(self.core.theme_policy, ThemePolicy::Pinned(_)) {
            return Ok(());
        }
        self.apply_theme_mode(mode)
    }

    /// Make `mode` the session's effective chrome theme mode. It
    /// 1. de-duplicates against the last-known mode,
    /// 2. swaps the chrome styling to the configured `theme_dark` / `theme_light`
    ///    (when both are configured) by reusing the reconfigure propagation,
    /// 3. fans out an `Event::HostTerminalThemeChanged` plugin event.
    ///
    /// It never writes `CSI ?997;n` into shared PTYs nor changes the pane palette:
    /// applications follow the session baseline, which announces its own mode
    /// changes (`refresh_application_baseline`).
    pub(super) fn apply_theme_mode(&mut self, mode: HostTerminalThemeMode) -> Result<()> {
        let err_context = || "Failed to update host terminal theme mode".to_string();

        // dedupe
        if self.core.host_terminal_theme_mode == Some(mode) {
            return Ok(());
        }
        self.core.host_terminal_theme_mode = Some(mode);
        self.repaint_theme_mode(mode)?;
        if let Some(client) = self
            .palette
            .theme_preview
            .as_ref()
            .map(|preview| preview.client)
        {
            self.refresh_theme_preview(client);
        }

        // fan out the plugin event
        self.core
            .bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::HostTerminalThemeChanged(mode),
            )]))
            .with_context(err_context)?;

        self.render()?;
        Ok(())
    }

    /// Swap the chrome styling to the `theme_dark` / `theme_light` styling for `mode` and
    /// push it into every tab and client, without announcing a mode change to anyone.
    /// The pane palette stays on the application baseline.
    pub(super) fn repaint_theme_mode(&mut self, mode: HostTerminalThemeMode) -> Result<()> {
        if self.palette.theme_preview.is_some() {
            return Ok(());
        }
        if self.core.host_theme_dark_styling.is_some()
            && self.core.host_theme_light_styling.is_some()
        {
            let theme = match mode {
                HostTerminalThemeMode::Dark => self.core.host_theme_dark_styling,
                HostTerminalThemeMode::Light => self.core.host_theme_light_styling,
            };
            if let Some(theme) = theme {
                self.repaint_theme(theme)?;
            }
        }
        Ok(())
    }

    pub(super) fn repaint_theme(&mut self, theme: Styling) -> Result<()> {
        self.core.style.colors = theme;
        self.core.default_mode_info.update_theme(theme);
        for tab in self.core.tabs.values_mut() {
            tab.update_terminal_theme(self.core.style.terminal);
            tab.update_agent_theme_mapping(self.core.style.agent_theme_mapping);
            tab.update_theme(theme);
            tab.set_force_render();
        }
        let default_for_new = self.core.default_mode_info.clone();
        for client in self.core.active_tab_ids.keys().copied() {
            let mode_info = self
                .core
                .mode_info
                .entry(client)
                .or_insert_with(|| default_for_new.clone());
            mode_info.update_theme(theme);
            for tab in self.core.tabs.values_mut() {
                tab.change_mode_info(mode_info.clone(), client);
                tab.mark_active_pane_for_rerender(client);
            }
        }
        for tab in self.core.tabs.values_mut() {
            tab.update_input_modes()
                .context("failed to propagate theme to plugins")?;
        }
        Ok(())
    }

    /// Apply a manual host-terminal theme mode change requested via the CLI or a
    /// keybinding. Surfaces a clear error to the CLI if the session theme has no
    /// `theme_dark` / `theme_light` pair; otherwise pins the session to `mode` - a
    /// deliberate choice is not reverted by a later host report. The pin is
    /// session-wide: it moves the application baseline (answers, pane palette and one
    /// `CSI ?997;n` to opted-in panes per mode change) and the chrome.
    pub fn apply_manual_host_terminal_theme_mode(
        &mut self,
        mode: HostTerminalThemeMode,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> Result<()> {
        if !self.application_theme_has_pair() {
            if let Some(c) = completion_tx.as_mut() {
                c.set_exit_status(1);
                c.set_error_message(
                    "Manual theme switching requires both `theme_dark` and `theme_light` to be configured."
                        .to_string(),
                );
            }
            return Ok(());
        }
        self.pin_application_mode(mode);
        self.core.theme_policy = ThemePolicy::Pinned(mode);
        self.apply_theme_mode(mode)?;
        self.render()
    }

    /// `ToggleTheme`: pin the opposite of the mode applications are told, not of the
    /// last viewer report, so one press always flips what applications see.
    pub fn toggle_manual_host_terminal_theme_mode(
        &mut self,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> Result<()> {
        let next = match self.core.style.application.mode {
            HostTerminalThemeMode::Dark => HostTerminalThemeMode::Light,
            HostTerminalThemeMode::Light => HostTerminalThemeMode::Dark,
        };
        self.apply_manual_host_terminal_theme_mode(next, completion_tx)
    }

    /// Resolve the session to the dark theme when a `theme_dark` /
    /// `theme_light` pair is configured but nothing has decided the mode yet
    /// (the host has not reported and nothing is pinned). Without this the
    /// session would sit on the unrelated static `theme` until the first host
    /// report. This is a default, not a pin: a later host report still wins.
    pub(super) fn resolve_default_theme_mode(&mut self) -> Result<()> {
        if self.core.host_terminal_theme_mode.is_some() {
            return Ok(());
        }
        if !matches!(self.core.theme_policy, ThemePolicy::Auto) {
            return Ok(());
        }
        if self.core.host_theme_dark_styling.is_some()
            && self.core.host_theme_light_styling.is_some()
        {
            self.apply_theme_mode(HostTerminalThemeMode::Dark)?;
        }
        Ok(())
    }

    /// Re-resolve the session's current mode against the (possibly changed)
    /// `theme_dark` / `theme_light` definitions after a reconfigure, instead
    /// of leaving it painted with the static theme `reconfigure` applied.
    ///
    /// When the mode itself is unchanged only the palette is repainted: the plugin event and
    /// the `CSI ?997;n` DSR announce a *change* of mode, and re-sending them on every config
    /// reload would make every opted-in app re-run its theme hooks for nothing.
    pub(super) fn reapply_effective_theme_mode(&mut self) -> Result<()> {
        let known_mode = match self.core.theme_policy {
            ThemePolicy::Pinned(mode) => Some(mode),
            ThemePolicy::Auto => self.core.host_terminal_theme_mode,
        };
        let Some(mode) = known_mode else {
            return Ok(());
        };
        if self.core.host_terminal_theme_mode == Some(mode) {
            self.repaint_theme_mode(mode)?;
            self.render()?;
        } else {
            self.apply_theme_mode(mode)?;
        }
        Ok(())
    }

    /// Apply the configured `explicit_theme_hue`: `Some` pins the session to
    /// that hue, `None` hands authority back to the host terminal and
    /// restores whatever it last reported. The effective mode is cleared
    /// first so that pinning (or unpinning) to the hue the session is
    /// already in still repaints the resolved palette rather than deduping
    /// into a no-op and leaving the static theme on screen.
    pub(super) fn apply_configured_explicit_theme_hue(
        &mut self,
        explicit_theme_hue: Option<ThemeHue>,
    ) -> Result<()> {
        self.core.configured_explicit_theme_hue = explicit_theme_hue;
        self.core.host_terminal_theme_mode = None;
        match explicit_theme_hue {
            Some(hue) => {
                let mode = HostTerminalThemeMode::from(hue);
                self.core.theme_policy = ThemePolicy::Pinned(mode);
                self.apply_theme_mode(mode)
            },
            None => {
                self.core.theme_policy = ThemePolicy::Auto;
                match self.core.last_host_reported_theme_mode {
                    Some(mode) => self.apply_theme_mode(mode),
                    None => Ok(()),
                }
            },
        }
    }
}
