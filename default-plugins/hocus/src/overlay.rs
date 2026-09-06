//! Where hocus draws itself: a panel in the bottom-left corner of the screen.
//!
//! hocus is an **overlay**: drawn over the session surface rather than inside a tab's layout, so
//! a tab keeps its full width and hocus floats above whatever is underneath. The panel is **two
//! panes drawn as one**: `hocus-agents`, the agent list, on the left, and the conversation to
//! its right, butted together so that their two touching frame columns read as a divider.
//!
//! **One panel, two plugins.** There is no host mechanism for two plugins in one pane, so the
//! merge is coupled geometry: hocus computes both rectangles as the two halves of a single
//! rectangle (`overlay_rects`, `split_panel`) and places them in one call
//! (`apply_overlay_geometry`), which is also what tells the server they belong together - a
//! placement batch that lands two overlays edge to edge links them, and the link is what makes
//! the divider draggable (`OverlayPanes::link_touching`, `seam_at`). Everything a placement
//! decides - the corner, the size, the padding - is decided once for the panel and both halves
//! inherit it: a press of Ctrl-o moves the panel, a frame drag carries both halves, a terminal
//! resize re-resolves the panel and cuts it again.
//!
//! **The divider is the user's, and it reaches hocus through the manifest.** A drag on the seam
//! is handled in the server (a frame press never reaches a plugin) and moves the two halves'
//! shared edge; it deliberately sets no user-placement claim, so hocus stays free to re-place
//! both halves. What hocus does instead is *read the split back*: `read_split` takes the agents
//! half's share off every `PaneUpdate` on which the two halves are adjacent, and every later
//! placement cuts the panel at that ratio (`State::split`). The ratio rather than a column
//! count, so a terminal resize keeps the proportion the user chose. The configured `split` is
//! only the starting point.
//!
//! **The panel has one visibility, not two.** Esc in either half puts the whole panel away and
//! `Ctrl e a` / `Ctrl e c` bring the whole of it back, focused on the half they name - so a half
//! is never on screen alone. hocus is the one that can act on that, being the only half that
//! knows both plugin ids: `hide_panel` suppresses both, `reveal_panel` unsuppresses the list
//! beside it (or `open_agents_pane` launches one if the panel has no other half), and
//! `reveal_self_beside_agents` is the mirror for a list revealed by its own keybinding. Esc in
//! the list reaches this through `wire::AGENT_STRIP_HIDE`; a reveal of the list reaches it
//! through `on_panes` noticing the list's suppression changed. Because the two halves are
//! always on screen together, hiding changes no rectangle: `split_panel` cuts a hidden panel
//! exactly as it cuts the one that comes back.
//!
//! **The panel has one focus ring, and no index anywhere.** The two halves are four stops -
//! the list, its history half, the conversation's tab bar, the box you type in - and `Tab` and
//! `Shift+Tab` walk them in screen order, wrapping. Nothing stores where in the ring the user
//! is, because nothing has to: the stop is *which pane the server has focused* times *that
//! pane's own sub-focus* (`Focus::List`/`History` in the list, `chat_tab_focus` and
//! `chat_input_released` here), and only the focused pane is sent a key at all. A shared index
//! would be a second copy of what the server already knows, stale after every mouse click.
//!
//! What does have to cross the seam is the two steps that leave a half, and they are
//! asymmetric for the same reason the visibility is: hocus knows both plugin ids and the list
//! knows only its own. So hocus moves the keyboard itself (`hand_keyboard_to_agents`, which
//! focuses the list and sends `AGENT_STRIP_TAKE` naming the half to land in), and the list
//! *asks* (`AGENT_STRIP_YIELD`, answered by `take_ring_focus`) exactly as it does for Esc and
//! Ctrl-o. Neither message carries a `plugin_url`: a Tab never launches a half, and never
//! reveals a panel that is down. The box's own claim on `Tab` is completion, so a draft
//! standing in it holds the key - `Esc` releases the box, keeping the draft, and the ring
//! moves again.
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
//! born half the surface, *centered* (`OverlayPanes::base_geom`) - which, for two halves that
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
//! panel *clear of* a tiled pane - whichever one it is covering, or the largest one the user is
//! not working in, by `pick_anchor` - and remembers that pane as the panel's `anchor`. The press
//! puts the panel in the corner that clears that pane best (`corner_for`), at exactly the size
//! the corner placement would have given it.
//!
//! **Both halves take the key, and a press moves the panel.** The conversation handles Ctrl-o
//! itself; the list has no geometry of its own to work with, so it asks over
//! `wire::AGENT_STRIP_REPOSITION` and hocus does the arithmetic, as it does for every other
//! placement. Either way it is the one panel that moves, with its split intact.
//!
//! **Nothing the layout does moves the panel.** A placement is two steps - *choose* a corner,
//! then *resolve* it against the surface (`corner_for`, then `rect_in_corner`) - and only the
//! first reads the layout. The corner a press chose is remembered (`Panel::placed_corner`), so
//! every later placement is a function of that corner, the screen and the config: a pane opening,
//! a pane closing, a re-tile and a swap layout are not inputs to it. `track_anchor` still reads
//! every `PaneUpdate`, but only to keep the anchor the *next* press ranks against - it places
//! nothing. What places is a press, a reveal, a config change, and a terminal resize,
//! which resolves the same corner against the screen's new size.
//!
//! The config is where the *size* comes from, which is the one thing a placement does not carry
//! over: a press threads the size the manifest reports (`own_rect`, then `reposition`), so it
//! keeps a panel the user resized, but every other placement passes `None` and gets `panel_size`'s
//! configured size back.
//!
//! **A press always moves the panel.** Every placement is one of four corners (`Corner`), so a
//! repeat press is a walk: `pick_anchor` tours the panes nobody has been sent clear of yet
//! (`Panel::warp_visited`), and once they are all toured the press steps to the next corner of
//! the anchor's ordering (`Panel::warp_step`) instead of doing nothing. Four corners, then the
//! tour re-opens. The only press that moves nothing is one on a panel the size of the screen,
//! where all four corners are the same rectangle.
//!
//! **A half is never narrower than `MIN_PANE_COLS`, the panel never shorter than `MIN_PANE_ROWS`,
//! and nothing is ever off the screen.** That is a guarantee of this module rather than a hope:
//! `panel_size` applies the floor - one half's worth, or two - after the room the padding leaves
//! and before the hard edge of the surface, `pad_of` gives the padding up before the panel does,
//! `corner_rect` is the one place a rectangle is put on the screen, and `agents_cols` floors both
//! halves of the configured cut. The one exception is a divider the user dragged: the server lets
//! it go a good deal narrower than that, and the split is honoured where they left it rather
//! than pulled back on the next placement. On a surface too small even for the floor - or a half
//! dragged to a sliver - the pane gets the whole of what there is and `ui::render` drops chrome
//! to fit - see `ui::chrome_for`. Nothing here ever asks
//! for a rectangle the server would have to clip back (`PaneGeom::adjust_coordinates` does that
//! silently), so the rectangle hocus remembers is the rectangle it got.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use zellij_tile::prelude::*;

use hocus_proto::launch;

use crate::ui;
use crate::State;

/// Cells of clear screen left around the panel on every side.
const DEFAULT_PAD: usize = 3;
/// How many `PaneUpdate`s a dismissal may discount before the manifest is believed - see
/// [`crate::State::hide_pending`].
///
/// One Esc sends two hides, on two panes, and the snapshots taken in between report the panel
/// half up. Two frames covers that; the extra pair is slack for a busy screen thread. It is
/// deliberately small: every frame of budget is a frame in which a `c` pressed on the heels of
/// the Esc is read as the stale hide instead, and the panel does not come back until the next
/// manifest. Two wrong frames is a flicker; a flag that never lapsed was a wedge.
const HIDE_PENDING_FRAMES: u8 = 4;
/// How many `PaneUpdate`s a focus this half asked for itself may discount before the manifest
/// is believed - see [`crate::State::ring_focus_pending`].
///
/// The same problem `HIDE_PENDING_FRAMES` solves, from the other side: the host builds one
/// update per attached client, so a snapshot taken before `show_self` still reports this half
/// unfocused and can arrive after it. Believing one of those would reset the ring stop the
/// yield had just set. The budget is small and lapses for the same reason: a focus request
/// that never lands must not suppress a genuine reset for ever.
const RING_FOCUS_PENDING_FRAMES: u8 = 4;
/// The panel's width, as a percentage of the display area. Wide enough for a chat message not
/// to be wrapped every other word beside the agent list, and still well short of the whole
/// screen, so the panel reads as something sitting on the work rather than a split of it.
const DEFAULT_WIDTH_PERCENT: usize = 60;
/// The panel's height, as a percentage of the display area. Two thirds: taller than it is wide,
/// and short enough that the top of the screen stays the user's own.
const DEFAULT_HEIGHT_PERCENT: usize = 66;
/// The agent list's share of the panel's width, as a percentage, until a seam drag says
/// otherwise.
const DEFAULT_SPLIT_PERCENT: usize = 50;
/// The fewest columns a half may be given.
///
/// An overlay is framed, so the plugin is rendered two cells narrower and two shorter than the
/// pane it is in: the pane's minimum is what `ui::render` needs plus that frame. With
/// `pane_frames false` the two cells are slack rather than a deficit, which costs a slightly
/// roomier panel and nothing else.
const MIN_PANE_COLS: usize = ui::MIN_COLS + 2;
/// The fewest rows the panel may be given - `ui::MIN_ROWS` and the frame around it. This is a
/// real minimum: the title, a transcript row, the input box and the rules around it.
const MIN_PANE_ROWS: usize = ui::MIN_ROWS + 2;

/// A configured size, resolved against `whole`. Accepts `"40%"` and `"12"`, and falls back to
/// `default_percent` of `whole` for anything missing or unparseable - a typo in the KDL costs a
/// default-sized panel rather than a crash.
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

/// Which halves the panel has to hold.
///
/// What a placement is told, and all it is told: the halves share the panel's width when there
/// are two of them and one takes the whole of it when it is alone.
///
/// **Not a visibility question.** Both halves hide and come back together, so the cut is the
/// same whether the panel is up or down - a hidden panel is placed as it will return. What
/// makes a half absent is that it does not *exist*: a conversation revealed before the list it
/// asked for has loaded, or one the user closed the list's pane out from under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shown {
    pub chat: bool,
    pub agents: bool,
}

impl Shown {
    #[cfg(test)]
    pub(crate) const BOTH: Shown = Shown {
        chat: true,
        agents: true,
    };

    /// How many halves the panel's width has to hold.
    fn halves(self) -> usize {
        match self.chat != self.agents {
            true => 1,
            false => 2,
        }
    }
}

/// Where the divider is: the agent list's columns out of the panel's, as the manifest last
/// reported the two halves sitting together. `None` is the configured `split`.
///
/// A ratio rather than a column count, so that a panel re-resolved against a resized screen is
/// cut in the proportion the user dragged it to rather than at a column that may no longer be
/// inside it. Re-derived from the manifest rather than remembered from a drag, because the drag
/// happens in the server and this is the only word hocus gets of it.
pub(crate) type Split = Option<(usize, usize)>;

/// The two rectangles, in the order `(chat, agents)`, as they sit with the panel in its home
/// corner: the agent list at the panel's left edge and the conversation from the divider to its
/// right edge, sharing a `y` and a height, with no gap between them.
///
/// This is *home*, not the whole of a placement: a panel Ctrl-o has moved is placed from its
/// remembered corner instead (`State::panel_rect`). Rectangles all the way in, coordinates only
/// at the edge - see [`fixed_rect`].
///
/// `None` before any `TabUpdate` has said how big the screen is - there is nothing to compute
/// against yet, and a guess would put the panes somewhere the user would see them move away from.
///
/// The production path goes through [`State::panel_rect`] and [`split_panel`] directly, since
/// the corner is the panel's to remember; this is the same two calls with the home corner named,
/// and is what the geometry is tested as.
#[cfg(test)]
pub(crate) fn overlay_rects(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    shown: Shown,
    split: Split,
) -> Option<(Rect, Rect)> {
    let panel = rect_in_corner(config, surface_rows, surface_cols, shown, CORNERS[0], None)?;
    Some(split_panel(config, panel, shown, split))
}

/// Cut `panel` into `(chat, agents)`.
///
/// The list takes the left `agents_cols` columns and the conversation the rest; a half with no
/// sibling to share with takes the whole panel. Both are always placed - a hidden overlay is
/// re-resolved by the server like any other - so a reveal finds its half already where it
/// belongs rather than a placement behind.
pub(crate) fn split_panel(
    config: &BTreeMap<String, String>,
    panel: Rect,
    shown: Shown,
    split: Split,
) -> (Rect, Rect) {
    let agents_cols = agents_cols(config, panel.cols, split);
    let agents_share = Rect {
        cols: agents_cols,
        ..panel
    };
    // A panel of one column cannot hold two halves; the conversation then overlaps the list
    // rather than being placed with no width, which `FloatingPaneCoordinates::new` refuses and
    // the host answers with half the screen, centered.
    let chat_share = Rect {
        x: (panel.x + agents_cols).min(panel.x + panel.cols.saturating_sub(1)),
        cols: panel.cols.saturating_sub(agents_cols).max(1),
        ..panel
    };
    match (shown.chat, shown.agents) {
        (true, false) => (panel, agents_share),
        (false, true) => (chat_share, panel),
        _ => (chat_share, agents_share),
    }
}

/// The agent list's width within a panel `panel_cols` wide: the ratio the last seam drag left,
/// else the configured `split` floored so that each half keeps [`min_half_cols`].
///
/// **A dragged split is honoured as it is.** The server's floor for the drag is a few columns,
/// well under what a half can draw a conversation in - but the user put the divider there, and
/// pulling it back to the floor on the next hide, reveal or resize would move a divider they
/// had just placed. The floor is for the configured split, which nobody has looked at yet; a
/// half dragged to a sliver draws what it can (`ui::chrome_for`) until it is dragged back.
fn agents_cols(config: &BTreeMap<String, String>, panel_cols: usize, split: Split) -> usize {
    match split {
        Some((agents, panel)) if panel > 0 => {
            (panel_cols * agents / panel).clamp(1, panel_cols.saturating_sub(1).max(1))
        },
        _ => {
            let wanted = resolve(config.get("split"), panel_cols, DEFAULT_SPLIT_PERCENT);
            let min = min_half_cols(panel_cols);
            wanted.clamp(min, panel_cols.saturating_sub(min).max(min))
        },
    }
}

/// The fewest columns a half of the cut may have: [`MIN_PANE_COLS`], or half the panel when the
/// panel is too narrow for two of that. Splitting what there is evenly is better than pushing one
/// half off the panel, and `ui::render` covers the difference by dropping chrome rather than
/// refusing to draw.
fn min_half_cols(panel_cols: usize) -> usize {
    MIN_PANE_COLS.min(panel_cols / 2).max(1)
}

/// The fewest columns the panel may be given: one half's minimum per half it has to hold, or the
/// whole screen when that is less.
fn min_panel_cols(cols: usize, halves: usize) -> usize {
    (halves * MIN_PANE_COLS).min(cols.max(1))
}

/// The panel's `(width, height)`, resolved against the surface and clamped to it.
///
/// The size half of *every* placement: moving the panel clear of a pane, or one corner along,
/// must never resize it, so there is one answer to the size rather than a copy per path.
///
/// **The minimum is a floor, not a preference.** See [`fit`] for the order that makes it one, and
/// [`min_panel_cols`] for what happens on a screen too narrow to honour it.
fn panel_size(
    config: &BTreeMap<String, String>,
    rows: usize,
    cols: usize,
    shown: Shown,
) -> (usize, usize) {
    let halves = shown.halves();
    let pad = pad_of(config, rows, cols, halves);
    let (configured_width, configured_height) = configured_size(config, rows, cols);
    // What is between the paddings, which is the most the panel may take.
    let across = cols.saturating_sub(pad * 2);
    let width = fit(
        configured_width,
        min_panel_cols(cols, halves),
        across,
        cols.max(1),
    );
    // What is left of the screen once the padding is taken off the top and the bottom, and the
    // share of it the panel actually takes.
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

/// The padding, resolved against a surface of `rows` by `cols`.
///
/// Capped at a quarter of the surface on *both* axes, so a fat `pad` on a small terminal cannot
/// eat the panel it is supposed to be insetting. Capping against the rows alone was not enough:
/// `pad` is the rectangle's `x` as well as its inset off the top and bottom, so on a tall, narrow
/// display a rows-only cap let it exceed the column count outright and put the panel off the
/// right-hand edge of the screen.
///
/// **The padding gives way before the panel does.** A quarter of the screen is still too much
/// when what is left cannot hold a panel anybody could read, so the cap also asks what the
/// surface has to fit: the minimum on each axis, and across the columns one half's minimum per
/// half on screen. A thinner inset is a smaller loss than an unusable panel, and this is what
/// keeps the two halves side by side and on screen down to surfaces of a few dozen columns.
fn pad_of(config: &BTreeMap<String, String>, rows: usize, cols: usize, halves: usize) -> usize {
    config
        .get("pad")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_PAD)
        .min(rows / 4)
        .min(cols / 4)
        // the panel's minimum, with a padding above and below it
        .min(rows.saturating_sub(MIN_PANE_ROWS) / 2)
        // and beside it
        .min(cols.saturating_sub(min_panel_cols(cols, halves)) / 2)
}

/// `(width, height)` as configured, resolved against the surface and not yet clamped to anything.
fn configured_size(config: &BTreeMap<String, String>, rows: usize, cols: usize) -> (usize, usize) {
    (
        resolve(config.get("width"), cols, DEFAULT_WIDTH_PERCENT),
        resolve(config.get("height"), rows, DEFAULT_HEIGHT_PERCENT),
    )
}

/// Coordinates stated as fixed cells, never percentages.
///
/// These are already resolved against the surface we were told about, and a percentage would be
/// resolved a second time by the server. The two denominators would agree now that `surface_of`
/// reads the session surface, but stating the arithmetic once is still the point - the panel is
/// anchored to the *bottom*, which no percentage can say.
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
/// the panel's own manifest entries can be compared against the tiled panes beside them.
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

    /// The smallest rectangle holding both - the panel, given its two halves.
    fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect {
            x,
            y,
            cols: (self.x + self.cols).max(other.x + other.cols) - x,
            rows: (self.y + self.rows).max(other.y + other.rows) - y,
        }
    }

    /// Whether `other` starts on the column this rectangle ends on, along the same row - the two
    /// halves of a panel, as the manifest reports them.
    fn is_beside(&self, other: &Rect) -> bool {
        self.x + self.cols == other.x && self.y == other.y
    }
}

/// A corner of the surface the panel may sit in.
///
/// Every placement is one of these four - the default corner, the placement that gets clear of an
/// anchor, and every step of the cycle a repeat Ctrl-o walks - so naming them is what lets a press
/// ask for "the next place" rather than restate the arithmetic. It is also what lets a placement
/// be *remembered* rather than re-derived: see [`Panel::placed_corner`].
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

/// The order for a panel that has the tab to itself - the far corner from the default first, and
/// then the rest of the perimeter. See [`corner_order`].
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

/// The four corners, best first, for a panel asked to get clear of `anchor`.
///
/// The first is where the placement goes; the rest are where a repeat press walks, in order.
///
/// - No anchor - the panel is in the corner and has been asked of nothing - is [`CORNERS`], the
///   plain perimeter starting at home.
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
    let Some(anchor) = anchor.filter(|_| !alone) else {
        // Nothing to rank against - no anchor at all, or an anchor that fills the tab and so ties
        // every corner - so the fixed ordering is the whole answer.
        return match anchor {
            None => CORNERS,
            Some(_) => CORNERS_ALONE,
        };
    };
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
        true => PaneId::Plugin(zellij_tile::prelude::PluginId::new(pane.id)),
        false => PaneId::Terminal(zellij_tile::prelude::TerminalId::new(pane.id)),
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

/// The tiled pane the panel should get clear of, given every pane on its tab.
///
/// The pane the user is working in is out: it is the one they are already looking at, and hocus
/// is asked to uncover something else. The exception is a tab with a single tiled pane, where the
/// focused pane is the only thing there is to be clear of - without it Ctrl-o would do nothing at
/// all on the commonest layout there is.
///
/// Of the rest, **the pane the panel is covering most wins**, and the larger pane wins among
/// equals. That ranking is what makes a press walk pane to pane: the first press takes hocus off
/// the pane it was on, which lands it on some other pane, and the second press takes it off that
/// one. Where hocus covers nothing the overlaps are all zero and the ranking reads as "the largest
/// pane you are not in". The sort is stable, so a tie beyond that goes to manifest order.
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

/// Where the panel goes: the size [`panel_size`] gives, at the corner `step` of the order
/// [`corner_order`] gives for `anchor`.
///
/// **The two steps of every placement, in one call** - [`corner_for`] chooses, [`rect_in_corner`]
/// resolves. `anchor: None` at `step: 0` is the default bottom-left corner, which is what
/// [`overlay_rects`] asks for; a press asks the same two functions itself, because it has a size
/// to carry. They are still the one arithmetic, so a size or a padding the user configured means
/// the same thing in all of them. The panel is never resized to suit the anchor and neither is
/// the anchor: Ctrl-o is a request to *see* a pane, and the whole job is to stop covering it.
///
/// `step` is taken modulo the four corners, so a cycle that has walked past the end comes back to
/// the best one.
///
/// `None` when there is no surface to compute against, which leaves the caller with the placement
/// it already has. An anchor with no size is not an anchor at all and falls back to the corner.
///
/// A press threads a size and so calls the two halves itself (`State::corner_at`, `State::rect_in`);
/// this is the pair with nothing threaded, which is what the placement is tested as.
#[cfg(test)]
pub(crate) fn panel_placement(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    shown: Shown,
    anchor: Option<Rect>,
    alone: bool,
    step: usize,
) -> Option<Rect> {
    let corner = corner_for(
        config,
        surface_rows,
        surface_cols,
        shown,
        anchor,
        alone,
        step,
        None,
    )?;
    rect_in_corner(config, surface_rows, surface_cols, shown, corner, None)
}

/// The choosing half of a placement: which corner the panel belongs in, given the pane it has
/// been asked to get clear of.
///
/// **The only half that reads the layout.** The anchor's rectangle and whether it has the tab to
/// itself are what rank the four corners, and they are read when a press asks and at no other
/// time. What a press produces is a `Corner`, which [`Panel::placed_corner`] remembers - so every
/// later placement goes straight to [`rect_in_corner`], and no manifest can move the panel.
#[allow(clippy::too_many_arguments)]
fn corner_for(
    config: &BTreeMap<String, String>,
    surface_rows: usize,
    surface_cols: usize,
    shown: Shown,
    anchor: Option<Rect>,
    alone: bool,
    step: usize,
    size: Option<(usize, usize)>,
) -> Option<Corner> {
    let (pad, size) = placement_terms(config, surface_rows, surface_cols, shown, size)?;
    let anchor = anchor.filter(|anchor| anchor.rows > 0 && anchor.cols > 0);
    let order = corner_order(anchor, alone, surface_rows, surface_cols, pad, size);
    Some(order[step % CORNERS.len()])
}

/// The placing half: the rectangle `corner` names on the surface as it is now, at the size
/// [`panel_size`] gives unless the caller states one.
///
/// Reads the config and the screen and nothing else, which is the whole of the stability the
/// module header claims: with the corner remembered, no pane event is an input to where the
/// panel sits.
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

/// The padding and the panel size both halves of a placement resolve against, so the two cannot
/// disagree about either. `None` on a surface with no extent to put anything on.
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

/// The surface the panel is placed against, off the active tab's `TabInfo`.
///
/// An overlay's coordinates are resolved against the whole session surface
/// (`OverlayPanes::surface`), which is the tab's display area - chrome included - at the screen
/// origin. That is the pair the arithmetic in `overlay_rects` has to use.
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

/// The panel's placement: where it is, and what the next press moves it clear of.
///
/// One of these on `State`, for the one panel: both halves take Ctrl-o, and either press moves
/// the panel along the same tour.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Panel {
    /// The tiled pane this panel was last asked to keep clear of, chosen with Ctrl-o. `None` is
    /// "nothing in particular".
    ///
    /// A pane rather than a position, because it is the *next* press that reads it: `pick_anchor`
    /// will not offer the same pane twice in a tour, and `corner_for` ranks the four corners
    /// against wherever the pane is by then. It is not something the panel follows -
    /// `track_anchor` keeps it current and places nothing. Cleared when the pane is no longer on
    /// our tab - closed, or we followed a client to another tab - which starts a fresh tour and
    /// leaves the panel where it is.
    pub(crate) anchor: Option<PaneId>,
    /// The anchor's rectangle as the last `PaneUpdate` reported it - what the next press ranks
    /// the corners against.
    pub(crate) anchor_rect: Option<Rect>,
    /// The panes Ctrl-o has toured since the cycle last wrapped.
    ///
    /// What makes a repeat press walk to a *different* pane instead of re-picking the best one:
    /// the ranking is a function of rectangles, so it answers the same pane twice whenever nothing
    /// overlaps or the manifest has not caught up. Pruned to the panes still on the tab by
    /// `track_anchor`, and emptied when the cycle comes round.
    pub(crate) warp_visited: Vec<PaneId>,
    /// How far into the anchor's corner ordering the *next press* is.
    ///
    /// Reset to 0 by a press that reaches a new pane, advanced by a press that has none left to
    /// reach - which is what turns "nowhere better to go" from a no-op into the next corner. Read
    /// only by `reposition`: where the panel actually sits is [`Panel::placed_corner`].
    pub(crate) warp_step: usize,
    /// The corner the panel was last placed in, and `None` for one that has never been placed -
    /// which is its home, the bottom-left corner.
    ///
    /// **What makes a placement independent of the layout.** Choosing a corner reads the anchor
    /// and the tab; resolving one reads only the screen. Remembering the choice means every
    /// placement after a press - a hide, a reveal, a config change, a terminal resize - is the
    /// second half alone, so a pane opening or closing cannot move the panel and a resize
    /// re-resolves the corner the user left it in rather than dragging it home.
    pub(crate) placed_corner: Option<Corner>,
    /// The panel rectangle the last placement asked for.
    ///
    /// The reference for "did this press actually move me?", and better than the manifest for
    /// ranking because it is never stale - two presses can arrive between two `PaneUpdate`s.
    pub(crate) placed_rect: Option<Rect>,
}

impl Panel {
    /// Forget the pane this panel was getting clear of, and the tour it was on.
    ///
    /// The anchor is a pane on a particular tab, so a panel carried to another one has nothing
    /// left to be clear of and the next press starts fresh. Moves nothing: where the panel sits
    /// is [`Panel::placed_corner`], which a tab change does not touch.
    fn reset_tour(&mut self) {
        self.anchor = None;
        self.anchor_rect = None;
        self.warp_step = 0;
        self.warp_visited.clear();
    }
}

impl State {
    /// Whether this hocus is the panel this module is about.
    ///
    /// Everything below - the corner, the two halves, the divider, the one visibility - belongs to
    /// a hocus drawn *over* the session. A hocus the user put in their layout
    /// (`pane { plugin location="hocus" }`) is a plain tiled pane: it fills what the layout gave
    /// it, grows no agent list, moves nowhere and is never hidden. The layout is the user's
    /// arrangement, and a plugin has no business rearranging it.
    ///
    /// Said by the manifest and by nothing else - see [`State::own_is_floating`] - so this is
    /// `false` until the first `PaneUpdate` names our pane. That costs a real panel nothing: the
    /// only placement before that manifest is the one on load, which already does nothing (no
    /// `TabUpdate` has said how big the screen is, so `panel_rect` is `None`), and `on_panes`
    /// retries every placement that could not land.
    ///
    /// Without this a tiled hocus took the whole panel path: its first manifest launched an agent
    /// list beside it, the list's greeting launched a *second*, overlay hocus over the user's
    /// transcript, and every frame asked the server to move a pane that is not an overlay - which
    /// fails the whole placement batch, so the list stayed at the centred default it was born at.
    pub(crate) fn is_panel(&self) -> bool {
        self.own_is_floating == Some(true)
    }

    /// Which halves there are to cut the panel into.
    ///
    /// Not a visibility question any more - the two halves hide and come back together, so
    /// hiding never changes the cut. What it answers is whether there is a *list*: one has to
    /// exist before it can be given a share, and between a reveal and the launch landing there
    /// is a conversation with nothing beside it.
    ///
    /// `chat` is unconditionally true, since this plugin *is* that half: it is a field rather
    /// than an assumption because [`split_panel`] has to be told which of the two a lone half is,
    /// and a test naming only a list is the case that says why.
    ///
    /// A hidden panel answers the same as a shown one. That is the point of hiding both halves
    /// together: there is one cut, and it is the cut the panel comes back at.
    pub(crate) fn shown(&self) -> Shown {
        Shown {
            chat: true,
            agents: self.agents_plugin.is_some(),
        }
    }

    /// Re-place both halves against the display area as it is now.
    ///
    /// Safe to call whenever there is a panel to place: the ids are looked up rather than assumed,
    /// and once the user has dragged the panel the server keeps their position and takes only the
    /// size from here.
    ///
    /// **Only for a panel**, which is what the guard is for.
    /// `Tab::change_overlay_coordinates_batch` refuses a batch naming a pane that is not an
    /// overlay - and refuses the *whole* batch, on the first such id, so one wrong id leaves every
    /// other half unplaced too. [`State::is_panel`] is what keeps hocus from ever sending one; the
    /// server stays strict, because with that guard in place no correct caller can.
    ///
    /// **Doing nothing is exactly what happens when the pane is not on a tab yet**, and it is
    /// silent - `Screen::change_overlay_coordinates` looks the id up and moves on. That is why
    /// `placed` is set from `own_tab_index` rather than from having made the call: the manifest
    /// saying where our pane is, is the only evidence the call could have landed. A placement that
    /// could not land leaves `placed` false, and `on_panes` tries again the moment it can.
    ///
    /// Both halves in **one call**, always: the batch is what links them in the server, and the
    /// link is what makes the divider between them draggable. Which halves are on screen decides
    /// how the panel is cut (`split_panel`), so hiding and revealing either half come back
    /// through here.
    ///
    /// Both halves are tracked, and separately: see [`State::agents_placed`] for why one flag was
    /// not enough.
    pub(crate) fn apply_overlay_geometry(&mut self) {
        if !self.is_panel() {
            return;
        }
        let Some(panel) = self.panel_rect() else {
            return;
        };
        self.panel.placed_corner = Some(self.panel.placed_corner.unwrap_or(CORNERS[0]));
        // What the next press ranks against, and what tells it whether it has moved. Set on every
        // placement and not only Ctrl-o's, because a resize re-places and so moves us too.
        self.panel.placed_rect = Some(panel);
        let placements = self.placements_of(panel);
        #[cfg(test)]
        {
            self.last_placement = Some((placements.clone(), false));
        }
        change_overlay_coordinates(placements);
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

    /// The panel, cut into the halves there are to place: the conversation always, the list when
    /// one has announced itself.
    fn placements_of(&self, panel: Rect) -> Vec<(PaneId, FloatingPaneCoordinates)> {
        let (chat, agents) = split_panel(&self.kdl, panel, self.shown(), self.split);
        let mut placements = vec![(PaneId::Plugin(self.own_plugin_id), fixed_rect(chat))];
        if let Some(agents_id) = self.agents_plugin {
            placements.push((PaneId::Plugin(agents_id), fixed_rect(agents)));
        }
        placements
    }

    /// Where the panel sits now: in the corner it was last placed in, or - never placed - at
    /// home.
    ///
    /// `None` before any `TabUpdate` has said how big the screen is.
    fn panel_rect(&self) -> Option<Rect> {
        self.rect_in(self.panel.placed_corner.unwrap_or(CORNERS[0]), None)
    }

    /// A press's placement: the panel, at the corner it just chose, over anything the user
    /// dragged.
    ///
    /// The override is the one thing a press adds: the server keeps a position the user dragged
    /// over anything a plugin asks for, and a key the user pressed to move it is theirs to
    /// override. A user-stated size remains independent - it is threaded in as `size`.
    fn place_panel(&mut self, size: Option<(usize, usize)>, corner: Corner) {
        // The other `change_overlay_coordinates` caller, and gated for the same reason as
        // `apply_overlay_geometry`: a tiled hocus has no panel to move, and asking the server to
        // move one is an error rather than a no-op.
        if !self.is_panel() {
            return;
        }
        let Some(panel) = self.rect_in(corner, size) else {
            return;
        };
        self.panel.placed_corner = Some(corner);
        self.panel.placed_rect = Some(panel);
        let placements = self.placements_of(panel);
        #[cfg(test)]
        {
            self.last_placement = Some((placements.clone(), true));
        }
        change_overlay_coordinates_overriding_user_placement(placements);
        // The manifest naming a tab is the only evidence the call could have landed - the same
        // test `apply_overlay_geometry` makes.
        self.placed = self.own_tab_index.is_some();
        self.agents_placed = self.agents_plugin.is_none() || self.agents_tab_index.is_some();
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
            self.shown(),
            self.panel.anchor.and(self.panel.anchor_rect),
            sole_tiled_pane(&self.tab_panes),
            step,
            size,
        )
    }

    /// The panel's rectangle in `corner`, optionally retaining its current size.
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

    /// Where the panel is now: the rectangle the last placement asked for, else the manifest's
    /// word for it, else the home it has not been moved from.
    ///
    /// The last placement comes first because it is never stale. Two presses can arrive between
    /// two manifests, and ranking the second against the *first* press's rectangle is what stops
    /// them computing the same answer twice. A position the user **dragged** is not reflected here
    /// because the last placement wins, which costs a slightly worse choice of next pane and never
    /// a stalled press. The manifest's size is used separately so a move cannot undo a resize.
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
            // The last placement wins for position so two quick presses cannot read the same
            // stale manifest, but the manifest owns size: it is how a user resize reaches us.
            own.cols = reported.cols;
            own.rows = reported.rows;
        }
        own
    }

    /// The panel as the manifest reports it: the rectangle round whichever halves are drawn, or
    /// `None` with neither on screen. Both halves together are the panel; one alone has the whole
    /// of it.
    fn reported_panel(&self) -> Option<Rect> {
        let drawn = |plugin_id: Option<zellij_tile::prelude::PluginId>| {
            let plugin_id = plugin_id?;
            self.tab_panes
                .iter()
                .find(|pane| {
                    pane.is_plugin && PluginId::new(pane.id) == plugin_id && !pane.is_suppressed
                })
                .map(Rect::of)
        };
        match (drawn(Some(self.own_plugin_id)), drawn(self.agents_plugin)) {
            (Some(chat), Some(agents)) => Some(chat.union(&agents)),
            (Some(half), None) | (None, Some(half)) => Some(half),
            (None, None) => None,
        }
    }

    /// Read the divider back off a fresh manifest of our tab.
    ///
    /// A seam drag happens in the server and claims nothing, so the manifest is the only word
    /// hocus gets of it: whenever both halves are drawn and the list ends on the column the
    /// conversation starts on, the list's share of their joint width is the split, and every
    /// later placement cuts the panel there. A manifest on which the two are not beside each
    /// other - one hidden and filling the panel, or the frame between them not yet re-drawn -
    /// says nothing about the split and leaves it as it was.
    ///
    /// **Only a divider somebody moved is read.** Most manifests with the halves side by side
    /// are the server echoing hocus's own placement, and that cut is the split *floored* to a
    /// column: reading it back would replace the ratio with its floor, and the next resize would
    /// floor that again, so the divider crept left a column at a time for as long as the
    /// terminal kept resizing. A manifest showing the cut the split already makes of that
    /// width is therefore left alone, and the configured split stays configured until a drag.
    pub(crate) fn read_split(&mut self, panes: &[PaneInfo]) {
        let Some(agents_id) = self.agents_plugin else {
            return;
        };
        let drawn = |plugin_id: zellij_tile::prelude::PluginId| {
            panes
                .iter()
                .find(|pane| {
                    pane.is_plugin && PluginId::new(pane.id) == plugin_id && !pane.is_suppressed
                })
                .map(Rect::of)
        };
        let (Some(chat), Some(agents)) = (drawn(self.own_plugin_id), drawn(agents_id)) else {
            return;
        };
        if !agents.is_beside(&chat) {
            return;
        }
        let panel_cols = agents.cols + chat.cols;
        if agents.cols == agents_cols(&self.kdl, panel_cols, self.split) {
            // Our own cut, echoed. See above.
            return;
        }
        self.split = Some((agents.cols, panel_cols));
    }

    /// Ctrl-o: move the panel somewhere else.
    ///
    /// **Both halves take the key**, and either press moves the panel: the conversation handles
    /// it itself, the agent list asks for it over `wire::AGENT_STRIP_REPOSITION`, and there is
    /// one anchor and one tour between them.
    ///
    /// **A press moves.** Where there is a tiled pane nobody has been sent clear of yet,
    /// [`pick_anchor`] names it and the panel is placed at the corner that clears it best, as it
    /// always was. Where every pane has been toured - a tab of two panes with the other one
    /// focused is the commonest case there is - the press steps to the next corner of the same
    /// anchor's ordering instead of doing nothing, and once round the four the tour re-opens and
    /// the ranking starts again.
    ///
    /// Sent as the user's own placement, so it lands even on a panel they dragged.
    ///
    /// The one press that moves nothing is one on a panel the size of the screen, where all four
    /// corners are the same rectangle and there is genuinely nowhere to go.
    pub(crate) fn reposition(&mut self) {
        let own = self.own_rect();
        let size = Some((own.cols, own.rows));
        match pick_anchor(own, &self.tab_panes, &self.panel.warp_visited) {
            Some(next) => {
                let rect = self
                    .tab_panes
                    .iter()
                    .find(|pane| pane_id_of(pane) == next)
                    .map(Rect::of);
                self.panel.anchor_rect = rect;
                self.panel.anchor = Some(next);
                self.panel.warp_visited.push(next);
                // A fresh pane to be clear of: back to the corner that clears it best.
                self.panel.warp_step = 0;
            },
            None => self.advance_warp(),
        }
        // The invariant, and the whole of the fix: where the step names the rectangle the
        // panel is already at, take the next one. Once round at most.
        let mut target = None;
        for _ in 0..CORNERS.len() {
            let Some(corner) = self.corner_at(self.panel.warp_step, size) else {
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
            self.place_panel(size, corner);
        }
    }

    /// Ctrl-o pressed in the agent list, relayed over `wire::AGENT_STRIP_REPOSITION`.
    ///
    /// The same move as [`State::reposition`], with one refusal in front of it: a list that is
    /// not on the conversation's tab is not moved. A press ranks the corners against
    /// `tab_panes`, which is *our* tab, and placing a parted list against panes it is not sitting
    /// with would put it clear of nothing - and would mark it placed while it is still elsewhere.
    /// `on_panes` drags the pair back together, and the press after that lands.
    pub(crate) fn reposition_from_list(&mut self) {
        let list_is_here = self.agents_plugin.is_some()
            && self.own_tab_index.is_some()
            && self.agents_tab_index == self.own_tab_index;
        if !list_is_here {
            return;
        }
        self.reposition();
    }

    /// Step one corner along, wrapping back to the best one.
    ///
    /// The wrap re-opens the tour: every pane is a candidate again, so the press after a full
    /// cycle goes back to walking pane to pane rather than round the corners forever.
    fn advance_warp(&mut self) {
        self.panel.warp_step += 1;
        if self.panel.warp_step >= CORNERS.len() {
            self.panel.warp_step = 0;
            self.panel.warp_visited.clear();
        }
    }

    /// Read the panel's anchor off a fresh manifest of our tab: gone, moved, or as it was.
    ///
    /// **Bookkeeping for the next press, and not a placement.** The anchor is what Ctrl-o ranks
    /// the four corners against, so it has to keep up with the pane it names - but keeping up is
    /// all it does. Nothing here moves the panel, because nothing the *layout* does is allowed
    /// to: a pane closing, opening or being re-tiled leaves the panel exactly where it is, and
    /// the corner it is in is remembered rather than re-derived (see [`Panel::placed_corner`] and
    /// the module header). Following a pane around used to be this function's job, and it is
    /// what made a pane close jump the panel across the screen - a re-tile could flip
    /// `sole_tiled_pane` and swap the whole corner ordering underneath it.
    ///
    /// A pane that is no longer a tiled pane on our tab - closed, floated, or left behind on the
    /// tab we moved from - releases the anchor: there is nothing left to be clear of, so the next
    /// press starts a fresh tour. The panel stays put through that too.
    pub(crate) fn track_anchor(&mut self, panes: &[PaneInfo]) {
        let panel = &mut self.panel;
        // A pane that has closed cannot hold a slot in the tour: the press that came round to it
        // would find it gone and go nowhere new.
        panel
            .warp_visited
            .retain(|id| panes.iter().any(|pane| pane_id_of(pane) == *id));
        let Some(anchor) = panel.anchor else {
            return;
        };
        let now = panes
            .iter()
            .find(|pane| pane_id_of(pane) == anchor && !pane.is_floating && !pane.is_suppressed)
            .map(Rect::of);
        match now {
            // The cycle belongs to the anchor: with the pane gone there is nothing to be clear
            // of, and the next press starts a fresh tour. Safe to reset because the panel's
            // position is `placed_corner` now, not `(anchor_rect, warp_step)`.
            None => panel.reset_tour(),
            // The rectangle a press ranks against, kept current. The step is kept too, so a press
            // after a re-tile carries on from where the cycle had got to.
            Some(rect) => panel.anchor_rect = Some(rect),
        }
    }

    /// Put the **panel** away without unloading either half.
    ///
    /// One Esc, both halves: the panel is one thing to the user, so it has one visibility, and
    /// hocus is the half that can act on that because it is the one that knows both plugin ids.
    /// Esc in the list arrives here over `wire::AGENT_STRIP_HIDE` (that pane hides itself on the
    /// same press, so this call finds nothing left to hide and says so harmlessly), and so does
    /// the Enter that walks into an agent - by which time that agent's pane holds the keyboard,
    /// so hiding this half takes nothing from it.
    ///
    /// Nothing is torn down: a stream keeps arriving into a suppressed pane, and a tool call
    /// waiting on y/n is still waiting when the panel comes back.
    ///
    /// **No re-placement.** The cut no longer depends on what is hidden - both halves are on
    /// screen together or neither is - so a hide changes no rectangle. Placing here would only
    /// restate the geometry the panes already have.
    ///
    /// **A list still loading cannot be hidden here** - it has no id yet, so there is nothing to
    /// name. `agents_open_requested` is dropped below so a *later* manifest does not read the
    /// dismissed panel as owing itself a half, but the launch already in flight still arrives.
    /// It is caught where its id first exists: `on_selection` hides a list that announces itself
    /// to a panel that is down, so it joins at the panel's visibility rather than at its own.
    pub(crate) fn hide_panel(&mut self) {
        // Nothing to put away: a hocus in the user's layout is a pane they arranged, and Esc is not
        // a licence to suppress it. There would also be no way back - the reveal bindings summon
        // the overlay panel, not a tab pane.
        if !self.is_panel() {
            return;
        }
        hide_self();
        self.note_visibility(PaneId::Plugin(self.own_plugin_id), false);
        if let Some(agents_id) = self.agents_plugin {
            hide_pane_with_id(PaneId::Plugin(agents_id));
            self.note_visibility(PaneId::Plugin(agents_id), false);
        }
        self.hidden = true;
        self.hide_pending = HIDE_PENDING_FRAMES;
        // A list asked for while the panel was up is not wanted by a panel that is down: it would
        // arrive, unsuppressed and alone, on top of a panel the user had just dismissed.
        self.agents_open_requested = false;
        // And a list the user closed before this is not held against the panel they ask back:
        // the reveal below grows both halves, whatever became of the last one.
        self.agents_closed = false;
        // The bar's highlight swallows every key it does not use, with nothing
        // on screen to say so once the pane is hidden. Shift+Tab is easy to hit
        // on the way to Esc; the next reveal should type again. The released box goes with it,
        // for the same reason: both are ring stops, and a hidden panel is on none of them.
        self.reset_ring_stop();
    }

    /// Put the ring back on the input box, typing.
    ///
    /// The two fields that say where in the ring this half stands - the bar's highlight and the
    /// released box - only mean anything while the keyboard is here. Whenever it is not, or is
    /// about not to be, the stop resets: arriving at the conversation from anywhere, by any
    /// route, should be the box, ready to type. Called by `hide_panel`, by `on_panes` when the
    /// focus has gone elsewhere, and by the ring steps that leave this half.
    pub(crate) fn reset_ring_stop(&mut self) {
        self.chat_tab_focus = None;
        self.chat_input_released = false;
    }

    /// Record a ring step across the seam for the tests - see [`State::ring_focus_calls`].
    /// Compiled away outside them.
    fn note_ring_focus(&mut self, pane: PaneId, stop: &'static str) {
        #[cfg(test)]
        self.ring_focus_calls.push((pane, stop));
        #[cfg(not(test))]
        let _ = (pane, stop);
    }

    /// Hand the keyboard to the agent list, landing on `stop`.
    ///
    /// The two halves of the panel are four stops of one ring, and this is the crossing hocus
    /// can make on its own: it holds the list's plugin id (`agents_plugin`, off
    /// `AGENT_STRIP_YIELD`'s mirror `AGENT_SELECTION`), so it moves the keyboard itself and
    /// only *tells* the list which of its two halves the arriving keys belong to.
    ///
    /// **A ring step never grows the panel a half it is missing.** With no list there is nothing
    /// to step to, and the answer is `false` so the caller can wrap within this half instead.
    /// Opening one is what a reveal does; walking the ring is not a request for a bigger panel.
    fn hand_keyboard_to_agents(&mut self, stop: &'static str) -> bool {
        let Some(agents_id) = self.agents_plugin.filter(|_| self.is_panel()) else {
            return false;
        };
        let pane = PaneId::Plugin(agents_id);
        // Leaving, so this half's stop is spent: coming back should be the box.
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

    /// Tab off the input box: forward to the agent list, or round to the tab bar with no list.
    ///
    /// Only reached with nothing to complete - a blank box, or one the user released with Esc -
    /// so the box's own claim on Tab is already settled by the time this is called.
    pub(crate) fn ring_forward_from_input(&mut self) -> bool {
        if self.hand_keyboard_to_agents(hocus_proto::wire::STOP_LIST) {
            return true;
        }
        // A panel with one half is a ring with two stops, and Tab wraps round it.
        self.chat_input_released = false;
        self.chat_tab_focus = Some(self.chat_tab);
        true
    }

    /// Shift+Tab off the tab bar: back to the agent list's history half, or round to the box.
    pub(crate) fn ring_back_from_tab_bar(&mut self) -> bool {
        if self.hand_keyboard_to_agents(hocus_proto::wire::STOP_HISTORY) {
            return true;
        }
        self.reset_ring_stop();
        true
    }

    /// `AGENT_STRIP_YIELD`: the ring stepped off the agent list, so take the keyboard.
    ///
    /// The mirror of [`State::hand_keyboard_to_agents`], and asymmetric with it because the
    /// panel is: the list knows only its own plugin id, so it cannot focus us - it asks, and
    /// this is the answer. `show_self` is the focus, being this half's own pane.
    pub(crate) fn take_ring_focus(&mut self, stop: Option<&str>) -> bool {
        // The request is a broadcast - the list has no id to address it with - so it reaches
        // every hocus there is, including one the user put in their own layout. That instance
        // is on no ring and must not be yanked the keyboard by a Tab pressed in the panel.
        if !self.is_panel() {
            return false;
        }
        // Nor may a Tab reveal a panel that is down, any more than an Esc may launch one to
        // hide it. `show_self` below would do exactly that, and the only way to get here with
        // the panel hidden is a list racing its own Esc.
        if self.hidden {
            return false;
        }
        self.chat_input_released = false;
        self.chat_tab_focus = match stop {
            Some(hocus_proto::wire::STOP_TABS) => Some(self.chat_tab),
            _ => None,
        };
        // The stop above outlives the manifests still in flight: `on_panes` reads a half going
        // unfocused as the user leaving and drops the stop, and the updates generated before
        // the focus below still say exactly that. See `RING_FOCUS_PENDING_FRAMES`.
        self.ring_focus_pending = RING_FOCUS_PENDING_FRAMES;
        show_self(true);
        true
    }

    /// Put the panel back, this half having been revealed from outside - the `c` keybinding, or
    /// `show_self` from a tool.
    ///
    /// **Both halves come back**, because the panel has one visibility: the list is unsuppressed
    /// beside us, or launched if there is none. Without focus either way - the user named the
    /// conversation, and the keyboard belongs where they put it.
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
    pub(crate) fn reveal_panel(&mut self) {
        // A tiled hocus is never hidden - `hide_panel` refuses - so there is nothing to reveal, and
        // growing it a second half would be the panel opening itself inside someone's layout.
        if !self.is_panel() {
            return;
        }
        self.hidden = false;
        self.hide_pending = 0;
        // A launch that never announced itself would otherwise hold the claim for the rest of the
        // session and every reveal after it would find the panel owing a half it can never ask
        // for. Nothing in the plugin can see a load that failed, so the user's own gesture is the
        // recovery: this is the one that re-asks. Before the `None` arm, which reads the flag.
        self.agents_open_requested = false;
        match self.agents_plugin {
            Some(agents_id) => {
                show_pane_with_id(PaneId::Plugin(agents_id), false, false);
                self.note_visibility(PaneId::Plugin(agents_id), true);
            },
            // Nothing to unsuppress: the panel has to grow its other half first. The placement
            // below still runs - the list announces itself with an id, and `on_panes` places it
            // then.
            None => self.open_agents_pane(),
        }
        self.apply_overlay_geometry();
    }

    /// Put *this* half back beside a list that was revealed without us - the `a` keybinding.
    ///
    /// `show_self` is not what this wants: it lowers to `Action::FocusPluginPaneWithId`, which
    /// would take the keyboard off the list the user just asked for. `show_pane_with_id` with
    /// `should_focus_pane` false is the unsuppress on its own.
    ///
    /// Deliberately not the whole of `reveal_panel`: the list is already on screen, so there is
    /// no other half to summon, and asking for one would be asking for a second list.
    pub(crate) fn reveal_self_beside_agents(&mut self) {
        // Its one caller is already inside `on_panes`'s panel gate; stated here too because every
        // other entry point into the panel says it, and a half-guarded set is the one that grows a
        // hole later.
        if !self.is_panel() {
            return;
        }
        self.hidden = false;
        self.hide_pending = 0;
        // The list on screen is the one that was asked for, whatever hocus last claimed - and a
        // stale claim held here is a panel that can never grow its half again. See `reveal_panel`.
        self.agents_open_requested = false;
        show_pane_with_id(PaneId::Plugin(self.own_plugin_id), false, false);
        self.note_visibility(PaneId::Plugin(self.own_plugin_id), true);
        self.apply_overlay_geometry();
    }

    /// Put the panel back because *hocus* asked - a job that fired with `show: true`.
    ///
    /// `show_self` on its own is not enough, and that was a bug: it reveals this half through the
    /// host and tells hocus nothing, so `hidden` stayed true and `hide_pending` - if an Esc had
    /// just been pressed - stayed set. The manifest that followed then showed our pane on screen
    /// while hocus still intended it hidden, which is exactly the shape the stale-hide guard in
    /// `on_panes` swallows, so the reveal was eaten and the panel wedged: no list, no tab
    /// following, and only another Esc to get out of it.
    ///
    /// So the intent is flipped here, at the source, and the rest is `reveal_panel`: both halves
    /// come back, because a job showing the conversation is showing the panel.
    pub(crate) fn show_panel(&mut self) {
        show_self(true);
        self.note_visibility(PaneId::Plugin(self.own_plugin_id), true);
        self.reveal_panel();
    }

    /// Record a `show`/`hide` for the tests - see [`State::visibility_calls`]. Compiled away
    /// outside them.
    pub(crate) fn note_visibility(&mut self, pane: PaneId, shown: bool) {
        #[cfg(test)]
        self.visibility_calls.push((pane, shown));
        #[cfg(not(test))]
        let _ = (pane, shown);
    }

    /// Carry whichever panes are ours to `tab_index`.
    ///
    /// Both in one call so they cannot end up on different tabs - and so they arrive as the
    /// panel they were, seam included: the server links a batch it carries over as one exactly
    /// as it links a batch it places as one. Without focus - a tab switch is not a request to
    /// type into hocus.
    ///
    /// Called when *either* half is on screen, not only when this one is: a hidden conversation
    /// with a visible list still has to bring the list along, or a tab switch strands it.
    pub(crate) fn follow_tab(&mut self, tab_index: usize) {
        // A pane in the layout belongs to its tab. Following the user around would take it off the
        // tab they built it into - the opposite of what an overlay following them achieves.
        if !self.is_panel() {
            return;
        }
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
        self.panel.reset_tour();
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
    /// launches nobody. That matters on load, which is when this is sent: nothing has yet said
    /// whether the panel is up, and a list appearing beside a conversation the user has not
    /// revealed would be the panel opening itself. [`State::open_agents_pane`] is the call that
    /// does open one, made once the manifest has shown the panel on screen.
    ///
    /// It is still worth sending. A hocus that loaded after the list - a reload, a restore, the
    /// `a` keybinding having got here first - has no idea the list exists, and the list's plugin
    /// id is the one thing hocus needs to place it. `hocus-agents` answers `AGENT_STRIP_OPEN` with
    /// its selection, which carries that id.
    pub(crate) fn greet_agents_pane(&self) {
        pipe_message_to_plugin(MessageToPlugin::new(hocus_proto::wire::AGENT_STRIP_OPEN));
    }

    /// Grow the panel's missing half: launch an agent list, or reveal one that is only hidden.
    ///
    /// The same message as [`State::greet_agents_pane`] with the url attached, which is what
    /// makes the host launch-or-reuse rather than merely deliver - the mirror of
    /// `hocus-agents::say_hello_to_hocus`. The config has to match the `a` keybinding's
    /// character for character, since `overlay` and `awaits_placement` are part of plugin
    /// identity: `launch::overlay_configuration` is the one copy of it.
    ///
    /// Unfocused, because every caller is a reveal the user asked for by naming the *other*
    /// half. `awaits_placement` keeps the new pane unpainted until `apply_overlay_geometry`
    /// names it, so it never flashes at the centered default it is born with.
    ///
    /// Asked for at most once per launch: `agents_open_requested` is what stops a manifest
    /// arriving while the list is still loading from asking for a second one.
    pub(crate) fn open_agents_pane(&mut self) {
        // The half a panel owes itself is not a half a tab pane owes anyone: an overlay list
        // launched beside a hocus the user put in their layout lands on top of their session,
        // uninvited. This is the launch the tiled integration tests were tripping.
        if !self.is_panel() {
            return;
        }
        if self.agents_open_requested {
            return;
        }
        self.agents_open_requested = true;
        // Whatever became of the last list, this is a new one: the closure `on_panes` remembered
        // has been answered by the very act of asking again.
        self.agents_closed = false;
        #[cfg(test)]
        {
            self.agents_launches += 1;
        }
        pipe_message_to_plugin(
            MessageToPlugin::new(hocus_proto::wire::AGENT_STRIP_OPEN)
                .with_plugin_url(launch::HOCUS_AGENTS_ALIAS)
                .with_plugin_config(launch::overlay_configuration())
                .new_plugin_instance_should_not_be_focused(),
        );
    }

    /// Take an `AGENT_STRIP_HELLO` and note that this instance is the one it launched.
    ///
    /// The greeting carries hocus's url, so it opens a hocus when there is none to ask - and the
    /// panel is both halves, so that conversation belongs on screen. What it does *not* get is
    /// the focus: the user pressed `a`, and the keyboard belongs on the list. The greeting says
    /// so itself (`new_plugin_instance_should_not_be_focused`), so there is nothing to undo
    /// here - what is recorded is that the list beside us is already open and already ours, so
    /// the first manifest does not go asking for another one.
    ///
    /// Nothing but that launch can reach us before our own first `PaneUpdate`, so `own_tab_index`
    /// being unset is the whole test; `from_agents` is the marker the greeting carries
    /// (`wire::OPENED_BY`), which keeps a greeting from some other sender out of it.
    pub(crate) fn note_greeting_from_agents(&mut self, from_agents: bool) {
        if from_agents && self.own_tab_index.is_none() {
            // Not a launch we made, but the same claim: a list exists and is on its way to
            // announcing its id, so `on_panes` must not ask for a second one in the meantime.
            self.agents_open_requested = true;
        }
    }
}

#[cfg(test)]
#[path = "unit/overlay_tests.rs"]
mod overlay_tests;
