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
use crate::session_serialization::get_tiled_panes_layout_from_panegeoms;
use yinz_types::geometry::SplitDirection;
use yinz_types::input::command::RunCommand;
use yinz_types::pane_size::PaneGeom;
use {
    yinz_types::input::layout::Run, yinz_types::input::layout::RunPlugin,
    yinz_types::input::layout::RunPluginOrAlias,
};

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
/// yinz will not make a pane smaller than a few cells, so no amount of
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
    tc.draw(tab_arrangement_maybe_stacked(viewport, plugin_panes, false))
}

/// The same, with stacks in it.
///
/// A stack is what a tab holding a stacked pane list travels as: the members
/// share a `stacked` id, all but one are a single row, and the expanded one takes
/// what is left. That is the geometry `Tab::stack_list_serialization_geoms`
/// synthesises for the hidden members, so generating it here is generating the
/// shape a warped stack list actually arrives in.
#[hegel::composite]
fn tab_arrangement_maybe_stacked(
    tc: &hegel::TestCase,
    viewport: Size,
    plugin_panes: bool,
    stacks: bool,
) -> Vec<PaneLayoutManifest> {
    let mut layout = TiledPaneLayout::default();
    let depth = tc.draw(generators::integers::<usize>().min_value(0).max_value(3));
    split(tc, &mut layout, depth, plugin_panes, stacks);

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

/// Generate a stack directly so stack-specific properties never reject stackless samples.
#[hegel::composite]
fn tab_arrangement_with_stack(
    tc: &hegel::TestCase,
    viewport: Size,
    plugin_panes: bool,
) -> Vec<PaneLayoutManifest> {
    let columns = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
    let required = tc.draw(
        generators::integers::<usize>()
            .min_value(0)
            .max_value(columns - 1),
    );
    let layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: (0..columns)
            .map(|column| {
                let mut node = TiledPaneLayout::default();
                if column == required || tc.draw(generators::booleans()) {
                    let members =
                        tc.draw(generators::integers::<usize>().min_value(2).max_value(
                            viewport.rows - yinz_types::pane_size::MIN_TERMINAL_HEIGHT + 1,
                        ));
                    let expanded = tc.draw(
                        generators::integers::<usize>()
                            .min_value(0)
                            .max_value(members - 1),
                    );
                    node.children_are_stacked = true;
                    node.children_split_direction = SplitDirection::Horizontal;
                    node.children = (0..members)
                        .map(|index| {
                            let mut child = TiledPaneLayout::default();
                            // The required stack always exercises ordering between terminals.
                            split(
                                tc,
                                &mut child,
                                0,
                                plugin_panes && (column != required || index >= 2),
                                false,
                            );
                            child.is_expanded_in_stack = index == expanded;
                            child
                        })
                        .collect();
                } else {
                    split(tc, &mut node, 0, plugin_panes, false);
                }
                node
            })
            .collect(),
        ..Default::default()
    };
    layout
        .position_panes_in_space(&space(viewport), None, false, false)
        .expect("bounded columns and stack members must fit the viewport")
        .into_iter()
        .map(|(layout, geom)| PaneLayoutManifest {
            geom,
            run: layout.run,
            ..Default::default()
        })
        .collect()
}

fn split(
    tc: &hegel::TestCase,
    node: &mut TiledPaneLayout,
    depth: usize,
    plugin_panes: bool,
    stacks: bool,
) {
    let children = if depth == 0 {
        0
    } else {
        tc.draw(generators::integers::<usize>().min_value(0).max_value(3))
    };
    // A stack needs something to stack, so only nodes with more than one child
    // can be one. Its members are leaves: a stack of subdivisions is not a shape
    // a tab can be in.
    if stacks && children > 1 && tc.draw(generators::booleans()) {
        node.children_are_stacked = true;
        node.children_split_direction = SplitDirection::Horizontal;
        let expanded = tc.draw(
            generators::integers::<usize>()
                .min_value(0)
                .max_value(children - 1),
        );
        node.children = (0..children)
            .map(|index| {
                let mut child = TiledPaneLayout::default();
                split(tc, &mut child, 0, plugin_panes, false);
                child.is_expanded_in_stack = index == expanded;
                child
            })
            .collect();
        return;
    }
    if children == 0 {
        // Every leaf gets a run, and some of them get a plugin, because a slot
        // holding a plugin is the one kind the receiving side does *not* take a
        // pane off the queue for.
        node.run = Some(if plugin_panes && tc.draw(generators::booleans()) {
            Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                location: yinz_runtime::plugin_location::parse_plugin_location(
                    "yinz:tab-bar",
                    None,
                )
                .unwrap(),
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
            split(tc, &mut child, depth - 1, plugin_panes, stacks);
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
    let layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: (0..4)
            .map(|_| TiledPaneLayout {
                run: Some(Run::Command(RunCommand {
                    command: "sh".into(),
                    ..Default::default()
                })),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };

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

// ---------------------------------------------------------------------------
// Tabs holding a stack
// ---------------------------------------------------------------------------
//
// A stacked pane list travels as an ordinary in-grid stack: its hidden members
// are one row each, its visible one takes the rest, and they share a `stacked`
// id. The properties worth pinning down are the ones about the *queue* - that
// every member travels once and the slots waiting for them are exactly as many.
// Deliberately not the "no two panes would be better off swapped" property:
// stacked members are a single row apart, which is nearer than the rounding a
// reconstructed layout is allowed, so distance stops being evidence there.

#[hegel::test]
fn every_member_of_a_stack_travels_exactly_once(tc: hegel::TestCase) {
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let tiled = tc.draw(tab_arrangement_maybe_stacked(viewport, true, true));

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &[], viewport) else {
        return;
    };

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
fn a_stack_arrives_with_as_many_slots_as_it_had_members(tc: hegel::TestCase) {
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let tiled = tc.draw(tab_arrangement_maybe_stacked(viewport, true, true));

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &[], viewport) else {
        return;
    };

    let Some(positioned) = position_as_the_receiver_does(&arrangement.tiled_layout, viewport)
    else {
        return;
    };
    let slots = positioned
        .iter()
        .filter(|(layout, _)| !matches!(layout.run, Some(Run::Plugin(_))))
        .count();
    assert_eq!(slots, arrangement.tiled_order.len());
}

#[test]
fn a_stacks_members_travel_top_to_bottom_and_keep_which_one_was_open() {
    // The shape a warped stacked pane list is in: three members sharing a stack,
    // the middle one expanded and the other two collapsed to their title row.
    // Both halves have to survive - the order, or the panes arrive under each
    // other's titles, and the expansion, or the tab arrives showing the wrong one.
    let viewport = Size { cols: 80, rows: 24 };
    let layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children_are_stacked: true,
        children: (0..3)
            .map(|index| TiledPaneLayout {
                run: Some(Run::Command(RunCommand {
                    command: "sh".into(),
                    ..Default::default()
                })),
                is_expanded_in_stack: index == 1,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };

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
    assert_eq!(tiled.len(), 3);

    let arrangement = layout_for_warped_tab(&tiled, &[], viewport).unwrap();
    assert_eq!(
        arrangement.tiled_order,
        vec![0, 1, 2],
        "a stack's members are queued in the order they are stacked",
    );

    let filled = slots_as_the_receiver_fills_them(&arrangement, &arrangement.tiled_order, viewport)
        .expect("an 80x24 terminal has room for a stack of three");
    let mut by_y: Vec<(usize, usize, usize)> = filled
        .iter()
        .map(|(pane, geom)| (geom.y, *pane, geom.rows.as_usize()))
        .collect();
    by_y.sort_unstable();
    assert_eq!(
        by_y.iter().map(|(_, pane, _)| *pane).collect::<Vec<_>>(),
        vec![0, 1, 2],
        "members landed in each other's rows",
    );
    assert_eq!(
        by_y.iter().map(|(_, _, rows)| *rows).collect::<Vec<_>>(),
        vec![1, 22, 1],
        "the open member should arrive open and the rest collapsed",
    );
}

/// A tab that is nothing but one stack, plus which member was asked to be open.
///
/// `tab_arrangement_maybe_stacked` can produce stacks but cannot say which of the
/// panes it produced was the expanded one, and that is the subject here. Members
/// are all terminals: a plugin cannot be the pane whose process is handed over,
/// so a stack made of them would have nothing to say about the queue.
#[hegel::composite]
fn a_single_stack(tc: &hegel::TestCase, viewport: Size) -> (Vec<PaneLayoutManifest>, usize) {
    let members = tc.draw(generators::integers::<usize>().min_value(2).max_value(8));
    let expanded = tc.draw(
        generators::integers::<usize>()
            .min_value(0)
            .max_value(members - 1),
    );
    let layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children_are_stacked: true,
        children: (0..members)
            .map(|index| TiledPaneLayout {
                run: Some(Run::Command(RunCommand {
                    command: "sh".into(),
                    ..Default::default()
                })),
                is_expanded_in_stack: index == expanded,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };

    let positioned = layout
        .position_panes_in_space(&space(viewport), None, false, false)
        .unwrap_or_default();
    let manifests = positioned
        .into_iter()
        .map(|(layout, geom)| PaneLayoutManifest {
            geom,
            run: layout.run,
            ..Default::default()
        })
        .collect();
    (manifests, expanded)
}

#[hegel::test]
fn a_stack_arrives_with_the_same_member_open(tc: hegel::TestCase) {
    // The property behind `a_stacks_members_travel_top_to_bottom_and_keep_which
    // _one_was_open`, which pins it with a single stack of three at 80x24. Which
    // member is open is not carried as a flag - it is carried as the one member
    // taller than its title row - so it survives the trip only if the geometry
    // does, at whatever size and depth the stack happens to have.
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let (tiled, expanded) = tc.draw(a_single_stack(viewport));
    // Too many members for the rows there are: the stack is refused rather than
    // squeezed, so there is no arrangement to move.
    tc.assume(tiled.len() > 1);

    let open_before: Vec<usize> = tiled
        .iter()
        .enumerate()
        .filter(|(_, pane)| pane.geom.rows.as_usize() > 1)
        .map(|(index, _)| index)
        .collect();
    // The generator's claim, read back off the geometry rather than trusted. The
    // empty case is a stack with exactly one row per member, where nothing is
    // open and there is nothing for the trip to lose.
    assert!(
        open_before.is_empty() || open_before == vec![expanded],
        "a stack should arrange itself with member {expanded} open, not {open_before:?}",
    );

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &[], viewport) else {
        return;
    };
    let Some(filled) =
        slots_as_the_receiver_fills_them(&arrangement, &arrangement.tiled_order, viewport)
    else {
        return;
    };

    let mut open_after: Vec<usize> = filled
        .iter()
        .filter(|(_, geom)| geom.rows.as_usize() > 1)
        .map(|(pane, _)| *pane)
        .collect();
    open_after.sort_unstable();
    assert_eq!(
        open_after, open_before,
        "the tab arrived showing a different member of the stack",
    );
}

#[hegel::test]
fn a_stacks_members_arrive_in_the_order_they_were_stacked(tc: hegel::TestCase) {
    // `every_member_of_a_stack_travels_exactly_once` establishes that the queue is
    // a permutation of the members, which leaves the permutation itself unchecked.
    // Order is the half that matters on screen: a stack renders its members'
    // titles in a column, so two members swapped means two panes sitting under
    // each other's names. Distance is no evidence here - members are a row apart -
    // but their order still is.
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let tiled = tc.draw(tab_arrangement_with_stack(viewport, true));
    assert!(tiled.iter().any(|pane| pane.geom.stacked.is_some()));

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &[], viewport) else {
        return;
    };
    let Some(filled) =
        slots_as_the_receiver_fills_them(&arrangement, &arrangement.tiled_order, viewport)
    else {
        return;
    };

    let mut arrived_by_stack: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    for (pane, geom) in &filled {
        if let Some(stack_id) = tiled[*pane].geom.stacked {
            arrived_by_stack
                .entry(stack_id)
                .or_default()
                .push((geom.y, *pane));
        }
    }

    for (stack_id, mut arrived) in arrived_by_stack {
        arrived.sort_unstable();
        let arrived: Vec<usize> = arrived.into_iter().map(|(_, pane)| pane).collect();

        // Plugin members are left out of both sides: the receiving session starts
        // those afresh from the layout rather than taking one off the queue, so
        // they occupy no slot for a pane to land in the wrong one of.
        let mut before: Vec<(usize, usize)> = tiled
            .iter()
            .enumerate()
            .filter(|(_, pane)| pane.geom.stacked == Some(stack_id) && !is_plugin_manifest(pane))
            .map(|(index, pane)| (pane.geom.y, index))
            .collect();
        before.sort_unstable();
        let before: Vec<usize> = before.into_iter().map(|(_, pane)| pane).collect();

        assert_eq!(
            arrived, before,
            "stack {stack_id}'s members landed under each other's titles",
        );
    }
}

#[hegel::test]
fn a_tab_with_stacks_keeps_its_shape_at_a_size_it_was_never_arranged_for(tc: hegel::TestCase) {
    // The same promise as `a_tab_keeps_its_shape_at_a_size_it_was_never_arranged
    // _for`, asked of the arrangement a stack list travels as. Worth asking
    // separately: a stack's collapsed members are a fixed one row each while
    // everything around them is a percentage, so a receiving session of a
    // different height divides the space by a rule the unstacked case never
    // reaches - and dropping a member there would strand its process.
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let tiled = tc.draw(tab_arrangement_with_stack(viewport, false));
    assert!(tiled.iter().any(|pane| pane.geom.stacked.is_some()));

    let Ok(arrangement) = layout_for_warped_tab(&tiled, &[], viewport) else {
        return;
    };

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

// ---------------------------------------------------------------------------
// Tabs that are not a tree of splits
// ---------------------------------------------------------------------------
//
// `TiledPanes` keeps rectangles, not a tree, and nothing holds those rectangles
// to a shape a tree could have produced. The arrangement below is the one that
// sent us here: a tab whose ui strips span the screen while the panes between
// them stay inset - so the tab is a plus sign rather than a rectangle, and no
// split line spans it in either direction. Every one of those panes still has a
// process in it, so every one of them still has to travel.

fn manifest(x: usize, y: usize, cols: usize, rows: usize, run: Run) -> PaneLayoutManifest {
    // `cols`/`rows` are `Dimension`s and are set through `set_inner` rather than assigned, so the
    // struct literal covers x and y and the two dimensions follow.
    let mut geom = PaneGeom {
        x,
        y,
        ..Default::default()
    };
    geom.cols.set_inner(cols);
    geom.rows.set_inner(rows);
    PaneLayoutManifest {
        geom,
        run: Some(run),
        ..Default::default()
    }
}

fn shell() -> Run {
    Run::Command(RunCommand {
        command: "sh".into(),
        ..Default::default()
    })
}

fn ui_plugin() -> Run {
    Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
        location: yinz_runtime::plugin_location::parse_plugin_location("yinz:tab-bar", None)
            .unwrap(),
        ..Default::default()
    }))
}

/// A 100x40 screen whose ui strips span all 100 columns while the four
/// terminals between them have only the 80 to their right.
fn a_tab_whose_ui_strips_reach_past_its_panes() -> Vec<PaneLayoutManifest> {
    let strip = |y| {
        let mut pane = manifest(0, y, 100, 1, ui_plugin());
        pane.geom.rows = yinz_types::pane_size::Dimension::fixed(1);
        pane
    };
    vec![
        strip(0),
        manifest(20, 1, 40, 19, shell()),
        manifest(60, 1, 40, 19, shell()),
        manifest(20, 20, 40, 19, shell()),
        manifest(60, 20, 40, 19, shell()),
        strip(39),
    ]
}

#[test]
fn a_tab_that_is_not_a_tree_of_splits_travels_anyway() {
    let viewport = Size { cols: 80, rows: 40 };
    let tiled = a_tab_whose_ui_strips_reach_past_its_panes();

    // The premise: this is genuinely not a layout. If some later change makes it
    // one, this test stops testing what it was written for.
    assert!(
        get_tiled_panes_layout_from_panegeoms(&tiled, None).is_none(),
        "a tab whose strips reach past its panes is expected to be inexpressible as a layout",
    );

    let arrangement = layout_for_warped_tab(&tiled, &[], viewport)
        .expect("every tab can be warped, expressible as a layout or not");
    assert_eq!(
        arrangement.tiled_order.len(),
        4,
        "all four terminals should travel",
    );

    // And the receiving session has a slot waiting for each of them, plus the two
    // it starts afresh from the layout.
    let positioned = position_as_the_receiver_does(&arrangement.tiled_layout, viewport)
        .expect("an 80x40 terminal has room for six panes");
    assert_eq!(positioned.len(), 6);
    let slots = positioned
        .iter()
        .filter(|(layout, _)| !matches!(layout.run, Some(Run::Plugin(_))))
        .count();
    assert_eq!(slots, arrangement.tiled_order.len());
}

#[test]
fn an_approximated_tab_keeps_the_shape_it_can() {
    // Approximating is not licence to shuffle. The four terminals are a 2x2 grid
    // with two full-width strips over and under it - a shape a layout can hold
    // once the strips are pulled in - so each pane should land in its own
    // quadrant rather than in a neighbour's.
    let viewport = Size { cols: 80, rows: 40 };
    let tiled = a_tab_whose_ui_strips_reach_past_its_panes();
    let arrangement = layout_for_warped_tab(&tiled, &[], viewport).unwrap();

    let filled = slots_as_the_receiver_fills_them(&arrangement, &arrangement.tiled_order, viewport)
        .expect("an 80x40 terminal has room for six panes");
    assert_eq!(filled.len(), 4);
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
fn any_arrangement_at_all_can_be_warped(tc: hegel::TestCase) {
    // The promise in its widest form, asked of rectangles that owe nothing to
    // each other: overlapping, leaving gaps, in no order. A tab should never be
    // stuck in a session because of the shape it is in, and no shape is refused
    // here - what is checked is that every pane travels exactly once, which is
    // the half that cannot be approximated.
    let viewport = Size {
        cols: 100,
        rows: 40,
    };
    let panes = tc.draw(generators::integers::<usize>().min_value(1).max_value(8));
    let tiled: Vec<PaneLayoutManifest> = (0..panes)
        .map(|_| {
            let x = tc.draw(generators::integers::<usize>().min_value(0).max_value(90));
            let y = tc.draw(generators::integers::<usize>().min_value(0).max_value(35));
            let cols = tc.draw(
                generators::integers::<usize>()
                    .min_value(1)
                    .max_value(100 - x),
            );
            let rows = tc.draw(
                generators::integers::<usize>()
                    .min_value(1)
                    .max_value(40 - y),
            );
            manifest(x, y, cols, rows, shell())
        })
        .collect();

    let arrangement = layout_for_warped_tab(&tiled, &[], viewport)
        .expect("no arrangement is a reason to refuse a warp");
    let mut seen = arrangement.tiled_order.clone();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen, (0..panes).collect::<Vec<_>>());

    // And the layout it travels as has a slot per pane wherever it lands, not
    // only in the session it left.
    let elsewhere = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    if let Some(positioned) = position_as_the_receiver_does(&arrangement.tiled_layout, elsewhere) {
        assert_eq!(positioned.len(), panes);
    }
}

#[hegel::test]
fn widening_a_tabs_ui_strips_does_not_strand_a_pane(tc: hegel::TestCase) {
    // The same wrinkle, put into arrangements of every shape: the topmost and
    // bottommost full-width panes reach further out than the panes between them.
    let viewport = Size {
        cols: tc.draw(generators::integers::<usize>().min_value(20).max_value(400)),
        rows: tc.draw(generators::integers::<usize>().min_value(10).max_value(200)),
    };
    let mut tiled = tc.draw(tab_arrangement(viewport, true));
    tc.assume(!tiled.is_empty());
    let widened = tc.draw(generators::integers::<usize>().min_value(1).max_value(20));

    let bottom = tiled
        .iter()
        .map(|pane| pane.geom.y + pane.geom.rows.as_usize())
        .max()
        .unwrap_or(0);
    for pane in tiled.iter_mut() {
        let touches_edge = pane.geom.y == 0 || pane.geom.y + pane.geom.rows.as_usize() == bottom;
        if touches_edge && pane.geom.cols.as_usize() == viewport.cols {
            let cols = pane.geom.cols.as_usize() + widened;
            pane.geom.cols.set_inner(cols);
        }
    }

    let arrangement = layout_for_warped_tab(&tiled, &[], viewport)
        .expect("a widened ui strip is not a reason to refuse a warp");
    let expected: Vec<usize> = tiled
        .iter()
        .enumerate()
        .filter(|(_, pane)| !is_plugin_manifest(pane))
        .map(|(index, _)| index)
        .collect();
    let mut seen = arrangement.tiled_order.clone();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen, expected);
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
            location: yinz_runtime::plugin_location::parse_plugin_location("yinz:tab-bar", None)
                .unwrap(),
            ..Default::default()
        }))),
        ..Default::default()
    }];
    let arrangement = layout_for_warped_tab(&tiled, &[], viewport).unwrap();
    assert!(arrangement.tiled_order.is_empty());
    assert!(matches!(arrangement.tiled_layout.run, Some(Run::Plugin(_))));
}
