//! Turning a live tab into something another session can rebuild.
//!
//! A tab's arrangement is not stored anywhere as a tree — `TiledPanes` keeps
//! only a flat map of panes to geometry, and the tree the tab was built from is
//! discarded once it has been applied. So moving a tab means reconstructing a
//! layout from geometry, which [`crate::session_serialization`] already does for
//! session resurrection. Reusing it is what makes a warped tab fit a session of
//! a different size: a layout is proportional, and the receiving session
//! positions it against its own viewport.
//!
//! Not every arrangement is a layout, though. A layout is a tree of splits and
//! `TiledPanes` is a flat map of rectangles, so a tab can be in shapes no tree
//! describes — a tab whose ui strips reach past the panes between them is not
//! even a rectangle. Those are approximated rather than refused
//! (`tiled_layout_from_panegeoms_lossy`): every pane travels, and the
//! arrangement is the nearest one a layout can hold. A warp moves running
//! processes, and losing the lot of them to keep a border in place is the wrong
//! trade.
//!
//! What this module adds on top is an *order*. The receiving side fills the
//! layout's terminal slots positionally — `position_new_panes` walks the
//! flattened layout and pops the head of a queue of terminal ids for every slot
//! that is not a plugin. So the panes have to travel in exactly that order, or
//! they arrive in each other's places. Working that order out is what most of
//! this file is for.

use std::path::PathBuf;

use zellij_types::geometry::PercentOrFixed;
use zellij_types::pane_size::Size;
use {
    crate::session_serialization::get_floating_panes_layout_from_panegeoms,
    crate::session_serialization::tiled_layout_from_panegeoms_lossy,
    zellij_types::session_manifest::PaneLayoutManifest,
};
use {
    zellij_types::input::layout::FloatingPaneLayout, zellij_types::input::layout::Run,
    zellij_types::input::layout::TiledPaneLayout,
};

/// A tab's arrangement, plus which pane belongs in each of its terminal slots.
///
/// The orders are indices back into the manifest slices handed to
/// [`layout_for_warped_tab`], so the caller can pair them with whatever it is
/// holding alongside each manifest — a `PaneId`, a released supervisor — without
/// this module having to know about any of it.
#[derive(Debug, Clone, PartialEq)]
pub struct WarpedTabLayout {
    /// All layout slots, including plugins, for transfer-local relationship references.
    pub all_tiled_order: Vec<usize>,
    pub tiled_layout: TiledPaneLayout,
    pub floating_layout: Vec<FloatingPaneLayout>,
    /// Indices of the tiled manifests, in the order the receiving session will
    /// fill tiled terminal slots. Plugin panes are not in here: they have no
    /// process to hand over and are rebuilt from their `Run::Plugin` node.
    pub tiled_order: Vec<usize>,
    /// Indices of the floating manifests, same idea. Floating slots are filled
    /// in plain list order, so this is just the non-plugin indices in sequence.
    pub floating_order: Vec<usize>,
}

/// A `Run` no real pane could have, used to tell otherwise identical panes apart.
///
/// The ordering problem is that two panes running the same command are
/// indistinguishable by their `Run`, so reading the order back off a layout
/// needs something unique per pane to read. This is that: a marker built from
/// the pane's index, stamped into a throwaway copy of the layout purely to be
/// read back out. It never reaches the wire, and never reaches a real layout —
/// see [`layout_for_warped_tab`], which builds the real one separately.
///
/// The NUL prefix is what makes it safe rather than merely unlikely: a path
/// containing one cannot be passed to any syscall, so no genuine `Run::Cwd`
/// can collide with it.
const SLOT_MARKER: &str = "\0zellij-warp-slot/";

fn slot_marker(index: usize) -> Run {
    Run::Cwd(PathBuf::from(format!("{SLOT_MARKER}{index}")))
}

fn slot_marker_index(run: &Option<Run>) -> Option<usize> {
    match run {
        Some(Run::Cwd(path)) => path.to_str()?.strip_prefix(SLOT_MARKER)?.parse().ok(),
        _ => None,
    }
}

fn is_plugin(manifest: &PaneLayoutManifest) -> bool {
    matches!(manifest.run, Some(Run::Plugin(_)))
}

/// Build a tab's layout and work out which pane fills each of its slots.
///
/// `viewport` is the size the tab is being taken *from*, needed to express
/// floating panes as fractions of it rather than as the cell counts they
/// currently occupy.
pub fn layout_for_warped_tab(
    tiled: &[PaneLayoutManifest],
    floating: &[PaneLayoutManifest],
    viewport: Size,
) -> Result<WarpedTabLayout, String> {
    let all_tiled_order = tiled_slot_order(tiled)?;
    let tiled_order = all_tiled_order
        .iter()
        .copied()
        .filter(|i| !is_plugin(&tiled[*i]))
        .collect();
    let tiled_layout = tiled_layout_from_panegeoms_lossy(&tiled.to_vec());

    let mut floating_layout = get_floating_panes_layout_from_panegeoms(&floating.to_vec());
    scale_floating_to_viewport(&mut floating_layout, viewport);
    let floating_order = floating
        .iter()
        .enumerate()
        .filter(|(_, manifest)| !is_plugin(manifest))
        .map(|(index, _)| index)
        .collect();

    Ok(WarpedTabLayout {
        all_tiled_order,
        tiled_layout,
        floating_layout,
        tiled_order,
        floating_order,
    })
}

/// Read the order the receiving session will fill tiled terminal slots in.
///
/// Done by building a throwaway layout whose every terminal leaf carries a
/// marker, then asking that layout for its run instructions — because
/// `extract_run_instructions` is defined to return them in the same order
/// `position_panes_in_space` flattens the tree, which is the order the ids are
/// consumed in. Deriving the order from the layout rather than assuming the two
/// traversals agree is the point: if they ever stop agreeing, this notices
/// instead of silently swapping two panes.
fn tiled_slot_order(tiled: &[PaneLayoutManifest]) -> Result<Vec<usize>, String> {
    // There is no layout that describes no panes: the smallest one describes a
    // single empty pane, so an empty tab would arrive with one slot the queue
    // has nothing to fill. Refused rather than reconstructed, because a tab with
    // nothing tiled in it is not something this can round-trip honestly.
    if tiled.is_empty() {
        return Err("this tab has no tiled panes, so there is no arrangement to move".to_owned());
    }

    let marked: Vec<PaneLayoutManifest> = tiled
        .iter()
        .enumerate()
        .map(|(index, manifest)| {
            let mut marked = manifest.clone();
            {
                marked.run = Some(slot_marker(index));
                // A cwd would be folded into the run and blur the marker.
                marked.cwd = None;
            }
            marked
        })
        .collect();

    // The same builder the real layout goes through, on the same geometry: the
    // markers change what is *in* the leaves, never where they are, so both take
    // the same branch and this order describes the layout that travels.
    let probe = tiled_layout_from_panegeoms_lossy(&marked);
    let order: Vec<usize> = probe
        .extract_run_instructions()
        .iter()
        .filter_map(slot_marker_index)
        .collect();

    // Every terminal pane must appear exactly once. A layout that dropped one
    // would land the remaining panes in the wrong slots, and a layout that
    // repeated one would put the same supervisor in two places — both silent
    // once the tab has arrived, so both are checked here rather than assumed.
    //
    // Not expected to fire: `tiled_layout_from_panegeoms_lossy` gives every pane
    // a leaf whatever the arrangement looks like, so this failing means the
    // builder has lost that property, and refusing the warp is the safe answer
    // to that. It is not a shape of tab the user can be in — nothing they could
    // rearrange makes it happen — so there is nothing to tell them to undo.
    let expected = tiled.len();
    let mut seen: Vec<usize> = order.clone();
    seen.sort_unstable();
    seen.dedup();
    if order.len() != expected || seen.len() != expected {
        return Err(format!(
            "this tab's arrangement could not be expressed as a layout: {} of its {expected} \
             panes could be placed",
            seen.len()
        ));
    }
    Ok(order)
}

/// Re-express floating panes as fractions of the viewport they were floating in.
///
/// `get_floating_panes_layout_from_panegeoms` emits cell counts, which is right
/// for session resurrection — a session comes back the size it left. A warp has
/// no such promise, and a pane at `x=150` arriving in an 80-column session would
/// be shoved back on screen with its arrangement lost. Percentages survive the
/// trip instead.
///
/// Only `Fixed` values are touched. A pane already sized as a percentage was
/// asked for in the terms we want, and rescaling it would be rounding the same
/// intent twice.
fn scale_floating_to_viewport(layout: &mut [FloatingPaneLayout], viewport: Size) {
    if viewport.cols == 0 || viewport.rows == 0 {
        return;
    }
    // `floor` is what keeps a pane on screen: rounding a position up can push it
    // past the edge, and rounding a size up can make it overflow the viewport.
    // The one exception is a size that would round away to nothing.
    let scale = |value: &mut Option<PercentOrFixed>, whole: usize, floor: usize| {
        if let Some(PercentOrFixed::Fixed(fixed)) = value {
            let percent = (*fixed * 100 / whole).clamp(floor, 100);
            *value = Some(PercentOrFixed::Percent(percent));
        }
    };

    for pane in layout.iter_mut() {
        scale(&mut pane.x, viewport.cols, 0);
        scale(&mut pane.y, viewport.rows, 0);
        scale(&mut pane.width, viewport.cols, 1);
        scale(&mut pane.height, viewport.rows, 1);
    }
}

#[cfg(test)]
mod test;
