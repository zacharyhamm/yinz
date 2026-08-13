//! Tests for the one thing a tab warp cannot get wrong.
//!
//! The receiving session fills a layout's terminal slots positionally: it walks
//! the flattened layout and, for every slot that is not a plugin, takes the next
//! pane off a queue. So the whole feature rests on the source having put the
//! panes in that queue in the same order. If it has not, nothing errors — the
//! tab simply arrives with its panes in each other's places, which is the kind
//! of bug that survives a demo.
//!
//! These tests therefore do not check the ordering against a second copy of the
//! rule. They generate a real tab arrangement, run it through the real
//! positioning code the receiving side uses, and check that the pane that lands
//! in each slot is the pane that was there to begin with.

use std::collections::HashMap;

use hegel::generators::{self};

use super::*;
use crate::input::command::RunCommand;
use crate::input::layout::{Run, RunPlugin, RunPluginLocation, RunPluginOrAlias, SplitDirection};
use crate::pane_size::PaneGeom;

/// The whole viewport, as `position_panes_in_space` wants it.
///
/// Built the way `total_space_for_tiled_panes` builds it — a default
/// (percentage) `Dimension` with its inner size set — rather than a fixed one.
/// A fixed space makes `split_space` refuse to size children implicitly, which
/// is every child a warped layout has.
fn space(viewport: Size) -> PaneGeom {
    let mut space = PaneGeom::default();
    space.cols.set_inner(viewport.cols);
    space.rows.set_inner(viewport.rows);
    space
}

/// How far a pane moved, in cells.
///
/// Used to ask whether a pane got *its own* rectangle rather than whether it got
/// exactly the rectangle it had. The two differ because reconstructing a layout
/// from geometry goes through percentages, so a pane can come back a cell off
/// without anything having gone wrong. What cannot be tolerated is a pane
/// landing in another pane's rectangle, and that is a much larger distance —
/// zellij will not make a pane smaller than a few cells, so no amount of
/// rounding can move one far enough to be mistaken for its neighbour.
fn moved_by(from: &PaneGeom, to: &PaneGeom) -> usize {
    from.x.abs_diff(to.x) + from.y.abs_diff(to.y)
}

/// A tab arrangement built the way a real one is: by splitting a viewport.
///
/// Generated as a layout and then positioned, rather than as geometry directly,
/// because only a tiling that some layout could have produced is one a tab could
/// actually be in — and a random set of rectangles is almost never one.
#[hegel::composite]
fn tab_arrangement(
    tc: &hegel::TestCase,
    viewport: Size,
    plugin_panes: bool,
) -> Vec<PaneLayoutManifest> {
    let mut layout = TiledPaneLayout::default();
    let depth = tc.draw(generators::integers::<usize>().min_value(0).max_value(3));
    split(&tc, &mut layout, depth, plugin_panes);

    let positioned = layout
        .position_panes_in_space(&space(viewport), None, false, false)
        .unwrap_or_default();
    positioned
        .into_iter()
        .map(|(layout, geom)| PaneLayoutManifest {
            geom,
            run: layout.run,
            ..Default::default()
        })
        .collect()
}

fn split(tc: &hegel::TestCase, node: &mut TiledPaneLayout, depth: usize, plugin_panes: bool) {
    let children = if depth == 0 {
        0
    } else {
        tc.draw(generators::integers::<usize>().min_value(0).max_value(3))
    };
    if children == 0 {
        // Every leaf gets a run, and some of them get a plugin, because a slot
        // holding a plugin is the one kind the receiving side does *not* take a
        // pane off the queue for.
        node.run = Some(if plugin_panes && tc.draw(generators::booleans()) {
            Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                location: RunPluginLocation::parse("zellij:tab-bar", None).unwrap(),
                ..Default::default()
            }))
        } else {
            Run::Command(RunCommand {
                command: "sh".into(),
                ..Default::default()
            })
        });
        return;
    }
    node.children_split_direction = if tc.draw(generators::booleans()) {
        SplitDirection::Horizontal
    } else {
        SplitDirection::Vertical
    };
    node.children = (0..children)
        .map(|_| {
            let mut child = TiledPaneLayout::default();
            split(tc, &mut child, depth - 1, plugin_panes);
            child
        })
        .collect();
}

fn is_plugin_manifest(manifest: &PaneLayoutManifest) -> bool {
    matches!(manifest.run, Some(Run::Plugin(_)))
}

/// Lay a warped layout out the way the receiving session will.
///
/// Both steps of the way, which matters: `LayoutApplier::flatten_layout` retries
/// with percentage sizes ignored when the first attempt finds no room, and a
/// reconstructed layout in a small viewport reaches that second attempt often
/// enough that an oracle without it would be testing something the receiver
/// never does.
fn position_as_the_receiver_does(
    layout: &TiledPaneLayout,
    viewport: Size,
) -> Option<Vec<(TiledPaneLayout, PaneGeom)>> {
    layout
        .position_panes_in_space(&space(viewport), None, false, false)
        .or_else(|_| layout.position_panes_in_space(&space(viewport), None, true, false))
        .ok()
}

/// Fill a layout's terminal slots the way `position_new_panes` does, and report
/// which of the original panes ended up where.
///
/// This is the oracle: it is the receiving side's rule — walk the flattened
/// layout, skip plugin slots, take the next queued pane for everything else —
/// run against the real positioning code rather than against a description of it.
///
/// `None` when the tab does not fit the receiving session as laid out —
/// deliberately without `flatten_layout`'s relaxed second attempt, even though
/// that is what the receiver falls back on: relaxing means throwing the
/// requested sizes away and dividing the space evenly, so panes land nowhere in
/// particular by design. Positions after that are not evidence of anything, and
/// a test that compared them would be asserting that a deliberate approximation
/// came out exact.
fn slots_as_the_receiver_fills_them(
    arrangement: &WarpedTabLayout,
    queue: &[usize],
    viewport: Size,
) -> Option<Vec<(usize, PaneGeom)>> {
    let positioned = arrangement
        .tiled_layout
        .position_panes_in_space(&space(viewport), None, false, false)
        .ok()?;

    let mut queue = queue.iter();
    let mut filled = vec![];
    for (layout, geom) in positioned {
        if matches!(layout.run, Some(Run::Plugin(_))) {
            continue;
        }
        if let Some(pane) = queue.next() {
            filled.push((*pane, geom));
        }
    }
    Some(filled)
}

#[hegel::test(test_cases = 500)]
fn every_pane_lands_in_the_place_it_came_from(tc: hegel::TestCase) {
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let tiled = tc.draw(tab_arrangement(viewport, true));
    tc.assume(tiled.iter().any(|pane| !is_plugin_manifest(pane)));

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &[], viewport) else {
        // A tiling this code cannot express is refused rather than mangled, and
        // refusing is a documented outcome. Nothing to check.
        return;
    };

    // Rebuilt at the size it left, so the rectangles are the ones the source
    // had. The check is that no two panes would be better off swapped: if the
    // queue put pane A where pane B belonged, then each is sitting nearer the
    // other's old place than its own, and this says so.
    let Some(filled) =
        slots_as_the_receiver_fills_them(&arrangement, &arrangement.tiled_order, viewport)
    else {
        return;
    };
    for (a, landed_a) in &filled {
        for (b, landed_b) in &filled {
            let (was_a, was_b) = (&tiled[*a].geom, &tiled[*b].geom);
            assert!(
                moved_by(was_a, landed_a) + moved_by(was_b, landed_b)
                    <= moved_by(was_a, landed_b) + moved_by(was_b, landed_a),
                "panes {a} and {b} were queued for each other's slots",
            );
        }
    }
}

#[hegel::test]
fn every_pane_with_a_process_travels_exactly_once(tc: hegel::TestCase) {
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let tiled = tc.draw(tab_arrangement(viewport, true));

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &[], viewport) else {
        return;
    };

    // A pane left out would strand a running process in a session that no longer
    // has a tab for it; a pane sent twice would name the same supervisor in two
    // slots. Both are silent, so both are checked rather than assumed.
    let mut seen = arrangement.tiled_order.clone();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), arrangement.tiled_order.len());

    let expected: Vec<usize> = tiled
        .iter()
        .enumerate()
        .filter(|(_, pane)| !is_plugin_manifest(pane))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(seen, expected);
}

#[hegel::test]
fn the_queue_is_never_shorter_than_the_slots_waiting_for_it(tc: hegel::TestCase) {
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let tiled = tc.draw(tab_arrangement(viewport, true));

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &[], viewport) else {
        return;
    };

    // If the layout has more terminal slots than the queue has panes, the extra
    // slots are filled with nothing — `position_new_panes` simply stops — and
    // the tab arrives with holes in it.
    let Some(positioned) = position_as_the_receiver_does(&arrangement.tiled_layout, viewport)
    else {
        // No room even with the sizes relaxed. The receiver says so rather than
        // building a tab with holes in it.
        return;
    };
    let slots = positioned
        .iter()
        .filter(|(layout, _)| !matches!(layout.run, Some(Run::Plugin(_))))
        .count();
    assert_eq!(slots, arrangement.tiled_order.len());
}

#[hegel::test]
fn floating_panes_travel_in_the_order_their_slots_are_filled(tc: hegel::TestCase) {
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let tiled = tc.draw(tab_arrangement(viewport, false));
    let floating = tc.draw(tab_arrangement(viewport, true));

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &floating, viewport) else {
        return;
    };

    // Floating slots are filled in plain list order, skipping plugins — so the
    // order is the non-plugin indices in sequence, and the layout keeps every
    // pane including the plugins.
    let expected: Vec<usize> = floating
        .iter()
        .enumerate()
        .filter(|(_, pane)| !is_plugin_manifest(pane))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(arrangement.floating_order, expected);
    assert_eq!(arrangement.floating_layout.len(), floating.len());
}

#[hegel::test]
fn a_floating_pane_is_placed_as_a_fraction_of_the_viewport_it_left(tc: hegel::TestCase) {
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let tiled = tc.draw(tab_arrangement(viewport, false));
    let floating = tc.draw(tab_arrangement(viewport, false));

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &floating, viewport) else {
        return;
    };

    // Fixed coordinates would put a pane from a wide session off the edge of a
    // narrow one. Percentages are what make the arrangement survive the trip, so
    // nothing may arrive still measured in cells.
    for pane in &arrangement.floating_layout {
        for value in [pane.x, pane.y, pane.width, pane.height]
            .into_iter()
            .flatten()
        {
            match value {
                PercentOrFixed::Percent(percent) => assert!(percent <= 100),
                PercentOrFixed::Fixed(fixed) => {
                    panic!("a floating pane arrived measured in cells: {fixed}")
                },
            }
        }
    }
}

#[test]
fn panes_running_the_same_command_are_still_told_apart() {
    // The reason the ordering is read off a marked layout rather than off the
    // panes' own run instructions: four identical shells are indistinguishable
    // by what they are running, and matching on that would put them in
    // arbitrary slots.
    let viewport = Size { cols: 80, rows: 24 };
    let mut layout = TiledPaneLayout::default();
    layout.children_split_direction = SplitDirection::Vertical;
    layout.children = (0..4)
        .map(|_| TiledPaneLayout {
            run: Some(Run::Command(RunCommand {
                command: "sh".into(),
                ..Default::default()
            })),
            ..Default::default()
        })
        .collect();

    let tiled: Vec<PaneLayoutManifest> = layout
        .position_panes_in_space(&space(viewport), None, false, false)
        .unwrap()
        .into_iter()
        .map(|(layout, geom)| PaneLayoutManifest {
            geom,
            run: layout.run,
            ..Default::default()
        })
        .collect();

    let arrangement = layout_for_warped_tab(&tiled, &[], viewport).unwrap();
    let by_x: HashMap<usize, usize> = tiled
        .iter()
        .enumerate()
        .map(|(index, pane)| (index, pane.geom.x))
        .collect();

    let filled = slots_as_the_receiver_fills_them(&arrangement, &arrangement.tiled_order, viewport)
        .expect("an 80x24 terminal has room for four panes");
    assert_eq!(filled.len(), 4);
    for (pane, landed) in filled {
        assert_eq!(
            by_x[&pane], landed.x,
            "identical shells were shuffled between slots"
        );
    }
}

#[hegel::test]
fn a_tab_keeps_its_shape_at_a_size_it_was_never_arranged_for(tc: hegel::TestCase) {
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let tiled = tc.draw(tab_arrangement(viewport, false));
    tc.assume(tiled.len() > 1);

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &[], viewport) else {
        return;
    };

    // Whichever way a receiving session divides its own viewport, it must still
    // divide it into as many panes as were sent — a layout that loses a pane at
    // some sizes and not others would strand a process for no visible reason.
    let elsewhere = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let Some(positioned) = position_as_the_receiver_does(&arrangement.tiled_layout, elsewhere)
    else {
        // Genuinely no room: the receiving session is too small for this many
        // panes, which it says rather than silently dropping some.
        return;
    };
    assert_eq!(positioned.len(), tiled.len());
}

#[test]
fn a_tab_with_nothing_tiled_in_it_is_refused() {
    // The smallest layout there is still describes one pane, so an empty tab
    // would arrive with a slot nothing fills. Saying so beats reconstructing
    // something that was never there.
    let viewport = Size { cols: 80, rows: 24 };
    assert_eq!(
        layout_for_warped_tab(&[], &[], viewport),
        Err("this tab has no tiled panes, so there is no arrangement to move".to_owned())
    );
}

#[test]
fn a_tab_of_one_pane_is_a_queue_of_one() {
    let viewport = Size { cols: 80, rows: 24 };
    let tiled = vec![PaneLayoutManifest {
        geom: space(viewport),
        run: Some(Run::Command(RunCommand {
            command: "sh".into(),
            ..Default::default()
        })),
        ..Default::default()
    }];
    let arrangement = layout_for_warped_tab(&tiled, &[], viewport).unwrap();
    assert_eq!(arrangement.tiled_order, vec![0]);
}

#[test]
fn a_tab_of_nothing_but_plugins_sends_no_panes() {
    // Nothing to hand over, but the layout still has to carry the plugin so the
    // receiving session can start it. `Screen` refuses this case earlier, for a
    // different reason: a tab with no processes is not worth moving.
    let viewport = Size { cols: 80, rows: 24 };
    let tiled = vec![PaneLayoutManifest {
        geom: space(viewport),
        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::parse("zellij:tab-bar", None).unwrap(),
            ..Default::default()
        }))),
        ..Default::default()
    }];
    let arrangement = layout_for_warped_tab(&tiled, &[], viewport).unwrap();
    assert!(arrangement.tiled_order.is_empty());
    assert!(matches!(arrangement.tiled_layout.run, Some(Run::Plugin(_))));
}
