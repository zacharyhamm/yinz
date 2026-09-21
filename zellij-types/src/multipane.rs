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

    /// Subtract one outer frame and one divider, then derive both content viewports.
    /// Zero-sized slots remain present when the surface cannot accommodate them.
    pub fn geometry(self, outer: PaneGeom, framed: bool) -> ([PaneGeom; 2], PaneGeom) {
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
        let minimum = usize::from(available >= 2);
        let first = (available * usize::from(self.ratio) / 10_000)
            .clamp(minimum, available.saturating_sub(minimum));
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
            for rows in 0..12 {
                for cols in 0..12 {
                    let outer = PaneGeom::from(&Size { rows, cols });
                    let ([first, second], divider) = split.geometry(outer, true);
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
        assert!(MultipaneSplit {
            direction: MultipaneDirection::LeftRight,
            ratio: 0
        }
        .validate()
        .is_err());
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
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateMultipane {
    pub id: MultipaneId,
    pub coordinates: Option<crate::geometry::FloatingPaneCoordinates>,
    pub split: Option<MultipaneSplit>,
    pub override_user_placement: bool,
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
