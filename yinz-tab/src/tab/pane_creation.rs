//! Creating and replacing panes in the tab's layers.

use super::{overlay_placement_for, run_awaits_placement, Tab};
use crate::background_jobs::BackgroundJob;
use crate::panes::floating_panes::floating_pane_grid::half_size_middle_geom;
use crate::panes::OverlayCarry;
use crate::panes::PaneFactory;
use crate::panes::PaneId;
use crate::panes::PluginPane;
use crate::panes::TerminalPane;
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::pty::ReplacedPane;
use crate::route::NotificationEnd;
use crate::screen::ScreenInstruction;
use crate::ClientId;
use std::str;
use std::time::Instant;
use yinz_panes::pane::HoldForCommand;
use yinz_panes::pane::Pane;
use yinz_panes::resize_pty;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::Event;
use yinz_types::data::NewPanePlacement;
use yinz_types::data::OverlayPlacement;
use yinz_types::geometry::Direction;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::geometry::SplitDirection;
use yinz_types::input::command::RunCommand;
use yinz_types::input::layout::FloatingPaneLayout;
use yinz_types::input::layout::Run;
use yinz_types::internal_ids::TerminalId;
use yinz_types::pane_size::Offset;
use yinz_types::pane_size::PaneGeom;

impl Tab {
    fn normalize_invoked_with_for_default_shell(&self, invoked_with: Option<Run>) -> Option<Run> {
        let default_shell_run_command = Run::Command(RunCommand {
            command: self.default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        });
        if invoked_with == Some(default_shell_run_command) {
            None
        } else {
            invoked_with
        }
    }
    pub fn new_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        new_pane_placement: NewPanePlacement,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
    ) -> Result<()> {
        let invoked_with = self.normalize_invoked_with_for_default_shell(invoked_with);
        match new_pane_placement {
            NewPanePlacement::NoPreference { borderless } => self.new_no_preference_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                client_id,
                blocking_notification,
                borderless,
            ),
            NewPanePlacement::Tiled {
                direction: None,
                borderless,
            } => self.new_tiled_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                client_id,
                blocking_notification,
                borderless,
            ),
            NewPanePlacement::Tiled {
                direction: Some(direction),
                borderless,
            } => {
                let is_vertical = direction == Direction::Left || direction == Direction::Right;
                let focused_client_id = client_id.filter(|_| should_focus_pane);
                match focused_client_id {
                    Some(client_id) => {
                        if is_vertical {
                            self.vertical_split(
                                pid,
                                initial_pane_title,
                                invoked_with,
                                client_id,
                                blocking_notification,
                                borderless,
                            )?;
                        } else {
                            self.horizontal_split(
                                pid,
                                initial_pane_title,
                                invoked_with,
                                client_id,
                                blocking_notification,
                                borderless,
                            )?;
                        }
                    },
                    None => {
                        // Nobody to focus for (e.g. `yinz action new-pane -d right` against a
                        // session with no client attached): split the client's active pane if it
                        // has one, else the first selectable pane, else just add a tiled pane.
                        let target_pane_id = client_id
                            .and_then(|client_id| self.tiled_panes.get_active_pane_id(client_id))
                            .or_else(|| self.tiled_panes.first_selectable_pane_id());
                        match target_pane_id {
                            Some(target_pane_id) if is_vertical => {
                                self.vertical_split_of_pane_id(
                                    pid,
                                    initial_pane_title,
                                    invoked_with,
                                    target_pane_id,
                                    blocking_notification,
                                    borderless,
                                )?;
                            },
                            Some(target_pane_id) => {
                                self.horizontal_split_of_pane_id(
                                    pid,
                                    initial_pane_title,
                                    invoked_with,
                                    target_pane_id,
                                    blocking_notification,
                                    borderless,
                                )?;
                            },
                            None => {
                                self.new_tiled_pane(
                                    pid,
                                    initial_pane_title,
                                    invoked_with,
                                    start_suppressed,
                                    should_focus_pane,
                                    client_id,
                                    blocking_notification,
                                    borderless,
                                )?;
                            },
                        }
                    },
                }
                Ok(())
            },
            NewPanePlacement::Floating(floating_pane_coordinates) => self.new_floating_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                floating_pane_coordinates,
                blocking_notification,
            ),
            // The kind travels with the placement all the way into `OverlayPanes`. It used to be
            // flattened to `overlay: true` here and then re-derived from the plugin's own
            // configuration wherever it was needed - which is why a `--modal` *terminal* pane got
            // overlay geometry but no modal focus semantics.
            NewPanePlacement::Overlay(overlay) => self.new_overlay_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                overlay,
                client_id,
                blocking_notification,
            ),
            NewPanePlacement::InPlace {
                pane_id_to_replace,
                close_replaced_pane,
                borderless,
            } => self.new_in_place_pane(
                pid,
                initial_pane_title,
                invoked_with,
                pane_id_to_replace,
                close_replaced_pane,
                client_id,
                blocking_notification,
                borderless,
            ),
            NewPanePlacement::Stacked {
                pane_id_to_stack_under,
                borderless,
            } => self.new_stacked_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                pane_id_to_stack_under,
                client_id,
                blocking_notification,
                borderless,
            ),
        }
    }
    pub fn new_no_preference_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context = || format!("failed to create new pane with id {pid:?}");
        if !start_suppressed {
            self.close_down_to_max_terminals()
                .with_context(err_context)?;
        }
        let mut new_pane =
            self.build_pane(pid, initial_pane_title, invoked_with, blocking_notification)?;

        if let Some(borderless) = borderless {
            new_pane.set_borderless(borderless);
        }

        if start_suppressed {
            self.park_new_pane(new_pane).with_context(err_context)
        } else {
            // no overlay exclusion needed here any more: an overlay is not in `floating_panes`,
            // so "the client's active pane is a floating pane" can no longer accidentally mean
            // "the client is looking at a modal drawn over a hidden layer"
            let active_pane_is_floating = client_id
                .and_then(|client_id| self.get_active_pane_id(client_id))
                .is_some_and(|pane_id| self.floating_panes.panes_contain(&pane_id));
            if active_pane_is_floating && !self.floating_panes.panes_are_visible() {
                self.show_floating_panes();
            }
            let should_add_floating =
                self.floating_panes.panes_are_visible() || active_pane_is_floating;
            if should_focus_pane && should_add_floating {
                self.add_floating_pane(new_pane, pid, None, true, client_id)
            } else if should_focus_pane {
                // an overlay holds this client's focus while the floating layer is down, and
                // `get_active_pane_id` would keep preferring it over the pane we are about to
                // focus. The new pane is what the user just asked for, so a dismissible overlay
                // gives the keyboard up here and a modal keeps it - see
                // `defocus_pane_if_dismissible` for why the two differ. When the modal keeps it,
                // the new pane opens tiled but unfocused behind it.
                let keyboard_is_free =
                    client_id.is_none_or(|client_id| self.defocus_overlay_pane(client_id, true));
                if keyboard_is_free {
                    self.add_tiled_pane(new_pane, pid, false, client_id)
                } else {
                    self.add_tiled_pane(new_pane, pid, false, None)
                }
            } else if should_add_floating {
                self.add_floating_pane(new_pane, pid, None, false, client_id)
            } else {
                self.add_tiled_pane(new_pane, pid, false, None)
            }
        }
    }
    pub fn new_tiled_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context = || format!("failed to create new pane with id {pid:?}");
        if should_focus_pane {
            self.hide_floating_panes();
        }
        if !start_suppressed {
            self.close_down_to_max_terminals()
                .with_context(err_context)?;
        }
        let mut new_pane =
            self.build_pane(pid, initial_pane_title, invoked_with, blocking_notification)?;

        if let Some(borderless) = borderless {
            new_pane.set_borderless(borderless);
        }

        if start_suppressed {
            self.park_new_pane(new_pane).with_context(err_context)
        } else {
            let focus_client_id = if should_focus_pane { client_id } else { None };
            self.add_tiled_pane(new_pane, pid, false, focus_client_id)
        }
    }
    /// Build the pane a `new_*_pane` path is about to place, without deciding where it goes.
    ///
    /// Shared by no-preference, tiled, stacked, floating, and overlay creation. Each caller
    /// retains its placement, focus, and borderless decisions.
    pub(super) fn build_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        blocking_notification: Option<NotificationEnd>,
    ) -> Result<Box<dyn Pane>> {
        let err_context = || format!("failed to create new pane with id {pid:?}");
        Ok(match pid {
            PaneId::Terminal(term_pid) => {
                let next_terminal_position = self.get_next_terminal_position();
                PaneFactory::terminal(
                    term_pid,
                    PaneGeom::default(), // this will be filled out later
                    self.style,
                    next_terminal_position,
                    initial_pane_title.clone().unwrap_or_default(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    initial_pane_title,
                    invoked_with,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                    self.osc8_hyperlinks,
                    self.explicitly_disable_kitty_keyboard_protocol,
                    blocking_notification,
                )
            },
            PaneId::Plugin(plugin_pid) => {
                PaneFactory::plugin(
                    plugin_pid,
                    PaneGeom::default(), // this will be filled out later
                    self.senders
                        .to_plugin
                        .as_ref()
                        .with_context(err_context)?
                        .clone(),
                    initial_pane_title.unwrap_or("".to_owned()),
                    String::new(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.connected_clients_in_app
                        .borrow()
                        .iter()
                        .copied()
                        .collect(),
                    self.style,
                    invoked_with,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                )
            },
        })
    }

    pub fn create_warp_companion(
        &mut self,
        id: PaneId,
        layout: FloatingPaneLayout,
        held_command: HoldForCommand,
    ) -> Result<()> {
        let initial_title = match &layout.run {
            Some(Run::Command(command)) => Some(command.to_string()),
            Some(Run::Plugin(plugin)) => Some(plugin.location_string()),
            _ => None,
        };
        let mut pane = self.build_pane(id, initial_title, layout.run, None)?;
        pane.update_name("\0");
        pane.update_name(&layout.name.unwrap_or_default());
        pane.set_borderless(layout.borderless.unwrap_or(false));
        pane.set_pane_default_colors(layout.default_fg, layout.default_bg);
        self.park_new_pane(pane).context("park warp companion")?;
        let pane = self
            .get_pane_with_id_mut(id)
            .context("find parked warp companion")?;
        if let Some(contents) = layout.pane_initial_contents {
            pane.set_initial_contents(&contents);
        }
        if let Some(command) = held_command {
            pane.hold(None, true, command);
        }
        Ok(())
    }

    /// A rejected landing cannot return supervisors already adopted from another session.
    /// Keep them reachable, with one visible pane if this tab would otherwise be empty.
    pub fn retain_adopted_layout_terminals(
        &mut self,
        terminals: Vec<(TerminalId, RunCommand, HoldForCommand)>,
        client_id: ClientId,
    ) -> Result<()> {
        for (terminal_id, command, held_command) in terminals {
            let pane_id = PaneId::Terminal(terminal_id);
            if self.get_pane_with_id(pane_id).is_some() {
                continue;
            }
            let mut pane = self
                .build_pane(pane_id, None, Some(Run::Command(command)), None)
                .context("failed to retain an adopted terminal after layout rejection")?;
            if let Some(command) = held_command {
                pane.hold(None, true, command);
            }
            let show_pane = self.tiled_panes.get_panes().next().is_none();
            let geom = if show_pane {
                PaneGeom::from(&*self.display_area.borrow())
            } else {
                half_size_middle_geom(&self.viewport.borrow(), 0)
            };
            pane.set_geom(geom);
            resize_pty!(pane, self.os_api, self.senders, self.character_cell_size).non_fatal();
            if show_pane {
                self.tiled_panes.add_pane_with_existing_geom(pane_id, pane);
                self.tiled_panes.focus_pane(pane_id, client_id);
                self.tiled_panes.reapply_pane_frames();
            } else {
                self.park_pane(pane);
            }
        }
        self.finish_pending_layout();
        Ok(())
    }

    /// A failed layout still leaves any panes already in the tab usable.
    pub fn complete_agent_placement(&mut self, pane_id: PaneId, mut completion: NotificationEnd) {
        self.agent_tab_waiting_for_pane = false;
        if self.is_pending {
            completion.wait_for_placement();
            self.pending_agent_placements.push((pane_id, completion));
        } else if self.has_pane_with_pid(&pane_id) {
            completion.complete_pane_placement(self.id);
        }
    }

    pub fn fail_pending_agent_placements(&mut self, message: &str) {
        for (_, mut completion) in std::mem::take(&mut self.pending_agent_placements) {
            completion.set_error_message(message.to_owned());
        }
    }

    pub(super) fn complete_pending_agent_placements(&mut self) {
        for (pane_id, completion) in std::mem::take(&mut self.pending_agent_placements) {
            self.complete_agent_placement(pane_id, completion);
        }
    }

    pub fn finish_pending_layout(&mut self) {
        self.is_pending = false;
        // The rejected initial layout never supplied final geometry for arriving panes.
        self.tiled_panes.force_resize();
        self.apply_buffered_instructions()
            .context("failed to replay retained pane output")
            .non_fatal();
        self.complete_pending_agent_placements();
        self.set_force_render();
    }

    /// Park a pane in the suppressed panes rather than placing it, giving it a plausible geometry
    /// so that it looks right the moment something takes it back out.
    pub(super) fn park_new_pane(&mut self, mut new_pane: Box<dyn Pane>) -> Result<()> {
        let err_context = || "failed to park new pane".to_string();
        let viewport = { *self.viewport.borrow() };
        let new_pane_geom = half_size_middle_geom(&viewport, 0);
        new_pane.set_active_at(Instant::now());
        new_pane.set_geom(new_pane_geom);
        new_pane.set_content_offset(Offset::frame(1));
        resize_pty!(
            new_pane,
            self.os_api,
            self.senders,
            self.character_cell_size
        )
        .with_context(err_context)?;
        self.park_pane(new_pane);
        Ok(())
    }

    pub fn new_floating_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        blocking_notification: Option<NotificationEnd>,
    ) -> Result<()> {
        // A modal is an overlay because of what it is, not because of the coordinates whichever
        // caller happened to pass: `ScreenInstruction::AddPlugin` only synthesizes overlay
        // coordinates when nobody passed any, and a keybinding written before `overlay`/`modal`
        // existed passes none of it at all. Asking the plugin's own configuration catches both.
        //
        // Both spellings are still *inputs* - what they no longer do is decide where the pane is
        // stored. An overlay goes to `overlay_panes` and never touches the floating layer.
        if let Some(placement) = overlay_placement_for(&floating_pane_coordinates, &invoked_with) {
            return self.new_overlay_pane(
                pid,
                initial_pane_title,
                invoked_with,
                start_suppressed,
                should_focus_pane,
                placement,
                None,
                blocking_notification,
            );
        }
        if should_focus_pane {
            self.show_floating_panes();
        }
        // A suppressed slot has no place in the layout yet. Reserving it must not evict
        // an existing pane before the operation that adopts it has succeeded.
        if !start_suppressed {
            self.close_down_to_max_terminals()?;
        }
        let new_pane =
            self.build_pane(pid, initial_pane_title, invoked_with, blocking_notification)?;

        if start_suppressed {
            // this pane needs to start in the background (suppressed), only accessible if a plugin
            // takes it out of there in one way or another
            self.park_new_pane(new_pane)
        } else {
            self.add_floating_pane(
                new_pane,
                pid,
                floating_pane_coordinates,
                should_focus_pane,
                None,
            )
        }
    }

    /// Create a pane directly onto the session surface as an overlay.
    ///
    /// Nothing raises the floating layer here, which is the whole point: an overlay that is never
    /// on that layer has no reveal to undo. The bookkeeping this used to be contrasted with -
    /// `layer_visible_when_added`, `hide_floating_panes_under_overlay` - is gone with the
    /// two-stage birth it compensated for.
    pub fn new_overlay_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        placement: OverlayPlacement,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
    ) -> Result<()> {
        placement.validate().map_err(|error| anyhow!(error))?;
        if !start_suppressed {
            self.close_down_to_max_terminals()?;
        }
        let new_pane =
            self.build_pane(pid, initial_pane_title, invoked_with, blocking_notification)?;
        let awaiting_placement = run_awaits_placement(new_pane.invoked_with());
        self.add_overlay_pane(
            new_pane,
            pid,
            placement,
            // Hidden overlays still accept geometry before reveal.
            OverlayCarry {
                hidden: start_suppressed,
                awaiting_placement,
                ..Default::default()
            },
            should_focus_pane,
            client_id,
        )
    }

    pub fn new_in_place_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        pane_id_to_replace: Option<PaneId>,
        close_replaced_pane: bool,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let replaced = ReplacedPane::closed_if(close_replaced_pane);
        match (pane_id_to_replace, client_id) {
            (Some(pane_id_to_replace), _) => {
                self.suppress_pane_and_replace_with_pid(
                    pane_id_to_replace,
                    pid,
                    replaced,
                    invoked_with,
                    blocking_notification,
                    borderless,
                )?;
            },
            (None, Some(client_id)) => match self.get_active_pane_id(client_id) {
                Some(active_pane_id) => {
                    self.suppress_pane_and_replace_with_pid(
                        active_pane_id,
                        pid,
                        replaced,
                        invoked_with,
                        blocking_notification,
                        borderless,
                    )?;
                },
                None => {
                    log::error!("Cannot find active pane");
                },
            },
            _ => {
                log::error!("Must have pane id to replace or client id to start pane in place>");
            },
        }
        if let Some(initial_pane_title) = initial_pane_title {
            let _ = self.rename_pane(initial_pane_title.as_bytes().to_vec(), pid);
        }
        Ok(())
    }
    pub fn new_stacked_pane(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        start_suppressed: bool,
        should_focus_pane: bool,
        pane_id_to_stack_under: Option<PaneId>,
        client_id: Option<ClientId>,
        blocking_notification: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context = || format!("failed to create new pane with id {pid:?}");
        let root = pane_id_to_stack_under
            .or_else(|| client_id.and_then(|client| self.get_active_pane_id(client)));
        if !start_suppressed && root.is_some_and(|id| self.multipane_info(id).is_some()) {
            let message = "cannot stack a pane into a multipane child";
            if let Some(mut notification) = blocking_notification {
                notification.set_error_message(message.into());
            }
            self.senders
                .send_to_pty(PtyInstruction::ClosePane(pid, None))
                .non_fatal();
            Err::<(), _>(anyhow!(message)).non_fatal();
            return Ok(());
        }
        if should_focus_pane {
            self.hide_floating_panes();
        }
        if !start_suppressed {
            self.close_down_to_max_terminals()
                .with_context(err_context)?;
        }
        let mut new_pane =
            self.build_pane(pid, initial_pane_title, invoked_with, blocking_notification)?;

        if let Some(borderless) = borderless {
            new_pane.set_borderless(borderless);
        }

        if start_suppressed {
            self.park_new_pane(new_pane).with_context(err_context)
        } else {
            if let Some(pane_id_to_stack_under) = pane_id_to_stack_under {
                self.add_stacked_pane_to_pane_id(
                    new_pane,
                    pid,
                    pane_id_to_stack_under,
                    should_focus_pane,
                )
            } else if let Some(client_id) = client_id {
                self.add_stacked_pane_to_active_pane(new_pane, pid, client_id, should_focus_pane)
            } else {
                // Nothing to stack under, and the pane is already built: parked, not dropped,
                // for the reason `add_stacked_pane_to_pane_id` gives.
                log::error!("Must have client id or pane id to stack pane, hiding it instead");
                self.park_new_pane(new_pane).with_context(err_context)
            }
        }
    }
    pub fn replace_active_pane_with_editor_pane(
        &mut self,
        pid: PaneId,
        client_id: ClientId,
    ) -> Result<()> {
        // this method creates a new pane from pid and replaces it with the active pane
        // the active pane is then suppressed (hidden and not rendered) until the current
        // created pane is closed, in which case it will be replaced back by it
        let err_context = || format!("failed to suppress active pane for client {client_id}");

        match pid {
            PaneId::Terminal(pid) => {
                let new_pane = self.new_scrollback_editor_pane(pid);
                let replaced_pane = if self.floating_panes.panes_are_visible() {
                    self.floating_panes
                        .replace_active_pane(Box::new(new_pane), client_id)
                        .ok()
                } else {
                    self.tiled_panes
                        .replace_active_pane(Box::new(new_pane), client_id)
                };
                match replaced_pane {
                    Some(replaced_pane) => {
                        self.track_stack_list_member_id_swap(
                            replaced_pane.pid(),
                            PaneId::Terminal(pid),
                        );
                        self.insert_scrollback_editor_replaced_pane(replaced_pane, pid);
                        self.get_active_pane(client_id)
                            .with_context(|| format!("no active pane found for client {client_id}"))
                            .and_then(|current_active_pane| {
                                resize_pty!(
                                    current_active_pane,
                                    self.os_api,
                                    self.senders,
                                    self.character_cell_size
                                )
                            })
                            .with_context(err_context)?;
                    },
                    None => {
                        Err::<(), _>(anyhow!(
                            "Could not find editor pane to replace - is no pane focused?"
                        ))
                        .with_context(err_context)
                        .non_fatal();
                    },
                }
            },
            PaneId::Plugin(_pid) => {
                // TBD, currently unsupported
            },
        }
        Ok(())
    }
    pub fn replace_pane_with_editor_pane(
        &mut self,
        pid: PaneId,
        pane_id_to_replace: PaneId,
    ) -> Result<()> {
        // this method creates a new pane from pid and replaces it with the pane iwth the given pane_id_to_replace
        // the pane with the given pane_id_to_replace is then suppressed (hidden and not rendered) until the current
        // created pane is closed, in which case it will be replaced back by it
        let err_context = || "failed to suppress pane".to_string();
        if !self.has_non_suppressed_pane_with_pid(&pane_id_to_replace) {
            return Err(self.no_slot_error(pane_id_to_replace));
        }

        match pid {
            PaneId::Terminal(pid) => {
                let new_pane = self.new_scrollback_editor_pane(pid);
                let replaced_pane = if self.floating_panes.panes_contain(&pane_id_to_replace) {
                    self.floating_panes
                        .replace_pane(pane_id_to_replace, Box::new(new_pane))
                        .ok()
                } else if self.overlay_panes.panes_contain(&pane_id_to_replace) {
                    self.overlay_panes
                        .replace_pane(pane_id_to_replace, Box::new(new_pane))
                        .ok()
                } else {
                    self.tiled_panes
                        .replace_pane(pane_id_to_replace, Box::new(new_pane))
                };
                match replaced_pane {
                    Some(replaced_pane) => {
                        resize_pty!(
                            replaced_pane,
                            self.os_api,
                            self.senders,
                            self.character_cell_size
                        )
                        .non_fatal();
                        self.track_stack_list_member_id_swap(
                            pane_id_to_replace,
                            PaneId::Terminal(pid),
                        );
                        self.insert_scrollback_editor_replaced_pane(replaced_pane, pid);
                    },
                    None => {
                        Err::<(), _>(anyhow!("Could not find editor pane to replace"))
                            .with_context(err_context)
                            .non_fatal();
                    },
                }
            },
            PaneId::Plugin(_pid) => {
                // TBD, currently unsupported
            },
        }
        Ok(())
    }
    /// Open `new_pane_id` in `old_pane_id`'s slot, and close, suppress or lend the pane that
    /// was there as `replaced` says.
    ///
    /// Fails before building anything when `old_pane_id` holds no slot: every collection's
    /// `replace_pane` consumes the replacement, so asking one to replace a pane it does not
    /// have would silently drop the new pane and orphan whatever it was running. The caller
    /// owns the new pane id and has to forget it.
    pub fn suppress_pane_and_replace_with_pid(
        &mut self,
        old_pane_id: PaneId,
        new_pane_id: PaneId,
        replaced: ReplacedPane,
        run: Option<Run>,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        // this method creates a new pane from pid and replaces it with the active pane
        // the active pane is then suppressed (hidden and not rendered) until the current
        // created pane is closed, in which case it will be replaced back by it
        let err_context = || "failed to suppress active pane".to_string();
        if !self.has_non_suppressed_pane_with_pid(&old_pane_id) {
            return Err(self.no_slot_error(old_pane_id));
        }
        let close_replaced_pane = replaced.closes();
        let lent = replaced == ReplacedPane::Lend;

        match new_pane_id {
            PaneId::Terminal(new_pane_id) => {
                let next_terminal_position = self.get_next_terminal_position();
                let mut new_pane = TerminalPane::new(
                    new_pane_id,
                    PaneGeom::default(), // the initial size will be set later
                    self.style,
                    next_terminal_position,
                    String::new(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    None,
                    run,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                    self.osc8_hyperlinks,
                    self.explicitly_disable_kitty_keyboard_protocol,
                    completion_tx,
                );
                if let Some(supported) = self.kitty_host_support {
                    new_pane.update_kitty_host_support(supported);
                }
                if let Some(supported) = self.sixel_host_support {
                    new_pane.update_sixel_host_support(supported);
                }
                if let Some(borderless) = borderless {
                    new_pane.set_borderless(borderless);
                }
                let replaced_pane = if self.floating_panes.panes_contain(&old_pane_id) {
                    self.floating_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                        .ok()
                } else if self.overlay_panes.panes_contain(&old_pane_id) {
                    self.overlay_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                        .ok()
                } else {
                    self.tiled_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                };
                if replaced_pane.is_some() {
                    self.track_stack_list_member_id_swap(
                        old_pane_id,
                        PaneId::Terminal(new_pane_id),
                    );
                }
                if close_replaced_pane {
                    if let Some(replaced_pane) = replaced_pane {
                        let pid = replaced_pane.pid();
                        if self.pane_tabs_of(pid).is_some() {
                            self.park_pane(replaced_pane);
                            self.close_pane_explicitly(pid, None);
                        } else {
                            self.senders
                                .send_to_pty(PtyInstruction::ClosePane(pid, None))
                                .with_context(err_context)?;
                        }
                    }
                } else {
                    match replaced_pane {
                        Some(replaced_pane) => {
                            let _ = resize_pty!(
                                replaced_pane,
                                self.os_api,
                                self.senders,
                                self.character_cell_size
                            );
                            let is_scrollback_editor = false;
                            self.suppress_pane_behind(
                                replaced_pane,
                                PaneId::Terminal(new_pane_id),
                                is_scrollback_editor,
                                lent,
                            );
                        },
                        None => {
                            Err::<(), _>(anyhow!(
                                "Could not find editor pane to replace - is no pane focused?"
                            ))
                            .with_context(err_context)
                            .non_fatal();
                        },
                    }
                }
            },
            PaneId::Plugin(plugin_pid) => {
                let mut new_pane = PluginPane::new(
                    plugin_pid,
                    PaneGeom::default(), // this will be filled out later
                    self.senders
                        .to_plugin
                        .as_ref()
                        .with_context(err_context)?
                        .clone(),
                    String::new(),
                    String::new(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.connected_clients_in_app
                        .borrow()
                        .iter()
                        .copied()
                        .collect(),
                    self.style,
                    run,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                );
                if let Some(borderless) = borderless {
                    new_pane.set_borderless(borderless);
                }
                let replaced_pane = if self.floating_panes.panes_contain(&old_pane_id) {
                    self.floating_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                        .ok()
                } else if self.overlay_panes.panes_contain(&old_pane_id) {
                    self.overlay_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                        .ok()
                } else {
                    self.tiled_panes
                        .replace_pane(old_pane_id, Box::new(new_pane))
                };
                if replaced_pane.is_some() {
                    self.track_stack_list_member_id_swap(old_pane_id, PaneId::Plugin(plugin_pid));
                }
                if close_replaced_pane {
                    if let Some(replaced_pane) = replaced_pane {
                        let pid = replaced_pane.pid();
                        if self.pane_tabs_of(pid).is_some() {
                            self.park_pane(replaced_pane);
                            self.close_pane_explicitly(pid, None);
                        } else {
                            self.senders
                                .send_to_pty(PtyInstruction::ClosePane(pid, None))
                                .with_context(err_context)?;
                        }
                    }
                } else {
                    match replaced_pane {
                        Some(replaced_pane) => {
                            let _ = resize_pty!(
                                replaced_pane,
                                self.os_api,
                                self.senders,
                                self.character_cell_size
                            );
                            let is_scrollback_editor = false;
                            self.suppress_pane_behind(
                                replaced_pane,
                                PaneId::Plugin(plugin_pid),
                                is_scrollback_editor,
                                lent,
                            );
                        },
                        None => {
                            Err::<(), _>(anyhow!(
                                "Could not find editor pane to replace - is no pane focused?"
                            ))
                            .with_context(err_context)
                            .non_fatal();
                        },
                    }
                }
            },
        }
        Ok(())
    }
    pub fn close_pane_and_replace_with_other_pane(
        &mut self,
        pane_id_to_replace: PaneId,
        pane_to_replace_with: Box<dyn Pane>,
        completion_tx: Option<NotificationEnd>,
    ) {
        let Some(pane_to_replace_with) =
            self.park_unless_slot_held(pane_id_to_replace, pane_to_replace_with)
        else {
            return;
        };
        let replacement_pane_id = pane_to_replace_with.pid();
        let mut replaced_pane = if self.floating_panes.panes_contain(&pane_id_to_replace) {
            self.floating_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
                .ok()
        } else if self.overlay_panes.panes_contain(&pane_id_to_replace) {
            self.overlay_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
                .ok()
        } else {
            self.tiled_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
        };
        if replaced_pane.is_some() {
            self.track_stack_list_member_id_swap(pane_id_to_replace, replacement_pane_id);
        }
        if let Some(replaced_pane) = replaced_pane.take() {
            let pane_id = replaced_pane.pid();
            if self.pane_tabs_of(pane_id).is_some() {
                self.park_pane(replaced_pane);
                self.close_pane_explicitly(pane_id, completion_tx);
                return;
            }
            let _ = self
                .senders
                .send_to_pty(PtyInstruction::ClosePane(pane_id, completion_tx));
            let _ = self.senders.send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::PaneClosed(pane_id),
            )]));
            let _ = self
                .senders
                .send_to_screen(ScreenInstruction::NotifyPaneClosedToSubscribers { pane_id });
            drop(replaced_pane);
        }
    }
    /// Hand `pane` back when `pane_id_to_replace` holds a slot it can take. Otherwise park it
    /// here and return `None`: every collection consumes the pane it is asked to put in place
    /// of one it does not have, and a parked pane can still be found and closed.
    fn park_unless_slot_held(
        &mut self,
        pane_id_to_replace: PaneId,
        pane: Box<dyn Pane>,
    ) -> Option<Box<dyn Pane>> {
        if self.has_non_suppressed_pane_with_pid(&pane_id_to_replace) {
            return Some(pane);
        }
        log::error!(
            "pane {pane_id_to_replace:?} holds no slot; parking {:?} rather than dropping it",
            pane.pid()
        );
        self.park_pane(pane);
        None
    }
    pub fn suppress_pane_and_replace_with_other_pane(
        &mut self,
        pane_id_to_replace: PaneId,
        pane_to_replace_with: Box<dyn Pane>,
        _completion_tx: Option<NotificationEnd>,
    ) {
        let Some(pane_to_replace_with) =
            self.park_unless_slot_held(pane_id_to_replace, pane_to_replace_with)
        else {
            return;
        };
        let mut replaced_pane = if self.floating_panes.panes_contain(&pane_id_to_replace) {
            self.floating_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
                .ok()
        } else if self.overlay_panes.panes_contain(&pane_id_to_replace) {
            self.overlay_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
                .ok()
        } else {
            self.tiled_panes
                .replace_pane(pane_id_to_replace, pane_to_replace_with)
        };

        if let Some(replaced_pane) = replaced_pane.take() {
            // parked rather than suppressed behind its replacement: this one is not
            // restored by the replacement closing, it has to be asked for by name
            self.park_pane(replaced_pane);
        }
    }
    pub fn horizontal_split(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context =
            || format!("failed to split pane {pid:?} horizontally for client {client_id}");
        if self.floating_panes.panes_are_visible() {
            return Ok(());
        }
        self.close_down_to_max_terminals()
            .with_context(err_context)?;
        if self.tiled_panes.fullscreen_is_active() {
            self.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        if self.tiled_panes.can_split_pane_horizontally(client_id) {
            if let PaneId::Terminal(term_pid) = pid {
                let next_terminal_position = self.get_next_terminal_position();
                let mut new_terminal = TerminalPane::new(
                    term_pid,
                    PaneGeom::default(), // the initial size will be set later
                    self.style,
                    next_terminal_position,
                    initial_pane_title.clone().unwrap_or_default(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    initial_pane_title,
                    invoked_with,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                    self.osc8_hyperlinks,
                    self.explicitly_disable_kitty_keyboard_protocol,
                    completion_tx,
                );
                if let Some(supported) = self.kitty_host_support {
                    new_terminal.update_kitty_host_support(supported);
                }
                if let Some(supported) = self.sixel_host_support {
                    new_terminal.update_sixel_host_support(supported);
                }
                if let Some(borderless) = borderless {
                    new_terminal.set_borderless(borderless);
                }
                self.tiled_panes
                    .split_pane_horizontally(pid, Box::new(new_terminal), client_id);
                self.set_should_clear_display_before_rendering();
                self.tiled_panes.focus_pane(pid, client_id);
                self.swap_layouts.set_is_tiled_damaged();
            }
        } else {
            log::error!("No room to split pane horizontally");
            if let Some(active_pane_id) = self.tiled_panes.get_active_pane_id(client_id) {
                self.senders
                    .send_to_background_jobs(BackgroundJob::DisplayPaneError(
                        vec![active_pane_id],
                        "CAN'T SPLIT!".into(),
                    ))
                    .with_context(err_context)?;
            }
            self.senders
                .send_to_pty(PtyInstruction::ClosePane(pid, completion_tx))
                .with_context(err_context)?;
            return Ok(());
        }
        Ok(())
    }
    pub fn vertical_split(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context =
            || format!("failed to split pane {pid:?} vertically for client {client_id}");
        if self.floating_panes.panes_are_visible() {
            return Ok(());
        }
        self.close_down_to_max_terminals()
            .with_context(err_context)?;
        if self.tiled_panes.fullscreen_is_active() {
            self.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        if self.tiled_panes.can_split_pane_vertically(client_id) {
            if let PaneId::Terminal(term_pid) = pid {
                let next_terminal_position = self.get_next_terminal_position();
                let mut new_terminal = TerminalPane::new(
                    term_pid,
                    PaneGeom::default(), // the initial size will be set later
                    self.style,
                    next_terminal_position,
                    initial_pane_title.clone().unwrap_or_default(),
                    self.link_handler.clone(),
                    self.character_cell_size.clone(),
                    self.sixel_image_store.clone(),
                    self.kitty_image_store.clone(),
                    self.terminal_emulator_colors.clone(),
                    self.terminal_emulator_color_codes.clone(),
                    initial_pane_title,
                    invoked_with,
                    self.debug,
                    self.arrow_fonts,
                    self.styled_underlines,
                    self.osc8_hyperlinks,
                    self.explicitly_disable_kitty_keyboard_protocol,
                    completion_tx,
                );
                if let Some(supported) = self.kitty_host_support {
                    new_terminal.update_kitty_host_support(supported);
                }
                if let Some(supported) = self.sixel_host_support {
                    new_terminal.update_sixel_host_support(supported);
                }
                if let Some(borderless) = borderless {
                    new_terminal.set_borderless(borderless);
                }
                self.tiled_panes
                    .split_pane_vertically(pid, Box::new(new_terminal), client_id);
                self.set_should_clear_display_before_rendering();
                self.tiled_panes.focus_pane(pid, client_id);
                self.swap_layouts.set_is_tiled_damaged();
            }
        } else {
            log::error!("No room to split pane vertically");
            if let Some(active_pane_id) = self.tiled_panes.get_active_pane_id(client_id) {
                self.senders
                    .send_to_background_jobs(BackgroundJob::DisplayPaneError(
                        vec![active_pane_id],
                        "CAN'T SPLIT!".into(),
                    ))
                    .with_context(err_context)?;
            }
            self.senders
                .send_to_pty(PtyInstruction::ClosePane(pid, completion_tx))
                .with_context(err_context)?;
            return Ok(());
        }
        Ok(())
    }

    pub fn horizontal_split_of_pane_id(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        target_pane_id: PaneId,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        self.split_of_pane_id(
            pid,
            initial_pane_title,
            invoked_with,
            target_pane_id,
            SplitDirection::Horizontal,
            completion_tx,
            borderless,
        )
    }
    pub fn vertical_split_of_pane_id(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        target_pane_id: PaneId,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        self.split_of_pane_id(
            pid,
            initial_pane_title,
            invoked_with,
            target_pane_id,
            SplitDirection::Vertical,
            completion_tx,
            borderless,
        )
    }
    fn split_of_pane_id(
        &mut self,
        pid: PaneId,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        target_pane_id: PaneId,
        split_direction: SplitDirection,
        completion_tx: Option<NotificationEnd>,
        borderless: Option<bool>,
    ) -> Result<()> {
        let err_context =
            || format!("failed to split pane {target_pane_id:?} without changing focus");
        if self.floating_panes.panes_are_visible() {
            return Ok(());
        }
        if !self.tiled_panes.panes_contain(&target_pane_id) {
            return Ok(());
        }
        self.close_down_to_max_terminals()
            .with_context(err_context)?;
        if self.tiled_panes.fullscreen_is_active() {
            self.tiled_panes.unset_fullscreen();
        }
        self.dissolve_stack_lists_for_classic_mutation();
        let can_split = match split_direction {
            SplitDirection::Horizontal => self
                .tiled_panes
                .can_split_pane_id_horizontally(target_pane_id),
            SplitDirection::Vertical => self
                .tiled_panes
                .can_split_pane_id_vertically(target_pane_id),
        };
        if !can_split {
            self.senders
                .send_to_pty(PtyInstruction::ClosePane(pid, completion_tx))
                .with_context(err_context)?;
            return Ok(());
        }
        if let PaneId::Terminal(term_pid) = pid {
            let next_terminal_position = self.get_next_terminal_position();
            let mut new_terminal = TerminalPane::new(
                term_pid,
                PaneGeom::default(),
                self.style,
                next_terminal_position,
                initial_pane_title.clone().unwrap_or_default(),
                self.link_handler.clone(),
                self.character_cell_size.clone(),
                self.sixel_image_store.clone(),
                self.kitty_image_store.clone(),
                self.terminal_emulator_colors.clone(),
                self.terminal_emulator_color_codes.clone(),
                initial_pane_title,
                invoked_with,
                self.debug,
                self.arrow_fonts,
                self.styled_underlines,
                self.osc8_hyperlinks,
                self.explicitly_disable_kitty_keyboard_protocol,
                completion_tx,
            );
            if let Some(supported) = self.kitty_host_support {
                new_terminal.update_kitty_host_support(supported);
            }
            if let Some(supported) = self.sixel_host_support {
                new_terminal.update_sixel_host_support(supported);
            }
            if let Some(borderless) = borderless {
                new_terminal.set_borderless(borderless);
            }
            match split_direction {
                SplitDirection::Horizontal => self.tiled_panes.split_pane_id_horizontally(
                    pid,
                    Box::new(new_terminal),
                    target_pane_id,
                ),
                SplitDirection::Vertical => self.tiled_panes.split_pane_id_vertically(
                    pid,
                    Box::new(new_terminal),
                    target_pane_id,
                ),
            }
            self.set_should_clear_display_before_rendering();
            self.swap_layouts.set_is_tiled_damaged();
        }
        Ok(())
    }
}
