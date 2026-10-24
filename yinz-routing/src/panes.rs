//! Resolve pane destinations and construct PTY spawn instructions.
use tokio::sync::oneshot;
use yinz_bus::pty::{ClientTabIndexOrPaneId, PtyInstruction, ReplacedPane};
use yinz_bus::route::{ActionCompletionResult, NotificationEnd};
use yinz_bus::thread_bus::ThreadSenders;
use yinz_bus::{ClientId, PaneId};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{NewPanePlacement, UnblockCondition};
use yinz_types::geometry::Direction;
use yinz_types::input::command::{OpenFilePayload, RunCommandAction, TerminalAction};
use yinz_types::internal_ids::TabId;

pub(super) fn new_pane_routing(
    no_focus: bool,
    near_current_pane: bool,
    tab_id: Option<TabId>,
    pane_id: Option<PaneId>,
    client_id: ClientId,
) -> ClientTabIndexOrPaneId {
    if let Some(tab_id) = tab_id {
        if no_focus {
            ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id)
        } else {
            ClientTabIndexOrPaneId::TabIndex(tab_id)
        }
    } else if let Some(pane_id) = pane_id.filter(|_| no_focus || near_current_pane) {
        ClientTabIndexOrPaneId::PaneId(pane_id)
    } else if no_focus {
        ClientTabIndexOrPaneId::ClientIdNoFocus(client_id)
    } else {
        ClientTabIndexOrPaneId::ClientId(client_id)
    }
}

pub(super) fn new_pane(
    direction: Option<Direction>,
    pane_name: Option<String>,
    start_suppressed: bool,
    senders: &ThreadSenders,
    default_shell: &Option<TerminalAction>,
    client_id: ClientId,
    completion_tx: oneshot::Sender<ActionCompletionResult>,
) -> Result<()> {
    let err_context = || format!("failed to route action for client {client_id}");
    let shell = default_shell.clone();
    let new_pane_placement = match direction {
        Some(direction) => NewPanePlacement::Tiled {
            direction: Some(direction),
            borderless: None,
        },
        None => NewPanePlacement::NoPreference { borderless: None },
    };
    senders
        .send_to_pty(PtyInstruction::SpawnTerminal(
            shell,
            pane_name,
            new_pane_placement,
            start_suppressed,
            ClientTabIndexOrPaneId::ClientId(client_id),
            Some(NotificationEnd::new(completion_tx)),
            false, // set_blocking
        ))
        .with_context(err_context)?;
    Ok(())
}

pub(super) fn new_blocking_pane(
    placement: NewPanePlacement,
    pane_name: Option<String>,
    command: Option<RunCommandAction>,
    unblock_condition: Option<UnblockCondition>,
    near_current_pane: bool,
    no_focus: bool,
    tab_id: Option<TabId>,
    senders: &ThreadSenders,
    default_shell: &Option<TerminalAction>,
    client_id: ClientId,
    pane_id: Option<PaneId>,
    completion_tx: oneshot::Sender<ActionCompletionResult>,
) -> Result<()> {
    let err_context = || format!("failed to route action for client {client_id}");
    let command = command
        .map(|cmd| TerminalAction::RunCommand(cmd.into()))
        .or_else(|| default_shell.clone());
    let set_pane_blocking = true;

    let notification_end = if let Some(condition) = unblock_condition {
        Some(NotificationEnd::new_with_condition(
            completion_tx,
            condition,
        ))
    } else {
        Some(NotificationEnd::new(completion_tx))
    };

    // we prefer the pane id provided by the action explicitly over the one that originated
    // it (this might be a bit misleading with "near_current_pane", but it's still the
    // right behavior - in the latter case, if the originator does not wish for this
    // behavior, they should not provide pane
    // inside the placement, but rather have the current pane id be picked up instead)
    let explicit_pane_id = match placement {
        NewPanePlacement::Stacked {
            pane_id_to_stack_under,
            ..
        } => pane_id_to_stack_under,
        NewPanePlacement::InPlace {
            pane_id_to_replace, ..
        } => pane_id_to_replace,
        _ => None,
    };
    let pane_id = explicit_pane_id.or(pane_id);

    // An explicit target routes to its own tab, as `new_in_place_pane` does: routed to the
    // client instead, a target outside the client's tab cannot be found there.
    let client_tab_index_or_paneid = new_pane_routing(
        no_focus,
        near_current_pane || explicit_pane_id.is_some(),
        tab_id,
        pane_id,
        client_id,
    );
    senders
        .send_to_pty(PtyInstruction::SpawnTerminal(
            command,
            pane_name,
            placement,
            false,
            client_tab_index_or_paneid,
            notification_end,
            set_pane_blocking,
        ))
        .with_context(err_context)?;
    Ok(())
}

pub(super) fn edit_file(
    open_file_payload: OpenFilePayload,
    split_direction: Option<Direction>,
    should_float: bool,
    should_open_in_place: bool,
    close_replaced_pane: bool,
    start_suppressed: bool,
    placement: NewPanePlacement,
    near_current_pane: bool,
    no_focus: bool,
    tab_id: Option<TabId>,
    senders: &ThreadSenders,
    client_id: ClientId,
    pane_id: Option<PaneId>,
    completion_tx: oneshot::Sender<ActionCompletionResult>,
) -> Result<()> {
    let err_context = || format!("failed to route action for client {client_id}");
    let title = format!("Editing: {}", open_file_payload.path.display());
    let open_file = TerminalAction::OpenFile(open_file_payload);
    let client_tab_index_or_paneid =
        new_pane_routing(no_focus, near_current_pane, tab_id, pane_id, client_id);
    let pty_instr = if should_open_in_place {
        PtyInstruction::SpawnInPlaceTerminal(
            Some(open_file),
            Some(title),
            ReplacedPane::closed_if(close_replaced_pane),
            client_tab_index_or_paneid,
            Some(NotificationEnd::new(completion_tx)),
        )
    } else {
        PtyInstruction::SpawnTerminal(
            Some(open_file),
            Some(title),
            if should_float {
                placement
            } else {
                NewPanePlacement::Tiled {
                    direction: split_direction,
                    borderless: None,
                }
            },
            start_suppressed,
            client_tab_index_or_paneid,
            Some(NotificationEnd::new(completion_tx)),
            false, // set_blocking
        )
    };
    senders.send_to_pty(pty_instr).with_context(err_context)?;
    Ok(())
}

pub(super) fn new_floating_pane(
    run_command: Option<RunCommandAction>,
    name: Option<String>,
    placement: NewPanePlacement,
    near_current_pane: bool,
    no_focus: bool,
    tab_id: Option<TabId>,
    senders: &ThreadSenders,
    default_shell: &Option<TerminalAction>,
    client_id: ClientId,
    pane_id: Option<PaneId>,
    completion_tx: oneshot::Sender<ActionCompletionResult>,
) -> Result<()> {
    let err_context = || format!("failed to route action for client {client_id}");
    let run_cmd = run_command
        .map(|cmd| TerminalAction::RunCommand(cmd.into()))
        .or_else(|| default_shell.clone());
    let client_tab_index_or_paneid =
        new_pane_routing(no_focus, near_current_pane, tab_id, pane_id, client_id);
    senders
        .send_to_pty(PtyInstruction::SpawnTerminal(
            run_cmd,
            name,
            placement,
            false,
            client_tab_index_or_paneid,
            Some(NotificationEnd::new(completion_tx)),
            false, // set_blocking
        ))
        .with_context(err_context)?;
    Ok(())
}

pub(super) fn new_in_place_pane(
    run_command: Option<RunCommandAction>,
    name: Option<String>,
    near_current_pane: bool,
    no_focus: bool,
    pane_id_to_replace: Option<PaneId>,
    close_replaced_pane: bool,
    tab_id: Option<TabId>,
    senders: &ThreadSenders,
    default_shell: &Option<TerminalAction>,
    client_id: ClientId,
    pane_id: Option<PaneId>,
    completion_tx: oneshot::Sender<ActionCompletionResult>,
) -> Result<()> {
    let err_context = || format!("failed to route action for client {client_id}");
    let run_cmd = run_command
        .map(|cmd| TerminalAction::RunCommand(cmd.into()))
        .or_else(|| default_shell.clone());
    let client_tab_index_or_paneid = new_pane_routing(
        no_focus,
        near_current_pane || pane_id_to_replace.is_some(),
        tab_id,
        pane_id_to_replace.or(pane_id),
        client_id,
    );
    senders
        .send_to_pty(PtyInstruction::SpawnInPlaceTerminal(
            run_cmd,
            name,
            ReplacedPane::closed_if(close_replaced_pane),
            client_tab_index_or_paneid,
            Some(NotificationEnd::new(completion_tx)),
        ))
        .with_context(err_context)?;
    Ok(())
}

pub(super) fn new_stacked_pane(
    run_command: Option<RunCommandAction>,
    name: Option<String>,
    near_current_pane: bool,
    no_focus: bool,
    tab_id: Option<TabId>,
    senders: &ThreadSenders,
    default_shell: &Option<TerminalAction>,
    client_id: ClientId,
    pane_id: Option<PaneId>,
    completion_tx: oneshot::Sender<ActionCompletionResult>,
) -> Result<()> {
    let err_context = || format!("failed to route action for client {client_id}");
    let run_cmd = run_command
        .map(|cmd| TerminalAction::RunCommand(cmd.into()))
        .or_else(|| default_shell.clone());

    let client_tab_index_or_paneid =
        new_pane_routing(no_focus, near_current_pane, tab_id, pane_id, client_id);
    // Stacked under the pane it was routed to, if it was routed to one.
    let pane_placement = NewPanePlacement::Stacked {
        pane_id_to_stack_under: match client_tab_index_or_paneid {
            ClientTabIndexOrPaneId::PaneId(pane_id) => Some(pane_id),
            _ => None,
        },
        borderless: None,
    };
    senders
        .send_to_pty(PtyInstruction::SpawnTerminal(
            run_cmd,
            name,
            pane_placement,
            false,
            client_tab_index_or_paneid,
            Some(NotificationEnd::new(completion_tx)),
            false, // set_blocking
        ))
        .with_context(err_context)?;
    Ok(())
}

pub(super) fn new_tiled_pane(
    direction: Option<Direction>,
    run_command: Option<RunCommandAction>,
    name: Option<String>,
    near_current_pane: bool,
    no_focus: bool,
    borderless: Option<bool>,
    tab_id: Option<TabId>,
    senders: &ThreadSenders,
    default_shell: &Option<TerminalAction>,
    client_id: ClientId,
    pane_id: Option<PaneId>,
    completion_tx: oneshot::Sender<ActionCompletionResult>,
) -> Result<()> {
    let err_context = || format!("failed to route action for client {client_id}");
    let run_cmd = run_command
        .map(|cmd| TerminalAction::RunCommand(cmd.into()))
        .or_else(|| default_shell.clone());
    let client_tab_index_or_paneid =
        new_pane_routing(no_focus, near_current_pane, tab_id, pane_id, client_id);
    senders
        .send_to_pty(PtyInstruction::SpawnTerminal(
            run_cmd,
            name,
            NewPanePlacement::Tiled {
                direction,
                borderless,
            },
            false,
            client_tab_index_or_paneid,
            Some(NotificationEnd::new(completion_tx)),
            false, // set_blocking
        ))
        .with_context(err_context)?;
    Ok(())
}
