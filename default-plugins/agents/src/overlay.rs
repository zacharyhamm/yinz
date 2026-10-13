use super::*;
const DEFAULT_PAD: usize = 3;
const DEFAULT_WIDTH_PERCENT: usize = 60;
const DEFAULT_HEIGHT_PERCENT: usize = 66;
const MIN_PANE_COLS: usize = 24;
const MIN_PANE_ROWS: usize = 8;
/// One `width`, `height` or `pad` value as written: a count of cells ("120"), or a
/// share of the whole ("60%", "60 %").
///
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

fn min_panel_cols(cols: usize, halves: usize) -> usize {
    (halves * MIN_PANE_COLS).min(cols.max(1))
}

fn panel_size(config: &BTreeMap<String, String>, rows: usize, cols: usize) -> (usize, usize) {
    let halves = 1;
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

    corner: Corner,
    size: Option<(usize, usize)>,
) -> Option<Rect> {
    let (pad, size) = placement_terms(config, surface_rows, surface_cols, size)?;
    Some(corner_rect(corner, surface_rows, surface_cols, pad, size))
}

fn placement_terms(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,

    size: Option<(usize, usize)>,
) -> Option<(usize, (usize, usize))> {
    if surface_rows == 0 || surface_cols == 0 {
        return None;
    }
    let pad = pad_of(config, surface_rows, surface_cols, 1);
    let size = size.unwrap_or_else(|| panel_size(config, surface_rows, surface_cols));
    Some((
        pad,
        (size.0.clamp(1, surface_cols), size.1.clamp(1, surface_rows)),
    ))
}

#[derive(Default)]
pub(super) struct Panel {
    pub(super) pane_id: Option<PaneId>,
    pub own_tab: Option<TabPosition>,
    client_tabs: BTreeMap<ClientId, TabPosition>,
    surface: (usize, usize),
    corner: Option<Corner>,
    rect: Option<Rect>,
    requested: Option<Rect>,
    floating: bool,
}
impl State {
    pub(super) fn panel_panes_changed(&mut self, manifest: &PaneManifest) {
        let Some((tab, own)) = manifest.panes.iter().find_map(|(tab, panes)| {
            panel_pane(manifest, self.own_plugin_id)
                .filter(|own| panes.iter().any(|p| p.id == own.id))
                .map(|own| (*tab, own))
        }) else {
            return;
        };
        self.panel.own_tab = Some(tab);
        self.panel.pane_id = Some(own.id);
        self.panel.floating = own.is_floating;
        if !own.is_floating {
            return;
        }
        if own.pane_columns.get() > 0 {
            self.panel.rect = Some(Rect {
                x: own.pane_x.get(),
                y: own.pane_y.get(),
                cols: own.pane_columns.get(),
                rows: own.pane_rows.get(),
            });
        }
        self.place_panel(false);
    }
    pub(super) fn panel_tabs_changed(&mut self, tabs: Vec<TabInfo>) {
        if let Some(active) = tabs.iter().find(|t| t.active) {
            let was = self
                .panel
                .client_tabs
                .insert(self.own_client_id, active.position);
            if self.pane_shown
                && was == self.panel.own_tab
                && self.panel.own_tab != Some(active.position)
            {
                break_panes_to_tab_with_index(
                    &[self
                        .panel
                        .pane_id
                        .unwrap_or(PaneId::Plugin(self.own_plugin_id))],
                    active.position,
                    false,
                );
            }
            let surface = (
                active.display_area_rows.get(),
                active.display_area_columns.get(),
            );
            if surface != self.panel.surface {
                self.panel.surface = surface;
                self.place_panel(false);
            }
        }
    }
    pub(super) fn place_panel(&mut self, override_user: bool) {
        if !self.panel.floating {
            return;
        }
        let Some(rect) = rect_in_corner(
            &self.configuration,
            self.panel.surface.0,
            self.panel.surface.1,
            self.panel.corner.unwrap_or(CORNERS[0]),
            None,
        ) else {
            return;
        };
        if !override_user && self.panel.requested == Some(rect) {
            return;
        }
        self.panel.requested = Some(rect);
        let coordinates = vec![(
            self.panel
                .pane_id
                .unwrap_or(PaneId::Plugin(self.own_plugin_id)),
            fixed_rect(rect),
        )];
        if override_user {
            change_overlay_coordinates_overriding_user_placement(coordinates);
        } else {
            change_overlay_coordinates(coordinates);
        }
        self.panel.rect.get_or_insert(rect);
    }
    pub(super) fn reposition(&mut self) {
        if !self.pane_shown || !self.panel.floating {
            return;
        }
        let corner = next_clockwise(self.panel.corner.unwrap_or(CORNERS[0]));
        self.panel.corner = Some(corner);
        let size = self.panel.rect.map(|r| (r.cols, r.rows));
        if let Some(rect) = rect_in_corner(
            &self.configuration,
            self.panel.surface.0,
            self.panel.surface.1,
            corner,
            size,
        ) {
            change_overlay_coordinates_overriding_user_placement(vec![(
                self.panel
                    .pane_id
                    .unwrap_or(PaneId::Plugin(self.own_plugin_id)),
                fixed_rect(rect),
            )]);
            self.panel.rect = Some(rect);
            self.panel.requested = rect_in_corner(
                &self.configuration,
                self.panel.surface.0,
                self.panel.surface.1,
                corner,
                None,
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_focus_updates_cannot_hide_a_revealed_panel() {
        let agents = PluginId::new(3);
        let mut state = State {
            own_plugin_id: agents,
            pane_shown: true,
            ..Default::default()
        };
        let manifest = |focused| PaneManifest {
            panes: std::collections::HashMap::from([(
                TabPosition::new(0),
                vec![
                    PaneInfo {
                        id: PaneId::Plugin(agents),
                        is_floating: true,
                        is_focused: focused,
                        ..Default::default()
                    },
                    PaneInfo {
                        id: PaneId::Terminal(TerminalId::new(1)),
                        is_focused: !focused,
                        ..Default::default()
                    },
                ],
            )]),
        };
        state.panel_panes_changed(&manifest(true));
        // The host has dismissed and revealed the panel before delivering this older update.
        state.panel_panes_changed(&manifest(false));
        assert_eq!(
            state.panel_hides, 0,
            "focus updates must not enqueue a delayed dismissal"
        );
        assert!(state.pane_shown);
    }

    #[test]
    fn default_panel_fits_and_moves_clockwise() {
        let rect = rect_in_corner(&BTreeMap::new(), 60, 200, Corner::BottomLeft, None).unwrap();
        assert_eq!((rect.x, rect.y, rect.cols, rect.rows), (3, 18, 120, 39));
        assert_eq!(next_clockwise(Corner::BottomLeft), Corner::TopLeft);
        for rows in [1, 4, 60] {
            for cols in [1, 20, 200] {
                let r = rect_in_corner(&BTreeMap::new(), rows, cols, Corner::BottomRight, None)
                    .unwrap();
                assert!(r.x + r.cols <= cols && r.y + r.rows <= rows);
            }
        }
    }
}
