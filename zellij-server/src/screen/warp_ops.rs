use zellij_utils::errors::prelude::*;

use super::{PreparedTabWarp, Screen, ScreenInstruction, WarpAddress};
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty::{
    AdoptedPane, AdoptedTabPanes, AdoptedTabTerminal, ClientTabIndexOrPaneId, PtyInstruction,
};
use crate::pty_host::client::ReleasedPane;
use crate::session_layout_metadata::PaneLayoutMetadata;
use crate::ClientId;
use std::collections::HashMap;
use std::path::PathBuf;
use zellij_utils::data::{
    NewPanePlacement, RemotePaneHandoff, WarpPlacement, WarpedPane, WarpedTab,
};
use zellij_utils::input::command::RunCommand;
use zellij_utils::input::layout::Run;
use zellij_utils::session_serialization::PaneLayoutManifest;
use zellij_utils::warped_tab::layout_for_warped_tab;

impl Screen {
    /// Keep a warped pane's screen dump inside something a message bus should be
    /// asked to carry.
    ///
    /// A 10,000-line scrollback with heavy styling runs to tens of megabytes, and
    /// it is copied several times between the two processes. Truncating from the
    /// *front* is the safe end: the receiving side feeds this to a VTE parser,
    /// which resumes mid-stream quite happily — what is lost is the styling state
    /// established before the cut, not the integrity of what follows.
    fn fit_to_warp_budget(contents: String) -> String {
        Self::fit_to_budget(contents, Self::WARP_CONTENTS_BUDGET)
    }

    /// Roughly 200 columns of styled text over a few thousand lines. Well inside
    /// `MAX_IPC_FRAME_LEN`, and far past what anyone reads back.
    pub(super) const WARP_CONTENTS_BUDGET: usize = 4 * 1024 * 1024;

    /// What one pane of a warped tab may spend on its scrollback.
    ///
    /// A whole tab travels in a single frame, so the per-pane budget cannot also
    /// be the whole budget: sixteen panes at 4 MiB each would be exactly
    /// `MAX_IPC_FRAME_LEN`, and the seventeenth would make the frame
    /// unsendable — losing the tab at the last step, after every supervisor had
    /// already been released. Sharing a fixed allowance out means a tab of any
    /// size fits, and only the scrollback gets shorter.
    pub(super) fn warp_budget_per_pane(panes: usize) -> usize {
        /// Half of `MAX_IPC_FRAME_LEN`, leaving the layout, the titles and
        /// protobuf's own framing somewhere to live.
        const WARP_TAB_BUDGET: usize = 32 * 1024 * 1024;

        (WARP_TAB_BUDGET / panes.max(1)).min(Self::WARP_CONTENTS_BUDGET)
    }

    pub(super) fn fit_to_budget(contents: String, budget: usize) -> String {
        if contents.len() <= budget {
            return contents;
        }
        let mut cut = contents.len() - budget;
        // Never split a character; a lone continuation byte would desync the
        // parser on the far side.
        while cut < contents.len() && !contents.is_char_boundary(cut) {
            cut += 1;
        }
        log::warn!(
            "a warped pane's scrollback was {} bytes; keeping the most recent {}",
            contents.len(),
            contents.len() - cut
        );
        contents[cut..].to_owned()
    }

    /// Decide whether a pane may be moved to another session, and say which
    /// terminal it is if so.
    ///
    /// Every reason a warp cannot work is checked here, before the pty thread is
    /// asked for anything. Once the supervisor is released the pane is briefly in
    /// nobody's hands, so nothing that could have been decided beforehand should
    /// be discovered then.
    pub(super) fn check_pane_can_be_warped(
        &mut self,
        pane_id: Option<PaneId>,
        client_id: ClientId,
    ) -> Result<u32, String> {
        let pane_id = match pane_id {
            Some(pane_id) => pane_id,
            None => self
                .active_pane_id(client_id)
                .ok_or_else(|| "no pane is focused".to_owned())?,
        };
        let terminal_id = match pane_id {
            PaneId::Terminal(terminal_id) => terminal_id,
            PaneId::Plugin(_) => {
                return Err(
                    "this is a plugin pane; it has no process, so there is nothing to move"
                        .to_owned(),
                );
            },
        };

        // `get_pane_with_id` rather than a walk of the ids, because it is what
        // `take_released_pane_for_warp` looks the pane up with once the supervisor
        // is gone. Any pane found here is findable then, suppressed or not - which
        // is the whole reason a suppressed pane can be moved at all.
        let panes_leaving = match self
            .tabs
            .values()
            .find(|tab| tab.get_pane_with_id(pane_id).is_some())
        {
            // A scrollback editor goes with the pane it is editing: it would
            // otherwise be left holding a dump of a pane that is in another
            // session now, and restoring nothing when closed.
            Some(tab) => 1 + usize::from(tab.scrollback_editor_standing_in_for(pane_id).is_some()),
            None => return Err(format!("there is no pane {terminal_id} in this session")),
        };

        // Warping the session's last pane would end the session, which is not what
        // moving a pane should mean. A tab's last pane is fine — the render pass
        // sweeps tabs with nothing selectable left in them.
        if self.selectable_terminal_pane_count() <= panes_leaving {
            return Err(if panes_leaving > 1 {
                "this pane and the scrollback editor open on it are the session's last panes; \
                 moving it would close the session"
                    .to_owned()
            } else {
                "this is the session's only pane; moving it would close the session".to_owned()
            });
        }
        Ok(terminal_id)
    }

    /// Panes with a process in them, whether or not anything on screen shows them.
    ///
    /// Suppressed panes count: a pane hidden behind a scrollback editor, or parked
    /// as a stack-list member, is still a pane this session would be left with.
    fn selectable_terminal_pane_count(&self) -> usize {
        self.tabs
            .values()
            .map(|tab| {
                tab.get_selectable_tiled_panes_count()
                    + tab.get_visible_selectable_floating_panes_count()
                    + tab.suppressed_selectable_terminal_count()
            })
            .sum()
    }

    /// How a released pane says where its process can be picked up.
    ///
    /// The one thing a `WarpedPane` cannot get from the tab. A local supervisor
    /// that never bound a socket has no address to hand out — but the pty
    /// thread's pre-flight check has already refused that pane before releasing
    /// anything, so reaching the refusal below means something changed between
    /// the check and the release. It is kept as the backstop for that, not as
    /// the place the case is expected to be caught.
    ///
    /// The check has no remote counterpart — a broker is reached by ssh and a
    /// token, and a pane the broker confirmed it released is reachable by
    /// definition.
    fn warp_address(released: ReleasedPane) -> Result<WarpAddress, String> {
        match released {
            ReleasedPane::Local(local) => {
                let Some(control_socket) = local.control_socket else {
                    // No address to hand out. The pty thread refuses this before
                    // releasing anything, so a pane that gets here has already
                    // been cut loose and has been told so in its own scrollback;
                    // there is nothing left for this branch to do but say why.
                    return Err(format!(
                        "the supervisor for this pane ({}) never bound a socket, so no other \
                         session can reach it — this usually means the runtime directory path \
                         is too long",
                        local.host_id
                    ));
                };
                Ok(WarpAddress {
                    host_id: local.host_id,
                    control_socket,
                    remote: None,
                })
            },
            ReleasedPane::Remote(remote) => Ok(WarpAddress {
                // Both empty on purpose rather than filled with something
                // plausible: they name a local supervisor socket, and reading
                // them for a remote pane is a mistake `remote` being `Some` is
                // there to catch.
                host_id: String::new(),
                control_socket: PathBuf::new(),
                remote: Some(RemotePaneHandoff {
                    host: remote.host,
                    pane_token: remote.pane_token,
                    received: remote.received,
                    cmd: remote.cmd,
                }),
            }),
        }
    }

    /// Serialize the released pane and take it out of its tab.
    ///
    /// Note what is *not* here for the warped pane: `PtyInstruction::ClosePane`.
    /// That is what the `ScreenInstruction::ClosePane` handler sends, and it
    /// SIGHUPs the child and terminates the supervisor. `Tab::close_pane` itself
    /// touches nothing on the pty thread — it just tidies the tab up, moves
    /// clients off the pane and relayouts, which is exactly what is wanted when a
    /// pane leaves. A scrollback editor left behind by the pane it was editing is
    /// the exception: that one is being closed rather than moved, so it is closed
    /// the way anything else is.
    pub(super) fn take_released_pane_for_warp(
        &mut self,
        terminal_id: u32,
        released: Result<ReleasedPane, String>,
    ) -> Result<Box<WarpedPane>, String> {
        let address = Self::warp_address(released?)?;
        let pane_id = PaneId::Terminal(terminal_id);

        let mut payload = None;
        let mut editor_left_behind = None;
        for tab in self.tabs.values_mut() {
            let scrollback_lines = self.scrollback_lines_to_serialize;
            let Some(pane) = tab.get_pane_with_id(pane_id) else {
                continue;
            };
            // Not gated on `serialize_pane_viewport`: that option governs
            // automatic session serialization, and a warp is something the user
            // asked for by name.
            let contents = pane
                .serialize(scrollback_lines)
                .map(Self::fit_to_warp_budget);
            let title = Some(pane.current_title());
            let borderless = pane.borderless();
            // Asked before the pane goes, closed after: while the pane is still
            // suppressed behind it, closing the editor would swap the pane back
            // into the grid rather than close anything.
            editor_left_behind = tab.scrollback_editor_standing_in_for(pane_id);
            tab.close_pane(pane_id, false, None);
            if let Some(editor_pane_id) = editor_left_behind {
                tab.close_pane(editor_pane_id, false, None);
            }
            payload = Some(Box::new(WarpedPane {
                host_id: address.host_id.clone(),
                control_socket: address.control_socket.clone(),
                title,
                contents,
                borderless,
                remote: address.remote.clone(),
            }));
            break;
        }
        if let Some(editor_pane_id) = editor_left_behind {
            let _ = self
                .bus
                .senders
                .send_to_pty(PtyInstruction::ClosePane(editor_pane_id, None));
            self.clear_nested_guest(editor_pane_id);
        }
        self.clear_nested_guest(pane_id);
        payload.ok_or_else(|| {
            format!("pane {terminal_id} vanished while its supervisor was being released")
        })
    }

    /// Work out how a tab would be moved, so that everything which can fail has
    /// failed before anything is released.
    ///
    /// The arrangement is built *here*, not after the release, and that ordering
    /// is the point: a tab whose supervisors had already been let go by the time
    /// something failed would be left on screen with nothing attached to it. Not
    /// every tiling is a layout — see `tiled_layout_from_panegeoms_lossy` — but
    /// that is approximated rather than refused, so what is left to fail here is
    /// the checks above. Only the two things that genuinely cannot be known yet
    /// are left for later: which socket each supervisor ended up on, and what was
    /// on each pane's screen once its reader had stopped.
    ///
    /// The pane version of this also refuses the session's last pane, on the
    /// grounds that moving it would end the session. There is deliberately no
    /// equivalent here: a session whose only tab is asked for has been asked to
    /// give up everything, and answering "no" would leave the user with no way to
    /// say what they meant.
    ///
    /// `tab_index` is a **stable tab id**, not a display position — `self.tabs` is
    /// keyed by id, and the focused-tab default below reads `active_tab_ids`. The
    /// name is historical; a caller that hands over a position addresses whichever
    /// tab owns that number as an id, which diverges the moment a tab closes.
    pub(super) fn check_tab_can_be_warped(
        &mut self,
        tab_index: Option<usize>,
        client_id: ClientId,
    ) -> Result<(usize, Vec<u32>, PreparedTabWarp), String> {
        let tab_index = match tab_index {
            Some(tab_index) => tab_index,
            None => self
                .active_tab_ids
                .get(&client_id)
                .copied()
                .ok_or_else(|| "no tab is focused".to_owned())?,
        };
        let tab = self
            .tabs
            .get(&tab_index)
            .ok_or_else(|| format!("there is no tab {tab_index} in this session"))?;

        // A hidden terminal is refused unless it is a stack-list member. The
        // distinction is not whether it is hidden but *how*: a pane with a
        // stand-in is one half of a pair — a scrollback editor, or a pane replaced
        // in place — and closing the tab swaps the hidden one back in rather than
        // removing it, dismantling the tab around a pty nobody released. A
        // stack-list member is parked with nothing standing in for it, has a slot
        // in the layout the tab travels as (see `stack_list_serialization_geoms`),
        // and is taken out by `empty_and_close_tab` with everything else.
        //
        // Hidden *plugin* panes are exempt throughout: they have no process to
        // strand, and `close_tab_by_id` re-homes them into another tab.
        if tab.get_suppressed_panes().any(|(hidden, suppressed)| {
            matches!(hidden, PaneId::Terminal(_))
                && !(suppressed.restored_by.is_none() && tab.pane_is_stack_list_member(hidden))
        }) {
            return Err(
                "this tab has a pane hidden behind another one; close it before moving the tab"
                    .to_owned(),
            );
        }

        // Asked of the tab, and asked first: a tab with no process in it is not a
        // tab worth moving, and saying so is more use than the layout complaint
        // an empty tab would otherwise earn. What actually gets released is
        // decided further down, from the slots.
        //
        // Asked of the panes with a slot rather than of all of them: an overlay
        // does not travel, so a tab whose only terminal is an overlay has nothing
        // to move and should be told so here rather than warp with an empty
        // payload.
        if !tab
            .tiled_and_floating_pane_ids()
            .iter()
            .any(|pane_id| matches!(pane_id, PaneId::Terminal(_)))
        {
            return Err("this tab has no panes with a process to move".to_owned());
        }

        let name = Some(tab.name.clone()).filter(|name| !name.is_empty());
        let viewport = tab.size;
        // Contents are left out: the reader tasks are still running, so anything
        // serialized now would be missing whatever arrives between here and the
        // release. They are read again in `take_released_tab_for_warp`, by which
        // time the screen has stopped changing.
        let (tiled, floating, hide_floating_panes) =
            self.tab_panes_for_layout(tab_index, tab, false);
        let manifests =
            |panes: &[(PaneId, PaneLayoutMetadata)]| -> Vec<(PaneId, PaneLayoutManifest)> {
                panes
                    .iter()
                    .map(|(pane_id, pane)| {
                        let mut manifest: PaneLayoutManifest = pane.clone().into();
                        // `current_title`, not the manifest's own, which carries
                        // `custom_title` and is therefore usually empty — that is
                        // all a *layout* should pin down. A warp is moving the
                        // pane the user is looking at, so it should arrive
                        // captioned the way it was, and the layout's `name` is
                        // what the receiving session reads to caption it.
                        manifest.title = tab
                            .get_pane_with_id(*pane_id)
                            .map(|pane| pane.current_title())
                            .or(manifest.title);
                        (*pane_id, manifest)
                    })
                    .collect()
            };
        let tiled = manifests(&tiled);
        let floating = manifests(&floating);

        let arrangement = layout_for_warped_tab(
            &tiled
                .iter()
                .map(|(_, pane)| pane.clone())
                .collect::<Vec<_>>(),
            &floating
                .iter()
                .map(|(_, pane)| pane.clone())
                .collect::<Vec<_>>(),
            viewport,
        )?;

        let in_slot_order = |panes: &[(PaneId, PaneLayoutManifest)], order: &[usize]| {
            order.iter().map(|index| panes[*index].0).collect()
        };
        let tiled_panes: Vec<PaneId> = in_slot_order(&tiled, &arrangement.tiled_order);
        let floating_panes: Vec<PaneId> = in_slot_order(&floating, &arrangement.floating_order);

        // Released from the slots rather than from the tab's pane ids, so that the
        // two sets cannot drift apart. `take_released_tab_for_warp` builds the
        // payload by walking the slots, so a terminal released without one would
        // be handed over to nobody — and by then it is too late to say so.
        //
        // Plugin panes have no slot here on purpose. They have no process to hand
        // over, but they do have a `Run::Plugin` in the layout, so the receiving
        // session builds the same tab by starting them afresh.
        let terminal_ids: Vec<u32> = tiled_panes
            .iter()
            .chain(floating_panes.iter())
            .filter_map(|pane_id| match pane_id {
                PaneId::Terminal(terminal_id) => Some(*terminal_id),
                PaneId::Plugin(_) => None,
            })
            .collect();

        Ok((
            tab_index,
            terminal_ids,
            PreparedTabWarp {
                name,
                tiled_layout: arrangement.tiled_layout,
                floating_layout: arrangement.floating_layout,
                tiled_panes,
                floating_panes,
                hide_floating_panes,
            },
        ))
    }

    /// Fill in what only the release could tell us, and take the tab out of this
    /// session.
    ///
    /// Everything fallible about the *arrangement* was settled in
    /// `check_tab_can_be_warped`, before any supervisor was let go. What is left
    /// here genuinely could not have been done earlier: which socket each
    /// supervisor ended up on, and what each pane's screen finally said — which
    /// is only reliable now, because the release means the reader tasks are gone
    /// and everything they emitted is already queued ahead of this.
    ///
    /// Closing the tab is the other careful part. `close_tab_by_id` ends by
    /// telling the pty thread to close whatever panes are left, which would
    /// SIGHUP every child; taking the panes out first leaves it nothing to kill.
    pub(super) fn take_released_tab_for_warp(
        &mut self,
        tab_index: usize,
        released: Vec<(u32, Result<ReleasedPane, String>)>,
        prepared: PreparedTabWarp,
    ) -> Result<Box<WarpedTab>, String> {
        let mut addresses: HashMap<u32, WarpAddress> = HashMap::new();
        for (terminal_id, released) in released {
            // The refusal `warp_address` can produce is worded for one pane, and
            // a tab is refused whole for it. Reaching it at all means the pty
            // thread's pre-flight check passed and the release then failed, in
            // which case that thread has already told every pane in the tab what
            // happened to it — this is the reason travelling back to the caller,
            // not the pane's only account of itself.
            addresses.insert(terminal_id, Self::warp_address(released?)?);
        }

        // The budget is shared out only now, because only now is it known how
        // many panes are sharing it.
        let budget =
            Self::warp_budget_per_pane(prepared.tiled_panes.len() + prepared.floating_panes.len());
        let scrollback_lines = self.scrollback_lines_to_serialize;
        let tab = self
            .tabs
            .get(&tab_index)
            .ok_or_else(|| format!("tab {tab_index} vanished while it was being released"))?;
        let warped_pane = |pane_id: &PaneId| {
            // A plugin pane has no process and so no address; it rides along as
            // its `Run::Plugin` layout node and starts afresh in the same slot.
            let address = match pane_id {
                PaneId::Terminal(terminal_id) => addresses.get(terminal_id).cloned(),
                PaneId::Plugin(_) => None,
            }
            .unwrap_or_default();
            WarpedPane {
                host_id: address.host_id,
                control_socket: address.control_socket,
                remote: address.remote,
                // A tab's titles and borders ride in its layout, which is what
                // the receiving session reads when it builds the panes. Repeating
                // them here would be a second answer to the same question.
                title: None,
                borderless: false,
                // Not gated on `serialize_pane_viewport`: that option governs
                // automatic session serialization, and a warp is something the
                // user asked for by name.
                contents: tab
                    .get_pane_with_id(*pane_id)
                    .and_then(|pane| pane.serialize(scrollback_lines))
                    .map(|contents| Self::fit_to_budget(contents, budget)),
            }
        };
        let tiled_panes: Vec<WarpedPane> = prepared.tiled_panes.iter().map(warped_pane).collect();
        let floating_panes: Vec<WarpedPane> =
            prepared.floating_panes.iter().map(warped_pane).collect();

        self.empty_and_close_tab(tab_index);

        Ok(Box::new(WarpedTab {
            name: prepared.name,
            tiled_layout: prepared.tiled_layout,
            floating_layout: prepared.floating_layout,
            tiled_panes,
            floating_panes,
            hide_floating_panes: prepared.hide_floating_panes,
        }))
    }

    /// Take a warped tab's own panes out of it and then close it.
    ///
    /// `close_tab_by_id` ends by telling the pty thread to close whatever panes
    /// are left, which for a warped tab would kill the processes that are the
    /// whole point of moving it. Emptying the tab first leaves that instruction
    /// with nothing to act on.
    ///
    /// The tab's own panes, which is not the same as `get_all_pane_ids`: that also
    /// reports the suppressed *plugin* panes and the overlays, and closing one
    /// would take it away from the whole session rather than from this tab.
    /// `close_tab_by_id` already knows to re-home both. The suppressed panes that do have to go are the
    /// hidden members of the tab's stack lists — they are travelling, and being
    /// re-homed would leave them in another tab with their supervisors handed away.
    pub(super) fn empty_and_close_tab(&mut self, tab_index: usize) {
        let mut emptied = vec![];
        if let Some(tab) = self.tabs.get_mut(&tab_index) {
            // Hidden stack-list members first: closing a list's *visible* member
            // promotes a hidden one into the grid, so sweeping the tiled panes
            // while members were still parked would keep handing the sweep panes
            // it had already passed. `close_pane` routes each one through
            // `close_stack_list_member`, which does the list's bookkeeping and,
            // like the rest of this function, tells the pty thread nothing.
            let hidden_members: Vec<PaneId> = tab
                .suppressed_stack_list_members()
                .map(|(pane_id, _)| *pane_id)
                .collect();
            for pane_id in hidden_members {
                tab.close_pane(pane_id, false, None);
                emptied.push(pane_id);
            }
            // Only what was released, which is what the tiled and floating slots were built from.
            // An overlay has no slot and so was never handed over (see
            // `Tab::tiled_and_floating_pane_ids`); taking it out here would remove it from the tab
            // before `close_tab_by_id` could see it, and that is the one thing that knows what to
            // do with one. Left where it is, it is re-homed into a surviving tab
            // (`Screen::move_overlay_panes_from_closed_tab`) - which is right for a warp too: an
            // overlay is a local plugin instance with no process to hand over, so it stays in this
            // session rather than travelling with the tab.
            for pane_id in tab.tiled_and_floating_pane_ids() {
                tab.close_pane(pane_id, false, None);
                emptied.push(pane_id);
            }
        }
        for pane_id in emptied {
            self.clear_nested_guest(pane_id);
        }
        self.close_tab_by_id(tab_index).non_fatal();
    }

    /// Give an adopted supervisor a pane in this session.
    ///
    /// Deliberately expressed as the ordinary pane-creation instructions rather
    /// than by building a `TerminalPane` here: a pane object is full of
    /// per-session `Rc`s and could never have been carried across anyway, and
    /// `NewPane` already knows how to place one.
    pub(super) fn place_adopted_pane(
        &mut self,
        terminal_id: u32,
        payload: WarpedPane,
        placement: WarpPlacement,
        client_id: ClientId,
        cmd: RunCommand,
        buffered: Vec<u8>,
        child_exit_status: Option<Option<i32>>,
    ) -> Result<()> {
        let err_context = || format!("failed to place adopted pane {terminal_id}");
        let pane_id = PaneId::Terminal(terminal_id);
        let invoked_with = Some(Run::Command(cmd.clone()));
        // A pane whose child is already gone should arrive held, showing its exit
        // status, rather than pretending to run.
        let hold_for_command = child_exit_status.map(|_| cmd.clone());

        let (placement, tab_target) = match placement {
            WarpPlacement::Floating => (
                NewPanePlacement::Floating(None),
                ClientTabIndexOrPaneId::ClientId(client_id),
            ),
            WarpPlacement::FocusedTab => (
                NewPanePlacement::Tiled {
                    direction: None,
                    borderless: Some(payload.borderless),
                },
                ClientTabIndexOrPaneId::ClientId(client_id),
            ),
            WarpPlacement::NewTab => {
                let tab_index = self.new_tab_for_adopted_pane(invoked_with.clone(), client_id)?;
                (
                    NewPanePlacement::Tiled {
                        direction: None,
                        borderless: Some(payload.borderless),
                    },
                    ClientTabIndexOrPaneId::TabIndex(tab_index),
                )
            },
        };
        self.bus
            .senders
            .send_to_screen(ScreenInstruction::NewPane(
                pane_id,
                payload.title.clone(),
                hold_for_command,
                invoked_with,
                placement,
                false,
                tab_target,
                None,
                false,
            ))
            .with_context(err_context)?;

        // The transferred screen first, then whatever the supervisor caught during
        // the hand-off, then live output. `NewPane` is ahead of these on the same
        // channel, and `pending_events_waiting_for_pane` would buffer them anyway.
        if let Some(contents) = payload.contents {
            if !contents.is_empty() {
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::PtyBytes(
                        terminal_id,
                        contents.into_bytes(),
                    ))
                    .with_context(err_context)?;
            }
        }
        if !buffered.is_empty() {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::PtyBytes(terminal_id, buffered))
                .with_context(err_context)?;
        }
        if let Some(exit_status) = child_exit_status {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::HoldPane(pane_id, exit_status, cmd))
                .with_context(err_context)?;
        }
        Ok(())
    }

    /// A tab of its own for an adopted pane, following `break_pane`'s dance.
    ///
    /// `ignore_run_instruction` is the load-bearing part: without it the new tab's
    /// layout template also spawns a default shell, and the user gets two panes
    /// where they asked for one.
    fn new_tab_for_adopted_pane(
        &mut self,
        invoked_with: Option<Run>,
        client_id: ClientId,
    ) -> Result<usize> {
        let err_context = || "failed to open a tab for an adopted pane".to_string();
        let default_layout = self.default_layout.clone();
        let tab_index = self.get_new_tab_id();
        let swap_layouts = (
            default_layout.swap_tiled_layouts.clone(),
            default_layout.swap_floating_layouts.clone(),
        );
        self.new_tab(tab_index, swap_layouts, None, Some(client_id))?;
        let (mut tiled_panes_layout, floating_panes_layout) = default_layout.new_tab();
        tiled_panes_layout.ignore_run_instruction(invoked_with);
        let is_web_client = self
            .connected_clients
            .borrow()
            .get(&client_id)
            .copied()
            .unwrap_or(false);
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::NewTab(
                None,
                None,
                Some(tiled_panes_layout),
                floating_panes_layout,
                tab_index,
                None,
                false,
                true,
                (client_id, is_web_client),
                None,
            ))
            .with_context(err_context)?;
        Ok(tab_index)
    }

    /// Build a tab in this session around supervisors the pty thread has already
    /// adopted.
    ///
    /// Expressed as the ordinary new-tab flow rather than by assembling a `Tab`
    /// here, for the same reason `place_adopted_pane` uses `NewPane`: that flow
    /// already knows how to do all of it. Going out through the plugin thread is
    /// what rebuilds the tab's plugin panes — they arrive as `Run::Plugin` nodes
    /// in the layout and come back with plugin ids attached — and the pty thread
    /// then fills the layout's terminal slots from the queue left for it, rather
    /// than spawning anything.
    ///
    /// The tab's name is set afterwards rather than passed in, because the name
    /// travels with the tab while the layout does not carry one.
    pub(super) fn place_adopted_tab(
        &mut self,
        payload: WarpedTab,
        adopted: AdoptedTabPanes,
        client_id: ClientId,
    ) -> Result<()> {
        let err_context = || "failed to place an adopted tab".to_string();
        let tab_index = self.get_new_tab_id();
        let default_layout = self.default_layout.clone();
        let swap_layouts = (
            default_layout.swap_tiled_layouts.clone(),
            default_layout.swap_floating_layouts.clone(),
        );
        self.new_tab(
            tab_index,
            swap_layouts,
            payload.name.clone(),
            Some(client_id),
        )?;

        let is_web_client = self
            .connected_clients
            .borrow()
            .get(&client_id)
            .copied()
            .unwrap_or(false);
        let mut tiled_layout = payload.tiled_layout;
        tiled_layout.hide_floating_panes = payload.hide_floating_panes;

        // Set the terminals aside for this tab *before* asking for it. The
        // request goes out through the plugin thread and comes back to the pty
        // thread, so anything the user opens in the meantime arrives on that
        // same channel — and would otherwise be the tab that got these panes.
        // Sending this first is what rules that out: it is enqueued before the
        // plugin thread has even been asked, so it cannot arrive late.
        self.bus
            .senders
            .send_to_pty(PtyInstruction::ClaimAdoptedTab {
                tab_index,
                terminals: (
                    Self::adopted_terminals(payload.tiled_panes, adopted.tiled),
                    Self::adopted_terminals(payload.floating_panes, adopted.floating),
                ),
            })
            .with_context(err_context)?;
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::NewTab(
                None,
                None,
                Some(tiled_layout),
                payload.floating_layout,
                tab_index,
                None,
                false,
                true,
                (client_id, is_web_client),
                None,
            ))
            .with_context(err_context)?;

        Ok(())
    }

    /// Pair each pane's transferred screen with the supervisor that was adopted
    /// for it, ready for the pty thread to replay once the tab exists.
    ///
    /// The two lists are in the same order by construction: the pty thread
    /// adopted them in the order they arrived in, and that order is the one the
    /// layout's slots are filled in.
    fn adopted_terminals(
        panes: Vec<WarpedPane>,
        adopted: Vec<AdoptedPane>,
    ) -> Vec<AdoptedTabTerminal> {
        panes
            .into_iter()
            .zip(adopted)
            .map(|(pane, adopted)| {
                // The transferred screen first, then whatever the supervisor
                // caught during the hand-off — the same order a single adopted
                // pane is replayed in, and the same order it happened in.
                let mut replay = pane.contents.map(String::into_bytes).unwrap_or_default();
                replay.extend(adopted.buffered);
                AdoptedTabTerminal {
                    terminal_id: adopted.terminal_id,
                    cmd: adopted.cmd,
                    child_exit_status: adopted.child_exit_status,
                    replay,
                }
            })
            .collect()
    }
}
