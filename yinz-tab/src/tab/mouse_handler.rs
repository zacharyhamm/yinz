use std::time::Instant;
use yinz_runtime::errors::prelude::*;
use yinz_types::geometry::{Direction, Resize, ResizeStrategy};
use yinz_types::input::layout::Run;
use yinz_types::internal_ids::ContextMenuRequestId;
#[cfg(test)]
use yinz_types::internal_ids::{PluginId, TerminalId};
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::PaneGeom;
use yinz_types::position::Position;
use {yinz_types::data::ContextMenuContext, yinz_types::data::ContextMenuLink};

use crate::background_jobs::BackgroundJob;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::screen::{GuestModalOutcome, ScreenInstruction};
use crate::ClientId;

use super::{Pane, PaneEdge, Tab};

fn clear_hover_for_client(tab: &mut Tab, client_id: ClientId) -> bool {
    clear_hover_for_client_except(tab, client_id, None)
}

/// Drop the client's hover, leaving `keep`'s alone.
///
/// A plugin is told the pointer left it by a buttonless motion at a column that is not on it,
/// and that is a claim about where the pointer is - so it must not be made about the very pane
/// the pointer is clicking. The status bar reads it exactly that way: its hover reveal puts
/// the clickable ribbons in the slot the clock otherwise has, and a leave sent ahead of the
/// click took them back down before the click could land on one.
fn clear_hover_for_client_except(tab: &mut Tab, client_id: ClientId, keep: Option<PaneId>) -> bool {
    let mut cleared = false;
    if let Some(prev_pid) = tab.mouse_hover_pane_id.remove(&client_id) {
        if let Some(pane) = tab.get_pane_with_id_mut(prev_pid) {
            pane.set_hover_position(None);
        }
        cleared = true;
    }
    if tab.plugin_hover_pane_id.get(&client_id).copied() == keep {
        return cleared;
    }
    if let Some(prev_plugin_pid) = tab.plugin_hover_pane_id.remove(&client_id) {
        if let Some(pane) = tab.get_pane_with_id(prev_plugin_pid) {
            let _ = pane.mouse_event(
                &MouseEvent::new_buttonless_motion(Position::new(0, u16::MAX)),
                client_id,
            );
        }
        cleared = true;
    }
    cleared
}

/// What the tab did with a mouse event that the screen has to finish.
///
/// No longer `Copy`: `open_context_menu` carries a whole click context, and boxing it keeps the
/// common case - every other effect, where it is `None` - one pointer wide.
#[derive(Debug, Default, Clone)]
pub struct MouseEffect {
    pub state_changed: bool,
    pub leave_clipboard_message: bool,
    pub group_toggle: Option<PaneId>,
    pub group_add: Option<PaneId>,
    pub ungroup: bool,
    /// A right-click asked for a context menu. Only `Screen` can open one - a menu outlives the
    /// tab's view of the event and is drawn above every tab - so the tab gathers the context and
    /// hands it up.
    pub open_context_menu: Option<Box<ContextMenuContext>>,
}

impl MouseEffect {
    pub fn state_changed() -> Self {
        MouseEffect {
            state_changed: true,
            leave_clipboard_message: false,
            group_toggle: None,
            group_add: None,
            ungroup: false,
            open_context_menu: None,
        }
    }
    pub fn leave_clipboard_message() -> Self {
        MouseEffect {
            state_changed: false,
            leave_clipboard_message: true,
            group_toggle: None,
            group_add: None,
            ungroup: false,
            open_context_menu: None,
        }
    }
    pub fn state_changed_and_leave_clipboard_message() -> Self {
        MouseEffect {
            state_changed: true,
            leave_clipboard_message: true,
            group_toggle: None,
            group_add: None,
            ungroup: false,
            open_context_menu: None,
        }
    }
    pub fn group_toggle(pane_id: PaneId) -> Self {
        MouseEffect {
            state_changed: true,
            leave_clipboard_message: false,
            group_toggle: Some(pane_id),
            group_add: None,
            ungroup: false,
            open_context_menu: None,
        }
    }
    pub fn group_add(pane_id: PaneId) -> Self {
        MouseEffect {
            state_changed: true,
            leave_clipboard_message: false,
            group_toggle: None,
            group_add: Some(pane_id),
            ungroup: false,
            open_context_menu: None,
        }
    }
    /// A right-click that asked for a context menu.
    ///
    /// `state_changed` because the click focused the pane it landed in - the focus move the menu
    /// depends on, and the reason its items act on the right pane.
    pub fn open_context_menu(context: ContextMenuContext) -> Self {
        MouseEffect {
            state_changed: true,
            leave_clipboard_message: false,
            group_toggle: None,
            group_add: None,
            ungroup: false,
            open_context_menu: Some(Box::new(context)),
        }
    }
    pub fn ungroup() -> Self {
        MouseEffect {
            state_changed: true,
            leave_clipboard_message: false,
            group_toggle: None,
            group_add: None,
            ungroup: true,
            open_context_menu: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum MouseAction {
    GroupToggle(PaneId),
    /// A right-press that should open a context menu over `pane_id`.
    ///
    /// `position` is the **absolute** click cell, not the pane-relative one. The variant carries
    /// it rather than recomputing it in the executor precisely so the conversion to pane-relative
    /// coordinates - which the executor also needs - cannot lose the anchor the menu is placed at.
    OpenContextMenu {
        pane_id: PaneId,
        position: Position,
        on_frame: bool,
    },
    GroupAdd(PaneId),
    Ungroup,
    StartResize {
        pane_id: PaneId,
        edge: PaneEdge,
        target: ResizeDragTarget,
        position: Position,
    },
    ContinueResize {
        position: Position,
    },
    StopResize {
        position: Position,
    },
    FocusPane {
        pane_id: PaneId,
        position: Position,
    },
    FocusPaneAndClickThrough {
        pane_id: PaneId,
        position: Position,
        event: MouseEvent,
    },
    ShowFloatingPanesAndFocus {
        pane_id: PaneId,
    },
    StartSelection {
        pane_id: PaneId,
        position: Position,
    },
    UpdateSelection {
        position: Position,
    },
    EndSelection {
        position: Position,
    },
    StartMovingFloatingPane {
        position: Position,
    },
    ContinueMovingFloatingPane {
        position: Position,
    },
    StopMovingFloatingPane {
        position: Position,
    },
    StartMovingOverlay {
        position: Position,
    },
    ContinueMovingOverlay {
        position: Position,
    },
    StopMovingOverlay {
        position: Position,
    },
    /// A plain left press on the seam between two linked overlays - the divider of a panel
    /// drawn as two panes. Checked before `StartMovingOverlay`, because the seam is on both
    /// panes' frames and a press there would otherwise pick the panel up.
    StartOverlaySeamDrag {
        left: PaneId,
        right: PaneId,
        position: Position,
    },
    DragOverlaySeam {
        position: Position,
    },
    StopOverlaySeamDrag {
        position: Position,
    },
    ScrollUp {
        pane_id: PaneId,
        lines: usize,
    },
    ScrollDown {
        pane_id: PaneId,
        lines: usize,
    },
    ScrollLeft {
        pane_id: PaneId,
        cols: usize,
    },
    ScrollRight {
        pane_id: PaneId,
        cols: usize,
    },
    ResizeScrollUp {
        pane_id: PaneId,
    },
    ResizeScrollDown {
        pane_id: PaneId,
    },
    ScrollToPreviousPrompt {
        pane_id: PaneId,
    },
    ScrollToNextPrompt {
        pane_id: PaneId,
    },
    UpdateHover {
        pane_id: Option<PaneId>,
        position: Option<Position>,
    },
    FocusOnHover {
        pane_id: PaneId,
        position: Position,
    },
    SendToTerminal {
        pane_id: PaneId,
        event: MouseEvent,
    },
    FrameIntercepted {
        pane_id: PaneId,
    },
    NoAction,
}

/// Which collection is holding a mouse *resize*, while one is in flight.
///
/// The resize twin of `MouseDragTarget`, and a third state rather than `is_floating: bool`:
/// since overlays gained a resize they are neither floating nor tiled, and a boolean would send
/// one to `tiled_panes` - which does not hold it, and would error the whole mouse event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeDragTarget {
    Tiled,
    Floating,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneResizeState {
    pub pane_id: PaneId,
    pub edge: PaneEdge,
    pub start_position: Position,
    pub start_geom: PaneGeom,
    pub target: ResizeDragTarget,
}

/// Which collection is holding a mouse drag, while one is in flight.
///
/// An enum rather than two booleans because only one drag can be in flight at a time: the two
/// collections have separate drag state, but the pointer is one pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseDragTarget {
    Floating,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClickedPaneDetails {
    pane_id: PaneId,
    on_frame: bool,
    frame_intercepted: bool,
    edge: Option<PaneEdge>,
    is_floating: bool,
    /// Not the negation of `is_floating`: since overlays moved out of the floating collection
    /// they answer `false` to both `panes_contain` *and* the layer-visibility flag, so the frame
    /// branches below had no way to tell an overlay from a tiled pane and treated it as one.
    is_overlay: bool,
    /// An overlay the user may resize - one that is not a modal. Separate from `is_overlay`
    /// because the two frame branches answer differently: only a non-modal one is resizable.
    overlay_is_resizable: bool,
    /// An overlay the user may drag - one that is not expanded over the screen. A modal is
    /// draggable but not resizable and an expanded overlay is neither, which is why this is not
    /// the same field as `overlay_is_resizable`: an expansion fills the screen, so there is
    /// nowhere to carry it to and the rectangle a drag would write is the one it comes back to.
    overlay_is_movable: bool,
    /// The linked pair whose seam the press landed on, when the frame cell under the pointer
    /// is the divider between two overlays placed together (`OverlayPanes::seam_at`). A press
    /// there drags the divider rather than the panel.
    on_seam: Option<(PaneId, PaneId)>,
    terminal_wants_mouse: bool,
    /// The pane's terminal emulator has mouse tracking on, so right-click is its own.
    ///
    /// Not `terminal_wants_mouse` above: that one probes with a *left* click and is only computed
    /// for the active pane, because that is all its two callers need. A background TUI still owns
    /// its own right-click, so this asks the pane directly and asks it for every pane.
    terminal_emulator_wants_mouse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MouseEventContext {
    pane_id_at_position: Option<PaneId>,
    active_pane_id: Option<PaneId>,
    floating_visible: bool,
    pane_being_resized: bool,
    selecting_with_mouse: bool,
    pane_being_moved: Option<MouseDragTarget>,
    /// A seam drag is in flight (`OverlayPanes::seam_is_being_dragged`). Exclusive with
    /// `pane_being_moved` - the pointer is one pointer - and checked first, as the resize is.
    seam_being_dragged: bool,
    clicked_pane: Option<ClickedPaneDetails>,
    advanced_mouse_actions: bool,
    pinned_selectable: Option<PaneId>,
    pinned_unselectable: Option<PaneId>,
    focus_follows_mouse: bool,
    mouse_click_through: bool,
    mouse_scroll_resize: bool,
    context_menu_enabled: bool,
    /// The pane the client is descended into (a nested guest), if any. Alt+mouse over it goes
    /// to the pane when it tracks the mouse, instead of toggling host pane groups.
    passthrough_pane_id: Option<PaneId>,
}

fn edge_and_delta_to_strategies(
    edge: PaneEdge,
    delta_x: isize,
    delta_y: isize,
) -> Vec<ResizeStrategy> {
    use Direction::*;
    use Resize::*;

    match edge {
        PaneEdge::Left => {
            let resize = if delta_x < 0 { Increase } else { Decrease };
            vec![ResizeStrategy {
                resize,
                direction: Some(Left),
                invert_on_boundaries: false,
            }]
        },
        PaneEdge::Right => {
            let resize = if delta_x > 0 { Increase } else { Decrease };
            vec![ResizeStrategy {
                resize,
                direction: Some(Right),
                invert_on_boundaries: false,
            }]
        },
        PaneEdge::Top => {
            let resize = if delta_y < 0 { Increase } else { Decrease };
            vec![ResizeStrategy {
                resize,
                direction: Some(Up),
                invert_on_boundaries: false,
            }]
        },
        PaneEdge::Bottom => {
            let resize = if delta_y > 0 { Increase } else { Decrease };
            vec![ResizeStrategy {
                resize,
                direction: Some(Down),
                invert_on_boundaries: false,
            }]
        },
        PaneEdge::TopLeft => {
            let mut strategies = vec![];
            let resize_y = if delta_y < 0 { Increase } else { Decrease };
            strategies.push(ResizeStrategy {
                resize: resize_y,
                direction: Some(Up),
                invert_on_boundaries: false,
            });
            let resize_x = if delta_x < 0 { Increase } else { Decrease };
            strategies.push(ResizeStrategy {
                resize: resize_x,
                direction: Some(Left),
                invert_on_boundaries: false,
            });
            strategies
        },
        PaneEdge::TopRight => {
            let mut strategies = vec![];
            let resize_y = if delta_y < 0 { Increase } else { Decrease };
            strategies.push(ResizeStrategy {
                resize: resize_y,
                direction: Some(Up),
                invert_on_boundaries: false,
            });
            let resize_x = if delta_x > 0 { Increase } else { Decrease };
            strategies.push(ResizeStrategy {
                resize: resize_x,
                direction: Some(Right),
                invert_on_boundaries: false,
            });
            strategies
        },
        PaneEdge::BottomLeft => {
            let mut strategies = vec![];
            let resize_y = if delta_y > 0 { Increase } else { Decrease };
            strategies.push(ResizeStrategy {
                resize: resize_y,
                direction: Some(Down),
                invert_on_boundaries: false,
            });
            let resize_x = if delta_x < 0 { Increase } else { Decrease };
            strategies.push(ResizeStrategy {
                resize: resize_x,
                direction: Some(Left),
                invert_on_boundaries: false,
            });
            strategies
        },
        PaneEdge::BottomRight => {
            let mut strategies = vec![];
            let resize_y = if delta_y > 0 { Increase } else { Decrease };
            strategies.push(ResizeStrategy {
                resize: resize_y,
                direction: Some(Down),
                invert_on_boundaries: false,
            });
            let resize_x = if delta_x > 0 { Increase } else { Decrease };
            strategies.push(ResizeStrategy {
                resize: resize_x,
                direction: Some(Right),
                invert_on_boundaries: false,
            });
            strategies
        },
    }
}

/// Whether a press in this pane should be followed until it is released.
///
/// A pane that does its own text selection needs the drag to extend the selection. A plugin pane
/// needs it so its `Mouse::LeftClick`, `Mouse::Hold` and `Mouse::Release` events are all sent from
/// those same three `Pane` calls (see `PluginPane::start_selection`); otherwise a form widget that
/// activates on release would never fire.
fn drag_should_be_tracked(pane_id: PaneId, pane: &dyn Pane) -> bool {
    pane.supports_mouse_selection() || matches!(pane_id, PaneId::Plugin(_))
}

/// Which collection a frame press belongs to, for the resize that press starts.
fn resize_target(details: &ClickedPaneDetails) -> ResizeDragTarget {
    if details.is_overlay {
        ResizeDragTarget::Overlay
    } else if details.is_floating {
        ResizeDragTarget::Floating
    } else {
        ResizeDragTarget::Tiled
    }
}

pub struct MouseHandler;

impl MouseHandler {
    pub fn handle_mouse_event(
        tab: &mut Tab,
        event: &MouseEvent,
        client_id: ClientId,
        passthrough_pane_id: Option<PaneId>,
    ) -> Result<MouseEffect> {
        if let Some(effect) = Self::intercept_guest_modal_mouse_event(tab, event, client_id)? {
            return Ok(effect);
        }
        let context = Self::gather_mouse_event_context(tab, event, client_id, passthrough_pane_id)?;
        let action = Self::determine_mouse_action(event, &context)?;
        Self::execute_mouse_action(tab, action, event, client_id)
    }

    fn intercept_guest_modal_mouse_event(
        tab: &mut Tab,
        event: &MouseEvent,
        client_id: ClientId,
    ) -> Result<Option<MouseEffect>> {
        let pane_id_at_position = Self::get_pane_at(tab, &event.position, false)?.map(|p| p.pid());
        let pane_id = match pane_id_at_position {
            Some(pane_id) => pane_id,
            None => return Ok(None),
        };
        if !tab.pane_has_guest_modal_for_client(pane_id, client_id) {
            return Ok(None);
        }
        let hit_option = if event.event_type == MouseEventType::Release && event.left {
            let style = tab.style;
            if let Some(pane) = tab.get_pane_with_id(pane_id) {
                let relative_position = pane.relative_position(&event.position);
                let rows = pane.get_content_rows();
                let columns = pane.get_content_columns();
                let row = relative_position.line();
                if row < 0 {
                    None
                } else {
                    let session_name = pane.guest_session_name().unwrap_or_default();
                    let selection = pane.guest_modal_selection(client_id).unwrap_or(0);
                    let shortcuts = pane.guest_modal_shortcuts();
                    crate::panes::nested_session_modal::guest_modal_option_at_content_row(
                        rows,
                        columns,
                        row as usize,
                        &style,
                        session_name.as_str(),
                        selection,
                        &shortcuts,
                    )
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(option) = hit_option {
            let outcome = match option {
                0 => GuestModalOutcome::Zoom,
                _ => GuestModalOutcome::Descend,
            };
            let _ = tab
                .senders
                .send_to_screen(ScreenInstruction::GuestModalChoice {
                    client_id,
                    pane_id,
                    outcome,
                });
        }
        Ok(Some(MouseEffect::state_changed()))
    }

    fn gather_mouse_event_context(
        tab: &mut Tab,
        event: &MouseEvent,
        client_id: ClientId,
        passthrough_pane_id: Option<PaneId>,
    ) -> Result<MouseEventContext> {
        let err_context = || format!("failed to gather context for event {event:?}");

        let pane_id_at_position = Self::get_pane_at(tab, &event.position, false)
            .with_context(err_context)?
            .map(|p| p.pid());
        let active_pane_id = tab.get_active_pane_id(client_id);
        let floating_visible = tab.floating_panes.panes_are_visible();

        let clicked_pane = pane_id_at_position.and_then(|id| {
            Self::gather_clicked_pane_details(
                tab,
                id,
                &event.position,
                active_pane_id,
                event,
                client_id,
            )
        });

        let (pinned_selectable, pinned_unselectable) = if !floating_visible {
            let selectable = tab
                .floating_panes
                .get_pinned_pane_id_at(&event.position, true)
                .ok()
                .flatten()
                .map(|id| tab.stack_list_member_at_point(id, &event.position))
                .filter(|id| {
                    tab.get_pane_with_id(*id)
                        .is_some_and(|pane| pane.selectable())
                });
            let unselectable = tab
                .floating_panes
                .get_pinned_pane_id_at(&event.position, false)
                .ok()
                .flatten()
                .map(|id| tab.stack_list_member_at_point(id, &event.position));
            (selectable, unselectable)
        } else {
            (None, None)
        };

        Ok(MouseEventContext {
            pane_id_at_position,
            active_pane_id,
            floating_visible,
            pane_being_resized: tab.pane_being_resized_with_mouse.is_some(),
            selecting_with_mouse: tab.selecting_with_mouse_in_pane.is_some(),
            seam_being_dragged: tab.overlay_panes.seam_is_being_dragged(),
            pane_being_moved: if tab.overlay_panes.pane_is_being_moved_with_mouse() {
                Some(MouseDragTarget::Overlay)
            } else if tab.floating_panes.pane_is_being_moved_with_mouse() {
                Some(MouseDragTarget::Floating)
            } else {
                None
            },
            clicked_pane,
            advanced_mouse_actions: tab.advanced_mouse_actions,
            pinned_selectable,
            pinned_unselectable,
            focus_follows_mouse: tab.focus_follows_mouse,
            mouse_click_through: tab.mouse_click_through,
            mouse_scroll_resize: tab.mouse_scroll_resize,
            context_menu_enabled: tab.context_menu_on_right_click,
            passthrough_pane_id,
        })
    }

    fn gather_clicked_pane_details(
        tab: &mut Tab,
        pane_id: PaneId,
        position: &Position,
        active_pane_id: Option<PaneId>,
        event: &MouseEvent,
        client_id: ClientId,
    ) -> Option<ClickedPaneDetails> {
        let is_floating = tab.floating_panes.panes_contain(&pane_id);
        let is_overlay = tab.pane_is_overlay(pane_id);
        let overlay_is_resizable = tab.overlay_panes.is_resizable(&pane_id);
        let overlay_is_movable = !tab.overlay_panes.pane_is_fullscreen(&pane_id);
        let on_seam = tab.overlay_panes.seam_at(position);
        let is_hidden_stack_list_member = tab.pane_is_hidden_stack_list_member(&pane_id);
        let overlay_frame = is_overlay && tab.overlay_panes.position_is_on_frame(pane_id, position);
        let outer = tab.overlay_panes.outer_geom(pane_id);
        let pane = Self::get_pane_at(tab, position, false).ok()??;

        let on_frame = if is_overlay {
            overlay_frame || on_seam.is_some()
        } else {
            !is_hidden_stack_list_member && pane.position_is_on_frame(position)
        };
        let frame_intercepted = on_frame && pane.intercept_mouse_event_on_frame(event, client_id);
        let edge = if overlay_frame {
            outer.map(|geom| {
                match (
                    (position.line() as usize) < geom.y + geom.rows.as_usize() / 2,
                    position.column() < geom.x + geom.cols.as_usize() / 2,
                ) {
                    (true, true) => PaneEdge::TopLeft,
                    (true, false) => PaneEdge::TopRight,
                    (false, true) => PaneEdge::BottomLeft,
                    (false, false) => PaneEdge::BottomRight,
                }
            })
        } else if on_frame {
            pane.get_edge_at_position(position)
        } else {
            None
        };
        let terminal_wants_mouse = if Some(pane_id) == active_pane_id {
            let relative_position = pane.relative_position(position);
            pane.mouse_left_click(&relative_position, false).is_some()
        } else {
            false
        };
        let terminal_emulator_wants_mouse = pane.terminal_emulator_wants_mouse();

        Some(ClickedPaneDetails {
            pane_id,
            on_frame,
            frame_intercepted,
            edge,
            is_floating,
            is_overlay,
            overlay_is_resizable,
            overlay_is_movable,
            on_seam,
            terminal_wants_mouse,
            terminal_emulator_wants_mouse,
        })
    }

    fn start_pane_resize_with_mouse(
        tab: &mut Tab,
        pane_id: PaneId,
        edge: PaneEdge,
        position: Position,
        _client_id: ClientId,
    ) -> Result<()> {
        let err_context = || format!("failed to start pane resize for pane {pane_id:?}");

        let target = if tab.floating_panes.panes_contain(&pane_id) {
            ResizeDragTarget::Floating
        } else if tab.overlay_panes.panes_contain(&pane_id) {
            ResizeDragTarget::Overlay
        } else {
            ResizeDragTarget::Tiled
        };

        let start_geom = match target {
            ResizeDragTarget::Overlay => tab
                .overlay_panes
                .outer_geom(pane_id)
                .with_context(err_context)?,
            ResizeDragTarget::Floating | ResizeDragTarget::Tiled => tab
                .rendered_pane(pane_id)
                .map(|pane| pane.position_and_size())
                .with_context(err_context)?,
        };

        tab.pane_being_resized_with_mouse = Some(PaneResizeState {
            pane_id,
            edge,
            start_position: position,
            start_geom,
            target,
        });

        Ok(())
    }

    fn continue_pane_resize_with_mouse(
        tab: &mut Tab,
        current_position: Position,
        _client_id: ClientId,
    ) -> Result<bool> {
        let err_context = || "failed to continue pane resize with mouse";

        if let Some(state) = &tab.pane_being_resized_with_mouse {
            let exists = match state.target {
                ResizeDragTarget::Floating => tab.floating_panes.panes_contain(&state.pane_id),
                ResizeDragTarget::Overlay => tab.overlay_panes.panes_contain(&state.pane_id),
                ResizeDragTarget::Tiled => tab.tiled_panes.panes_contain(&state.pane_id),
            };
            if !exists {
                tab.pane_being_resized_with_mouse = None;
                return Ok(false);
            }
        }

        let (pane_id, edge, target, delta_x, delta_y) =
            if let Some(resize_state) = &tab.pane_being_resized_with_mouse {
                let delta_x = current_position.column() as isize
                    - resize_state.start_position.column() as isize;
                let delta_y = current_position.line() - resize_state.start_position.line();

                if delta_x == 0 && delta_y == 0 {
                    return Ok(false);
                }

                (
                    resize_state.pane_id,
                    resize_state.edge,
                    resize_state.target,
                    delta_x,
                    delta_y,
                )
            } else {
                return Ok(true);
            };

        let strategies = edge_and_delta_to_strategies(edge, delta_x, delta_y);

        let resize_result = match target {
            ResizeDragTarget::Floating => Self::resize_floating_pane_with_strategies(
                tab,
                pane_id,
                &strategies,
                (delta_x.unsigned_abs(), delta_y.unsigned_abs()),
            ),
            ResizeDragTarget::Overlay => Self::resize_overlay_with_strategies(
                tab,
                pane_id,
                &strategies,
                (delta_x.unsigned_abs(), delta_y.unsigned_abs()),
            ),
            ResizeDragTarget::Tiled => Self::resize_tiled_pane_with_strategies(
                tab,
                pane_id,
                &strategies,
                (delta_x.abs() as f64, delta_y.abs() as f64),
            ),
        };
        if resize_result.is_err() {
            tab.pane_being_resized_with_mouse = None;
        }
        resize_result.with_context(err_context)?;

        if let Some(resize_state) = tab.pane_being_resized_with_mouse.as_mut() {
            resize_state.start_position = current_position;
        }

        tab.set_force_render();

        Ok(true)
    }

    fn stop_pane_resize_with_mouse(
        tab: &mut Tab,
        final_position: Position,
        client_id: ClientId,
    ) -> Result<bool> {
        let err_context = || "failed to stop pane resize with mouse";

        let start_geom = tab
            .pane_being_resized_with_mouse
            .as_ref()
            .map(|p| p.start_geom);
        let pane_id = tab
            .pane_being_resized_with_mouse
            .as_ref()
            .map(|p| p.pane_id);
        let resize_result = Self::continue_pane_resize_with_mouse(tab, final_position, client_id)
            .with_context(err_context);
        tab.pane_being_resized_with_mouse = None;
        resize_result?;
        let last_geom = pane_id
            .and_then(|pane_id| tab.get_pane_with_id(pane_id))
            .map(|p| p.position_and_size());
        let never_resized = match (start_geom, last_geom) {
            (Some(start_geom), Some(last_geom)) => start_geom == last_geom,
            _ => false,
        };

        Ok(never_resized)
    }

    fn resize_floating_pane_with_strategies(
        tab: &mut Tab,
        pane_id: PaneId,
        strategies: &[ResizeStrategy],
        change_by: (usize, usize),
    ) -> Result<()> {
        let err_context = || format!("failed to resize floating pane {pane_id:?}");

        tab.floating_panes
            .resize_pane_with_strategies(pane_id, strategies, change_by)
            .with_context(err_context)?;

        tab.swap_layouts.set_is_floating_damaged();

        Ok(())
    }

    /// The overlay twin of `resize_floating_pane_with_strategies`. No swap-layout damage flag:
    /// an overlay is not part of a swap layout, and the collection re-renders itself.
    fn resize_overlay_with_strategies(
        tab: &mut Tab,
        pane_id: PaneId,
        strategies: &[ResizeStrategy],
        change_by: (usize, usize),
    ) -> Result<()> {
        let err_context = || format!("failed to resize overlay {pane_id:?}");

        tab.overlay_panes
            .resize_pane_with_strategies(pane_id, strategies, change_by)
            .with_context(err_context)?;

        Ok(())
    }

    fn resize_tiled_pane_with_strategies(
        tab: &mut Tab,
        pane_id: PaneId,
        strategies: &[ResizeStrategy],
        change_by: (f64, f64),
    ) -> Result<()> {
        let err_context = || format!("failed to resize tiled pane {pane_id:?}");

        let viewport = tab.viewport.borrow();
        let viewport_cols = viewport.cols;
        let viewport_rows = viewport.rows;

        let change_by_percent = (
            if viewport_cols > 0 {
                (change_by.0 / viewport_cols as f64) * 100.0
            } else {
                0.0
            },
            if viewport_rows > 0 {
                (change_by.1 / viewport_rows as f64) * 100.0
            } else {
                0.0
            },
        );

        tab.tiled_panes
            .resize_pane_with_strategies(pane_id, strategies, change_by_percent)
            .with_context(err_context)?;

        tab.swap_layouts.set_is_tiled_damaged();

        Ok(())
    }

    fn resize_tiled_pane_with_stacked_resize(
        tab: &mut Tab,
        pane_id: PaneId,
        strategy: &ResizeStrategy,
    ) -> Result<()> {
        let err_context = || format!("failed to resize tiled pane {pane_id:?}");

        tab.tiled_panes
            .stacked_resize_pane_with_id(pane_id, strategy, Some((5.0, 5.0)))
            .with_context(err_context)?;

        tab.swap_layouts.set_is_tiled_damaged();

        Ok(())
    }

    fn execute_mouse_action(
        tab: &mut Tab,
        action: MouseAction,
        event: &MouseEvent,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context =
            || format!("failed to execute mouse action {action:?} for client {client_id}");

        let preserves_help_text = matches!(&action, MouseAction::UpdateHover { .. })
            || matches!(
                &action,
                MouseAction::SendToTerminal { event, .. }
                    if event.event_type == MouseEventType::Motion
            );
        if !preserves_help_text {
            tab.mouse_help_text_visible.remove(&client_id);
        }

        match action {
            MouseAction::GroupToggle(pane_id) => {
                if let Some(pane) = tab.get_pane_with_id_mut(pane_id) {
                    let relative_position = pane.relative_position(&event.position);
                    if let Some((hit_plugin_id, pattern, matched_string, context)) =
                        pane.plugin_highlight_at(&relative_position)
                    {
                        let _ = tab
                            .senders
                            .send_to_plugin(PluginInstruction::HighlightClicked {
                                plugin_id: hit_plugin_id,
                                client_id,
                                pane_id,
                                pattern,
                                matched_string,
                                context,
                            });
                        return Ok(MouseEffect::state_changed());
                    }
                }
                Ok(MouseEffect::group_toggle(pane_id))
            },
            MouseAction::OpenContextMenu {
                pane_id,
                position,
                on_frame,
            } => Self::execute_open_context_menu(tab, pane_id, position, on_frame, client_id),
            MouseAction::GroupAdd(pane_id) => Ok(MouseEffect::group_add(pane_id)),
            MouseAction::Ungroup => Ok(MouseEffect::ungroup()),
            MouseAction::StartResize {
                pane_id,
                edge,
                target: _,
                position,
            } => {
                clear_hover_for_client(tab, client_id);
                Self::start_pane_resize_with_mouse(tab, pane_id, edge, position, client_id)
                    .with_context(err_context)?;
                Ok(MouseEffect::state_changed())
            },
            MouseAction::ContinueResize { position } => {
                let state_changed = Self::continue_pane_resize_with_mouse(tab, position, client_id)
                    .with_context(err_context)?;
                if state_changed {
                    Ok(MouseEffect::state_changed())
                } else {
                    Ok(MouseEffect::default())
                }
            },
            MouseAction::StopResize { position } => {
                Self::execute_stop_resize(tab, position, client_id)
            },
            MouseAction::FocusPane {
                pane_id: _,
                position,
            } => Self::execute_focus_pane(tab, position, client_id),
            MouseAction::FocusPaneAndClickThrough {
                pane_id: _,
                position,
                event: click_event,
            } => Self::execute_focus_pane_and_click_through(tab, position, click_event, client_id),
            MouseAction::ShowFloatingPanesAndFocus { pane_id: _ } => {
                Self::focus_pane_at(tab, &event.position, client_id).with_context(err_context)?;
                Ok(MouseEffect::state_changed())
            },
            MouseAction::StartSelection { pane_id, position } => {
                let osc133_command_selection = tab.osc133_command_selection;
                let word_separators = tab.word_separators.clone();
                let pane = tab
                    .get_pane_with_id_mut(pane_id)
                    .ok_or_else(|| anyhow!("Failed to find pane {pane_id:?}"))?;
                let relative_position = pane.relative_position(&position);

                let mut leave_clipboard_message = false;
                pane.set_selection_options(osc133_command_selection, &word_separators);
                pane.start_selection(&relative_position, client_id);
                if pane.get_selected_text(client_id).is_some() {
                    leave_clipboard_message = true;
                }
                if drag_should_be_tracked(pane_id, &**pane) {
                    tab.selecting_with_mouse_in_pane = Some(pane_id);
                }
                if leave_clipboard_message {
                    Ok(MouseEffect::state_changed_and_leave_clipboard_message())
                } else {
                    Ok(MouseEffect::default())
                }
            },
            MouseAction::UpdateSelection { position } => {
                if let Some(pane_id_with_selection) = tab.selecting_with_mouse_in_pane {
                    if let Some(pane_with_selection) =
                        tab.get_pane_with_id_mut(pane_id_with_selection)
                    {
                        let relative_position = pane_with_selection.relative_position(&position);
                        pane_with_selection.update_selection(&relative_position, client_id);
                    }
                }
                Ok(MouseEffect::default())
            },
            MouseAction::EndSelection { position } => {
                Self::execute_end_selection(tab, position, client_id)
            },
            MouseAction::StartMovingFloatingPane { position } => {
                Self::execute_move_floating_pane(tab, position)
            },
            MouseAction::ContinueMovingFloatingPane { position } => {
                Self::execute_move_floating_pane(tab, position)
            },
            MouseAction::StopMovingFloatingPane { position } => {
                Self::execute_stop_moving_floating_pane(tab, position, client_id)
            },
            MouseAction::StartMovingOverlay { position } => {
                Self::execute_move_overlay(tab, position)
            },
            MouseAction::ContinueMovingOverlay { position } => {
                Self::execute_move_overlay(tab, position)
            },
            MouseAction::StopMovingOverlay { position } => {
                Self::execute_stop_moving_overlay(tab, position, client_id)
            },
            MouseAction::StartOverlaySeamDrag {
                left,
                right,
                position,
            } => {
                tab.overlay_panes.start_seam_drag(left, right, position);
                Ok(MouseEffect::default())
            },
            MouseAction::DragOverlaySeam { position } => {
                Self::execute_drag_overlay_seam(tab, position)
            },
            MouseAction::StopOverlaySeamDrag { position } => {
                tab.overlay_panes.stop_seam_drag(position);
                tab.set_force_render();
                Ok(MouseEffect::state_changed())
            },
            MouseAction::ScrollUp { pane_id: _, lines } => {
                Self::handle_scrollwheel_up(tab, &event.position, lines, client_id)
                    .with_context(err_context)
            },
            MouseAction::ScrollDown { pane_id: _, lines } => {
                Self::handle_scrollwheel_down(tab, &event.position, lines, client_id)
                    .with_context(err_context)
            },
            MouseAction::ScrollLeft { pane_id, cols } => {
                let scroll_right = false;
                Self::handle_scrollwheel_horizontal(
                    tab,
                    pane_id,
                    &event.position,
                    cols,
                    scroll_right,
                    client_id,
                )
                .with_context(err_context)
            },
            MouseAction::ScrollRight { pane_id, cols } => {
                let scroll_right = true;
                Self::handle_scrollwheel_horizontal(
                    tab,
                    pane_id,
                    &event.position,
                    cols,
                    scroll_right,
                    client_id,
                )
                .with_context(err_context)
            },
            MouseAction::ResizeScrollUp { pane_id } => {
                Self::handle_resize_scroll(tab, pane_id, Resize::Increase, client_id)
                    .with_context(err_context)
            },
            MouseAction::ResizeScrollDown { pane_id } => {
                Self::handle_resize_scroll(tab, pane_id, Resize::Decrease, client_id)
                    .with_context(err_context)
            },
            MouseAction::ScrollToPreviousPrompt { pane_id } => {
                Self::handle_prompt_jump(tab, pane_id, true, event, client_id)
                    .with_context(err_context)
            },
            MouseAction::ScrollToNextPrompt { pane_id } => {
                Self::handle_prompt_jump(tab, pane_id, false, event, client_id)
                    .with_context(err_context)
            },
            MouseAction::UpdateHover { pane_id, position } => {
                Self::execute_update_hover(tab, pane_id, position, client_id)
            },
            MouseAction::FocusOnHover { pane_id, position } => {
                Self::execute_focus_on_hover(tab, pane_id, position, client_id)
            },
            MouseAction::SendToTerminal { pane_id, event } => {
                Self::execute_send_to_terminal(tab, pane_id, event, client_id)
            },
            MouseAction::FrameIntercepted { pane_id: _ } => {
                tab.set_force_render();
                Ok(MouseEffect::state_changed())
            },
            MouseAction::NoAction => Ok(MouseEffect::default()),
        }
    }

    fn execute_stop_resize(
        tab: &mut Tab,
        position: Position,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || "failed to stop resize";
        let never_resized = Self::stop_pane_resize_with_mouse(tab, position, client_id)
            .with_context(err_context)?;
        if never_resized {
            let pane_id_at_position = Self::get_pane_at(tab, &position, false)
                .with_context(err_context)?
                .map(|p| p.pid());
            let active_pane_id = tab
                .get_active_pane_id(client_id)
                .ok_or_else(|| anyhow!("Failed to find active pane"))?;
            if let Some(pane_id) = pane_id_at_position {
                if pane_id != active_pane_id {
                    Self::focus_pane_at(tab, &position, client_id).with_context(err_context)?;
                }
            }
        }
        Ok(MouseEffect::state_changed())
    }

    /// Focus the clicked pane and gather everything the menu's hook plugin will be told.
    ///
    /// Focusing first, and with `focus_pane_at` rather than `execute_focus_pane`, is the whole
    /// trick of this feature:
    ///
    /// - **Focus first** so that every focus-relative action a menu item runs - `Copy`,
    ///   `CloseFocus`, `TogglePaneFrames` - means the pane the user right-clicked. Because a menu
    ///   is not a pane and takes no focus, nothing has to hand it back afterwards.
    /// - **`focus_pane_at`, not `execute_focus_pane`**, because the latter ends by starting a
    ///   selection, which would wipe the selection this is about to record. A menu that offers
    ///   "Copy" and then copies nothing is worse than no menu.
    fn execute_open_context_menu(
        tab: &mut Tab,
        pane_id: PaneId,
        position: Position,
        on_frame: bool,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || "failed to open context menu";
        clear_hover_for_client(tab, client_id);
        Self::focus_pane_at(tab, &position, client_id).with_context(err_context)?;

        if tab.get_active_pane_id(client_id) != Some(pane_id) {
            return Ok(MouseEffect::state_changed());
        }

        let tab_position = tab.position;
        let is_floating = tab.floating_panes.panes_contain(&pane_id);
        let Some(pane) = tab.get_pane_with_id(pane_id) else {
            return Ok(MouseEffect::state_changed());
        };
        // Both coordinate systems are kept: the absolute one anchors the menu, the pane-relative
        // one is what a plugin reasoning about pane content wants. Neither derives from the other
        // once the context has left here.
        let relative_position = pane.relative_position(&position);
        let text_around = pane
            .text_around_position(&relative_position)
            .unwrap_or_default();
        let link = pane.plugin_highlight_at(&relative_position).map(
            |(plugin_id, pattern, matched, context)| ContextMenuLink {
                plugin_id,
                pattern,
                matched,
                context,
            },
        );
        let pane_command = match pane.invoked_with() {
            Some(Run::Command(command)) => Some(command.to_string()),
            _ => None,
        };
        let context = ContextMenuContext {
            // Filled in by `Screen::open_context_menu`, which owns the counter.
            request_id: ContextMenuRequestId::new(0),
            client_id,
            pane_id,
            tab_position,
            click: (position.column(), position.line().max(0) as usize),
            click_in_pane: (
                relative_position.column(),
                relative_position.line().max(0) as usize,
            ),
            on_frame,
            pane_title: pane.current_title(),
            pane_command,
            pane_exited: pane.exited(),
            pane_exit_status: pane.exit_status(),
            pane_is_held: pane.is_held(),
            pane_is_floating: is_floating,
            selected_text: pane
                .get_selected_text(client_id)
                .filter(|text| !text.is_empty()),
            word_under_cursor: text_around.word,
            line_under_cursor: text_around.line,
            link,
            // The configured items are attached by the plugin thread, which is where the
            // configuration lives. The tab knows the pane; it does not know the menu.
            items: vec![],
        };
        Ok(MouseEffect::open_context_menu(context))
    }
    fn execute_focus_pane(
        tab: &mut Tab,
        position: Position,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || "failed to focus pane";
        let clicked_unselectable_pane_id =
            Self::unselectable_pane_at_position(tab, &position).map(|pane| pane.pid());
        clear_hover_for_client_except(tab, client_id, clicked_unselectable_pane_id);
        let active_pane_id_before = tab
            .get_active_pane_id(client_id)
            .ok_or_else(|| anyhow!("Failed to find active pane"))?;

        Self::focus_pane_at(tab, &position, client_id).with_context(err_context)?;

        let osc133_command_selection = tab.osc133_command_selection;
        let word_separators = tab.word_separators.clone();
        if let Some(pane_at_position) = Self::unselectable_pane_at_position(tab, &position) {
            let relative_position = pane_at_position.relative_position(&position);
            pane_at_position.set_selection_options(osc133_command_selection, &word_separators);
            pane_at_position.start_selection(&relative_position, client_id);
        }

        // `focus_pane_at` above already swallowed the click if it landed on an overlay - but
        // swallowing it there is not enough, because this is a second door into the same drag:
        // `move_pane_with_mouse` re-resolves the position through `FloatingPanes` alone and
        // would pick up whatever the overlay is covering.
        if tab.floating_panes.panes_are_visible() && !tab.position_is_on_an_overlay(&position) {
            let search_selectable = false;
            let moved_pane_with_mouse = tab
                .floating_panes
                .move_pane_with_mouse(position, search_selectable);
            let active_pane_id_after = tab
                .get_active_pane_id(client_id)
                .ok_or_else(|| anyhow!("Failed to find active pane"))?;
            if moved_pane_with_mouse || active_pane_id_before != active_pane_id_after {
                return Ok(MouseEffect::state_changed());
            } else {
                return Ok(MouseEffect::default());
            }
        }

        let active_pane_id_after = tab
            .get_active_pane_id(client_id)
            .ok_or_else(|| anyhow!("Failed to find active pane"))?;
        if active_pane_id_before != active_pane_id_after {
            Ok(MouseEffect::state_changed())
        } else {
            Ok(MouseEffect::default())
        }
    }

    fn execute_focus_pane_and_click_through(
        tab: &mut Tab,
        position: Position,
        click_event: MouseEvent,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || "failed to focus pane and click through";

        // The pane about to be clicked keeps its hover: the pointer is on it, and a plugin
        // told otherwise takes down whatever the hover revealed before the click arrives.
        let clicked_unselectable_pane_id =
            Self::unselectable_pane_at_position(tab, &position).map(|pane| pane.pid());
        clear_hover_for_client_except(tab, client_id, clicked_unselectable_pane_id);
        Self::focus_pane_at(tab, &position, client_id).with_context(err_context)?;

        let osc133_command_selection = tab.osc133_command_selection;
        let word_separators = tab.word_separators.clone();
        if let Some(pane_at_position) = Self::unselectable_pane_at_position(tab, &position) {
            let relative_position = pane_at_position.relative_position(&position);
            pane_at_position.set_selection_options(osc133_command_selection, &word_separators);
            pane_at_position.start_selection(&relative_position, client_id);
            return Ok(MouseEffect::state_changed());
        }

        let active_pane_id = tab
            .get_active_pane_id(client_id)
            .ok_or_else(|| anyhow!("Failed to find active pane"))
            .with_context(err_context)?;

        let pane = tab
            .get_pane_with_id(active_pane_id)
            .ok_or_else(|| anyhow!("Failed to find pane {active_pane_id:?}"))
            .with_context(err_context)?;

        let terminal_wants_mouse = pane.terminal_emulator_wants_mouse();

        if terminal_wants_mouse {
            let relative_position = pane.relative_position(&click_event.position);
            let mut event_for_pane = click_event;
            event_for_pane.position = relative_position;
            if let Some(mouse_event) = pane.mouse_event(&event_for_pane, client_id) {
                if !pane.position_is_on_frame(&click_event.position) {
                    tab.clear_search(client_id);
                    tab.mouse_help_text_visible.clear();
                    Self::write_to_pane(tab, mouse_event.into_bytes(), active_pane_id, client_id)
                        .with_context(err_context)?;
                }
            }
        } else {
            if let Some(pane) = tab.get_pane_with_id_mut(active_pane_id) {
                let relative_position = pane.relative_position(&position);
                pane.set_selection_options(osc133_command_selection, &word_separators);
                pane.start_selection(&relative_position, client_id);
                if drag_should_be_tracked(active_pane_id, &**pane) {
                    tab.selecting_with_mouse_in_pane = Some(active_pane_id);
                }
            }
        }

        Ok(MouseEffect::state_changed())
    }

    fn execute_end_selection(
        tab: &mut Tab,
        position: Position,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || "failed to end selection";
        let mut leave_clipboard_message = false;
        let copy_on_release = tab.copy_on_select;

        if let Some(pane_with_selection) = tab
            .selecting_with_mouse_in_pane
            .and_then(|p_id| tab.get_pane_with_id_mut(p_id))
        {
            let mut relative_position = pane_with_selection.relative_position(&position);

            relative_position.change_column(
                (relative_position.column()).min(pane_with_selection.get_content_columns()),
            );

            relative_position.change_line(
                (relative_position.line())
                    .max(0)
                    .min(pane_with_selection.get_content_rows() as isize),
            );

            if let Some(mouse_event) =
                pane_with_selection.mouse_left_click_release(&relative_position)
            {
                let pane_id = pane_with_selection.pid();
                tab.clear_search(client_id);
                tab.mouse_help_text_visible.clear();
                Self::write_to_pane(tab, mouse_event.into_bytes(), pane_id, client_id)
                    .with_context(err_context)?;
            } else {
                let relative_position = pane_with_selection.relative_position(&position);
                pane_with_selection.end_selection(&relative_position, client_id);
                if pane_with_selection.supports_mouse_selection() && copy_on_release {
                    let selected_text = pane_with_selection.get_selected_text(client_id);
                    if let Some(selected_text) = selected_text {
                        leave_clipboard_message = true;
                        tab.write_selection_to_clipboard(&selected_text)
                            .with_context(err_context)?;
                    }
                }
                tab.selecting_with_mouse_in_pane = None;
            }
        }

        if leave_clipboard_message {
            Ok(MouseEffect::leave_clipboard_message())
        } else {
            Ok(MouseEffect::default())
        }
    }

    fn execute_move_floating_pane(tab: &mut Tab, position: Position) -> Result<MouseEffect> {
        let search_selectable = false;
        if tab
            .floating_panes
            .move_pane_with_mouse(position, search_selectable)
        {
            tab.swap_layouts.set_is_floating_damaged();
            tab.set_force_render();
            Ok(MouseEffect::state_changed())
        } else {
            Ok(MouseEffect::default())
        }
    }

    /// Deliberately no `swap_layouts.set_is_floating_damaged()`: an overlay is not on the floating
    /// layer and has no slot in a swap layout, so moving one damages nothing.
    fn execute_move_overlay(tab: &mut Tab, position: Position) -> Result<MouseEffect> {
        if tab.overlay_panes.move_pane_with_mouse(position) {
            tab.set_force_render();
            Ok(MouseEffect::state_changed())
        } else {
            Ok(MouseEffect::default())
        }
    }

    /// A seam press never changes focus, like a frame drag: the user is adjusting the panel,
    /// not choosing a half of it. Nothing to do with the floating layer's swap layouts either,
    /// for the reason `execute_move_overlay` gives.
    fn execute_drag_overlay_seam(tab: &mut Tab, position: Position) -> Result<MouseEffect> {
        if tab.overlay_panes.drag_seam_to(&position) {
            tab.set_force_render();
            Ok(MouseEffect::state_changed())
        } else {
            Ok(MouseEffect::default())
        }
    }

    fn execute_stop_moving_overlay(
        tab: &mut Tab,
        position: Position,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || "failed to stop moving overlay";
        let never_moved = tab.overlay_panes.stop_moving_pane_with_mouse(position);
        if never_moved {
            // the press was a click after all. `focus_pane_at` is the right thing to hand it to
            // rather than a focus of the dragged pane by id: it already routes an overlay position
            // to the overlay collection, and it already declines to move the keyboard onto a
            // non-modal while a modal is open.
            Self::focus_pane_at(tab, &position, client_id).with_context(err_context)?;
        }
        Ok(MouseEffect::default())
    }

    fn execute_stop_moving_floating_pane(
        tab: &mut Tab,
        position: Position,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || "failed to stop moving floating pane";
        let never_moved = tab.floating_panes.stop_moving_pane_with_mouse(position);
        if never_moved {
            let active_pane_id = tab
                .get_active_pane_id(client_id)
                .ok_or_else(|| anyhow!("Failed to find active pane"))?;
            let pane_id_at_position = Self::get_pane_at(tab, &position, false)
                .with_context(err_context)?
                .ok_or_else(|| anyhow!("Failed to find pane at position"))?
                .pid();
            if active_pane_id != pane_id_at_position {
                Self::focus_pane_at(tab, &position, client_id).with_context(err_context)?;
            }
        }
        Ok(MouseEffect::default())
    }

    fn execute_focus_on_hover(
        tab: &mut Tab,
        pane_id: PaneId,
        position: Position,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || format!("failed to focus pane on hover for client {client_id}");

        let is_selectable = tab
            .get_pane_with_id(pane_id)
            .map(|p| p.selectable())
            .unwrap_or(false);
        if !is_selectable {
            return Self::execute_update_hover(tab, Some(pane_id), Some(position), client_id);
        }

        let floating_visible = tab.floating_panes.panes_are_visible();
        // an overlay is painted over everything with no layer-visibility precondition - which is
        // the whole reason it is not a floating pane - so it passes this test the way a pane on
        // the visible layer does, rather than reading as "underneath something" and being
        // reduced to a hover
        let is_on_top =
            tab.floating_panes.get_pane(pane_id).is_some() || tab.pane_is_overlay(pane_id);
        if floating_visible && !is_on_top {
            return Self::execute_update_hover(tab, Some(pane_id), Some(position), client_id);
        }

        let is_stacked_one_liner = tab
            .get_pane_with_id(pane_id)
            .map(|p| {
                let geom = p.current_geom();
                geom.is_stacked() && geom.rows.is_fixed()
            })
            .unwrap_or(false);
        if is_stacked_one_liner {
            return Self::execute_update_hover(tab, Some(pane_id), Some(position), client_id);
        }

        if tab.pane_is_hidden_stack_list_member(&pane_id) {
            return Self::execute_update_hover(tab, Some(pane_id), Some(position), client_id);
        }

        let active_pane_id = tab.get_active_pane_id(client_id);
        if active_pane_id == Some(pane_id) {
            return Self::execute_update_hover(tab, Some(pane_id), Some(position), client_id);
        }

        Self::focus_pane_at(tab, &position, client_id).with_context(err_context)?;

        clear_hover_for_client(tab, client_id);

        Ok(MouseEffect::state_changed())
    }

    fn execute_update_hover(
        tab: &mut Tab,
        pane_id: Option<PaneId>,
        position: Option<Position>,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let previous_hover_pane_id = tab.mouse_hover_pane_id.get(&client_id).copied();

        if tab.mouse_hover_effects {
            let previous_plugin_hover_pane_id = tab.plugin_hover_pane_id.get(&client_id).copied();
            let current_plugin_hover_pane_id = match pane_id {
                Some(pid) if matches!(pid, PaneId::Plugin(_)) => Some(pid),
                _ => None,
            };
            if let (Some(pid), Some(position)) = (current_plugin_hover_pane_id, position) {
                if let Some(pane) = tab.get_pane_with_id(pid) {
                    let relative_position = pane.relative_position(&position);
                    let _ = pane.mouse_event(
                        &MouseEvent::new_buttonless_motion(relative_position),
                        client_id,
                    );
                }
            }
            if previous_plugin_hover_pane_id != current_plugin_hover_pane_id {
                if let Some(previous_pid) = previous_plugin_hover_pane_id {
                    if let Some(pane) = tab.get_pane_with_id(previous_pid) {
                        let _ = pane.mouse_event(
                            &MouseEvent::new_buttonless_motion(Position::new(0, u16::MAX)),
                            client_id,
                        );
                    }
                }
                match current_plugin_hover_pane_id {
                    Some(pid) => {
                        tab.plugin_hover_pane_id.insert(client_id, pid);
                    },
                    None => {
                        tab.plugin_hover_pane_id.remove(&client_id);
                    },
                }
            }
        }
        match pane_id {
            Some(pid) => {
                if let Some(pane) = tab.get_pane_with_id(pid) {
                    let pane_is_selectable = pane.selectable();
                    if tab.advanced_mouse_actions && tab.mouse_hover_effects && pane_is_selectable {
                        tab.mouse_hover_pane_id.insert(client_id, pid);
                    } else if tab.advanced_mouse_actions || !tab.mouse_hover_effects {
                        tab.mouse_hover_pane_id.remove(&client_id);
                    }
                    tab.mouse_last_pane_id.insert(client_id, pid);
                }
            },
            None => {
                tab.mouse_last_pane_id.remove(&client_id);
                tab.mouse_hover_pane_id.remove(&client_id);
            },
        }
        let mut should_render =
            previous_hover_pane_id != tab.mouse_hover_pane_id.get(&client_id).copied();

        if let Some(prev_pane_id) = previous_hover_pane_id {
            if Some(prev_pane_id) != pane_id {
                if let Some(pane) = tab.get_pane_with_id_mut(prev_pane_id) {
                    should_render |= pane.set_hover_position(None);
                }
            }
        }

        if tab.mouse_help_text_visible.remove(&client_id).is_some() {
            should_render = true;
        }

        let mut mouse_effect = if should_render {
            MouseEffect::state_changed()
        } else {
            MouseEffect::default()
        };
        mouse_effect.leave_clipboard_message = true;
        Ok(mouse_effect)
    }

    fn execute_send_to_terminal(
        tab: &mut Tab,
        pane_id: PaneId,
        event: MouseEvent,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || format!("failed to send to terminal for pane {pane_id:?}");
        let mut should_render = false;
        let active_pane_id = tab
            .get_active_pane_id(client_id)
            .ok_or_else(|| anyhow!("Failed to find active pane"))?;
        if pane_id == active_pane_id {
            let pane = tab
                .get_pane_with_id(pane_id)
                .ok_or_else(|| anyhow!("Failed to find pane {pane_id:?}"))?;
            let relative_position = pane.relative_position(&event.position);
            let mut event_for_pane = event;
            event_for_pane.position = relative_position;
            // Off its content a plugin hears only motion, so its hover can follow the pointer
            // out. Anything else there is on a frame, not outside the content: its own frame
            // (a right press may already have toggled a floating plugin's pin), or a multipane
            // overlay's outer frame or divider, which lie outside the child's geometry yet route
            // to its first child. A terminal's report is written only off its own frame.
            let on_frame = pane.position_is_on_frame(&event.position);
            let off_plugin_content = matches!(pane_id, PaneId::Plugin(_))
                && (on_frame || !pane.contains(&event.position));
            if !off_plugin_content || event.event_type == MouseEventType::Motion {
                if let Some(mouse_event) = pane.mouse_event(&event_for_pane, client_id) {
                    if !on_frame {
                        tab.clear_search(client_id);
                        tab.mouse_help_text_visible.clear();
                        Self::write_to_pane(tab, mouse_event.into_bytes(), pane_id, client_id)
                            .with_context(err_context)?;
                    }
                }
            }
            if clear_hover_for_client(tab, client_id) {
                should_render = true;
            }
            if event.event_type == MouseEventType::Motion {
                if let Some(pane) = tab.get_pane_with_id_mut(pane_id) {
                    if !pane.terminal_emulator_wants_mouse() {
                        let relative = pane.relative_position(&event.position);
                        if pane.set_hover_position(Some(relative)) {
                            should_render = true;
                        }
                    }
                }
            }

            if event.event_type == MouseEventType::Motion && tab.mouse_hover_effects {
                tab.last_mouse_activity_time
                    .insert(client_id, Instant::now());
                let entered_pane = tab.mouse_last_pane_id.get(&client_id) != Some(&pane_id);
                tab.mouse_last_pane_id.insert(client_id, pane_id);
                if entered_pane {
                    let was_visible = tab
                        .mouse_help_text_visible
                        .get(&client_id)
                        .copied()
                        .unwrap_or(false);
                    tab.mouse_help_text_visible.insert(client_id, true);
                    if !was_visible {
                        should_render = true;
                    }

                    tab.senders
                        .send_to_background_jobs(BackgroundJob::ClearHelpText { client_id })
                        .with_context(err_context)?;
                }
            }
        }
        let mouse_effect = if should_render {
            MouseEffect::state_changed()
        } else {
            MouseEffect::default()
        };
        Ok(mouse_effect)
    }

    fn determine_mouse_action(event: &MouseEvent, ctx: &MouseEventContext) -> Result<MouseAction> {
        if ctx.pane_being_resized {
            return Ok(match event.event_type {
                MouseEventType::Motion => MouseAction::ContinueResize {
                    position: event.position,
                },
                MouseEventType::Release => MouseAction::StopResize {
                    position: event.position,
                },
                _ => MouseAction::NoAction,
            });
        }

        if ctx.selecting_with_mouse {
            return Ok(match event.event_type {
                MouseEventType::Motion if event.left => MouseAction::UpdateSelection {
                    position: event.position,
                },
                MouseEventType::Release if event.left => MouseAction::EndSelection {
                    position: event.position,
                },
                _ => MouseAction::NoAction,
            });
        }

        if ctx.seam_being_dragged {
            return Ok(match event.event_type {
                MouseEventType::Motion if event.left => MouseAction::DragOverlaySeam {
                    position: event.position,
                },
                MouseEventType::Release if event.left => MouseAction::StopOverlaySeamDrag {
                    position: event.position,
                },
                _ => MouseAction::NoAction,
            });
        }

        if let Some(drag_target) = ctx.pane_being_moved {
            let (continue_move, stop_move) = match drag_target {
                MouseDragTarget::Floating => (
                    MouseAction::ContinueMovingFloatingPane {
                        position: event.position,
                    },
                    MouseAction::StopMovingFloatingPane {
                        position: event.position,
                    },
                ),
                MouseDragTarget::Overlay => (
                    MouseAction::ContinueMovingOverlay {
                        position: event.position,
                    },
                    MouseAction::StopMovingOverlay {
                        position: event.position,
                    },
                ),
            };
            return Ok(match event.event_type {
                MouseEventType::Motion if event.left => continue_move,
                MouseEventType::Release if event.left => stop_move,
                _ => MouseAction::NoAction,
            });
        }

        if event.alt {
            if let (Some(passthrough_pane_id), Some(details)) =
                (ctx.passthrough_pane_id, ctx.clicked_pane.as_ref())
            {
                if details.pane_id == passthrough_pane_id
                    && !details.on_frame
                    && details.terminal_wants_mouse
                {
                    return Ok(MouseAction::SendToTerminal {
                        pane_id: details.pane_id,
                        event: *event,
                    });
                }
            }

            // alt+wheel jumps between shell prompts; it is one of the advanced mouse actions
            // and is swallowed rather than scrolling when those are off
            if event.wheel_up || event.wheel_down {
                if !ctx.advanced_mouse_actions {
                    return Ok(MouseAction::NoAction);
                }
                if let Some(pane_id) = ctx.pane_id_at_position {
                    if event.wheel_up {
                        return Ok(MouseAction::ScrollToPreviousPrompt { pane_id });
                    }
                    return Ok(MouseAction::ScrollToNextPrompt { pane_id });
                }
                return Ok(MouseAction::NoAction);
            }

            let is_left_press = event.left && event.event_type == MouseEventType::Press;
            let is_left_motion = event.left && event.event_type == MouseEventType::Motion;

            if is_left_press {
                if let Some(pane_id) = ctx.pane_id_at_position {
                    return Ok(MouseAction::GroupToggle(pane_id));
                }
            }
            if is_left_motion {
                if let Some(pane_id) = ctx.pane_id_at_position {
                    return Ok(MouseAction::GroupAdd(pane_id));
                }
            }
            if event.right {
                return Ok(MouseAction::Ungroup);
            }
            return Ok(MouseAction::NoAction);
        }

        if event.wheel_up || event.wheel_down {
            if event.ctrl && !ctx.mouse_scroll_resize {
                return Ok(MouseAction::NoAction);
            }
            if let Some(pane_id) = ctx.pane_id_at_position {
                if event.ctrl {
                    // a modal is sized by the plugin that asked the question, so a scroll over one
                    // is nothing at all - the same answer the ctrl-left-press branch below gives a
                    // modal frame, and for the same reason: falling through would resize whatever
                    // is behind it instead of the pane under the pointer
                    let is_fixed_size_overlay = ctx
                        .clicked_pane
                        .as_ref()
                        .is_some_and(|details| details.is_overlay && !details.overlay_is_resizable);
                    if is_fixed_size_overlay {
                        return Ok(MouseAction::NoAction);
                    }
                    if event.wheel_up {
                        return Ok(MouseAction::ResizeScrollUp { pane_id });
                    }
                    if event.wheel_down {
                        return Ok(MouseAction::ResizeScrollDown { pane_id });
                    }
                }
                if event.wheel_up {
                    return Ok(MouseAction::ScrollUp { pane_id, lines: 3 });
                }
                if event.wheel_down {
                    return Ok(MouseAction::ScrollDown { pane_id, lines: 3 });
                }
            }
            return Ok(MouseAction::NoAction);
        }

        if event.wheel_left || event.wheel_right {
            if let Some(pane_id) = ctx.pane_id_at_position {
                if event.wheel_left {
                    return Ok(MouseAction::ScrollLeft { pane_id, cols: 4 });
                }
                if event.wheel_right {
                    return Ok(MouseAction::ScrollRight { pane_id, cols: 4 });
                }
            }
            return Ok(MouseAction::NoAction);
        }

        let is_ctrl_left_press =
            event.ctrl && event.left && event.event_type == MouseEventType::Press;
        if is_ctrl_left_press {
            let Some(details) = &ctx.clicked_pane else {
                return Ok(MouseAction::NoAction);
            };
            if details.on_frame {
                if details.frame_intercepted {
                    return Ok(MouseAction::FrameIntercepted {
                        pane_id: details.pane_id,
                    });
                }
                // The seam between two linked overlays is the one frame edge that is not the
                // panel's: a ctrl-drag there would resize one half into the other, so it moves
                // the divider exactly as a plain drag does. The panel's outer edges resize.
                if let Some((left, right)) = details.on_seam {
                    return Ok(MouseAction::StartOverlaySeamDrag {
                        left,
                        right,
                        position: event.position,
                    });
                }
                // a modal is not user-resizable, and neither is an overlay expanded over the
                // screen, so a ctrl-drag on one is nothing at all rather than falling through to
                // `start_pane_resize_with_mouse` - which would find it in neither the floating nor
                // the tiled collection and error the whole mouse event
                let may_resize = !details.is_overlay || details.overlay_is_resizable;
                if may_resize {
                    if let Some(edge) = details.edge {
                        return Ok(MouseAction::StartResize {
                            pane_id: details.pane_id,
                            edge,
                            target: resize_target(details),
                            position: event.position,
                        });
                    }
                }
            }
            return Ok(MouseAction::NoAction);
        }

        let is_plain_left_press =
            event.left && event.event_type == MouseEventType::Press && !event.ctrl && !event.alt;
        if is_plain_left_press {
            let Some(details) = &ctx.clicked_pane else {
                return Ok(MouseAction::NoAction);
            };

            let is_active_pane = Some(details.pane_id) == ctx.active_pane_id;
            let is_pinned_pane = ctx
                .pinned_selectable
                .map(|id| id == details.pane_id)
                .unwrap_or(false);

            if details.on_frame {
                if details.frame_intercepted {
                    return Ok(MouseAction::FrameIntercepted {
                        pane_id: details.pane_id,
                    });
                }

                // An overlay drags from its own branch, with no `should_start_moving` test in
                // front of it: that test asks whether the floating *layer* is up, and an overlay
                // is not on a layer. Routing one through the floating branch is how clicking a
                // modal's frame used to drag the pane underneath it - `execute_move_floating_pane`
                // throws the pane id away and re-resolves the position through `FloatingPanes`
                // alone. `execute_move_overlay` never re-resolves past the overlay, because the
                // overlay collection's own hit test cannot see what an overlay is covering.
                //
                // The frame press stays a *move* even though an overlay is now resizable: this
                // is the floating-pane split, which is what "resizable like a floating pane"
                // means - plain drag on the frame moves, ctrl-drag resizes. (A tiled pane is the
                // odd one out, resizing on a plain frame drag, because it has nowhere to move
                // to.)
                //
                // The seam comes first: it is on both halves' frames, so without this check a
                // press on the divider would pick the whole panel up instead of moving the
                // divider. Two touching overlays nobody placed together have no seam.
                if let Some((left, right)) = details.on_seam {
                    return Ok(MouseAction::StartOverlaySeamDrag {
                        left,
                        right,
                        position: event.position,
                    });
                }
                if details.is_overlay {
                    if details.overlay_is_movable {
                        return Ok(MouseAction::StartMovingOverlay {
                            position: event.position,
                        });
                    }
                    // An expanded overlay has nowhere to move to, so `move_pane_with_mouse`
                    // refuses the drag and nothing would ever be set moving - the press would be
                    // swallowed and the click never land. It is an ordinary click on the pane
                    // instead. Spelled out rather than left to fall through, because the next
                    // branch is the floating one, and routing an overlay into it is the very bug
                    // the comment above records.
                    return Ok(MouseAction::FocusPane {
                        pane_id: details.pane_id,
                        position: event.position,
                    });
                }
                let should_start_moving = ctx.floating_visible || is_pinned_pane;
                if should_start_moving {
                    return Ok(MouseAction::StartMovingFloatingPane {
                        position: event.position,
                    });
                }

                if let Some(edge) = details.edge {
                    return Ok(MouseAction::StartResize {
                        pane_id: details.pane_id,
                        edge,
                        target: ResizeDragTarget::Tiled,
                        position: event.position,
                    });
                }
            }

            if is_active_pane {
                if details.terminal_wants_mouse {
                    return Ok(MouseAction::SendToTerminal {
                        pane_id: details.pane_id,
                        event: *event,
                    });
                } else {
                    return Ok(MouseAction::StartSelection {
                        pane_id: details.pane_id,
                        position: event.position,
                    });
                }
            }

            if !ctx.floating_visible {
                if let Some(pinned_id) = ctx.pinned_selectable {
                    return Ok(MouseAction::ShowFloatingPanesAndFocus { pane_id: pinned_id });
                }
                if ctx.pinned_unselectable.is_some() {
                    return Ok(MouseAction::NoAction);
                }
            }

            if ctx.mouse_click_through && !ctx.focus_follows_mouse {
                return Ok(MouseAction::FocusPaneAndClickThrough {
                    pane_id: details.pane_id,
                    position: event.position,
                    event: *event,
                });
            } else {
                return Ok(MouseAction::FocusPane {
                    pane_id: details.pane_id,
                    position: event.position,
                });
            }
        }

        if event.right {
            let Some(pane_id) = ctx.pane_id_at_position else {
                return Ok(MouseAction::NoAction);
            };
            // Each guard here earns its place:
            //
            // - **press only**, because the release that follows would otherwise open a second
            //   menu on top of the first;
            // - **not `frame_intercepted`**, so a pane already claiming its own frame's mouse
            //   events keeps them, matching the ctrl-left and plain-left branches above;
            // - **not tracking the mouse**, which is the rule the option is documented under - and
            //   it is deliberately not conditioned on the pane being active, because a TUI in a
            //   background pane still owns its own right-click.
            //
            // Not gated on being the active pane either: right-clicking an unfocused pane should
            // open a menu there, and the executor focuses it first so the items act on it.
            if ctx.context_menu_enabled && event.event_type == MouseEventType::Press {
                if let Some(details) = ctx.clicked_pane.as_ref().filter(|details| {
                    details.pane_id == pane_id
                        && !details.frame_intercepted
                        && (!details.terminal_emulator_wants_mouse || details.on_frame)
                }) {
                    return Ok(MouseAction::OpenContextMenu {
                        pane_id,
                        position: event.position,
                        on_frame: details.on_frame,
                    });
                }
            }
            let is_active_pane = Some(pane_id) == ctx.active_pane_id;
            if is_active_pane {
                return Ok(MouseAction::SendToTerminal {
                    pane_id,
                    event: *event,
                });
            }
            return Ok(MouseAction::NoAction);
        }

        if event.middle {
            let Some(pane_id) = ctx.pane_id_at_position else {
                return Ok(MouseAction::NoAction);
            };
            let is_active_pane = Some(pane_id) == ctx.active_pane_id;
            if is_active_pane {
                return Ok(MouseAction::SendToTerminal {
                    pane_id,
                    event: *event,
                });
            }
            return Ok(MouseAction::NoAction);
        }

        let is_left_motion_or_release = event.left
            && (event.event_type == MouseEventType::Motion
                || event.event_type == MouseEventType::Release);
        if is_left_motion_or_release {
            let Some(details) = &ctx.clicked_pane else {
                return Ok(MouseAction::NoAction);
            };
            let is_active_pane = Some(details.pane_id) == ctx.active_pane_id;
            if is_active_pane && details.terminal_wants_mouse {
                return Ok(MouseAction::SendToTerminal {
                    pane_id: details.pane_id,
                    event: *event,
                });
            }
            return Ok(MouseAction::NoAction);
        }

        let is_buttonless_motion = event.event_type == MouseEventType::Motion
            && !event.left
            && !event.right
            && !event.middle;
        if is_buttonless_motion {
            let Some(pane_id) = ctx.pane_id_at_position else {
                return Ok(MouseAction::UpdateHover {
                    pane_id: None,
                    position: None,
                });
            };
            let is_active_pane = Some(pane_id) == ctx.active_pane_id;
            if is_active_pane {
                return Ok(MouseAction::SendToTerminal {
                    pane_id,
                    event: *event,
                });
            }
            if ctx.focus_follows_mouse {
                return Ok(MouseAction::FocusOnHover {
                    pane_id,
                    position: event.position,
                });
            }
            return Ok(MouseAction::UpdateHover {
                pane_id: Some(pane_id),
                position: Some(event.position),
            });
        }

        Ok(MouseAction::NoAction)
    }

    fn unselectable_pane_at_position<'a>(
        tab: &'a mut Tab,
        point: &Position,
    ) -> Option<&'a mut Box<dyn Pane>> {
        // the repetition in this function is to appease the borrow checker, I don't like it either
        let floating_panes_are_visible = tab.floating_panes.panes_are_visible();
        if floating_panes_are_visible {
            if let Ok(Some(clicked_pane_id)) = tab.floating_panes.get_pane_id_at(point, true) {
                if let Some(pane) = tab.floating_panes.get_pane_mut(clicked_pane_id) {
                    if !pane.selectable() {
                        return Some(pane);
                    }
                }
            } else if let Ok(Some(clicked_pane_id)) = tab.get_pane_id_at(point, false) {
                if let Some(pane) = tab.tiled_panes.get_pane_mut(clicked_pane_id) {
                    if !pane.selectable() {
                        return Some(pane);
                    }
                }
            }
        } else if let Ok(Some(clicked_pane_id)) = tab.get_pane_id_at(point, false) {
            if let Some(pane) = tab.tiled_panes.get_pane_mut(clicked_pane_id) {
                if !pane.selectable() {
                    return Some(pane);
                }
            }
        }
        None
    }

    fn focus_pane_at(tab: &mut Tab, point: &Position, client_id: ClientId) -> Result<()> {
        let err_context =
            || format!("failed to focus pane at position {point:?} for client {client_id}");

        // an overlay is painted over everything and takes the mouse wherever it is painted, with
        // no layer-visibility precondition - which is the whole reason it is not a floating pane
        if let Some(clicked_pane) = tab.overlay_panes.visible_pane_id_at(point) {
            // `may_be_focused` is false while a modal that is not this pane is open: the click is
            // still the overlay's, but the keyboard stays with the modal, so nothing about this
            // pane became active
            if tab
                .overlay_panes
                .get(&clicked_pane)
                .is_some_and(|pane| pane.selectable())
                && tab.overlay_panes.may_be_focused(&clicked_pane)
            {
                tab.overlay_panes.focus_pane(clicked_pane, client_id);
                tab.set_pane_active_at(clicked_pane);
            }
            // swallowed even when the overlay is not selectable: the click landed on it, not on
            // whatever it is covering
            return Ok(());
        }
        // below the overlays, then. A modal owns the keyboard until it closes, so a click on
        // whatever it is covering does nothing at all rather than moving a focus the user can
        // neither see nor act on. A dismissible overlay does give way - once something underneath
        // has actually taken the focus, which is what each `release_overlay_focus_to_lower_layer`
        // below marks. That is how a dismiss-on-defocus popup learns to close.
        if !tab.lower_layer_may_take_focus(client_id) {
            return Ok(());
        }
        if tab.floating_panes.panes_are_visible() {
            if let Some(clicked_pane) = tab
                .floating_panes
                .get_pane_id_at(point, true)
                .with_context(err_context)?
            {
                let clicked_pane = tab.stack_list_member_at_point(clicked_pane, point);
                if !tab
                    .get_pane_with_id(clicked_pane)
                    .is_some_and(|pane| pane.selectable())
                {
                    return Ok(());
                }
                if !tab.focus_hidden_stack_list_member(clicked_pane, client_id) {
                    tab.floating_panes.focus_pane(clicked_pane, client_id);
                }
                tab.set_pane_active_at(clicked_pane);
                tab.release_overlay_focus_to_lower_layer(client_id);
                return Ok(());
            }
        }
        if tab.floating_panes.has_pinned_panes() {
            let search_selectable = false;
            if let Some(pane_id) = tab
                .floating_panes
                .get_pinned_pane_id_at(point, search_selectable)
                .with_context(err_context)?
            {
                let pane_id = tab.stack_list_member_at_point(pane_id, point);
                if !tab.focus_hidden_stack_list_member(pane_id, client_id) {
                    tab.floating_panes.focus_pane(pane_id, client_id);
                }
                tab.set_pane_active_at(pane_id);
                tab.show_floating_panes();
                tab.release_overlay_focus_to_lower_layer(client_id);
                return Ok(());
            }
        }
        if let Some(clicked_pane) = tab.get_pane_id_at(point, true).with_context(err_context)? {
            if !tab.focus_hidden_stack_list_member(clicked_pane, client_id) {
                tab.tiled_panes.focus_pane(clicked_pane, client_id);
            }
            tab.set_pane_active_at(clicked_pane);
            if tab.floating_panes.panes_are_visible() {
                tab.hide_floating_panes();
                tab.set_force_render();
            }
            tab.release_overlay_focus_to_lower_layer(client_id);
        }
        Ok(())
    }

    pub fn handle_scrollwheel_up(
        tab: &mut Tab,
        point: &Position,
        lines: usize,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || {
            format!("failed to handle scrollwheel up at position {point:?} for client {client_id}")
        };

        if let Some(pane) = Self::get_pane_at(tab, point, false).with_context(err_context)? {
            // the pane under the cursor has already been resolved, overlays and all - write to it
            // by id rather than resolving the position a second time
            let pane_id = pane.pid();
            let relative_position = pane.relative_position(point);
            // Plugin wheel events carry only a count. Supply their actual position first,
            // including when the terminal has not sent any preceding pointer motion.
            if matches!(pane_id, PaneId::Plugin(_)) {
                let _ = pane.mouse_event(
                    &MouseEvent::new_buttonless_motion(relative_position),
                    client_id,
                );
            }
            if let Some(mouse_event) = pane.mouse_scroll_up(&relative_position) {
                Self::write_to_pane(tab, mouse_event.into_bytes(), pane_id, client_id)
                    .with_context(err_context)?;
            } else if pane.is_alternate_mode_active() {
                // separate writes so each sequence gets adjusted for cursor keys mode
                for _ in 0..lines {
                    tab.write_to_pane_id(
                        &None,
                        "\u{1b}[A".as_bytes().to_owned(),
                        false,
                        pane_id,
                        Some(client_id),
                        None,
                    )
                    .with_context(err_context)?;
                }
            } else {
                pane.scroll_up(lines, client_id);
            }
        }
        Ok(MouseEffect::default())
    }

    pub fn handle_scrollwheel_down(
        tab: &mut Tab,
        point: &Position,
        lines: usize,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || {
            format!(
                "failed to handle scrollwheel down at position {point:?} for client {client_id}"
            )
        };

        if let Some(pane) = Self::get_pane_at(tab, point, false).with_context(err_context)? {
            let pane_id = pane.pid();
            let relative_position = pane.relative_position(point);
            // Plugin wheel events carry only a count. Supply their actual position first,
            // including when the terminal has not sent any preceding pointer motion.
            if matches!(pane_id, PaneId::Plugin(_)) {
                let _ = pane.mouse_event(
                    &MouseEvent::new_buttonless_motion(relative_position),
                    client_id,
                );
            }
            if let Some(mouse_event) = pane.mouse_scroll_down(&relative_position) {
                Self::write_to_pane(tab, mouse_event.into_bytes(), pane_id, client_id)
                    .with_context(err_context)?;
            } else if pane.is_alternate_mode_active() {
                // separate writes so each sequence gets adjusted for cursor keys mode
                for _ in 0..lines {
                    tab.write_to_pane_id(
                        &None,
                        "\u{1b}[B".as_bytes().to_owned(),
                        false,
                        pane_id,
                        Some(client_id),
                        None,
                    )
                    .with_context(err_context)?;
                }
            } else {
                pane.scroll_down(lines, client_id);
                if !pane.is_scrolled() {
                    if let PaneId::Terminal(pid) = pane.pid() {
                        tab.process_pending_vte_events(pid)
                            .with_context(err_context)?;
                    }
                }
            }
        }
        Ok(MouseEffect::default())
    }

    /// Alt+wheel: a pane that tracks the mouse gets the (alt-modified) wheel report, as it
    /// would a plain wheel; otherwise the pane jumps to the previous or next shell prompt.
    fn handle_prompt_jump(
        tab: &mut Tab,
        pane_id: PaneId,
        to_previous_prompt: bool,
        event: &MouseEvent,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context =
            || format!("failed to jump to prompt in pane {pane_id:?} for client {client_id}");

        let report_for_pane = tab.get_pane_with_id(pane_id).and_then(|pane| {
            let mut event_for_pane = *event;
            event_for_pane.position = pane.relative_position(&event.position);
            pane.mouse_event(&event_for_pane, client_id)
        });
        if let Some(report_for_pane) = report_for_pane {
            Self::write_to_pane(tab, report_for_pane.into_bytes(), pane_id, client_id)
                .with_context(err_context)?;
            return Ok(MouseEffect::default());
        }

        if let Some(pane) = tab.get_pane_with_id_mut(pane_id) {
            if to_previous_prompt {
                pane.scroll_to_previous_prompt(client_id);
            } else {
                pane.scroll_to_next_prompt(client_id);
            }
        }
        Ok(MouseEffect::state_changed())
    }

    pub fn handle_scrollwheel_horizontal(
        tab: &mut Tab,
        pane_id: PaneId,
        point: &Position,
        cols: usize,
        scroll_right: bool,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context = || {
            format!(
                "failed to handle horizontal scrollwheel at position {point:?} for client {client_id}"
            )
        };
        if !matches!(pane_id, PaneId::Plugin(_)) {
            return Ok(MouseEffect::default());
        }
        if let Some(pane) = Self::get_pane_at(tab, point, false).with_context(err_context)? {
            if scroll_right {
                pane.scroll_right(cols, client_id);
            } else {
                pane.scroll_left(cols, client_id);
            }
        }
        Ok(MouseEffect::default())
    }

    /// One notch of a ctrl+scroll resize, applied to whichever collection holds `pane_id`.
    ///
    /// The overlay branch comes first, the same order `Tab::resize` dispatches in and for the same
    /// reason: an overlay is in neither of the other two collections, so without a branch of its
    /// own a scroll over one fell through to the *tiled* one and handed
    /// `stacked_resize_pane_with_id` a pane id that collection does not hold - while the overlay's
    /// frame advertised the gesture all along (`OverlayPanes::render` draws it as floating, so the
    /// hint offers `Ctrl <MouseScroll>`).
    ///
    /// `direction: None` moves all four borders, and `geom_resized_within` halves a directionless
    /// `change_by`, so a notch is 2 columns and 1 row on every side - the floating layer's step.
    /// Over a half of a linked overlay panel the four borders are the *panel's*
    /// (`OverlayPanes::resize_panel`), whichever half the pointer is on.
    fn handle_resize_scroll(
        tab: &mut Tab,
        pane_id: PaneId,
        resize: Resize,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        let err_context =
            || format!("failed to handle resize scroll {resize:?} for pane {pane_id:?}");

        let strategy = ResizeStrategy {
            resize,
            direction: None,
            invert_on_boundaries: false,
        };

        if tab.overlay_panes.panes_contain(&pane_id) {
            // No swap-layout damage flag and no stack dissolve: an overlay is part of no swap
            // layout, and the collection re-renders and re-ptys itself.
            Self::resize_overlay_with_strategies(tab, pane_id, &[strategy], (5, 2))
                .with_context(err_context)?;
        } else if tab.floating_panes.panes_contain(&pane_id) {
            Self::resize_floating_pane_with_strategies(tab, pane_id, &[strategy], (5, 2))
                .with_context(err_context)?;
            tab.swap_layouts.set_is_floating_damaged();
        } else {
            let active_pane_id = tab
                .get_active_pane_id(client_id)
                .ok_or_else(|| anyhow!("Failed to find active pane"))?;

            tab.dissolve_stack_lists_for_classic_mutation();
            Self::resize_tiled_pane_with_stacked_resize(tab, active_pane_id, &strategy)
                .with_context(err_context)?;
            tab.tiled_panes.reapply_pane_frames();
            tab.swap_layouts.set_is_tiled_damaged();
        }

        tab.set_force_render();
        Ok(MouseEffect::state_changed())
    }

    /// Send bytes to a pane the caller has already resolved.
    ///
    /// Scroll forwarding used to hand the *position* to `Tab::write_to_terminal_at`, which resolved
    /// it a second time and knew nothing about overlays: the sequence was generated from the
    /// overlay under the cursor and written to the tiled pane beneath it. `get_pane_at` is the one
    /// resolver now, and this only delivers what it found.
    fn write_to_pane(
        tab: &mut Tab,
        bytes: Vec<u8>,
        pane_id: PaneId,
        client_id: ClientId,
    ) -> Result<()> {
        // the bool is "the UI needs updating", which a scroll sequence never triggers
        tab.write_mouse_to_pane_id(bytes, pane_id, Some(client_id))
            .map(|_| ())
    }

    pub(super) fn get_pane_at<'a>(
        tab: &'a mut Tab,
        point: &Position,
        search_selectable: bool,
    ) -> Result<Option<&'a mut Box<dyn Pane>>> {
        let err_context = || format!("failed to get pane at position {point:?}");

        if let Some(pane_id) = tab.overlay_panes.visible_pane_id_at(point) {
            let pane = tab.overlay_panes.get_mut(&pane_id);
            if !search_selectable || pane.as_ref().is_some_and(|pane| pane.selectable()) {
                return Ok(pane);
            }
            return Ok(None);
        }
        if tab.floating_panes.panes_are_visible() {
            if let Some(pane_id) = tab
                .floating_panes
                .get_pane_id_at(point, search_selectable)
                .with_context(err_context)?
            {
                let pane_id = tab.stack_list_member_at_point(pane_id, point);
                return Ok(tab
                    .get_pane_with_id_mut(pane_id)
                    .filter(|pane| !search_selectable || pane.selectable()));
            }
        } else {
            if tab.floating_panes.has_pinned_panes() {
                if let Some(pane_id) = tab
                    .floating_panes
                    .get_pinned_pane_id_at(point, search_selectable)
                    .with_context(err_context)?
                {
                    let pane_id = tab.stack_list_member_at_point(pane_id, point);
                    return Ok(tab
                        .get_pane_with_id_mut(pane_id)
                        .filter(|pane| !search_selectable || pane.selectable()));
                }
            }
        }
        if let Some(pane_id) = tab
            .get_pane_id_at(point, search_selectable)
            .with_context(err_context)?
        {
            Ok(tab.get_pane_with_id_mut(pane_id))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse_event_context(mouse_scroll_resize: bool) -> MouseEventContext {
        MouseEventContext {
            pane_id_at_position: Some(PaneId::Terminal(TerminalId::new(1))),
            active_pane_id: Some(PaneId::Terminal(TerminalId::new(1))),
            floating_visible: false,
            pane_being_resized: false,
            selecting_with_mouse: false,
            pane_being_moved: None,
            seam_being_dragged: false,
            clicked_pane: None,
            advanced_mouse_actions: true,
            pinned_selectable: None,
            pinned_unselectable: None,
            focus_follows_mouse: false,
            mouse_click_through: false,
            mouse_scroll_resize,
            context_menu_enabled: false,
            passthrough_pane_id: None,
        }
    }

    /// A frame press on a pane that is either not an overlay, or a *resizable* (non-modal) one.
    fn frame_click_on(pane_id: PaneId, is_overlay: bool) -> ClickedPaneDetails {
        ClickedPaneDetails {
            pane_id,
            on_frame: true,
            frame_intercepted: false,
            edge: Some(PaneEdge::TopLeft),
            is_floating: false,
            is_overlay,
            overlay_is_resizable: is_overlay,
            overlay_is_movable: true,
            on_seam: None,
            terminal_wants_mouse: false,
            terminal_emulator_wants_mouse: false,
        }
    }

    fn frame_click_on_modal(pane_id: PaneId) -> ClickedPaneDetails {
        ClickedPaneDetails {
            overlay_is_resizable: false,
            ..frame_click_on(pane_id, true)
        }
    }

    /// An overlay drags its *own* collection. `should_start_moving` asks whether the floating
    /// *layer* is up, not whether the clicked pane is on it - and `execute_move_floating_pane`
    /// then throws the pane id away and re-resolves the position through `FloatingPanes` alone.
    /// So an overlay routed through the floating branch drags whatever it is covering, which is
    /// why the two are separate actions rather than one action with a flag.
    #[test]
    fn a_press_on_an_overlay_frame_starts_an_overlay_move_and_not_a_floating_one() {
        let overlay = PaneId::Terminal(TerminalId::new(9));
        let mut context = mouse_event_context(false);
        context.floating_visible = true;
        context.pane_id_at_position = Some(overlay);
        context.clicked_pane = Some(frame_click_on(overlay, true));

        let event = MouseEvent::new_left_press_event(Position::new(1, 1));
        let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
        assert!(
            matches!(action, MouseAction::StartMovingOverlay { .. }),
            "got {action:?}"
        );
        assert!(
            !matches!(action, MouseAction::StartMovingFloatingPane { .. }),
            "got {action:?}"
        );

        // the same click on a pane that really is under the visible layer still moves it
        context.clicked_pane = Some(frame_click_on(PaneId::Terminal(TerminalId::new(1)), false));
        assert!(matches!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::StartMovingFloatingPane { .. }
        ));
    }

    /// An expanded overlay fills the screen, so `OverlayPanes::move_pane_with_mouse` refuses the
    /// drag: routing the press there anyway set nothing moving, and the click that followed was
    /// swallowed by a drag that had never started. It is an ordinary click on the pane instead -
    /// and in particular not a fall-through to the floating branch, which would drag whatever the
    /// overlay is covering.
    #[test]
    fn a_press_on_an_expanded_overlay_s_frame_is_a_click_and_not_a_move() {
        let overlay = PaneId::Terminal(TerminalId::new(9));
        let mut context = mouse_event_context(false);
        context.floating_visible = true;
        context.pane_id_at_position = Some(overlay);
        context.clicked_pane = Some(ClickedPaneDetails {
            overlay_is_movable: false,
            overlay_is_resizable: false,
            ..frame_click_on(overlay, true)
        });

        let event = MouseEvent::new_left_press_event(Position::new(1, 1));
        let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
        assert_eq!(
            action,
            MouseAction::FocusPane {
                pane_id: overlay,
                position: event.position,
            },
            "got {action:?}"
        );
    }

    /// An overlay is not on a layer, so nothing has to be raised for its frame to be draggable.
    /// The floating branch would refuse this press outright.
    #[test]
    fn an_overlay_frame_drags_with_the_floating_layer_down() {
        let overlay = PaneId::Terminal(TerminalId::new(9));
        let mut context = mouse_event_context(false);
        context.floating_visible = false;
        context.pane_id_at_position = Some(overlay);
        context.clicked_pane = Some(frame_click_on(overlay, true));

        let event = MouseEvent::new_left_press_event(Position::new(1, 1));
        assert!(matches!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::StartMovingOverlay { .. }
        ));
    }

    /// The seam between two linked overlays is on both their frames, so the seam check has to
    /// come before the move: a press on the divider drags the divider, not the panel.
    #[test]
    fn a_press_on_a_linked_overlay_seam_starts_a_seam_drag_and_not_a_move() {
        let (left, right) = (
            PaneId::Plugin(PluginId::new(8)),
            PaneId::Plugin(PluginId::new(9)),
        );
        let mut context = mouse_event_context(false);
        context.pane_id_at_position = Some(right);
        let mut details = frame_click_on(right, true);
        details.on_seam = Some((left, right));
        context.clicked_pane = Some(details);

        let event = MouseEvent::new_left_press_event(Position::new(1, 1));
        let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
        assert!(
            matches!(
                action,
                MouseAction::StartOverlaySeamDrag { left: l, right: r, .. } if l == left && r == right
            ),
            "got {action:?}"
        );

        // the same press on a frame cell that is not the seam still picks the panel up
        context.clicked_pane = Some(frame_click_on(right, true));
        assert!(matches!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::StartMovingOverlay { .. }
        ));

        // and a ctrl-press on the seam is a seam drag too: the seam is the one frame edge that
        // is not the panel's, and resizing one half into the other is not a resize anybody wants
        let mut details = frame_click_on(right, true);
        details.on_seam = Some((left, right));
        context.clicked_pane = Some(details);
        let ctrl_press = MouseEvent {
            ctrl: true,
            ..MouseEvent::new_left_press_event(Position::new(1, 1))
        };
        assert!(matches!(
            MouseHandler::determine_mouse_action(&ctrl_press, &context).unwrap(),
            MouseAction::StartOverlaySeamDrag { .. }
        ));
        // while a ctrl-press on any other frame cell of the half is the resize it always was
        context.clicked_pane = Some(frame_click_on(right, true));
        assert!(matches!(
            MouseHandler::determine_mouse_action(&ctrl_press, &context).unwrap(),
            MouseAction::StartResize { .. }
        ));
    }

    /// An in-flight seam drag routes motion and release to the seam, ahead of any move.
    #[test]
    fn an_in_flight_seam_drag_routes_to_the_seam() {
        let position = Position::new(3, 3);
        let mut context = mouse_event_context(false);
        context.seam_being_dragged = true;
        assert!(matches!(
            MouseHandler::determine_mouse_action(
                &MouseEvent::new_left_motion_event(position),
                &context
            )
            .unwrap(),
            MouseAction::DragOverlaySeam { .. }
        ));
        assert!(matches!(
            MouseHandler::determine_mouse_action(
                &MouseEvent::new_left_release_event(position),
                &context
            )
            .unwrap(),
            MouseAction::StopOverlaySeamDrag { .. }
        ));
    }

    /// The drag is the frame's, not the pane's: a press in an overlay's body is still a press in
    /// a live terminal or plugin, and must not pick the popup up.
    #[test]
    fn a_press_inside_an_overlay_does_not_start_a_move() {
        let overlay = PaneId::Terminal(TerminalId::new(9));
        let mut context = mouse_event_context(false);
        context.floating_visible = true;
        context.pane_id_at_position = Some(overlay);
        let mut details = frame_click_on(overlay, true);
        details.on_frame = false;
        details.edge = None;
        context.clicked_pane = Some(details);

        let event = MouseEvent::new_left_press_event(Position::new(1, 1));
        let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
        assert!(
            !matches!(action, MouseAction::StartMovingOverlay { .. }),
            "got {action:?}"
        );
    }

    /// A plugin overlay's frame buttons keep working: the interception is checked before the drag,
    /// as it is on the floating branch.
    #[test]
    fn an_intercepted_overlay_frame_press_is_not_a_move() {
        let overlay = PaneId::Plugin(PluginId::new(9));
        let mut context = mouse_event_context(false);
        context.pane_id_at_position = Some(overlay);
        let mut details = frame_click_on(overlay, true);
        details.frame_intercepted = true;
        context.clicked_pane = Some(details);

        let event = MouseEvent::new_left_press_event(Position::new(1, 1));
        assert!(matches!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::FrameIntercepted { .. }
        ));
    }

    /// An in-flight drag routes motion and release to the collection that is holding it. Reading
    /// the target off a single enum rather than two booleans is what makes this unambiguous.
    #[test]
    fn an_in_flight_drag_routes_to_the_collection_holding_it() {
        let position = Position::new(3, 3);
        let motion = MouseEvent::new_left_motion_event(position);
        let release = MouseEvent::new_left_release_event(position);

        let mut context = mouse_event_context(false);
        context.pane_being_moved = Some(MouseDragTarget::Overlay);
        assert!(matches!(
            MouseHandler::determine_mouse_action(&motion, &context).unwrap(),
            MouseAction::ContinueMovingOverlay { .. }
        ));
        assert!(matches!(
            MouseHandler::determine_mouse_action(&release, &context).unwrap(),
            MouseAction::StopMovingOverlay { .. }
        ));

        context.pane_being_moved = Some(MouseDragTarget::Floating);
        assert!(matches!(
            MouseHandler::determine_mouse_action(&motion, &context).unwrap(),
            MouseAction::ContinueMovingFloatingPane { .. }
        ));
        assert!(matches!(
            MouseHandler::determine_mouse_action(&release, &context).unwrap(),
            MouseAction::StopMovingFloatingPane { .. }
        ));
    }

    /// A *plain* frame press on an overlay is a move, never a resize - the floating-pane split,
    /// which is what "resizable like a floating pane" means. With the layer down this press used
    /// to fall to `StartResize` for an overlay, and `start_pane_resize_with_mouse` then looked in
    /// the floating and tiled collections only, so the overlay id missed both and errored the
    /// whole mouse event.
    #[test]
    fn a_plain_press_on_an_overlay_frame_does_not_start_a_resize() {
        let overlay = PaneId::Terminal(TerminalId::new(9));
        let mut context = mouse_event_context(false);
        context.pane_id_at_position = Some(overlay);
        context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
        context.clicked_pane = Some(frame_click_on(overlay, true));

        let action = MouseHandler::determine_mouse_action(
            &MouseEvent::new_left_press_event(Position::new(1, 1)),
            &context,
        )
        .unwrap();
        assert!(
            !matches!(action, MouseAction::StartResize { .. }),
            "got {action:?}"
        );
    }

    /// The ctrl-drag half: a non-modal overlay resizes, and it resizes through its own collection
    /// rather than being handed to the floating or tiled one, neither of which holds it.
    #[test]
    fn a_ctrl_press_on_an_overlay_frame_starts_an_overlay_resize() {
        let overlay = PaneId::Terminal(TerminalId::new(9));
        let mut context = mouse_event_context(false);
        context.pane_id_at_position = Some(overlay);
        context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
        context.clicked_pane = Some(frame_click_on(overlay, true));

        let mut event = MouseEvent::new_left_press_event(Position::new(1, 1));
        event.ctrl = true;
        let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
        assert!(
            matches!(
                action,
                MouseAction::StartResize {
                    target: ResizeDragTarget::Overlay,
                    ..
                }
            ),
            "got {action:?}"
        );
    }

    /// A modal keeps the size its plugin asked for, so the ctrl-drag is nothing at all - and in
    /// particular not a `StartResize` that `start_pane_resize_with_mouse` would then fail to
    /// resolve.
    #[test]
    fn a_ctrl_press_on_a_modal_frame_does_not_start_a_resize() {
        let modal = PaneId::Terminal(TerminalId::new(9));
        let mut context = mouse_event_context(false);
        context.pane_id_at_position = Some(modal);
        context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
        context.clicked_pane = Some(frame_click_on_modal(modal));

        let mut event = MouseEvent::new_left_press_event(Position::new(1, 1));
        event.ctrl = true;
        let action = MouseHandler::determine_mouse_action(&event, &context).unwrap();
        assert!(matches!(action, MouseAction::NoAction), "got {action:?}");
    }

    /// A modal is still draggable - only its *size* is the plugin's.
    #[test]
    fn a_plain_press_on_a_modal_frame_still_starts_a_move() {
        let modal = PaneId::Terminal(TerminalId::new(9));
        let mut context = mouse_event_context(false);
        context.pane_id_at_position = Some(modal);
        context.clicked_pane = Some(frame_click_on_modal(modal));

        let action = MouseHandler::determine_mouse_action(
            &MouseEvent::new_left_press_event(Position::new(1, 1)),
            &context,
        )
        .unwrap();
        assert!(
            matches!(action, MouseAction::StartMovingOverlay { .. }),
            "got {action:?}"
        );
    }

    /// The scroll half of the ctrl-drag pair above: the overlay under the pointer is the pane that
    /// resizes. Without this the action still carried the overlay's id, but
    /// `handle_resize_scroll` knew only the floating and tiled collections and fell through to
    /// resizing whatever the client was focused on.
    #[test]
    fn a_ctrl_scroll_over_a_resizable_overlay_asks_for_a_resize() {
        let overlay = PaneId::Terminal(TerminalId::new(9));
        let mut context = mouse_event_context(true);
        context.pane_id_at_position = Some(overlay);
        context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
        context.clicked_pane = Some(ClickedPaneDetails {
            is_overlay: true,
            overlay_is_resizable: true,
            ..content_click_on(overlay)
        });

        let position = Position::new(1, 1);
        assert_eq!(
            MouseHandler::determine_mouse_action(
                &MouseEvent::new_ctrl_scroll_up_event(position),
                &context
            )
            .unwrap(),
            MouseAction::ResizeScrollUp { pane_id: overlay }
        );
        assert_eq!(
            MouseHandler::determine_mouse_action(
                &MouseEvent::new_ctrl_scroll_down_event(position),
                &context
            )
            .unwrap(),
            MouseAction::ResizeScrollDown { pane_id: overlay }
        );
    }

    /// A modal's size is the plugin's, so the scroll is nothing at all - and in particular not a
    /// plain scroll and not a resize of the pane behind it, which is where a fall-through would
    /// land it.
    #[test]
    fn a_ctrl_scroll_over_a_modal_does_nothing() {
        let modal = PaneId::Terminal(TerminalId::new(9));
        let mut context = mouse_event_context(true);
        context.pane_id_at_position = Some(modal);
        context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
        context.clicked_pane = Some(ClickedPaneDetails {
            is_overlay: true,
            overlay_is_resizable: false,
            ..content_click_on(modal)
        });

        let position = Position::new(1, 1);
        for event in [
            MouseEvent::new_ctrl_scroll_up_event(position),
            MouseEvent::new_ctrl_scroll_down_event(position),
        ] {
            assert_eq!(
                MouseHandler::determine_mouse_action(&event, &context).unwrap(),
                MouseAction::NoAction
            );
        }
    }

    #[test]
    fn disabled_ctrl_scroll_does_not_fall_through_to_regular_scrolling() {
        let context = mouse_event_context(false);
        let position = Position::new(1, 1);
        let events = [
            MouseEvent::new_ctrl_scroll_up_event(position),
            MouseEvent::new_ctrl_scroll_down_event(position),
        ];

        for event in events {
            assert_eq!(
                MouseHandler::determine_mouse_action(&event, &context).unwrap(),
                MouseAction::NoAction
            );
        }
    }

    /// A content click - not on the frame, nothing intercepted, no mouse tracking. The shape every
    /// context-menu case below varies one field of.
    fn content_click_on(pane_id: PaneId) -> ClickedPaneDetails {
        ClickedPaneDetails {
            pane_id,
            on_frame: false,
            frame_intercepted: false,
            edge: None,
            is_floating: false,
            is_overlay: false,
            overlay_is_resizable: false,
            overlay_is_movable: true,
            on_seam: None,
            terminal_wants_mouse: false,
            terminal_emulator_wants_mouse: false,
        }
    }

    fn right_press_context(pane_id: PaneId, enabled: bool) -> MouseEventContext {
        let mut context = mouse_event_context(false);
        context.context_menu_enabled = enabled;
        context.pane_id_at_position = Some(pane_id);
        context.clicked_pane = Some(content_click_on(pane_id));
        context
    }

    #[test]
    fn a_right_press_opens_the_context_menu_when_it_is_enabled() {
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let context = right_press_context(pane_id, true);
        let event = MouseEvent::new_right_press_event(Position::new(4, 7));
        assert_eq!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::OpenContextMenu {
                pane_id,
                position: Position::new(4, 7),
                on_frame: false,
            },
            "and the position is the absolute click, which is what anchors the menu"
        );
    }

    /// The regression guard for every existing right-click test: with the option off nothing at
    /// all about right-click changes.
    #[test]
    fn a_right_press_with_the_option_off_still_reaches_the_terminal() {
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let context = right_press_context(pane_id, false);
        let event = MouseEvent::new_right_press_event(Position::new(4, 7));
        assert!(matches!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::SendToTerminal { .. }
        ));
    }

    #[test]
    fn a_right_press_in_a_pane_tracking_the_mouse_belongs_to_the_program() {
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let mut context = right_press_context(pane_id, true);
        context.clicked_pane = Some(ClickedPaneDetails {
            terminal_emulator_wants_mouse: true,
            ..content_click_on(pane_id)
        });
        let event = MouseEvent::new_right_press_event(Position::new(4, 7));
        assert!(
            matches!(
                MouseHandler::determine_mouse_action(&event, &context).unwrap(),
                MouseAction::SendToTerminal { .. }
            ),
            "an editor that asked for right-click keeps it"
        );
    }

    /// A background TUI still owns its own right-click, so this must not be conditioned on the
    /// pane being active the way `terminal_wants_mouse` is.
    #[test]
    fn mouse_tracking_wins_even_in_a_pane_that_is_not_focused() {
        let clicked = PaneId::Terminal(TerminalId::new(2));
        let mut context = right_press_context(clicked, true);
        context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
        context.clicked_pane = Some(ClickedPaneDetails {
            terminal_emulator_wants_mouse: true,
            ..content_click_on(clicked)
        });
        let event = MouseEvent::new_right_press_event(Position::new(4, 7));
        assert_eq!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::NoAction,
            "not the menu, and not another pane's terminal either"
        );
    }

    #[test]
    fn mouse_tracking_does_not_block_a_frame_menu() {
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let mut context = right_press_context(pane_id, true);
        context.clicked_pane = Some(ClickedPaneDetails {
            on_frame: true,
            terminal_emulator_wants_mouse: true,
            ..content_click_on(pane_id)
        });
        let event = MouseEvent::new_right_press_event(Position::new(0, 3));
        assert!(matches!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::OpenContextMenu { on_frame: true, .. }
        ));
    }

    #[test]
    fn a_right_press_on_an_unfocused_pane_still_opens_a_menu() {
        let clicked = PaneId::Terminal(TerminalId::new(2));
        let mut context = right_press_context(clicked, true);
        context.active_pane_id = Some(PaneId::Terminal(TerminalId::new(1)));
        assert!(matches!(
            MouseHandler::determine_mouse_action(
                &MouseEvent::new_right_press_event(Position::new(4, 7)),
                &context
            )
            .unwrap(),
            MouseAction::OpenContextMenu { .. }
        ));
    }

    /// The release that follows the opening press must not open a second menu.
    #[test]
    fn only_the_press_opens_a_menu_not_the_release() {
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let context = right_press_context(pane_id, true);
        let release = MouseEvent::new_right_release_event(Position::new(4, 7));
        assert!(!matches!(
            MouseHandler::determine_mouse_action(&release, &context).unwrap(),
            MouseAction::OpenContextMenu { .. }
        ));
    }

    #[test]
    fn a_right_press_on_a_frame_opens_a_menu_and_says_it_was_the_frame() {
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let mut context = right_press_context(pane_id, true);
        context.clicked_pane = Some(ClickedPaneDetails {
            on_frame: true,
            ..content_click_on(pane_id)
        });
        assert_eq!(
            MouseHandler::determine_mouse_action(
                &MouseEvent::new_right_press_event(Position::new(0, 3)),
                &context
            )
            .unwrap(),
            MouseAction::OpenContextMenu {
                pane_id,
                position: Position::new(0, 3),
                on_frame: true,
            }
        );
    }

    #[test]
    fn a_frame_that_claims_its_own_mouse_events_keeps_the_right_click() {
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let mut context = right_press_context(pane_id, true);
        context.clicked_pane = Some(ClickedPaneDetails {
            on_frame: true,
            frame_intercepted: true,
            ..content_click_on(pane_id)
        });
        assert!(!matches!(
            MouseHandler::determine_mouse_action(
                &MouseEvent::new_right_press_event(Position::new(0, 3)),
                &context
            )
            .unwrap(),
            MouseAction::OpenContextMenu { .. }
        ));
    }

    /// Alt+right is `Ungroup`, and was before menus existed. The menu branch sits after the alt
    /// branch precisely so this keeps working.
    #[test]
    fn alt_right_still_ungroups_rather_than_opening_a_menu() {
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let context = right_press_context(pane_id, true);
        let mut event = MouseEvent::new_right_press_event(Position::new(4, 7));
        event.alt = true;
        assert_eq!(
            MouseHandler::determine_mouse_action(&event, &context).unwrap(),
            MouseAction::Ungroup
        );
    }
}
