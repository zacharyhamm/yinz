//! Composition identities and geometry shared by the host and plugin APIs.
use serde::{Deserialize, Serialize};

use crate::{pane_id::PaneId, pane_size::PaneGeom};

/// A container identity, independent of either child's process or plugin identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MultipaneId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MultipaneDirection {
    LeftRight,
    TopBottom,
}

/// Requested first-child share, in basis points. Effective sizes may be clamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MultipaneSplit {
    pub direction: MultipaneDirection,
    pub ratio: u16,
}

impl MultipaneSplit {
    pub fn validate(self) -> Result<(), &'static str> {
        if self.ratio == 0 || self.ratio >= 10_000 {
            return Err("multipane ratio must be between 1 and 9999 basis points");
        }
        Ok(())
    }

    /// The fewest cells either half may be given out of `available`: `min_cells`, or an even
    /// share when the surface cannot hold two of those, and never under one cell while both
    /// halves fit. Taking the even share rather than dropping the floor keeps the halves from
    /// jumping to a one-cell sliver the moment the panel narrows past twice the floor.
    pub fn floor(available: usize, min_cells: u16) -> usize {
        usize::from(available >= 2).max(usize::from(min_cells).min(available / 2))
    }

    /// Subtract one outer frame and one divider, then derive both content viewports.
    /// Zero-sized slots remain present when the surface cannot accommodate them.
    ///
    /// Each half keeps at least `Self::floor(available, min_cells)` cells whatever the ratio
    /// asks for. The floor clamps only the effective sizes: the ratio itself is left alone, so a
    /// divider squeezed by a narrow surface returns to where it was when the surface grows.
    pub fn geometry(
        self,
        outer: PaneGeom,
        framed: bool,
        min_cells: u16,
    ) -> ([PaneGeom; 2], PaneGeom) {
        let mut content = outer;
        let inset_x = usize::from(framed).min(outer.cols.as_usize());
        let inset_y = usize::from(framed).min(outer.rows.as_usize());
        content.x += inset_x;
        content.y += inset_y;
        content
            .cols
            .set_inner(outer.cols.as_usize().saturating_sub(2 * inset_x));
        content
            .rows
            .set_inner(outer.rows.as_usize().saturating_sub(2 * inset_y));
        let horizontal = self.direction == MultipaneDirection::LeftRight;
        let length = if horizontal {
            content.cols.as_usize()
        } else {
            content.rows.as_usize()
        };
        let available = length.saturating_sub(1);
        let floor = Self::floor(available, min_cells);
        let first = (available * usize::from(self.ratio) / 10_000).clamp(floor, available - floor);
        let mut children = [content; 2];
        let mut divider = content;
        if horizontal {
            children[0].cols.set_inner(first);
            divider.x += first;
            divider.cols.set_inner(length.min(1));
            children[1].x += first + length.min(1);
            children[1].cols.set_inner(available - first);
        } else {
            children[0].rows.set_inner(first);
            divider.y += first;
            divider.rows.set_inner(length.min(1));
            children[1].y += first + length.min(1);
            children[1].rows.set_inner(available - first);
        }
        (children, divider)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MultipaneInfo {
    pub id: MultipaneId,
    pub children: [PaneId; 2],
    pub x: usize,
    pub y: usize,
    pub rows: usize,
    pub columns: usize,
    pub split: MultipaneSplit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_size::Size;

    #[test]
    fn both_directions_partition_content_even_on_tiny_surfaces() {
        for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
            let split = MultipaneSplit {
                direction,
                ratio: 3700,
            };
            for min_cells in [0, 1, 4, 24] {
                for rows in 0..12 {
                    for cols in 0..12 {
                        let outer = PaneGeom::from(&Size { rows, cols });
                        let ([first, second], divider) = split.geometry(outer, true, min_cells);
                        for geom in [first, second, divider] {
                            assert!(geom.x + geom.cols.as_usize() <= cols);
                            assert!(geom.y + geom.rows.as_usize() <= rows);
                        }
                        if direction == MultipaneDirection::LeftRight {
                            assert_eq!(first.x + first.cols.as_usize(), divider.x);
                            assert_eq!(divider.x + divider.cols.as_usize(), second.x);
                        } else {
                            assert_eq!(first.y + first.rows.as_usize(), divider.y);
                            assert_eq!(divider.y + divider.rows.as_usize(), second.y);
                        }
                        assert_eq!(split.ratio, 3700);
                    }
                }
            }
        }
        assert!(MultipaneSplit {
            direction: MultipaneDirection::LeftRight,
            ratio: 0
        }
        .validate()
        .is_err());
    }

    /// The agent panel's `split "0"` and `split "100%"` became a 1 and a 9999 basis-point
    /// ratio, and left one half a single column wide. With a floor each half keeps it, and on a
    /// panel too narrow for two floors the halves share what there is evenly instead.
    #[test]
    fn a_floor_keeps_both_halves_usable_whatever_the_ratio() {
        let widths = |ratio, cols, min_cells| {
            let split = MultipaneSplit {
                direction: MultipaneDirection::LeftRight,
                ratio,
            };
            let outer = PaneGeom::from(&Size { rows: 10, cols });
            let ([first, second], _) = split.geometry(outer, true, min_cells);
            (first.cols.as_usize(), second.cols.as_usize())
        };
        // 80 columns less two frame columns and the divider leave 77 to share.
        assert_eq!(
            widths(1, 80, 0),
            (1, 76),
            "no floor: the old one-column half"
        );
        assert_eq!(widths(1, 80, 24), (24, 53));
        assert_eq!(widths(9999, 80, 24), (53, 24));
        assert_eq!(
            widths(5000, 80, 24),
            (38, 39),
            "a ratio inside the floors is untouched"
        );
        // 41 columns leave 38: too few for two floors of 24, so each half gets 19.
        assert_eq!(widths(1, 41, 24), (19, 19));
        assert_eq!(widths(9999, 41, 24), (19, 19));
        assert_eq!(MultipaneSplit::floor(3, 24), 1);
        assert_eq!(MultipaneSplit::floor(1, 24), 0);
    }

    /// `min_cells` arrived after `CreateMultipane` was already on the wire, so a request that
    /// does not name it still parses, with no floor beyond the one cell a half always keeps.
    #[test]
    fn a_create_request_without_a_floor_still_parses() {
        let request = CreateMultipane {
            children: [1, 2].map(|id| {
                MultipaneChild::Existing(PaneId::Plugin(crate::internal_ids::PluginId::new(id)))
            }),
            placement: crate::data::OverlayPlacement::default(),
            split: MultipaneSplit {
                direction: MultipaneDirection::LeftRight,
                ratio: 5000,
            },
            focus: None,
            min_cells: 24,
        };
        let mut json = serde_json::to_value(&request).unwrap();
        json.as_object_mut().unwrap().remove("min_cells");
        let parsed: CreateMultipane = serde_json::from_value(json).unwrap();
        assert_eq!(
            parsed,
            CreateMultipane {
                min_cells: 0,
                ..request
            }
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MultipaneChild {
    Existing(PaneId),
    Terminal(crate::command::CommandToRun),
    Plugin {
        url: String,
        configuration: std::collections::BTreeMap<String, String>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateMultipane {
    pub children: [MultipaneChild; 2],
    pub placement: crate::data::OverlayPlacement,
    pub split: MultipaneSplit,
    /// Child index to focus for the requesting client; None preserves current focus.
    pub focus: Option<usize>,
    /// Fewest cells either half keeps along the split direction, whatever the ratio or a divider
    /// drag asks for. When the container cannot hold two of these, the halves share it evenly.
    /// 0 asks for no floor beyond the one cell a half always keeps.
    #[serde(default)]
    pub min_cells: u16,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateMultipane {
    pub id: MultipaneId,
    pub coordinates: Option<crate::geometry::FloatingPaneCoordinates>,
    pub split: Option<MultipaneSplit>,
    pub override_user_placement: bool,
    /// Below 100 opts into global background_opacity; the percentage does not override it.
    /// None preserves eligibility; 100 makes the container opaque.
    #[serde(default)]
    pub background_opacity: Option<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceMultipaneChild {
    pub id: MultipaneId,
    pub slot: usize,
    pub child: MultipaneChild,
}
/// Host operations after launch descriptions have been resolved to reserved pane slots.
#[derive(Debug, Clone)]
pub enum MultipaneOperation {
    Validate(Vec<PaneId>),
    Create {
        children: [PaneId; 2],
        placement: crate::data::OverlayPlacement,
        split: MultipaneSplit,
        focus: Option<usize>,
        min_cells: u16,
    },
    Get(MultipaneId),
    Update(UpdateMultipane),
    Replace {
        id: MultipaneId,
        slot: usize,
        child: PaneId,
    },
}
pub type MultipaneResponse = Result<Option<MultipaneInfo>, String>;
