use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    rc::Rc,
};

use crate::{
    output::Output, panes::PaneId, plugins::PluginInstruction, pty::VteBytes,
    pty_writer::PtyWriteInstruction, tab::Pane, thread_bus::ThreadSenders,
    ui::pane_contents_and_ui::PaneContentsAndUi, ClientId,
};

use zellij_utils::{
    data::{
        Direction, Event, GlobalPaneEdge, GlobalPanePlacement, ModeInfo,
        PaneId as ZellijUtilsPaneId, Resize, ResizeStrategy, Style,
    },
    errors::prelude::*,
    input::layout::PercentOrFixed,
    input::layout::RunPluginOrAlias,
    input::options::PaneFrameStyle,
    pane_size::{Dimension, Offset, PaneGeom, SizeInPixels, Viewport},
};

#[derive(Debug)]
pub struct AllocatedGlobalPanes {
    pub panes: BTreeMap<ZellijUtilsPaneId, PaneGeom>,
    pub center: Viewport,
}

/// A visible global pane's footprint, as far as a tab needs to know about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalPaneStrip {
    pub geom: PaneGeom,
    pub edge: GlobalPaneEdge,
}

/// Grow `viewport` over every global strip that directly abuts it.
///
/// `Boundaries::add_rect` draws a pane's left/top boundary one cell *outside* the pane and skips it
/// entirely when the pane sits flush against the viewport edge. Widening the viewport over the
/// strips is therefore what makes a tab's edge pane draw the separator between itself and a global
/// pane, with junctions merging into the tab's own boundaries.
///
/// The adjacency test is what keeps chrome safe: when a tab bar sits between a top strip and the
/// tab's panes, the strip does not abut the viewport, nothing grows, and no line is drawn over the
/// tab bar. With no strips the viewport is returned unchanged.
///
/// ponytail: separators *between* two global panes stacked on the same edge are not drawn — only
/// strips are grown over, they never contribute rects of their own. Same-edge stacking is therefore
/// rejected up front (see `GlobalPaneManager::reject_occupied_edge`); add global/global rects here
/// if it ever becomes a real layout.
pub fn expand_viewport_over_global_panes(
    viewport: Viewport,
    strips: &[GlobalPaneStrip],
) -> Viewport {
    // inner-to-outer: growing over the innermost strip is what makes the next one out adjacent
    strips.iter().rev().fold(viewport, |mut viewport, strip| {
        let (x, y) = (strip.geom.x, strip.geom.y);
        let (cols, rows) = (strip.geom.cols.as_usize(), strip.geom.rows.as_usize());
        match strip.edge {
            GlobalPaneEdge::Left if x + cols == viewport.x => {
                viewport.x -= cols;
                viewport.cols += cols;
            },
            GlobalPaneEdge::Right if x == viewport.x + viewport.cols => {
                viewport.cols += cols;
            },
            GlobalPaneEdge::Top if y + rows == viewport.y => {
                viewport.y -= rows;
                viewport.rows += rows;
            },
            GlobalPaneEdge::Bottom if y == viewport.y + viewport.rows => {
                viewport.rows += rows;
            },
            _ => {},
        }
        viewport
    })
}

/// The rows inside a left/right strip's columns that neither the strip nor `tab_area`'s tab paints,
/// as `(x, y, cols)` single-row rects.
///
/// A left/right pane's band is inset by the *largest* chrome any tab declares (`chrome_rows` here,
/// `Screen::max_chrome_rows` there), so it can never cover a tab's tab bar or status bar. A tab with
/// less chrome than that has the difference left over: the band skips those rows and the tab cannot
/// reach them either, because only ui panes are widened over a strip
/// (`TiledPanes::expand_ui_panes_over_global_strips`) and that tab has none at that row.
///
/// Read off the strip's actual geometry rather than recomputed from `chrome_rows`, so the
/// "chrome doesn't fit, give the pane the whole height" fallback in `allocate_global_panes` needs no
/// second branch here - it simply yields nothing.
///
/// ponytail: only the rows between the tab's own chrome and the band. A left/right pane declared
/// *before* a top/bottom one is allocated over the full surface rather than the reduced center, so
/// its band leaves rows outside `tab_area` that no top/bottom strip covers either - a hole that owes
/// nothing to per-tab chrome and is not filled here. Allocate top/bottom edges first if it matters.
pub fn unowned_strip_rows(
    tab_area: Viewport,
    strips: &[GlobalPaneStrip],
    tab_chrome_rows: (usize, usize),
) -> Vec<(usize, usize, usize)> {
    let (own_top, own_bottom) = tab_chrome_rows;
    let mut rects = vec![];
    for strip in strips {
        if !matches!(strip.edge, GlobalPaneEdge::Left | GlobalPaneEdge::Right) {
            // top/bottom strips span the full width and leave no chrome rows to anyone
            continue;
        }
        let cols = strip.geom.cols.as_usize();
        let below_the_tabs_top_chrome = tab_area.y + own_top;
        let above_the_tabs_bottom_chrome = (tab_area.y + tab_area.rows).saturating_sub(own_bottom);
        let gaps = [
            (below_the_tabs_top_chrome, strip.geom.y),
            (
                strip.geom.y + strip.geom.rows.as_usize(),
                above_the_tabs_bottom_chrome,
            ),
        ];
        for (start, end) in gaps {
            rects.extend((start..end).map(|y| (strip.geom.x, y, cols)));
        }
    }
    rects
}

pub enum GlobalPaneMouseEffect {
    WriteRaw(Vec<u8>),
    WriteAdjusted(Vec<Vec<u8>>),
    Copy(String),
}

/// `chrome_rows` is the tab's own top/bottom ui strip heights (tab bar, status bar). A left or right
/// global pane gives those rows back so the chrome can span the full screen width beside it - see
/// `TiledPanes::expand_ui_panes_over_global_strips`, which is the other half of this deal. Top and
/// bottom panes already span the full width, so they ignore it.
pub fn allocate_global_panes(
    area: Viewport,
    panes: &[(ZellijUtilsPaneId, GlobalPanePlacement)],
    clamp_to_screen: bool,
    chrome_rows: (usize, usize),
) -> Result<AllocatedGlobalPanes, String> {
    let (panes, center) = panes.iter().try_fold(
        (BTreeMap::new(), area),
        |(mut allocated, mut center), (pane_id, placement)| {
            let remaining_axis = match placement.edge {
                GlobalPaneEdge::Left | GlobalPaneEdge::Right => center.cols,
                GlobalPaneEdge::Top | GlobalPaneEdge::Bottom => center.rows,
            };
            let thickness = placement.size.to_fixed(remaining_axis);
            // Small-screen policy for resize/reflow (clamp_to_screen): rather than failing the
            // whole reflow when the terminal shrinks past what the declared sizes fit, clamp so
            // every pane keeps at least one cell and the tab area keeps at least one cell. The
            // declared size stays in `placement`, so the pane reverts to it on the next reflow
            // with room. Explicit insertions/show/placement-changes validate strictly instead,
            // so a layout that leaves no usable tab area is rejected up front. Both paths still
            // fail when the axis is too small to keep both a pane cell and a tab-area cell.
            let thickness = if clamp_to_screen {
                thickness.clamp(1, remaining_axis.saturating_sub(1).max(1))
            } else {
                thickness
            };
            if thickness == 0 {
                return Err("global pane size must be positive".to_owned());
            }
            if thickness >= remaining_axis {
                return Err("global panes leave no usable tab area".to_owned());
            }

            // the rows a left/right pane leaves to the tab's chrome, unless that would leave it
            // with nothing at all (a tab whose layout is nothing but chrome)
            let (chrome_top, chrome_bottom) = chrome_rows;
            let (band_y, band_rows) = match center.rows.checked_sub(chrome_top + chrome_bottom) {
                Some(band_rows) if band_rows > 0 => (center.y + chrome_top, band_rows),
                _ => (center.y, center.rows),
            };

            let geom = match placement.edge {
                GlobalPaneEdge::Left => {
                    let geom = PaneGeom {
                        x: center.x,
                        y: band_y,
                        cols: Dimension::fixed(thickness),
                        rows: Dimension::fixed(band_rows),
                        ..Default::default()
                    };
                    center.x += thickness;
                    center.cols -= thickness;
                    geom
                },
                GlobalPaneEdge::Right => {
                    let geom = PaneGeom {
                        x: center.x + center.cols - thickness,
                        y: band_y,
                        cols: Dimension::fixed(thickness),
                        rows: Dimension::fixed(band_rows),
                        ..Default::default()
                    };
                    center.cols -= thickness;
                    geom
                },
                GlobalPaneEdge::Top => {
                    let geom = PaneGeom {
                        x: center.x,
                        y: center.y,
                        cols: Dimension::fixed(center.cols),
                        rows: Dimension::fixed(thickness),
                        ..Default::default()
                    };
                    center.y += thickness;
                    center.rows -= thickness;
                    geom
                },
                GlobalPaneEdge::Bottom => {
                    let geom = PaneGeom {
                        x: center.x,
                        y: center.y + center.rows - thickness,
                        cols: Dimension::fixed(center.cols),
                        rows: Dimension::fixed(thickness),
                        ..Default::default()
                    };
                    center.rows -= thickness;
                    geom
                },
            };
            allocated.insert(*pane_id, geom);
            Ok((allocated, center))
        },
    )?;

    if !center.has_positive_size() {
        return Err("global panes leave no usable tab area".to_owned());
    }

    Ok(AllocatedGlobalPanes { panes, center })
}

struct GlobalPane {
    pane: Box<dyn Pane>,
    placement: GlobalPanePlacement,
    hidden: bool,
    layout_applied: bool,
    declaration_order: Option<usize>,
    // `Pane::set_frame` ignores its argument and clears the frame render cache, so calling it
    // unconditionally re-emits every frame on every render tick. Track what we last applied.
    frame_applied: Option<bool>,
}

pub struct GlobalPaneManager {
    panes: Vec<GlobalPane>,
    pane_indices: HashMap<PaneId, usize>,
    focused_panes: BTreeMap<ClientId, PaneId>,
    selecting_with_mouse: BTreeMap<ClientId, PaneId>,
    resizing_with_mouse: BTreeMap<ClientId, PaneId>,
    area: Option<Viewport>,
    senders: ThreadSenders,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    pane_frame_style: PaneFrameStyle,
    // the top/bottom ui strip heights of the tab being rendered, see `allocate_global_panes`
    chrome_rows: (usize, usize),
}

impl Default for GlobalPaneManager {
    fn default() -> Self {
        Self::new(
            ThreadSenders::default().silently_fail_on_send(),
            Rc::new(RefCell::new(None)),
            PaneFrameStyle::default(),
        )
    }
}

impl GlobalPaneManager {
    pub fn new(
        senders: ThreadSenders,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        pane_frame_style: PaneFrameStyle,
    ) -> Self {
        Self {
            panes: vec![],
            pane_indices: HashMap::new(),
            focused_panes: BTreeMap::new(),
            selecting_with_mouse: BTreeMap::new(),
            resizing_with_mouse: BTreeMap::new(),
            area: None,
            chrome_rows: (0, 0),
            senders,
            character_cell_size,
            pane_frame_style,
        }
    }

    #[allow(dead_code)]
    pub fn insert(
        &mut self,
        pane: Box<dyn Pane>,
        placement: GlobalPanePlacement,
        hidden: bool,
    ) -> Result<(), String> {
        self.insert_with_order(pane, placement, hidden, None)
    }

    pub fn insert_with_order(
        &mut self,
        pane: Box<dyn Pane>,
        placement: GlobalPanePlacement,
        hidden: bool,
        declaration_order: Option<usize>,
    ) -> Result<(), String> {
        let pane_id = pane.pid();
        if self.pane_indices.contains_key(&pane_id) {
            return Err(format!("global pane {pane_id:?} already exists"));
        }
        if !hidden {
            // before any allocation, so layout global panes inserted while `area` is still `None`
            // are rejected too
            self.reject_occupied_edge(placement.edge, None)?;
        }
        // Layout panes (Some order) are kept sorted by declaration_order before
        // runtime panes (None). New layout panes are inserted in order among layouts;
        // runtime panes are always appended after all layouts.
        let insertion_index = match declaration_order {
            Some(new_order) => {
                // position among layout panes
                let layout_pos = self
                    .panes
                    .iter()
                    .filter(|g| g.declaration_order.is_some())
                    .position(|g| g.declaration_order.unwrap() > new_order);
                if let Some(pos) = layout_pos {
                    // map nth layout pane to absolute index
                    self.panes
                        .iter()
                        .enumerate()
                        .filter(|(_, g)| g.declaration_order.is_some())
                        .nth(pos)
                        .map(|(idx, _)| idx)
                        .unwrap()
                } else {
                    // after last layout, before first runtime
                    self.panes
                        .iter()
                        .position(|g| g.declaration_order.is_none())
                        .unwrap_or(self.panes.len())
                }
            },
            None => self.panes.len(),
        };
        // When `area` is `None` (before first reflow) the session surface size is
        // not yet known, so only `reject_occupied_edge` is checked eagerly. An
        // oversized `Fixed` or `Percent` size that would overflow the surface is
        // not rejected here; it is deferred to the first `resize`/`apply_layout`
        // where `allocate_global_panes` will clamp (or error) once `area` is set.
        // This is intentional — see `sparse` vs `clamped` handling in `allocate`.
        let layout = if let Some(area) = self.area {
            let mut candidate = self.visible_placements();
            if !hidden {
                let visible_insertion_index = self.panes[..insertion_index]
                    .iter()
                    .filter(|global_pane| !global_pane.hidden)
                    .count();
                candidate.insert(visible_insertion_index, (pane_id.into(), placement));
            }
            Some(allocate_global_panes(
                area,
                &candidate,
                false,
                self.chrome_rows,
            )?)
        } else {
            None
        };
        self.panes.insert(
            insertion_index,
            GlobalPane {
                pane,
                placement,
                hidden,
                layout_applied: false,
                declaration_order,
                frame_applied: None,
            },
        );
        for (index, global_pane) in self.panes.iter().enumerate().skip(insertion_index) {
            self.pane_indices.insert(global_pane.pane.pid(), index);
        }
        if let Some(layout) = layout {
            if let Err(error) = self.apply_layout(layout) {
                self.panes.remove(insertion_index);
                self.pane_indices.remove(&pane_id);
                for (index, global_pane) in self.panes.iter().enumerate().skip(insertion_index) {
                    self.pane_indices.insert(global_pane.pane.pid(), index);
                }
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn remove(&mut self, pane_id: PaneId) -> Option<Box<dyn Pane>> {
        let pane_index = self.pane_indices.remove(&pane_id)?;
        let removed = self.panes.remove(pane_index);
        for (index, global_pane) in self.panes.iter().enumerate().skip(pane_index) {
            self.pane_indices.insert(global_pane.pane.pid(), index);
        }
        self.focused_panes
            .retain(|_, focused_pane_id| *focused_pane_id != pane_id);
        self.selecting_with_mouse
            .retain(|_, selecting_pane_id| *selecting_pane_id != pane_id);
        self.resizing_with_mouse
            .retain(|_, resizing_pane_id| *resizing_pane_id != pane_id);
        Some(removed.pane)
    }

    pub fn hide(&mut self, pane_id: PaneId) -> Result<(), String> {
        let pane_index = self.pane_index(pane_id)?;
        if self.panes[pane_index].hidden {
            return Ok(());
        }
        let layout = if let Some(area) = self.area {
            let candidate: Vec<_> = self
                .panes
                .iter()
                .filter(|global_pane| global_pane.pane.pid() != pane_id && !global_pane.hidden)
                .map(|global_pane| (global_pane.pane.pid().into(), global_pane.placement))
                .collect();
            Some(allocate_global_panes(
                area,
                &candidate,
                true,
                self.chrome_rows,
            )?)
        } else {
            None
        };
        if let Some(layout) = layout {
            self.apply_layout(layout)?;
        }
        self.panes[pane_index].hidden = true;
        self.focused_panes
            .retain(|_, focused_pane_id| *focused_pane_id != pane_id);
        self.selecting_with_mouse
            .retain(|_, selecting_pane_id| *selecting_pane_id != pane_id);
        self.resizing_with_mouse
            .retain(|_, resizing_pane_id| *resizing_pane_id != pane_id);
        Ok(())
    }

    pub fn show(&mut self, pane_id: PaneId, area: Viewport) -> Result<(), String> {
        let pane_index = self.pane_index(pane_id)?;
        if !self.panes[pane_index].hidden {
            return self.resize(area).map(|_| ());
        }
        self.reject_occupied_edge(self.panes[pane_index].placement.edge, Some(pane_id))?;
        let candidate: Vec<_> = self
            .panes
            .iter()
            .filter(|global_pane| !global_pane.hidden || global_pane.pane.pid() == pane_id)
            .map(|global_pane| (global_pane.pane.pid().into(), global_pane.placement))
            .collect();
        let layout = allocate_global_panes(area, &candidate, false, self.chrome_rows)?;
        self.apply_layout(layout)?;
        self.panes[pane_index].hidden = false;
        self.area = Some(area);
        Ok(())
    }

    pub fn set_placement(
        &mut self,
        pane_id: PaneId,
        placement: GlobalPanePlacement,
        area: Viewport,
    ) -> Result<(), String> {
        let pane_index = self.pane_index(pane_id)?;
        let target_is_hidden = self.panes[pane_index].hidden;
        if !target_is_hidden {
            // a hidden pane can be parked on an occupied edge; `show` rejects it then
            self.reject_occupied_edge(placement.edge, Some(pane_id))?;
        }
        let candidate: Vec<_> = self
            .panes
            .iter()
            .filter(|global_pane| !global_pane.hidden || global_pane.pane.pid() == pane_id)
            .map(|global_pane| {
                let candidate_placement = if global_pane.pane.pid() == pane_id {
                    placement
                } else {
                    global_pane.placement
                };
                (global_pane.pane.pid().into(), candidate_placement)
            })
            .collect();
        // Hidden panes are validated as if visible (candidate includes visibles + hidden).
        // A hidden 60col right pane plus visible 60col left pane on 100col surface is
        // therefore rejected here even though the hidden pane alone fits; `show()` would
        // reject it later anyway. This keeps `set_placement` for hidden panes conservative.
        // If hidden-only validation is desired, replace `candidate` with `vec![(pane_id.into(), placement)]`.
        let candidate_layout = allocate_global_panes(area, &candidate, false, self.chrome_rows)?;
        let layout = if target_is_hidden {
            allocate_global_panes(area, &self.visible_placements(), false, self.chrome_rows)?
        } else {
            candidate_layout
        };
        self.apply_layout(layout)?;
        self.panes[pane_index].placement = placement;
        self.area = Some(area);
        Ok(())
    }

    /// The tab's ui strips (tab bar, status bar) that a left/right global pane must not cover.
    /// Returns whether this changed anything, so the caller can skip a needless reflow.
    pub fn set_chrome_rows(&mut self, chrome_rows: (usize, usize)) -> bool {
        let changed = self.chrome_rows != chrome_rows;
        self.chrome_rows = chrome_rows;
        changed
    }

    /// The area the panes were last laid out over, `None` before the first `resize`.
    pub fn area(&self) -> Option<Viewport> {
        self.area
    }

    #[cfg(test)]
    pub fn center(&self, area: Viewport) -> Result<Viewport, String> {
        self.tab_layout(area).map(|layout| layout.center)
    }

    /// Where the global panes sit for a tab of `tab_area`, and the center that tab gets.
    ///
    /// A global pane is one `Box<dyn Pane>` with one grid, so its *size* cannot vary per tab -
    /// only its position can. Thicknesses are therefore resolved once against the session surface
    /// (`self.area`) and applied verbatim to every tab, and the cross-axis extent is copied from
    /// the session allocation. A right pane in a wider tab is the same strip, further right.
    ///
    /// This is only consistent because `Screen` pins the axis a global pane spans to the session
    /// size (see `Screen::global_pane_constrained_axes`): were a tab taller than the session while
    /// a side pane is visible, the strip would leave a band of that tab's column unowned.
    pub fn tab_layout(&self, tab_area: Viewport) -> Result<AllocatedGlobalPanes, String> {
        let placements = self.visible_placements();
        let Some(surface) = self.area else {
            // before the first reflow there is no session allocation to hold sizes to
            return allocate_global_panes(tab_area, &placements, true, self.chrome_rows);
        };
        let session = allocate_global_panes(surface, &placements, true, self.chrome_rows)?;
        // reuse the session allocation's own resolution rather than re-deriving percentages
        let resolved: Vec<_> = placements
            .iter()
            .map(|(pane_id, placement)| {
                let mut placement = *placement;
                if let Some(geom) = session.panes.get(pane_id) {
                    placement.size = PercentOrFixed::Fixed(match placement.edge {
                        GlobalPaneEdge::Left | GlobalPaneEdge::Right => geom.cols.as_usize(),
                        GlobalPaneEdge::Top | GlobalPaneEdge::Bottom => geom.rows.as_usize(),
                    });
                }
                (*pane_id, placement)
            })
            .collect();
        let mut layout = allocate_global_panes(tab_area, &resolved, true, self.chrome_rows)?;
        for (pane_id, geom) in layout.panes.iter_mut() {
            if let Some(session_geom) = session.panes.get(pane_id) {
                geom.cols = session_geom.cols;
                geom.rows = session_geom.rows;
            }
        }
        Ok(layout)
    }

    /// Move the panes to where they sit for the tab being rendered. Position only - the grids keep
    /// the size `resize` gave them, so no pty or plugin is resized here.
    ///
    /// ponytail: clients on differently sized tabs make the position alternate between render
    /// groups, so the panes repaint every frame while that holds. Diff per group if it turns out
    /// to be a common setup rather than a transient one.
    pub fn set_positions(&mut self, layout: &AllocatedGlobalPanes) {
        for (pane_id, geom) in &layout.panes {
            let Some(pane_index) = self.pane_indices.get(&PaneId::from(*pane_id)).copied() else {
                continue;
            };
            let pane = &mut self.panes[pane_index].pane;
            let current = pane.position_and_size();
            if (current.x, current.y) == (geom.x, geom.y) {
                continue;
            }
            pane.set_geom(PaneGeom {
                x: geom.x,
                y: geom.y,
                ..current
            });
            pane.set_should_render(true);
            pane.render_full_viewport();
        }
    }

    pub fn resize(&mut self, area: Viewport) -> Result<Viewport, String> {
        let layout =
            allocate_global_panes(area, &self.visible_placements(), true, self.chrome_rows)?;
        let center = layout.center;
        self.apply_layout(layout)?;
        self.area = Some(area);
        Ok(center)
    }

    pub fn set_pane_frame_style(&mut self, pane_frame_style: PaneFrameStyle) -> Result<(), String> {
        let previous_pane_frame_style = self.pane_frame_style;
        self.pane_frame_style = pane_frame_style;
        if let Some(area) = self.area {
            if let Err(error) = self.resize(area) {
                self.pane_frame_style = previous_pane_frame_style;
                return Err(error);
            }
        }
        self.set_force_render();
        Ok(())
    }

    pub fn set_force_render(&mut self) {
        for global_pane in self
            .panes
            .iter_mut()
            .filter(|global_pane| !global_pane.hidden)
        {
            global_pane.pane.set_should_render(true);
            global_pane.pane.render_full_viewport();
        }
    }

    pub fn visible_pane_ids(&self) -> HashSet<PaneId> {
        self.panes
            .iter()
            .filter(|global_pane| !global_pane.hidden)
            .map(|global_pane| global_pane.pane.pid())
            .collect()
    }

    /// Visible, selectable global panes in declaration order - the ones keyboard focus can reach.
    pub fn focusable_pane_ids(&self) -> Vec<PaneId> {
        self.panes
            .iter()
            .filter(|global_pane| !global_pane.hidden && global_pane.pane.selectable())
            .map(|global_pane| global_pane.pane.pid())
            .collect()
    }

    /// At most one visible global pane may occupy an edge (see `reject_occupied_edge`), so
    /// directional focus is an edge lookup rather than a geometry search.
    pub fn pane_id_on_edge(&self, edge: GlobalPaneEdge) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|global_pane| {
                !global_pane.hidden
                    && global_pane.pane.selectable()
                    && global_pane.placement.edge == edge
            })
            .map(|global_pane| global_pane.pane.pid())
    }

    pub fn plugin_ids(&self) -> Vec<u32> {
        self.panes
            .iter()
            .filter_map(|global_pane| match global_pane.pane.pid() {
                PaneId::Plugin(plugin_id) => Some(plugin_id),
                PaneId::Terminal(_) => None,
            })
            .collect()
    }

    pub fn find_plugin(&self, run_plugin_or_alias: &RunPluginOrAlias) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|global_pane| {
                matches!(global_pane.pane.pid(), PaneId::Plugin(_))
                    && run_plugin_or_alias.is_equivalent_to_run(global_pane.pane.invoked_with())
            })
            .map(|global_pane| global_pane.pane.pid())
    }

    pub fn handle_pty_bytes(&mut self, terminal_id: u32, bytes: VteBytes) -> bool {
        let Some(pane) = self.pane_mut(PaneId::Terminal(terminal_id)) else {
            return false;
        };
        pane.handle_pty_bytes(bytes);
        true
    }

    pub fn handle_plugin_bytes(
        &mut self,
        plugin_id: u32,
        client_id: ClientId,
        bytes: VteBytes,
    ) -> bool {
        let Some(pane) = self.pane_mut(PaneId::Plugin(plugin_id)) else {
            return false;
        };
        pane.handle_plugin_bytes(client_id, bytes);
        true
    }

    pub fn terminal_pane_count(&self) -> usize {
        self.panes
            .iter()
            .filter(|global_pane| matches!(global_pane.pane.pid(), PaneId::Terminal(_)))
            .count()
    }

    pub fn focus_pane(&mut self, pane_id: PaneId, client_id: ClientId) -> Result<(), String> {
        let pane_index = self.pane_index(pane_id)?;
        if self.panes[pane_index].hidden {
            return Err(format!("cannot focus hidden global pane {pane_id:?}"));
        }
        self.focused_panes.insert(client_id, pane_id);
        Ok(())
    }

    pub fn defocus_pane(&mut self, client_id: ClientId) {
        self.focused_panes.remove(&client_id);
    }

    pub fn focused_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.focused_panes.get(&client_id).copied()
    }

    pub fn focused_clients(&self) -> HashSet<ClientId> {
        self.focused_panes.keys().copied().collect()
    }

    pub fn placement(&self, pane_id: PaneId) -> Option<GlobalPanePlacement> {
        self.pane_indices
            .get(&pane_id)
            .map(|pane_index| self.panes[*pane_index].placement)
    }

    pub fn panes(&self) -> impl Iterator<Item = (&dyn Pane, GlobalPanePlacement, bool)> {
        self.panes.iter().map(|global_pane| {
            (
                global_pane.pane.as_ref(),
                global_pane.placement,
                global_pane.hidden,
            )
        })
    }

    #[cfg(test)]
    pub fn is_hidden(&self, pane_id: PaneId) -> Result<bool, String> {
        self.pane_index(pane_id)
            .map(|pane_index| self.panes[pane_index].hidden)
    }

    pub fn pane_id_at(&self, position: &zellij_utils::position::Position) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|global_pane| !global_pane.hidden && global_pane.pane.contains(position))
            .map(|global_pane| global_pane.pane.pid())
    }

    /// The global pane whose separator - the cell carrying the border it shares with the tab area -
    /// is under `position`. A left or top pane reserves that cell out of its own content (see
    /// `apply_layout`), for a right or bottom one it is the last cell of the tab pane beside it, so
    /// this never overlaps any pane's content.
    pub fn border_pane_id_at(&self, position: &zellij_utils::position::Position) -> Option<PaneId> {
        let line = position.line.0;
        let column = position.column.0 as isize;
        self.panes
            .iter()
            .filter(|global_pane| !global_pane.hidden)
            .find(|global_pane| {
                let geom = global_pane.pane.position_and_size();
                let (x, y) = (geom.x as isize, geom.y as isize);
                let (cols, rows) = (geom.cols.as_usize() as isize, geom.rows.as_usize() as isize);
                let within_rows = line >= y && line < y + rows;
                let within_cols = column >= x && column < x + cols;
                match global_pane.placement.edge {
                    GlobalPaneEdge::Left => within_rows && column == x + cols - 1,
                    GlobalPaneEdge::Right => within_rows && column == x - 1,
                    GlobalPaneEdge::Top => within_cols && line == y + rows - 1,
                    GlobalPaneEdge::Bottom => within_cols && line == y - 1,
                }
            })
            .map(|global_pane| global_pane.pane.pid())
    }

    pub fn start_mouse_resize(&mut self, pane_id: PaneId, client_id: ClientId) {
        self.resizing_with_mouse.insert(client_id, pane_id);
    }

    pub fn mouse_resize_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.resizing_with_mouse.get(&client_id).copied()
    }

    pub fn stop_mouse_resize(&mut self, client_id: ClientId) {
        self.resizing_with_mouse.remove(&client_id);
    }

    pub fn stop_selecting_with_mouse(&mut self, client_id: ClientId) {
        self.selecting_with_mouse.remove(&client_id);
    }

    /// Move the border of `pane_id` to `position`, sizing the pane from the absolute pointer
    /// position rather than from a delta, so a drag can never accumulate drift. Returns whether
    /// anything changed, so the caller can skip re-flowing every tab for a no-op motion.
    ///
    /// ponytail: a percentage-sized pane is re-derived against the whole area rather than against
    /// what the panes allocated before it left over, so dragging one that sits behind another pane
    /// on the same axis lags the pointer slightly. Track the per-pane basis if that combination
    /// ever gets used.
    pub fn resize_pane_to_position(
        &mut self,
        pane_id: PaneId,
        position: &zellij_utils::position::Position,
        area: Viewport,
    ) -> Result<bool, String> {
        let mut placement = self
            .placement(pane_id)
            .ok_or_else(|| format!("global pane {pane_id:?} not found"))?;
        let geom = self
            .pane(pane_id)
            .ok_or_else(|| format!("global pane {pane_id:?} not found"))?
            .position_and_size();
        let line = position.line.0;
        let column = position.column.0 as isize;
        let (thickness, axis) = match placement.edge {
            GlobalPaneEdge::Left => (column - geom.x as isize + 1, area.cols),
            GlobalPaneEdge::Right => {
                let right_edge = (geom.x + geom.cols.as_usize()) as isize;
                (right_edge - column, area.cols)
            },
            GlobalPaneEdge::Top => (line - geom.y as isize + 1, area.rows),
            GlobalPaneEdge::Bottom => {
                let bottom_edge = (geom.y + geom.rows.as_usize()) as isize;
                (bottom_edge - line, area.rows)
            },
        };
        let thickness = thickness.clamp(1, axis.saturating_sub(1).max(1) as isize) as usize;
        let size = match placement.size {
            PercentOrFixed::Fixed(current) if current == thickness => return Ok(false),
            PercentOrFixed::Fixed(_) => PercentOrFixed::Fixed(thickness),
            PercentOrFixed::Percent(_) => {
                PercentOrFixed::Percent(((thickness * 100) / axis.max(1)).max(1))
            },
        };
        if size == placement.size {
            return Ok(false);
        }
        placement.size = size;
        self.set_placement(pane_id, placement, area).map(|_| true)
    }

    pub fn pane_id_for_mouse_event(
        &mut self,
        event: &zellij_utils::input::mouse::MouseEvent,
        client_id: ClientId,
    ) -> Option<PaneId> {
        // only a real button press starts over - a wheel event is also a `Press` and must not
        // cancel an in-progress drag (the tab path is a no-op for a wheel while selecting)
        if event.event_type == zellij_utils::input::mouse::MouseEventType::Press
            && (event.left || event.right || event.middle)
        {
            self.selecting_with_mouse.remove(&client_id);
            return self.pane_id_at(&event.position);
        }
        self.selecting_with_mouse
            .get(&client_id)
            .copied()
            .or_else(|| self.pane_id_at(&event.position))
    }

    pub fn handle_mouse_event(
        &mut self,
        pane_id: PaneId,
        event: &zellij_utils::input::mouse::MouseEvent,
        client_id: ClientId,
    ) -> Option<GlobalPaneMouseEffect> {
        use zellij_utils::input::mouse::MouseEventType;

        let selecting_this_pane = self.selecting_with_mouse.get(&client_id) == Some(&pane_id);
        let pane = self.pane_mut(pane_id)?;
        let mut relative_event = *event;
        relative_event.position = pane.relative_position(&event.position);

        // an in-progress drag is resolved before the frame bailout below: a drag that wanders onto
        // the frame (a left/top pane reserves a cell there for the tab separator and another for
        // its title) must keep updating, and a release there must end the selection - otherwise
        // `selecting_with_mouse` stays set and this client's mouse remains captured
        if selecting_this_pane {
            match event.event_type {
                MouseEventType::Motion if event.left => {
                    pane.update_selection(&relative_event.position, client_id);
                    return None;
                },
                MouseEventType::Release if event.left => {
                    let mut end = relative_event.position;
                    end.change_column(end.column().min(pane.get_content_columns()));
                    end.change_line(end.line().max(0).min(pane.get_content_rows() as isize));
                    pane.end_selection(&end, client_id);
                    let selected_text = pane.get_selected_text(client_id);
                    self.selecting_with_mouse.remove(&client_id);
                    return selected_text.map(GlobalPaneMouseEffect::Copy);
                },
                // eg. a wheel mid-drag - the tab path is a no-op here too
                _ => return None,
            }
        }
        if pane.position_is_on_frame(&event.position) {
            return None;
        }

        if event.wheel_up {
            return pane
                .mouse_scroll_up(&relative_event.position)
                .map(|bytes| GlobalPaneMouseEffect::WriteRaw(bytes.into_bytes()))
                .or_else(|| {
                    if pane.is_alternate_mode_active() {
                        Some(GlobalPaneMouseEffect::WriteAdjusted(vec![
                            b"\x1b[A".to_vec();
                            3
                        ]))
                    } else {
                        pane.scroll_up(3, client_id);
                        None
                    }
                });
        }
        if event.wheel_down {
            return pane
                .mouse_scroll_down(&relative_event.position)
                .map(|bytes| GlobalPaneMouseEffect::WriteRaw(bytes.into_bytes()))
                .or_else(|| {
                    if pane.is_alternate_mode_active() {
                        Some(GlobalPaneMouseEffect::WriteAdjusted(vec![
                            b"\x1b[B".to_vec();
                            3
                        ]))
                    } else {
                        pane.scroll_down(3, client_id);
                        None
                    }
                });
        }
        if event.right
            && event.event_type == MouseEventType::Press
            && matches!(pane_id, PaneId::Plugin(_))
        {
            pane.handle_right_click(&relative_event.position, client_id);
            return None;
        }
        if event.left && event.event_type == MouseEventType::Press {
            if pane.terminal_emulator_wants_mouse() {
                return pane
                    .mouse_event(&relative_event, client_id)
                    .map(|bytes| GlobalPaneMouseEffect::WriteRaw(bytes.into_bytes()));
            }
            // deliberately not gated on `supports_mouse_selection` the way the tab path is: a
            // plugin that opts out still routes its `Mouse::LeftClick/Hold/Release` through these
            // three calls (see `PluginPane::start_selection`), so it needs the drag tracked too
            pane.start_selection(&relative_event.position, client_id);
            self.selecting_with_mouse.insert(client_id, pane_id);
            return None;
        }
        pane.mouse_event(&relative_event, client_id)
            .map(|bytes| GlobalPaneMouseEffect::WriteRaw(bytes.into_bytes()))
    }

    /// A global pane has one movable border, the one facing the tab, so only a resize along its own
    /// axis means anything - `Resize "Increase" "Up"` on a left pane is nothing to do. Both
    /// directions along that axis grow (or shrink) it, the same way zellij inverts a resize made
    /// towards a screen boundary.
    pub fn resize_pane(
        &mut self,
        pane_id: PaneId,
        strategy: ResizeStrategy,
        area: Viewport,
    ) -> Result<(), String> {
        let mut placement = self
            .placement(pane_id)
            .ok_or_else(|| format!("global pane {pane_id:?} not found"))?;
        let is_horizontal = matches!(placement.edge, GlobalPaneEdge::Left | GlobalPaneEdge::Right);
        match strategy.direction {
            Some(Direction::Left) | Some(Direction::Right) if !is_horizontal => return Ok(()),
            Some(Direction::Up) | Some(Direction::Down) if is_horizontal => return Ok(()),
            _ => {},
        }
        placement.size = match (placement.size, strategy.resize) {
            (PercentOrFixed::Fixed(size), Resize::Increase) => {
                PercentOrFixed::Fixed(size.saturating_add(1))
            },
            (PercentOrFixed::Fixed(size), Resize::Decrease) => {
                PercentOrFixed::Fixed(size.saturating_sub(1))
            },
            (PercentOrFixed::Percent(size), Resize::Increase) => {
                PercentOrFixed::Percent(size.saturating_add(5))
            },
            (PercentOrFixed::Percent(size), Resize::Decrease) => {
                PercentOrFixed::Percent(size.saturating_sub(5))
            },
        };
        self.set_placement(pane_id, placement, area)
    }

    pub fn update_input_modes(&self, mode_infos: &[(ClientId, ModeInfo)]) -> Result<()> {
        let plugin_ids = self.plugin_ids();
        let mut plugin_updates = vec![];
        for (client_id, mode_info) in mode_infos {
            for plugin_id in &plugin_ids {
                plugin_updates.push((
                    Some(*plugin_id),
                    Some(*client_id),
                    Event::ModeUpdate(mode_info.clone()),
                ));
            }
        }
        if !plugin_updates.is_empty() {
            self.senders
                .send_to_plugin(PluginInstruction::Update(plugin_updates))
                .context("failed to update global plugins with mode info")?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        output: &mut Output,
        client_ids: &[ClientId],
        mode_info: &BTreeMap<ClientId, ModeInfo>,
        default_mode_info: &ModeInfo,
        style: Style,
        session_is_mirrored: bool,
        multiple_users_exist_in_session: bool,
    ) -> Result<()> {
        let err_context = || "failed to render global panes";
        if client_ids.is_empty() {
            return Ok(());
        }
        let focused_panes: HashMap<_, _> = self
            .focused_panes
            .iter()
            .map(|(client_id, pane_id)| (*client_id, *pane_id))
            .collect();

        for global_pane in self
            .panes
            .iter_mut()
            .filter(|global_pane| !global_pane.hidden)
        {
            let pane_id = global_pane.pane.pid();
            match pane_id {
                PaneId::Terminal(_) => {
                    output.add_pane_contents(
                        client_ids,
                        pane_id,
                        global_pane.pane.pane_contents(None, false, None),
                    );
                    if output.collect_ansi_pane_contents {
                        output.add_pane_contents_with_ansi(
                            client_ids,
                            pane_id,
                            global_pane.pane.pane_contents_with_ansi(None, false, None),
                        );
                    }
                },
                PaneId::Plugin(_) => {
                    for client_id in client_ids {
                        output.add_pane_contents(
                            &[*client_id],
                            pane_id,
                            global_pane
                                .pane
                                .pane_contents(Some(*client_id), false, None),
                        );
                        if output.collect_ansi_pane_contents {
                            output.add_pane_contents_with_ansi(
                                &[*client_id],
                                pane_id,
                                global_pane.pane.pane_contents_with_ansi(
                                    Some(*client_id),
                                    false,
                                    None,
                                ),
                            );
                        }
                    }
                },
            }

            let pane_is_selectable = global_pane.pane.selectable();
            let mut pane_contents_and_ui = PaneContentsAndUi::new(
                &mut global_pane.pane,
                output,
                style,
                &focused_panes,
                multiple_users_exist_in_session,
                None,
                false,
                false,
                self.pane_frame_style.draws_full_frames(),
                &HashMap::new(),
                HashMap::new(),
                false,
                false,
                false,
                HashSet::new(),
            );
            for client_id in client_ids {
                let client_mode = mode_info.get(client_id).unwrap_or(default_mode_info).mode;
                if matches!(pane_id, PaneId::Plugin(_)) {
                    pane_contents_and_ui
                        .render_pane_contents_for_client(*client_id)
                        .with_context(err_context)?;
                }
                if self.pane_frame_style.draws_full_frames() || self.pane_frame_style.draws_titles()
                {
                    pane_contents_and_ui
                        .render_pane_frame(
                            *client_id,
                            client_mode,
                            session_is_mirrored,
                            false,
                            pane_is_selectable,
                        )
                        .with_context(err_context)?;
                }
                // a fake cursor moves the host terminal cursor as a side effect, and global panes
                // render after the tab positioned each client's cursor - bracket it with
                // save/restore so clients not focused on this pane keep their cursor position
                let will_render_fake_cursor = focused_panes
                    .values()
                    .any(|focused_pane_id| *focused_pane_id == pane_id)
                    && focused_panes.get(client_id) != Some(&pane_id);
                if will_render_fake_cursor {
                    pane_contents_and_ui.save_host_cursor_position(*client_id);
                }
                pane_contents_and_ui
                    .render_fake_cursor_if_needed(*client_id)
                    .with_context(err_context)?;
                if will_render_fake_cursor {
                    pane_contents_and_ui.restore_host_cursor_position(*client_id);
                }
            }
            if matches!(pane_id, PaneId::Terminal(_)) {
                pane_contents_and_ui
                    .render_pane_contents_to_multiple_clients(client_ids.iter().copied())
                    .with_context(err_context)?;
            }
            drop(pane_contents_and_ui);
            for client_id in client_ids {
                if focused_panes.get(client_id) != Some(&pane_id) {
                    continue;
                }
                if let Some((cursor_x, cursor_y, is_visible)) =
                    global_pane.pane.cursor_coordinates(Some(*client_id))
                {
                    let cursor_x = global_pane.pane.x() + cursor_x;
                    let cursor_y = global_pane.pane.y() + cursor_y;
                    output.set_cursor_coordinates(*client_id, cursor_x, cursor_y);
                    if output.cursor_is_visible(cursor_x, cursor_y, None) {
                        output.add_post_vte_instruction_to_client(
                            *client_id,
                            if is_visible {
                                "\u{1b}[?25h"
                            } else {
                                "\u{1b}[?25l"
                            },
                        );
                        output.add_post_vte_instruction_to_client(
                            *client_id,
                            &format!(
                                "\u{1b}[{};{}H\u{1b}[m{}",
                                cursor_y + 1,
                                cursor_x + 1,
                                global_pane.pane.cursor_shape_csi()
                            ),
                        );
                    }
                } else {
                    output.add_post_vte_instruction_to_client(*client_id, "\u{1b}[?25l");
                }
            }
        }
        Ok(())
    }

    pub fn pane(&self, pane_id: PaneId) -> Option<&dyn Pane> {
        self.pane_indices
            .get(&pane_id)
            .map(|pane_index| self.panes[*pane_index].pane.as_ref())
    }

    pub fn pane_mut(&mut self, pane_id: PaneId) -> Option<&mut (dyn Pane + '_)> {
        let pane_index = *self.pane_indices.get(&pane_id)?;
        Some(self.panes[pane_index].pane.as_mut())
    }

    fn pane_index(&self, pane_id: PaneId) -> Result<usize, String> {
        self.pane_indices
            .get(&pane_id)
            .copied()
            .ok_or_else(|| format!("global pane {pane_id:?} not found"))
    }

    /// ponytail: one visible global pane per edge. Panes stacked on the same edge have no separator
    /// geometry between them (only strips are grown over, they never contribute rects of their own -
    /// see `expand_viewport_over_global_panes`), so they render a blank gutter instead of a boundary.
    /// Reject the placement rather than half-supporting it; add global/global separators to the
    /// boundary model if same-edge stacking ever becomes a real layout.
    fn reject_occupied_edge(
        &self,
        edge: GlobalPaneEdge,
        except: Option<PaneId>,
    ) -> Result<(), String> {
        let occupied = self.panes.iter().any(|global_pane| {
            !global_pane.hidden
                && global_pane.placement.edge == edge
                && Some(global_pane.pane.pid()) != except
        });
        if occupied {
            let edge = match edge {
                GlobalPaneEdge::Left => "left",
                GlobalPaneEdge::Right => "right",
                GlobalPaneEdge::Top => "top",
                GlobalPaneEdge::Bottom => "bottom",
            };
            return Err(format!(
                "a global pane is already docked to the {edge} edge"
            ));
        }
        Ok(())
    }

    fn visible_placements(&self) -> Vec<(ZellijUtilsPaneId, GlobalPanePlacement)> {
        self.panes
            .iter()
            .filter(|global_pane| !global_pane.hidden)
            .map(|global_pane| (global_pane.pane.pid().into(), global_pane.placement))
            .collect()
    }

    fn apply_layout(&mut self, layout: AllocatedGlobalPanes) -> Result<(), String> {
        let desired_layout: Vec<_> = self
            .panes
            .iter()
            .enumerate()
            .filter_map(|(pane_index, global_pane)| {
                layout
                    .panes
                    .get(&global_pane.pane.pid().into())
                    .copied()
                    .map(|geom| {
                        let borderless = global_pane.pane.borderless();
                        let content_offset =
                            if self.pane_frame_style.draws_full_frames() && !borderless {
                                Offset::frame(1)
                            } else {
                                // Reserve the cell facing the tab area, so the boundary line the tab's
                                // edge pane draws there (at `rect.x() - 1` / `rect.y() - 1`, see
                                // `Boundaries::add_rect`) lands on a cell we are not writing content
                                // into. Right and Bottom global panes need no reservation: there the
                                // boundary falls on the tab pane's own last column/row, which its
                                // `pane_content_offset` already reserves.
                                //
                                // `borderless` suppresses the pane's own frame, not the tab/global
                                // separator: the tab grows its boundary viewport over every visible
                                // strip (see `expand_viewport_over_global_panes`) and draws that line
                                // regardless, so the cell is reserved either way.
                                let mut content_offset = Offset {
                                    top: if !borderless && self.pane_frame_style.draws_titles() {
                                        1
                                    } else {
                                        0
                                    },
                                    ..Default::default()
                                };
                                match global_pane.placement.edge {
                                    GlobalPaneEdge::Left => content_offset.right = 1,
                                    GlobalPaneEdge::Top => content_offset.bottom = 1,
                                    GlobalPaneEdge::Right | GlobalPaneEdge::Bottom => {},
                                }
                                content_offset
                            };
                        (pane_index, geom, content_offset)
                    })
            })
            .collect();
        if desired_layout.len() != layout.panes.len() {
            return Err("allocated layout contains an unknown global pane".to_owned());
        }
        let desired_layout: Vec<_> = desired_layout
            .into_iter()
            .map(|(pane_index, geom, content_offset)| {
                let global_pane = &self.panes[pane_index];
                let current = global_pane.pane.position_and_size();
                // a pane the render loop moved to another tab's position (see `set_positions`) is
                // not a resize: only the size and content offset reach the pty or the plugin
                let size_changed = !global_pane.layout_applied
                    || current.cols != geom.cols
                    || current.rows != geom.rows
                    || global_pane.pane.get_content_offset() != content_offset;
                let moved = (current.x, current.y) != (geom.x, geom.y);
                (pane_index, geom, content_offset, size_changed, moved)
            })
            .collect();

        // ponytail: channel sends are not transactional. A successful resize can be observed by
        // an external worker before a later pane's send fails, but in-memory state is committed
        // only after every send succeeds.
        for (pane_index, geom, content_offset, size_changed, _moved) in &desired_layout {
            if *size_changed {
                self.send_resize(self.panes[*pane_index].pane.pid(), *geom, *content_offset)?;
            }
        }

        for (pane_index, geom, content_offset, size_changed, moved) in desired_layout {
            let global_pane = &mut self.panes[pane_index];
            let should_frame =
                self.pane_frame_style.draws_full_frames() && !global_pane.pane.borderless();
            if global_pane.frame_applied != Some(should_frame) {
                global_pane.pane.set_frame(should_frame);
                global_pane.frame_applied = Some(should_frame);
            }
            if size_changed || moved {
                global_pane.pane.set_geom(geom);
                global_pane.pane.set_content_offset(content_offset);
                global_pane.pane.set_should_render(true);
                global_pane.layout_applied = true;
            }
        }
        Ok(())
    }

    fn send_resize(
        &self,
        pane_id: PaneId,
        geom: PaneGeom,
        content_offset: Offset,
    ) -> Result<(), String> {
        let content_columns = geom
            .cols
            .as_usize()
            .saturating_sub(content_offset.left + content_offset.right);
        let content_rows = geom
            .rows
            .as_usize()
            .saturating_sub(content_offset.top + content_offset.bottom);
        let (width_in_pixels, height_in_pixels) = match *self.character_cell_size.borrow() {
            Some(size_in_pixels) => (
                Some((size_in_pixels.width * content_columns) as u16),
                Some((size_in_pixels.height * content_rows) as u16),
            ),
            None => (None, None),
        };
        match pane_id {
            PaneId::Terminal(terminal_id) => self
                .senders
                .send_to_pty_writer(PtyWriteInstruction::ResizePty(
                    terminal_id,
                    content_columns as u16,
                    content_rows as u16,
                    width_in_pixels,
                    height_in_pixels,
                ))
                .map_err(|error| error.to_string()),
            PaneId::Plugin(plugin_id) => self
                .senders
                .send_to_plugin(PluginInstruction::Resize(
                    plugin_id,
                    content_columns,
                    content_rows,
                ))
                .map_err(|error| error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{allocate_global_panes, GlobalPaneManager};
    use crate::{
        channels::SenderWithContext,
        panes::{KittyImageStore, LinkHandler, PaneId, PluginPane, SixelImageStore},
        plugins::PluginInstruction,
        tab::Pane,
    };
    use std::{cell::RefCell, collections::HashMap, rc::Rc};
    use zellij_utils::data::{Palette, Style};
    use zellij_utils::{
        channels,
        data::{GlobalPaneEdge, GlobalPanePlacement, ModeInfo, PaneId as ZellijUtilsPaneId},
        input::layout::PercentOrFixed,
        pane_size::{Dimension, Offset, PaneGeom, SizeInPixels, Viewport},
    };

    fn placement(edge: GlobalPaneEdge, size: PercentOrFixed) -> GlobalPanePlacement {
        GlobalPanePlacement { edge, size }
    }

    fn geom(x: usize, y: usize, cols: usize, rows: usize) -> PaneGeom {
        PaneGeom {
            x,
            y,
            cols: Dimension::fixed(cols),
            rows: Dimension::fixed(rows),
            ..Default::default()
        }
    }

    fn mock_pane(pane_id: PaneId) -> Box<dyn Pane> {
        let PaneId::Plugin(plugin_id) = pane_id else {
            panic!("global pane test mock must be a plugin")
        };
        let (plugin_sender, _plugin_receiver) = channels::unbounded();
        Box::new(PluginPane::new(
            plugin_id,
            PaneGeom::default(),
            SenderWithContext::<PluginInstruction>::new(plugin_sender),
            String::new(),
            String::new(),
            Rc::new(RefCell::new(SixelImageStore::default())),
            Rc::new(RefCell::new(KittyImageStore::default())),
            Rc::new(RefCell::new(Palette::default())),
            Rc::new(RefCell::new(HashMap::new())),
            Rc::new(RefCell::new(LinkHandler::new())),
            Rc::new(RefCell::new(None::<SizeInPixels>)),
            vec![],
            Style::default(),
            None,
            false,
            true,
            true,
        ))
    }

    fn left_percent(percent: usize) -> GlobalPanePlacement {
        placement(GlobalPaneEdge::Left, PercentOrFixed::Percent(percent))
    }

    #[test]
    fn left_then_bottom_allocates_outer_to_inner() {
        let area = Viewport {
            x: 0,
            y: 0,
            cols: 100,
            rows: 40,
        };
        let panes = [
            (
                ZellijUtilsPaneId::Plugin(1),
                placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
            ),
            (
                ZellijUtilsPaneId::Terminal(2),
                placement(GlobalPaneEdge::Bottom, PercentOrFixed::Fixed(10)),
            ),
        ];

        let layout = allocate_global_panes(area, &panes, false, (0, 0)).unwrap();
        assert_eq!(
            layout.panes[&ZellijUtilsPaneId::Plugin(1)],
            geom(0, 0, 20, 40)
        );
        assert_eq!(
            layout.panes[&ZellijUtilsPaneId::Terminal(2)],
            geom(20, 30, 80, 10)
        );
        assert_eq!(
            layout.center,
            Viewport {
                x: 20,
                y: 0,
                cols: 80,
                rows: 30
            }
        );
    }

    #[test]
    fn percentages_use_the_remaining_axis() {
        let area = Viewport {
            x: 0,
            y: 0,
            cols: 100,
            rows: 40,
        };
        let panes = [
            (
                ZellijUtilsPaneId::Terminal(1),
                placement(GlobalPaneEdge::Left, PercentOrFixed::Percent(20)),
            ),
            (
                ZellijUtilsPaneId::Terminal(2),
                placement(GlobalPaneEdge::Right, PercentOrFixed::Percent(25)),
            ),
        ];
        let layout = allocate_global_panes(area, &panes, false, (0, 0)).unwrap();
        assert_eq!(
            layout.panes[&ZellijUtilsPaneId::Terminal(1)]
                .cols
                .as_usize(),
            20
        );
        assert_eq!(
            layout.panes[&ZellijUtilsPaneId::Terminal(2)]
                .cols
                .as_usize(),
            20
        );
        assert_eq!(layout.center.cols, 60);
    }

    #[test]
    fn allocation_rejects_a_zero_sized_center() {
        let result = allocate_global_panes(
            Viewport {
                x: 0,
                y: 0,
                cols: 20,
                rows: 10,
            },
            &[(
                ZellijUtilsPaneId::Terminal(1),
                placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
            )],
            false,
            (0, 0),
        );
        assert_eq!(result.unwrap_err(), "global panes leave no usable tab area");
    }

    #[test]
    fn clamped_allocation_keeps_an_oversized_pane_and_a_one_cell_tab_area() {
        let layout = allocate_global_panes(
            Viewport {
                x: 0,
                y: 0,
                cols: 20,
                rows: 10,
            },
            &[(
                ZellijUtilsPaneId::Terminal(1),
                placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(20)),
            )],
            true,
            (0, 0),
        )
        .unwrap();
        assert_eq!(
            layout.panes[&ZellijUtilsPaneId::Terminal(1)],
            geom(0, 0, 19, 10)
        );
        assert_eq!(
            layout.center,
            Viewport {
                x: 19,
                y: 0,
                cols: 1,
                rows: 10
            }
        );
    }

    #[test]
    fn clamped_allocation_keeps_a_percentage_that_rounds_to_zero_at_one_cell() {
        let layout = allocate_global_panes(
            Viewport {
                x: 0,
                y: 0,
                cols: 4,
                rows: 10,
            },
            &[(
                ZellijUtilsPaneId::Terminal(1),
                placement(GlobalPaneEdge::Left, PercentOrFixed::Percent(20)),
            )],
            true,
            (0, 0),
        )
        .unwrap();
        assert_eq!(
            layout.panes[&ZellijUtilsPaneId::Terminal(1)],
            geom(0, 0, 1, 10)
        );
        assert_eq!(layout.center.cols, 3);
    }

    #[test]
    fn clamped_allocation_still_rejects_an_axis_too_small_for_a_pane_and_a_tab_area() {
        let result = allocate_global_panes(
            Viewport {
                x: 0,
                y: 0,
                cols: 1,
                rows: 10,
            },
            &[(
                ZellijUtilsPaneId::Terminal(1),
                placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(1)),
            )],
            true,
            (0, 0),
        );
        assert_eq!(result.unwrap_err(), "global panes leave no usable tab area");
    }

    #[test]
    fn manager_preserves_order_and_ownership_through_visibility_changes() {
        let full_area = Viewport {
            cols: 120,
            rows: 40,
            ..Default::default()
        };
        let mut manager = GlobalPaneManager::default();

        manager
            .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
            .unwrap();
        assert_eq!(manager.center(full_area).unwrap().cols, 90);
        manager.hide(PaneId::Plugin(1)).unwrap();
        assert_eq!(manager.center(full_area).unwrap().cols, 120);
        manager.show(PaneId::Plugin(1), full_area).unwrap();
        assert_eq!(
            manager.pane(PaneId::Plugin(1)).unwrap().pid(),
            PaneId::Plugin(1)
        );
        manager.focus_pane(PaneId::Plugin(1), 7).unwrap();
        assert_eq!(manager.focused_pane_id(7), Some(PaneId::Plugin(1)));
        manager.defocus_pane(7);
        assert_eq!(manager.focused_pane_id(7), None);
        manager.focus_pane(PaneId::Plugin(1), 7).unwrap();
        manager
            .insert(mock_pane(PaneId::Plugin(2)), left_percent(10), true)
            .unwrap();
        manager
            .insert(mock_pane(PaneId::Plugin(3)), left_percent(10), true)
            .unwrap();
        assert_eq!(
            manager.remove(PaneId::Plugin(2)).unwrap().pid(),
            PaneId::Plugin(2)
        );
        assert_eq!(manager.pane_indices.get(&PaneId::Plugin(1)), Some(&0));
        assert_eq!(manager.pane_indices.get(&PaneId::Plugin(3)), Some(&1));
        assert_eq!(
            manager
                .panes
                .iter()
                .map(|global_pane| global_pane.pane.pid())
                .collect::<Vec<_>>(),
            vec![PaneId::Plugin(1), PaneId::Plugin(3)]
        );
        assert!(manager.remove(PaneId::Plugin(1)).is_some());
        assert_eq!(manager.focused_pane_id(7), None);
    }

    #[test]
    fn failed_show_preserves_hidden_state() {
        let full_area = Viewport {
            cols: 120,
            rows: 40,
            ..Default::default()
        };
        let mut manager = GlobalPaneManager::default();
        manager
            .insert(
                mock_pane(PaneId::Plugin(1)),
                placement(GlobalPaneEdge::Left, PercentOrFixed::Fixed(120)),
                true,
            )
            .unwrap();

        assert!(manager.show(PaneId::Plugin(1), full_area).is_err());
        assert_eq!(manager.center(full_area).unwrap(), full_area);
    }

    #[test]
    fn failed_placement_change_preserves_prior_placement() {
        let full_area = Viewport {
            cols: 120,
            rows: 40,
            ..Default::default()
        };
        let mut manager = GlobalPaneManager::default();
        manager
            .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
            .unwrap();

        assert!(manager
            .set_placement(
                PaneId::Plugin(1),
                placement(GlobalPaneEdge::Right, PercentOrFixed::Fixed(120)),
                full_area,
            )
            .is_err());
        assert_eq!(manager.center(full_area).unwrap().cols, 90);
    }

    #[test]
    fn hidden_placement_change_is_validated_without_revealing_the_pane() {
        let full_area = Viewport {
            cols: 120,
            rows: 40,
            ..Default::default()
        };
        let mut manager = GlobalPaneManager::default();
        manager
            .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), true)
            .unwrap();

        assert!(manager
            .set_placement(
                PaneId::Plugin(1),
                placement(GlobalPaneEdge::Right, PercentOrFixed::Fixed(120)),
                full_area,
            )
            .is_err());

        assert_eq!(manager.placement(PaneId::Plugin(1)), Some(left_percent(25)));
        assert!(manager.is_hidden(PaneId::Plugin(1)).unwrap());
        assert_eq!(manager.center(full_area).unwrap(), full_area);
    }

    #[test]
    fn resize_send_failure_restores_manager_and_pane_state() {
        let full_area = Viewport {
            cols: 120,
            rows: 40,
            ..Default::default()
        };
        let (plugin_sender, plugin_receiver) = channels::unbounded();
        drop(plugin_receiver);
        let senders = crate::thread_bus::ThreadSenders {
            to_plugin: Some(SenderWithContext::new(plugin_sender)),
            ..Default::default()
        };
        let mut manager =
            GlobalPaneManager::new(senders, Rc::new(RefCell::new(None)), Default::default());
        manager
            .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), true)
            .unwrap();
        let original_geom = manager.pane(PaneId::Plugin(1)).unwrap().position_and_size();
        let original_offset = manager
            .pane(PaneId::Plugin(1))
            .unwrap()
            .get_content_offset();
        let original_should_render = manager.pane(PaneId::Plugin(1)).unwrap().should_render();

        assert!(manager.show(PaneId::Plugin(1), full_area).is_err());

        assert!(manager.visible_pane_ids().is_empty());
        assert_eq!(manager.area, None);
        assert_eq!(manager.panes[0].placement, left_percent(25));
        assert!(manager.panes[0].hidden);
        assert!(!manager.panes[0].layout_applied);
        assert_eq!(manager.pane_indices.get(&PaneId::Plugin(1)), Some(&0));
        let pane = manager.pane(PaneId::Plugin(1)).unwrap();
        assert_eq!(pane.position_and_size(), original_geom);
        assert_eq!(pane.get_content_offset(), original_offset);
        assert_eq!(pane.should_render(), original_should_render);

        manager.resize(full_area).unwrap();
        assert!(manager
            .insert(mock_pane(PaneId::Plugin(2)), left_percent(25), false)
            .is_err());
        assert!(manager.pane(PaneId::Plugin(2)).is_none());
        assert_eq!(manager.pane_indices.get(&PaneId::Plugin(1)), Some(&0));
        assert_eq!(manager.panes.len(), 1);

        let (plugin_sender, plugin_receiver) = channels::unbounded();
        let senders = crate::thread_bus::ThreadSenders {
            to_plugin: Some(SenderWithContext::new(plugin_sender)),
            ..Default::default()
        };
        let mut visible_manager =
            GlobalPaneManager::new(senders, Rc::new(RefCell::new(None)), Default::default());
        visible_manager
            .insert(mock_pane(PaneId::Plugin(10)), left_percent(25), true)
            .unwrap();
        visible_manager.show(PaneId::Plugin(10), full_area).unwrap();
        plugin_receiver.recv().unwrap();
        let original_geom = visible_manager
            .pane(PaneId::Plugin(10))
            .unwrap()
            .position_and_size();
        let original_offset = visible_manager
            .pane(PaneId::Plugin(10))
            .unwrap()
            .get_content_offset();
        let original_should_render = visible_manager
            .pane(PaneId::Plugin(10))
            .unwrap()
            .should_render();
        drop(plugin_receiver);

        assert!(visible_manager
            .set_placement(
                PaneId::Plugin(10),
                placement(GlobalPaneEdge::Right, PercentOrFixed::Fixed(20)),
                full_area,
            )
            .is_err());
        assert_eq!(visible_manager.panes[0].placement, left_percent(25));
        assert_eq!(visible_manager.area, Some(full_area));
        let pane = visible_manager.pane(PaneId::Plugin(10)).unwrap();
        assert_eq!(pane.position_and_size(), original_geom);
        assert_eq!(pane.get_content_offset(), original_offset);
        assert_eq!(pane.should_render(), original_should_render);

        assert!(visible_manager
            .resize(Viewport {
                cols: 160,
                rows: 50,
                ..Default::default()
            })
            .is_err());
        assert_eq!(visible_manager.area, Some(full_area));
        let pane = visible_manager.pane(PaneId::Plugin(10)).unwrap();
        assert_eq!(pane.position_and_size(), original_geom);
        assert_eq!(pane.get_content_offset(), original_offset);
        assert_eq!(pane.should_render(), original_should_render);

        assert!(visible_manager
            .set_pane_frame_style(zellij_utils::input::options::PaneFrameStyle::Full)
            .is_err());
        assert_eq!(
            visible_manager.pane_frame_style,
            zellij_utils::input::options::PaneFrameStyle::Titles
        );
        let pane = visible_manager.pane(PaneId::Plugin(10)).unwrap();
        assert_eq!(pane.position_and_size(), original_geom);
        assert_eq!(pane.get_content_offset(), original_offset);
        assert_eq!(pane.should_render(), original_should_render);
    }

    #[test]
    fn title_frames_reserve_the_top_content_row() {
        let mut manager = GlobalPaneManager::new(
            crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
            Rc::new(RefCell::new(None)),
            zellij_utils::input::options::PaneFrameStyle::Titles,
        );
        manager
            .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
            .unwrap();
        manager
            .resize(Viewport {
                cols: 120,
                rows: 40,
                ..Default::default()
            })
            .unwrap();

        let pane = manager.pane(PaneId::Plugin(1)).unwrap();
        // right: 1 is the boundary cell, see `the_edge_facing_the_tab_reserves_a_boundary_cell`
        assert_eq!(
            pane.get_content_offset(),
            Offset::shift_right_top_and_bottom(1, 1, 0)
        );
        assert_eq!(pane.get_content_rows(), 39);
    }

    #[test]
    fn the_edge_facing_the_tab_reserves_a_boundary_cell() {
        let area = Viewport {
            cols: 120,
            rows: 40,
            ..Default::default()
        };
        let mut manager = GlobalPaneManager::new(
            crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
            Rc::new(RefCell::new(None)),
            zellij_utils::input::options::PaneFrameStyle::Titles,
        );
        manager
            .insert(mock_pane(PaneId::Plugin(1)), left_percent(25), false)
            .unwrap();
        manager
            .insert(
                mock_pane(PaneId::Plugin(2)),
                placement(GlobalPaneEdge::Right, PercentOrFixed::Percent(25)),
                false,
            )
            .unwrap();
        manager.resize(area).unwrap();

        // a Left pane reserves its own last column for the line the tab's edge pane draws there
        assert_eq!(
            manager
                .pane(PaneId::Plugin(1))
                .unwrap()
                .get_content_offset(),
            Offset {
                top: 1,
                right: 1,
                ..Default::default()
            }
        );
        // a Right pane reserves nothing: there the line falls on the tab pane's own last column
        assert_eq!(
            manager
                .pane(PaneId::Plugin(2))
                .unwrap()
                .get_content_offset(),
            Offset {
                top: 1,
                ..Default::default()
            }
        );
    }

    #[test]
    fn borderless_panes_still_reserve_the_edge_facing_the_tab() {
        // `borderless` drops the pane's own frame, not the tab/global separator: the tab draws that
        // line one cell inside a Left/Top global pane regardless, so the cell stays reserved.
        let area = Viewport {
            cols: 120,
            rows: 40,
            ..Default::default()
        };
        for pane_frame_style in [
            zellij_utils::input::options::PaneFrameStyle::None,
            zellij_utils::input::options::PaneFrameStyle::Titles,
            zellij_utils::input::options::PaneFrameStyle::Full,
        ] {
            let mut manager = GlobalPaneManager::new(
                crate::thread_bus::ThreadSenders::default().silently_fail_on_send(),
                Rc::new(RefCell::new(None)),
                pane_frame_style,
            );
            let mut left = mock_pane(PaneId::Plugin(1));
            left.set_borderless(true);
            let mut top = mock_pane(PaneId::Plugin(2));
            top.set_borderless(true);
            let mut right = mock_pane(PaneId::Plugin(3));
            right.set_borderless(true);
            manager.insert(left, left_percent(25), false).unwrap();
            manager
                .insert(
                    top,
                    placement(GlobalPaneEdge::Top, PercentOrFixed::Percent(25)),
                    false,
                )
                .unwrap();
            manager
                .insert(
                    right,
                    placement(GlobalPaneEdge::Right, PercentOrFixed::Percent(25)),
                    false,
                )
                .unwrap();
            manager.resize(area).unwrap();

            // no title row is reserved for a borderless pane, in any frame style
            assert_eq!(
                manager
                    .pane(PaneId::Plugin(1))
                    .unwrap()
                    .get_content_offset(),
                Offset {
                    right: 1,
                    ..Default::default()
                },
                "borderless Left pane in {pane_frame_style:?}"
            );
            assert_eq!(
                manager
                    .pane(PaneId::Plugin(2))
                    .unwrap()
                    .get_content_offset(),
                Offset {
                    bottom: 1,
                    ..Default::default()
                },
                "borderless Top pane in {pane_frame_style:?}"
            );
            assert_eq!(
                manager
                    .pane(PaneId::Plugin(3))
                    .unwrap()
                    .get_content_offset(),
                Offset::default(),
                "borderless Right pane in {pane_frame_style:?}"
            );
        }
    }

    #[test]
    fn the_boundary_viewport_grows_only_over_adjacent_strips() {
        use super::{expand_viewport_over_global_panes, GlobalPaneStrip};

        let tab_viewport = Viewport {
            x: 20,
            y: 1,
            cols: 100,
            rows: 38,
        };
        let strip = |edge, x, y, cols, rows| GlobalPaneStrip {
            geom: geom(x, y, cols, rows),
            edge,
        };

        // no strips at all leaves the viewport untouched
        assert_eq!(
            expand_viewport_over_global_panes(tab_viewport, &[]),
            tab_viewport
        );

        // a left strip ending exactly where the tab begins is adjacent
        assert_eq!(
            expand_viewport_over_global_panes(
                tab_viewport,
                &[strip(GlobalPaneEdge::Left, 0, 0, 20, 40)]
            ),
            Viewport {
                x: 0,
                y: 1,
                cols: 120,
                rows: 38
            }
        );

        // a right strip grows the far edge, which is what makes the tab's rightmost pane reserve
        // its own last column for the boundary (see `pane_content_offset`)
        assert_eq!(
            expand_viewport_over_global_panes(
                tab_viewport,
                &[strip(GlobalPaneEdge::Right, 120, 0, 20, 40)]
            ),
            Viewport {
                x: 20,
                y: 1,
                cols: 120,
                rows: 38
            }
        );

        // a top strip with a tab bar between it and the tab's panes is not adjacent, so nothing
        // grows and no boundary is drawn over the tab bar
        assert_eq!(
            expand_viewport_over_global_panes(
                tab_viewport,
                &[strip(GlobalPaneEdge::Top, 0, 0, 120, 5)]
            ),
            tab_viewport
        );
    }

    #[test]
    fn mode_updates_reach_each_global_plugin_once_per_client() {
        let (plugin_sender, plugin_receiver) = channels::unbounded();
        let senders = crate::thread_bus::ThreadSenders {
            to_plugin: Some(SenderWithContext::new(plugin_sender)),
            should_silently_fail: true,
            ..Default::default()
        };
        let mut manager =
            GlobalPaneManager::new(senders, Rc::new(RefCell::new(None)), Default::default());
        manager
            .insert(mock_pane(PaneId::Plugin(1)), left_percent(10), true)
            .unwrap();
        manager
            .insert(mock_pane(PaneId::Plugin(2)), left_percent(10), true)
            .unwrap();

        manager
            .update_input_modes(&[(1, ModeInfo::default()), (2, ModeInfo::default())])
            .unwrap();

        let (PluginInstruction::Update(updates), _) = plugin_receiver.recv().unwrap() else {
            panic!("expected plugin mode updates")
        };
        let recipients: Vec<_> = updates
            .iter()
            .map(|(plugin_id, client_id, _event)| (*plugin_id, *client_id))
            .collect();
        assert_eq!(
            recipients,
            vec![
                (Some(1), Some(1)),
                (Some(2), Some(1)),
                (Some(1), Some(2)),
                (Some(2), Some(2)),
            ]
        );
    }

    #[test]
    fn layout_declaration_order_survives_out_of_order_creation() {
        let mut manager = GlobalPaneManager::default();
        manager
            .insert_with_order(
                mock_pane(PaneId::Plugin(2)),
                left_percent(10),
                true,
                Some(1),
            )
            .unwrap();
        manager
            .insert_with_order(
                mock_pane(PaneId::Plugin(1)),
                left_percent(10),
                true,
                Some(0),
            )
            .unwrap();

        assert_eq!(manager.plugin_ids(), vec![1, 2]);
    }

    // ---- property-based tests (hegel) ----

    use hegel::generators::{self, Generator};

    /// A `(PaneId, GlobalPanePlacement)` over every edge and both size flavours, unbounded.
    #[hegel::composite]
    fn any_placement(
        tc: &hegel::TestCase,
        pane_index: u32,
    ) -> (ZellijUtilsPaneId, GlobalPanePlacement) {
        let edge = tc.draw(generators::sampled_from(vec![
            GlobalPaneEdge::Left,
            GlobalPaneEdge::Right,
            GlobalPaneEdge::Top,
            GlobalPaneEdge::Bottom,
        ]));
        let size = if tc.draw(generators::booleans()) {
            PercentOrFixed::Percent(tc.draw(generators::integers::<usize>()))
        } else {
            PercentOrFixed::Fixed(tc.draw(generators::integers::<usize>()))
        };
        let pane_id = if tc.draw(generators::booleans()) {
            ZellijUtilsPaneId::Terminal(pane_index)
        } else {
            ZellijUtilsPaneId::Plugin(pane_index)
        };
        (pane_id, GlobalPanePlacement { edge, size })
    }

    /// Distinct pane ids - `allocate_global_panes` collects into a `BTreeMap`, so duplicate ids
    /// would silently collapse and make "one rect per input" meaningless.
    #[hegel::composite]
    fn any_placements(tc: &hegel::TestCase) -> Vec<(ZellijUtilsPaneId, GlobalPanePlacement)> {
        let count = tc.draw(generators::integers::<usize>().max_value(6));
        (0..count as u32).map(|i| tc.draw(any_placement(i))).collect()
    }

    #[hegel::composite]
    fn any_viewport(tc: &hegel::TestCase) -> Viewport {
        Viewport {
            x: tc.draw(generators::integers::<usize>()),
            y: tc.draw(generators::integers::<usize>()),
            cols: tc.draw(generators::integers::<usize>()),
            rows: tc.draw(generators::integers::<usize>()),
        }
    }

    fn rect_of(geom: &PaneGeom) -> (usize, usize, usize, usize) {
        (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize())
    }

    fn rects_overlap(
        a: (usize, usize, usize, usize),
        b: (usize, usize, usize, usize),
    ) -> bool {
        let (ax, ay, aw, ah) = a;
        let (bx, by, bw, bh) = b;
        ax < bx.saturating_add(bw)
            && bx < ax.saturating_add(aw)
            && ay < by.saturating_add(bh)
            && by < ay.saturating_add(ah)
    }

    fn contains(area: &Viewport, r: (usize, usize, usize, usize)) -> bool {
        let (x, y, w, h) = r;
        x >= area.x
            && y >= area.y
            && x.saturating_add(w) <= area.x.saturating_add(area.cols)
            && y.saturating_add(h) <= area.y.saturating_add(area.rows)
    }

    #[hegel::test]
    fn allocated_global_panes_tile_the_area_without_overlapping(tc: hegel::TestCase) {
        let area = tc.draw(any_viewport());
        let placements = tc.draw(any_placements());
        let clamp_to_screen = tc.draw(generators::booleans());
        let chrome_rows = (
            tc.draw(generators::integers::<usize>()),
            tc.draw(generators::integers::<usize>()),
        );

        let Ok(allocated) =
            allocate_global_panes(area, &placements, clamp_to_screen, chrome_rows)
        else {
            return; // an `Err` is always an acceptable answer
        };

        let rects: Vec<_> = allocated
            .panes
            .iter()
            .map(|(pane_id, geom)| (*pane_id, rect_of(geom)))
            .collect();

        for (pane_id, rect) in &rects {
            assert!(
                contains(&area, *rect),
                "{:?} at {:?} escapes area {:?} (placements {:?}, clamp {}, chrome {:?})",
                pane_id,
                rect,
                area,
                placements,
                clamp_to_screen,
                chrome_rows
            );
        }
        for (i, (id_a, a)) in rects.iter().enumerate() {
            for (id_b, b) in rects.iter().skip(i + 1) {
                assert!(
                    !rects_overlap(*a, *b),
                    "{:?} at {:?} overlaps {:?} at {:?} (area {:?}, placements {:?}, clamp {}, chrome {:?})",
                    id_a,
                    a,
                    id_b,
                    b,
                    area,
                    placements,
                    clamp_to_screen,
                    chrome_rows
                );
            }
        }
        let center = allocated.center;
        let center_rect = (center.x, center.y, center.cols, center.rows);
        for (pane_id, rect) in &rects {
            assert!(
                !rects_overlap(center_rect, *rect),
                "center {:?} overlaps {:?} at {:?} (area {:?}, placements {:?}, clamp {}, chrome {:?})",
                center_rect,
                pane_id,
                rect,
                area,
                placements,
                clamp_to_screen,
                chrome_rows
            );
        }
        assert!(
            contains(&area, center_rect),
            "center {:?} escapes area {:?} (placements {:?}, clamp {}, chrome {:?})",
            center_rect,
            area,
            placements,
            clamp_to_screen,
            chrome_rows
        );
    }
}
