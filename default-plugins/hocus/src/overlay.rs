//! Hocus owns one overlay placement; the host owns its two child slots and divider.

use std::collections::BTreeMap;
use zellij_tile::prelude::TabPosition;

use zellij_tile::prelude::*;

use hocus_proto::launch;

use crate::ui;
use crate::State;

const DEFAULT_PAD: usize = 3;
const RING_FOCUS_PENDING_FRAMES: u8 = 4;
const DEFAULT_WIDTH_PERCENT: usize = 60;
const DEFAULT_HEIGHT_PERCENT: usize = 66;
const DEFAULT_SPLIT_PERCENT: usize = 50;
const MIN_PANE_COLS: usize = ui::MIN_COLS + 2;
const MIN_PANE_ROWS: usize = ui::MIN_ROWS + 2;

fn resolve(value: Option<&String>, whole: usize, default_percent: usize) -> usize {
    let fallback = whole * default_percent / 100;
    let Some(value) = value.map(|value| value.trim()).filter(|v| !v.is_empty()) else {
        return fallback;
    };
    match value.strip_suffix('%') {
        Some(percent) => match percent.trim().parse::<usize>() {
            Ok(percent) => whole * percent / 100,
            Err(_) => fallback,
        },
        None => value.parse::<usize>().unwrap_or(fallback),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shown {
    pub chat: bool,
    pub agents: bool,
}

impl Shown {
    pub(crate) const BOTH: Shown = Shown {
        chat: true,
        agents: true,
    };

    fn halves(self) -> usize {
        match self.chat != self.agents {
            true => 1,
            false => 2,
        }
    }
}

fn min_panel_cols(cols: usize, halves: usize) -> usize {
    (halves * MIN_PANE_COLS).min(cols.max(1))
}

fn panel_size(
    config: &BTreeMap<String, String>,
    rows: usize,
    cols: usize,
    shown: Shown,
) -> (usize, usize) {
    let halves = shown.halves();
    let pad = pad_of(config, rows, cols, halves);
    let (configured_width, configured_height) = configured_size(config, rows, cols);
    let across = cols.saturating_sub(pad * 2);
    let width = fit(
        configured_width,
        min_panel_cols(cols, halves),
        across,
        cols.max(1),
    );
    let available = rows.saturating_sub(pad * 2);
    let whole = rows.max(1);
    let height = fit(
        configured_height,
        MIN_PANE_ROWS.min(whole),
        available,
        whole,
    );
    (width, height)
}

fn fit(configured: usize, min: usize, room: usize, whole: usize) -> usize {
    configured.min(room).max(min).min(whole)
}

fn pad_of(config: &BTreeMap<String, String>, rows: usize, cols: usize, halves: usize) -> usize {
    config
        .get("pad")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_PAD)
        .min(rows / 4)
        .min(cols / 4)
        .min(rows.saturating_sub(MIN_PANE_ROWS) / 2)
        .min(cols.saturating_sub(min_panel_cols(cols, halves)) / 2)
}

fn configured_size(config: &BTreeMap<String, String>, rows: usize, cols: usize) -> (usize, usize) {
    (
        resolve(config.get("width"), cols, DEFAULT_WIDTH_PERCENT),
        resolve(config.get("height"), rows, DEFAULT_HEIGHT_PERCENT),
    )
}

fn fixed_rect(rect: Rect) -> FloatingPaneCoordinates {
    FloatingPaneCoordinates::new(
        Some(rect.x.to_string()),
        Some(rect.y.to_string()),
        Some(rect.cols.to_string()),
        Some(rect.rows.to_string()),
        None,
        None,
    )
    .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: usize,
    pub y: usize,
    pub cols: usize,
    pub rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Corner {
    BottomLeft,
    TopLeft,
    TopRight,
    BottomRight,
}

const CORNERS: [Corner; 4] = [
    Corner::BottomLeft,
    Corner::TopLeft,
    Corner::TopRight,
    Corner::BottomRight,
];

fn next_clockwise(corner: Corner) -> Corner {
    let index = CORNERS
        .iter()
        .position(|candidate| *candidate == corner)
        .unwrap_or(0);
    CORNERS[(index + 1) % CORNERS.len()]
}

fn corner_rect(
    corner: Corner,
    surface_rows: usize,
    surface_cols: usize,
    pad: usize,
    size: (usize, usize),
) -> Rect {
    let (width, height) = size;
    let (left, top) = (pad, pad);
    let right = surface_cols.saturating_sub(pad + width).max(left);
    let bottom = surface_rows.saturating_sub(pad + height).max(top);
    let x = match corner {
        Corner::BottomLeft | Corner::TopLeft => left,
        Corner::BottomRight | Corner::TopRight => right,
    };
    let y = match corner {
        Corner::BottomLeft | Corner::BottomRight => bottom,
        Corner::TopLeft | Corner::TopRight => top,
    };
    Rect {
        x: x.min(surface_cols.saturating_sub(width)),
        y: y.min(surface_rows.saturating_sub(height)),
        cols: width,
        rows: height,
    }
}

fn rect_in_corner(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    shown: Shown,
    corner: Corner,
    size: Option<(usize, usize)>,
) -> Option<Rect> {
    let (pad, size) = placement_terms(config, surface_rows, surface_cols, shown, size)?;
    Some(corner_rect(corner, surface_rows, surface_cols, pad, size))
}

fn placement_terms(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    shown: Shown,
    size: Option<(usize, usize)>,
) -> Option<(usize, (usize, usize))> {
    if surface_rows == 0 || surface_cols == 0 {
        return None;
    }
    let pad = pad_of(config, surface_rows, surface_cols, shown.halves());
    let size = size.unwrap_or_else(|| panel_size(config, surface_rows, surface_cols, shown));
    Some((
        pad,
        (size.0.clamp(1, surface_cols), size.1.clamp(1, surface_rows)),
    ))
}

pub(crate) fn surface_of(tab: &TabInfo) -> (usize, usize) {
    (
        (tab.display_area_rows).get(),
        (tab.display_area_columns).get(),
    )
}

pub(crate) fn tab_to_follow(
    own_tab: Option<TabPosition>,
    client_was_on: Option<TabPosition>,
    client_is_on: TabPosition,
) -> Option<TabPosition> {
    let own = own_tab?;
    (client_was_on == Some(own) && client_is_on != own).then_some(client_is_on)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Panel {
    pub(crate) placed_corner: Option<Corner>,
    pub(crate) placed_rect: Option<Rect>,
}

impl State {
    pub(crate) fn is_panel(&self) -> bool {
        self.own_is_floating == Some(true)
    }

    pub(crate) fn shown(&self) -> Shown {
        Shown::BOTH
    }

    pub(crate) fn apply_overlay_geometry(&mut self) {
        if !self.is_panel() {
            return;
        }
        let Some(panel) = self.panel_rect() else {
            return;
        };
        self.place_multipane(panel, false);
    }
    fn placement_command(
        &self,
        panel: Rect,
        override_user_placement: bool,
    ) -> Option<PluginCommand> {
        if !self.is_panel() || self.own_tab_index.is_none() {
            return None;
        }
        let coordinates = fixed_rect(panel);
        Some(if let Some(info) = &self.multipane {
            PluginCommand::UpdateMultipane(UpdateMultipane {
                id: info.id,
                coordinates: Some(coordinates),
                split: None,
                override_user_placement,
            })
        } else {
            let agents = if let Some(agents) = self.agents_plugin {
                if self.agents_tab_index != self.own_tab_index {
                    return None;
                }
                MultipaneChild::Existing(PaneId::Plugin(agents))
            } else {
                MultipaneChild::Plugin {
                    url: launch::HOCUS_AGENTS_ALIAS.into(),
                    configuration: launch::overlay_configuration(),
                }
            };
            let available = panel.cols.saturating_sub(3).max(1);
            let ratio = self
                .kdl
                .get("split")
                .map(|value| value.trim())
                .and_then(|value| {
                    if let Some(percent) = value.strip_suffix('%') {
                        percent
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .map(|percent| percent.saturating_mul(100))
                    } else {
                        value
                            .parse::<usize>()
                            .ok()
                            .map(|columns| columns.saturating_mul(10_000) / available)
                    }
                })
                .unwrap_or(DEFAULT_SPLIT_PERCENT * 100)
                .clamp(1, 9999) as u16;
            PluginCommand::CreateMultipane(CreateMultipane {
                children: [
                    agents,
                    MultipaneChild::Existing(PaneId::Plugin(self.own_plugin_id)),
                ],
                placement: OverlayPlacement {
                    kind: OverlayKind::Dismissible,
                    coordinates: Some(coordinates),
                },
                split: MultipaneSplit {
                    direction: MultipaneDirection::LeftRight,
                    ratio,
                },
                focus: None,
            })
        })
    }
    fn place_multipane(&mut self, panel: Rect, override_user_placement: bool) {
        let Some(command) = self.placement_command(panel, override_user_placement) else {
            return;
        };
        #[cfg(test)]
        {
            self.last_placement = Some((
                vec![(PaneId::Plugin(self.own_plugin_id), fixed_rect(panel))],
                override_user_placement,
            ));
            self.placed_against = (self.surface_rows, self.surface_cols);
            if matches!(&command, PluginCommand::CreateMultipane(request) if matches!(request.children[0], MultipaneChild::Plugin { .. }))
            {
                self.agents_launches += 1;
            }
        }
        #[cfg(target_family = "wasm")]
        let result = match command {
            PluginCommand::CreateMultipane(request) => create_multipane(request),
            PluginCommand::UpdateMultipane(request) => update_multipane(request),
            _ => unreachable!(),
        };
        #[cfg(not(target_family = "wasm"))]
        let result: Result<Option<MultipaneInfo>, String> = {
            let _ = command;
            Err("multipanes require the zellij host".into())
        };
        if let Ok(Some(info)) = result {
            self.agents_plugin = info.children.iter().find_map(|pane| match pane {
                PaneId::Plugin(id) if *id != self.own_plugin_id => Some(*id),
                _ => None,
            });
            self.agents_tab_index = self.own_tab_index;
            self.multipane = Some(info);
            self.panel.placed_rect = self.reported_panel();
            self.panel.placed_corner.get_or_insert(CORNERS[0]);
            self.placed = true;
        }
    }

    fn panel_rect(&self) -> Option<Rect> {
        self.rect_in(self.panel.placed_corner.unwrap_or(CORNERS[0]), None)
    }

    fn place_panel(&mut self, size: Option<(usize, usize)>, corner: Corner) {
        if !self.is_panel() {
            return;
        }
        let Some(panel) = self.rect_in(corner, size) else {
            return;
        };
        self.panel.placed_corner = Some(corner);
        self.place_multipane(panel, true);
    }

    fn rect_in(&self, corner: Corner, size: Option<(usize, usize)>) -> Option<Rect> {
        rect_in_corner(
            &self.kdl,
            self.surface_rows,
            self.surface_cols,
            self.shown(),
            corner,
            size,
        )
    }

    fn own_rect(&self) -> Rect {
        let reported = self.reported_panel();
        let mut own = self
            .panel
            .placed_rect
            .or(reported)
            .or_else(|| self.panel_rect())
            .unwrap_or(Rect {
                x: 0,
                y: 0,
                cols: 0,
                rows: 0,
            });
        if let Some(reported) = reported {
            own.cols = reported.cols;
            own.rows = reported.rows;
        }
        own
    }

    pub(crate) fn reported_panel(&self) -> Option<Rect> {
        self.multipane.as_ref().map(|info| Rect {
            x: info.x,
            y: info.y,
            cols: info.columns,
            rows: info.rows,
        })
    }

    pub(crate) fn reposition(&mut self) {
        let own = self.own_rect();
        let size = Some((own.cols, own.rows));
        let drawn = self.reported_panel();
        let mut corner = self.panel.placed_corner.unwrap_or(CORNERS[0]);
        let mut target = None;
        for _ in 0..CORNERS.len() {
            corner = next_clockwise(corner);
            let candidate = self.rect_in(corner, size);
            if candidate != Some(own) && candidate != drawn {
                target = Some(corner);
                break;
            }
        }
        if let Some(corner) = target {
            self.place_panel(size, corner);
        }
    }

    pub(crate) fn reposition_from_list(&mut self) {
        let list_is_here = self.agents_plugin.is_some()
            && self.own_tab_index.is_some()
            && self.agents_tab_index == self.own_tab_index;
        if !list_is_here {
            return;
        }
        self.reposition();
    }

    pub(crate) fn hide_panel(&mut self) {
        if !self.is_panel() {
            return;
        }
        hide_self();
        self.note_visibility(PaneId::Plugin(self.own_plugin_id), false);
        self.send_vm_panel_visibility(false);
        self.hidden = true;
        self.reset_ring_stop();
    }

    pub(crate) fn reset_ring_stop(&mut self) {
        self.chat_tab_focus = None;
        self.chat_input_released = false;
    }

    fn note_ring_focus(&mut self, pane: PaneId, stop: &'static str) {
        #[cfg(test)]
        self.ring_focus_calls.push((pane, stop));
        #[cfg(not(test))]
        let _ = (pane, stop);
    }

    fn hand_keyboard_to_agents(&mut self, stop: &'static str) -> bool {
        let Some(agents_id) = self.agents_plugin.filter(|_| self.is_panel()) else {
            return false;
        };
        let pane = PaneId::Plugin(agents_id);
        self.reset_ring_stop();
        focus_pane_with_id(pane, false, false);
        pipe_message_to_plugin(
            MessageToPlugin::new(hocus_proto::wire::AGENT_STRIP_TAKE).with_args(BTreeMap::from([
                (hocus_proto::wire::PANEL_STOP.to_owned(), stop.to_owned()),
            ])),
        );
        self.note_ring_focus(pane, stop);
        true
    }

    pub(crate) fn ring_forward_from_input(&mut self) -> bool {
        if self.hand_keyboard_to_agents(hocus_proto::vms::STOP_TABS) {
            return true;
        }
        self.chat_input_released = false;
        self.chat_tab_focus = Some(self.chat_tab);
        true
    }

    pub(crate) fn ring_back_from_tab_bar(&mut self) -> bool {
        if self.hand_keyboard_to_agents(hocus_proto::wire::STOP_HISTORY) {
            return true;
        }
        self.reset_ring_stop();
        true
    }

    pub(crate) fn take_ring_focus(&mut self, stop: Option<&str>) -> bool {
        if !self.is_panel() {
            return false;
        }
        if self.hidden {
            return false;
        }
        self.chat_input_released = false;
        self.chat_tab_focus = match stop {
            Some(hocus_proto::wire::STOP_TABS) => Some(self.chat_tab),
            _ => None,
        };
        self.ring_focus_pending = RING_FOCUS_PENDING_FRAMES;
        show_self(true);
        true
    }

    pub(crate) fn reveal_panel(&mut self) {
        if !self.is_panel() {
            return;
        }
        self.hidden = false;
        show_pane_with_id(PaneId::Plugin(self.own_plugin_id), false, false);
        self.note_visibility(PaneId::Plugin(self.own_plugin_id), true);
        self.apply_overlay_geometry();
    }

    pub(crate) fn show_panel(&mut self) {
        show_self(true);
        self.note_visibility(PaneId::Plugin(self.own_plugin_id), true);
        self.reveal_panel();
    }

    pub(crate) fn on_toast_click(&mut self, chat_id: Option<u64>) -> bool {
        self.show_panel();
        let Some(chat_id) = chat_id.filter(|chat_id| self.chat_id != Some(*chat_id)) else {
            return true;
        };
        if let Some(index) = self
            .parked
            .iter()
            .position(|chat| chat.chat_id == Some(chat_id))
        {
            self.switch_to_tab(self.tab_of_parked(index));
        }
        true
    }

    pub(crate) fn note_visibility(&mut self, pane: PaneId, shown: bool) {
        if let PaneId::Plugin(id) = pane {
            if self.agents_plugin == Some(id) {
                self.send_vm_panel_visibility(shown);
            }
        }
        #[cfg(test)]
        self.visibility_calls.push((pane, shown));
        #[cfg(not(test))]
        let _ = (pane, shown);
    }

    pub(crate) fn follow_tab(&mut self, tab_index: TabPosition) {
        if !self.is_panel() || self.own_tab_index.is_none() || self.own_tab_index == Some(tab_index)
        {
            return;
        }
        break_panes_to_tab_with_index(&[PaneId::Plugin(self.own_plugin_id)], tab_index, false);
        self.own_tab_index = Some(tab_index);
        if self.multipane.is_some() {
            self.agents_tab_index = Some(tab_index);
        }
    }

    pub(crate) fn greet_agents_pane(&self) {
        pipe_message_to_plugin(MessageToPlugin::new(hocus_proto::wire::AGENT_STRIP_OPEN));
    }
}

#[cfg(test)]
#[path = "unit/overlay_tests.rs"]
mod overlay_tests;
