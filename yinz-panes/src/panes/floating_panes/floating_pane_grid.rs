use crate::pane::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};
use crate::{pane::Pane, panes::PaneId};
use std::cmp::Ordering;
use std::collections::HashMap;
use yinz_runtime::errors::prelude::*;
use yinz_types::geometry::{Direction, ResizeStrategy};
use yinz_types::pane_size::{Dimension, PaneGeom, Size, Viewport};

use std::cell::RefCell;
use std::rc::Rc;

const MOVE_INCREMENT_HORIZONTAL: usize = 10;
const MOVE_INCREMENT_VERTICAL: usize = 5;

const MAX_PANES: usize = 100;

// For error reporting
fn no_pane_id(pane_id: &PaneId) -> String {
    format!("no floating pane with ID {:?} found", pane_id)
}

pub struct FloatingPaneGrid<'a> {
    panes: Rc<RefCell<HashMap<PaneId, &'a mut Box<dyn Pane>>>>,
    desired_pane_positions: Rc<RefCell<&'a mut HashMap<PaneId, PaneGeom>>>,
    display_area: Size, // includes all panes (including eg. the status bar and tab bar in the default layout)
    viewport: Viewport, // includes all non-UI panes
}

impl<'a> FloatingPaneGrid<'a> {
    pub fn new(
        panes: impl IntoIterator<Item = (&'a PaneId, &'a mut Box<dyn Pane>)>,
        desired_pane_positions: &'a mut HashMap<PaneId, PaneGeom>,
        display_area: Size,
        viewport: Viewport,
    ) -> Self {
        let panes: HashMap<_, _> = panes.into_iter().map(|(p_id, p)| (*p_id, p)).collect();
        FloatingPaneGrid {
            panes: Rc::new(RefCell::new(panes)),
            desired_pane_positions: Rc::new(RefCell::new(desired_pane_positions)),
            display_area,
            viewport,
        }
    }
    pub fn move_pane_by(&mut self, pane_id: PaneId, x: isize, y: isize) -> Result<()> {
        let err_context = || format!("failed to move pane {pane_id:?} by ({x}, {y})");

        // true => succeeded to move, false => failed to move
        let new_pane_position = {
            let mut panes = self.panes.borrow_mut();
            let pane = panes
                .iter_mut()
                .find(|(p_id, _p)| **p_id == pane_id)
                .with_context(|| no_pane_id(&pane_id))
                .with_context(err_context)?
                .1;
            let moved = geom_moved_within(pane.render_geom(), self.viewport, x, y);
            let mut requested = pane.position_and_size();
            requested.x = moved.x;
            requested.y = moved.y;
            pane.set_render_width(Some((moved.x, moved.cols.as_usize())));
            requested
        };
        self.set_pane_geom(pane_id, new_pane_position)
            .with_context(err_context)
    }

    fn set_pane_geom(&mut self, pane_id: PaneId, new_pane_geom: PaneGeom) -> Result<()> {
        let err_context = || {
            format!(
                "failed to set pane {pane_id:?} geometry to {:?}",
                new_pane_geom
            )
        };

        let mut panes = self.panes.borrow_mut();
        let pane = panes
            .iter_mut()
            .find(|(p_id, _p)| **p_id == pane_id)
            .with_context(|| no_pane_id(&pane_id))
            .with_context(err_context)?
            .1;
        pane.set_geom(new_pane_geom);
        let mut desired_pane_positions = self.desired_pane_positions.borrow_mut();
        desired_pane_positions.insert(pane_id, new_pane_geom);
        Ok(())
    }

    pub fn resize(&mut self, space: Size) -> Result<()> {
        let err_context = || {
            format!(
                "failed to resize from {:?} to {:?}",
                self.display_area, space
            )
        };

        let mut panes = self.panes.borrow_mut();
        let desired_pane_positions = self.desired_pane_positions.borrow();

        // account for the difference between the viewport (including non-ui pane items which we
        // do not want to override) and the display_area, which is the area we can go over
        let display_size_row_difference = self.display_area.rows.saturating_sub(self.viewport.rows);
        let display_size_column_difference =
            self.display_area.cols.saturating_sub(self.viewport.cols);

        let mut new_viewport = self.viewport;
        new_viewport.cols = space.cols.saturating_sub(display_size_column_difference);
        new_viewport.rows = space.rows.saturating_sub(display_size_row_difference);

        for (pane_id, pane) in panes.iter_mut() {
            let mut new_pane_geom = pane.current_geom();
            let desired_pane_geom = desired_pane_positions
                .get(pane_id)
                .with_context(|| {
                    format!(
                        "failed to acquire desired pane geometry for pane {:?}",
                        pane_id
                    )
                })
                .with_context(err_context)?;
            let desired_pane_geom_is_inside_viewport =
                pane_geom_is_inside_viewport(&new_viewport, desired_pane_geom);
            let pane_is_in_desired_position = new_pane_geom == *desired_pane_geom;
            if pane_is_in_desired_position && desired_pane_geom_is_inside_viewport {
                continue;
            } else if desired_pane_geom_is_inside_viewport {
                pane.set_geom(*desired_pane_geom);
            } else {
                let pane_right_side = new_pane_geom.x + new_pane_geom.cols.as_usize();
                let pane_bottom_side = new_pane_geom.y + new_pane_geom.rows.as_usize();
                let viewport_right_side = new_viewport.x + new_viewport.cols;
                let viewport_bottom_side = new_viewport.y + new_viewport.rows;
                let excess_width = pane_right_side.saturating_sub(viewport_right_side);
                let excess_height = pane_bottom_side.saturating_sub(viewport_bottom_side);
                let extra_width = viewport_right_side.saturating_sub(pane_right_side);
                let extra_height = viewport_bottom_side.saturating_sub(pane_bottom_side);

                // handle shrink width
                if excess_width > 0 && new_pane_geom.x.saturating_sub(excess_width) > new_viewport.x
                {
                    new_pane_geom.x = new_pane_geom.x.saturating_sub(excess_width);
                } else if excess_width > 0
                    && new_pane_geom.cols.as_usize().saturating_sub(excess_width)
                        > MIN_TERMINAL_WIDTH
                {
                    new_pane_geom
                        .cols
                        .set_inner(new_pane_geom.cols.as_usize().saturating_sub(excess_width));
                } else if excess_width > 0 {
                    let reduce_x_by = new_pane_geom.x.saturating_sub(new_viewport.x);
                    let reduced_width = new_pane_geom
                        .cols
                        .as_usize()
                        .saturating_sub(excess_width.saturating_sub(reduce_x_by));
                    new_pane_geom.x = new_viewport.x;
                    new_pane_geom
                        .cols
                        .set_inner(std::cmp::max(reduced_width, MIN_TERMINAL_WIDTH));
                }

                // handle shrink height
                if excess_height > 0
                    && new_pane_geom.y.saturating_sub(excess_height) > new_viewport.y
                {
                    new_pane_geom.y = new_pane_geom.y.saturating_sub(excess_height);
                } else if excess_height > 0
                    && new_pane_geom.rows.as_usize().saturating_sub(excess_height)
                        > MIN_TERMINAL_HEIGHT
                {
                    new_pane_geom
                        .rows
                        .set_inner(new_pane_geom.rows.as_usize().saturating_sub(excess_height));
                } else if excess_height > 0 {
                    let reduce_y_by = new_pane_geom.y.saturating_sub(new_viewport.y);
                    let reduced_height = new_pane_geom
                        .rows
                        .as_usize()
                        .saturating_sub(excess_height.saturating_sub(reduce_y_by));
                    new_pane_geom.y = new_viewport.y;
                    new_pane_geom
                        .rows
                        .set_inner(std::cmp::max(reduced_height, MIN_TERMINAL_HEIGHT));
                }

                // handle expand width
                if extra_width > 0 {
                    let max_right_coords = new_viewport.x + new_viewport.cols;
                    if new_pane_geom.x < desired_pane_geom.x {
                        if desired_pane_geom.x + new_pane_geom.cols.as_usize() <= max_right_coords {
                            new_pane_geom.x = desired_pane_geom.x
                        } else if new_pane_geom.x + new_pane_geom.cols.as_usize() + extra_width
                            < max_right_coords
                        {
                            new_pane_geom.x += extra_width;
                        } else {
                            new_pane_geom.x =
                                max_right_coords.saturating_sub(new_pane_geom.cols.as_usize());
                        }
                    }
                    if new_pane_geom.cols.as_usize() < desired_pane_geom.cols.as_usize() {
                        if new_pane_geom.x + desired_pane_geom.cols.as_usize() <= max_right_coords {
                            new_pane_geom
                                .cols
                                .set_inner(desired_pane_geom.cols.as_usize());
                        } else if new_pane_geom.x + new_pane_geom.cols.as_usize() + extra_width
                            < max_right_coords
                        {
                            new_pane_geom
                                .cols
                                .set_inner(new_pane_geom.cols.as_usize() + extra_width);
                        } else {
                            new_pane_geom.cols.set_inner(
                                new_pane_geom.cols.as_usize()
                                    + max_right_coords.saturating_sub(
                                        new_pane_geom.x + new_pane_geom.cols.as_usize(),
                                    ),
                            );
                        }
                    }
                }

                // handle expand height
                if extra_height > 0 {
                    let max_bottom_coords = new_viewport.y + new_viewport.rows;
                    if new_pane_geom.y < desired_pane_geom.y {
                        if desired_pane_geom.y + new_pane_geom.rows.as_usize() <= max_bottom_coords
                        {
                            new_pane_geom.y = desired_pane_geom.y
                        } else if new_pane_geom.y + new_pane_geom.rows.as_usize() + extra_height
                            < max_bottom_coords
                        {
                            new_pane_geom.y += extra_height;
                        } else {
                            new_pane_geom.y =
                                max_bottom_coords.saturating_sub(new_pane_geom.rows.as_usize());
                        }
                    }
                    if new_pane_geom.rows.as_usize() < desired_pane_geom.rows.as_usize() {
                        if new_pane_geom.y + desired_pane_geom.rows.as_usize() <= max_bottom_coords
                        {
                            new_pane_geom
                                .rows
                                .set_inner(desired_pane_geom.rows.as_usize());
                        } else if new_pane_geom.y + new_pane_geom.rows.as_usize() + extra_height
                            < max_bottom_coords
                        {
                            new_pane_geom
                                .rows
                                .set_inner(new_pane_geom.rows.as_usize() + extra_height);
                        } else {
                            new_pane_geom.rows.set_inner(
                                new_pane_geom.rows.as_usize()
                                    + max_bottom_coords.saturating_sub(
                                        new_pane_geom.y + new_pane_geom.rows.as_usize(),
                                    ),
                            );
                        }
                    }
                }
                pane.set_geom(new_pane_geom);
            }
        }
        Ok(())
    }

    pub fn move_pane_left(&mut self, pane_id: &PaneId) -> Result<()> {
        self.move_pane_by(*pane_id, -(MOVE_INCREMENT_HORIZONTAL as isize), 0)
    }

    pub fn move_pane_right(&mut self, pane_id: &PaneId) -> Result<()> {
        self.move_pane_by(*pane_id, MOVE_INCREMENT_HORIZONTAL as isize, 0)
    }

    pub fn move_pane_down(&mut self, pane_id: &PaneId) -> Result<()> {
        self.move_pane_by(*pane_id, 0, MOVE_INCREMENT_VERTICAL as isize)
    }

    pub fn move_pane_up(&mut self, pane_id: &PaneId) -> Result<()> {
        self.move_pane_by(*pane_id, 0, -(MOVE_INCREMENT_VERTICAL as isize))
    }

    pub fn change_pane_size(
        &mut self,
        pane_id: &PaneId,
        strategy: &ResizeStrategy,
        change_by: (usize, usize), // (x, y)
    ) -> Result<()> {
        let err_context = || format!("failed to {strategy} for pane {pane_id:?}");

        let geometry = self
            .panes
            .borrow()
            .get(pane_id)
            .with_context(|| no_pane_id(pane_id))
            .with_context(err_context)?
            .position_and_size();

        let geometry = geom_resized_within(geometry, self.viewport, strategy, change_by);

        self.set_pane_geom(*pane_id, geometry)
            .with_context(err_context)
    }

    pub fn next_selectable_pane_id_to_the_left(&self, current_pane_id: &PaneId) -> Option<PaneId> {
        let panes = self.panes.borrow();
        let current_pane = panes.get(current_pane_id)?.render_geom();
        let panes: Vec<(PaneId, PaneGeom)> = panes
            .iter()
            .filter(|(_, p)| p.selectable())
            .map(|(p_id, p)| (*p_id, p.render_geom()))
            .collect();

        panes
            .iter()
            .enumerate()
            .filter(|(_, (_, c))| {
                c.x < current_pane.x
                    && c.y < current_pane.y + current_pane.rows.as_usize()
                    && current_pane.y < c.y + c.rows.as_usize()
            })
            .max_by(|(_, (_, a)), (_, (_, b))| {
                let x_comparison = a.x.cmp(&b.x);
                match x_comparison {
                    Ordering::Equal => a.y.cmp(&b.y),
                    _ => x_comparison,
                }
            })
            .map(|(_, (pid, _))| pid)
            .copied()
    }
    pub fn pane_id_on_edge(&self, direction: Direction) -> Option<PaneId> {
        let panes = self.panes.borrow();
        let panes: Vec<(PaneId, PaneGeom)> = panes
            .iter()
            .filter(|(_, p)| p.selectable())
            .map(|(p_id, p)| (*p_id, p.render_geom()))
            .collect();

        panes
            .iter()
            .enumerate()
            .max_by(|(_, (_, a)), (_, (_, b))| match direction {
                Direction::Left => {
                    let x_comparison = a.x.cmp(&b.x);
                    match x_comparison {
                        Ordering::Equal => a.y.cmp(&b.y),
                        _ => x_comparison,
                    }
                },
                Direction::Right => {
                    let x_comparison = b.x.cmp(&a.x);
                    match x_comparison {
                        Ordering::Equal => a.y.cmp(&b.y),
                        _ => x_comparison,
                    }
                },
                Direction::Up => {
                    let y_comparison = a.y.cmp(&b.y);
                    match y_comparison {
                        Ordering::Equal => a.x.cmp(&b.x),
                        _ => y_comparison,
                    }
                },
                Direction::Down => {
                    let y_comparison = b.y.cmp(&a.y);
                    match y_comparison {
                        Ordering::Equal => b.x.cmp(&a.x),
                        _ => y_comparison,
                    }
                },
            })
            .map(|(_, (pid, _))| pid)
            .copied()
    }
    pub fn next_selectable_pane_id_below(&self, current_pane_id: &PaneId) -> Option<PaneId> {
        let panes = self.panes.borrow();
        let current_pane = panes.get(current_pane_id)?.render_geom();
        let panes: Vec<(PaneId, PaneGeom)> = panes
            .iter()
            .filter(|(_, p)| p.selectable())
            .map(|(p_id, p)| (*p_id, p.render_geom()))
            .collect();

        panes
            .iter()
            .enumerate()
            .filter(|(_, (_, c))| {
                c.y > current_pane.y
                    && c.x < current_pane.x + current_pane.cols.as_usize()
                    && current_pane.x < c.x + c.cols.as_usize()
            })
            .min_by(|(_, (_, a)), (_, (_, b))| {
                let y_comparison = a.y.cmp(&b.y);
                match y_comparison {
                    Ordering::Equal => b.x.cmp(&a.x),
                    _ => y_comparison,
                }
            })
            .map(|(_, (pid, _))| pid)
            .copied()
    }
    pub fn next_selectable_pane_id_above(&self, current_pane_id: &PaneId) -> Option<PaneId> {
        let panes = self.panes.borrow();
        let current_pane = panes.get(current_pane_id)?.render_geom();
        let panes: Vec<(PaneId, PaneGeom)> = panes
            .iter()
            .filter(|(_, p)| p.selectable())
            .map(|(p_id, p)| (*p_id, p.render_geom()))
            .collect();

        panes
            .iter()
            .enumerate()
            .filter(|(_, (_, c))| {
                c.y < current_pane.y
                    && c.x < current_pane.x + current_pane.cols.as_usize()
                    && current_pane.x < c.x + c.cols.as_usize()
            })
            .max_by(|(_, (_, a)), (_, (_, b))| {
                let y_comparison = a.y.cmp(&b.y);
                match y_comparison {
                    Ordering::Equal => b.x.cmp(&a.x),
                    _ => y_comparison,
                }
            })
            .map(|(_, (pid, _))| pid)
            .copied()
    }
    pub fn next_selectable_pane_id_to_the_right(&self, current_pane_id: &PaneId) -> Option<PaneId> {
        let panes = self.panes.borrow();
        let current_pane = panes.get(current_pane_id)?.render_geom();
        let panes: Vec<(PaneId, PaneGeom)> = panes
            .iter()
            .filter(|(_, p)| p.selectable())
            .map(|(p_id, p)| (*p_id, p.render_geom()))
            .collect();

        panes
            .iter()
            .enumerate()
            .filter(|(_, (_, c))| {
                c.x > current_pane.x
                    && c.y < current_pane.y + current_pane.rows.as_usize()
                    && current_pane.y < c.y + c.rows.as_usize()
            })
            .min_by(|(_, (_, a)), (_, (_, b))| {
                let x_comparison = a.x.cmp(&b.x);
                match x_comparison {
                    Ordering::Equal => a.y.cmp(&b.y),
                    _ => x_comparison,
                }
            })
            .map(|(_, (pid, _))| pid)
            .copied()
    }
    pub fn next_selectable_pane_id(&self, current_pane_id: &PaneId) -> Option<PaneId> {
        let panes = self.panes.borrow();
        let mut panes: Vec<(PaneId, &&mut Box<dyn Pane>)> = panes
            .iter()
            .filter(|(_, p)| p.selectable())
            .map(|(p_id, p)| (*p_id, p))
            .collect();
        panes.sort_by(|(_a_id, a_pane), (_b_id, b_pane)| {
            if a_pane.y() == b_pane.y() {
                a_pane.x().cmp(&b_pane.x())
            } else {
                a_pane.y().cmp(&b_pane.y())
            }
        });
        let active_pane_position = panes.iter().position(|(id, _)| id == current_pane_id)?;

        let next_active_pane_id = panes
            .get(active_pane_position + 1)
            .or_else(|| panes.first())
            .map(|p| p.0)?;
        Some(next_active_pane_id)
    }
    pub fn previous_selectable_pane_id(&self, current_pane_id: &PaneId) -> Option<PaneId> {
        let panes = self.panes.borrow();
        let mut panes: Vec<(PaneId, &&mut Box<dyn Pane>)> = panes
            .iter()
            .filter(|(_, p)| p.selectable())
            .map(|(p_id, p)| (*p_id, p))
            .collect();
        panes.sort_by(|(_a_id, a_pane), (_b_id, b_pane)| {
            if a_pane.y() == b_pane.y() {
                a_pane.x().cmp(&b_pane.x())
            } else {
                a_pane.y().cmp(&b_pane.y())
            }
        });
        let active_pane_position = panes.iter().position(|(id, _)| id == current_pane_id)?;

        let last_pane = panes.last()?;
        let previous_active_pane_id = if active_pane_position == 0 {
            last_pane.0
        } else {
            panes.get(active_pane_position - 1)?.0
        };
        Some(previous_active_pane_id)
    }
    pub fn find_room_for_new_pane(&self) -> Option<PaneGeom> {
        let panes = self.panes.borrow();
        let pane_geoms: Vec<PaneGeom> = panes.values().map(|p| p.position_and_size()).collect();
        Self::find_room_in_viewport(self.viewport, &pane_geoms)
    }

    pub(super) fn find_room_in_viewport(
        viewport: Viewport,
        pane_geoms: &[PaneGeom],
    ) -> Option<PaneGeom> {
        if viewport.cols < MIN_TERMINAL_WIDTH || viewport.rows < MIN_TERMINAL_HEIGHT {
            return None;
        }
        macro_rules! find_unoccupied_offset {
            ($get_geom_with_offset:expr_2021, $viewport:expr_2021, $other_geoms:expr_2021) => {
                let mut offset = 0;
                loop {
                    let geom_with_current_offset = $get_geom_with_offset(offset);
                    if pane_geom_is_big_enough(&geom_with_current_offset)
                        && pane_geom_is_unoccupied_and_inside_viewport(
                            $viewport,
                            &geom_with_current_offset,
                            $other_geoms,
                        )
                    {
                        return Some(geom_with_current_offset);
                    } else if !pane_geom_is_inside_viewport($viewport, &geom_with_current_offset) {
                        break;
                    } else if offset > MAX_PANES {
                        // this is mostly to kill the loop no matter what
                        break;
                    } else {
                        offset += 2;
                    }
                }
            };
        }
        find_unoccupied_offset!(
            |offset| half_size_middle_geom(&viewport, offset),
            &viewport,
            pane_geoms
        );
        find_unoccupied_offset!(
            |offset| half_size_top_left_geom(&viewport, offset),
            &viewport,
            pane_geoms
        );
        find_unoccupied_offset!(
            |offset| half_size_top_right_geom(&viewport, offset),
            &viewport,
            pane_geoms
        );
        find_unoccupied_offset!(
            |offset| half_size_bottom_left_geom(&viewport, offset),
            &viewport,
            pane_geoms
        );
        find_unoccupied_offset!(
            |offset| half_size_bottom_right_geom(&viewport, offset),
            &viewport,
            pane_geoms
        );
        None
    }
}

/// `geom` shifted by `(x, y)` cells and clamped so it stays wholly inside `viewport`.
///
/// The clamp, rather than the shift, is the reason this is shared: it is what stops a dragged pane
/// from being carried off the edge of the surface it is drawn on. `FloatingPanes` and
/// `OverlayPanes` drag against different surfaces - the tab's placement space and the whole
/// session surface respectively - but the bounds arithmetic is the same one, so there is one of it.
pub fn geom_moved_within(geom: PaneGeom, viewport: Viewport, x: isize, y: isize) -> PaneGeom {
    let mut moved = geom;
    let min_x = viewport.x as isize;
    let min_y = viewport.y as isize;
    let max_x = (viewport.cols + viewport.x).saturating_sub(moved.cols.as_usize());
    let max_y = (viewport.rows + viewport.y).saturating_sub(moved.rows.as_usize());
    let new_x = std::cmp::max(min_x, moved.x as isize + x);
    let new_x = std::cmp::min(new_x, max_x as isize);
    let new_y = std::cmp::max(min_y, moved.y as isize + y);
    let new_y = std::cmp::min(new_y, max_y as isize);
    moved.x = new_x as usize;
    moved.y = new_y as usize;
    moved
}

/// `geom` with the borders `strategy` names moved by `change_by`, clamped so it stays inside
/// `viewport` and never shrinks below `MIN_TERMINAL_WIDTH` x `MIN_TERMINAL_HEIGHT`.
///
/// Shared with `OverlayPanes` for the same reason `geom_moved_within` is: the two collections
/// resize against different surfaces - the tab's placement space and the whole session surface -
/// but the border arithmetic, the floors and the "already on the boundary, so invert" rule are
/// one piece of logic, and there is one of it.
///
/// The inversion only happens when the strategy asks for it: a keyboard resize (`ResizeStrategy`'s
/// default) grows from the far border once the near one is flush with the boundary, while a mouse
/// drag sets `invert_on_boundaries: false` and stays clamped under the pointer instead of jumping
/// to the opposite edge. This is the same condition `TiledPaneGrid::change_pane_size` applies.
///
/// The inversion recurses on the *original* geom rather than on the partially adjusted one,
/// which is what `FloatingPaneGrid::change_pane_size` did when it re-read the pane's geometry.
pub fn geom_resized_within(
    geom: PaneGeom,
    viewport: Viewport,
    strategy: &ResizeStrategy,
    change_by: (usize, usize), // (x, y)
) -> PaneGeom {
    let mut geometry = geom;

    let change_by = if strategy.direction.is_none() {
        (change_by.0 / 2, change_by.1 / 2)
    } else {
        change_by
    };

    // Move left border
    if strategy.move_left_border_left() || strategy.move_all_borders_out() {
        let increment = std::cmp::min(geometry.x.saturating_sub(viewport.x), change_by.0);
        // Invert if on boundary already, when the strategy allows it
        if increment == 0 && strategy.direction.is_some() && strategy.invert_on_boundaries {
            return geom_resized_within(geom, viewport, &strategy.invert(), change_by);
        }

        geometry.x -= increment;
        geometry
            .cols
            .set_inner(geometry.cols.as_usize() + increment);
    } else if strategy.move_left_border_right() || strategy.move_all_borders_in() {
        let increment = std::cmp::min(
            geometry.cols.as_usize().saturating_sub(MIN_TERMINAL_WIDTH),
            change_by.0,
        );
        geometry.x += increment;
        geometry
            .cols
            .set_inner(geometry.cols.as_usize().saturating_sub(increment));
    };

    // Move right border
    if strategy.move_right_border_right() || strategy.move_all_borders_out() {
        let increment = std::cmp::min(
            (viewport.x + viewport.cols).saturating_sub(geometry.x + geometry.cols.as_usize()),
            change_by.0,
        );
        // Invert if on boundary already, when the strategy allows it
        if increment == 0 && strategy.direction.is_some() && strategy.invert_on_boundaries {
            return geom_resized_within(geom, viewport, &strategy.invert(), change_by);
        }

        geometry
            .cols
            .set_inner(geometry.cols.as_usize() + increment);
    } else if strategy.move_right_border_left() || strategy.move_all_borders_in() {
        let increment = std::cmp::min(
            geometry.cols.as_usize().saturating_sub(MIN_TERMINAL_WIDTH),
            change_by.0,
        );
        geometry
            .cols
            .set_inner(geometry.cols.as_usize().saturating_sub(increment));
    };

    // Move upper border
    if strategy.move_upper_border_up() || strategy.move_all_borders_out() {
        let increment = std::cmp::min(geometry.y.saturating_sub(viewport.y), change_by.1);
        // Invert if on boundary already, when the strategy allows it
        if increment == 0 && strategy.direction.is_some() && strategy.invert_on_boundaries {
            return geom_resized_within(geom, viewport, &strategy.invert(), change_by);
        }

        geometry.y -= increment;
        geometry
            .rows
            .set_inner(geometry.rows.as_usize() + increment);
    } else if strategy.move_upper_border_down() || strategy.move_all_borders_in() {
        let increment = std::cmp::min(
            geometry.rows.as_usize().saturating_sub(MIN_TERMINAL_HEIGHT),
            change_by.1,
        );
        geometry.y += increment;
        geometry
            .rows
            .set_inner(geometry.rows.as_usize().saturating_sub(increment));
    }

    // Move lower border
    if strategy.move_lower_border_down() || strategy.move_all_borders_out() {
        let increment = std::cmp::min(
            (viewport.y + viewport.rows).saturating_sub(geometry.y + geometry.rows.as_usize()),
            change_by.1,
        );
        // Invert if on boundary already, when the strategy allows it
        if increment == 0 && strategy.direction.is_some() && strategy.invert_on_boundaries {
            return geom_resized_within(geom, viewport, &strategy.invert(), change_by);
        }

        geometry
            .rows
            .set_inner(geometry.rows.as_usize() + increment);
    } else if strategy.move_lower_border_up() || strategy.move_all_borders_in() {
        let increment = std::cmp::min(
            geometry.rows.as_usize().saturating_sub(MIN_TERMINAL_HEIGHT),
            change_by.1,
        );
        geometry
            .rows
            .set_inner(geometry.rows.as_usize().saturating_sub(increment));
    }

    geometry
}

pub fn half_size_middle_geom(space: &Viewport, offset: usize) -> PaneGeom {
    let mut geom = PaneGeom {
        x: space.x + (space.cols as f64 / 4.0).round() as usize + offset,
        y: space.y + (space.rows as f64 / 4.0).round() as usize + offset,
        cols: Dimension::fixed(space.cols / 2),
        rows: Dimension::fixed(space.rows / 2),
        stacked: None,
        is_pinned: false,
        logical_position: None,
    };
    geom.cols.set_inner(space.cols / 2);
    geom.rows.set_inner(space.rows / 2);
    geom
}

fn half_size_top_left_geom(space: &Viewport, offset: usize) -> PaneGeom {
    let mut geom = PaneGeom {
        x: space.x + 2 + offset,
        y: space.y + 2 + offset,
        cols: Dimension::fixed(space.cols / 3),
        rows: Dimension::fixed(space.rows / 3),
        stacked: None,
        is_pinned: false,
        logical_position: None,
    };
    geom.cols.set_inner(space.cols / 3);
    geom.rows.set_inner(space.rows / 3);
    geom
}

fn half_size_top_right_geom(space: &Viewport, offset: usize) -> PaneGeom {
    let mut geom = PaneGeom {
        x: (space.x + space.cols)
            .saturating_sub(space.cols / 3)
            .saturating_sub(2)
            .saturating_sub(offset),
        y: space.y + 2 + offset,
        cols: Dimension::fixed(space.cols / 3),
        rows: Dimension::fixed(space.rows / 3),
        stacked: None,
        is_pinned: false,
        logical_position: None,
    };
    geom.cols.set_inner(space.cols / 3);
    geom.rows.set_inner(space.rows / 3);
    geom
}

fn half_size_bottom_left_geom(space: &Viewport, offset: usize) -> PaneGeom {
    let mut geom = PaneGeom {
        x: space.x + 2 + offset,
        y: (space.y + space.rows)
            .saturating_sub(space.rows / 3)
            .saturating_sub(2)
            .saturating_sub(offset),
        cols: Dimension::fixed(space.cols / 3),
        rows: Dimension::fixed(space.rows / 3),
        stacked: None,
        is_pinned: false,
        logical_position: None,
    };
    geom.cols.set_inner(space.cols / 3);
    geom.rows.set_inner(space.rows / 3);
    geom
}

fn half_size_bottom_right_geom(space: &Viewport, offset: usize) -> PaneGeom {
    let mut geom = PaneGeom {
        x: (space.x + space.cols)
            .saturating_sub(space.cols / 3)
            .saturating_sub(2)
            .saturating_sub(offset),
        y: (space.y + space.rows)
            .saturating_sub(space.rows / 3)
            .saturating_sub(2)
            .saturating_sub(offset),
        cols: Dimension::fixed(space.cols / 3),
        rows: Dimension::fixed(space.rows / 3),
        stacked: None,
        is_pinned: false,
        logical_position: None,
    };
    geom.cols.set_inner(space.cols / 3);
    geom.rows.set_inner(space.rows / 3);
    geom
}

fn pane_geom_is_inside_viewport(viewport: &Viewport, geom: &PaneGeom) -> bool {
    geom.y >= viewport.y
        && geom.y + geom.rows.as_usize() <= viewport.y + viewport.rows
        && geom.x >= viewport.x
        && geom.x + geom.cols.as_usize() <= viewport.x + viewport.cols
}

fn pane_geom_is_big_enough(geom: &PaneGeom) -> bool {
    geom.rows.as_usize() >= MIN_TERMINAL_HEIGHT && geom.cols.as_usize() >= MIN_TERMINAL_WIDTH
}

fn pane_geom_is_unoccupied_and_inside_viewport(
    viewport: &Viewport,
    geom: &PaneGeom,
    existing_geoms: &[PaneGeom],
) -> bool {
    pane_geom_is_inside_viewport(viewport, geom) && !existing_geoms.iter().any(|p| p == geom)
}

#[cfg(test)]
#[path = "unit/geometry_tests.rs"]
mod geometry_tests;
