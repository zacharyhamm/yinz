use super::{format_guest_modal_shortcut, GuestModalOutcome, NestedGuestChoice, Screen};
use crate::background_jobs::BackgroundJob;
use crate::panes::nested_session_modal::GuestModalShortcuts;
use crate::panes::PaneId;
use crate::pty_writer::PtyWriteInstruction;
use crate::tab::GuestChoiceIndicator;
use crate::TerminalId;
use crate::{ClientId, ServerInstruction};
use std::collections::HashMap;
use std::time::Instant;
use yinz_ipc::nested_session;
use yinz_types::geometry::Direction;
use yinz_types::input::actions::Action;
use yinz_types::input::keybinds::shortcut_for_action;
use yinz_types::input::options::NestedSessionHandling;
use yinz_types::internal_ids::SessionName;
use yinz_types::keys::KeyWithModifier;
use yinz_types::style::HostTerminalThemeMode;
use {
    yinz_ipc::nested_session::NestedSessionCapability,
    yinz_ipc::nested_session::NestedSessionMessage,
};
use {yinz_types::data::InputMode, yinz_types::data::ModeInfo, yinz_types::data::PaletteColor};

impl Screen {
    pub fn handle_nested_session_message_from_pane(
        &mut self,
        pane_id: PaneId,
        message: NestedSessionMessage,
    ) {
        match message {
            NestedSessionMessage::Announce { session_name, .. } => {
                self.handle_nested_guest_announce(pane_id, session_name);
            },
            NestedSessionMessage::Pong => {
                self.nested_guest_tracker.on_pong(pane_id, Instant::now());
                log::debug!("nested session guest in pane {:?} ponged", pane_id);
            },
            NestedSessionMessage::Bye => {
                log::info!(
                    "nested session guest in pane {:?} sent bye, clearing guest flag",
                    pane_id
                );
                self.clear_nested_guest(pane_id);
            },
            NestedSessionMessage::FocusHost { direction } => {
                let clients_focused_on_pane = self.clients_with_pane_focused(pane_id);
                match direction {
                    None => {
                        self.unset_nested_fullscreen_if_active(pane_id);
                        for client_id in clients_focused_on_pane {
                            self.nested_guest_choices
                                .insert((client_id, pane_id), NestedGuestChoice::Dismissed);
                            self.sync_guest_choice_indicator(client_id, pane_id);
                            self.set_client_dimmed(client_id, false, None);
                            let _ = self.bus.senders.send_to_server(
                                ServerInstruction::KeyPassthroughChanged(
                                    client_id, pane_id, pane_id, false, None, true,
                                ),
                            );
                        }
                        let _ = self.render();
                    },
                    Some(direction) => {
                        for client_id in clients_focused_on_pane {
                            let Some(tab) = self.tab_containing_pane_mut(pane_id) else {
                                // the guest pane is gone: nowhere to land, hand back to the host
                                self.bubble_focus_to_host(client_id, direction);
                                continue;
                            };
                            // a modal owns this client's keyboard: the hand-back is a no-op,
                            // not a hand-off to the host session
                            if !tab.lower_layer_may_take_focus(client_id) {
                                continue;
                            }
                            match tab.focus_pane_adjacent_to(pane_id, direction, client_id) {
                                Some(new_pane_id) => {
                                    self.report_key_passthrough_state_with_direction(
                                        client_id,
                                        pane_id,
                                        new_pane_id,
                                        Some(direction),
                                    );
                                    let _ = self.render();
                                    let _ = self.log_and_report_session_state();
                                },
                                None => {
                                    self.bubble_focus_to_host(client_id, direction);
                                },
                            }
                        }
                    },
                }
            },
            NestedSessionMessage::ShortcutUpdate { ascend_keys, .. } => {
                if !self.nested_guest_tracker.is_tracked(pane_id) {
                    log::debug!(
                        "ignoring nested shortcut update from non-live guest pane {:?}",
                        pane_id
                    );
                    return;
                }
                self.guest_ascend_keys.insert(pane_id, ascend_keys);
                self.refresh_nested_ascend_keys_for_pane(pane_id);
            },
            NestedSessionMessage::ToggleHostFullscreen { fullscreen } => {
                if !self.nested_guest_tracker.is_tracked(pane_id) {
                    log::debug!(
                        "ignoring nested fullscreen request from non-live guest pane {:?}",
                        pane_id
                    );
                    return;
                }
                self.apply_nested_guest_fullscreen(pane_id, fullscreen);
                if let Some(client_id) = self.nested_via_client_id {
                    let payload = nested_session::encode_payload(
                        &NestedSessionMessage::ToggleHostFullscreen { fullscreen },
                    );
                    let _ = self.bus.senders.send_to_server(
                        ServerInstruction::EmitNestedSessionFrameToClient(client_id, payload),
                    );
                }
                let _ = self.render();
                let _ = self.log_and_report_session_state();
            },
            other => {
                log::debug!(
                    "dropping unsupported nested session message from pane {:?}: {:?}",
                    pane_id,
                    other
                );
            },
        }
    }

    fn handle_nested_guest_announce(&mut self, pane_id: PaneId, guest_session_name: SessionName) {
        use crate::nested_guest::AnnounceKind;
        let terminal_id = match pane_id {
            PaneId::Terminal(terminal_id) => terminal_id,
            PaneId::Plugin(_) => return,
        };
        let announce_kind = self
            .nested_guest_tracker
            .on_announce(pane_id, Instant::now());
        let is_fresh = announce_kind == AnnounceKind::New;
        if is_fresh {
            self.nested_guest_choices
                .retain(|(_, choice_pane_id), _| *choice_pane_id != pane_id);
        }
        let connected_client_ids: Vec<ClientId> =
            self.connected_clients.borrow().iter().copied().collect();
        let guest_modal_shortcuts = self.guest_modal_shortcuts();
        let should_ask = self.nested_session_handling == NestedSessionHandling::Ask;
        match self.tab_containing_pane_mut(pane_id) {
            Some(tab) => {
                tab.set_pane_is_nested_guest(pane_id, true);
                tab.set_guest_session_name_on_pane(pane_id, Some(guest_session_name.clone()));
                tab.set_guest_modal_shortcuts_on_pane(pane_id, guest_modal_shortcuts);
                if is_fresh {
                    tab.clear_all_guest_choice_indicators_on_pane(pane_id);
                    if should_ask {
                        tab.set_guest_modal_on_pane(pane_id, &connected_client_ids);
                    }
                }
            },
            None => {
                self.nested_guest_tracker.remove(pane_id);
                return;
            },
        }
        let mut ancestry = self.nested_ancestry.clone();
        ancestry.push(self.session_name.clone());
        let announce_ack = NestedSessionMessage::AnnounceAck {
            ancestry,
            capabilities: vec![NestedSessionCapability::NestedControl],
            descend_keys: self.own_descend_shortcut(),
        };
        let _ = self
            .bus
            .senders
            .send_to_pty_writer(PtyWriteInstruction::Write(
                nested_session::encode_frame(&announce_ack),
                terminal_id,
                None,
            ));
        let _ = self
            .bus
            .senders
            .send_to_background_jobs(BackgroundJob::StartNestedGuestPing(pane_id));
        if is_fresh {
            self.auto_apply_nested_session_handling(pane_id, &connected_client_ids);
        } else {
            self.revive_nested_guest(pane_id);
        }
        let _ = self.render();
        log::info!(
            "nested session handshake ({:?}): guest session {:?} announced in pane {:?}, sent announce_ack",
            announce_kind,
            guest_session_name,
            pane_id
        );
    }

    pub(super) fn suspend_nested_guest(&mut self, pane_id: PaneId) {
        let _ = self
            .bus
            .senders
            .send_to_background_jobs(BackgroundJob::StopNestedGuestPing(pane_id));
        self.unset_nested_fullscreen_if_active(pane_id);
        self.reported_guest_fullscreen.remove(&pane_id);
        let affected_clients: Vec<ClientId> = self
            .clients_with_pane_focused(pane_id)
            .into_iter()
            .chain(self.dimmed_clients.iter().copied())
            .collect();
        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
            tab.clear_all_guest_modals_on_pane(pane_id);
        }
        for client_id in affected_clients {
            self.set_client_dimmed(client_id, false, None);
            let _ = self
                .bus
                .senders
                .send_to_server(ServerInstruction::KeyPassthroughChanged(
                    client_id, pane_id, pane_id, false, None, false,
                ));
        }
    }

    fn revive_nested_guest(&mut self, pane_id: PaneId) {
        self.broadcast_ancestry_update_to_guests();
        self.sync_nested_guest_fullscreen_state();
        let client_ids: Vec<ClientId> = self.connected_clients.borrow().iter().copied().collect();
        for client_id in client_ids {
            self.sync_guest_choice_indicator(client_id, pane_id);
            if self.get_active_pane_id(&client_id) == Some(pane_id) {
                self.report_key_passthrough_state(client_id, pane_id, pane_id);
            }
        }
    }

    pub(super) fn handle_guest_modal_choice(
        &mut self,
        client_id: ClientId,
        pane_id: PaneId,
        outcome: GuestModalOutcome,
    ) {
        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
            tab.clear_guest_modal_on_pane(pane_id, client_id);
        }
        match outcome {
            GuestModalOutcome::Descend => {
                self.nested_guest_choices
                    .insert((client_id, pane_id), NestedGuestChoice::Descend);
                self.sync_guest_choice_indicator(client_id, pane_id);
                if self.get_active_pane_id(&client_id) == Some(pane_id) {
                    self.report_key_passthrough_state(client_id, pane_id, pane_id);
                }
                let _ = self.render();
                let _ = self.log_and_report_session_state();
            },
            GuestModalOutcome::Zoom => {
                self.nested_guest_choices
                    .insert((client_id, pane_id), NestedGuestChoice::Zoom);
                self.sync_guest_choice_indicator(client_id, pane_id);
                if self.get_active_pane_id(&client_id) == Some(pane_id) {
                    self.report_key_passthrough_state(client_id, pane_id, pane_id);
                }
                self.apply_nested_guest_fullscreen(pane_id, true);
                self.sync_nested_guest_fullscreen_state();
                if let Some(via_client_id) = self.nested_via_client_id {
                    let payload = nested_session::encode_payload(
                        &NestedSessionMessage::ToggleHostFullscreen { fullscreen: true },
                    );
                    let _ = self.bus.senders.send_to_server(
                        ServerInstruction::EmitNestedSessionFrameToClient(via_client_id, payload),
                    );
                }
                let _ = self.render();
                let _ = self.log_and_report_session_state();
            },
        }
    }

    fn record_nested_guest_choice_for_client(
        &mut self,
        client_id: ClientId,
        pane_id: PaneId,
        choice: NestedGuestChoice,
    ) {
        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
            tab.clear_guest_modal_on_pane(pane_id, client_id);
        }
        self.nested_guest_choices
            .insert((client_id, pane_id), choice);
        self.sync_guest_choice_indicator(client_id, pane_id);
        if self.get_active_pane_id(&client_id) == Some(pane_id) {
            self.report_key_passthrough_state(client_id, pane_id, pane_id);
        }
    }

    fn auto_apply_nested_session_handling(&mut self, pane_id: PaneId, client_ids: &[ClientId]) {
        match self.nested_session_handling {
            NestedSessionHandling::Descend => {
                for client_id in client_ids {
                    self.record_nested_guest_choice_for_client(
                        *client_id,
                        pane_id,
                        NestedGuestChoice::Descend,
                    );
                }
            },
            NestedSessionHandling::Fullscreen => {
                for client_id in client_ids {
                    self.record_nested_guest_choice_for_client(
                        *client_id,
                        pane_id,
                        NestedGuestChoice::Zoom,
                    );
                }
                self.apply_nested_guest_fullscreen(pane_id, true);
                self.sync_nested_guest_fullscreen_state();
                if let Some(via_client_id) = self.nested_via_client_id {
                    let payload = nested_session::encode_payload(
                        &NestedSessionMessage::ToggleHostFullscreen { fullscreen: true },
                    );
                    let _ = self.bus.senders.send_to_server(
                        ServerInstruction::EmitNestedSessionFrameToClient(via_client_id, payload),
                    );
                }
            },
            NestedSessionHandling::Ask | NestedSessionHandling::Never => {},
        }
    }

    pub(super) fn focus_guest_session(&mut self, client_id: ClientId) {
        let focused_pane_id = match self.get_active_pane_id(&client_id) {
            Some(pane_id) => pane_id,
            None => return,
        };
        let is_live_guest = self
            .tab_containing_pane(focused_pane_id)
            .is_some_and(|tab| tab.is_pane_nested_guest(focused_pane_id))
            && self.nested_guest_tracker.is_tracked(focused_pane_id);
        if !is_live_guest {
            return;
        }
        self.nested_guest_choices
            .insert((client_id, focused_pane_id), NestedGuestChoice::Descend);
        if let Some(tab) = self.tab_containing_pane_mut(focused_pane_id) {
            tab.clear_guest_modal_on_pane(focused_pane_id, client_id);
        }
        self.sync_guest_choice_indicator(client_id, focused_pane_id);
        self.report_key_passthrough_state(client_id, focused_pane_id, focused_pane_id);
        let _ = self.render();
        let _ = self.log_and_report_session_state();
    }

    pub fn clear_nested_guest(&mut self, pane_id: PaneId) {
        self.nested_guest_tracker.remove(pane_id);
        self.guest_ascend_keys.remove(&pane_id);
        let _ = self
            .bus
            .senders
            .send_to_background_jobs(BackgroundJob::StopNestedGuestPing(pane_id));
        self.unset_nested_fullscreen_if_active(pane_id);
        self.reported_guest_fullscreen.remove(&pane_id);
        let mut affected_clients: Vec<ClientId> = self
            .nested_guest_choices
            .iter()
            .filter(|((_, choice_pane_id), choice)| {
                *choice_pane_id == pane_id && choice.enters_passthrough()
            })
            .map(|((choice_client_id, _), _)| *choice_client_id)
            .collect();
        self.nested_guest_choices
            .retain(|(_, choice_pane_id), _| *choice_pane_id != pane_id);
        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
            tab.set_pane_is_nested_guest(pane_id, false);
            tab.clear_all_guest_modals_on_pane(pane_id);
            tab.clear_all_guest_choice_indicators_on_pane(pane_id);
        }

        for client_id in self.clients_with_pane_focused(pane_id) {
            if !affected_clients.contains(&client_id) {
                affected_clients.push(client_id);
            }
        }
        for client_id in affected_clients {
            self.set_client_dimmed(client_id, false, None);
            let _ = self
                .bus
                .senders
                .send_to_server(ServerInstruction::KeyPassthroughChanged(
                    client_id, pane_id, pane_id, false, None, false,
                ));
        }
    }

    pub(super) fn show_guest_modals_for_new_client(&mut self, client_id: ClientId) {
        let mut live_guest_panes = vec![];
        for tab in self.tabs.values() {
            for pane_id in tab.nested_guest_pane_ids() {
                if self.nested_guest_tracker.is_tracked(pane_id) {
                    live_guest_panes.push(pane_id);
                }
            }
        }
        for pane_id in live_guest_panes {
            if self
                .nested_guest_choices
                .contains_key(&(client_id, pane_id))
            {
                continue;
            }
            match self.nested_session_handling {
                NestedSessionHandling::Ask => {
                    if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
                        tab.set_guest_modal_on_pane(pane_id, &[client_id]);
                    }
                },
                NestedSessionHandling::Descend => {
                    self.record_nested_guest_choice_for_client(
                        client_id,
                        pane_id,
                        NestedGuestChoice::Descend,
                    );
                },
                NestedSessionHandling::Fullscreen => {
                    self.record_nested_guest_choice_for_client(
                        client_id,
                        pane_id,
                        NestedGuestChoice::Zoom,
                    );
                },
                NestedSessionHandling::Never => {},
            }
        }
    }

    pub(super) fn should_route_keys_to_pane(&self, client_id: ClientId, pane_id: PaneId) -> bool {
        let Some(tab) = self.tab_containing_pane(pane_id) else {
            return false;
        };
        let is_guest = tab.is_pane_nested_guest(pane_id);
        let is_tracked = self.nested_guest_tracker.is_tracked(pane_id);
        let has_descended = self
            .nested_guest_choices
            .get(&(client_id, pane_id))
            .map(|choice| choice.enters_passthrough())
            .unwrap_or(false);
        is_guest && is_tracked && has_descended
    }

    fn downgrade_zoom_choice_to_descend(&mut self, pane_id: PaneId) {
        let mut affected_clients = vec![];
        for ((client_id, choice_pane_id), choice) in self.nested_guest_choices.iter_mut() {
            if *choice_pane_id == pane_id && *choice == NestedGuestChoice::Zoom {
                *choice = NestedGuestChoice::Descend;
                affected_clients.push(*client_id);
            }
        }
        for client_id in affected_clients {
            self.sync_guest_choice_indicator(client_id, pane_id);
        }
    }

    fn guest_choice_indicator_for(
        &self,
        client_id: ClientId,
        pane_id: PaneId,
    ) -> Option<GuestChoiceIndicator> {
        match self.nested_guest_choices.get(&(client_id, pane_id)) {
            Some(NestedGuestChoice::Descend) => Some(GuestChoiceIndicator::Descended),
            Some(NestedGuestChoice::Dismissed) => Some(GuestChoiceIndicator::Dismissed),
            Some(NestedGuestChoice::Zoom) | None => None,
        }
    }

    fn sync_guest_choice_indicator(&mut self, client_id: ClientId, pane_id: PaneId) {
        let indicator = self.guest_choice_indicator_for(client_id, pane_id);
        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
            tab.set_guest_choice_indicator_on_pane(pane_id, client_id, indicator);
        }
    }

    pub(super) fn report_key_passthrough_state(
        &mut self,
        client_id: ClientId,
        old_pane_id: PaneId,
        new_pane_id: PaneId,
    ) {
        self.report_key_passthrough_state_with_direction(client_id, old_pane_id, new_pane_id, None);
    }

    pub(super) fn report_key_passthrough_state_with_direction(
        &mut self,
        client_id: ClientId,
        old_pane_id: PaneId,
        new_pane_id: PaneId,
        entered_from_direction: Option<Direction>,
    ) {
        let should_route = self.should_route_keys_to_pane(client_id, new_pane_id);
        let guest_pane_id = if should_route {
            Some(new_pane_id)
        } else {
            None
        };
        self.set_client_dimmed(client_id, should_route, guest_pane_id);
        let _ = self
            .bus
            .senders
            .send_to_server(ServerInstruction::KeyPassthroughChanged(
                client_id,
                old_pane_id,
                new_pane_id,
                should_route,
                entered_from_direction,
                true,
            ));
    }

    pub(super) fn set_client_dimmed(
        &mut self,
        client_id: ClientId,
        dimmed: bool,
        guest_pane_id: Option<PaneId>,
    ) {
        let changed = if dimmed {
            self.dimmed_clients.insert(client_id)
        } else {
            self.dimmed_clients.remove(&client_id)
        };
        let ascend_keys = if dimmed {
            guest_pane_id
                .and_then(|pane_id| self.guest_ascend_keys.get(&pane_id).cloned())
                .unwrap_or_default()
        } else {
            vec![]
        };
        let ascend_keys_changed = self
            .mode_info
            .get(&client_id)
            .map(|mode_info| mode_info.nested_ascend_keys != ascend_keys)
            .unwrap_or_else(|| !ascend_keys.is_empty());
        if !changed && !ascend_keys_changed {
            return;
        }
        let mode_info = self
            .mode_info
            .entry(client_id)
            .or_insert_with(|| self.default_mode_info.clone());
        mode_info.session_dimmed = if dimmed { Some(true) } else { None };
        mode_info.nested_ascend_keys = ascend_keys;
        let mode_info = mode_info.clone();
        for tab in self.tabs.values_mut() {
            tab.change_mode_info(mode_info.clone(), client_id);
            tab.mark_active_pane_for_rerender(client_id);
            tab.set_client_dimmed(client_id, dimmed);
        }
        for tab in self.tabs.values_mut() {
            let _ = tab.update_input_modes();
        }
        let _ = self.render();
    }

    pub(super) fn base_input_mode(&self) -> InputMode {
        self.default_mode_info.base_mode.unwrap_or_default()
    }

    fn own_ascend_shortcut(&self) -> Vec<KeyWithModifier> {
        shortcut_for_action(
            &self.default_mode_info.keybinds,
            self.base_input_mode(),
            |action| matches!(action, Action::FocusHostSession),
        )
        .unwrap_or_default()
    }

    fn own_descend_shortcut(&self) -> Vec<KeyWithModifier> {
        shortcut_for_action(
            &self.default_mode_info.keybinds,
            self.base_input_mode(),
            |action| matches!(action, Action::FocusGuestSession),
        )
        .unwrap_or_default()
    }

    fn own_host_zoom_shortcut(&self) -> Vec<KeyWithModifier> {
        shortcut_for_action(
            &self.default_mode_info.keybinds,
            self.base_input_mode(),
            |action| matches!(action, Action::ToggleHostFullscreen),
        )
        .unwrap_or_default()
    }

    fn guest_modal_shortcuts(&self) -> GuestModalShortcuts {
        GuestModalShortcuts {
            zoom: format_guest_modal_shortcut(&self.own_host_zoom_shortcut()),
            ascend: format_guest_modal_shortcut(&self.own_ascend_shortcut()),
            descend: format_guest_modal_shortcut(&self.own_descend_shortcut()),
        }
    }

    fn refresh_nested_ascend_keys_for_pane(&mut self, pane_id: PaneId) {
        let keys = self
            .guest_ascend_keys
            .get(&pane_id)
            .cloned()
            .unwrap_or_default();
        let dimmed_client_ids: Vec<ClientId> = self.dimmed_clients.iter().copied().collect();
        let mut clients_to_update: Vec<(ClientId, ModeInfo)> = vec![];
        for client_id in dimmed_client_ids {
            if self.get_active_pane_id(&client_id) != Some(pane_id) {
                continue;
            }
            if let Some(mode_info) = self.mode_info.get_mut(&client_id)
                && mode_info.nested_ascend_keys != keys
            {
                mode_info.nested_ascend_keys = keys.clone();
                clients_to_update.push((client_id, mode_info.clone()));
            }
        }
        if clients_to_update.is_empty() {
            return;
        }
        for (client_id, mode_info) in clients_to_update {
            for tab in self.tabs.values_mut() {
                tab.change_mode_info(mode_info.clone(), client_id);
            }
        }
        for tab in self.tabs.values_mut() {
            let _ = tab.update_input_modes();
        }
        let _ = self.render();
    }

    fn set_host_descended(&mut self, host_descended: bool) {
        if self.host_descended != host_descended {
            self.host_descended = host_descended;
            self.update_all_clients_nesting_mode_info();
        }
    }

    pub(super) fn broadcast_nested_shortcuts(&mut self) {
        if let Some(client_id) = self.nested_via_client_id {
            let payload = nested_session::encode_payload(&NestedSessionMessage::ShortcutUpdate {
                ascend_keys: self.own_ascend_shortcut(),
                descend_keys: vec![],
            });
            let _ =
                self.bus
                    .senders
                    .send_to_server(ServerInstruction::EmitNestedSessionFrameToClient(
                        client_id, payload,
                    ));
        }
        let frame = nested_session::encode_frame(&NestedSessionMessage::ShortcutUpdate {
            ascend_keys: vec![],
            descend_keys: self.own_descend_shortcut(),
        });
        for terminal_id in self.nested_guest_terminal_ids() {
            let _ = self
                .bus
                .senders
                .send_to_pty_writer(PtyWriteInstruction::Write(frame.clone(), terminal_id, None));
        }
    }

    pub(super) fn update_all_clients_nesting_mode_info(&mut self) {
        let ancestry = self.nested_ancestry.clone();
        let host_fullscreen = if self.host_fullscreen {
            Some(true)
        } else {
            None
        };
        let session_ascended = if !ancestry.is_empty() && !self.host_descended {
            Some(true)
        } else {
            None
        };
        let descend_keys = self.host_descend_keys.clone();
        self.default_mode_info.session_ancestry = ancestry.clone();
        self.default_mode_info.host_fullscreen = host_fullscreen;
        self.default_mode_info.session_ascended = session_ascended;
        self.default_mode_info.nested_descend_keys = descend_keys.clone();
        let mut client_ids: Vec<ClientId> = self.mode_info.keys().copied().collect();
        for connected_client_id in self.connected_clients.borrow().iter() {
            if !client_ids.contains(connected_client_id) {
                client_ids.push(*connected_client_id);
            }
        }
        for client_id in &client_ids {
            let mode_info = self
                .mode_info
                .entry(*client_id)
                .or_insert_with(|| self.default_mode_info.clone());
            mode_info.session_ancestry = ancestry.clone();
            mode_info.host_fullscreen = host_fullscreen;
            mode_info.session_ascended = session_ascended;
            mode_info.nested_descend_keys = descend_keys.clone();
            let mode_info = mode_info.clone();
            for tab in self.tabs.values_mut() {
                tab.change_mode_info(mode_info.clone(), *client_id);
            }
        }
        for tab in self.tabs.values_mut() {
            let _ = tab.update_input_modes();
        }
        let _ = self.render();
    }

    fn nested_guest_terminal_ids(&self) -> Vec<TerminalId> {
        let mut terminal_ids = vec![];
        for tab in self.tabs.values() {
            for pane_id in tab.nested_guest_pane_ids() {
                if let PaneId::Terminal(terminal_id) = pane_id
                    && self.nested_guest_tracker.is_tracked(pane_id)
                {
                    terminal_ids.push(terminal_id);
                }
            }
        }
        terminal_ids
    }

    pub(super) fn broadcast_ancestry_update_to_guests(&self) {
        let mut ancestry = self.nested_ancestry.clone();
        ancestry.push(self.session_name.clone());
        let message = NestedSessionMessage::AncestryUpdate { ancestry };
        let frame = nested_session::encode_frame(&message);
        for terminal_id in self.nested_guest_terminal_ids() {
            let _ = self
                .bus
                .senders
                .send_to_pty_writer(PtyWriteInstruction::Write(frame.clone(), terminal_id, None));
        }
    }

    pub(super) fn apply_nested_guest_fullscreen(&mut self, pane_id: PaneId, fullscreen: bool) {
        let mut actually_fullscreen = false;
        let mut displaced_pane_id = None;
        if let Some(tab) = self.tab_containing_pane_mut(pane_id) {
            // `tiled_or_floating_fullscreen` rather than the tab-wide `fullscreen_pane_id`: a
            // guest is never an overlay, and an overlay expanded over the screen is neither this
            // guest's fullscreen to report nor another's to displace
            let currently_fullscreen =
                matches!(tab.tiled_or_floating_fullscreen(), Some((id, true)) if id == pane_id);
            if fullscreen && !currently_fullscreen {
                if let Some(existing_fullscreen) =
                    tab.tiled_or_floating_fullscreen().map(|(id, _)| id)
                    && existing_fullscreen != pane_id
                {
                    displaced_pane_id = Some(existing_fullscreen);
                    tab.toggle_pane_no_ui_fullscreen(existing_fullscreen);
                }
                tab.toggle_pane_no_ui_fullscreen(pane_id);
            } else if !fullscreen && currently_fullscreen {
                tab.toggle_pane_no_ui_fullscreen(pane_id);
            }
            actually_fullscreen =
                matches!(tab.tiled_or_floating_fullscreen(), Some((id, true)) if id == pane_id);
        }
        if let Some(displaced_pane_id) = displaced_pane_id {
            self.nested_fullscreen_panes.remove(&displaced_pane_id);
        }
        if actually_fullscreen {
            self.nested_fullscreen_panes.insert(pane_id);
        } else {
            self.nested_fullscreen_panes.remove(&pane_id);
        }
    }

    fn unset_nested_fullscreen_if_active(&mut self, pane_id: PaneId) {
        if !self.nested_fullscreen_panes.contains(&pane_id) {
            return;
        }
        self.apply_nested_guest_fullscreen(pane_id, false);
        if let Some(client_id) = self.nested_via_client_id {
            let payload =
                nested_session::encode_payload(&NestedSessionMessage::ToggleHostFullscreen {
                    fullscreen: false,
                });
            let _ =
                self.bus
                    .senders
                    .send_to_server(ServerInstruction::EmitNestedSessionFrameToClient(
                        client_id, payload,
                    ));
        }
    }

    pub(super) fn sync_nested_guest_fullscreen_state(&mut self) {
        let mut actual: HashMap<PaneId, bool> = HashMap::new();
        for tab in self.tabs.values() {
            let tab_fullscreen = tab.tiled_or_floating_fullscreen();
            for pane_id in tab.nested_guest_pane_ids() {
                if !self.nested_guest_tracker.is_tracked(pane_id) {
                    continue;
                }
                let is_fullscreen = matches!(tab_fullscreen, Some((id, true)) if id == pane_id);
                actual.insert(pane_id, is_fullscreen);
            }
        }
        let mut updates: Vec<(TerminalId, bool)> = vec![];
        for (pane_id, is_fullscreen) in actual.iter() {
            let previously = self.reported_guest_fullscreen.get(pane_id).copied();
            if previously != Some(*is_fullscreen) {
                if let PaneId::Terminal(terminal_id) = pane_id {
                    updates.push((*terminal_id, *is_fullscreen));
                }
                self.reported_guest_fullscreen
                    .insert(*pane_id, *is_fullscreen);
            }
        }
        self.reported_guest_fullscreen
            .retain(|pane_id, _| actual.contains_key(pane_id));
        for (pane_id, is_fullscreen) in actual.iter() {
            if !is_fullscreen {
                self.downgrade_zoom_choice_to_descend(*pane_id);
            }
        }
        for (terminal_id, fullscreen) in updates {
            let message = NestedSessionMessage::FullscreenState { fullscreen };
            let _ = self
                .bus
                .senders
                .send_to_pty_writer(PtyWriteInstruction::Write(
                    nested_session::encode_frame(&message),
                    terminal_id,
                    None,
                ));
        }
    }

    fn clients_with_pane_focused(&self, pane_id: PaneId) -> Vec<ClientId> {
        let mut clients = vec![];
        for (client_id, tab_id) in self.active_tab_ids.iter() {
            if let Some(focused) = self
                .tabs
                .get(tab_id)
                .and_then(|t| t.get_active_pane_id(*client_id))
                && focused == pane_id
            {
                clients.push(*client_id);
            }
        }
        clients
    }

    pub(super) fn bubble_focus_to_host(&self, client_id: ClientId, direction: Direction) {
        let payload = nested_session::encode_payload(&NestedSessionMessage::FocusHost {
            direction: Some(direction),
        });
        let _ = self
            .bus
            .senders
            .send_to_server(ServerInstruction::EmitNestedSessionFrameToClient(
                client_id, payload,
            ));
    }

    pub fn handle_nested_session_message_from_host(
        &mut self,
        client_id: ClientId,
        message: NestedSessionMessage,
    ) {
        match message {
            NestedSessionMessage::AnnounceAck {
                ancestry,
                descend_keys,
                ..
            } => {
                log::info!(
                    "nested session handshake complete: this session is nested inside ancestry {:?}",
                    ancestry
                );
                self.nested_ancestry = ancestry;
                self.nested_via_client_id = Some(client_id);
                self.host_descend_keys = descend_keys;
                self.update_all_clients_nesting_mode_info();
                self.broadcast_ancestry_update_to_guests();
                let payload =
                    nested_session::encode_payload(&NestedSessionMessage::ShortcutUpdate {
                        ascend_keys: self.own_ascend_shortcut(),
                        descend_keys: vec![],
                    });
                let _ = self.bus.senders.send_to_server(
                    ServerInstruction::EmitNestedSessionFrameToClient(client_id, payload),
                );
            },
            NestedSessionMessage::ShortcutUpdate { descend_keys, .. } => {
                if self.host_descend_keys != descend_keys {
                    self.host_descend_keys = descend_keys;
                    self.update_all_clients_nesting_mode_info();
                }
            },
            NestedSessionMessage::FullscreenState { fullscreen } => {
                self.host_fullscreen = fullscreen;
                self.own_fullscreen_requested = fullscreen;
                self.update_all_clients_nesting_mode_info();
            },
            NestedSessionMessage::AncestryUpdate { ancestry } => {
                self.nested_ancestry = ancestry;
                self.update_all_clients_nesting_mode_info();
                self.broadcast_ancestry_update_to_guests();
            },
            NestedSessionMessage::FocusGained {
                from_direction: Some(direction),
            } => {
                self.set_host_descended(true);
                let old_pane_id = self.get_active_pane_id(&client_id);
                if let Ok(tab) = self.get_active_tab_mut(client_id) {
                    tab.focus_pane_on_edge(direction, client_id);
                }
                let new_pane_id = self.get_active_pane_id(&client_id);
                if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                    self.report_key_passthrough_state_with_direction(
                        client_id,
                        old,
                        new,
                        Some(direction),
                    );
                }
                let _ = self.render();
                let _ = self.log_and_report_session_state();
            },
            NestedSessionMessage::FocusGained {
                from_direction: None,
            } => {
                self.set_host_descended(true);
                let _ = self.render();
            },
            NestedSessionMessage::FocusLost => {
                self.set_host_descended(false);
                let _ = self.render();
            },
            other => {
                log::debug!(
                    "dropping unsupported nested session message from host: {:?}",
                    other
                );
            },
        }
    }

    pub fn handle_nested_guest_ping_tick(&mut self, pane_id: PaneId) {
        use crate::nested_guest::GuestPingTickAction;
        match self.nested_guest_tracker.on_tick(pane_id, Instant::now()) {
            GuestPingTickAction::SendPing => {
                if let PaneId::Terminal(terminal_id) = pane_id {
                    let _ = self
                        .bus
                        .senders
                        .send_to_pty_writer(PtyWriteInstruction::Write(
                            nested_session::encode_frame(&NestedSessionMessage::Ping),
                            terminal_id,
                            None,
                        ));
                    log::debug!("pinged nested session guest in pane {:?}", pane_id);
                }
            },
            GuestPingTickAction::Suspended => {
                log::info!(
                    "nested session guest in pane {:?} missed the pong timeout, suspending until it re-announces",
                    pane_id
                );
                self.suspend_nested_guest(pane_id);
            },
            GuestPingTickAction::Unknown => {
                let _ = self
                    .bus
                    .senders
                    .send_to_background_jobs(BackgroundJob::StopNestedGuestPing(pane_id));
            },
        }
    }

    /// Build a reply for `query` from whatever host state Yinz has
    /// cached, or an empty `Vec` if nothing relevant is cached. The
    /// classification happened at intercept time (Grid produces a
    /// [`HostQuery`]), so this method is pure structural dispatch —
    /// no re-parsing of bytes.
    pub(super) fn synthesize_cached_reply(&self, query: &crate::host_query::HostQuery) -> Vec<u8> {
        use crate::host_query::HostQuery;
        match query {
            HostQuery::TextAreaPixelSize => self
                .pixel_dimensions
                .text_area_size
                .map(|dims| format!("\x1b[4;{};{}t", dims.height, dims.width).into_bytes())
                .unwrap_or_default(),
            HostQuery::CharacterCellPixelSize => self
                .pixel_dimensions
                .character_cell_size
                .map(|dims| format!("\x1b[6;{};{}t", dims.height, dims.width).into_bytes())
                .unwrap_or_default(),
            HostQuery::DefaultForeground { terminator }
            | HostQuery::DefaultBackground { terminator } => {
                let palette = self.terminal_emulator_colors.borrow();
                let (channel, color) = match query {
                    HostQuery::DefaultForeground { .. } => (10u32, palette.fg),
                    HostQuery::DefaultBackground { .. } => (11u32, palette.bg),
                    _ => unreachable!(),
                };
                if let PaletteColor::Rgb((r, g, b)) = color {
                    let mut out = format!(
                        "\x1b]{};rgb:{:04x}/{:04x}/{:04x}",
                        channel,
                        (r as u16) * 0x0101,
                        (g as u16) * 0x0101,
                        (b as u16) * 0x0101,
                    )
                    .into_bytes();
                    out.extend_from_slice(terminator.as_bytes());
                    out
                } else {
                    Vec::new()
                }
            },
            HostQuery::PaletteRegister { index, terminator } => {
                let codes = self.terminal_emulator_color_codes.borrow();
                if let Some(color) = codes.get(&(*index as usize)) {
                    let mut out = format!("\x1b]4;{};{}", index, color).into_bytes();
                    out.extend_from_slice(terminator.as_bytes());
                    out
                } else {
                    Vec::new()
                }
            },
            HostQuery::ClipboardContent { .. } => query.empty_reply_bytes(),
            // Should not reach here: ColorPaletteMode short-circuits in
            // `forward_host_query` before any cache-fallback path runs.
            HostQuery::ColorPaletteMode => match self.effective_host_terminal_theme_mode() {
                HostTerminalThemeMode::Dark => b"\x1b[?997;1n".to_vec(),
                HostTerminalThemeMode::Light => b"\x1b[?997;2n".to_vec(),
            },
        }
    }
}
