use zellij_runtime::errors::prelude::*;
use zellij_types::internal_ids::SupervisorId;
use zellij_types::internal_ids::TabId;
use zellij_types::internal_ids::TerminalId;

use super::{PreparedTabWarp, Screen, ScreenInstruction, WarpAddress};
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty::{
    held_command_on_exit, terminal_exit_instruction, AdoptedPane, AdoptedTabPanes,
    AdoptedTabTerminal, ClientTabIndexOrPaneId, PtyInstruction,
};
use crate::pty_host::client::ReleasedPane;
use crate::session_layout_metadata::PaneLayoutMetadata;
use crate::tab::Pane;
use crate::ClientId;
use std::collections::HashMap;
use std::path::PathBuf;
use zellij_session::warped_tab::layout_for_warped_tab;
use zellij_types::input::command::RunCommand;
use zellij_types::input::layout::Run;
use zellij_types::session_manifest::PaneLayoutManifest;
use {
    zellij_types::data::NewPanePlacement, zellij_types::data::RemotePaneHandoff,
    zellij_types::data::WarpPlacement, zellij_types::data::WarpedPane,
    zellij_types::data::WarpedTab,
};

/// What Screen keeps back from a tab warp's budget for its own work.
///
/// Once the pty thread reports the release, this thread still has to serialize
/// every pane in the tab — up to 32 MiB of scrollback across the lot, see
/// [`Screen::warp_budget_per_pane`] — build the payload and close the tab, all
/// before the caller's `recv_timeout` fires. Handing the pty thread the caller's
/// whole deadline would mean a release that finished exactly on time still
/// answered too late, and a tab released into a channel nobody is reading is the
/// one outcome this feature must never produce.
///
/// Generous rather than measured: the cost of reserving too much is a warp
/// refused a little early, and the cost of reserving too little is a tab full of
/// orphaned processes.
pub(super) const TAB_WARP_SCREEN_RESERVE: std::time::Duration =
    std::time::Duration::from_millis(1500);

/// The single-pane version of [`TAB_WARP_SCREEN_RESERVE`], and smaller for the
/// obvious reason: one pane's scrollback is capped at
/// [`Screen::WARP_CONTENTS_BUDGET`] rather than a whole tab's worth.
pub(super) const PANE_WARP_SCREEN_RESERVE: std::time::Duration =
    std::time::Duration::from_millis(500);

impl Screen {
    fn notify_warp_departure(&mut self, pane_id: PaneId) {
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                zellij_types::data::Event::PaneClosed(pane_id.into()),
            )]))
            .non_fatal();
        self.notify_pane_closed_to_subscribers(pane_id.into());
    }

    pub(super) fn finish_warp_group(
        &mut self,
        tab_id: TabId,
        slots: &[PaneId],
        relationships: &zellij_types::data::WarpRelationships,
    ) -> Result<()> {
        use crate::tab::{AcceptedPane, ExtractedPanePlacement, SuppressedPane};
        let Some((placement, target, client_id, response_channel)) =
            self.pending_group_placements.remove(&tab_id)
        else {
            return Ok(());
        };
        let result = (|| {
            if placement == WarpPlacement::NewTab {
                return Ok(());
            }
            let Some(target) = target.filter(|id| self.tabs.contains_key(id)) else {
                return Ok(());
            };
            let source = self
                .tabs
                .get_mut(&tab_id)
                .context("missing adopted group tab")?;
            let mut panes = source.extract_warp_panes(slots).into_iter();
            let root = panes.next().context("missing adopted group root")?;
            let placement = if placement == WarpPlacement::Floating {
                let min_height = relationships
                    .stacks
                    .iter()
                    .find(|stack| stack.selected == 0)
                    // Headers, padding, bottom frame, and at least one content row.
                    .map_or(0, |stack| stack.members.len() + 3);
                ExtractedPanePlacement::FloatingDefault { min_height }
            } else {
                ExtractedPanePlacement::Tiled
            };
            let destination = self
                .tabs
                .get_mut(&target)
                .context("missing warp destination")?;
            match destination.accept_extracted_pane(root, placement) {
                AcceptedPane::Accepted => {},
                AcceptedPane::NoRoom(root) => {
                    let source = self
                        .tabs
                        .get_mut(&tab_id)
                        .context("missing group recovery tab")?;
                    source.add_tiled_pane(root, slots[0], true, Some(client_id))?;
                    source.add_suppressed_panes(
                        panes
                            .map(|pane| {
                                (
                                    pane.pid(),
                                    SuppressedPane {
                                        pane,
                                        restored_by: None,
                                    },
                                )
                            })
                            .collect(),
                    );
                    source.restore_warp_relationships(slots, relationships)?;
                    // Retaining the adopted group is success; the source must not roll it back.
                    return Ok(());
                },
                AcceptedPane::LostInAdd => {
                    return Err(anyhow!("the destination lost the adopted root pane"))
                },
            }
            destination.add_suppressed_panes(
                panes
                    .map(|pane| {
                        (
                            pane.pid(),
                            SuppressedPane {
                                pane,
                                restored_by: None,
                            },
                        )
                    })
                    .collect(),
            );
            destination.restore_warp_relationships(slots, relationships)?;
            destination.focus_pane_with_id(slots[0], false, false, client_id)?;
            self.close_tab_by_id(tab_id)?;
            if let Some(position) = self.get_tab_position_by_id(target) {
                self.switch_active_tab(position, None, true, client_id)?;
            }
            Ok(())
        })();
        if let Some(response_channel) = response_channel {
            let _ = response_channel.send(result);
            Ok(())
        } else {
            result
        }
    }

    pub(super) fn prepare_pane_group(
        &self,
        pane_id: Option<PaneId>,
        client_id: ClientId,
    ) -> Result<Option<(TabId, Vec<TerminalId>, PreparedTabWarp)>, String> {
        let pane_id = pane_id
            .or_else(|| self.active_pane_id(client_id))
            .ok_or_else(|| "no pane is focused".to_owned())?;
        let (tab_id, tab) = self
            .tabs
            .iter()
            .find(|(_, tab)| tab.get_pane_with_id(pane_id).is_some())
            .ok_or_else(|| format!("there is no pane {pane_id:?} in this session"))?;
        let ids = tab.warp_pane_ids(Some(pane_id))?;
        if ids.len() == 1 && matches!(pane_id, PaneId::Terminal(_)) {
            return Ok(None);
        }
        let leaving = ids
            .iter()
            .filter(|id| {
                tab.get_pane_with_id(**id)
                    .is_some_and(|pane| pane.selectable())
            })
            .count();
        if self.selectable_warp_pane_count()? <= leaving {
            return Err(
                "these are the session's last panes; moving them would close the session".into(),
            );
        }
        let mut slots = vec![pane_id];
        slots.extend(ids.into_iter().filter(|id| *id != pane_id));
        let root = tab
            .get_pane_with_id(pane_id)
            .ok_or_else(|| "warp root vanished".to_owned())?;
        let tiled_layout = zellij_types::input::layout::TiledPaneLayout {
            run: root.invoked_with().clone(),
            name: carried_caption(root),
            borderless: Some(root.borderless()),
            ..Default::default()
        };
        let floating_layout = slots
            .iter()
            .skip(1)
            .map(|id| {
                let pane = tab
                    .get_pane_with_id(*id)
                    .ok_or_else(|| "warp companion vanished".to_owned())?;
                Ok(zellij_types::input::layout::FloatingPaneLayout {
                    run: pane.invoked_with().clone(),
                    name: carried_caption(pane),
                    borderless: Some(pane.borderless()),
                    ..Default::default()
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let relationships = tab.warp_relationships(&slots);
        relationships.validate(slots.len())?;
        let terminals = |panes: &[PaneId]| {
            panes
                .iter()
                .copied()
                .filter(|id| matches!(id, PaneId::Terminal(_)))
                .collect::<Vec<_>>()
        };
        let terminal_ids = slots
            .iter()
            .filter_map(|id| {
                if let PaneId::Terminal(id) = id {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        Ok(Some((
            *tab_id,
            terminal_ids,
            PreparedTabWarp {
                name: None,
                user_named: false,
                tiled_layout,
                floating_layout,
                tiled_panes: terminals(&slots[..1]),
                floating_panes: terminals(&slots[1..]),
                hide_floating_panes: true,
                relationships: Some(relationships),
                slots,
                pane_group: true,
            },
        )))
    }

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

    /// Check the ordinary terminal path after companion-group preparation.
    /// Hidden IDs are rejected here too, before asking the pty thread to release.
    pub(super) fn check_pane_can_be_warped(
        &mut self,
        pane_id: Option<PaneId>,
        client_id: ClientId,
    ) -> Result<TerminalId, String> {
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

        let tab = self
            .tabs
            .values()
            .find(|tab| tab.get_pane_with_id(pane_id).is_some())
            .ok_or_else(|| format!("there is no pane {terminal_id} in this session"))?;
        let panes_leaving = tab.warp_pane_ids(Some(pane_id))?.len();

        // Warping the session's last pane would end the session, which is not what
        // moving a pane should mean. A tab's last pane is fine — the render pass
        // sweeps tabs with nothing selectable left in them.
        if self.selectable_warp_pane_count()? <= panes_leaving {
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

    /// Count the same selectable content that pane and tab transfers discover.
    /// Independent background plugins are excluded, while companions count.
    fn selectable_warp_pane_count(&self) -> Result<usize, String> {
        self.tabs.values().try_fold(0, |count, tab| {
            Ok(count
                + tab
                    .warp_pane_ids(None)?
                    .iter()
                    .filter(|id| {
                        tab.get_pane_with_id(**id)
                            .is_some_and(|pane| pane.selectable())
                    })
                    .count())
        })
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
                host_id: SupervisorId::default(),
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

    /// Serialize an ordinary pane after its reader has stopped. Companion groups
    /// use the batch path below, including during rollback.
    pub(super) fn take_released_pane_for_warp(
        &mut self,
        terminal_id: TerminalId,
        released: Result<ReleasedPane, String>,
    ) -> Result<Box<WarpedPane>, String> {
        let address = Self::warp_address(released?)?;
        let pane_id = PaneId::Terminal(terminal_id);
        let scrollback_lines = self.scrollback_lines_to_serialize;
        let tab = self
            .tabs
            .values_mut()
            .find(|tab| tab.get_pane_with_id(pane_id).is_some())
            .ok_or_else(|| {
                format!("pane {terminal_id} vanished while its supervisor was being released")
            })?;
        let pane = tab
            .get_pane_with_id(pane_id)
            .ok_or_else(|| "warp pane vanished".to_owned())?;
        let payload = Box::new(WarpedPane {
            group: None,
            host_id: address.host_id,
            control_socket: address.control_socket,
            remote: address.remote,
            title: carried_caption(pane),
            caption: default_caption(pane),
            contents: pane
                .serialize(scrollback_lines)
                .map(Self::fit_to_warp_budget),
            borderless: pane.borderless(),
            agent: pane.agent_info().cloned(),
        });
        let removed = tab.extract_warp_panes(&[pane_id]);
        for pane in removed {
            self.notify_warp_departure(pane.pid());
        }
        self.clear_nested_guest(pane_id);
        Ok(payload)
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
        tab_index: Option<TabId>,
        client_id: ClientId,
    ) -> Result<(TabId, Vec<TerminalId>, PreparedTabWarp), String> {
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

        let transfer_ids = tab.warp_pane_ids(None)?;
        if transfer_ids.is_empty() {
            return Err("this tab has no panes to move".into());
        }

        let name = Some(tab.name.clone()).filter(|name| !name.is_empty());
        let user_named = tab.is_user_named();
        let viewport = tab.size;
        // Contents are left out: the reader tasks are still running, so anything
        // serialized now would be missing whatever arrives between here and the
        // release. They are read again in `take_released_tab_for_warp`, by which
        // time the screen has stopped changing.
        let (tiled, floating, hide_floating_panes) =
            self.tab_panes_for_layout(tab_index, tab, false, true);
        let manifests =
            |panes: &[(PaneId, PaneLayoutMetadata)]| -> Vec<(PaneId, PaneLayoutManifest)> {
                panes
                    .iter()
                    .map(|(pane_id, pane)| {
                        let mut manifest: PaneLayoutManifest = pane.clone().into();
                        // More than the manifest's own `custom_title`, which is
                        // all a *layout* should pin down: a warp is moving the
                        // pane the user is looking at, so a title its program set
                        // arrives too. See `carried_caption` for what stays behind.
                        manifest.title = tab
                            .get_pane_with_id(*pane_id)
                            .and_then(carried_caption)
                            .or(manifest.title);
                        (*pane_id, manifest)
                    })
                    .collect()
            };
        let tiled = manifests(&tiled);
        let floating = manifests(&floating);

        let mut arrangement = layout_for_warped_tab(
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

        let mut slots: Vec<PaneId> = arrangement
            .all_tiled_order
            .iter()
            .map(|i| tiled[*i].0)
            .chain(floating.iter().map(|(id, _)| *id))
            .collect();
        let tiled_panes = arrangement
            .tiled_order
            .iter()
            .map(|i| tiled[*i].0)
            .filter(|id| matches!(id, PaneId::Terminal(_)))
            .collect::<Vec<_>>();
        let mut floating_panes = arrangement
            .floating_order
            .iter()
            .map(|i| floating[*i].0)
            .filter(|id| matches!(id, PaneId::Terminal(_)))
            .collect::<Vec<_>>();
        for id in transfer_ids {
            if slots.contains(&id) {
                continue;
            }
            let pane = tab
                .get_pane_with_id(id)
                .ok_or_else(|| format!("missing companion {id:?}"))?;
            arrangement
                .floating_layout
                .push(zellij_types::input::layout::FloatingPaneLayout {
                    name: carried_caption(pane),
                    run: pane.invoked_with().clone(),
                    borderless: Some(pane.borderless()),
                    ..Default::default()
                });
            slots.push(id);
            if matches!(id, PaneId::Terminal(_)) {
                floating_panes.push(id);
            }
        }
        let relationships = tab.warp_relationships(&slots);
        relationships.validate(slots.len())?;
        let relationships = (!relationships.hidden.is_empty() || !relationships.stacks.is_empty())
            .then_some(relationships);

        // Released from the slots rather than from the tab's pane ids, so that the
        // two sets cannot drift apart. `take_released_tab_for_warp` builds the
        // payload by walking the slots, so a terminal released without one would
        // be handed over to nobody — and by then it is too late to say so.
        //
        // Plugin panes have no slot here on purpose. They have no process to hand
        // over, but they do have a `Run::Plugin` in the layout, so the receiving
        // session builds the same tab by starting them afresh.
        let terminal_ids: Vec<TerminalId> = tiled_panes
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
                relationships,
                slots,
                pane_group: false,
                name,
                user_named,
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
        tab_index: TabId,
        released: Vec<(TerminalId, Result<ReleasedPane, String>)>,
        prepared: PreparedTabWarp,
    ) -> Result<Box<WarpedTab>, String> {
        let mut addresses: HashMap<TerminalId, WarpAddress> = HashMap::new();
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
                group: None,
                host_id: address.host_id,
                control_socket: address.control_socket,
                remote: address.remote,
                // A tab's titles and borders ride in its layout, which is what
                // the receiving session reads when it builds the panes. Repeating
                // them here would be a second answer to the same question. A
                // default caption cannot ride there: a layout's name is explicit.
                title: None,
                caption: tab.get_pane_with_id(*pane_id).and_then(default_caption),
                borderless: false,
                // Not gated on `serialize_pane_viewport`: that option governs
                // automatic session serialization, and a warp is something the
                // user asked for by name.
                contents: tab
                    .get_pane_with_id(*pane_id)
                    .and_then(|pane| pane.serialize(scrollback_lines))
                    .map(|contents| Self::fit_to_budget(contents, budget)),
                agent: tab
                    .get_pane_with_id(*pane_id)
                    .and_then(|pane| pane.agent_info().cloned()),
            }
        };
        let tiled_panes: Vec<WarpedPane> = prepared.tiled_panes.iter().map(warped_pane).collect();
        let floating_panes: Vec<WarpedPane> =
            prepared.floating_panes.iter().map(warped_pane).collect();

        let tab = self
            .tabs
            .get_mut(&tab_index)
            .ok_or_else(|| "warp tab disappeared".to_owned())?;
        let removed = tab.extract_warp_panes(&prepared.slots);
        for pane in removed {
            let id = pane.pid();
            self.notify_warp_departure(id);
            if let PaneId::Plugin(plugin_id) = id {
                self.drop_quit_notice(plugin_id);
                self.bus
                    .senders
                    .send_to_pty(PtyInstruction::ClosePane(id, None))
                    .non_fatal();
            }
            self.clear_nested_guest(id);
        }
        if !prepared.pane_group {
            self.empty_and_close_tab(tab_index);
        }

        Ok(Box::new(WarpedTab {
            relationships: prepared.relationships,
            name: prepared.name,
            user_named: prepared.user_named,
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
    pub(super) fn empty_and_close_tab(&mut self, tab_index: TabId) {
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
            if let PaneId::Plugin(plugin_id) = pane_id {
                self.drop_quit_notice(plugin_id);
            }
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
        terminal_id: TerminalId,
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
        // status, rather than pretending to run — but only if it would have been
        // held in the session it came from. Otherwise it is closed below, exactly
        // as the supervisor's replayed exit would close it through `quit_cb`.
        let hold_for_command = held_command_on_exit(&cmd, child_exit_status);
        let focus_client = match placement {
            WarpPlacement::FocusedTab => self.connected_client_to_act_for(client_id),
            _ => None,
        };

        let (placement, tab_target) = match placement {
            WarpPlacement::Floating => (
                NewPanePlacement::Floating(None),
                ClientTabIndexOrPaneId::ClientId(client_id),
            ),
            // The tab is named rather than left as "wherever the client is": a warp's
            // placement is the user's explicit choice, and `agent_pane_target` re-routes a
            // client-relative agent pane out of a crowded tab while it honours a named one.
            // Resolved now, on the screen thread, it is still the tab the client is looking at.
            WarpPlacement::FocusedTab => (
                NewPanePlacement::Tiled {
                    direction: None,
                    borderless: Some(payload.borderless),
                },
                match self.tab_a_client_is_looking_at(client_id) {
                    Some(tab_index) => ClientTabIndexOrPaneId::TabIndex(tab_index),
                    None => ClientTabIndexOrPaneId::ClientId(client_id),
                },
            ),
            WarpPlacement::NewTab => {
                let tab_index =
                    self.new_tab_for_adopted_pane(invoked_with.clone(), client_id, true)?;
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

        if let Some(client_id) = focus_client {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::FocusPaneWithId(
                    pane_id, false, false, client_id, None,
                ))
                .with_context(err_context)?;
        }

        if let Some(agent) = payload.agent {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::SetPaneAgent(pane_id, Some(agent), None))
                .with_context(err_context)?;
        }
        if let Some(caption) = payload.caption {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::SetPaneCaption(pane_id, caption))
                .with_context(err_context)?;
        }

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
            let hold_on_close = cmd.hold_on_close;
            let instruction = terminal_exit_instruction(pane_id, exit_status, cmd, hold_on_close);
            self.bus
                .senders
                .send_to_screen(instruction)
                .with_context(err_context)?;
        }
        Ok(())
    }

    /// A tab of its own for an adopted pane, following `break_pane`'s dance. Also what a
    /// spawned agent pane gets when no tab has room for it (`agent_placement`), which is why
    /// the caller says whether the client follows the pane there.
    ///
    /// `ignore_run_instruction` is the load-bearing part: without it the new tab's
    /// layout template also spawns a default shell, and the user gets two panes
    /// where they asked for one.
    ///
    /// The client is seated in the tab only when it follows the pane there, as the
    /// `NewTab` arm in `thread_main` does: `apply_layout` moves the client out of its
    /// source tab only on a focus change, so seating it here for an unfocused spawn would
    /// leave it connected to two tabs at once.
    pub(super) fn new_tab_for_adopted_pane(
        &mut self,
        invoked_with: Option<Run>,
        client_id: ClientId,
        should_change_focus_to_new_tab: bool,
    ) -> Result<TabId> {
        let err_context = || "failed to open a tab for an adopted pane".to_string();
        let default_layout = self.default_layout.clone();
        let tab_index = self.get_new_tab_id();
        let swap_layouts = (
            default_layout.swap_tiled_layouts.clone(),
            default_layout.swap_floating_layouts.clone(),
        );
        let client_for_tab = should_change_focus_to_new_tab.then_some(client_id);
        self.new_tab(tab_index, swap_layouts, None, client_for_tab)?;
        let (mut tiled_panes_layout, floating_panes_layout) = default_layout.new_tab();
        tiled_panes_layout.ignore_run_instruction(invoked_with);
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::NewTab(
                None,
                None,
                Some(tiled_panes_layout),
                (!floating_panes_layout.is_empty()).then_some(floating_panes_layout),
                tab_index,
                None,
                false,
                should_change_focus_to_new_tab,
                client_id,
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
        placement: Option<WarpPlacement>,
        response_channel: Option<crossbeam::channel::Sender<Result<()>>>,
    ) -> Result<()> {
        let target = self.tab_a_client_is_looking_at(client_id);
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
        // A name the tab was born with elsewhere is only a fallback here, as it was there;
        // a rename the user made there still holds.
        if let Some(tab) = self.tabs.get_mut(&tab_index) {
            tab.set_user_named(payload.user_named);
        }

        if let Some(relationships) = payload.relationships {
            self.pending_warp_relationships
                .insert(tab_index, relationships);
        }
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
                Some(payload.floating_layout),
                tab_index,
                None,
                false,
                true,
                client_id,
                None,
            ))
            .with_context(err_context)?;

        if let Some(placement) = placement {
            self.pending_group_placements
                .insert(tab_index, (placement, target, client_id, response_channel));
        }

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
                    agent: pane.agent,
                    child_exit_status: adopted.child_exit_status,
                    caption: pane.caption,
                    replay,
                }
            })
            .collect()
    }
}

/// The title a warped pane arrives *named*, when the user or its program gave it one.
///
/// A default - "Pane #3", or the command it runs - is not one: arriving as a name would make it
/// explicit there, and an explicit title is lent to the tab a lone pane sits in, where it
/// outranks the project. A warped tab would wear "Pane #3" instead of the work in it. The
/// default travels as a caption instead ([`default_caption`]).
fn carried_caption(pane: &dyn Pane) -> Option<String> {
    (pane.has_explicit_title() || pane.custom_title().is_some()).then(|| pane.current_title())
}

/// The default caption a terminal pane was wearing, so it arrives captioned the way it was
/// without that caption becoming its name. A stack's members keep their distinct "Pane #N";
/// the receiving session would otherwise number them from where they land, and collide.
///
/// Plugin panes are restarted and caption themselves.
fn default_caption(pane: &dyn Pane) -> Option<String> {
    (matches!(pane.pid(), PaneId::Terminal(_)) && carried_caption(pane).is_none())
        .then(|| pane.current_title())
}
