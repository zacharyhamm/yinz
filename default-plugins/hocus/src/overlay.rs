//! Where hocus draws itself: a panel in the bottom-left corner of the screen.
//!
//! hocus is an **overlay**: drawn over the session surface rather than inside a tab's layout, so
//! a tab keeps its full width and hocus floats above whatever is underneath. `hocus-agents` is a
//! second overlay, the same size, and the two sit **side by side**: the conversation on the left,
//! the agent list to its right.
//!
//! **They are two panes, not one column.** Either can be open without the other, either hides
//! on its own, and neither opens the other unbidden. The one thing that couples them is the
//! left-hand slot: the conversation owns it, so the list sits one padding to the right of it
//! while the conversation is on screen and slides back to `pad` when it is not. hocus is the half
//! that knows both, so hocus does the arithmetic for both.
//!
//! **The plugin computes the geometry, not the server.** `FloatingPaneCoordinates` measures `x`
//! and `y` from the top-left corner only - there is no bottom or right anchoring - so "the bottom
//! left corner, inset by a padding" cannot be stated as a coordinate. What can be stated is the
//! arithmetic, once the display area is known: `y = rows - height - pad`. That is what this module
//! does, on load and again whenever the display area changes, exactly as compact-bar positions its
//! tooltip. The alternative was teaching the server to anchor, which is a protobuf change to
//! `FloatingPaneCoordinates` and a second denominator to keep in step.
//!
//! **A pane is not painted until it has been placed.** An overlay that named no coordinates is
//! born half the surface, *centered* (`OverlayPanes::base_geom`) - which, for two panels that
//! belong in the same corner, is squarely on top of each other. Only the arithmetic below moves
//! them out of there, and it cannot run until the pane is on a tab for the placement to find, so
//! the launch used to be visible as a flash in the middle of the screen. Both halves are launched
//! `awaits_placement true` (`hocus_proto::launch::overlay_configuration`, and the `a`/`c`
//! keybindings), which tells the server to hold the paint until the first
//! `change_overlay_coordinates` names the pane. `apply_overlay_geometry` is that call, and
//! `State::placed` / `State::agents_placed` are what keep making it until it lands.
//!
//! **The overlays live on a tab, and hocus moves them.** An overlay belongs to a `Tab`, so
//! `on_tabs` moves whichever halves are on screen to the tab that became active - which is what
//! keeps hocus present wherever the user goes. This is
//! also why the server routes events to overlays in *every* tab and not only the active one -
//! without that, an overlay left behind on another tab would never hear the `TabUpdate` it moves
//! on.
//!
//! **The corner is the default, not the only place.** Ctrl-o (`State::reposition`) moves the
//! conversation *clear of* a tiled pane - whichever one it is covering, or the largest one the
//! user is not working in, by `pick_anchor` - and remembers that pane as `State::anchor`. The
//! press puts the conversation in the corner that clears that pane best (`corner_for`), at
//! exactly the size the corner placement would have given it. The list is not part of this; it
//! keeps its corner arithmetic as if the conversation were still there.
//!
//! **Nothing the layout does moves either panel.** A placement is two steps - *choose* a corner,
//! then *resolve* it against the surface (`corner_for`, then `rect_in_corner`) - and only the
//! first reads the layout. The corner a press chose is remembered (`State::placed_corner`), so
//! every later placement is a function of that corner, the screen and the config: a pane opening,
//! a pane closing, a re-tile and a swap layout are not inputs to it. `track_anchor` still reads
//! every `PaneUpdate`, but only to keep the anchor the *next* press ranks against - it places
//! nothing. What places is a press, a hide, a reveal, a config change, and a terminal resize,
//! which resolves the same corner against the screen's new size.
//!
//! The config is where the *size* comes from, which is the one thing a placement does not carry
//! over: a press threads the size the manifest reports (`own_rect`, then `reposition`), so it
//! keeps a pane the user resized, but every other placement passes `None` and gets `panel_size`'s
//! configured size back.
//!
//! **A press always moves the conversation.** Every placement is one of four corners
//! (`Corner`), so a repeat press is a walk: `pick_anchor` tours the panes nobody has been sent
//! clear of yet (`State::warp_visited`), and once they are all toured the press steps to the next
//! corner of the anchor's ordering (`State::warp_step`) instead of doing nothing. Four corners,
//! then the tour re-opens. The only press that moves nothing is one on a panel the size of the
//! screen, where all four corners are the same rectangle.
//!
//! **A pane is never smaller than `MIN_PANE_COLS` by `MIN_PANE_ROWS`, and never off the screen.**
//! That is a guarantee of this module rather than a hope: `panel_size` applies the floor after the
//! room the padding leaves and before the hard edge of the surface, `pad_of` gives the padding up
//! before the panes do, and `corner_rect` is the one place a rectangle is put on the screen, so
//! every path is clamped the same way. On a surface too small even for the floor the pane gets
//! the whole of what there is and `ui::render` drops chrome to fit - see `ui::chrome_for`. Nothing
//! here ever asks for a rectangle the server would have to clip back
//! (`PaneGeom::adjust_coordinates` does that silently), so the rectangle hocus remembers is the
//! rectangle it got.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use zellij_tile::prelude::*;

use crate::ui;
use crate::State;

/// Cells of clear screen left around the panel on every side, and between the two panels.
const DEFAULT_PAD: usize = 3;
/// One pane's width, as a percentage of the display area. Under a third of the screen, so a pane
/// reads as a panel beside the work rather than a split of it, and wide enough that a chat message
/// is not wrapped every other word.
const DEFAULT_WIDTH_PERCENT: usize = 30;
/// A pane's height, as a percentage of the display area. Two thirds: taller than it is wide, and
/// short enough that the top of the screen stays the user's own.
const DEFAULT_HEIGHT_PERCENT: usize = 66;
/// The fewest columns a pane may be given.
///
/// An overlay is framed, so the plugin is rendered two cells narrower and two shorter than the
/// pane it is in: the pane's minimum is what `ui::render` needs plus that frame. With
/// `pane_frames false` the two cells are slack rather than a deficit, which costs a slightly
/// roomier panel and nothing else.
const MIN_PANE_COLS: usize = ui::MIN_COLS + 2;
/// The fewest rows a pane may be given - `ui::MIN_ROWS` and the frame around it. This is a real
/// minimum: the title, a transcript row, the input box and the rules around it.
const MIN_PANE_ROWS: usize = ui::MIN_ROWS + 2;

/// A configured size, resolved against `whole`. Accepts `"40%"` and `"12"`, and falls back to
/// `default_percent` of `whole` for anything missing or unparseable - a typo in the KDL costs a
/// default-sized column rather than a crash.
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

/// The two rectangles, in the order `(chat, agents)`.
///
/// Both are the same size and share a `y`: they sit beside each other along the bottom of the
/// surface, inset by `pad` and separated by another `pad`. The conversation owns the left-hand
/// slot, so `chat_visible` is what decides where the agent list goes - beside the conversation at
/// `pad + width + pad`, or in its place at `pad` when there is no conversation on screen to sit
/// beside.
///
/// `None` before any `TabUpdate` has said how big the screen is - there is nothing to compute
/// against yet, and a guess would put the panes somewhere the user would see them move away from.
pub(crate) fn overlay_geometry(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    chat_visible: bool,
) -> Option<(FloatingPaneCoordinates, FloatingPaneCoordinates)> {
    let (chat, agents) = overlay_rects(config, surface_rows, surface_cols, chat_visible)?;
    // Fixed rather than percentages: these are already resolved against the surface we were told
    // about, and a percentage would be resolved a second time by the server. The two denominators
    // would agree now that `surface_of` reads the session surface, but stating the arithmetic once
    // is still the point - the panes are anchored to the *bottom*, which no percentage can say.
    Some((fixed_rect(chat), fixed_rect(agents)))
}

/// The same two rectangles, before they are put into the coordinates the host takes. Rectangles
/// all the way in, coordinates only at the edge - see [`fixed_rect`].
fn overlay_rects(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    chat_visible: bool,
) -> Option<(Rect, Rect)> {
    let chat = chat_placement(
        config,
        surface_rows,
        surface_cols,
        chat_visible,
        None,
        false,
        0,
    )?;
    let pad = pad_of(config, surface_rows, surface_cols, chat_visible);
    let agents_x = match chat_visible {
        // One padding of clear screen between the two, the same padding that insets the pair from
        // the edges of the screen. They used to meet frame to frame; two framed panels touching
        // read as one panel with a line down it, and the gap is what makes them two things sitting
        // beside each other. One value rather than a second key, so a `pad` the user changes moves
        // the panels and the seam together.
        //
        // The `min` is the same clamp `corner_rect` does, for a slot rather than a corner: on a
        // surface too narrow for two panes and three paddings the list slides left against the
        // right-hand edge rather than off it. `panel_size` keeps the pair inside the screen, so
        // this is a guard rather than a routine path.
        true => (pad + chat.cols + pad).min(surface_cols.saturating_sub(chat.cols)),
        false => chat.x,
    };
    Some((
        chat,
        Rect {
            x: agents_x,
            ..chat
        },
    ))
}

/// One panel's `(width, height)`, resolved against the surface and clamped to it.
///
/// The size half of *every* placement: moving the conversation clear of a pane, or one corner
/// along, must never resize it, so there is one answer to the size rather than a copy per path.
///
/// **The minimum is a floor, not a preference.** See [`fit`] for the order that makes it one, and
/// [`min_pane_cols`] for what happens on a screen too narrow to honour it.
fn panel_size(
    config: &BTreeMap<String, String>,
    rows: usize,
    cols: usize,
    chat_visible: bool,
) -> (usize, usize) {
    let pad = pad_of(config, rows, cols, chat_visible);
    let (configured_width, configured_height) = configured_size(config, rows, cols);
    // What is between the paddings, and how much of it one pane may take. With the conversation
    // on screen the two panes and the gap between them are laid end to end, so neither may exceed
    // half of what is left once the gap is taken off - a `width` wide enough for one is not wide
    // enough for two, and the list would otherwise run off the right-hand edge. Alone, a pane may
    // take the whole of it: there is no gap to leave room for.
    let across = cols.saturating_sub(pad * 2);
    let each = match chat_visible {
        true => across.saturating_sub(pad) / 2,
        false => across,
    };
    let width = fit(
        configured_width,
        min_pane_cols(cols, chat_visible),
        each,
        share_of_cols(cols, chat_visible),
    );
    // What is left of the screen once the padding is taken off the top and the bottom, and the
    // share of it a pane actually takes.
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

/// A configured size resolved against one axis: what was asked for, capped at the room the padding
/// leaves, raised to the floor, and capped again at the hard edge of the screen.
///
/// **The order is the guarantee.** Taking the room first and the floor after it is what makes the
/// minimum a minimum: the old arithmetic clamped to the room *last*, so on a narrow screen the
/// room won and the pane came back below a size anything could be drawn in. The screen is the only
/// thing that beats the floor, and only because there is nothing else to give.
fn fit(configured: usize, min: usize, room: usize, whole: usize) -> usize {
    configured.min(room).max(min).min(whole)
}

/// The columns one pane may have of the screen: half of it with both panes on screen, since they
/// sit side by side, and the whole of it alone.
///
/// Never zero. A pane with no width is not placed at all - `FloatingPaneCoordinates::new` rejects
/// it - and what the host draws instead is half the surface, centered.
fn share_of_cols(cols: usize, chat_visible: bool) -> usize {
    match chat_visible {
        true => cols / 2,
        false => cols,
    }
    .max(1)
}

/// The fewest columns a pane is given here: [`MIN_PANE_COLS`], or the share of the screen it may
/// have when that is less.
///
/// Below `2 * MIN_PANE_COLS` two panes cannot both have the minimum. Splitting what there is
/// evenly is better than overlapping them or pushing one off the screen, and `ui::render` covers
/// the difference by dropping chrome rather than refusing to draw.
fn min_pane_cols(cols: usize, chat_visible: bool) -> usize {
    MIN_PANE_COLS.min(share_of_cols(cols, chat_visible))
}

/// The padding, resolved against a surface of `rows` by `cols`.
///
/// Capped at a quarter of the surface on *both* axes, so a fat `pad` on a small terminal cannot
/// eat the panes it is supposed to be insetting. Capping against the rows alone was not enough:
/// `pad` is the rectangles' `x` as well as their inset off the top and bottom, so on a tall,
/// narrow display a rows-only cap let it exceed the column count outright and put the panes off
/// the right-hand edge of the screen.
///
/// **The padding gives way before the panes do.** A quarter of the screen is still too much when
/// what is left cannot hold a pane anybody could read, so the cap also asks what the surface has
/// to fit: the minimum on each axis, twice over across the columns when both panes are on screen,
/// plus the gap between them. A thinner inset is a smaller loss than an unusable pane, and this is
/// what keeps the two halves side by side and on screen down to surfaces of a few dozen columns.
fn pad_of(
    config: &BTreeMap<String, String>,
    rows: usize,
    cols: usize,
    chat_visible: bool,
) -> usize {
    config
        .get("pad")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_PAD)
        .min(rows / 4)
        .min(cols / 4)
        // one pane's minimum, with a padding above and below it
        .min(rows.saturating_sub(MIN_PANE_ROWS) / 2)
        .min(match chat_visible {
            // two panes' minimum, with a padding at each edge and one between them: three across
            true => cols.saturating_sub(2 * min_pane_cols(cols, true)) / 3,
            false => cols.saturating_sub(min_pane_cols(cols, false)) / 2,
        })
}

/// `(width, height)` as configured, resolved against the surface and not yet clamped to anything.
fn configured_size(config: &BTreeMap<String, String>, rows: usize, cols: usize) -> (usize, usize) {
    (
        resolve(config.get("width"), cols, DEFAULT_WIDTH_PERCENT),
        resolve(config.get("height"), rows, DEFAULT_HEIGHT_PERCENT),
    )
}

/// Coordinates stated as fixed cells - see the note in `overlay_geometry` on why never
/// percentages.
///
/// The edge of the module: everything above works in [`Rect`], and converts once, here. That is
/// what lets a placement remember the rectangle it asked for without parsing a `PercentOrFixed`
/// back out of the coordinates it sent.
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

/// An absolute rectangle on the screen, as a `PaneInfo` reports one.
///
/// Tiled panes and overlays are reported in the same coordinates - every rect comes off the
/// pane's own geometry, and an overlay's surface is the display area at the screen origin - so
/// the conversation's own manifest entry can be compared against the tiled panes beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: usize,
    pub y: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Rect {
    pub(crate) fn of(pane: &PaneInfo) -> Rect {
        Rect {
            x: pane.pane_x,
            y: pane.pane_y,
            cols: pane.pane_columns,
            rows: pane.pane_rows,
        }
    }

    fn area(&self) -> usize {
        self.cols * self.rows
    }

    /// Cells the two rectangles share.
    fn overlap(&self, other: &Rect) -> usize {
        let across = (self.x + self.cols)
            .min(other.x + other.cols)
            .saturating_sub(self.x.max(other.x));
        let down = (self.y + self.rows)
            .min(other.y + other.rows)
            .saturating_sub(self.y.max(other.y));
        across * down
    }
}

/// A corner of the surface the conversation may sit in.
///
/// Every placement is one of these four - the default corner, the placement that gets clear of an
/// anchor, and every step of the cycle a repeat Ctrl-o walks - so naming them is what lets a press
/// ask for "the next place" rather than restate the arithmetic. It is also what lets a placement
/// be *remembered* rather than re-derived: see [`State::placed_corner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Corner {
    BottomLeft,
    BottomRight,
    TopRight,
    TopLeft,
}

/// Every corner, in the order a press walks them: the home corner, then across, then up, then
/// back - once round the perimeter, so no step is a diagonal jump across the screen.
///
/// This order is also the tie-break in [`corner_order`], and it is the one the clear placement
/// has always had: the bottom row before the top, and the near column before the far one.
const CORNERS: [Corner; 4] = [
    Corner::BottomLeft,
    Corner::BottomRight,
    Corner::TopRight,
    Corner::TopLeft,
];

/// The order for a conversation that has the tab to itself - the far corner from the default
/// first, and then the rest of the perimeter. See [`corner_order`].
const CORNERS_ALONE: [Corner; 4] = [
    Corner::TopRight,
    Corner::TopLeft,
    Corner::BottomLeft,
    Corner::BottomRight,
];

/// The rectangle `corner` names, given the surface, the padding and the panel's `(width, height)`.
///
/// **The one place `x` and `y` are computed, and the one place they are clamped onto the screen.**
/// The clamp is what makes the minimum in [`panel_size`] a guarantee: a rectangle that ran off an
/// edge would be pulled back by the server (`PaneGeom::adjust_coordinates`) without telling
/// anybody, and what hocus remembered placing would not be what it got.
fn corner_rect(
    corner: Corner,
    surface_rows: usize,
    surface_cols: usize,
    pad: usize,
    size: (usize, usize),
) -> Rect {
    let (width, height) = size;
    let (left, top) = (pad, pad);
    // `max` is the degenerate guard: a far edge that underflowed would sit on top of the near one
    // rather than opposite it.
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

/// The four corners, best first, for a conversation asked to get clear of `anchor`.
///
/// The first is where the placement goes; the rest are where a repeat press walks, in order.
///
/// - No anchor - the conversation is in the corner and has been asked of nothing - is [`CORNERS`],
///   the plain perimeter starting at home.
/// - `alone`, one tiled pane on the whole tab, is [`CORNERS_ALONE`]. There is no other side of the
///   screen to be on, so the rule is "the far corner from the default" rather than anything
///   computed: every corner would tie against a pane that fills the tab.
/// - Otherwise the corners are *sorted*, by the column the pane rules out and then by how much of
///   it each corner covers. A pane whose centre is left of the screen's rules out the left-hand
///   column and one whose centre is right of it rules out the right; a centred pane rules out
///   neither. Sorting rather than filtering is what keeps the ruled-out column reachable as a last
///   resort, which a full cycle needs; the first element is what filtering would have chosen,
///   because the key order and the tie-break order are the same.
///
/// The vertical slot is chosen by overlap even when the horizontal one was forced, which is what
/// answers a pane too wide for either side to clear outright: it is put where it hides the least
/// of it. Covering *other* panes is not this function's problem - only the pane the user asked to
/// see is.
fn corner_order(
    anchor: Option<Rect>,
    alone: bool,
    surface_rows: usize,
    surface_cols: usize,
    pad: usize,
    size: (usize, usize),
) -> [Corner; 4] {
    let Some(anchor) = anchor else {
        return CORNERS;
    };
    if alone {
        return CORNERS_ALONE;
    }
    // Doubled rather than halved, so a pane an odd number of columns wide is not rounded onto one
    // side of a screen it is centred on.
    let anchor_centre = 2 * anchor.x + anchor.cols;
    let wrong_side = |corner: &Corner| {
        let on_the_left = matches!(corner, Corner::BottomLeft | Corner::TopLeft);
        usize::from(match anchor_centre.cmp(&surface_cols) {
            std::cmp::Ordering::Less => on_the_left,
            std::cmp::Ordering::Greater => !on_the_left,
            std::cmp::Ordering::Equal => false,
        })
    };
    let mut order = CORNERS;
    // Stable, so corners that tie keep `CORNERS`' order - which is what makes that list the
    // tie-break rather than something restated here.
    order.sort_by_key(|corner| {
        (
            wrong_side(corner),
            anchor.overlap(&corner_rect(*corner, surface_rows, surface_cols, pad, size)),
        )
    });
    order
}

/// The id a manifest entry names.
pub(crate) fn pane_id_of(pane: &PaneInfo) -> PaneId {
    match pane.is_plugin {
        true => PaneId::Plugin(pane.id),
        false => PaneId::Terminal(pane.id),
    }
}

/// Whether a manifest entry is one of the panes the layout is made of.
///
/// Floating, suppressed and unselectable panes are not: a floating pane is not the layout (which
/// is what keeps hocus's own two overlays out of this), a suppressed one is not on screen, and an
/// unselectable one is chrome such as the status bar.
fn is_tiled(pane: &PaneInfo) -> bool {
    !pane.is_floating && !pane.is_suppressed && pane.is_selectable
}

/// Whether the tab holds exactly one tiled pane.
///
/// The one case the placement cannot answer with "the other side of the screen", because a single
/// pane has no other side - see [`corner_order`]. Read from the manifest at placement time
/// rather than remembered, so a second pane opening drops the rule on the next re-tile.
pub(crate) fn sole_tiled_pane(panes: &[PaneInfo]) -> bool {
    panes.iter().filter(|pane| is_tiled(pane)).count() == 1
}

/// The tiled pane the conversation should get clear of, given every pane on its tab.
///
/// The pane the user is working in is out: it is the one they are already looking at, and hocus
/// is asked to uncover something else. The exception is a tab with a single tiled pane, where the
/// focused pane is the only thing there is to be clear of - without it Ctrl-o would do nothing at
/// all on the commonest layout there is.
///
/// Of the rest, **the pane the conversation is covering most wins**, and the larger pane wins
/// among equals. That ranking is what makes a press walk pane to pane: the first press takes hocus
/// off the pane it was on, which lands it on some other pane, and the second press takes it off
/// that one. Where hocus covers nothing the overlaps are all zero and the ranking reads as "the
/// largest pane you are not in". The sort is stable, so a tie beyond that goes to manifest order.
///
/// **`visited` is what guarantees progress.** The ranking alone does not: it is a function of
/// rectangles, so on a tab where nothing overlaps - or on a second press before the manifest has
/// caught up - it answers the same pane again and the walk stops. Every pane already toured is out
/// of the running, so each press either reaches a pane nobody has been sent clear of yet or says
/// there are none left, and the caller walks the corners instead.
///
/// `None` when every candidate has been toured, or when there are none at all.
pub(crate) fn pick_anchor(
    own_rect: Rect,
    panes: &[PaneInfo],
    visited: &[PaneId],
) -> Option<PaneId> {
    let tiled: Vec<&PaneInfo> = panes.iter().filter(|pane| is_tiled(pane)).collect();
    let mut candidates: Vec<&PaneInfo> = match tiled.len() {
        1 => tiled,
        _ => tiled.into_iter().filter(|pane| !pane.is_focused).collect(),
    };
    candidates.retain(|pane| !visited.contains(&pane_id_of(pane)));
    candidates.sort_by_key(|pane| {
        let rect = Rect::of(pane);
        (Reverse(own_rect.overlap(&rect)), Reverse(rect.area()))
    });
    candidates.first().map(|pane| pane_id_of(pane))
}

/// Where the conversation goes: the size [`panel_size`] gives, at the corner `step` of the order
/// [`corner_order`] gives for `anchor`.
///
/// **The one placement path**, and it is two steps - [`corner_for`] chooses, [`rect_in_corner`]
/// resolves. `anchor: None` at `step: 0` is the default bottom-left corner, so the corner
/// placement, the placement that gets clear of a pane and every step of Ctrl-o's cycle are the
/// same arithmetic asked a different question - they cannot drift apart, and a size or a padding
/// the user configured means the same thing in all of them. The pane is never resized to suit the
/// anchor and neither is the anchor: Ctrl-o is a request to *see* a pane, and the whole job is to
/// stop covering it.
///
/// `step` is taken modulo the four corners, so a cycle that has walked past the end comes back to
/// the best one.
///
/// `None` when there is no surface to compute against, which leaves the caller with the placement
/// it already has. An anchor with no size is not an anchor at all and falls back to the corner.
pub(crate) fn chat_placement(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    chat_visible: bool,
    anchor: Option<Rect>,
    alone: bool,
    step: usize,
) -> Option<Rect> {
    let corner = corner_for(
        config,
        surface_rows,
        surface_cols,
        chat_visible,
        anchor,
        alone,
        step,
        None,
    )?;
    rect_in_corner(
        config,
        surface_rows,
        surface_cols,
        chat_visible,
        corner,
        None,
    )
}

/// The choosing half of a placement: which corner the conversation belongs in, given the pane it
/// has been asked to get clear of.
///
/// **The only half that reads the layout.** The anchor's rectangle and whether it has the tab to
/// itself are what rank the four corners, and they are read when a press asks and at no other
/// time. What a press produces is a `Corner`, which [`State::placed_corner`] remembers - so every
/// later placement goes straight to [`rect_in_corner`], and no manifest can move the panel.
fn corner_for(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    chat_visible: bool,
    anchor: Option<Rect>,
    alone: bool,
    step: usize,
    size: Option<(usize, usize)>,
) -> Option<Corner> {
    let (pad, size) = placement_terms(config, surface_rows, surface_cols, chat_visible, size)?;
    let anchor = anchor.filter(|anchor| anchor.rows > 0 && anchor.cols > 0);
    let order = corner_order(anchor, alone, surface_rows, surface_cols, pad, size);
    Some(order[step % CORNERS.len()])
}

/// The placing half: the rectangle `corner` names on the surface as it is now, at the size
/// [`panel_size`] gives unless the caller states one.
///
/// Reads the config and the screen and nothing else, which is the whole of the stability the
/// module header claims: with the corner remembered, no pane event is an input to where either
/// panel sits.
fn rect_in_corner(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    chat_visible: bool,
    corner: Corner,
    size: Option<(usize, usize)>,
) -> Option<Rect> {
    let (pad, size) = placement_terms(config, surface_rows, surface_cols, chat_visible, size)?;
    Some(corner_rect(corner, surface_rows, surface_cols, pad, size))
}

/// The padding and the panel size both halves of a placement resolve against, so the two cannot
/// disagree about either. `None` on a surface with no extent to put anything on.
fn placement_terms(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    chat_visible: bool,
    size: Option<(usize, usize)>,
) -> Option<(usize, (usize, usize))> {
    if surface_rows == 0 || surface_cols == 0 {
        return None;
    }
    let pad = pad_of(config, surface_rows, surface_cols, chat_visible);
    let size = size.unwrap_or_else(|| panel_size(config, surface_rows, surface_cols, chat_visible));
    Some((
        pad,
        (size.0.clamp(1, surface_cols), size.1.clamp(1, surface_rows)),
    ))
}

/// The surface the panes are placed against, off the active tab's `TabInfo`.
///
/// An overlay's coordinates are resolved against the whole session surface
/// (`OverlayPanes::surface`), which is the tab's display area - chrome included - at the screen
/// origin. That is the pair the arithmetic in `overlay_geometry` has to use.
pub(crate) fn surface_of(tab: &TabInfo) -> (usize, usize) {
    (tab.display_area_rows, tab.display_area_columns)
}

/// What to do about an agent list that is not on the conversation's tab.
///
/// See [`rejoin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rejoin {
    /// The list is where the user asked for it. Carry the conversation over to join it.
    Follow(usize),
    /// The list was left behind. Bring it back beside the conversation.
    Drag(usize),
}

/// Which way a parted pair should be put back together.
///
/// The two panes open and hide on their own, but they are still drawn as one panel and so are
/// still carried between tabs together. `follow_tab` moves the pair in one call and so cannot part
/// them; the `a` and `c` keybindings can, because `move_to_focused_tab` moves only the pane the
/// binding *names*. Which half moved is the whole question, and it is answered by which half the
/// server focused, because both bindings focus what they summon:
///
/// - `a` summons the **list** onto the summoning client's tab and focuses it. Dragging it back is
///   undoing what the user just asked for - the pane disappears in front of them. Follow it, and
///   the conversation arrives beside it if it is on screen at all.
/// - `c` summons the **conversation** and leaves the list behind, unfocused. Bring the list over.
///
/// `agents_tab_is_someones` is a corroborating guard rather than a second opinion: focus is
/// per-pane and outlives the client that set it, so a list that was focused and then abandoned on
/// a tab nobody is looking at must not be allowed to pull the conversation onto it. Read it from
/// `State::client_tabs`, which is per-client and therefore says where people actually are;
/// `State::tabs` is one client's `TabUpdate` and would answer for whoever was processed first.
///
/// Converges either way: both arms end with the pair on one tab, and the manifest that follows
/// reads them as together and asks for nothing.
pub(crate) fn rejoin(
    own_tab: usize,
    agents_tab: usize,
    agents_is_focused: bool,
    agents_tab_is_someones: bool,
) -> Option<Rejoin> {
    if own_tab == agents_tab {
        return None;
    }
    Some(if agents_is_focused && agents_tab_is_someones {
        Rejoin::Follow(agents_tab)
    } else {
        Rejoin::Drag(own_tab)
    })
}

/// The tab the panes should move to on this client's `TabUpdate`, if any.
///
/// **They move with the client they are sitting with, and with nobody else.** hocus is a single
/// shared instance, but the host builds one `TabUpdate` per client - `active` is *that* client's
/// active tab - and routes every one of them to every overlay. Obeying all of them is an unbounded
/// loop: with two clients on different tabs, one update drags the panes across, the move makes the
/// server report session state again, the other client's update drags them back, and so on for as
/// long as both stay attached.
///
/// So a client only gets to move us if they were on our tab to begin with. Three properties fall
/// out of that, and they are what make this the whole of the fix:
///
/// - A client on some other tab never matches, so they can never pull the panes off the client
///   who has them.
/// - The move is self-cancelling: `follow_tab` sets `own_tab_index` to where we landed, so the
///   report the move provokes re-evaluates to `None` rather than to another move.
/// - Authority transfers on its own. `move_to_focused_tab` on the `a` and `c` keybindings puts the
///   summoned pane on the summoning client's tab, at which point their entry matches and the
///   previous client's no longer does - no owner has to be recorded anywhere.
pub(crate) fn tab_to_follow(
    own_tab: Option<usize>,
    client_was_on: Option<usize>,
    client_is_on: usize,
) -> Option<usize> {
    let own = own_tab?;
    (client_was_on == Some(own) && client_is_on != own).then_some(client_is_on)
}

impl State {
    /// Re-place both panes against the display area as it is now.
    ///
    /// Safe to call whenever: `change_overlay_coordinates` names panes by id and does nothing for
    /// an id that is not an overlay, and once the user has dragged an overlay the server keeps
    /// their position and takes only the size from here.
    ///
    /// **Doing nothing is exactly what happens when the pane is not on a tab yet**, and it is
    /// silent - `Screen::change_overlay_coordinates` looks the id up and moves on. That is why
    /// `placed` is set from `own_tab_index` rather than from having made the call: the manifest
    /// saying where our pane is, is the only evidence the call could have landed. A placement that
    /// could not land leaves `placed` false, and `on_panes` tries again the moment it can.
    ///
    /// The agent list's `x` depends on whether *this* pane is on screen, so hiding and revealing
    /// the conversation both come back through here - that is what slides the list into the
    /// left-hand slot and back out of it.
    ///
    /// Both halves are tracked, and separately: see [`State::agents_placed`] for why one flag was
    /// not enough.
    pub(crate) fn apply_overlay_geometry(&mut self) {
        self.place(false, None, None);
    }

    /// The placement itself. `override_user_placement` is the one thing Ctrl-o adds: the server
    /// keeps a position the user dragged over anything a plugin asks for, and a key the user
    /// pressed to move it is theirs to override. A user-stated size remains independent.
    ///
    /// **An anchored conversation is placed clear of its pane; the list is placed as if it were
    /// not.** The list's slot is a function of whether the conversation is on screen, not of
    /// where it is, so the corner arithmetic answers for it unchanged - the user moved the
    /// conversation, and the list is a separate pane they did not ask to move.
    ///
    /// **`corner` is what a press has to say and nothing else does.** Every other caller passes
    /// `None`, which means "the corner we are already in" ([`State::placed_corner`]) - so a
    /// resize re-places the conversation where the last press left it, and a `PaneUpdate` cannot
    /// re-place it at all. `CORNERS[0]` is the fallback for the very first placement, which is
    /// the same bottom-left corner an unanchored order would have named at step 0.
    fn place(
        &mut self,
        override_user_placement: bool,
        size: Option<(usize, usize)>,
        corner: Option<Corner>,
    ) {
        let Some((_, agents)) = overlay_geometry(
            &self.kdl,
            self.surface_rows,
            self.surface_cols,
            !self.hidden,
        ) else {
            return;
        };
        let corner = corner.or(self.placed_corner).unwrap_or(CORNERS[0]);
        let Some(chat) = self.rect_in(corner, size) else {
            return;
        };
        self.placed_corner = Some(corner);
        // What the next press ranks against, and what tells it whether it has moved. Set on every
        // placement and not only Ctrl-o's, because a resize re-places and so moves us too.
        self.placed_rect = Some(chat);
        let mut placements = vec![(PaneId::Plugin(self.own_plugin_id), fixed_rect(chat))];
        if let Some(agents_id) = self.agents_plugin {
            placements.push((PaneId::Plugin(agents_id), agents));
        }
        #[cfg(test)]
        {
            self.last_placement = Some((placements.clone(), override_user_placement));
        }
        match override_user_placement {
            true => change_overlay_coordinates_overriding_user_placement(placements),
            false => change_overlay_coordinates(placements),
        }
        self.placed = self.own_tab_index.is_some();
        // The same test, pane for pane: the manifest naming a tab is the only evidence the call
        // could have landed. A list nobody has opened is nothing to place and so counts as placed,
        // which is what keeps `on_panes` and `on_tabs` from retrying on every event forever.
        self.agents_placed = self.agents_plugin.is_none() || self.agents_tab_index.is_some();
        #[cfg(test)]
        {
            self.placed_against = (self.surface_rows, self.surface_cols);
        }
    }

    /// The corner a press at `step` would choose, against the anchor and the tab as the last
    /// manifest reported them.
    ///
    /// Asked by [`State::reposition`] and by nothing else - a press is the only thing allowed to
    /// read the layout, which is what keeps a re-tile from moving the panel. `alone` is read from
    /// the manifest rather than remembered from the last press, so the single-pane rule is
    /// re-decided by each one.
    fn corner_at(&self, step: usize, size: Option<(usize, usize)>) -> Option<Corner> {
        corner_for(
            &self.kdl,
            self.surface_rows,
            self.surface_cols,
            !self.hidden,
            self.anchor.and(self.anchor_rect),
            sole_tiled_pane(&self.tab_panes),
            step,
            size,
        )
    }

    /// The conversation's rectangle in `corner`, optionally retaining its current size.
    fn rect_in(&self, corner: Corner, size: Option<(usize, usize)>) -> Option<Rect> {
        rect_in_corner(
            &self.kdl,
            self.surface_rows,
            self.surface_cols,
            !self.hidden,
            corner,
            size,
        )
    }

    /// Where the conversation is now: the rectangle the last placement asked for, else the
    /// manifest's word for it, else the corner it would be placed in.
    ///
    /// The last placement comes first because it is never stale. Two presses can arrive between
    /// two manifests, and ranking the second against the *first* press's rectangle is what stops
    /// them computing the same answer twice. A position the user **dragged** is not reflected here
    /// because the last placement wins, which costs a slightly worse choice of next pane and never
    /// a stalled press. The manifest's size is used separately so a move cannot undo a resize.
    fn own_rect(&self) -> Rect {
        let own_id = PaneId::Plugin(self.own_plugin_id);
        let reported = self
            .tab_panes
            .iter()
            .find(|pane| pane_id_of(pane) == own_id)
            .map(Rect::of);
        let mut own = self
            .placed_rect
            .or(reported)
            .or_else(|| {
                chat_placement(
                    &self.kdl,
                    self.surface_rows,
                    self.surface_cols,
                    !self.hidden,
                    None,
                    false,
                    0,
                )
            })
            .unwrap_or(Rect {
                x: 0,
                y: 0,
                cols: 0,
                rows: 0,
            });
        if let Some(reported) = reported {
            // The last placement wins for position so two quick presses cannot read the same
            // stale manifest, but the manifest owns size: it is how a user resize reaches us.
            own.cols = reported.cols;
            own.rows = reported.rows;
        }
        own
    }

    /// Ctrl-o: move the conversation somewhere else.
    ///
    /// **A press moves.** Where there is a tiled pane nobody has been sent clear of yet,
    /// [`pick_anchor`] names it and the conversation is placed at the corner that clears it best,
    /// as it always was. Where every pane has been toured - a tab of two panes with the other one
    /// focused is the commonest case there is - the press steps to the next corner of the same
    /// anchor's ordering instead of doing nothing, and once round the four the tour re-opens and
    /// the ranking starts again.
    ///
    /// Sent as the user's own placement, so it lands even on a conversation they dragged.
    ///
    /// The one press that moves nothing is one on a panel the size of the screen, where all four
    /// corners are the same rectangle and there is genuinely nowhere to go.
    pub(crate) fn reposition(&mut self) {
        let own = self.own_rect();
        let size = Some((own.cols, own.rows));
        match pick_anchor(own, &self.tab_panes, &self.warp_visited) {
            Some(next) => {
                self.anchor_rect = self
                    .tab_panes
                    .iter()
                    .find(|pane| pane_id_of(pane) == next)
                    .map(Rect::of);
                self.anchor = Some(next);
                self.warp_visited.push(next);
                // A fresh pane to be clear of: back to the corner that clears it best.
                self.warp_step = 0;
            },
            None => self.advance_warp(),
        }
        // The invariant, and the whole of the fix: where the step names the rectangle the
        // conversation is already at, take the next one. Once round at most.
        let mut target = None;
        for _ in 0..CORNERS.len() {
            let Some(corner) = self.corner_at(self.warp_step, size) else {
                // No surface to rank against, so there is no corner to move to either.
                break;
            };
            if self.rect_in(corner, size) != Some(own) {
                target = Some(corner);
                break;
            }
            self.advance_warp();
        }
        if let Some(corner) = target {
            self.place(true, size, Some(corner));
        }
    }

    /// Step one corner along, wrapping back to the best one.
    ///
    /// The wrap re-opens the tour: every pane is a candidate again, so the press after a full
    /// cycle goes back to walking pane to pane rather than round the corners forever.
    fn advance_warp(&mut self) {
        self.warp_step += 1;
        if self.warp_step >= CORNERS.len() {
            self.warp_step = 0;
            self.warp_visited.clear();
        }
    }

    /// Read the anchor off a fresh manifest of our tab: gone, moved, or as it was.
    ///
    /// **Bookkeeping for the next press, and not a placement.** The anchor is what Ctrl-o ranks
    /// the four corners against, so it has to keep up with the pane it names - but keeping up is
    /// all it does. Nothing here moves the conversation, because nothing the *layout* does is
    /// allowed to: a pane closing, opening or being re-tiled leaves both panels exactly where
    /// they are, and the corner they are in is remembered rather than re-derived (see
    /// [`State::placed_corner`] and the module header). Following a pane around used to be this
    /// function's job, and it is what made a pane close jump the panel across the screen - a
    /// re-tile could flip `sole_tiled_pane` and swap the whole corner ordering underneath it.
    ///
    /// A pane that is no longer a tiled pane on our tab - closed, floated, or left behind on the
    /// tab we moved from - releases the anchor: there is nothing left to be clear of, so the next
    /// press starts a fresh tour. The conversation stays put through that too.
    pub(crate) fn track_anchor(&mut self, panes: &[PaneInfo]) {
        // A pane that has closed cannot hold a slot in the tour: the press that came round to it
        // would find it gone and go nowhere new.
        self.warp_visited
            .retain(|id| panes.iter().any(|pane| pane_id_of(pane) == *id));
        let Some(anchor) = self.anchor else {
            return;
        };
        let now = panes
            .iter()
            .find(|pane| pane_id_of(pane) == anchor && !pane.is_floating && !pane.is_suppressed)
            .map(Rect::of);
        match now {
            None => {
                self.anchor = None;
                self.anchor_rect = None;
                // The cycle belongs to the anchor: with the pane gone there is nothing to be
                // clear of, and the next press starts a fresh tour. Safe to reset because the
                // conversation's position is `placed_corner` now, not `(anchor_rect, warp_step)`.
                self.warp_step = 0;
                self.warp_visited.clear();
            },
            // The rectangle a press ranks against, kept current. The step is kept too, so a press
            // after a re-tile carries on from where the cycle had got to.
            Some(rect) => self.anchor_rect = Some(rect),
        }
    }

    /// Put the conversation away without unloading it, leaving the agent list where it is.
    ///
    /// The two panes are separate surfaces, so Esc here means this pane and nothing else. What it
    /// does reach is the list's *position*: with the conversation gone the left-hand slot is free,
    /// and `apply_overlay_geometry` moves the list into it.
    pub(crate) fn hide_self_only(&mut self) {
        hide_self();
        self.hidden = true;
        self.hide_pending = true;
        // The bar's highlight swallows every key it does not use, with nothing
        // on screen to say so once the pane is hidden. Shift+Tab is easy to hit
        // on the way to the toggle key; the next reveal should type again.
        self.chat_tab_focus = None;
        self.apply_overlay_geometry();
    }

    /// Put the conversation back, having been revealed from outside - the `c` keybinding, or
    /// `show_self`.
    ///
    /// Only this pane is revealed: an agent list the user put away stays away. The list does move,
    /// though - the conversation is taking the left-hand slot back, so the list slides right.
    ///
    /// **No tab is followed here, and that is the whole of the correctness.** Both routes in have
    /// already put the pane and the client who asked for it on the same tab before hocus hears a
    /// word: `LaunchOrFocusPlugin { move_to_focused_tab }` extracts the overlay and re-adds it,
    /// unhidden, on the focused tab, and `show_self` goes through `FocusPluginPaneWithId`, which
    /// takes the *client* to the pane's tab. `on_panes` sets `own_tab_index` from the manifest
    /// immediately before calling this, so where we are is known rather than guessed.
    ///
    /// Guessing was a bug rather than a redundancy. The host builds one `PaneUpdate` per attached
    /// client and sends every one of them here, so `self.client` on the copy that happens to be
    /// processed first is an arbitrary client - not the one who pressed the key. Following *their*
    /// tab drags the freshly revealed pane off the summoner's screen.
    pub(crate) fn reveal_self(&mut self) {
        self.hidden = false;
        self.hide_pending = false;
        self.apply_overlay_geometry();
    }

    /// Carry whichever panes are ours to `tab_index`.
    ///
    /// Both in one call so they cannot end up on different tabs, and without focus - a tab switch
    /// is not a request to type into hocus.
    ///
    /// Called when *either* half is on screen, not only when this one is: a hidden conversation
    /// with a visible list still has to bring the list along, or a tab switch strands it.
    pub(crate) fn follow_tab(&mut self, tab_index: usize) {
        // Nothing to do until `PaneUpdate` has said where we actually are. Before that the panes
        // are wherever they were opened - which is the active tab, since every path that opens one
        // is the user asking for it from the tab they are on - and a move to "the active tab" would
        // be a move to where we already are. `on_panes` is the authority on this, not `on_tabs`:
        // one reports where our pane *is*, the other only which tab is in front.
        let Some(own_tab_index) = self.own_tab_index else {
            return;
        };
        if own_tab_index == tab_index {
            return;
        }
        let mut pane_ids = vec![PaneId::Plugin(self.own_plugin_id)];
        if let Some(agents_id) = self.agents_plugin {
            pane_ids.push(PaneId::Plugin(agents_id));
        }
        break_panes_to_tab_with_index(&pane_ids, tab_index, false);
        self.own_tab_index = Some(tab_index);
        // The anchor is a pane on the tab being left. The cycle goes with it - the panes it
        // toured are all on the tab we are leaving.
        self.anchor = None;
        self.anchor_rect = None;
        self.warp_step = 0;
        self.warp_visited.clear();
        if self.agents_plugin.is_some() {
            // Both ids went in one call, so the list is wherever we are. Recorded here rather than
            // waited for, so the next `PaneUpdate` - taken before the move, possibly - does not
            // read the two as parted and issue a second move.
            self.agents_tab_index = Some(tab_index);
        }
        // The move carries each overlay's exact coordinates and user-placement claims. Replacing
        // them here would undo the position and size the user had on the previous tab.
    }

    /// Ask an agent list that already exists to announce itself - and open nothing if there is
    /// none.
    ///
    /// The message carries no `plugin_url`, so it is delivered to whoever is listening and
    /// launches nobody. That is the whole of the point: the `a` keybinding is the one thing that
    /// opens a list, and a list the user did not ask for must not appear beside a conversation
    /// they did.
    ///
    /// It is still worth sending. A hocus that loaded after the list - a reload, a restore, the
    /// `a` keybinding having got here first - has no idea the list exists, and the list's plugin
    /// id is the one thing hocus needs to place it. `hocus-agents` answers `AGENT_STRIP_OPEN` with
    /// its selection, which carries that id.
    pub(crate) fn greet_agents_pane(&self) {
        pipe_message_to_plugin(MessageToPlugin::new(hocus_proto::wire::AGENT_STRIP_OPEN));
    }

    /// Take an `AGENT_STRIP_HELLO` and decide whether it is the thing that made us.
    ///
    /// The greeting carries hocus's url, so it opens a hocus when there is none to ask - and the
    /// user who opened a list asked for a list, not for a conversation. Nothing but that launch
    /// can reach us before our own first `PaneUpdate`, so `own_tab_index` being unset is the whole
    /// test; `from_agents` is the marker the greeting carries (`wire::OPENED_BY`), which keeps a
    /// greeting from some other sender out of it.
    ///
    /// Only remembered here. `on_panes` does the hiding, because a `hide_self` issued before our
    /// pane is on a tab is dropped without a word - the same silence a placement runs into.
    pub(crate) fn note_greeting_from_agents(&mut self, from_agents: bool) {
        if from_agents && self.own_tab_index.is_none() {
            self.open_hidden = true;
        }
    }
}

#[cfg(test)]
#[path = "unit/overlay_tests.rs"]
mod overlay_tests;
