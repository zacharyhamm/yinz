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
