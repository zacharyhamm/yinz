use zellij_utils::errors::prelude::*;

use super::Screen;
use zellij_utils::pane_size::{Size, Viewport};

impl Screen {
    /// Every tab shares the smallest interactive client bounds, regardless of which tab each
    /// client is viewing. Watchers and fit-disabled mobile clients are already handled by
    /// `session_size`.
    pub(super) fn tab_surface_size(&self, _tab_id: usize) -> Option<Size> {
        self.session_size()
    }

    /// How much of a watcher's viewport the followed client's frame covers, so
    /// `serialize_with_size` clears only what is beyond it.
    pub(super) fn followed_frame_size(&self, tab_id: usize) -> Option<Size> {
        if !self.tabs.contains_key(&tab_id) {
            return None;
        }
        Some(
            self.tab_surface_size(tab_id)
                .unwrap_or_else(|| self.surface_size()),
        )
    }

    /// Lay every tab out over the shared session surface, and record that surface as the one the
    /// tabs now sit on.
    ///
    /// The recording belongs here rather than at the debounced render that follows: a client that
    /// attaches and detaches inside that window would otherwise leave `last_surface_size` on its
    /// pre-attach value, and `surface_size` / `size_for_client` stand in with it once every client
    /// has detached - so the next render tick would snap the tabs back, and a `zellij action
    /// new-tab` would build a tab the reattaching client cannot match.
    pub(super) fn reflow_all_tabs(&mut self) -> Result<()> {
        for tab_id in self.tabs.keys().copied().collect::<Vec<_>>() {
            self.reflow_tab(tab_id)?;
        }
        self.remember_surface_size();
        Ok(())
    }

    /// Lay tab `tab_id` out over the shared session surface.
    ///
    /// With no interactive client there is no size to derive, so the tab keeps its last layout.
    pub(super) fn reflow_tab(&mut self, tab_id: usize) -> Result<()> {
        let Some(tab_size) = self.tab_surface_size(tab_id) else {
            return Ok(());
        };
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            if tab.get_display_area() != tab_size {
                tab.resize_whole_tab_to(tab_size)?;
                tab.set_force_render();
            }
        }
        Ok(())
    }

    /// The whole session surface tab `tab_id` sits on. A tab is laid out over the entire surface,
    /// so this is its display area at the screen origin.
    pub(super) fn session_surface_for_tab(&self, tab_id: usize) -> Option<Viewport> {
        let tab = self.tabs.get(&tab_id)?;
        let size = self
            .tab_surface_size(tab_id)
            .unwrap_or_else(|| tab.get_display_area());
        Some(Viewport {
            x: 0,
            y: 0,
            cols: size.cols,
            rows: size.rows,
        })
    }
}
