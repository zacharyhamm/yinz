//! Where a newly spawned agent pane goes.
//!
//! Every route that opens a coding agent in a pane - agent's tools and modal, `yinz run --
//! claude`, a keybinding's `Run` - ends in a `ScreenInstruction::NewPane` that names the tab it
//! was asked for only indirectly: the client's active tab, or the tab of the plugin that asked.
//! None of them can see whether that tab has room, and a tab does not know its siblings, so the
//! choice is made here, once, on the screen thread, where every tab's geometry is in view.
//!
//! The rule is about size, not count. Before an agent pane joins a tab, the tab's tiled panes
//! and the newcomer are imagined sharing the viewport equally, and the share is compared with a
//! configured floor - `agent_pane_min_columns` by `agent_pane_min_rows`. A tab that would fall
//! below it sends the pane to the roomiest other tab that clears it, or to a tab of its own.
//! Six panes is the right cap on one screen and wrong on the next; a floor in cells follows the
//! screen.
//!
//! The share is an area rather than a width and a height because an equal share fixes an area
//! and not a shape: the grid halves one axis of one pane at a time
//! (`TiledPaneGrid::find_room_for_new_pane`), and stacked resize collapses siblings to a row,
//! so what a pane's rows and columns would be is a property of the split tree. Frames are
//! counted as pane area and a pane collapsed in a stack counts as one; both are approximations
//! the floor absorbs.

use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;

use super::Screen;
use crate::pty::ClientTabIndexOrPaneId;
use crate::ClientId;
use yinz_types::input::layout::Run;
use yinz_types::pane_size::{Size, Viewport};
use {yinz_types::data::NewPanePlacement, yinz_types::data::AGENT_PANE_CONTEXT};

/// What `Screen::agent_pane_target` decided: where the pane goes, and whether a client has to
/// be brought to it afterwards.
pub(super) struct AgentPlacement {
    pub target: ClientTabIndexOrPaneId,
    pub created_tab_id: Option<TabId>,
    /// A client to focus on the pane once it is placed. Set when the pane was sent to a tab
    /// that already exists and the spawn wanted focus: a `TabIndex` target places without a
    /// client, so nothing else would move one. A tab made for the pane needs no such step -
    /// `apply_layout` moves the client when the layout lands, if it was asked to. Focusing a
    /// tiled pane lowers the destination tab's floating layer, as it would in the source tab:
    /// a focused tiled pane and a raised floating layer cannot coexist.
    pub focus_after_placing: Option<ClientId>,
}

impl AgentPlacement {
    fn unchanged(target: ClientTabIndexOrPaneId) -> Self {
        AgentPlacement {
            target,
            focus_after_placing: None,
            created_tab_id: None,
        }
    }
}

/// Whether a pane about to be created is a coding agent's, from what the session knows before
/// the process has said anything: a plugin opened it as an agent pane - the context marker
/// `agent_from_run` in `panes::terminal_pane` starts the record from - or its *program* names
/// a known agent, by the list `agent-service` detects with. The program only, not any argument:
/// a wrong answer here moves a pane, where a wrong label costs nothing.
pub(super) fn is_agent_spawn(invoked_with: Option<&Run>) -> bool {
    let Some(Run::Command(run_command)) = invoked_with else {
        return false;
    };
    let marked = run_command
        .originating_plugin
        .as_ref()
        .is_some_and(|plugin| plugin.context.contains_key(AGENT_PANE_CONTEXT));
    marked
        || yinz_types::agent::kind_from_program_and_args(&run_command.command, &run_command.args)
            .is_some()
}

/// The cells each tiled pane would have if `tiled_panes` of them and one newcomer shared a
/// viewport equally.
pub(super) fn projected_share(viewport: Viewport, tiled_panes: usize) -> usize {
    viewport.rows.saturating_mul(viewport.cols) / (tiled_panes + 1)
}

/// The area a share has to clear, or `None` when either side of the floor is zero and the rule
/// is off.
pub(super) fn floor_of(min_size: Size) -> Option<usize> {
    (min_size.rows > 0 && min_size.cols > 0).then(|| min_size.rows.saturating_mul(min_size.cols))
}

/// A tab that could take the pane: its stable id, its position on the bar, and the share it
/// would give the newcomer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Candidate {
    pub tab_id: TabId,
    pub position: TabPosition,
    pub share: usize,
}

/// The roomiest candidate whose share clears `floor`. Ties go to the lowest position, so the
/// same tabs give the same answer every time.
pub(super) fn choose_tab(candidates: &[Candidate], floor: usize) -> Option<TabId> {
    candidates
        .iter()
        .filter(|candidate| candidate.share >= floor)
        .max_by(|a, b| {
            a.share
                .cmp(&b.share)
                .then_with(|| b.position.cmp(&a.position))
        })
        .map(|candidate| candidate.tab_id)
}

impl Screen {
    /// The tab a "focused tab" request from `client_id` would act on, resolved the way the
    /// `ClientId` arm in `thread_main` resolves it: this client's active tab if it is attached,
    /// else the first attached client's, else the first tab there is. A `yinz run` or a warp
    /// request is a client of its own with no screen, which is why the fallbacks exist.
    pub(super) fn tab_a_client_is_looking_at(&self, client_id: ClientId) -> Option<TabId> {
        self.connected_client_to_act_for(client_id)
            .and_then(|client_id| self.active_tab_ids.get(&client_id).copied())
            .or_else(|| self.tabs.keys().next().copied())
    }

    /// The attached client a "focused" action from `client_id` acts for: `client_id` if it is
    /// attached, else the first client that is.
    pub(super) fn connected_client_to_act_for(&self, client_id: ClientId) -> Option<ClientId> {
        if self.active_tab_ids.contains_key(&client_id) {
            Some(client_id)
        } else {
            self.get_first_client_id()
        }
    }

    /// The share a tab would give one more tiled pane.
    fn projected_share_of_tab(&self, tab_id: TabId) -> Option<usize> {
        let tab = self.tabs.get(&tab_id)?;
        Some(projected_share(
            tab.get_viewport(),
            tab.get_selectable_tiled_panes_count(),
        ))
    }

    /// The tab a spawned agent pane goes to, given where it was asked for.
    ///
    /// `target` comes back unchanged unless all of these hold: the rule is on, the pane is an
    /// agent's ([`is_agent_spawn`]), it will not start suppressed, it was asked for with no
    /// placement of its own - a tiled pane with no direction, or no preference at all in a tab
    /// whose floating layer is hidden (with it shown, no preference means a floating pane, and
    /// crowding is about the tiled ones) - it names no tab (an explicit `--tab-id` is
    /// honoured), and the tab it would land in projects a share below the floor. Then the
    /// target is the roomiest other tab that clears the floor, or a tab made for the pane, as
    /// `TabIndex` when the spawn wanted focus and `TabIndexNoFocus` when it did not. A
    /// `ClientId` target - `yinz run`, a keybinding's `Run` - wanted focus, since the arm it
    /// came from would have focused the pane, so the client follows it. A `PaneId` target -
    /// every agent route opens its agent panes near the plugin - did not: the plugin focuses
    /// the pane itself if it means to, and a spawn from `agent_spawn` should not pull the user
    /// off what they were looking at.
    ///
    /// Pending and fullscreen tabs are not candidates: a pending layout has not landed, so its
    /// pane count is not yet real, and adding a pane to a fullscreen tab would dismiss its
    /// fullscreen state. A tab made here is pending for the plugin → pty → screen round trip
    /// that applies its layout, so a burst of spawns faster than that opens a tab each; the next
    /// spawn after the round trip sees the tab as it is.
    ///
    /// A failure to make a tab is not a reason to lose the pane: it is logged and the pane
    /// keeps the target it came with.
    pub(super) fn agent_pane_target(
        &mut self,
        invoked_with: Option<&Run>,
        placement: &mut NewPanePlacement,
        start_suppressed: bool,
        target: ClientTabIndexOrPaneId,
    ) -> AgentPlacement {
        let Some(floor) = floor_of(self.agent_pane_min_size) else {
            return AgentPlacement::unchanged(target);
        };
        if start_suppressed || !is_agent_spawn(invoked_with) {
            return AgentPlacement::unchanged(target);
        }
        // The tab the pane would land in, the client on whose behalf a tab may be made, and
        // whether the spawn wanted focus - each as the arm in `thread_main` that handles this
        // target would resolve it.
        let (source_tab_id, client_id, wants_focus) = match target {
            ClientTabIndexOrPaneId::ClientId(client_id)
            | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => {
                let wants_focus = matches!(target, ClientTabIndexOrPaneId::ClientId(_));
                // A `yinz run` is a client of its own with no screen, so the client whose
                // tab is crowded and who follows the pane is the attached one the arm for this
                // target would act for: this client if attached, else the first that is.
                let connected = self.connected_client_to_act_for(client_id);
                let source = self.tab_a_client_is_looking_at(client_id);
                (source, connected, wants_focus)
            },
            ClientTabIndexOrPaneId::PaneId(pane_id)
            | ClientTabIndexOrPaneId::PaneIdOrClient(pane_id, _) => {
                let source = self.tab_id_containing_pane(pane_id);
                let client_id = self
                    .active_tab_ids
                    .iter()
                    .find(|(_, tab_id)| Some(**tab_id) == source)
                    .map(|(client_id, _)| *client_id)
                    .or_else(|| self.get_first_client_id());
                (source, client_id, false)
            },
            ClientTabIndexOrPaneId::TabIndex(_)
            | ClientTabIndexOrPaneId::TabIndexNoFocus(_)
            | ClientTabIndexOrPaneId::TabIndexForClient(_, _) => {
                return AgentPlacement::unchanged(target);
            },
        };
        let (Some(source_tab_id), Some(client_id)) = (source_tab_id, client_id) else {
            return AgentPlacement::unchanged(target);
        };
        let Some(source) = self.tabs.get(&source_tab_id) else {
            return AgentPlacement::unchanged(target);
        };
        let tiled_without_direction = match placement {
            NewPanePlacement::Tiled {
                direction: None, ..
            } => true,
            NewPanePlacement::NoPreference { .. } => !source.are_floating_panes_visible(),
            _ => false,
        };
        if !tiled_without_direction {
            return AgentPlacement::unchanged(target);
        }
        if projected_share(
            source.get_viewport(),
            source.get_selectable_tiled_panes_count(),
        ) >= floor
        {
            return AgentPlacement::unchanged(target);
        }
        let candidates: Vec<Candidate> = self
            .tabs
            .values()
            .filter(|tab| {
                tab.id != source_tab_id && !tab.is_pending() && !tab.is_fullscreen_active()
            })
            .filter_map(|tab| {
                Some(Candidate {
                    tab_id: tab.id,
                    position: tab.position,
                    share: self.projected_share_of_tab(tab.id)?,
                })
            })
            .collect();
        let mut created_tab_id = None;
        let (destination, focus_after_placing) = match choose_tab(&candidates, floor) {
            Some(tab_id) => (tab_id, wants_focus.then_some(client_id)),
            None => {
                let made = self
                    .new_tab_for_adopted_pane(invoked_with.cloned(), client_id, wants_focus)
                    .context(
                        "failed to open a tab for an agent pane; it goes where it was asked for",
                    );
                match made {
                    Ok(tab_id) => {
                        created_tab_id = Some(tab_id);
                        (tab_id, None)
                    },
                    Err(error) => {
                        Err::<(), _>(error).non_fatal();
                        return AgentPlacement::unchanged(target);
                    },
                }
            },
        };
        // The destination is a tab, so the pane tiles there whatever the source would have
        // done with no preference.
        if let NewPanePlacement::NoPreference { borderless } = *placement {
            *placement = NewPanePlacement::Tiled {
                direction: None,
                borderless,
            };
        }
        AgentPlacement {
            target: if wants_focus {
                ClientTabIndexOrPaneId::TabIndex(destination)
            } else {
                ClientTabIndexOrPaneId::TabIndexNoFocus(destination)
            },
            focus_after_placing,
            created_tab_id,
        }
    }
}

impl Screen {
    pub(super) fn resolve_agent_tab(
        &mut self,
        placement: yinz_types::agent::control::AgentPlacement,
        invoked_with: Option<Run>,
        client_id: ClientId,
    ) -> Result<yinz_bus::route::AgentTabPlacement, String> {
        use yinz_types::agent::control::AgentPlacement;
        placement.validate().map_err(str::to_owned)?;
        let (tab_id, created) = match placement {
            AgentPlacement::Tab(id) | AgentPlacement::Suppressed(Some(id)) => {
                if let Some(tab) = self.tabs.get_mut(&id) {
                    tab.agent_tab_cleanup_allowed = false;
                }
                (id, false)
            },
            AgentPlacement::Suppressed(None) => {
                let id = self
                    .tab_a_client_is_looking_at(client_id)
                    .ok_or("no current tab for suppressed agent")?;
                if let Some(tab) = self.tabs.get_mut(&id) {
                    tab.agent_tab_cleanup_allowed = false;
                }
                (id, false)
            },
            AgentPlacement::NewTab(name) => {
                let id = self
                    .new_tab_for_adopted_pane(invoked_with, client_id, false)
                    .map_err(|error| format!("cannot create agent tab: {error:#}"))?;
                if let Some(tab) = self.tabs.get_mut(&id) {
                    tab.agent_tab_waiting_for_pane = true;
                }
                if let Some(name) = name
                    && let Some(tab) = self.tabs.get_mut(&id)
                {
                    tab.name = name;
                    tab.set_user_named(true);
                }
                (id, true)
            },
            AgentPlacement::Automatic => {
                return Err("automatic placement does not reserve a tab".into())
            },
        };
        if self.deferred_tab_closes.contains(&tab_id) {
            return Err(format!("tab {tab_id} is closing"));
        }
        let tab = self
            .tabs
            .get(&tab_id)
            .ok_or_else(|| format!("tab {tab_id} does not exist"))?;
        Ok(yinz_bus::route::AgentTabPlacement {
            tab_id,
            lifetime: std::sync::Arc::downgrade(&tab.agent_tab_lifetime),
            created,
        })
    }

    pub(super) fn cleanup_agent_tab(
        &mut self,
        placement: &yinz_bus::route::AgentTabPlacement,
    ) -> bool {
        if placement.lifetime.upgrade().is_none() {
            return true;
        }
        let owned_empty = placement.created
            && self.tabs.get(&placement.tab_id).is_some_and(|tab| {
                placement
                    .lifetime
                    .ptr_eq(&std::sync::Arc::downgrade(&tab.agent_tab_lifetime))
                    && tab.agent_tab_cleanup_allowed
                    && !tab.is_pending()
                    && tab.get_all_pane_ids().is_empty()
            });
        owned_empty
            && self
                .close_tab_by_id(placement.tab_id)
                .map_err(|error| {
                    log::error!("cannot remove empty agent tab: {error:#}");
                    error
                })
                .is_ok()
            && !self.tabs.contains_key(&placement.tab_id)
    }
}
