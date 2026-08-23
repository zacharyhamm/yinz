use zellij_utils::errors::prelude::*;

use super::Screen;
use crate::global_panes::{unowned_strip_rows, AllocatedGlobalPanes, GlobalPaneStrip};
use crate::output::{CharacterChunk, Output};
use crate::panes::EMPTY_TERMINAL_CHARACTER;
use crate::ClientId;
use zellij_utils::pane_size::{Dimension, Size, Viewport};

impl Screen {
    /// Where the global panes sit for a tab of `size`, and the center that tab gets.
    pub(super) fn global_layout_for(&self, size: Size) -> Result<AllocatedGlobalPanes> {
        self.global_panes
            .tab_layout(Viewport {
                x: 0,
                y: 0,
                cols: size.cols,
                rows: size.rows,
            })
            .map_err(|error| anyhow!(error))
    }

    /// The top/bottom ui strips (tab bar, status bar) a left/right global pane must leave to the
    /// tab, so they can span the full screen width beside it.
    ///
    /// A global pane is one grid shared by every tab, so this cannot follow the tab being
    /// rendered: it is the tallest chrome any tab declares, which no tab's chrome can then be
    /// covered by. The cost is that a tab with less chrome than that has rows in the strip's columns
    /// that neither its chrome nor the shortened band reaches - `unowned_strip_rows` names them and
    /// `Screen::fill_unowned_strip_rows` paints them.
    pub(super) fn max_chrome_rows(&self) -> (usize, usize) {
        self.tabs
            .values()
            .map(|tab| tab.chrome_rows())
            .reduce(|acc, rows| (acc.0.max(rows.0), acc.1.max(rows.1)))
            .unwrap_or((0, 0))
    }

    /// Every tab shares the smallest interactive client bounds, regardless of which tab each
    /// client is viewing. Watchers and fit-disabled mobile clients are already handled by
    /// `session_size`.
    pub(super) fn tab_surface_size(&self, _tab_id: usize) -> Option<Size> {
        self.session_size()
    }

    /// How much of a watcher's viewport the followed client's frame covers, so
    /// `serialize_with_size` clears only what is beyond it.
    ///
    /// That frame is the whole tab surface, global pane strips included - `Tab::size` is just the
    /// center between them, and padding from there would clear the strips on every frame the panes
    /// themselves did not redraw.
    pub(super) fn followed_frame_size(&self, tab_id: usize) -> Option<Size> {
        if !self.tabs.contains_key(&tab_id) {
            return None;
        }
        Some(
            self.tab_surface_size(tab_id)
                .unwrap_or_else(|| self.surface_size()),
        )
    }

    pub(super) fn resize_global_panes_to(&mut self, size: Size) -> Result<Viewport> {
        self.global_panes.set_chrome_rows(self.max_chrome_rows());
        self.global_panes
            .resize(Viewport {
                x: 0,
                y: 0,
                cols: size.cols,
                rows: size.rows,
            })
            .map_err(|error| anyhow!(error))
    }

    pub(super) fn reflow_global_panes(&mut self) -> Result<()> {
        let surface = self.surface_size();
        self.resize_global_panes_to(surface)?;
        for tab_id in self.tabs.keys().copied().collect::<Vec<_>>() {
            self.reflow_tab(tab_id)?;
        }
        Ok(())
    }

    /// Lay tab `tab_id` out over the shared session surface, beside the global panes.
    ///
    /// With no interactive client there is no size to derive, so the tab keeps its last layout.
    pub(super) fn reflow_tab(&mut self, tab_id: usize) -> Result<()> {
        let Some(tab_size) = self.tab_surface_size(tab_id) else {
            return Ok(());
        };
        let center = self.global_layout_for(tab_size)?.center;
        let center_size = Size {
            cols: center.cols,
            rows: center.rows,
        };
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            if tab.get_display_area() != center_size
                || tab.get_display_area_origin() != (center.x, center.y)
            {
                tab.resize_whole_tab_to(center)?;
                tab.set_force_render();
            }
        }
        Ok(())
    }

    /// The strips a tab must leave to the global panes for the supplied layout.
    /// One strip per edge: the union of that edge's pane geometries. Panes sharing an edge split
    /// one strip among themselves, and everything tab-side - boundary growth
    /// (`expand_viewport_over_global_panes`), the unowned-row fill (`unowned_strip_rows`), ui pane
    /// widening - reasons about the strip as a single rectangle. Per-pane strips here would make
    /// each same-edge pane's span look "unowned" from its sibling's strip and get blanked over.
    pub(super) fn strips_from_layout(&self, layout: &AllocatedGlobalPanes) -> Vec<GlobalPaneStrip> {
        let mut strips: Vec<GlobalPaneStrip> = vec![];
        for (pane, placement, hidden) in self.global_panes.panes() {
            if hidden {
                continue;
            }
            let Some(geom) = layout.panes.get(&pane.pid().into()) else {
                continue;
            };
            match strips.iter_mut().find(|strip| strip.edge == placement.edge) {
                Some(strip) => {
                    let right = (strip.geom.x + strip.geom.cols.as_usize())
                        .max(geom.x + geom.cols.as_usize());
                    let bottom = (strip.geom.y + strip.geom.rows.as_usize())
                        .max(geom.y + geom.rows.as_usize());
                    strip.geom.x = strip.geom.x.min(geom.x);
                    strip.geom.y = strip.geom.y.min(geom.y);
                    strip.geom.cols = Dimension::fixed(right - strip.geom.x);
                    strip.geom.rows = Dimension::fixed(bottom - strip.geom.y);
                },
                None => strips.push(GlobalPaneStrip {
                    geom: *geom,
                    edge: placement.edge,
                }),
            }
        }
        strips
    }

    /// Paint the strip cells tab `tab_id` leaves unowned (see `unowned_strip_rows`) for the clients
    /// looking at it, so the previous tab's chrome does not linger beside the sidebar.
    ///
    /// Only for clients whose fill actually changed, unless `force` - `Output` re-emits every chunk
    /// it is handed and counts them towards `is_dirty`, so painting unconditionally would send a
    /// frame on every render tick.
    pub(super) fn fill_unowned_strip_rows(
        &mut self,
        output: &mut Output,
        tab_id: usize,
        layout: &AllocatedGlobalPanes,
        client_ids: &[ClientId],
        force: bool,
    ) -> Result<()> {
        let strips = self.strips_from_layout(layout);
        let Some(tab) = self.tabs.get(&tab_id) else {
            return Ok(());
        };
        let rects = unowned_strip_rows(layout.center, &strips, tab.chrome_rows());
        let stale: Vec<ClientId> = client_ids
            .iter()
            .copied()
            .filter(|client_id| force || self.unowned_strip_fill.get(client_id) != Some(&rects))
            .collect();
        for client_id in &stale {
            self.unowned_strip_fill.insert(*client_id, rects.clone());
        }
        if rects.is_empty() || stale.is_empty() {
            return Ok(());
        }
        let chunks: Vec<CharacterChunk> = rects
            .iter()
            .map(|(x, y, cols)| CharacterChunk::new(vec![EMPTY_TERMINAL_CHARACTER; *cols], *x, *y))
            .collect();
        output
            .add_character_chunks_to_multiple_clients(chunks, stale.into_iter(), None)
            .context("failed to fill the strip rows no tab owns")
    }
}
