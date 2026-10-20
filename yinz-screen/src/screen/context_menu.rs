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
//! - **A menu draws nothing until it knows what it says, and then says it until it closes.** The
//!   plugin hook answers asynchronously, so a fresh menu is `AwaitingPlugin` and paints no cells at
//!   all. `BackgroundJob::ContextMenuTimeout` promotes it to the configured items if no answer
//!   arrives, which is what stops an unbound or wedged hook from making right-click silently do
//!   nothing. Whichever of the two gets there first is the list the menu keeps - a drawn menu is a
//!   clickable one, and rows that move under a press the user has already made run the wrong item.
//! - **Closing forces a repaint.** Nothing underneath a menu is dirty when it goes, so its cells
//!   would keep its glyphs. [`Screen::close_context_menu`] force-renders every tab and the global
//!   panes, exactly as `advance_notification_animations` does when a toast leaves.

use std::collections::HashMap;
use std::thread;
use yinz_types::internal_ids::ContextMenuRequestId;
use yinz_types::internal_ids::PluginId;

use yinz_runtime::errors::prelude::*;
use yinz_types::input::actions::Action;
use yinz_types::keys::KeyWithModifier;
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::Viewport;
use yinz_types::position::Position;
use {
    yinz_types::data::ContextMenuContext, yinz_types::data::ContextMenuItem,
    yinz_types::data::Event,
};

use super::{ContextMenuItems, ContextMenuState, Screen, ScreenInstruction};
use crate::background_jobs::BackgroundJob;
use crate::output::Output;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::route::{actions_leave_clipboard_message, route_action};
use crate::ui::context_menu::{
    context_menu_chunks, context_menu_contains, context_menu_item_at, context_menu_layout,
    next_selectable, ContextMenuLayout,
};
use crate::ui::damage::CellRect;
use crate::ClientId;

/// How long a menu waits for its hook plugin before falling back to the configured items.
///
/// Short enough that a wedged plugin is not a visible stall, long enough that a plugin doing
/// ordinary work - a few `BTreeMap` lookups and a filter - always wins the race. A plugin that
/// answers after the timer has fired is dropped: the configured items are already drawn, and
/// therefore already being clicked, so replacing the rows now could run an item nobody aimed at.
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
    ) -> ContextMenuRequestId {
        self.close_command_palette(client_id);
        let request_id = self.next_context_menu_request_id;
        self.next_context_menu_request_id =
            ContextMenuRequestId::new(self.next_context_menu_request_id.get() + 1);
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
                pane_id: context.pane_id,
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
        request_id: ContextMenuRequestId,
        items: Vec<ContextMenuItem>,
        plugin_id: Option<PluginId>,
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
            ContextMenuItems::AwaitingPlugin { configured } => configured,
            ContextMenuItems::Ready(_) => {
                // An answer to a menu that is already drawn - a second answer, or one that lost the
                // race to the timeout. Either way the user is looking at, and may already be
                // pressing, the list this would replace.
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
        self.render().non_fatal();
    }

    /// The plugin did not answer in time. Show what the configuration declared.
    pub(crate) fn context_menu_timed_out(
        &mut self,
        request_id: ContextMenuRequestId,
        client_id: ClientId,
    ) {
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
        // `Ready` rather than a fallback state of its own: nothing replaces this list either, so
        // there is nothing left for the distinction to decide.
        menu.items = ContextMenuItems::Ready(configured.clone());
        if menu.items.visible().is_empty() {
            self.close_context_menu(client_id);
        }
        self.render().non_fatal();
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
    pub(super) fn force_render_under_context_menu(&mut self) {
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

    /// The cells these clients' menus will paint into `viewport`.
    ///
    /// The same relationship to `render_context_menus` that `notification_cell_rects` has to
    /// `render_notifications`: one mapping, derived from the one layout function, so what occludes
    /// the cursor and what is drawn cannot drift apart. Told to `Output` before anything renders,
    /// because a fake cursor is decided while the panes draw.
    pub(super) fn context_menu_cell_rects(
        &self,
        client_ids: &[ClientId],
        viewport: Viewport,
    ) -> HashMap<ClientId, Vec<CellRect>> {
        client_ids
            .iter()
            .filter_map(|client_id| {
                let layout = self.context_menu_layout_for(*client_id, viewport)?;
                Some((
                    *client_id,
                    vec![CellRect {
                        x: layout.x,
                        y: layout.y,
                        cols: layout.width,
                        rows: layout.height(),
                    }],
                ))
            })
            .collect()
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
                    self.render().non_fatal();
                }
            },
            MouseEventType::Press => {
                if !inside {
                    self.close_context_menu(client_id);
                    self.render().non_fatal();
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
        use yinz_types::keys::BareKey;
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
                self.render().non_fatal();
                true
            },
            BareKey::Down | BareKey::Up | BareKey::Char('j') | BareKey::Char('k')
                if has_items && key.key_modifiers.is_empty() =>
            {
                let forward = matches!(key.bare_key, BareKey::Down | BareKey::Char('j'));
                if let Some(menu) = self.context_menus.get_mut(&client_id) {
                    let items = menu.items.visible().to_vec();
                    menu.selected = next_selectable(&items, menu.selected, forward);
                }
                self.render().non_fatal();
                true
            },
            BareKey::Enter if has_items && key.key_modifiers.is_empty() => {
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
                        self.render().non_fatal();
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
            // actions can only be dispatched from another thread (see `run_overlay_actions`).
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
        self.render().non_fatal();
    }

    /// Run a menu item's actions against the pane its menu was opened over.
    ///
    /// Reached from `ScreenInstruction::ActivateContextMenuItem`, which
    /// [`Screen::activate_context_menu_item`] enqueued - so everything the queue was already
    /// holding has been processed, and whatever focus *those* instructions wanted is about to be
    /// overridden by the pane the user actually right-clicked. The override itself belongs to
    /// [`Screen::run_overlay_actions`], which puts it next to the action it is for.
    pub(crate) fn dispatch_context_menu_actions(
        &mut self,
        pane_id: PaneId,
        actions: Vec<Action>,
        client_id: ClientId,
    ) {
        // The menu's pane can be gone by now - most closures take the menu with them
        // (`close_context_menus_for_pane`), but the item was activated a queue-turn ago and
        // nothing stops a pane closing in between. An item offering to act on a pane that no
        // longer exists must do nothing at all: the focus assertion would only log, and the
        // focus-relative actions would then run against whichever pane the client was moved to.
        if self.tab_containing_pane(pane_id).is_none() {
            log::debug!("context menu pane {pane_id:?} is gone, dropping its item's actions");
            return;
        }
        self.run_overlay_actions(pane_id, actions, client_id, None);
    }

    /// Route an item's actions, in order, off the screen thread - each one against `pane_id`.
    ///
    /// Off-thread because `route_action` sends *to* the screen and blocks on the completion it
    /// gets back; running it here would deadlock the thread it is waiting on. `run_action` in the
    /// plugin host spawns for exactly this reason, and this is the same shape.
    ///
    /// Sequential rather than concurrent: an item that splits a pane and then renames it means
    /// those in that order, and `route_action` already blocks on each action's completion (with a
    /// timeout), so awaiting them is just running them one after another.
    ///
    /// A menu takes no focus, so the clicked pane is normally still the focused one and
    /// `Copy`/`CloseFocus` already mean it. "Normally" is not "always": the hook - or any other
    /// plugin - can focus elsewhere while the menu is up. Asserting the focus from *this* thread,
    /// immediately before the action it belongs to, is what makes the pair inseparable: restoring
    /// it on the screen thread instead would leave everything that arrives while this thread is
    /// starting up free to land between the two. What remains is another thread's send falling
    /// between these two sends, which is a focus change genuinely concurrent with the item rather
    /// than one that was already queued.
    pub(super) fn run_overlay_actions(
        &self,
        pane_id: PaneId,
        actions: Vec<Action>,
        client_id: ClientId,
        palette_tab: Option<(yinz_types::internal_ids::TabId, String)>,
    ) {
        let senders = self.bus.senders.clone();
        let default_mode = self.default_mode_info.mode;
        let default_shell = self.default_shell_action.clone();
        // `UserInput` calls this a terminal id, and the only other place the two are related -
        // routing a CLI action - reads one back as `PaneId::Terminal`. A menu over a plugin pane
        // therefore has no terminal to name.
        let terminal_id = match pane_id {
            PaneId::Terminal(id) => Some(id),
            PaneId::Plugin(_) => None,
        };
        let spawned = thread::Builder::new()
            .name("context_menu_actions".to_string())
            .spawn(move || {
                // A menu item's actions are a batch like a key's, and "Copy" is a stock item: the
                // action that copies must not also tell the plugins it was input, or the status
                // bar drops the message the copy just put up. See
                // `route::actions_leave_clipboard_message`.
                let leaves_clipboard_message = actions_leave_clipboard_message(&actions);
                for action in actions {
                    let name = palette_tab
                        .as_ref()
                        .map(|(_, label)| label.clone())
                        .unwrap_or_else(|| action.to_string());
                    let departure = matches!(action, Action::Detach | Action::SwitchSession { .. });
                    // A no-op when the pane is already focused, which is the ordinary case - see
                    // the `already_focused` arm of `ScreenInstruction::FocusPaneWithId`.
                    if palette_tab.is_none() {
                        senders
                            .send_to_screen(ScreenInstruction::FocusPaneWithId(
                                pane_id, false, false, client_id, None,
                            ))
                            .non_fatal();
                    }
                    // An item's action is something the user performed, so the plugins holding
                    // `InterceptInput` and watching `Event::UserAction` have to see it - the key
                    // and CLI paths in `route.rs` both send this before routing, and a menu is
                    // just a third way of asking for the same action.
                    senders
                        .send_to_plugin(PluginInstruction::UserInput {
                            client_id,
                            action: action.clone(),
                            terminal_id,
                            cli_client_id: None,
                        })
                        .non_fatal();
                    if let Some((tab_id, _)) = &palette_tab {
                        let direction = match action {
                            Action::BreakPane => Some(None),
                            Action::BreakPaneLeft => {
                                Some(Some(yinz_types::geometry::Direction::Left))
                            },
                            Action::BreakPaneRight => {
                                Some(Some(yinz_types::geometry::Direction::Right))
                            },
                            _ => None,
                        };
                        if let Some(direction) = direction {
                            senders
                                .send_to_plugin(PluginInstruction::Update(vec![(
                                    None,
                                    Some(client_id),
                                    Event::InputReceived,
                                )]))
                                .non_fatal();
                            senders
                                .send_to_screen(ScreenInstruction::MoveCommandPalettePane {
                                    pane_id,
                                    tab_id: *tab_id,
                                    direction,
                                    client_id,
                                })
                                .non_fatal();
                            continue;
                        }
                    }
                    // Named rather than left as `None`: `new_pane_routing` consults it for
                    // `no_focus`/`near_current_pane` placements, which without it would be placed
                    // relative to whatever the client is focused on rather than to the pane the
                    // menu was opened over. Everything else routes by client, which the
                    // `FocusPaneWithId` above has just pointed at the same pane.
                    let focus_created_pane = palette_tab.is_some()
                        && matches!(
                            action,
                            Action::NewTiledPane { .. } | Action::NewFloatingPane { .. }
                        );
                    let focus_toggled_pane = palette_tab.is_some()
                        && matches!(action, Action::TogglePaneEmbedOrFloatingByPaneId { .. });
                    let result = route_action(
                        action,
                        client_id,
                        None,
                        Some(pane_id),
                        senders.clone(),
                        default_shell.clone(),
                        None,
                        default_mode,
                        None,
                        leaves_clipboard_message,
                        true, // is_client_input
                    );
                    let confirmed =
                        matches!(&result, Ok((_, Some(completion))) if completion.confirmed);
                    let error = match result {
                        Err(error) => Some(format!("{error:#}")),
                        Ok((_, Some(completion))) => {
                            // ID-relative actions leave focus alone. The palette belongs to a
                            // client, so reveal and focus its created or toggled pane afterward.
                            let pane_to_focus = if focus_toggled_pane {
                                Some(pane_id)
                            } else if focus_created_pane {
                                completion.affected_pane_id
                            } else {
                                None
                            };
                            let error = completion.error_message.or_else(|| {
                                completion
                                    .exit_status
                                    .filter(|status| *status != 0)
                                    .map(|status| format!("action exited with status {status}"))
                            });
                            if error.is_none()
                                && let Some(pane_id) = pane_to_focus
                            {
                                route_action(
                                    Action::FocusPaneByPaneId { pane_id },
                                    client_id,
                                    None,
                                    Some(pane_id),
                                    senders.clone(),
                                    default_shell.clone(),
                                    None,
                                    default_mode,
                                    None,
                                    false,
                                    true,
                                )
                                .non_fatal();
                            }
                            error
                        },
                        _ => None,
                    };
                    if let Some(error) = error {
                        log::error!("overlay action '{name}' failed: {error}");
                        senders
                            .send_to_screen(ScreenInstruction::Notify(
                                if palette_tab.is_some() {
                                    format!(
                                        "Could not {}: action failed (see logs)",
                                        name.to_lowercase()
                                    )
                                } else {
                                    format!("{name}: {error}")
                                },
                                None,
                                None,
                                None,
                            ))
                            .non_fatal();
                        break;
                    }
                    if palette_tab.is_some() && !departure {
                        let message = if confirmed {
                            format!("Completed: {name}")
                        } else {
                            format!("Completion not confirmed: {name}")
                        };
                        super::command_palette::notify(&senders, message);
                    }
                }
            });
        // A thread that will not spawn means the item silently did nothing - worth a log line, not
        // worth taking the session down for.
        if let Err(e) = spawned {
            log::error!("failed to spawn overlay action thread: {e}");
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::Notify(
                    format!("Could not start action: {e}"),
                    None,
                    None,
                    None,
                ))
                .non_fatal();
        }
    }
}
