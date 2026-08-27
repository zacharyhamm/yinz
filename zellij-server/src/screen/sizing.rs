use zellij_utils::errors::prelude::*;

use super::Screen;
use crate::mobile_web::MobileWebPrefs;
use crate::panes::PaneId;
use crate::ClientId;
use std::collections::HashSet;
use zellij_utils::ipc::PixelDimensions;
use zellij_utils::pane_size::Size;

impl Screen {
    pub fn set_client_size(&mut self, client_id: ClientId, size: Size) {
        self.client_sizes.insert(client_id, size);
        // A terminal that reports its text area in pixels but not its cell size needs a client
        // size to divide by. That reply reaches us on the route thread while the size comes via
        // AddClient on the server-instruction thread, so it can arrive first - `Screen::size` was
        // seeded in `new` and was always there to divide by, `client_sizes` is not, and the client
        // re-sends only on a font size change. `self.pixel_dimensions` kept the reply, so derive
        // from it now that there is a size.
        //
        // Only when nothing has been derived yet: `text_area_size` belongs to whichever client
        // sent it, so dividing it by another client's - or a newly resized one's - rows and cols
        // would replace a good cell size with a wrong one.
        let cell_size_is_unknown = self.character_cell_size.borrow().is_none();
        if cell_size_is_unknown {
            self.update_pixel_dimensions(client_id, PixelDimensions::default());
        }
    }

    // The size a client contributes to the layout: a mobile web client with "fit" disabled must
    // not shrink anything, so it borrows the size of a client it shares a tab with instead.
    fn effective_client_size(&self, client_id: ClientId) -> Option<Size> {
        if self
            .mobile_web_prefs
            .get(&client_id)
            .map(|prefs| !prefs.fit)
            .unwrap_or(false)
        {
            self.reference_size_for_client(client_id)
        } else {
            self.client_sizes.get(&client_id).copied()
        }
    }

    // The screen is shared by every client, so it can only be as large as the smallest one on
    // each axis. `None` when no client contributes a size.
    pub(super) fn session_size(&self) -> Option<Size> {
        let connected = self.connected_clients.borrow();
        connected
            .keys()
            .filter(|client_id| !self.watcher_clients.contains_key(client_id))
            .filter_map(|client_id| self.effective_client_size(*client_id))
            .reduce(|acc, size| Size {
                rows: acc.rows.min(size.rows),
                cols: acc.cols.min(size.cols),
            })
    }

    // The single surface every tab is laid out over: the smallest client viewport, so every tab
    // stays visible to every client.
    //
    // When no client contributes a size - every regular client has detached and only watchers
    // remain - the last surface the tabs were laid out over stands in, so the layout is kept
    // rather than collapsed to nothing.
    //
    // Takes `connected_clients` via `session_size`, so bind it to a local before calling anything
    // that borrows it again (`get_first_client_id`).
    pub(super) fn surface_size(&self) -> Size {
        self.session_size().unwrap_or(self.last_surface_size)
    }

    // Remember the surface the tabs were last laid out over, for when every client has detached.
    pub(super) fn remember_surface_size(&mut self) {
        if let Some(size) = self.session_size() {
            self.last_surface_size = size;
        }
    }

    // Stands in for the terminal-wide resize the removed `Screen::size` used to model: put every
    // client at `new_screen_size` so `surface_size` resolves to it, then reflow.
    #[cfg(test)]
    pub(super) fn resize_to_screen(&mut self, new_screen_size: Size) -> Result<()> {
        let client_ids: Vec<ClientId> = self
            .connected_clients
            .borrow()
            .keys()
            .copied()
            .chain(self.client_sizes.keys().copied())
            .collect();
        for client_id in client_ids {
            self.set_client_size(client_id, new_screen_size);
        }
        self.resize_to_clients()
    }

    pub fn resize_to_clients(&mut self) -> Result<()> {
        let err_context = || "failed to resize to client sizes".to_string();
        self.remember_surface_size();
        self.reflow_all_tabs().with_context(err_context)?;
        self.log_and_report_session_state()
            .with_context(err_context)?;
        self.render(None).with_context(err_context)
    }

    pub(super) fn size_for_client(&self, client_id: Option<ClientId>) -> Size {
        client_id
            .and_then(|client_id| self.client_sizes.get(&client_id).copied())
            .or_else(|| {
                self.get_first_client_id()
                    .and_then(|client_id| self.client_sizes.get(&client_id).copied())
            })
            .or_else(|| self.client_sizes.values().next().copied())
            // every client has detached, so nobody contributes a size - the surface the tabs were
            // last laid out over is what stands in, and a tab created here (`zellij action
            // new-tab` against a detached session) has to match it or it is built with no rows to
            // put a pane in and closed on the next render tick. Before the first client has
            // reported a size that is the size the session was built with - never a zero size,
            // which is not a layout anything can be laid out against.
            .unwrap_or(self.last_surface_size)
    }

    pub(super) fn size_of_tab_of_client(&self, client_id: ClientId) -> Size {
        self.active_tab_ids
            .get(&client_id)
            .and_then(|tab_id| self.tabs.get(tab_id))
            .map(|tab| tab.size)
            .unwrap_or_else(|| self.size_for_client(Some(client_id)))
    }

    pub fn recompute_tab_size(&mut self, tab_id: usize) -> Result<()> {
        self.reflow_tab(tab_id)
            .with_context(|| format!("failed to recompute size for tab {tab_id}"))
    }

    // A fit-disabled mobile client borrows only from a client viewing the same tab; another tab
    // cannot be used as its rendering reference.
    fn shares_tab_with(&self, client_id: ClientId, other_id: ClientId) -> bool {
        if other_id == client_id {
            return false;
        }
        match (
            self.active_tab_ids.get(&client_id),
            self.active_tab_ids.get(&other_id),
        ) {
            (Some(tab_id), Some(other_tab_id)) => tab_id == other_tab_id,
            _ => false,
        }
    }

    pub(super) fn reference_size_for_client(&self, client_id: ClientId) -> Option<Size> {
        let connected = self.connected_clients.borrow();
        connected
            .keys()
            .filter(|other_id| self.shares_tab_with(client_id, **other_id))
            .filter_map(|other_id| self.client_sizes.get(other_id).copied())
            .max_by_key(|size| size.rows * size.cols)
    }

    pub(super) fn has_reference_client(&self, client_id: ClientId) -> bool {
        let connected = self.connected_clients.borrow();
        connected
            .keys()
            .any(|other_id| self.shares_tab_with(client_id, *other_id))
    }

    fn ensure_pane_no_ui_fullscreen(&mut self, pane_id: PaneId) {
        let Some(tab_id) = self.tab_id_containing_pane(pane_id) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        if tab.is_fullscreen_active() {
            if tab.fullscreen_pane_id() == Some(pane_id) {
                if tab.fullscreen_covers_ui() {
                    return;
                }
                tab.toggle_pane_no_ui_fullscreen(pane_id);
                return;
            }
            tab.unset_fullscreen();
        }
        tab.toggle_pane_no_ui_fullscreen(pane_id);
    }

    pub(super) fn unset_pane_no_ui_fullscreen(&mut self, pane_id: PaneId) {
        let Some(tab_id) = self.tab_id_containing_pane(pane_id) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        if tab.is_fullscreen_active() && tab.fullscreen_pane_id() == Some(pane_id) {
            tab.unset_fullscreen();
        }
    }

    fn active_pane_id_for_client(&mut self, client_id: ClientId) -> Option<PaneId> {
        self.get_active_tab(client_id)
            .ok()
            .and_then(|tab| tab.get_active_pane_id_or_first_selectable(client_id))
    }

    pub fn set_mobile_render_preferences(
        &mut self,
        client_id: ClientId,
        single_pane: bool,
        fit: bool,
    ) -> Result<()> {
        let reference_client_connected = self.has_reference_client(client_id);
        let fit = if !reference_client_connected {
            true
        } else {
            fit
        };

        let previous = self.mobile_web_prefs.get(&client_id).copied();
        let previous_fullscreen = previous.and_then(|p| p.fullscreened_pane);

        let mut fullscreened_pane = previous_fullscreen;
        if single_pane {
            let target = self.active_pane_id_for_client(client_id);
            if let Some(pane_id) = target {
                if previous_fullscreen != Some(pane_id) {
                    if let Some(previous_pane) = previous_fullscreen {
                        self.unset_pane_no_ui_fullscreen(previous_pane);
                    }
                }
                self.ensure_pane_no_ui_fullscreen(pane_id);
                fullscreened_pane = Some(pane_id);
            }
        } else if let Some(previous_pane) = previous_fullscreen {
            self.unset_pane_no_ui_fullscreen(previous_pane);
            fullscreened_pane = None;
        }

        self.mobile_web_prefs.insert(
            client_id,
            MobileWebPrefs {
                single_pane,
                fit,
                fullscreened_pane,
            },
        );

        if let Some(&tab_id) = self.active_tab_ids.get(&client_id) {
            self.recompute_tab_size(tab_id)?;
        }
        self.render(None)?;
        Ok(())
    }

    pub(super) fn revert_fit_disabled_without_reference_client(&mut self) -> Result<()> {
        let reverted: Vec<ClientId> = self
            .mobile_web_prefs
            .iter()
            .filter(|(_client_id, prefs)| !prefs.fit)
            .map(|(client_id, _)| *client_id)
            .filter(|client_id| !self.has_reference_client(*client_id))
            .collect();
        if reverted.is_empty() {
            return Ok(());
        }
        let mut tabs_to_recompute: HashSet<usize> = HashSet::new();
        for client_id in reverted {
            if let Some(prefs) = self.mobile_web_prefs.get_mut(&client_id) {
                prefs.fit = true;
            }
            if let Some(&tab_id) = self.active_tab_ids.get(&client_id) {
                tabs_to_recompute.insert(tab_id);
            }
        }
        for tab_id in tabs_to_recompute {
            self.recompute_tab_size(tab_id)?;
        }
        Ok(())
    }

    pub(super) fn reconcile_all_single_pane_focus(&mut self) {
        let single_pane_clients: Vec<ClientId> = self
            .mobile_web_prefs
            .iter()
            .filter(|(_client_id, prefs)| prefs.single_pane)
            .map(|(client_id, _)| *client_id)
            .collect();
        for client_id in single_pane_clients {
            self.reconcile_single_pane_focus(client_id);
        }
    }

    pub(super) fn reconcile_single_pane_focus(&mut self, client_id: ClientId) {
        let is_single_pane = self
            .mobile_web_prefs
            .get(&client_id)
            .map(|prefs| prefs.single_pane)
            .unwrap_or(false);
        if !is_single_pane {
            return;
        }
        let Some(target) = self.active_pane_id_for_client(client_id) else {
            return;
        };
        let previous = self
            .mobile_web_prefs
            .get(&client_id)
            .and_then(|prefs| prefs.fullscreened_pane);

        // Tab-global fullscreen is the source of truth. When the tracked pane is still the focused
        // one but the tab is no longer fullscreen on it, another actor deliberately left
        // fullscreen, so single-pane yields rather than fighting for it. A changed focus target
        // (own tap, own new pane, closed pane) is instead re-asserted against the new pane.
        let target_tab_fullscreen_ok = self
            .tab_id_containing_pane(target)
            .and_then(|tab_id| self.tabs.get(&tab_id))
            .map(|tab| {
                tab.is_fullscreen_active()
                    && tab.fullscreen_pane_id() == Some(target)
                    && tab.fullscreen_covers_ui()
            })
            .unwrap_or(false);

        if previous == Some(target) {
            if target_tab_fullscreen_ok {
                return;
            }
            self.demote_single_pane(client_id);
            return;
        }
        if let Some(previous_pane) = previous {
            if previous_pane != target {
                self.unset_pane_no_ui_fullscreen(previous_pane);
            }
        }
        self.ensure_pane_no_ui_fullscreen(target);
        if let Some(prefs) = self.mobile_web_prefs.get_mut(&client_id) {
            prefs.fullscreened_pane = Some(target);
        }
    }

    // Leaves the tab as the other client left it and drops the client's single-pane preference.
    // Fit is disabled alongside it when the tab is shared, otherwise the tab would immediately be
    // shrunk to this client's size by the min-size rule - trading one intrusion for another.
    fn demote_single_pane(&mut self, client_id: ClientId) {
        let disable_fit = self.has_reference_client(client_id);
        let mut fit_changed = false;
        if let Some(prefs) = self.mobile_web_prefs.get_mut(&client_id) {
            prefs.single_pane = false;
            prefs.fullscreened_pane = None;
            if disable_fit && prefs.fit {
                prefs.fit = false;
                fit_changed = true;
            }
        }
        if fit_changed {
            if let Some(&tab_id) = self.active_tab_ids.get(&client_id) {
                if let Err(e) = self.recompute_tab_size(tab_id) {
                    log::error!("Failed to recompute tab size after single-pane demotion: {e:?}");
                }
            }
        }
    }
}
