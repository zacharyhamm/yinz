use super::*;

pub(super) fn dispatch(
    screen: &mut Screen,
    state: &mut EventLoopState,
    event: ScreenInstruction,
) -> Result<std::ops::ControlFlow<()>> {
    match event {
        ScreenInstruction::SessionMigration(request) => screen.session_migration(request),
        ScreenInstruction::CloseSelf(..) => {
            unreachable!("close_self is resolved before dispatch")
        },
        ScreenInstruction::SubscribePaneOutput(plugin, client, context, generation) => {
            let subscriber = (plugin, client, generation);
            screen.core.pane_output_subscriptions.remove(&subscriber);
            if let Some(context) = context {
                screen
                    .core
                    .pane_output_subscriptions
                    .insert(subscriber, context);
            }
        },
        ScreenInstruction::UnsubscribePaneOutput(plugin, generation) => {
            screen
                .core
                .pane_output_subscriptions
                .retain(|(id, _, registered), _| {
                    *id != plugin || (generation.is_some() && *registered != generation)
                });
        },
        ScreenInstruction::PtyBytes(pid, vte_bytes) => {
            screen.handle_pty_bytes(pid, vte_bytes, &mut state.pending)?;
            let _ = screen
                .core
                .bus
                .senders
                .send_to_background_jobs(BackgroundJob::RenderToClients);
        },
        ScreenInstruction::RemotePaneReady(pid) => {
            if let Some(tab) = screen.tab_containing_pane_mut(PaneId::Terminal(pid)) {
                tab.remote_pane_ready(pid)
                    .context("failed to prepare remote pane")?;
            }
            screen.render()?;
        },
        ScreenInstruction::PluginBytes(plugin_render_assets) => {
            plugins::plugin_bytes(state, screen, plugin_render_assets)?
        },
        ScreenInstruction::Render => {
            screen.render()?;
        },
        ScreenInstruction::RenderWriterReady => {
            screen.resume_deferred_render();
        },
        ScreenInstruction::RenderToClients => clients::render_to_clients(state, screen)?,
        ScreenInstruction::CommitAgentLaunch { terminal_id, reply } => {
            agents::commit_agent_launch(screen, terminal_id, reply)?
        },
        ScreenInstruction::ReleaseAgentLaunch { terminal_id, reply } => {
            agents::release_agent_launch(screen, terminal_id, reply)?
        },
        ScreenInstruction::ClaimAgentLaunch {
            terminal_id,
            command,
            reply,
        } => {
            let size = screen.claim_agent_launch(terminal_id, command);
            let _ = reply.send(size);
        },
        ScreenInstruction::RegisterAgentLaunch { spawned, reply } => {
            let accepted = screen.register_launch(spawned);
            let _ = reply.send(accepted);
        },
        ScreenInstruction::ConfirmTerminalPlacement(terminal_id, generation) => {
            state.terminal_placements.insert(terminal_id, generation);
        },
        ScreenInstruction::NewPane(
            pid,
            initial_pane_title,
            hold_for_command,
            invoked_with,
            new_pane_placement,
            start_suppressed,
            client_or_tab_index,
            completion_tx,
            set_blocking,
        ) => panes::new_pane(
            state,
            screen,
            pid,
            initial_pane_title,
            hold_for_command,
            invoked_with,
            new_pane_placement,
            start_suppressed,
            client_or_tab_index,
            completion_tx,
            set_blocking,
        )?,
        ScreenInstruction::OpenInPlaceEditor(pid, client_tab_index_or_pane_id) => {
            panes::open_in_place_editor(state, screen, pid, client_tab_index_or_pane_id)?
        },
        ScreenInstruction::TogglePaneEmbedOrFloating(client_id, _completion_tx) => {
            active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| tab
                    .toggle_pane_embed_or_floating(client_id), ?);
            screen.log_and_report_session_state()?;
            screen.render()?;
        },
        ScreenInstruction::ToggleFloatingPanes(client_id, default_shell, completion_tx) => {
            panes::toggle_floating_panes(screen, client_id, default_shell, completion_tx)?
        },
        ScreenInstruction::ShowFloatingPanes {
            client_id,
            tab_id,
            completion,
        } => {
            screen.show_floating_panes_in_tab(client_id, tab_id, completion)?;
            screen.sync_scroll_mode_on_focus(client_id)?;
            screen.log_and_report_session_state()?;
            screen.render()?;
        },
        ScreenInstruction::HideFloatingPanes {
            client_id,
            tab_id,
            completion,
        } => {
            screen.hide_floating_panes_in_tab(client_id, tab_id, completion)?;
            screen.sync_scroll_mode_on_focus(client_id)?;
            screen.log_and_report_session_state()?;
            screen.render()?;
        },
        ScreenInstruction::AreFloatingPanesVisible {
            client_id,
            tab_id,
            completion,
        } => {
            screen.are_floating_panes_visible_in_tab(client_id, tab_id, completion)?;
        },
        ScreenInstruction::ContextMenuHandleKeyIfOpen {
            key,
            toggle_command_palette,
            open_agents,
            client_id,
            completion,
        } => input::context_menu_handle_key_if_open(
            screen,
            key,
            toggle_command_palette,
            open_agents,
            client_id,
            completion,
        )?,
        ScreenInstruction::PaletteThemeSaved {
            client,
            request,
            result,
        } => {
            screen.palette_theme_saved(client, request, result);
            screen.render()?;
        },
        ScreenInstruction::CommandPalette(instruction) => {
            if screen.plugin_palette_instruction(instruction) {
                screen.render()?;
            }
        },
        ScreenInstruction::ToggleCommandPalette(client_id, _completion) => {
            screen.toggle_command_palette(client_id);
            screen.render()?;
        },
        ScreenInstruction::OpenAgents(client_id, _completion) => {
            screen.open_agents(client_id);
            screen.render()?;
        },
        ScreenInstruction::ResetAgentEscape(client_id) => {
            for tab in screen.core.tabs.values_mut() {
                tab.pending_agent_escapes
                    .retain(|(client, _), _| *client != client_id);
            }
        },
        ScreenInstruction::TextFromClient(text, client_id, completion) => {
            screen.text_from_client(text, client_id, completion)?;
        },
        ScreenInstruction::PasteFromClient(text, client_id, completion) => {
            screen.paste_from_client(text, client_id, completion)?;
        },
        ScreenInstruction::WriteCharacter(
            key_with_modifier,
            raw_bytes,
            is_kitty_keyboard_protocol,
            client_id,
            completion,
        ) => input::write_character(
            state,
            screen,
            key_with_modifier,
            raw_bytes,
            is_kitty_keyboard_protocol,
            client_id,
            completion,
        )?,
        ScreenInstruction::Resize(client_id, strategy, _completion_tx) => {
            layout::resize(screen, client_id, strategy)?
        },
        ScreenInstruction::SwitchFocus(client_id, _completion_tx) => {
            input::switch_focus(screen, client_id)?
        },
        ScreenInstruction::FocusNextPane(client_id, _completion_tx) => {
            input::focus_next_pane(screen, client_id, _completion_tx)?
        },
        ScreenInstruction::FocusPreviousPane(client_id, _completion_tx) => {
            input::focus_previous_pane(screen, client_id, _completion_tx)?
        },
        ScreenInstruction::FocusLastPane(client_id, _completion_tx) => {
            input::focus_last_pane(screen, client_id)?
        },
        ScreenInstruction::MoveFocusLeft(client_id, _completion_tx) => {
            input::move_focus_left(screen, client_id, _completion_tx)?
        },
        ScreenInstruction::MoveFocusLeftOrPreviousTab(client_id, _completion_tx) => {
            input::move_focus_left_or_previous_tab(screen, client_id, _completion_tx)?
        },
        ScreenInstruction::MoveFocusDown(client_id, _completion_tx) => {
            input::move_focus_down(screen, client_id, _completion_tx)?
        },
        ScreenInstruction::MoveFocusRight(client_id, _completion_tx) => {
            input::move_focus_right(screen, client_id, _completion_tx)?
        },
        ScreenInstruction::MoveFocusRightOrNextTab(client_id, _completion_tx) => {
            input::move_focus_right_or_next_tab(screen, client_id, _completion_tx)?
        },
        ScreenInstruction::MoveFocusUp(client_id, _completion_tx) => {
            input::move_focus_up(screen, client_id, _completion_tx)?
        },
        ScreenInstruction::ClearScreen(client_id, _completion_tx) => {
            panes::clear_screen(screen, client_id)?
        },
        ScreenInstruction::DumpScreen(
            file,
            client_id,
            full,
            pane_id,
            completion_tx,
            cli_client_id,
            ansi,
        ) => panes::dump_screen(
            screen,
            file,
            client_id,
            full,
            pane_id,
            completion_tx,
            cli_client_id,
            ansi,
        )?,
        ScreenInstruction::DumpLayout(default_shell, client_id, completion_tx) => {
            layout::dump_layout(screen, default_shell, client_id, completion_tx)?
        },
        ScreenInstruction::ListClientsMetadata(default_shell, client_id, completion_tx) => {
            clients::list_clients_metadata(screen, default_shell, client_id, completion_tx)?
        },
        ScreenInstruction::ListPanes {
            show_all,
            response_channel,
        } => {
            let err_context = || "Failed to list panes";
            let pane_entries = screen
                .collect_pane_list(show_all)
                .with_context(err_context)?;
            let _ = response_channel.send(pane_entries);
        },
        ScreenInstruction::ListNotifications { response_channel } => {
            notifications::list_notifications(screen, response_channel)?
        },
        ScreenInstruction::ListTabs {
            client_id,
            response_channel,
        } => {
            let err_context = || "Failed to list tabs";
            let tab_infos = screen
                .collect_tab_list(client_id)
                .with_context(err_context)?;
            let _ = response_channel.send(tab_infos);
        },
        ScreenInstruction::GetCurrentTabInfo {
            client_id,
            response_channel,
        } => {
            let err_context = || "Failed to get current tab info";
            let tab_info = screen
                .get_current_tab_info(client_id)
                .with_context(err_context)?;
            let _ = response_channel.send(tab_info);
        },
        ScreenInstruction::DumpLayoutToPlugin {
            plugin_id,
            tab_index,
            response_channel,
            trace,
        } => plugins::dump_layout_to_plugin(screen, plugin_id, tab_index, response_channel, trace)?,
        ScreenInstruction::GetClientTimezone {
            client_id,
            response_channel,
        } => {
            response_channel
                .send(screen.client_timezone(client_id))
                .context("reply with client timezone")
                .non_fatal();
        },
        ScreenInstruction::ReportClientTimezones {
            plugin_id,
            client_id,
            instance,
            generation,
        } => clients::report_client_timezones(screen, plugin_id, client_id, instance, generation)?,
        ScreenInstruction::GetCellPixelSize {
            client_id,
            response_channel,
        } => {
            response_channel
                .send(screen.cell_pixel_size(client_id))
                .context("reply with client cell pixel size")
                .non_fatal();
        },
        ScreenInstruction::ReportCellPixelSize(plugin_id, client_id) => {
            screen.report_cell_pixel_size(Some(plugin_id), client_id);
        },
        ScreenInstruction::GetFocusedPaneInfo {
            client_id,
            response_channel,
        } => input::get_focused_pane_info(screen, client_id, response_channel)?,
        ScreenInstruction::GetPaneInfo {
            pane_id,
            response_channel,
        } => {
            let pane_info = screen.get_pane_info(pane_id);
            let _ = response_channel.send(pane_info);
        },
        ScreenInstruction::GetTabInfo {
            tab_id,
            response_channel,
        } => {
            let tab_info = screen.get_tab_info(tab_id, &screen.plugin_tab_names());
            let _ = response_channel.send(tab_info);
        },
        ScreenInstruction::ListClientsToPlugin(plugin_id, client_id, generation) => {
            plugins::list_clients_to_plugin(screen, plugin_id, client_id, generation)?
        },
        ScreenInstruction::EditScrollback(client_id, ansi, completion_tx) => {
            input::edit_scrollback(screen, client_id, ansi, completion_tx)?
        },
        ScreenInstruction::GetPaneScrollback {
            pane_id,
            client_id,
            get_full_scrollback,
            response_channel,
        } => input::get_pane_scrollback(
            screen,
            pane_id,
            client_id,
            get_full_scrollback,
            response_channel,
        )?,
        ScreenInstruction::ScrollUp(client_id, _completion_tx) => {
            input::scroll_up(screen, client_id)?
        },
        ScreenInstruction::MovePane(client_id, _completion_tx) => {
            panes::move_pane(screen, client_id)?
        },
        ScreenInstruction::MovePaneBackwards(client_id, _completion_tx) => {
            panes::move_pane_backwards(screen, client_id)?
        },
        ScreenInstruction::MovePaneDown(client_id, _completion_tx) => {
            panes::move_pane_down(screen, client_id)?
        },
        ScreenInstruction::MovePaneUp(client_id, _completion_tx) => {
            panes::move_pane_up(screen, client_id)?
        },
        ScreenInstruction::MovePaneRight(client_id, _completion_tx) => {
            panes::move_pane_right(screen, client_id)?
        },
        ScreenInstruction::MovePaneLeft(client_id, _completion_tx) => {
            panes::move_pane_left(screen, client_id)?
        },
        ScreenInstruction::ScrollUpAt(point, client_id, _completion_tx) => {
            input::scroll_up_at(screen, point, client_id)?
        },
        ScreenInstruction::ScrollDown(client_id, _completion_tx) => {
            input::scroll_down(screen, client_id)?
        },
        ScreenInstruction::ScrollDownAt(point, client_id, _completion_tx) => {
            input::scroll_down_at(screen, point, client_id)?
        },
        ScreenInstruction::ScrollToBottom(client_id, _completion_tx) => {
            input::scroll_to_bottom(screen, client_id)?
        },
        ScreenInstruction::ScrollToTop(client_id, _completion_tx) => {
            input::scroll_to_top(screen, client_id)?
        },
        ScreenInstruction::ScrollToPreviousPrompt(client_id, _completion_tx) => {
            input::scroll_to_previous_prompt(screen, client_id)?
        },
        ScreenInstruction::ScrollToNextPrompt(client_id, _completion_tx) => {
            input::scroll_to_next_prompt(screen, client_id)?
        },
        ScreenInstruction::SelectCommandAtScrollPosition(client_id, _completion_tx) => {
            input::select_command_at_scroll_position(screen, client_id)?
        },
        ScreenInstruction::CopyLastCommandOutput(client_id, _completion_tx) => {
            input::copy_last_command_output(screen, client_id)?
        },
        ScreenInstruction::MoveTextSelection(direction, client_id, _completion_tx) => {
            panes::move_text_selection(screen, direction, client_id)?
        },
        ScreenInstruction::SwapTextSelectionEnds(client_id, _completion_tx) => {
            layout::swap_text_selection_ends(screen, client_id)?
        },
        ScreenInstruction::MoveNotificationSelection(direction, client_id, _completion_tx) => {
            if screen.move_notification_selection(direction, client_id) {
                screen.render()?;
            }
        },
        ScreenInstruction::ActivateLastNotification(client_id, _completion_tx) => {
            screen.activate_last_notification(client_id, Instant::now());
            screen.render()?;
        },
        ScreenInstruction::ReturnFromNotification(client_id, _completion_tx) => {
            screen.return_from_notification(client_id)?;
        },
        ScreenInstruction::ActivateNotification(client_id, _completion_tx) => {
            screen.activate_selected_notification(client_id, Instant::now());
            screen.render()?;
        },
        ScreenInstruction::ToggleNotificationDetail(client_id, _completion_tx) => {
            if screen.toggle_notification_detail(client_id) {
                screen.render()?;
            }
        },
        ScreenInstruction::ClearCommandOutputFlash(pane_id) => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.clear_command_output_flash(pane_id);
            }
            screen.render()?;
        },
        ScreenInstruction::PageScrollUp(client_id, _completion_tx) => {
            input::page_scroll_up(screen, client_id)?
        },
        ScreenInstruction::PageScrollDown(client_id, _completion_tx) => {
            input::page_scroll_down(screen, client_id)?
        },
        ScreenInstruction::HalfPageScrollUp(client_id, _completion_tx) => {
            input::half_page_scroll_up(screen, client_id)?
        },
        ScreenInstruction::HalfPageScrollDown(client_id, _completion_tx) => {
            input::half_page_scroll_down(screen, client_id)?
        },
        ScreenInstruction::ClearScroll(None, client_id)
            if screen.palette.palette_owns_input(client_id)
                || screen.permissions.permission_modal_open(client_id) => {},
        ScreenInstruction::ClearScroll(Some(pane_id), _) => {
            if screen.clear_scroll_for_pane_id(pane_id)? {
                screen.render()?;
            }
        },
        ScreenInstruction::ClearScroll(None, client_id) => input::clear_scroll(screen, client_id)?,
        ScreenInstruction::CloseFocusedPane(client_id, completion_tx) => {
            input::close_focused_pane(screen, client_id, completion_tx)?
        },
        ScreenInstruction::SetSelectable(pid, selectable) => {
            panes::set_selectable(state, screen, pid, selectable)?
        },
        ScreenInstruction::ShowPluginCursor(pid, client_id, cursor_position) => {
            plugins::show_plugin_cursor(state, screen, pid, client_id, cursor_position)?
        },
        ScreenInstruction::SetMouseSelectionSupport(pid, selection_support) => {
            input::set_mouse_selection_support(state, screen, pid, selection_support)?
        },
        ScreenInstruction::RetirePicker {
            finder,
            pane,
            client,
        } => panes::retire_picker(state, screen, finder, pane, client)?,
        ScreenInstruction::ClosePane(id, client_id, completion_tx, exit_status) => {
            panes::close_pane(state, screen, id, client_id, completion_tx, exit_status)?
        },
        ScreenInstruction::RestoreHeldPane(id, held) => {
            panes::restore_held_pane(state, screen, id, held)?
        },
        ScreenInstruction::HoldPane(id, exit_status, run_command) => {
            panes::hold_pane(state, screen, id, exit_status, run_command)?
        },
        ScreenInstruction::SetPaneSuspended(id, suspended) => {
            panes::set_pane_suspended(screen, id, suspended)?
        },
        ScreenInstruction::UpdatePaneName(c, client_id, _completion_tx) => {
            panes::update_pane_name(screen, c, client_id)?
        },
        ScreenInstruction::UpdatePaneHost(terminal_id, host) => {
            screen.set_pane_host(terminal_id, host)?;
        },
        ScreenInstruction::UpdateDetectedAgentTheme(terminal_id, theme) => {
            agents::update_detected_agent_theme(state, screen, terminal_id, theme)?
        },
        ScreenInstruction::UpdatePaneForegroundCommand(terminal_id, command, program) => {
            panes::update_pane_foreground_command(state, screen, terminal_id, command, program)?
        },
        ScreenInstruction::UpdatePaneCwd(terminal_id, cwd) => {
            panes::update_pane_cwd(screen, terminal_id, cwd)?
        },
        ScreenInstruction::SetPaneSubtitle(pane_id, subtitle) => {
            panes::set_pane_subtitle(screen, pane_id, subtitle)?
        },
        ScreenInstruction::SetPaneTitlePrefix(pane_id, prefix) => {
            panes::set_pane_title_prefix(state, screen, pane_id, prefix)?
        },
        ScreenInstruction::SetPaneCaption(pane_id, caption) => {
            panes::set_pane_caption(state, screen, pane_id, caption)?
        },
        ScreenInstruction::UndoRenamePane(client_id, _completion_tx) => {
            active_tab_and_connected_client_id!(
                screen,
                client_id,
                |tab: &mut Tab, client_id: ClientId| tab.undo_active_rename_pane(client_id), ?
            );
            screen.render()?;
        },
        ScreenInstruction::ToggleActiveTerminalFullscreen(client_id, _completion_tx) => {
            panes::toggle_active_terminal_fullscreen(screen, client_id)?
        },
        ScreenInstruction::ToggleActiveTerminalNoUiFullscreen(client_id, _completion_tx) => {
            panes::toggle_active_terminal_no_ui_fullscreen(screen, client_id)?
        },
        ScreenInstruction::TogglePaneFrames(_completion_tx) => panes::toggle_pane_frames(screen)?,
        ScreenInstruction::SetPaneFrameStyle(pane_frame_style, _completion_tx) => {
            panes::set_pane_frame_style(screen, pane_frame_style)?
        },
        ScreenInstruction::SwitchTabNext(client_id, _completion_tx) => {
            screen.switch_tab_next(None, true, client_id)?;
            screen.render()?;
        },
        ScreenInstruction::SwitchTabPrev(client_id, _completion_tx) => {
            screen.switch_tab_prev(None, true, client_id)?;
            screen.render()?;
        },
        ScreenInstruction::CloseTab(client_id, _completion_tx) => {
            tabs::close_tab(screen, client_id)?
        },
        ScreenInstruction::OpenInitialTabs {
            cwd,
            default_shell,
            tabs,
            swap_tiled,
            swap_floating,
            focused,
            client_id,
        } => tabs::open_initial_tabs(
            screen,
            cwd,
            default_shell,
            tabs,
            swap_tiled,
            swap_floating,
            focused,
            client_id,
        )?,
        ScreenInstruction::NewTab(
            cwd,
            default_shell,
            layout,
            floating_panes_layout,
            tab_name,
            (swap_tiled_layouts, swap_floating_layouts),
            initial_panes,
            block_on_first_terminal,
            should_change_focus_to_new_tab,
            client_id,
            completion_tx,
        ) => {
            screen.open_new_tab(
                cwd,
                default_shell,
                layout,
                floating_panes_layout,
                tab_name,
                swap_tiled_layouts,
                swap_floating_layouts,
                initial_panes,
                block_on_first_terminal,
                should_change_focus_to_new_tab,
                client_id,
                completion_tx,
            )?;
        },
        ScreenInstruction::ApplyLayout(
            layout,
            floating_panes_layout,
            new_pane_pids,
            new_floating_pane_pids,
            adopted_terminals,
            new_plugin_ids,
            tab_id,
            should_change_focus_to_new_tab,
            client_id,
            completion_tx,
            blocking_terminal,
        ) => layout::apply_layout(
            state,
            screen,
            layout,
            floating_panes_layout,
            new_pane_pids,
            new_floating_pane_pids,
            adopted_terminals,
            new_plugin_ids,
            tab_id,
            should_change_focus_to_new_tab,
            client_id,
            completion_tx,
            blocking_terminal,
        )?,
        ScreenInstruction::LayoutSpawnFailed(tab_id, destination) => {
            layout::layout_spawn_failed(state, screen, tab_id, destination)?
        },
        ScreenInstruction::TabGroup(action, client, completion) => {
            screen
                .tab_group_action(action, client, completion, &mut state.pending.tab_switches)
                .non_fatal();
            screen.render()?;
        },
        ScreenInstruction::GoToTab(tab_index, client_id, _completion_tx) => {
            tabs::go_to_tab(state, screen, tab_index, client_id)?
        },
        ScreenInstruction::GoToTabName(
            tab_name,
            default_shell,
            create,
            client_id,
            completion_tx,
        ) => tabs::go_to_tab_name(
            screen,
            tab_name,
            default_shell,
            create,
            client_id,
            completion_tx,
        )?,
        ScreenInstruction::UpdateTabName(c, client_id, completion_tx) => {
            tabs::update_tab_name(screen, c, client_id, completion_tx)?
        },
        ScreenInstruction::UndoRenameTab(client_id, _completion_tx) => {
            screen.undo_active_rename_tab(client_id)?;
            screen.render()?;
        },
        ScreenInstruction::MoveTabLeft(client_id, completion_tx) => {
            tabs::move_tab_left(state, screen, client_id, completion_tx)?
        },
        ScreenInstruction::MoveTabRight(client_id, completion_tx) => {
            tabs::move_tab_right(state, screen, client_id, completion_tx)?
        },
        ScreenInstruction::RequestRepaint(client_id) => {
            screen.request_client_repaint(client_id)?;
        },
        ScreenInstruction::RecomputeTabSize(client_id, new_size) => {
            layout::recompute_tab_size(screen, client_id, new_size)?
        },
        ScreenInstruction::TerminalPixelDimensions(client_id, pixel_dimensions) => {
            let character_cell_size_changed =
                screen.update_pixel_dimensions(client_id, pixel_dimensions);
            if character_cell_size_changed {
                screen.resize_pty_all_panes()?;
                screen.render()?;
            }
        },
        ScreenInstruction::TerminalBackgroundColor(background_color_instruction) => {
            screen.update_terminal_background_color(background_color_instruction);
        },
        ScreenInstruction::TerminalForegroundColor(background_color_instruction) => {
            screen.update_terminal_foreground_color(background_color_instruction);
        },
        ScreenInstruction::TerminalColorRegisters(color_registers) => {
            screen.update_terminal_color_registers(color_registers);
        },
        ScreenInstruction::SetKittyGraphicsSupport {
            client_id,
            supported,
        } => {
            screen.update_kitty_graphics_support(client_id, supported);
            screen.render()?;
        },
        ScreenInstruction::SetStructuredRenderSupport {
            client_id,
            supported,
        } => clients::set_structured_render_support(screen, client_id, supported)?,
        ScreenInstruction::RenderFrameAck { client_id, seq } => {
            if let Some(state) = screen.core.native_render_clients.get_mut(&client_id) {
                state.acknowledge(seq);
            }
        },
        ScreenInstruction::SetSixelSupport {
            client_id,
            supported,
        } => {
            screen.update_sixel_support(client_id, supported);
        },
        ScreenInstruction::SetSynchronizedOutputSupport {
            client_id,
            supported,
        } => {
            screen
                .update_synchronized_output_support(client_id, supported)
                .context("failed to update synchronized output support")?;
        },
        ScreenInstruction::ForwardHostQuery { pane_id, query } => {
            screen.forward_host_query(pane_id, query);
        },
        ScreenInstruction::NestedSessionMessageFromPane { pane_id, message } => {
            screen.handle_nested_session_message_from_pane(pane_id, message);
        },
        ScreenInstruction::NestedGuestPingTick { pane_id } => {
            screen.handle_nested_guest_ping_tick(pane_id);
        },
        ScreenInstruction::NestedSessionMessageFromHost { client_id, message } => {
            screen.handle_nested_session_message_from_host(client_id, message);
        },
        ScreenInstruction::GuestModalChoice {
            client_id,
            pane_id,
            outcome,
        } => {
            screen.handle_guest_modal_choice(client_id, pane_id, outcome);
        },
        ScreenInstruction::ForwardedReplyFromHost { token, reply_bytes } => {
            screen.handle_forwarded_reply_from_host(token, reply_bytes)?;
            // The handler's replay of `pending_pty_input` mutates the
            // grid. Without a render scheduling here the clients
            // keep displaying the pre-reply frame
            screen.render()?;
        },
        ScreenInstruction::ResumePaneAfterForward {
            pane_id,
            reply_bytes,
        } => {
            screen.resume_pane_after_forward(pane_id, reply_bytes)?;
            // Same rationale as ForwardedReplyFromHost above — the
            // resume's replay paints the grid, so we must schedule
            // a render before yielding back to the screen loop.
            screen.render()?;
        },
        ScreenInstruction::HostTerminalThemeChanged(mode) => {
            screen.update_host_terminal_theme_mode(mode)?;
        },
        ScreenInstruction::SetApplicationThemeConfig(config) => {
            screen.set_application_theme_config(config)?;
        },
        ScreenInstruction::SetDarkTheme(mut completion_tx) => {
            screen.apply_manual_host_terminal_theme_mode(
                HostTerminalThemeMode::Dark,
                &mut completion_tx,
            )?;
        },
        ScreenInstruction::SetLightTheme(mut completion_tx) => {
            screen.apply_manual_host_terminal_theme_mode(
                HostTerminalThemeMode::Light,
                &mut completion_tx,
            )?;
        },
        ScreenInstruction::ToggleTheme(mut completion_tx) => {
            screen.toggle_manual_host_terminal_theme_mode(&mut completion_tx)?;
        },
        ScreenInstruction::ChangeMode(input_mode, base_mode, client_id, _completion_tx) => {
            input::change_mode(state, screen, input_mode, base_mode, client_id)?
        },
        ScreenInstruction::ChangeModeForAllClients(input_mode, base_mode, _completion_tx) => {
            screen.change_mode_for_all_clients(input_mode, base_mode)?;
            screen.render()?;
        },
        ScreenInstruction::ToggleActiveSyncTab(client_id, _completion_tx) => {
            tabs::toggle_active_sync_tab(screen, client_id)?
        },
        ScreenInstruction::MouseEvent(event, client_id, _completion_tx) => {
            screen.handle_mouse_event(event, client_id);
        },
        ScreenInstruction::Copy(client_id, _completion_tx) => input::copy(screen, client_id)?,
        ScreenInstruction::Exit => {
            return Ok(std::ops::ControlFlow::Break(()));
        },
        ScreenInstruction::ToggleTab(client_id, _completion_tx) => {
            screen.toggle_tab(client_id)?;
            screen.render()?;
        },
        ScreenInstruction::AddClient(
            client_id,
            client_size,
            tab_number_to_focus,
            pane_id_to_focus,
        ) => clients::add_client(
            state,
            screen,
            client_id,
            client_size,
            tab_number_to_focus,
            pane_id_to_focus,
        )?,
        ScreenInstruction::RemoveClient(client_id) => {
            clients::remove_client(state, screen, client_id)?
        },
        ScreenInstruction::UpdateSearch(c, client_id, _completion_tx) => {
            active_tab_and_connected_client_id!(
                screen,
                client_id,
                |tab: &mut Tab, client_id: ClientId| tab.update_search_term(c, client_id)
            );
            screen.render()?;
        },
        ScreenInstruction::SearchDown(client_id, _completion_tx) => {
            active_tab_and_connected_client_id!(
                screen,
                client_id,
                |tab: &mut Tab, client_id: ClientId| tab.search_down(client_id)
            );
            screen.render()?;
        },
        ScreenInstruction::SearchUp(client_id, _completion_tx) => {
            active_tab_and_connected_client_id!(
                screen,
                client_id,
                |tab: &mut Tab, client_id: ClientId| tab.search_up(client_id)
            );
            screen.render()?;
        },
        ScreenInstruction::SearchToggleCaseSensitivity(client_id, _completion_tx) => {
            input::search_toggle_case_sensitivity(screen, client_id)?
        },
        ScreenInstruction::SearchToggleWrap(client_id, _completion_tx) => {
            active_tab_and_connected_client_id!(
                screen,
                client_id,
                |tab: &mut Tab, client_id: ClientId| tab.toggle_search_wrap(client_id)
            );
            screen.render()?;
        },
        ScreenInstruction::SearchToggleWholeWord(client_id, _completion_tx) => {
            input::search_toggle_whole_word(screen, client_id)?
        },
        ScreenInstruction::AddRedPaneFrameColorOverride(pane_ids, error_text) => {
            for pane_id in pane_ids {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.add_red_pane_frame_color_override(pane_id, error_text.clone());
                }
            }
            screen.render()?;
        },
        ScreenInstruction::AddHighlightPaneFrameColorOverride(pane_ids, error_text) => {
            panes::add_highlight_pane_frame_color_override(screen, pane_ids, error_text)?
        },
        ScreenInstruction::FlashPaneFrameColor(pane_ids) => {
            panes::flash_pane_frame_color(screen, pane_ids)?
        },
        ScreenInstruction::ClearPaneFrameColorOverride(pane_ids) => {
            for pane_id in pane_ids {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.clear_pane_frame_color_override(pane_id, None);
                }
            }
            screen.render()?;
        },
        ScreenInstruction::HostTerminalFocusChanged(client_id, focused) => {
            screen.host_terminal_focus_changed(client_id, focused);
        },
        ScreenInstruction::SetClientTimezone(client_id, timezone) => {
            screen.set_client_timezone(client_id, timezone.as_deref());
        },
        ScreenInstruction::SetClientHostTerminalEnv(client_id, host_terminal_env) => {
            screen.set_client_host_terminal_env(client_id, host_terminal_env);
        },
        ScreenInstruction::ForwardDesktopNotifications {
            pane_id,
            notifications,
        } => {
            screen.forward_desktop_notifications(notifications, pane_id);
        },
        ScreenInstruction::SetTabBellFlash(tab_id, is_flashing) => {
            if let Some(tab) = screen.core.tabs.values_mut().find(|t| t.id == tab_id) {
                tab.tab_bell_flash = is_flashing;
                tab.clear_tab_bell_ring();
            }
            screen.render()?;
            screen.log_and_report_session_state()?;
        },
        ScreenInstruction::PreviousSwapLayout(client_id, _completion_tx) => {
            layout::previous_swap_layout(screen, client_id)?
        },
        ScreenInstruction::NextSwapLayout(client_id, _completion_tx) => {
            layout::next_swap_layout(screen, client_id)?
        },
        ScreenInstruction::OverrideLayout(
            cwd,
            default_shell,
            tab_layouts,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            apply_only_to_focused_tab,
            client_id,
            completion_tx,
        ) => layout::override_layout(
            screen,
            cwd,
            default_shell,
            tab_layouts,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            apply_only_to_focused_tab,
            client_id,
            completion_tx,
        )?,
        ScreenInstruction::OverrideLayoutComplete(
            tab_results,
            failed_tabs,
            tabs_to_close,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            client_id,
            completion_tx,
        ) => layout::override_layout_complete(
            state,
            screen,
            tab_results,
            failed_tabs,
            tabs_to_close,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            client_id,
            completion_tx,
        )?,
        ScreenInstruction::QueryTabNames(client_id, completion_tx) => {
            tabs::query_tab_names(screen, client_id, completion_tx)?
        },
        ScreenInstruction::NewTiledPluginPane(
            run_plugin,
            pane_title,
            skip_cache,
            cwd,
            client_id,
            completion_tx,
            explicit_tab_id,
            no_focus,
        ) => plugins::new_tiled_plugin_pane(
            screen,
            run_plugin,
            pane_title,
            skip_cache,
            cwd,
            client_id,
            completion_tx,
            explicit_tab_id,
            no_focus,
        )?,
        ScreenInstruction::NewFloatingPluginPane(
            run_plugin,
            pane_title,
            skip_cache,
            cwd,
            placement,
            client_id,
            completion_tx,
            explicit_tab_id,
            no_focus,
        ) => plugins::new_floating_plugin_pane(
            screen,
            run_plugin,
            pane_title,
            skip_cache,
            cwd,
            placement,
            client_id,
            completion_tx,
            explicit_tab_id,
            no_focus,
        )?,
        ScreenInstruction::NewInPlacePluginPane(
            run_plugin,
            pane_title,
            pane_id_to_replace,
            skip_cache,
            close_replaced_pane,
            client_id,
            completion_tx,
            explicit_tab_id,
            no_focus,
        ) => plugins::new_in_place_plugin_pane(
            screen,
            run_plugin,
            pane_title,
            pane_id_to_replace,
            skip_cache,
            close_replaced_pane,
            client_id,
            completion_tx,
            explicit_tab_id,
            no_focus,
        )?,
        ScreenInstruction::StartOrReloadPluginPane(
            run_plugin,
            pane_title,
            client_id,
            completion_tx,
        ) => plugins::start_or_reload_plugin_pane(
            screen,
            run_plugin,
            pane_title,
            client_id,
            completion_tx,
        )?,
        ScreenInstruction::AddPlugin(
            should_float,
            should_be_in_place,
            replaced,
            run_plugin_or_alias,
            pane_title,
            tab_index,
            plugin_id,
            pane_id_to_replace,
            cwd,
            start_suppressed,
            floating_pane_coordinates,
            should_focus_plugin,
            client_id,
            completion_tx,
            stated_overlay_placement,
        ) => plugins::add_plugin(
            state,
            screen,
            should_float,
            should_be_in_place,
            replaced,
            run_plugin_or_alias,
            pane_title,
            tab_index,
            plugin_id,
            pane_id_to_replace,
            cwd,
            start_suppressed,
            floating_pane_coordinates,
            should_focus_plugin,
            client_id,
            completion_tx,
            stated_overlay_placement,
        )?,
        ScreenInstruction::PluginPaneless {
            plugin_id,
            run,
            completion,
        } => {
            screen.plugin_paneless(plugin_id, *run, completion, &mut state.pending)?;
        },
        ScreenInstruction::PluginResolving {
            plugin_id,
            run,
            completion,
        } => {
            screen.plugin_resolving(plugin_id, *run, completion);
        },
        ScreenInstruction::PluginFailed(plugin_id) => {
            screen.plugin_failed(plugin_id);
        },
        ScreenInstruction::PluginRecovered(plugin_id) => {
            screen.core.failed_paneless_plugins.remove(&plugin_id);
        },
        // Here rather than in `Screen`: the key intercepts are this loop's.,
        ScreenInstruction::PluginUnloaded(plugin_id) => {
            screen.plugin_unloaded(plugin_id, &mut state.keybind_intercepts, &mut state.pending)?;
        },
        ScreenInstruction::GiveUpSlot(slot) => {
            screen.give_up_slot(slot, &mut state.pending)?;
        },
        ScreenInstruction::UpdatePluginLocation(pid, location) => {
            screen.update_plugin_location(pid, location);
            screen.log_and_report_session_state()?;
        },
        ScreenInstruction::UpdatePluginLoadingStage(pid, loading_indication) => {
            plugins::update_plugin_loading_stage(state, screen, pid, loading_indication)?
        },
        ScreenInstruction::StartPluginLoadingIndication(pid, loading_indication) => {
            plugins::start_plugin_loading_indication(state, screen, pid, loading_indication)?
        },
        ScreenInstruction::StartClientPluginLoadingIndication(
            pid,
            client_id,
            loading_indication,
        ) => plugins::start_client_plugin_loading_indication(
            state,
            screen,
            pid,
            client_id,
            loading_indication,
        )?,
        ScreenInstruction::StartPluginLoadingAnimation(pid) => {
            plugins::start_plugin_loading_animation(screen, pid)?
        },
        ScreenInstruction::StopPluginLoadingAnimation(pid) => {
            screen.core.animating_plugin_loads.remove(&pid);
            // so the clock stops on the spot when this was the last load, rather than at the
            // next tick a dot period later
            screen.sync_animation_clock(Instant::now())?;
            screen.render()?;
        },
        ScreenInstruction::RequestStateUpdateForPlugins => {
            let all_tabs = screen.get_tabs_mut();
            for tab in all_tabs.values_mut() {
                tab.update_input_modes()?;
            }
            screen.log_and_report_session_state()?;
            screen.render()?;
        },
        ScreenInstruction::LaunchOrFocusPlugin(
            run_plugin,
            should_float,
            move_to_focused_tab,
            should_open_in_place,
            close_replaced_pane,
            pane_id_to_replace,
            skip_cache,
            client_id,
            completion_tx,
            explicit_tab_id,
        ) => plugins::launch_or_focus_plugin(
            screen,
            run_plugin,
            should_float,
            move_to_focused_tab,
            should_open_in_place,
            close_replaced_pane,
            pane_id_to_replace,
            skip_cache,
            client_id,
            completion_tx,
            explicit_tab_id,
        )?,
        ScreenInstruction::LaunchPlugin(
            run_plugin,
            should_float,
            should_open_in_place,
            close_replaced_pane,
            pane_id_to_replace,
            skip_cache,
            cwd,
            client_id,
            completion_tx,
            explicit_tab_id,
            no_focus,
            overlay_placement,
        ) => plugins::launch_plugin(
            screen,
            run_plugin,
            should_float,
            should_open_in_place,
            close_replaced_pane,
            pane_id_to_replace,
            skip_cache,
            cwd,
            client_id,
            completion_tx,
            explicit_tab_id,
            no_focus,
            overlay_placement,
        )?,
        ScreenInstruction::SuppressPane(pane_id, client_id) => {
            panes::suppress_pane(screen, pane_id, client_id)?
        },
        ScreenInstruction::UnsuppressPane(pane_id, should_float_if_hidden) => {
            panes::unsuppress_pane(screen, pane_id, should_float_if_hidden)?
        },
        ScreenInstruction::UnsuppressOrExpandPane(pane_id, should_float_if_hidden) => {
            panes::unsuppress_or_expand_pane(screen, pane_id, should_float_if_hidden)?
        },
        ScreenInstruction::FocusPaneWithId(
            pane_id,
            should_float_if_hidden,
            should_be_in_place_if_hidden,
            client_id,
            completion_tx,
        ) => input::focus_pane_with_id(
            state,
            screen,
            pane_id,
            should_float_if_hidden,
            should_be_in_place_if_hidden,
            client_id,
            completion_tx,
        )?,
        ScreenInstruction::RenamePane(pane_id, new_name, _completion_tx) => {
            panes::rename_pane(screen, pane_id, new_name)?
        },
        ScreenInstruction::RenameActivePane(new_name, client_id, completion_tx) => {
            panes::rename_active_pane(screen, new_name, client_id, completion_tx)?
        },
        ScreenInstruction::RenameTab(tab_index, new_name, _completion_tx) => {
            tabs::rename_tab(screen, tab_index, new_name)?
        },
        ScreenInstruction::GoToTabWithId(tab_id, client_id, _completion_tx) => {
            tabs::go_to_tab_with_id(screen, tab_id, client_id)?
        },
        ScreenInstruction::RenameTabWithId(tab_id, new_name, _completion_tx) => {
            tabs::rename_tab_with_id(screen, tab_id, new_name)?
        },
        ScreenInstruction::ArrangeProjectPanes(groups, client_id) => {
            screen.arrange_project_panes(groups, client_id).non_fatal();
            screen.render()?;
        },
        ScreenInstruction::SetProjectContext(projects, panes, sources) => {
            panes::set_project_context(screen, projects, panes, sources)?
        },
        ScreenInstruction::SetTabProjects(tab_projects) => {
            tabs::set_tab_projects(screen, tab_projects)?
        },
        ScreenInstruction::CloseTabWithId(tab_id, _completion_tx) => {
            tabs::close_tab_with_id(state, screen, tab_id)?
        },
        ScreenInstruction::BreakPanesToTabWithId {
            pane_ids,
            tab_id,
            should_change_focus_to_target_tab,
            client_id,
            completion_tx,
        } => tabs::break_panes_to_tab_with_id(
            screen,
            pane_ids,
            tab_id,
            should_change_focus_to_target_tab,
            client_id,
            completion_tx,
        )?,
        ScreenInstruction::RequestPluginPermissions(plugin_id, plugin_permission) => {
            screen.open_permission_modal(plugin_id, plugin_permission);
            screen.render()?;
        },
        ScreenInstruction::CancelPluginPermission(token) => {
            screen.cancel_permission_modal(token);
            screen.render()?;
        },
        ScreenInstruction::WarpPane {
            supports_companions,
            supports_held,
            supports_tracking,
            pane_id,
            to_session,
            placement,
            client_id,
            response_channel,
            deadline,
        } => warp::warp_pane(
            screen,
            supports_companions,
            supports_held,
            supports_tracking,
            pane_id,
            to_session,
            placement,
            client_id,
            response_channel,
            deadline,
        )?,
        ScreenInstruction::PaneWarpFinished => {
            screen.warp.finish_pane_warp();
        },
        ScreenInstruction::AdoptedPaneFinished(finish) => {
            screen.finish_adopted_pane(finish);
        },
        ScreenInstruction::PaneReleasedForWarp {
            terminal_id,
            placement: _,
            to_session: _,
            client_id: _,
            released,
        } => {
            screen.pane_released_for_warp(terminal_id, released);
            screen.log_and_report_session_state().non_fatal();
            screen.render()?;
        },
        ScreenInstruction::WarpReplayFinished(id) => {
            screen
                .core
                .bus
                .senders
                .send_to_pty(PtyInstruction::WarpReplayFinished(id))
                .context("resume adopted terminal reader")
                .non_fatal();
        },
        ScreenInstruction::RequestPaneRedraw(terminal_id) => {
            if let Some(tab) = screen.tab_containing_pane_mut(PaneId::Terminal(terminal_id)) {
                tab.request_pane_redraw(terminal_id)
                    .context("failed to redraw adopted pane")
                    .non_fatal();
            }
        },
        ScreenInstruction::AdoptedPaneReady {
            hold,
            placed,
            terminal_id,
            payload,
            placement,
            client_id,
            cmd,
            buffered,
            child_exit_status,
        } => warp::adopted_pane_ready(
            screen,
            hold,
            placed,
            terminal_id,
            payload,
            placement,
            client_id,
            cmd,
            buffered,
            child_exit_status,
        )?,
        ScreenInstruction::WarpTab {
            supports_companions,
            supports_held,
            supports_tracking,
            tab_id,
            client_id,
            response_channel,
            deadline,
        } => warp::warp_tab(
            screen,
            supports_companions,
            supports_held,
            supports_tracking,
            tab_id,
            client_id,
            response_channel,
            deadline,
        )?,
        ScreenInstruction::TabReleasedForWarp {
            tab_index,
            released,
        } => {
            screen.tab_released_for_warp(tab_index, released);
            screen.log_and_report_session_state().non_fatal();
            screen.render()?;
        },
        ScreenInstruction::AdoptedTabReady {
            is_rollback,
            response_channel,
            hold,
            placement,
            payload,
            adopted,
            client_id,
        } => warp::adopted_tab_ready(
            screen,
            is_rollback,
            response_channel,
            hold,
            placement,
            *payload,
            *adopted,
            client_id,
        )?,
        ScreenInstruction::BreakPane(default_shell, client_id, _completion_tx) => {
            let default_layout = screen.core.default_layout.clone();
            screen.break_pane(default_shell, default_layout, client_id)?;
        },
        ScreenInstruction::BreakPaneRight(client_id, _completion_tx) => {
            screen.break_pane_to_new_tab(Direction::Right, client_id)?;
        },
        ScreenInstruction::BreakPaneLeft(client_id, _completion_tx) => {
            screen.break_pane_to_new_tab(Direction::Left, client_id)?;
        },
        ScreenInstruction::UpdateSessionInfos(new_session_infos, resurrectable_sessions) => {
            screen.update_session_infos(new_session_infos, resurrectable_sessions)?;
        },
        ScreenInstruction::UpdateAvailableLayouts {
            generation,
            layouts,
            errors,
        } => {
            screen.update_available_layouts(generation, layouts, errors);
        },
        ScreenInstruction::ReplacePane(
            new_pane_id,
            hold_for_command,
            pane_title,
            invoked_with,
            replaced,
            client_id_tab_index_or_pane_id,
            completion_tx,
        ) => panes::replace_pane(
            state,
            screen,
            new_pane_id,
            hold_for_command,
            pane_title,
            invoked_with,
            replaced,
            client_id_tab_index_or_pane_id,
            completion_tx,
        )?,
        ScreenInstruction::OpenPluginPaneInPlaceOfPaneId {
            pane_id,
            plugin_url,
            configuration,
            cwd,
            client_id,
            completion,
        } => plugins::open_plugin_pane_in_place_of_pane_id(
            screen,
            pane_id,
            plugin_url,
            configuration,
            cwd,
            client_id,
            completion,
        )?,
        ScreenInstruction::SerializeLayoutForResurrection => {
            if screen.core.session_serialization {
                screen.dump_layout_to_hd()?;
            }
        },
        ScreenInstruction::SaveSession(_client_id, completion_tx) => {
            screen.save_session(completion_tx)?;
        },
        ScreenInstruction::Notify(text, pane_id, on_click, completion) => {
            screen.receive_toast(text, pane_id, on_click, None, completion)?;
        },
        ScreenInstruction::PluginNotify {
            text,
            pane_id,
            on_click,
            plugin_id,
            completion,
        } => {
            screen.receive_toast(text, pane_id, on_click, Some(plugin_id), completion)?;
        },
        ScreenInstruction::NotificationPolicy(plugin_id, reason) => {
            screen.set_notification_policy(plugin_id, reason)?;
        },
        ScreenInstruction::NotificationPolicyUnavailable {
            plugin_id,
            reason,
            disable,
        } => {
            screen.notification_policy_unavailable(plugin_id, reason, disable)?;
        },
        ScreenInstruction::BindNotificationRequest {
            id,
            plugin_id,
            instance_id,
        } => {
            screen
                .notices
                .bind_notification_request(id, plugin_id, instance_id);
        },
        ScreenInstruction::ResolveNotification {
            id,
            decision,
            plugin_id,
            instance_id,
        } => {
            screen.resolve_notification(id, decision, plugin_id, instance_id)?;
        },
        ScreenInstruction::ContextMenuTimedOut(request_id, client_id) => {
            screen.context_menu_timed_out(request_id, client_id);
        },
        ScreenInstruction::SetContextMenu {
            request_id,
            items,
            plugin_id,
            client_id,
        } => {
            screen.set_context_menu_items(request_id, items, plugin_id, client_id);
        },
        ScreenInstruction::ActivateCommandPaletteAction {
            label,
            pane_id,
            tab_id,
            action,
            client_id,
        } => {
            screen.dispatch_command_palette_action(pane_id, tab_id, action, client_id, label);
            screen.render()?;
        },
        ScreenInstruction::MoveCommandPalettePane {
            pane_id,
            tab_id,
            direction,
            client_id,
        } => palette::move_command_palette_pane(screen, pane_id, tab_id, direction, client_id)?,
        ScreenInstruction::ActivateContextMenuItem {
            pane_id,
            actions,
            client_id,
        } => {
            screen.dispatch_context_menu_actions(pane_id, actions, client_id);
            screen.render()?;
        },
        ScreenInstruction::CloseContextMenu(client_id) => {
            if screen.close_context_menu(client_id) {
                screen.render()?;
            }
        },
        ScreenInstruction::DismissNotification(id) => {
            // the toast stays in the stack, sliding back out, until
            // `reap_settled_notifications` sees its reveal reach zero and actually removes it
            screen.dismiss_notification(id, Instant::now())?;
        },
        ScreenInstruction::AnimationTick => {
            screen.on_animation_tick(Instant::now())?;
        },
        ScreenInstruction::RenameSession(name, client_id, _completion_tx) => {
            screen.rename_session(name, client_id)?;
        },
        ScreenInstruction::Reconfigure {
            client_id,
            keybinds,
            config,
        } => {
            screen.reconfigure(config, keybinds, client_id).non_fatal();
        },
        ScreenInstruction::RerunCommandPane(terminal_pane_id, completion_tx) => {
            screen.rerun_command_pane_with_id(terminal_pane_id, completion_tx);
            screen.log_and_report_session_state()?;
            screen.render()?;
        },
        ScreenInstruction::RestartAgentPane {
            pane_id,
            prompt,
            session,
            originating_plugin,
            response,
        } => agents::restart_agent_pane(
            screen,
            pane_id,
            prompt,
            session,
            originating_plugin,
            response,
        )?,
        ScreenInstruction::RunCommandInPane(terminal_pane_id, run_command, keep_scrollback) => {
            panes::run_command_in_pane(screen, terminal_pane_id, run_command, keep_scrollback)?
        },
        ScreenInstruction::ResizePaneWithId(resize, pane_id) => {
            screen.resize_pane_with_id(resize, pane_id)?;
        },
        ScreenInstruction::EditScrollbackForPaneWithId(pane_id, completion_tx) => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.edit_scrollback_for_pane_with_id(pane_id, completion_tx)
                    .non_fatal();
            }
            screen.render()?;
        },
        ScreenInstruction::WriteToPaneId(bytes, pane_id, _completion) => {
            screen
                .write_to_pane_id(&None, bytes, false, pane_id, None, _completion)
                .non_fatal();
            screen.render()?;
        },
        ScreenInstruction::Paste(bytes, pane_id, client_id, _completion) => {
            input::paste(screen, bytes, pane_id, client_id, _completion)?
        },
        ScreenInstruction::ResolveAgentTab {
            placement,
            invoked_with,
            client_id,
            originating_pane,
            launch,
            reply,
        } => agents::resolve_agent_tab(
            screen,
            placement,
            invoked_with,
            client_id,
            originating_pane,
            launch,
            reply,
        )?,
        ScreenInstruction::CleanupAgentTab(placement, reply) => {
            let _ = reply.send(screen.cleanup_agent_tab(&placement));
        },
        ScreenInstruction::AgentSpawned(agent) => {
            screen.agents.register_spawn(agent);
        },
        ScreenInstruction::AgentControlReply(request, reply) => {
            screen.agent_control_with_reply(
                request,
                crate::screen::agent_control::Reply::Host(reply),
            );
        },
        ScreenInstruction::AgentControl(request, client) => {
            screen.agent_control(request, client);
        },
        ScreenInstruction::WebLiveAgents { reply } => {
            screen.web_live_agents(reply);
        },
        ScreenInstruction::WebFocusAgent {
            agent,
            owner,
            peer_instance,
            deadline,
            reply,
        } => {
            screen.web_focus_agent(agent, owner, peer_instance.as_deref(), deadline, reply);
        },
        ScreenInstruction::AgentReport(report, accepted_at) => {
            agents::agent_report(state, screen, report, accepted_at)?
        },
        ScreenInstruction::RestoreWarpedAgentInput(pane_id, input) => {
            agents::restore_warped_agent_input(state, screen, pane_id, input)?
        },
        ScreenInstruction::AgentWorktreeResolved {
            cwd,
            generation,
            root,
        } => {
            screen.agent_worktree_resolved(cwd, generation, root)?;
        },
        ScreenInstruction::SetPaneAgent(pane_id, agent, _completion) => {
            agents::set_pane_agent(state, screen, pane_id, agent, _completion)?
        },
        ScreenInstruction::SetPaneAgentActivity(pane_id, expected, state) => {
            screen.set_pane_agent_activity(pane_id, expected, state)?;
        },
        ScreenInstruction::ReconcileRestoredClaude {
            pane,
            generation,
            expected,
            input_revision,
            last_hook,
            state,
            reply,
        } => agents::reconcile_restored_claude(
            screen,
            pane,
            generation,
            expected,
            input_revision,
            last_hook,
            state,
            reply,
        )?,
        ScreenInstruction::SetPaneAgentRun(pane_id, run_id, _completion) => {
            agents::set_pane_agent_run(screen, pane_id, run_id)?
        },
        ScreenInstruction::SetQuitNotice(plugin_id, notice, guard) => {
            screen.set_quit_notice(plugin_id, notice, guard);
        },
        ScreenInstruction::ConfirmQuit(client_id, completion) => {
            screen.confirm_quit(client_id, false, completion)?;
        },
        ScreenInstruction::SetPaneColor(pane_id, fg, bg, _completion) => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.set_pane_color(pane_id, fg, bg).non_fatal();
            }
            screen.render()?;
        },
        ScreenInstruction::WriteKeyToPaneId(
            key_with_modifier,
            bytes,
            is_kitty,
            pane_id,
            _completion,
        ) => input::write_key_to_pane_id(
            screen,
            key_with_modifier,
            bytes,
            is_kitty,
            pane_id,
            _completion,
        )?,
        ScreenInstruction::CopyTextToClipboard(text, plugin_id) => {
            input::copy_text_to_clipboard(screen, text, plugin_id)?
        },
        ScreenInstruction::MovePaneWithPaneId(pane_id) => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.move_pane(pane_id);
            }
            screen.render()?;
        },
        ScreenInstruction::MovePaneWithPaneIdInDirection(pane_id, direction) => {
            panes::move_pane_with_pane_id_in_direction(screen, pane_id, direction)?
        },
        ScreenInstruction::ClearScreenForPaneId(pane_id) => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.clear_screen_for_pane_id(pane_id);
            }
            screen.render()?;
        },
        ScreenInstruction::ScrollUpInPaneId(pane_id) => {
            input::scroll_up_in_pane_id(screen, pane_id)?
        },
        ScreenInstruction::ScrollDownInPaneId(pane_id) => {
            input::scroll_down_in_pane_id(screen, pane_id)?
        },
        ScreenInstruction::ScrollToTopInPaneId(pane_id) => {
            input::scroll_to_top_in_pane_id(screen, pane_id)?
        },
        ScreenInstruction::ScrollToBottomInPaneId(pane_id) => {
            input::scroll_to_bottom_in_pane_id(screen, pane_id)?
        },
        ScreenInstruction::PageScrollUpInPaneId(pane_id) => {
            input::page_scroll_up_in_pane_id(screen, pane_id)?
        },
        ScreenInstruction::PageScrollDownInPaneId(pane_id) => {
            input::page_scroll_down_in_pane_id(screen, pane_id)?
        },
        ScreenInstruction::TogglePaneIdFullscreen(pane_id) => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.toggle_pane_fullscreen(pane_id);
            }
            // Rendering no longer reconciles nested guests, so a zoomed guest
            // must learn here that its host left fullscreen.
            screen.sync_nested_guest_fullscreen_state();
            screen.render()?;
        },
        ScreenInstruction::TogglePaneEmbedOrEjectForPaneId(pane_id) => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, None)
                    .non_fatal();
            }
            screen.sync_nested_guest_fullscreen_state();
            screen.render()?;
        },
        ScreenInstruction::CloseTabWithIndex(tab_index) => {
            tabs::close_tab_with_index(screen, tab_index)?
        },
        ScreenInstruction::BreakPanesToNewTab {
            pane_ids,
            default_shell,
            should_change_focus_to_new_tab,
            new_tab_name,
            client_id,
            completion_tx,
        } => tabs::break_panes_to_new_tab(
            screen,
            pane_ids,
            default_shell,
            should_change_focus_to_new_tab,
            new_tab_name,
            client_id,
            completion_tx,
        )?,
        ScreenInstruction::BreakPanesToTabWithIndex {
            pane_ids,
            tab_index,
            should_change_focus_to_new_tab,
            client_id,
            completion_tx,
        } => tabs::break_panes_to_tab_with_index(
            screen,
            pane_ids,
            tab_index,
            should_change_focus_to_new_tab,
            client_id,
            completion_tx,
        )?,
        ScreenInstruction::TogglePanePinned(client_id, _completion_tx) => {
            screen.toggle_pane_pinned(client_id);
        },
        ScreenInstruction::SetFloatingPanePinned(pane_id, should_be_pinned) => {
            screen.set_floating_pane_pinned(pane_id, should_be_pinned);
        },
        ScreenInstruction::StackPanes(pane_ids_to_stack, client_id, _completion_tx) => {
            panes::stack_panes(screen, pane_ids_to_stack, client_id)?
        },
        ScreenInstruction::ChangeFloatingPanesCoordinates(
            pane_ids_and_coordinates,
            _completion_tx,
        ) => {
            screen.change_floating_panes_coordinates(pane_ids_and_coordinates);
            let _ = screen.render();
        },
        ScreenInstruction::PaneTabsAction(step, client, _completion) => {
            screen.pane_tabs_action(step, client)?;
            screen.render()?;
        },
        ScreenInstruction::PaneTabs {
            command,
            client_id,
            response,
        } => {
            let result = screen
                .pane_tabs_command(command, client_id)
                .map_err(|error| error.to_string());
            let _ = response.send(result);
            screen.render()?;
        },
        ScreenInstruction::OverlayTabCwd {
            id,
            cwd,
            caller_cwd,
            caller_terminal_pane_id,
        } => {
            screen.overlay_tab_cwd(id, cwd, caller_cwd, caller_terminal_pane_id)?;
        },
        ScreenInstruction::PaneTabLoaded {
            id,
            index,
            generation,
            result,
            client_id: _,
        } => tabs::pane_tab_loaded(state, screen, id, index, generation, result)?,
        ScreenInstruction::Multipane {
            operation,
            client_id,
            response,
        } => {
            let result = screen
                .multipane_command(operation, client_id)
                .map_err(|error| error.to_string());
            let _ = response.send(result);
            screen.log_and_report_session_state().non_fatal();
            screen.render()?;
        },
        ScreenInstruction::ChangeOverlayCoordinates(
            pane_ids_and_coordinates,
            override_user_placement,
            _completion_tx,
        ) => panes::change_overlay_coordinates(
            state,
            screen,
            pane_ids_and_coordinates,
            override_user_placement,
        )?,
        ScreenInstruction::TogglePaneBorderless(pane_id, _completion_tx) => {
            screen.toggle_pane_borderless(pane_id);
            let _ = screen.render();
        },
        ScreenInstruction::SetPaneBorderless(pane_id, borderless, _completion_tx) => {
            screen.set_pane_borderless(pane_id, borderless);
            let _ = screen.render();
        },
        ScreenInstruction::GroupAndUngroupPanes(
            pane_ids_to_group,
            pane_ids_to_ungroup,
            for_all_clients,
            client_id,
        ) => panes::group_and_ungroup_panes(
            screen,
            pane_ids_to_group,
            pane_ids_to_ungroup,
            for_all_clients,
            client_id,
        )?,
        ScreenInstruction::TogglePaneInGroup(client_id, _completion_tx) => {
            screen.toggle_pane_in_group(client_id).non_fatal();
        },
        ScreenInstruction::ToggleGroupMarking(client_id, _completion_tx) => {
            screen.toggle_group_marking(client_id).non_fatal();
        },
        ScreenInstruction::HighlightAndUnhighlightPanes(
            pane_ids_to_highlight,
            pane_ids_to_unhighlight,
            client_id,
        ) => panes::highlight_and_unhighlight_panes(
            screen,
            pane_ids_to_highlight,
            pane_ids_to_unhighlight,
            client_id,
        )?,
        ScreenInstruction::RevealPaneWithId(pane_id, label, client_id) => {
            screen.reveal_pane_with_id(pane_id, label, client_id)?;
        },
        ScreenInstruction::FloatMultiplePanes(pane_ids_to_float, client_id) => {
            panes::float_multiple_panes(screen, pane_ids_to_float, client_id)?
        },
        ScreenInstruction::EmbedMultiplePanes(pane_ids_to_float, client_id) => {
            panes::embed_multiple_panes(screen, pane_ids_to_float, client_id)?
        },
        ScreenInstruction::InterceptKeyPresses(plugin_id, client_id) => {
            state.keybind_intercepts.insert(client_id, plugin_id);
        },
        ScreenInstruction::ClearKeyPressesIntercepts(plugin_id, client_id) => {
            if state.keybind_intercepts.get(&client_id) == Some(&plugin_id) {
                state.keybind_intercepts.remove(&client_id);
            }
        },
        ScreenInstruction::ReplacePaneWithExistingPane(
            old_pane_id,
            new_pane_id,
            suppress_replaced_pane,
            completion_tx,
        ) => screen
            .replace_pane_with_existing_pane(
                old_pane_id,
                new_pane_id,
                suppress_replaced_pane,
                completion_tx,
            )
            .non_fatal(),
        ScreenInstruction::AddWatcherClient(client_id, size) => {
            screen
                .add_watcher_client(client_id)
                .context("failed to add watcher client")?;
            screen.set_watcher_size(client_id, size);
            screen.render()?;
        },
        ScreenInstruction::RemoveWatcherClient(client_id) => {
            clients::remove_watcher_client(state, screen, client_id)?
        },
        ScreenInstruction::SetFollowedClient(client_id) => {
            clients::set_followed_client(state, screen, client_id)?
        },
        ScreenInstruction::WatcherTerminalResize(client_id, size) => {
            screen.set_watcher_size(client_id, size);
            screen.render()?;
        },
        ScreenInstruction::ClearMouseHelpText(client_id) => {
            if let Ok(tab) = screen.get_active_tab_mut(client_id) {
                tab.clear_mouse_help_text(client_id);
                screen.render()?;
            }
        },
        ScreenInstruction::SetPluginRegexHighlights {
            pane_id,
            plugin_id,
            highlights,
        } => plugins::set_plugin_regex_highlights(screen, pane_id, plugin_id, highlights)?,
        ScreenInstruction::ClearPluginHighlights { pane_id, plugin_id } => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.clear_plugin_highlights_for_pane(pane_id, plugin_id);
            }
            screen.render()?;
        },
        ScreenInstruction::ClearAllPluginHighlights(plugin_id) => {
            let all_tabs = screen.get_tabs_mut();
            for tab in all_tabs.values_mut() {
                tab.clear_all_plugin_highlights(plugin_id);
            }
            screen.render()?;
        },
        ScreenInstruction::DesktopNotificationResponse(raw_bytes, client_id) => {
            notifications::desktop_notification_response(screen, raw_bytes, client_id)?
        },
        ScreenInstruction::SubscribeToPaneRenders {
            client_id,
            pane_ids,
            scrollback,
            ansi,
        } => {
            screen.subscribe_to_pane_renders(client_id, pane_ids, scrollback, ansi);
        },
        ScreenInstruction::NotifyPaneClosedToSubscribers { pane_id } => {
            screen.cleanup_closed_pane(pane_id);
            state.pending.waiting_for_pane.remove(&pane_id);
            screen.notify_pane_closed_to_subscribers(pane_id);
        },
        ScreenInstruction::InvalidateAgentEventProjects => {
            screen.invalidate_agent_event_projects();
            screen.render()?;
        },
        ScreenInstruction::AgentEventProjects(version, result) => {
            screen.apply_agent_event_projects(version, result);
        },
        ScreenInstruction::SubscribeAgentEvents(client, options) => {
            screen.subscribe_agent_events(client, options);
        },
        ScreenInstruction::SetCliEventSubscribersPresent(present) => {
            screen.core.cli_event_subscribers_present = present;
            if present {
                // A new subscriber gets the state as it stands, not the next change to it.
                screen.log_and_report_session_state()?;
            }
        },
        ScreenInstruction::UpdateBackgroundPluginSubscriptions(
            plugin_id,
            client_id,
            subscriptions,
        ) => plugins::update_background_plugin_subscriptions(
            screen,
            plugin_id,
            client_id,
            subscriptions,
        )?,
        ScreenInstruction::ClearHintTextCache => {
            for tab in screen.core.tabs.values_mut() {
                tab.clear_hint_text_cache();
            }
            screen.render()?;
        },
        ScreenInstruction::BroadcastModeUpdate(mode_info, target_client_id) => {
            screen.broadcast_mode_update(mode_info, target_client_id)?;
        },
        // Pane-targeting CLI handlers,
        ScreenInstruction::ScrollUpWithPaneId(pane_id, _completion_tx) => {
            input::scroll_up_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::ScrollDownWithPaneId(pane_id, _completion_tx) => {
            input::scroll_down_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::ScrollToTopWithPaneId(pane_id, _completion_tx) => {
            input::scroll_to_top_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::ScrollToBottomWithPaneId(pane_id, _completion_tx) => {
            input::scroll_to_bottom_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::PageScrollUpWithPaneId(pane_id, _completion_tx) => {
            input::page_scroll_up_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::PageScrollDownWithPaneId(pane_id, _completion_tx) => {
            input::page_scroll_down_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::HalfPageScrollUpWithPaneId(pane_id, _completion_tx) => {
            input::half_page_scroll_up_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::HalfPageScrollDownWithPaneId(pane_id, _completion_tx) => {
            input::half_page_scroll_down_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::ResizeWithPaneId(pane_id, strategy, _completion_tx) => {
            layout::resize_with_pane_id(screen, pane_id, strategy, _completion_tx)?
        },
        ScreenInstruction::MovePaneWithPaneIdCli(pane_id, direction, _completion_tx) => {
            panes::move_pane_with_pane_id_cli(screen, pane_id, direction, _completion_tx)?
        },
        ScreenInstruction::MovePaneBackwardsWithPaneId(pane_id, _completion_tx) => {
            panes::move_pane_backwards_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::ClearScreenWithPaneId(pane_id, mut _completion_tx) => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.clear_screen_by_pane_id(pane_id).non_fatal();
            } else {
                Screen::report_pane_not_found(pane_id, &mut _completion_tx);
            }
            screen.render()?;
        },
        ScreenInstruction::EditScrollbackWithPaneId(pane_id, ansi, completion_tx) => {
            input::edit_scrollback_with_pane_id(screen, pane_id, ansi, completion_tx)?
        },
        ScreenInstruction::ToggleFullscreenWithPaneId(pane_id, _completion_tx) => {
            panes::toggle_fullscreen_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::ToggleNoUiFullscreenWithPaneId(pane_id, _completion_tx) => {
            panes::toggle_no_ui_fullscreen_with_pane_id(screen, pane_id, _completion_tx)?
        },
        ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(pane_id, completion_tx) => {
            panes::toggle_pane_embed_or_floating_with_pane_id(screen, pane_id, completion_tx)?
        },
        ScreenInstruction::CloseFocusWithPaneId(pane_id, completion_tx) => {
            input::close_focus_with_pane_id(screen, pane_id, completion_tx)?
        },
        ScreenInstruction::RenamePaneWithPaneId(pane_id, name, _completion_tx) => {
            panes::rename_pane_with_pane_id(screen, pane_id, name, _completion_tx)?
        },
        ScreenInstruction::UndoRenamePaneWithPaneId(pane_id, mut _completion_tx) => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.undo_rename_pane_by_pane_id(pane_id);
            } else {
                Screen::report_pane_not_found(pane_id, &mut _completion_tx);
            }
            screen.render()?;
        },
        ScreenInstruction::TogglePanePinnedWithPaneId(pane_id, mut _completion_tx) => {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.toggle_pane_pinned_by_pane_id(pane_id);
            } else {
                Screen::report_pane_not_found(pane_id, &mut _completion_tx);
            }
        },
        // Tab-targeting CLI handlers,
        ScreenInstruction::UndoRenameTabWithTabId(tab_id, _completion_tx) => {
            tabs::undo_rename_tab_with_tab_id(screen, tab_id, _completion_tx)?
        },
        ScreenInstruction::ToggleActiveSyncTabWithTabId(tab_id, _completion_tx) => {
            tabs::toggle_active_sync_tab_with_tab_id(screen, tab_id, _completion_tx)?
        },
        ScreenInstruction::ToggleFloatingPanesWithTabId(tab_id, default_shell, completion_tx) => {
            tabs::toggle_floating_panes_with_tab_id(screen, tab_id, default_shell, completion_tx)?
        },
        ScreenInstruction::PreviousSwapLayoutWithTabId(tab_id, _completion_tx) => {
            layout::previous_swap_layout_with_tab_id(screen, tab_id, _completion_tx)?
        },
        ScreenInstruction::NextSwapLayoutWithTabId(tab_id, _completion_tx) => {
            layout::next_swap_layout_with_tab_id(screen, tab_id, _completion_tx)?
        },
        ScreenInstruction::MoveTabWithTabId(tab_id, direction, _completion_tx) => {
            tabs::move_tab_with_tab_id(state, screen, tab_id, direction, _completion_tx)?
        },
        ScreenInstruction::FocusHostSession(client_id, _completion_tx) => {
            nested::focus_host_session(screen, client_id)?
        },
        ScreenInstruction::FocusGuestSession(client_id, _completion_tx) => {
            screen.focus_guest_session(client_id);
        },
        ScreenInstruction::ToggleHostFullscreen(client_id, _completion_tx) => {
            nested::toggle_host_fullscreen(screen, client_id)?
        },
    }
    Ok(std::ops::ControlFlow::Continue(()))
}
