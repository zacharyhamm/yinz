//! Where hocus draws itself: a column in the bottom-left corner of the screen.
//!
//! hocus used to be a *global pane* - a strip carved off the left edge of the session surface,
//! with every tab shrunk to fit beside it. It is now a pair of **overlays**, drawn over the
//! session surface rather than beside it, so a tab keeps its full width and hocus floats above
//! whatever is underneath. `hocus-agents` is the upper of the two; the conversation is the lower.
//!
//! **The plugin computes the geometry, not the server.** `FloatingPaneCoordinates` measures `x`
//! and `y` from the top-left corner only - there is no bottom or right anchoring - so "the bottom
//! left corner, inset by a padding" cannot be stated as a coordinate. What can be stated is the
//! arithmetic, once the display area is known: `y = rows - height - pad`. That is what this module
//! does, on load and again whenever the display area changes, exactly as compact-bar positions its
//! tooltip. The alternative was teaching the server to anchor, which is a protobuf change to
//! `FloatingPaneCoordinates` and a second denominator to keep in step.
//!
//! Two consequences worth knowing:
//!
//! - **The overlays live on a tab, and hocus moves them.** An overlay belongs to a `Tab`, where a
//!   global pane belonged to the session. `on_tabs` moves the pair to whichever tab became active,
//!   which is what keeps hocus present the way the strip used to be. This is also why the server
//!   routes events to overlays in *every* tab and not only the active one - without that, an
//!   overlay left behind on another tab would never hear the `TabUpdate` it moves on.
//! - **Hiding is per pane, so the pair has to be hidden together.** The old strip hid as one unit
//!   because hiding was per *edge*. Here `hide_column` names both panes, and `reveal_column` puts
//!   both back - hocus is the half that knows about both, so both directions are routed through
//!   it.

use std::collections::BTreeMap;

use zellij_tile::prelude::*;

use hocus_proto::launch::{overlay_configuration, HOCUS_AGENTS_ALIAS};

use crate::ui;
use crate::State;

/// Cells of clear screen left around the column on every side.
const DEFAULT_PAD: usize = 2;
/// The column's width, as a percentage of the display area - the width the left-edge global strip
/// used to take, so the change of placement is not also a change of size.
const DEFAULT_WIDTH_PERCENT: usize = 25;
/// The agent list's share of the column. The two-thirds-to-the-conversation split the global pane
/// used.
const DEFAULT_AGENTS_PERCENT: usize = 33;
/// The fewest rows either half of the column may be given: a frame, and a row inside it.
const MIN_HALF_ROWS: usize = 3;

/// A configured size, resolved against `whole`. Accepts `"40%"` and `"12"`, and falls back to
/// `default_percent` of `whole` for anything missing or unparseable - a typo in the KDL costs a
/// default-sized column rather than a crash, which is the rule `hocus-ask` follows for the same
/// kind of key.
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

/// The two rectangles the column is made of: the conversation, then the agent list above it.
///
/// `None` before any `TabUpdate` has said how big the screen is - there is nothing to compute
/// against yet, and a guess would put the column somewhere the user would see it move away from.
pub(crate) fn column_geometry(
    config: &BTreeMap<String, String>,
    rows: usize,
    cols: usize,
) -> Option<(FloatingPaneCoordinates, FloatingPaneCoordinates)> {
    if rows == 0 || cols == 0 {
        return None;
    }
    // Capped at a quarter of the screen so a fat `pad` on a small terminal cannot eat the column
    // it is supposed to be insetting.
    let pad = config
        .get("pad")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_PAD)
        .min(rows / 4);
    let width = resolve(config.get("width"), cols, DEFAULT_WIDTH_PERCENT)
        // an overlay is framed, so the plugin is rendered two cells narrower than the pane
        .max(ui::MIN_COLS + 2)
        .min(cols.saturating_sub(pad * 2).max(1));

    // What is left of the screen once the padding is taken off the top and the bottom.
    let column = rows.saturating_sub(pad * 2);
    // The smallest either half is allowed to be. A frame costs two rows, so this is the frame plus
    // one row of content: below it the pane draws nothing but "pane too small", but it is at least
    // *there*, in the column, where the user is looking for it.
    let floor = MIN_HALF_ROWS.min(column / 2).max(1);
    let agents_height = resolve(config.get("agents_height"), column, DEFAULT_AGENTS_PERCENT)
        .clamp(floor, column.saturating_sub(floor).max(floor));
    // The conversation takes the rest. The two heights sum to the column exactly - clamping here
    // rather than letting the rectangles overlap is what keeps the agent list off the top of the
    // transcript, since they are separate overlays and nothing else would stop them. Neither may
    // reach zero: `FloatingPaneCoordinates::new` rejects a zero height, and the default that comes
    // back in its place is half the surface, centered - the overlay would jump to the middle of
    // the screen rather than simply being small.
    let chat_height = column.saturating_sub(agents_height).max(1);
    let agents_height = column.saturating_sub(chat_height).max(1);

    let chat_y = rows.saturating_sub(chat_height + pad);
    let agents_y = chat_y.saturating_sub(agents_height);

    // Fixed rather than percentages throughout: these are already resolved against the display
    // area we were told about, and a percentage would be resolved a second time by the server -
    // against the *session surface*, which is the display area minus any global pane strips, a
    // different denominator.
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
    Some((
        rect(pad, chat_y, width, chat_height),
        rect(pad, agents_y, width, agents_height),
    ))
}

/// The tab the column should move to on this client's `TabUpdate`, if any.
///
/// **The column moves with the client it is sitting with, and with nobody else.** hocus is a
/// single shared instance, but the host builds one `TabUpdate` per client - `active` is *that*
/// client's active tab - and routes every one of them to every overlay. Obeying all of them is an
/// unbounded loop: with two clients on different tabs, one update drags the column across, the
/// move makes the server report session state again, the other client's update drags it back, and
/// so on for as long as both stay attached.
///
/// So a client only gets to move us if they were on our tab to begin with. Three properties fall
/// out of that, and they are what make this the whole of the fix:
///
/// - A client on some other tab never matches, so they can never pull the column off the client
///   who has it.
/// - The move is self-cancelling: `follow_tab` sets `own_tab_index` to where we landed, so the
///   report the move provokes re-evaluates to `None` rather than to another move.
/// - Authority transfers on its own. `move_to_focused_tab` on the `a` and `c` keybindings puts the
///   column on the summoning client's tab, at which point their entry matches and the previous
///   client's no longer does - no owner has to be recorded anywhere.
pub(crate) fn tab_to_follow(
    own_tab: Option<usize>,
    client_was_on: Option<usize>,
    client_is_on: usize,
) -> Option<usize> {
    let own = own_tab?;
    (client_was_on == Some(own) && client_is_on != own).then_some(client_is_on)
}

impl State {
    /// Re-place the column against the display area as it is now.
    ///
    /// Safe to call whenever: `change_overlay_coordinates` names panes by id and does nothing for
    /// an id that is not an overlay, and once the user has dragged an overlay the server keeps
    /// their position and takes only the size from here.
    pub(crate) fn apply_overlay_geometry(&self) {
        let Some((chat, agents)) =
            column_geometry(&self.kdl, self.display_area_rows, self.display_area_cols)
        else {
            return;
        };
        let mut placements = vec![(PaneId::Plugin(self.own_plugin_id), chat)];
        if let Some(agents_id) = self.agents_plugin {
            placements.push((PaneId::Plugin(agents_id), agents));
        }
        change_overlay_coordinates(placements);
    }

    /// Hide the column - this pane and the agent list above it - without unloading either.
    ///
    /// The old global strip hid as one unit because hiding was per edge. An overlay is hidden by
    /// name, so the pairing is stated here instead. The agent list goes first: after `hide_self`
    /// this pane is no longer focused, and there is no reason to leave the list on screen for the
    /// intervening frame.
    pub(crate) fn hide_column(&mut self) {
        if let Some(agents_id) = self.agents_plugin {
            hide_pane_with_id(PaneId::Plugin(agents_id));
        }
        hide_self();
        self.hidden = true;
        self.hide_pending = true;
    }

    /// Put the column back, having been revealed from outside - the `c` keybinding, or `show_self`.
    ///
    /// Only the agent list needs showing: whatever revealed hocus revealed *this* pane. The list
    /// is shown without focus, so the keyboard stays on the conversation the user asked for;
    /// `focus_agents_pane` is the path that means to move it.
    pub(crate) fn reveal_column(&mut self) {
        self.hidden = false;
        self.hide_pending = false;
        if let Some(agents_id) = self.agents_plugin {
            show_pane_with_id(PaneId::Plugin(agents_id), false, false);
        }
        // This client's tab rather than `self.tabs`' active flag: the flag is whatever the
        // last-arriving client's `TabUpdate` said, so with several attached it names nobody in
        // particular. `active_tab_index` remains the fallback for a reveal that arrives before any
        // tab state has - with one client attached the two answers are the same.
        let target = self
            .client_tabs
            .get(&self.client)
            .copied()
            .or_else(|| self.active_tab_index());
        if let Some(target) = target {
            self.follow_tab(target);
        }
        self.apply_overlay_geometry();
    }

    /// The zero-based index of the active tab, as the last `TabUpdate` reported it.
    pub(crate) fn active_tab_index(&self) -> Option<usize> {
        self.tabs
            .iter()
            .find(|tab| tab.active)
            .map(|tab| tab.position)
    }

    /// Carry the column to `tab_index`.
    ///
    /// Both panes in one call so they cannot end up on different tabs, and without focus - a tab
    /// switch is not a request to type into hocus.
    ///
    /// Never called while hidden. A hidden overlay that is moved arrives on the new tab *visible*,
    /// because the hidden flag is in-memory state that does not survive the move; `reveal_column`
    /// is what brings a hidden column back, and it moves the pair as part of doing so.
    pub(crate) fn follow_tab(&mut self, tab_index: usize) {
        // Nothing to do until `PaneUpdate` has said where we actually are. Before that the column
        // is wherever it was opened - which is the active tab, since every path that opens it is
        // the user asking for it from the tab they are on - and a move to "the active tab" would
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
        // the destination resolves the coordinates against its own surface, but the heights are
        // ours to restate - nothing else knows the column's proportions
        self.apply_overlay_geometry();
    }

    /// Open the agent list as the overlay above this one - or reveal the one that is already there.
    ///
    /// `MessageToPlugin` with a `plugin_url` is launch-or-reuse: `get_or_load_plugins` hands the
    /// message to an existing instance with the same (location, configuration) rather than
    /// spawning a second one, and even to one that is still loading. That is the same idempotence
    /// the old `launch_or_focus_global_pane_split` had, and the reason this is not
    /// `open_plugin_pane_floating`, which always creates a pane.
    pub(crate) fn launch_agents_pane(&mut self, should_focus: bool) {
        if let Some(agents_id) = self.agents_plugin {
            show_pane_with_id(PaneId::Plugin(agents_id), false, should_focus);
            self.apply_overlay_geometry();
            return;
        }
        let mut message = MessageToPlugin::new(hocus_proto::wire::AGENT_STRIP_OPEN)
            .with_plugin_url(HOCUS_AGENTS_ALIAS)
            .with_plugin_config(overlay_configuration());
        if let Some((_, agents)) =
            column_geometry(&self.kdl, self.display_area_rows, self.display_area_cols)
        {
            message = message.with_floating_pane_coordinates(agents);
        }
        pipe_message_to_plugin(message);
    }
}

#[cfg(test)]
#[path = "unit/overlay_tests.rs"]
mod overlay_tests;
