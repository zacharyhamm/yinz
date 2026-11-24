use super::pane_resizer::PaneResizer;
use crate::pane::{PaneGeometry, TiledGridPane};
use crate::panes::PaneId;
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant};
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    geometry::SplitDirection,
    pane_size::{Dimension, PaneGeom},
};

#[derive(Clone, Copy, Debug)]
pub struct PaneSpec {
    pub id: PaneId,
    pub x: usize,
    pub y: usize,
    pub cols: Dimension,
    pub rows: Dimension,
    pub stacked: Option<usize>,
    pub logical_position: Option<usize>,
    pub geom_override: Option<PaneGeom>,
}

impl PaneSpec {
    pub fn geom(&self) -> PaneGeom {
        PaneGeom {
            x: self.x,
            y: self.y,
            rows: self.rows,
            cols: self.cols,
            stacked: self.stacked,
            is_pinned: false,
            logical_position: self.logical_position,
        }
    }
}

pub fn dim_pct(percent: f64, inner: usize) -> Dimension {
    let mut dimension = Dimension::percent(percent);
    dimension.set_inner(inner);
    dimension
}

pub fn dim_fixed(size: usize) -> Dimension {
    Dimension::fixed(size)
}

pub fn spec(id: u32, x: usize, y: usize, cols: Dimension, rows: Dimension) -> PaneSpec {
    PaneSpec {
        id: PaneId::Terminal(crate::TerminalId::new(id)),
        x,
        y,
        cols,
        rows,
        stacked: None,
        logical_position: None,
        geom_override: None,
    }
}

pub fn spec_stacked(
    id: u32,
    x: usize,
    y: usize,
    cols: Dimension,
    rows: Dimension,
    stack_id: usize,
) -> PaneSpec {
    PaneSpec {
        id: PaneId::Terminal(crate::TerminalId::new(id)),
        x,
        y,
        cols,
        rows,
        stacked: Some(stack_id),
        logical_position: None,
        geom_override: None,
    }
}

pub fn with_override(base: PaneSpec, geom_override: PaneGeom) -> PaneSpec {
    PaneSpec {
        geom_override: Some(geom_override),
        ..base
    }
}

pub struct MockPane<N = ()> {
    navigation: N,
    pid: PaneId,
    geom: PaneGeom,
    geom_override: Option<PaneGeom>,
}

pub struct GridState {
    selectable: bool,
    active_at: Instant,
}

impl Default for GridState {
    fn default() -> Self {
        Self {
            selectable: true,
            active_at: Instant::now(),
        }
    }
}
impl MockPane {
    pub fn new(pid: PaneId, geom: PaneGeom) -> Self {
        Self {
            pid,
            geom,
            geom_override: None,
            navigation: (),
        }
    }
}
impl<N: Default> MockPane<N> {
    pub fn from_spec(spec: PaneSpec) -> Self {
        Self {
            navigation: N::default(),
            pid: spec.id,
            geom: spec.geom(),
            geom_override: spec.geom_override,
        }
    }
}
impl MockPane<GridState> {
    pub fn set_selectable(&mut self, selectable: bool) {
        self.navigation.selectable = selectable;
    }
}
impl TiledGridPane for MockPane<GridState> {
    fn selectable(&self) -> bool {
        self.navigation.selectable
    }
    fn borderless(&self) -> bool {
        false
    }
    fn active_at(&self) -> Instant {
        self.navigation.active_at
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PaneSnapshot {
    pub id: PaneId,
    pub geom: PaneGeom,
    pub current: PaneGeom,
    pub geom_override: Option<PaneGeom>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    PaneSizeUnchanged,
    LayoutRejected,
    OtherErr(String),
}

#[derive(Clone, Debug)]
pub struct Run {
    pub outcome: Outcome,
    pub panes: Vec<PaneSnapshot>,
}

pub fn run_layout(specs: &[PaneSpec], direction: SplitDirection, space: usize) -> Run {
    let mut owned: Vec<MockPane> = specs.iter().map(|s| MockPane::from_spec(*s)).collect();

    let mut map: HashMap<PaneId, &mut MockPane> = HashMap::new();
    for (s, b) in specs.iter().zip(owned.iter_mut()) {
        map.insert(s.id, b);
    }
    let map = Rc::new(RefCell::new(map));

    let result = PaneResizer::new(map.clone()).layout(direction, space);

    let outcome = match result {
        Ok(()) => Outcome::Ok,
        Err(e) => match e.downcast_ref::<YinzError>() {
            Some(YinzError::PaneSizeUnchanged) => Outcome::PaneSizeUnchanged,
            Some(YinzError::PaneLayoutRejected) => Outcome::LayoutRejected,
            _ => Outcome::OtherErr(format!("{:#}", e)),
        },
    };

    let mut panes: Vec<PaneSnapshot> = map
        .borrow()
        .iter()
        .map(|(id, p)| PaneSnapshot {
            id: *id,
            geom: p.position_and_size(),
            current: p.current_geom(),
            geom_override: p.geom_override(),
        })
        .collect();
    panes.sort_by_key(|s| s.id);

    Run { outcome, panes }
}

impl<N> PaneGeometry for MockPane<N> {
    fn pid(&self) -> PaneId {
        self.pid
    }
    fn position_and_size(&self) -> PaneGeom {
        self.geom
    }
    fn geom_override(&self) -> Option<PaneGeom> {
        self.geom_override
    }
    fn set_geom(&mut self, geom: PaneGeom) {
        self.geom = geom;
    }
    fn set_geom_override(&mut self, geom: PaneGeom) {
        self.geom_override = Some(geom);
    }
    fn reset_size_and_position_override(&mut self) {
        self.geom_override = None;
    }
    fn reduce_height(&mut self, percent: f64) {
        if let Some(current) = self.geom.rows.as_percent() {
            self.geom.rows.set_percent(current - percent);
        }
    }
    fn increase_height(&mut self, percent: f64) {
        if let Some(current) = self.geom.rows.as_percent() {
            self.geom.rows.set_percent(current + percent);
        }
    }
    fn reduce_width(&mut self, percent: f64) {
        if let Some(current) = self.geom.cols.as_percent() {
            self.geom.cols.set_percent(current - percent);
        }
    }
    fn increase_width(&mut self, percent: f64) {
        if let Some(current) = self.geom.cols.as_percent() {
            self.geom.cols.set_percent(current + percent);
        }
    }
    fn pull_up(&mut self, count: usize) {
        self.geom.y -= count;
    }
    fn push_down(&mut self, count: usize) {
        self.geom.y += count;
    }
    fn pull_left(&mut self, count: usize) {
        self.geom.x -= count;
    }
    fn push_right(&mut self, count: usize) {
        self.geom.x += count;
    }
}
