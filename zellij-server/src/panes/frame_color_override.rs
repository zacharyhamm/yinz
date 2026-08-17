//! The one slot a pane keeps for "draw my frame differently for a while".

use crate::ClientId;
use zellij_utils::data::PaletteColor;

/// A pane frame drawn in a colour other than the one focus would give it, and
/// optionally under a title other than its own.
///
/// A pane has at most one of these at a time — a bell flash, a red exit status
/// and a plugin's highlight all compete for the same slot, last writer wins.
/// That is the long-standing contract; the only thing recorded here beyond the
/// colour and the text is *who* the override is for.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameColorOverride {
    pub color: PaletteColor,
    /// Replaces the pane title while the override is up. `None` leaves the
    /// pane's own title alone.
    pub text: Option<String>,
    /// The one client that sees it, or `None` for every client.
    ///
    /// A bell flash and a failed command are facts about the pane and belong to
    /// everyone. A plugin marking a pane for the person it is talking to is a
    /// fact about that conversation, and putting it on another client's screen
    /// would be telling them about something they did not ask for.
    pub client_id: Option<ClientId>,
}

impl FrameColorOverride {
    pub fn new(color: PaletteColor, text: Option<String>, client_id: Option<ClientId>) -> Self {
        FrameColorOverride {
            color,
            text,
            client_id,
        }
    }

    /// Whether `client_id` is one of the clients this override is drawn for.
    pub fn applies_to(&self, client_id: ClientId) -> bool {
        self.client_id.is_none_or(|owner| owner == client_id)
    }

    /// Whether a clear aimed at `client_id` should take this override down.
    ///
    /// A clear with no client is unconditional: it is how the bell flash and the
    /// pane's own teardown put the slot back, and neither knows about clients.
    /// A clear *for* a client must not silently drop an override every client is
    /// looking at.
    pub fn is_cleared_by(&self, client_id: Option<ClientId>) -> bool {
        match client_id {
            Some(client_id) => self.client_id == Some(client_id),
            None => true,
        }
    }
}
