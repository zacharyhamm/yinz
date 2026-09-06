//! The declarative half of a right-click context menu: the items a user wrote in their config.
//!
//! ```kdl
//! context_menu {
//!     "Copy" {
//!         Copy
//!     }
//!     separator
//!     "Close pane" {
//!         CloseFocus
//!     }
//! }
//! ```
//!
//! A node's name is the label and its children are actions, parsed by the very same
//! `crate::kdl::action::from_kdl((&KdlNode, &Options))` a keybinding's action list goes through - so anything
//! bindable to a key is puttable on a menu, with no second action vocabulary to keep in step.
//!
//! **A `context_menu` block replaces the defaults; it does not add to them.** This diverges from
//! `load_plugins`, deliberately: a menu somebody wrote out is the menu they want, and an additive
//! rule would pin the shipped items to the top of every custom menu with no way to drop just one.
//! An empty block therefore means "no configured items", which is a perfectly good thing to write
//! when the `context_menu` hook plugin is supplying everything.
//!
//! The plugin never sees the `Action`s - see `data::ContextMenuItem` for why, and for what it sees
//! instead.

use serde::{Deserialize, Serialize};

use crate::data::ContextMenuItem;
use crate::geometry::Direction;
use crate::input::actions::Action;

fn new_pane(direction: Direction) -> Action {
    Action::NewPane {
        direction: Some(direction),
        pane_name: None,
        start_suppressed: false,
    }
}

/// The configured menu, in the order it was written.
///
/// A `Vec` rather than the `BTreeMap` `Hooks` uses: a menu's order is a thing the user chose, and
/// `KdlDocument::nodes()` is already deterministic, so nothing is bought by sorting it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenu {
    items: Vec<ContextMenuItem>,
}

/// The menu a session has when the config says nothing. Modest on purpose: these are the actions
/// that mean the same thing over any pane, so none of them is ever the wrong offer.
impl Default for ContextMenu {
    fn default() -> Self {
        ContextMenu::defaults()
    }
}

impl ContextMenu {
    pub fn defaults() -> Self {
        ContextMenu::from_items(vec![
            ContextMenuItem::new("Copy", vec![Action::Copy]),
            ContextMenuItem::separator(),
            ContextMenuItem::new("Split right", vec![new_pane(Direction::Right)]),
            ContextMenuItem::new("Split down", vec![new_pane(Direction::Down)]),
            ContextMenuItem::new("Fullscreen", vec![Action::ToggleFocusFullscreen]),
            ContextMenuItem::separator(),
            ContextMenuItem::new("Move to new tab", vec![Action::BreakPane]),
            ContextMenuItem::new("Close pane", vec![Action::CloseFocus]),
        ])
    }

    /// Ids are assigned here rather than by the parser so that every route into a `ContextMenu` -
    /// config, defaults, a test - produces the same identity scheme: position in the list. That is
    /// what the plugin sends back, and what re-attaches an item's actions on its return.
    pub fn from_items(items: Vec<ContextMenuItem>) -> Self {
        ContextMenu {
            items: items
                .into_iter()
                .enumerate()
                .map(|(index, item)| item.with_id(index as u64))
                .collect(),
        }
    }

    pub fn items(&self) -> &[ContextMenuItem] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The items as the hook plugin should see them: identified, but stripped of the actions the
    /// server keeps to itself.
    pub fn items_for_plugin(&self) -> Vec<ContextMenuItem> {
        self.items
            .iter()
            .map(|item| item.without_actions())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_menu_identifies_every_item_by_position() {
        let menu = ContextMenu::defaults();
        for (index, item) in menu.items().iter().enumerate() {
            assert_eq!(item.id, Some(index as u64));
        }
    }

    #[test]
    fn the_items_a_plugin_sees_carry_ids_but_no_actions() {
        let menu = ContextMenu::defaults();
        let for_plugin = menu.items_for_plugin();
        assert_eq!(for_plugin.len(), menu.items().len());
        for (sent, configured) in for_plugin.iter().zip(menu.items()) {
            assert_eq!(sent.id, configured.id);
            assert_eq!(sent.label, configured.label);
            assert!(
                sent.actions.is_empty(),
                "actions must not cross the plugin boundary - see data::ContextMenuItem"
            );
        }
    }
}
