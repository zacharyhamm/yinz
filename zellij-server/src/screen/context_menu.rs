//! The `Screen` half of right-click context menus.
//!
//! `ui::context_menu` decides where a menu goes and what it looks like; this decides when one
//! exists, who owns it, what it does when clicked, and when the cells underneath have to be
//! repainted. The split is the one `screen::notifications` and `ui::notification` already keep.
//!
//! Three properties are worth stating up front, because each is load-bearing and none is obvious
//! from the code that implements it:
//!
//! - **The clicked pane keeps focus for the menu's whole life.** A menu is not a pane and takes
//!   nothing; `Copy`, `CloseFocus` and every other focus-relative action therefore mean the pane
//!   the user right-clicked. This is the single reason the menu is drawn here rather than being an
//!   overlay pane. Nothing *stops* a plugin focusing elsewhere while a menu is up, so
//!   [`Screen::dispatch_context_menu_actions`] restores the focus before running an item rather
//!   than trusting that it held - and it runs a queue-turn later than the click that activated the
//!   item, so that a focus change already queued behind that click cannot land between the two.
//! - **A menu draws nothing until it knows what it says.** The plugin hook answers asynchronously,
//!   so a fresh menu is `AwaitingPlugin` and paints no cells at all. `BackgroundJob::ContextMenuTimeout`
//!   promotes it to the configured items if no answer arrives, which is what stops an unbound or
//!   wedged hook from making right-click silently do nothing.
//! - **Closing forces a repaint.** Nothing underneath a menu is dirty when it goes, so its cells
//!   would keep its glyphs. [`Screen::close_context_menu`] force-renders every tab and the global
//!   panes, exactly as `advance_notification_animations` does when a toast leaves.

use std::thread;

use zellij_utils::data::{ContextMenuContext, ContextMenuItem, Event, KeyWithModifier};
use zellij_utils::errors::prelude::*;
use zellij_utils::input::actions::Action;
use zellij_utils::input::mouse::{MouseEvent, MouseEventType};
use zellij_utils::pane_size::Viewport;
use zellij_utils::position::Position;

use super::{ContextMenuItems, ContextMenuState, Screen, ScreenInstruction};
use crate::background_jobs::BackgroundJob;
use crate::output::Output;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::route::route_action;
use crate::ui::context_menu::{
    context_menu_chunks, context_menu_contains, context_menu_item_at, context_menu_layout,
    next_selectable, ContextMenuLayout,
};
use crate::ClientId;

/// How long a menu waits for its hook plugin before falling back to the configured items.
///
/// Short enough that a wedged plugin is not a visible stall, long enough that a plugin doing
/// ordinary work - a few `BTreeMap` lookups and a filter - always wins the race. A plugin that
/// answers late is not wrong, only late: its answer arrives, matches the still-open menu's
/// `request_id`, and replaces the fallback list in place.
pub(crate) const CONTEXT_MENU_PLUGIN_TIMEOUT_MS: u64 = 100;

/// Give a plugin's items their behaviour back.
///
/// An item carrying an `id` this menu was opened with gets that item's `actions`; anything else -
/// one the plugin invented, or one whose `id` names nothing - keeps none and acts through its
/// `context` instead. Everything the plugin *can* see wins, so relabelling a configured item
/// (`Copy "foo"`) works without having to rebuild it.
///
/// Here rather than on `ContextMenu` because the authority is the list this menu was opened with,
/// which the menu is holding; the configuration could have been reloaded since.
fn reattach_actions(
    configured: &[ContextMenuItem],
    items: Vec<ContextMenuItem>,
) -> Vec<ContextMenuItem> {
    items
        .into_iter()
        .map(|mut item| {
            if let Some(source) = item
                .id
                .and_then(|id| configured.iter().find(|source| source.id == Some(id)))
            {
                item.actions = source.actions.clone();
            }
            item
        })
        .collect()
}

impl Screen {
    /// Open a menu for `client_id`, replacing any it already had.
    ///
    /// Returns the `request_id` the hook must answer with. The caller fires the hook - this does
    /// not, because the plugin thread is reachable from the mouse path and the timeout scheduler
    /// is not, and splitting them keeps this function testable without either.
    pub(crate) fn open_context_menu(
        &mut self,
        mut context: ContextMenuContext,
        client_id: ClientId,
    ) -> u64 {
        let request_id = self.next_context_menu_request_id;
        self.next_context_menu_request_id += 1;
        context.request_id = request_id;
        context.client_id = client_id;

        let anchor = Position::new(context.click.1 as i32, context.click.0 as u16);
        // The one place the configured items enter a menu. The tab that fired this knows the pane
        // and nothing about the configuration; filling them in here is what lets the fallback and
        // the re-attachment below both work off the same list the plugin was shown.
        context.items = self.context_menu.items_for_plugin();
        let configured = self.context_menu.items().to_vec();
        // Replacing an existing menu rather than stacking: two menus for one client have no
        // meaning, and the old one's cells are repainted by the same force-render a close does.
        if self.context_menus.contains_key(&client_id) {
            self.force_render_under_context_menu();
        }
        self.context_menus.insert(
            client_id,
            ContextMenuState {
                request_id,
                anchor,
                pane_id: context.pane_id.into(),
                context,
                plugin_id: None,
                items: ContextMenuItems::AwaitingPlugin { configured },
                selected: None,
            },
        );
        request_id
    }

    /// Open a menu and ask the hook plugin what it should say.
    ///
    /// The two halves are split deliberately - `open_context_menu` is state, this is the world -
    /// so a test can drive one without standing up the other.
    pub(crate) fn request_context_menu(
        &mut self,
        context: ContextMenuContext,
        client_id: ClientId,
    ) {
        let request_id = self.open_context_menu(context, client_id);
        let Some(menu) = self.context_menus.get(&client_id) else {
            return;
        };
        let context = menu.context.clone();
        // The plugin thread answers this even when no hook is bound - with the configured items -
        // so that "is a hook configured" is asked in the one place the configuration lives, the
        // way every other hook site already works.
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::RequestContextMenu {
                context: Box::new(context),
                client_id,
            })
            .non_fatal();
        // Armed regardless: a plugin that is loading, wedged, or simply slow must not be able to
        // make right-click do nothing at all.
        self.bus
            .senders
            .send_to_background_jobs(BackgroundJob::ContextMenuTimeout(
                request_id,
                client_id,
                CONTEXT_MENU_PLUGIN_TIMEOUT_MS,
            ))
            .non_fatal();
    }

    /// Take the plugin's answer, if it is still the answer to a question this client is asking.
    ///
    /// A `request_id` that does not match is dropped in silence: it belongs to a menu the user has
    /// already dismissed or replaced, and acting on it would put a stale list on screen.
    pub(crate) fn set_context_menu_items(
        &mut self,
        request_id: u64,
        items: Vec<ContextMenuItem>,
        plugin_id: Option<u32>,
        client_id: ClientId,
    ) {
        let Some(menu) = self.context_menus.get_mut(&client_id) else {
            return;
        };
        if menu.request_id != request_id {
            return;
        }
        // The plugin was handed items with ids and no actions, so this is where the behaviour goes
        // back on. Matching against what *this menu* was opened with rather than the live config:
        // the two are normally the same, but a menu should do what it offered when it opened.
        let configured = match &menu.items {
            ContextMenuItems::AwaitingPlugin { configured }
            | ContextMenuItems::Fallback { configured, .. } => configured,
            ContextMenuItems::Ready(_) => {
                // A second answer to a menu already showing one. Nothing to re-attach against,
                // and the first answer is the one the user is looking at.
                return;
            },
        };
        let items = reattach_actions(configured, items);
        menu.selected = None;
        menu.items = ContextMenuItems::Ready(items);
        // Remembered so a selection goes back to whoever offered it. `None` here is the plugin
        // thread answering from the configuration, which is nobody.
        menu.plugin_id = plugin_id;
        // An answer of nothing at all is a plugin saying "no menu here", not an empty box.
        if menu.items.visible().is_empty() {
            self.close_context_menu(client_id);
        }
        self.render(None).non_fatal();
    }

    /// The plugin did not answer in time. Show what the configuration declared.
    pub(crate) fn context_menu_timed_out(&mut self, request_id: u64, client_id: ClientId) {
        let Some(menu) = self.context_menus.get_mut(&client_id) else {
            return;
        };
        if menu.request_id != request_id {
            return;
        }
        let ContextMenuItems::AwaitingPlugin { configured } = &menu.items else {
            // The plugin answered between the timer firing and this running. Its answer wins.
            return;
        };
        let configured = configured.clone();
        menu.items = ContextMenuItems::Fallback {
            items: configured.clone(),
            configured,
        };
        if menu.items.visible().is_empty() {
            self.close_context_menu(client_id);
        }
        self.render(None).non_fatal();
    }

    /// Close this client's menu, if it has one. Returns whether there was one to close.
    pub(crate) fn close_context_menu(&mut self, client_id: ClientId) -> bool {
        if self.context_menus.remove(&client_id).is_none() {
            return false;
        }
        self.force_render_under_context_menu();
        true
    }

    /// Close any menu opened over `pane_id` - a menu over a pane that no longer exists would be
    /// offering to act on nothing.
    pub(crate) fn close_context_menus_for_pane(&mut self, pane_id: PaneId) {
        let stale: Vec<ClientId> = self
            .context_menus
            .iter()
            .filter(|(_, menu)| menu.pane_id == pane_id)
            .map(|(client_id, _)| *client_id)
            .collect();
        for client_id in stale {
            self.close_context_menu(client_id);
        }
    }

    pub(crate) fn context_menu_is_open(&self, client_id: ClientId) -> bool {
        self.context_menus.contains_key(&client_id)
    }

    /// Nothing under a closed menu is dirty, so its cells would keep the menu's glyphs. Every tab,
    /// not just the active ones: a menu is anchored in session-surface coordinates and the tabs
    /// all share that surface, so the cells it painted are not necessarily this client's tab's.
    fn force_render_under_context_menu(&mut self) {
        for tab in self.tabs.values_mut() {
            tab.set_force_render();
        }
    }

    /// This client's menu geometry for `viewport`, or `None` if it has no menu or the menu has
    /// nothing to show yet.
    ///
    /// Laid out fresh on every use rather than stored, for the reason `notification_layout` gives:
    /// a viewport can change under an open menu, and a remembered box would then be drawn and
    /// hit-tested in coordinates that no longer exist.
    pub(super) fn context_menu_layout_for(
        &self,
        client_id: ClientId,
        viewport: Viewport,
    ) -> Option<ContextMenuLayout> {
        let menu = self.context_menus.get(&client_id)?;
        let items = menu.items.visible();
        if items.is_empty() {
            return None;
        }
        let layout = context_menu_layout(items, menu.anchor, viewport, menu.selected);
        (!layout.is_empty()).then_some(layout)
    }

    /// Paint the menus for these clients, on top of everything already in the frame.
    ///
    /// The floating pane stack is cleared first for the same reason `render_notifications` clears
    /// it: `Output` would otherwise clip these chunks behind a floating pane the menu is over,
    /// which for a menu opened *by right-clicking that pane* is every time.
    pub(super) fn render_context_menus(
        &self,
        output: &mut Output,
        viewport: Viewport,
        client_ids: &[ClientId],
    ) -> Result<()> {
        if self.context_menus.is_empty() || client_ids.is_empty() {
            return Ok(());
        }
        let previous_stack = output.set_floating_panes_stack(None);
        let result = (|| -> Result<()> {
            for client_id in client_ids {
                let Some(layout) = self.context_menu_layout_for(*client_id, viewport) else {
                    continue;
                };
                let selected = self
                    .context_menus
                    .get(client_id)
                    .and_then(|menu| menu.selected);
                let chunks = context_menu_chunks(&layout, selected, &self.style);
                output.add_character_chunks_to_client(*client_id, chunks, None)?;
            }
            Ok(())
        })();
        output.set_floating_panes_stack(previous_stack);
        result.context("failed to render context menus")
    }

    /// Handle a mouse event while this client has a menu open. Always consumes the event.
    ///
    /// Consuming unconditionally is the point: while a menu is up it owns the mouse, so a click
    /// that dismisses it does not also land on whatever was underneath. That is what every context
    /// menu does, and doing otherwise would make "click away to dismiss" also focus a pane.
    pub(crate) fn handle_context_menu_mouse_event(
        &mut self,
        event: &MouseEvent,
        viewport: Viewport,
        client_id: ClientId,
    ) {
        let Some(layout) = self.context_menu_layout_for(client_id, viewport) else {
            // Still awaiting the plugin: nothing is drawn, so there is nothing to aim at. Swallow
            // the event rather than letting it through to a pane the user cannot see right now.
            return;
        };
        let inside = context_menu_contains(&layout, event.position);
        let hit = context_menu_item_at(&layout, event.position);

        match event.event_type {
            MouseEventType::Motion => {
                // Hovering off the rows clears the highlight rather than leaving a stale one - a
                // menu that still shows a selection the pointer has left looks like it is about to
                // run that item.
                let changed = self
                    .context_menus
                    .get_mut(&client_id)
                    .map(|menu| {
                        let changed = menu.selected != hit;
                        menu.selected = hit;
                        changed
                    })
                    .unwrap_or(false);
                if changed {
                    self.render(None).non_fatal();
                }
            },
            MouseEventType::Press => {
                if !inside {
                    self.close_context_menu(client_id);
                    self.render(None).non_fatal();
                }
                // A press on a row does nothing yet; the release is what activates. Dragging onto
                // an item and off again should leave the menu exactly as it was.
            },
            MouseEventType::Release => {
                if let Some(index) = hit.filter(|_| event.left) {
                    self.activate_context_menu_item(client_id, index);
                }
            },
        }
    }

    /// Handle a key while this client has a menu open. Returns whether the menu took it.
    ///
    /// Only the keys a menu actually uses are taken. Anything else falls through and reaches the
    /// pane as it normally would - and the focus/tab/layout change it causes closes the menu,
    /// which is what a user pressing an unrelated key means anyway.
    pub(crate) fn context_menu_handles_key(
        &mut self,
        key: &Option<KeyWithModifier>,
        client_id: ClientId,
    ) -> bool {
        use zellij_utils::data::BareKey;
        let Some(key) = key else {
            return false;
        };
        if !self.context_menus.contains_key(&client_id) {
            return false;
        }
        // A menu still waiting on its plugin shows nothing, so it has no selection to move and no
        // item to run - but Esc must still dismiss it, or a slow hook leaves the user stuck.
        let has_items = self
            .context_menus
            .get(&client_id)
            .map(|menu| !menu.items.visible().is_empty())
            .unwrap_or(false);

        match key.bare_key {
            BareKey::Esc => {
                self.close_context_menu(client_id);
                self.render(None).non_fatal();
                true
            },
            BareKey::Down | BareKey::Up | BareKey::Char('j') | BareKey::Char('k') if has_items => {
                let forward = matches!(key.bare_key, BareKey::Down | BareKey::Char('j'));
                if let Some(menu) = self.context_menus.get_mut(&client_id) {
                    let items = menu.items.visible().to_vec();
                    menu.selected = next_selectable(&items, menu.selected, forward);
                }
                self.render(None).non_fatal();
                true
            },
            BareKey::Enter if has_items => {
                let selected = self
                    .context_menus
                    .get(&client_id)
                    .and_then(|menu| menu.selected);
                match selected {
                    Some(index) => self.activate_context_menu_item(client_id, index),
                    // Enter with nothing highlighted closes rather than guessing at the first
                    // item: the user has not said which one they mean.
                    None => {
                        self.close_context_menu(client_id);
                        self.render(None).non_fatal();
                    },
                }
                true
            },
            _ => false,
        }
    }

    /// Run an item and close the menu.
    ///
    /// The menu is taken out of the map *before* the actions run, so an action that itself asks the
    /// screen to do something - most of them do - cannot find a menu that is on its way out.
    fn activate_context_menu_item(&mut self, client_id: ClientId, index: usize) {
        let Some(menu) = self.context_menus.get(&client_id) else {
            return;
        };
        let Some(item) = menu.items.visible().get(index).cloned() else {
            return;
        };
        if !item.is_selectable() {
            return;
        }
        let context = menu.context.clone();
        let pane_id = menu.pane_id;
        let plugin_id = menu.plugin_id;
        self.close_context_menu(client_id);

        if !item.actions.is_empty() {
            // Not run here. The focus restoration and the actions have to stay on the same side of
            // every instruction already queued behind the click that activated this item, and the
            // actions can only be dispatched from another thread (see `run_context_menu_actions`).
            // Enqueueing both as one instruction is what keeps them together; the screen channel is
            // unbounded, so this self-send cannot block the thread it is sent from.
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::ActivateContextMenuItem {
                    pane_id,
                    actions: item.actions.clone(),
                    client_id,
                })
                .non_fatal();
        }
        // An item may carry plugin data instead of - or as well as - actions. This is what lets a
        // plugin-supplied item do something no `Action` can express, and it is the same echo
        // `Event::HighlightClicked` performs for a clicked regex highlight - addressed, as that one
        // is, to the single plugin it concerns rather than to everything subscribed.
        if !item.context.is_empty() {
            match plugin_id {
                Some(plugin_id) => {
                    let _ = self
                        .bus
                        .senders
                        .send_to_plugin(PluginInstruction::Update(vec![(
                            Some(plugin_id),
                            Some(client_id),
                            Event::ContextMenuItemSelected {
                                label: item.label.clone(),
                                item_context: item.context.clone(),
                                context: Box::new(context),
                            },
                        )]));
                },
                // Configured items carry no context, so this is a menu that was answered from the
                // configuration showing an item that came from somewhere else - there is nobody to
                // send it to.
                None => log::debug!(
                    "context menu item '{}' carries context but no plugin answered this menu",
                    item.label
                ),
            }
        }
        self.render(None).non_fatal();
    }

    /// Restore the focus a menu item's actions are relative to, then run them.
    ///
    /// Reached from `ScreenInstruction::ActivateContextMenuItem`, which
    /// [`Screen::activate_context_menu_item`] enqueued - so everything the queue was already
    /// holding has been processed, and whatever focus *those* instructions wanted is now in place
    /// and about to be overridden by the pane the user actually right-clicked.
    pub(crate) fn dispatch_context_menu_actions(
        &mut self,
        pane_id: PaneId,
        actions: Vec<Action>,
        client_id: ClientId,
    ) {
        // The menu's pane can be gone by now - most closures take the menu with them
        // (`close_context_menus_for_pane`), but the item was activated a queue-turn ago and
        // nothing stops a pane closing in between. An item offering to act on a pane that no
        // longer exists must do nothing at all: `focus_pane_with_id` would only log, and the
        // focus-relative actions would then run against whichever pane the client was moved to.
        if self.tab_containing_pane(pane_id).is_none() {
            log::debug!("context menu pane {pane_id:?} is gone, dropping its item's actions");
            return;
        }
        // A menu takes no focus, so the clicked pane is normally still the focused one and
        // `Copy`/`CloseFocus` already mean it. "Normally" is not "always": the hook - or any other
        // plugin - can focus something else while the menu is up, and then the actions would land
        // on a pane the user never pointed at.
        if self.active_pane_id(client_id) != Some(pane_id) {
            self.focus_pane_with_id(pane_id, false, false, client_id)
                .non_fatal();
        }
        self.run_context_menu_actions(actions, client_id);
    }

    /// Route an item's actions, in order, off the screen thread.
    ///
    /// Off-thread because `route_action` sends *to* the screen and blocks on the completion it
    /// gets back; running it here would deadlock the thread it is waiting on. `run_action` in the
    /// plugin host spawns for exactly this reason, and this is the same shape.
    ///
    /// Sequential rather than concurrent: an item that splits a pane and then renames it means
    /// those in that order, and `route_action` already blocks on each action's completion (with a
    /// timeout), so awaiting them is just running them one after another.
    fn run_context_menu_actions(&self, actions: Vec<Action>, client_id: ClientId) {
        let senders = self.bus.senders.clone();
        let default_mode = self.default_mode_info.mode;
        let spawned = thread::Builder::new()
            .name("context_menu_actions".to_string())
            .spawn(move || {
                for action in actions {
                    let name = action.to_string();
                    // `pane_id: None` on purpose. `new_pane_routing` only consults it for
                    // `no_focus`/`near_current_pane` placements; everything else routes by client,
                    // which means the client's focused pane - and the caller has just made sure
                    // that is the pane the menu was opened over. Naming the pane here would say the
                    // same thing in a way that could drift from it.
                    if let Err(e) = route_action(
                        action,
                        client_id,
                        None,
                        None,
                        senders.clone(),
                        None,
                        None,
                        default_mode,
                        None,
                    ) {
                        log::error!("context menu action '{name}' failed: {e:?}");
                        // Stop at the first failure: the actions of one item are a sequence, and
                        // running the rest of a sequence whose start did not happen is worse than
                        // running none of it.
                        break;
                    }
                }
            });
        // A thread that will not spawn means the item silently did nothing - worth a log line, not
        // worth taking the session down for.
        if let Err(e) = spawned {
            log::error!("failed to spawn context menu action thread: {e}");
        }
    }
}
