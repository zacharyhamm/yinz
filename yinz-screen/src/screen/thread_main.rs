mod agents;
mod clients;
mod dispatch;
mod input;
mod layout;
mod nested;
mod notifications;
mod palette;
mod panes;
mod plugin_launch;
mod plugins;
mod tabs;
mod warp;

pub(crate) use super::agent_reports::apply_pane_agent_report;
pub(super) use super::agent_reports::{apply_pane_foreground_command, apply_queued_agent_report};
pub(super) use super::pending_events::{replay_pending_pane_event, PendingEvents};

use yinz_runtime::errors::prelude::*;

use super::{active_tab_and_connected_client_id, Screen, ScreenInstruction};
use crate::background_jobs::BackgroundJob;
use crate::notifications::denormalize_notification_response;
use crate::os_input_output::ResizeCache;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::tab::Tab;
use crate::thread_bus::Bus;
use crate::{ClientId, ServerInstruction};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use yinz_config::config::Config;
use yinz_config::screen_config::ScreenConfig;
use yinz_ipc::get_mode_info;
use yinz_ipc::ipc::ClientAttributes;
use yinz_ipc::nested_session;
use yinz_ipc::nested_session::NestedSessionMessage;
use yinz_types::diagnostics::ContextType;
use yinz_types::geometry::Direction;
use yinz_types::input::layout::Layout;
use yinz_types::style::HostTerminalThemeMode;
use {yinz_types::data::InputMode, yinz_types::data::PluginCapabilities};

/// Whether `raw_bytes` is text a tab name, a pane name or a search term can take.
///
/// A key that only has an escape encoding is not something anyone means to type into a name:
/// `clean_string_from_control_and_linebreak` strips the ESC and leaves the rest of the CSI
/// sequence behind as literal characters, so `Alt+t` in rename mode would append `[116;3u`. Under
/// the Kitty keyboard flags `report_modifier_keys` pushes, every printable key arrives as a CSI
/// sequence and is resolved back to a character before it gets here - but a host that ignores
/// `REPORT_ALTERNATE_KEYS` can still leave one unresolved, and that must not be typed either.
///
/// The bracketed-paste markers this used to name explicitly are covered by the same check: both
/// begin with ESC. Real pastes use `PasteFromClient` and update names/search directly.
pub(super) fn is_typable_text(raw_bytes: &[u8]) -> bool {
    !raw_bytes.starts_with(&[0x1b])
}

#[derive(Default)]
pub(super) struct EventLoopState {
    pending: PendingEvents,
    pub(super) terminal_placements: HashMap<crate::TerminalId, u64>,
    attach_traces_waiting_for_first_render: HashMap<ClientId, Instant>,
    keybind_intercepts: HashMap<ClientId, crate::plugins::PluginId>,
    escape_focus: HashMap<ClientId, (Option<PaneId>, Option<InputMode>)>,
    pending_escape_count: usize,
}

pub fn screen_thread_main(
    observer: Option<crate::RestoredAgentObserver>,
    bus: Bus<ScreenInstruction>,
    max_panes: Option<usize>,
    client_attributes: ClientAttributes,
    config: Config,
    debug: bool,
    config_file_path: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    default_layout: Box<Layout>,
) -> Result<()> {
    let mut screen_config = ScreenConfig::from_config(&config);
    screen_config.layout_dir = yinz_runtime::home::layout_dir_or_default(
        config.options.layout_dir.clone(),
        config_dir.clone(),
    );
    let explicit_theme_hue = screen_config.explicit_theme_hue;
    let default_mode = screen_config.default_mode;
    let mode_info = get_mode_info(
        default_mode.unwrap_or_default(),
        &client_attributes,
        PluginCapabilities {
            //  ¯\_(ツ)_/¯
            arrow_fonts: screen_config.simplified_ui,
            // Not known until a client's startup probe answers; see
            // `Screen::update_kitty_graphics_support`.
            kitty_graphics: false,
        },
        &config.keybinds,
        default_mode,
        Some(bus.senders.session.current()),
    );
    let mut screen = Screen::new(
        bus,
        &client_attributes,
        max_panes,
        mode_info,
        debug,
        default_layout,
        screen_config,
    );
    screen.core.restored_observer = observer;
    screen.core.config_file_path = config_file_path;
    screen.core.config_dir = config_dir;
    if let Some(data_dir) = data_dir {
        screen.core.last_project = yinz_runtime::last_project::LastProject::load(data_dir);
    }
    if screen.core.config_file_path.is_some() {
        screen.restart_layout_watcher();
    }
    // Resolve the session's hue before the first render: an explicit pin
    // wins; otherwise a dark/light pair defaults to dark until the host
    // terminal reports.
    if explicit_theme_hue.is_some() {
        screen
            .apply_configured_explicit_theme_hue(explicit_theme_hue)
            .non_fatal();
    } else {
        screen.resolve_default_theme_mode().non_fatal();
    }

    let mut state = EventLoopState::default();
    'instructions: loop {
        screen.advance_migration_drain();
        if !screen.migration_blocks_normal_work() {
            screen.refresh_agent_controls();
            if screen.agents.event_snapshot_dirty() {
                screen.refresh_agent_events();
                screen.refresh_agent_event_projects();
            }
            screen.retry_agent_event_projects(Instant::now());
            screen.flush_agent_events();
            if screen.core.agent_double_escape {
                // Observe every screen transition, including programmatic focus changes. Keeping the
                // previous focus/mode catches leaving and returning before the next key arrives.
                let focus: HashMap<_, _> = screen
                    .core
                    .connected_clients
                    .borrow()
                    .iter()
                    .map(|client| {
                        (
                            *client,
                            (
                                screen.get_active_pane_id(client),
                                screen.get_client_input_mode(*client),
                            ),
                        )
                    })
                    .collect();
                for tab in screen.core.tabs.values_mut() {
                    tab.agent_double_escape = screen.core.agent_double_escape;
                    tab.pending_agent_escapes.retain(|(client, _), _| {
                        focus.contains_key(client)
                            && focus.get(client) == state.escape_focus.get(client)
                    });
                }
                state.escape_focus = focus;
            }
            // Run before receiving, so even input handlers that `continue` repaint cancellations.
            let pending_now: usize = screen
                .core
                .tabs
                .values()
                .map(|tab| tab.pending_agent_escapes.len())
                .sum();
            if state.pending_escape_count != pending_now {
                state.pending_escape_count = pending_now;
                screen.render()?;
                screen.sync_animation_clock(Instant::now())?;
            }
            screen.expire_notification_requests(Instant::now())?;
            screen.expire_sync_output(Instant::now())?;
        }
        let deadline = if !screen.migration_blocks_normal_work() {
            [
                screen
                    .agents
                    .agent_control_deadline_at(std::time::Instant::now()),
                screen.notices.notification_deadline(),
                screen.sync_output_deadline(),
            ]
            .into_iter()
            .flatten()
            .min()
        } else {
            None
        };
        let replayed = screen.next_migration_event();
        let from_deferred = replayed.is_some();
        let (event, mut err_ctx) = if let Some(event) = replayed {
            (event, yinz_runtime::errors::get_current_ctx())
        } else if let Some(timeout) = deadline {
            match screen.core.bus.recv_timeout(timeout) {
                Ok(event) => event,
                Err(yinz_runtime::channels::RecvTimeoutError::Timeout) => continue,
                Err(error) => return Err(error).context("receive agent control event"),
            }
        } else {
            screen
                .core
                .bus
                .recv()
                .context("failed to receive event on channel")?
        };
        let deferred = if from_deferred {
            screen.defer_replayed_for_migration(event)
        } else {
            screen.defer_for_migration(event)
        };
        let Some(event) = deferred else {
            continue;
        };
        yinz_runtime::errors::add_call(&mut err_ctx, ContextType::Screen((&event).into()));
        if process_instruction(&mut screen, &mut state, event)?.is_break() {
            break 'instructions;
        }
    }
    Ok(())
}

pub(super) fn process_instruction(
    screen: &mut Screen,
    state: &mut EventLoopState,
    event: ScreenInstruction,
) -> Result<std::ops::ControlFlow<()>> {
    // These reports can precede AddClient or wait behind it. Admit them against the
    // listener's registry. ACK, seal and removal still address their retired owner.
    let recipient = match &event {
        ScreenInstruction::Reconfigure { client_id, .. } if *client_id != ClientId::NONE => {
            Some(*client_id)
        },
        ScreenInstruction::PaletteThemeSaved { client, .. }
        | ScreenInstruction::GoToTab(_, Some(client), _)
        | ScreenInstruction::GoToTabName(_, _, _, Some(client), _)
        | ScreenInstruction::GoToTabWithId(_, Some(client), _)
        | ScreenInstruction::FocusPaneWithId(_, _, _, client, _)
        | ScreenInstruction::ConfirmQuit(client, _)
        | ScreenInstruction::SwitchTabNext(client, _)
        | ScreenInstruction::SwitchTabPrev(client, _)
        | ScreenInstruction::ToggleTab(client, _)
        | ScreenInstruction::ChangeMode(_, _, client, _)
        | ScreenInstruction::MouseEvent(_, client, _)
        | ScreenInstruction::ResetAgentEscape(client)
        | ScreenInstruction::WriteCharacter(_, _, _, client, _)
        | ScreenInstruction::TextFromClient(_, client, _)
        | ScreenInstruction::PasteFromClient(_, client, _)
        | ScreenInstruction::AddRedPaneFrameColorOverride(Some(client), ..)
        | ScreenInstruction::ClearPaneFrameColorOverride(Some(client), _)
        | ScreenInstruction::AddClient(client, ..)
        | ScreenInstruction::AddWatcherClient(client, _)
        | ScreenInstruction::WatcherTerminalResize(client, _)
        | ScreenInstruction::RecomputeTabSize(client, _)
        | ScreenInstruction::RequestRepaint(client)
        | ScreenInstruction::TerminalPixelDimensions(client, _)
        | ScreenInstruction::TerminalForegroundColor(client, _)
        | ScreenInstruction::TerminalBackgroundColor(client, _)
        | ScreenInstruction::TerminalColorRegisters(client, _)
        | ScreenInstruction::HostTerminalThemeChanged(client, _)
        | ScreenInstruction::HostTerminalFocusChanged(client, _)
        | ScreenInstruction::NativeRenderRequest(client, _)
        | ScreenInstruction::ClearMouseHelpText(client)
        | ScreenInstruction::DesktopNotificationResponse(_, client)
        | ScreenInstruction::NestedSessionMessageFromHost {
            client_id: client, ..
        }
        | ScreenInstruction::SetKittyGraphicsSupport {
            client_id: client, ..
        }
        | ScreenInstruction::SetSixelSupport {
            client_id: client, ..
        }
        | ScreenInstruction::SetSynchronizedOutputSupport {
            client_id: client, ..
        }
        | ScreenInstruction::SetStructuredRenderSupport {
            client_id: client, ..
        } => Some(*client),
        _ => None,
    };
    if recipient.is_some_and(|owner| !screen.core.bus.senders.session.owns_attachment(owner)) {
        return Ok(std::ops::ControlFlow::Continue(()));
    }
    // Tab and pane senders share this scope through placement confirmation.
    let _resize_cache = ResizeCache::new(screen.core.bus.senders.clone());
    if screen.core.agent_double_escape {
        let reset_client = match &event {
            ScreenInstruction::MouseEvent(_, client, ..)
            | ScreenInstruction::HostTerminalFocusChanged(client, _)
            | ScreenInstruction::RemoveClient(client)
            | ScreenInstruction::ChangeMode(_, _, client, _) => Some(*client),
            ScreenInstruction::WriteCharacter(key, _, _, client, _)
                if key.as_ref().is_none_or(|key| {
                    key.bare_key != yinz_types::keys::BareKey::Esc || !key.has_no_modifiers()
                }) || state.keybind_intercepts.contains_key(client) =>
            {
                Some(*client)
            },
            _ => None,
        };
        if let Some(client_id) = reset_client {
            for tab in screen.core.tabs.values_mut() {
                tab.pending_agent_escapes
                    .retain(|(client, _), _| *client != client_id);
            }
        }
    }

    // The same conversion applies to exits replayed after layout application.
    let event = match event {
        ScreenInstruction::CloseSelf(plugin, client) => {
            screen.close_self_instruction(plugin, client)
        },
        ScreenInstruction::ClosePane(id, None, None, status) => {
            match screen.replacement_exit_command(id) {
                Some(command) => ScreenInstruction::HoldPane(id, status, command),
                None => ScreenInstruction::ClosePane(id, None, None, status),
            }
        },
        event => event,
    };
    'instruction: {
        // An instruction naming a headless plugin's pane is answered here: it has none.
        let Some(event) = screen.settle_headless_target(event) else {
            break 'instruction;
        };
        if dispatch::dispatch(screen, state, event)?.is_break() {
            return Ok(std::ops::ControlFlow::Break(()));
        }
    }
    // Whatever the instruction did, no tab it emptied outlives it: a tab with nothing
    // selectable is closed here - and the session with it, unless something asks it to go
    // on - rather than left to a frame that may never come (nobody attached) or was never
    // scheduled (the route removed a pane without asking for one).
    screen
        .settle_tabs()
        .context("failed to settle tabs after instruction")
        .non_fatal();
    screen.reconcile_web_focus_after_settlement();
    screen.revalidate_launches();
    screen.confirm_terminal_placements(&mut state.terminal_placements, &state.pending)?;
    if !screen.migration_blocks_normal_work() {
        screen.reconcile_project_contexts();
    }
    // Teardown can finish before its queued close notification reaches this thread.
    state
        .pending
        .waiting_for_pane
        .retain(|id, _| !screen.core.destroyed_panes.contains(id));
    // IDs can be reused after removal; discard stale focus before another tab is created.
    state
        .pending
        .tab_switches
        .retain(|(id, _)| screen.core.tabs.contains_key(id));
    Ok(std::ops::ControlFlow::Continue(()))
}

#[cfg(test)]
#[path = "../unit/typable_text_tests.rs"]
mod typable_text_tests;
