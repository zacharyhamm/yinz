//! Which of the ten multiplayer colors each attached client draws in.
//!
//! Client ids are never reused, so they grow without bound and cannot index the ten
//! `player_N` colors themselves. Each attached client holds a [`ColorSlot`] instead: the lowest
//! free one when it attaches, kept until it leaves.

use crate::data::PaletteColor;
use crate::internal_ids::ClientId;
use crate::style::MultiplayerColors;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::num::NonZeroU8;

/// One of the multiplayer colors `player_1` to `player_10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct ColorSlot(NonZeroU8);

impl ColorSlot {
    pub const COUNT: u8 = 10;

    /// `None` outside `1..=10`.
    pub fn new(slot: u8) -> Option<Self> {
        NonZeroU8::new(slot)
            .filter(|slot| slot.get() <= Self::COUNT)
            .map(Self)
    }
    pub fn get(self) -> u8 {
        self.0.get()
    }
    /// The slot's (primary, secondary) colors.
    pub fn colors(self, colors: MultiplayerColors) -> (PaletteColor, PaletteColor) {
        let primary = match self.get() {
            1 => colors.player_1,
            2 => colors.player_2,
            3 => colors.player_3,
            4 => colors.player_4,
            5 => colors.player_5,
            6 => colors.player_6,
            7 => colors.player_7,
            8 => colors.player_8,
            9 => colors.player_9,
            _ => colors.player_10,
        };
        (
            primary,
            PaletteColor::EightBit(crate::colors::default_colors::BLACK),
        )
    }
}

impl TryFrom<u8> for ColorSlot {
    type Error = String;
    fn try_from(slot: u8) -> Result<Self, Self::Error> {
        Self::new(slot).ok_or_else(|| format!("color slot {slot} is outside 1..=10"))
    }
}

impl From<ColorSlot> for u8 {
    fn from(slot: ColorSlot) -> u8 {
        slot.get()
    }
}

/// The clients attached to a session, each with the color slot it holds while attached. A
/// client that attaches takes the lowest free slot; once all ten are taken, later clients have
/// none until one frees up, and keep none even then.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttachedClients {
    slots: BTreeMap<ClientId, Option<ColorSlot>>,
}

impl AttachedClients {
    /// Attach a client, giving it the lowest free slot. `false` if it was already attached, in
    /// which case it keeps the slot it has.
    pub fn insert(&mut self, client_id: ClientId) -> bool {
        if self.slots.contains_key(&client_id) {
            return false;
        }
        let free = (1..=ColorSlot::COUNT)
            .filter_map(ColorSlot::new)
            .find(|slot| !self.slots.values().any(|held| *held == Some(*slot)));
        self.slots.insert(client_id, free);
        true
    }
    pub fn remove(&mut self, client_id: &ClientId) -> bool {
        self.slots.remove(client_id).is_some()
    }
    pub fn contains(&self, client_id: &ClientId) -> bool {
        self.slots.contains_key(client_id)
    }
    pub fn len(&self) -> usize {
        self.slots.len()
    }
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
    pub fn clear(&mut self) {
        self.slots.clear();
    }
    /// The attached clients, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &ClientId> {
        self.slots.keys()
    }
    pub fn color_slot(&self, client_id: &ClientId) -> Option<ColorSlot> {
        self.slots.get(client_id).copied().flatten()
    }
    /// A copy to render with, so a render pass need not hold a borrow of the shared set.
    pub fn snapshot(&self) -> MultiplayerSlots {
        let mut holders = [None; ColorSlot::COUNT as usize];
        for (client_id, slot) in &self.slots {
            if let Some(slot) = slot {
                holders[usize::from(slot.get() - 1)] = Some(*client_id);
            }
        }
        MultiplayerSlots {
            attached: self.slots.len(),
            holders,
        }
    }
}

/// Attaches the clients in id order, so that a set built from any collection is the same.
impl FromIterator<ClientId> for AttachedClients {
    fn from_iter<I: IntoIterator<Item = ClientId>>(clients: I) -> Self {
        let mut clients: Vec<ClientId> = clients.into_iter().collect();
        clients.sort_unstable();
        let mut attached = AttachedClients::default();
        for client_id in clients {
            attached.insert(client_id);
        }
        attached
    }
}

/// Attaches each client in turn, in the order given.
impl Extend<ClientId> for AttachedClients {
    fn extend<I: IntoIterator<Item = ClientId>>(&mut self, clients: I) {
        for client_id in clients {
            self.insert(client_id);
        }
    }
}

/// [`AttachedClients`] as it stood when a render began.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MultiplayerSlots {
    attached: usize,
    holders: [Option<ClientId>; ColorSlot::COUNT as usize],
}

impl MultiplayerSlots {
    /// A single client draws in the selected-frame color rather than a player color.
    pub fn multiple_users(&self) -> bool {
        self.attached > 1
    }
    pub fn color_slot(&self, client_id: ClientId) -> Option<ColorSlot> {
        let index = self
            .holders
            .iter()
            .position(|held| *held == Some(client_id))?;
        ColorSlot::new(u8::try_from(index + 1).ok()?)
    }
}

#[cfg(test)]
#[path = "unit/multiplayer_tests.rs"]
mod multiplayer_tests;
