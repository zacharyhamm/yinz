//! Hocus owns one overlay placement; the host owns its two child slots and divider.

use std::collections::BTreeMap;
use zellij_tile::prelude::TabPosition;

use zellij_tile::prelude::*;

use hocus_proto::launch;

use crate::message::Message;
use crate::ui;
use crate::State;

const DEFAULT_PAD: usize = 3;
const RING_FOCUS_PENDING_FRAMES: u8 = 4;
const DEFAULT_WIDTH_PERCENT: usize = 60;
const DEFAULT_HEIGHT_PERCENT: usize = 66;
const DEFAULT_SPLIT_PERCENT: usize = 50;
const MIN_PANE_COLS: usize = ui::MIN_COLS + 2;
const MIN_PANE_ROWS: usize = ui::MIN_ROWS + 2;
/// The fewest columns the host draws either half in, while the panel has room for two. The
/// conversation cannot lay itself out in less, and the agent list is no use narrower either.
const MIN_HALF_COLS: u16 = ui::MIN_COLS as u16;

/// One `width`, `height`, `split` or `pad` value as written: a count of cells ("120"), or a
/// share of the whole ("60%", "60 %").
///
/// The one reading of all four keys, so the placement, the config warnings and `/split` cannot
/// disagree about what a value means - or about which values mean nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Measure {
    Cells(usize),
    Percent(usize),
}

impl Measure {
    /// `None` for anything but a whole number, with or without a '%': a decimal, a minus sign,
    /// text, or nothing at all. A leading '+' is read as the number it signs, as
    /// `usize::from_str` reads it.
    fn parse(value: &str) -> Option<Measure> {
        let value = value.trim();
        match value.strip_suffix('%') {
            Some(percent) => percent.trim().parse().ok().map(Measure::Percent),
            None => value.parse().ok().map(Measure::Cells),
        }
    }

    fn of(self, whole: usize) -> usize {
        match self {
            Measure::Cells(cells) => cells,
            Measure::Percent(percent) => whole.saturating_mul(percent) / 100,
        }
    }
}

fn resolve(value: Option<&String>, whole: usize, default_percent: usize) -> usize {
    value
        .and_then(|value| Measure::parse(value))
        .unwrap_or(Measure::Percent(default_percent))
        .of(whole)
}

/// The agent list's share of a panel `panel_cols` wide, in the basis points
/// [`MultipaneSplit::ratio`] takes. `None` is the default even split.
///
/// A count of columns is turned into a share of this width, once: the host keeps a ratio, so a
/// divider set as "40" is a share of whatever the panel is when it is set.
///
/// The ratio is not clamped to [`MIN_HALF_COLS`]: the container carries that floor as its
/// `min_cells`, and the host applies it to what it draws at every width while leaving the ratio
/// alone (`MultipaneSplit::geometry`). So "0" and "100%" are drawn with a half at the floor
/// rather than a single column wide - and a share set while the panel is too narrow for it
/// comes back when the panel grows, where a ratio clamped to this width's floor would have
/// kept the narrow panel's squeezed divider for the rest of the session.
fn split_ratio(split: Option<Measure>, panel_cols: usize) -> u16 {
    // One outer frame either side and the divider between the halves, as the host counts it.
    let available = panel_cols.saturating_sub(3).max(1);
    // The host draws the list `available * ratio / 10_000` columns wide, rounded down, so a
    // count of columns rounds up to land on that count rather than one short of it.
    let ratio = match split.unwrap_or(Measure::Percent(DEFAULT_SPLIT_PERCENT)) {
        Measure::Percent(percent) => percent.saturating_mul(100),
        Measure::Cells(columns) => columns.saturating_mul(10_000).div_ceil(available),
    };
    ratio.clamp(1, 9999) as u16
}

/// The two halves' widths, `(list, conversation)`, for a split of a panel `rows` x `cols`, as the
/// host will draw them.
fn split_widths(ratio: u16, rows: usize, cols: usize) -> (usize, usize) {
    let split = MultipaneSplit {
        direction: MultipaneDirection::LeftRight,
        ratio,
    };
    let outer = zellij_types::pane_size::PaneGeom::from(&Size { rows, cols });
    let ([list, conversation], _) = split.geometry(outer, true, MIN_HALF_COLS);
    (list.cols.as_usize(), conversation.cols.as_usize())
}

/// One line per `width`, `height`, `split` or `pad` value hocus cannot read, naming what it
/// uses instead.
///
/// Only a value that does not parse. A zero, or a size larger than the screen, is read and then
/// clamped as the config comment says, which is an answer rather than a fallback; warning on "0"
/// and not on "10", which is raised just the same, would be arbitrary. An empty value is the key
/// left blank, which means the default and says so.
pub(crate) fn geometry_warnings(config: &BTreeMap<String, String>) -> Vec<String> {
    let keys = [
        (
            "width",
            "a column count like \"120\" or a percentage like \"60%\"",
            format!("{DEFAULT_WIDTH_PERCENT}%"),
        ),
        (
            "height",
            "a row count like \"30\" or a percentage like \"66%\"",
            format!("{DEFAULT_HEIGHT_PERCENT}%"),
        ),
        (
            "split",
            "a column count like \"40\" or a percentage like \"40%\"",
            format!("{DEFAULT_SPLIT_PERCENT}%"),
        ),
        (
            "pad",
            "a number of cells like \"3\"",
            DEFAULT_PAD.to_string(),
        ),
    ];
    keys.into_iter()
        .filter_map(|(key, shape, fallback)| {
            let value = config.get(key)?;
            let readable = match Measure::parse(value) {
                // The one key of the four that is never a share of anything.
                Some(Measure::Percent(_)) => key != "pad",
                Some(Measure::Cells(_)) => true,
                None => value.trim().is_empty(),
            };
            (!readable).then(|| format!("{key} {value:?} is not {shape}; using {fallback}"))
        })
        .collect()
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
        .and_then(|value| match Measure::parse(value) {
            Some(Measure::Cells(cells)) => Some(cells),
            _ => None,
        })
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
    fn panel_child_plugin(&self) -> Option<PluginId> {
        self.panel_tabs
            .as_ref()
            .and_then(|tabs| match tabs.pane_id {
                PaneId::Plugin(id) => Some(id),
                _ => None,
            })
            .or(self.agents_plugin)
    }

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

    /// Say so, once, when the config holds a panel size hocus cannot read.
    ///
    /// Each unreadable value falls back to its default, which is the right thing for a typo -
    /// the panel still opens - but without this the user sees only that their size did not
    /// take, not why. Once per distinct list: the host repeats `PluginConfigurationChanged` for
    /// every save once the config has changed at all, and a notice per save would bury the
    /// conversation. A fixed value followed by a fresh mistake is a new list, and is reported.
    pub(crate) fn warn_bad_geometry(&mut self) {
        let lines = geometry_warnings(&self.kdl);
        if lines == self.geometry_warned {
            return;
        }
        self.geometry_warned = lines.clone();
        if lines.is_empty() {
            return;
        }
        let notice = format!(
            "hocus cannot read a panel size in its plugin configuration:\n{}",
            lines.join("\n")
        );
        self.messages.push(Message::notice(notice));
        self.log_global("config_warning", serde_json::json!({ "geometry": lines }));
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
            let agents = if let Some(agents) = self.panel_child_plugin() {
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
            let ratio = split_ratio(self.configured_split(), panel.cols);
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
                // Neither half is drawn narrower than the conversation can lay itself out in,
                // however far the configured split or a divider drag pushes it.
                min_cells: MIN_HALF_COLS,
            })
        })
    }

    /// The `split` the config asks for, `None` when it is unset or unreadable - see
    /// [`geometry_warnings`] for the second.
    fn configured_split(&self) -> Option<Measure> {
        self.kdl
            .get("split")
            .and_then(|value| Measure::parse(value))
    }

    /// Move the divider to `ratio`, and nothing else.
    ///
    /// Its own request rather than a `placement_command` with a split in it: every placement -
    /// a resize, Ctrl-o, a reveal - sends `split: None`, which is what keeps a divider the user
    /// dragged where they put it. Only the two things that *are* the user choosing a split come
    /// through here: `/split`, and an edit to `split` in the config.
    fn send_split(&mut self, ratio: u16) {
        let Some(id) = self.multipane.as_ref().map(|info| info.id) else {
            return;
        };
        let split = MultipaneSplit {
            direction: MultipaneDirection::LeftRight,
            ratio,
        };
        #[cfg(test)]
        {
            self.last_split = Some(split);
        }
        #[cfg(target_family = "wasm")]
        let result = update_multipane(UpdateMultipane {
            id,
            coordinates: None,
            split: Some(split),
            override_user_placement: false,
        });
        #[cfg(not(target_family = "wasm"))]
        let result: Result<Option<MultipaneInfo>, String> = {
            let _ = (id, split);
            Err("multipanes require the zellij host".into())
        };
        if let Ok(Some(info)) = result {
            self.multipane = Some(info);
            self.panel.placed_rect = self.reported_panel();
        }
    }

    /// A live edit to `split` in the config: the divider moves to it, as `width`, `height` and
    /// `pad` already move the panel. Called only when the value itself changed, since the host
    /// repeats `PluginConfigurationChanged` for edits to anything in the file - resending the
    /// configured split on each of those would undo every drag.
    pub(crate) fn apply_split_edit(&mut self) {
        if !self.is_panel() {
            return;
        }
        let Some(columns) = self.multipane.as_ref().map(|info| info.columns) else {
            return;
        };
        self.send_split(split_ratio(self.configured_split(), columns));
    }

    /// `/split <columns | percent>`: set the divider from the keyboard, the one way to move it
    /// without a mouse. A bare `/split` changes nothing and reports the widths.
    ///
    /// Answers the `(list, conversation)` widths the host will draw, floor and all, so the
    /// notice says what the user will see rather than what they typed.
    pub(crate) fn set_split(&mut self, value: &str) -> Result<(usize, usize), String> {
        if !self.is_panel() {
            return Err("/split sets the panel's divider, and this hocus is not the panel".into());
        }
        let Some((rows, cols, ratio)) = self
            .multipane
            .as_ref()
            .map(|info| (info.rows, info.columns, info.split.ratio))
        else {
            return Err("the panel is not placed yet - try again in a moment".into());
        };
        let value = value.trim();
        if value.is_empty() {
            return Ok(split_widths(ratio, rows, cols));
        }
        let Some(split) = Measure::parse(value) else {
            return Err(format!(
                "/split {value}: give the agent list's width as a column count like /split 40 \
                 or a percentage like /split 35%"
            ));
        };
        let ratio = split_ratio(Some(split), cols);
        self.send_split(ratio);
        Ok(split_widths(ratio, rows, cols))
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
            self.agents_plugin = self
                .panel_tabs
                .as_ref()
                .and(self.agents_plugin)
                .or_else(|| {
                    info.children.iter().find_map(|pane| match pane {
                        PaneId::Plugin(id) if *id != self.own_plugin_id => Some(*id),
                        _ => None,
                    })
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

    /// The pane the host has this client's keyboard on.
    ///
    /// Asked, rather than read off the manifest, because the manifest cannot say: every layer
    /// reports `is_focused` for its own active pane, so the tiled pane under the panel reads as
    /// focused whether or not an overlay above it has the keys. Asked only while a leave is
    /// armed (see `on_panes`), as file-finder asks on its `PaneUpdate`: on the edge where both
    /// halves lose the keyboard, and on the arrival of a panel that followed a tab switch - and
    /// then on every manifest after either for as long as the leave stays armed, which is while
    /// a floating pane or an overlay on the panel's tab holds the keys, or the pane that held
    /// them when the panel followed still does (`State::followed_holder`). Each is a host round
    /// trip, so a holder that retitles itself often costs one per retitle.
    /// A host call off wasm would read the test runner's stdin, so tests name the holder.
    #[cfg(target_family = "wasm")]
    pub(crate) fn keyboard_holder(&self) -> Option<PaneId> {
        get_focused_pane_info().ok().map(|(_, pane)| pane)
    }
    #[cfg(all(test, not(target_family = "wasm")))]
    pub(crate) fn keyboard_holder(&self) -> Option<PaneId> {
        self.keyboard_holder_calls
            .set(self.keyboard_holder_calls.get() + 1);
        self.keyboard_holder_for_test
    }
    #[cfg(not(any(test, target_family = "wasm")))]
    pub(crate) fn keyboard_holder(&self) -> Option<PaneId> {
        None
    }

    fn note_ring_focus(&mut self, pane: PaneId, stop: &'static str) {
        #[cfg(test)]
        self.ring_focus_calls.push((pane, stop));
        #[cfg(not(test))]
        let _ = (pane, stop);
    }

    fn hand_keyboard_to_agents(&mut self, stop: &'static str) -> bool {
        let Some(agents_id) = self.panel_child_plugin().filter(|_| self.is_panel()) else {
            return false;
        };
        let pane = PaneId::Plugin(agents_id);
        self.reset_ring_stop();
        if let Some(tabs) = &self.panel_tabs {
            let _ = focus_pane_tabs(tabs.id, stop == hocus_proto::wire::STOP_TABS);
        } else {
            focus_pane_with_id(pane, false, false);
        }
        pipe_message_to_plugin(
            MessageToPlugin::new(hocus_proto::wire::AGENT_STRIP_TAKE)
                .with_destination_plugin_id(agents_id)
                .with_args(BTreeMap::from([(
                    hocus_proto::wire::PANEL_STOP.to_owned(),
                    stop.to_owned(),
                )])),
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

    /// A job's `show`. A panel the user put away comes back with the keyboard in it, as
    /// `show_panel` brings it. A panel that is already up is left alone: it follows the user's
    /// tab while visible, so it is already in front of them, and the keyboard may be mid-entry
    /// in the list half or in a terminal. Taking it would carry the rest of what they were
    /// typing into the chat box, where Enter queues it as a message to the model. The job
    /// itself needs no keys: a scheduled run never waits on a y/n (see `pump_tools`).
    pub(crate) fn show_panel_for_job(&mut self) {
        if self.is_panel() && !self.hidden {
            return;
        }
        self.show_panel();
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
