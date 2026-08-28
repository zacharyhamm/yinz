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
/// The fewest rows a pane may be given: a frame, and a row inside it.
const MIN_ROWS: usize = 3;

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
    let (rows, cols) = (surface_rows, surface_cols);
    if rows == 0 || cols == 0 {
        return None;
    }
    // Capped at a quarter of the screen on *both* axes, so a fat `pad` on a small terminal cannot
    // eat the panes it is supposed to be insetting. Capping against the rows alone was not enough:
    // `pad` is the rectangles' `x` as well as their inset off the top and bottom, so on a tall,
    // narrow display a rows-only cap let it exceed the column count outright and put the panes off
    // the right-hand edge of the screen.
    let pad = config
        .get("pad")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_PAD)
        .min(rows / 4)
        .min(cols / 4);
    // What is between the paddings, and how much of it one pane may take. With the conversation
    // on screen the two panes and the gap between them are laid end to end, so neither may exceed
    // half of what is left once the gap is taken off - a `width` wide enough for one is not wide
    // enough for two, and the list would otherwise run off the right-hand edge. Alone, a pane may
    // take the whole of it: there is no gap to leave room for.
    let across = cols.saturating_sub(pad * 2).max(1);
    let each = match chat_visible {
        true => across.saturating_sub(pad) / 2,
        false => across,
    };
    let width = resolve(config.get("width"), cols, DEFAULT_WIDTH_PERCENT)
        // an overlay is framed, so the plugin is rendered two cells narrower than the pane
        .max(ui::MIN_COLS + 2)
        .min(each.max(1));

    // What is left of the screen once the padding is taken off the top and the bottom, and the
    // share of it a pane actually takes. A pane is anchored to the *bottom*, so a height short of
    // the whole leaves the top of the screen alone rather than the bottom.
    let available = rows.saturating_sub(pad * 2);
    let height = resolve(config.get("height"), rows, DEFAULT_HEIGHT_PERCENT)
        .clamp(MIN_ROWS.min(available).max(1), available.max(1));
    let y = rows.saturating_sub(height + pad);

    // Fixed rather than percentages throughout: these are already resolved against the surface we
    // were told about, and a percentage would be resolved a second time by the server. The two
    // denominators would agree now that `surface_of` reads the session surface, but stating the
    // arithmetic once is still the point - the panes are anchored to the *bottom*, which no
    // percentage can say.
    let rect = |x: usize, y: usize, w: usize, h: usize| {
        FloatingPaneCoordinates::new(
            Some(x.to_string()),
            Some(y.to_string()),
            Some(w.to_string()),
            Some(h.to_string()),
            None,
            None,
        )
        .unwrap_or_default()
    };
    let agents_x = match chat_visible {
        // One padding of clear screen between the two, the same padding that insets the pair from
        // the edges of the screen. They used to meet frame to frame; two framed panels touching
        // read as one panel with a line down it, and the gap is what makes them two things sitting
        // beside each other. One value rather than a second key, so a `pad` the user changes moves
        // the panels and the seam together.
        true => pad + width + pad,
        false => pad,
    };
    Some((
        rect(pad, y, width, height),
        rect(agents_x, y, width, height),
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
        let Some((chat, agents)) = overlay_geometry(
            &self.kdl,
            self.surface_rows,
            self.surface_cols,
            !self.hidden,
        ) else {
            return;
        };
        let mut placements = vec![(PaneId::Plugin(self.own_plugin_id), chat)];
        if let Some(agents_id) = self.agents_plugin {
            placements.push((PaneId::Plugin(agents_id), agents));
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

    /// Put the conversation away without unloading it, leaving the agent list where it is.
    ///
    /// The two panes are separate surfaces, so Esc here means this pane and nothing else. What it
    /// does reach is the list's *position*: with the conversation gone the left-hand slot is free,
    /// and `apply_overlay_geometry` moves the list into it.
    pub(crate) fn hide_self_only(&mut self) {
        hide_self();
        self.hidden = true;
        self.hide_pending = true;
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
        if self.agents_plugin.is_some() {
            // Both ids went in one call, so the list is wherever we are. Recorded here rather than
            // waited for, so the next `PaneUpdate` - taken before the move, possibly - does not
            // read the two as parted and issue a second move.
            self.agents_tab_index = Some(tab_index);
        }
        // the destination resolves the coordinates against its own surface, but the sizes are
        // ours to restate - nothing else knows the panes' proportions
        self.apply_overlay_geometry();
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
