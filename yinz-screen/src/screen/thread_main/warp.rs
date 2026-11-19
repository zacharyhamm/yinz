use super::*;
use std::time::Instant;
use yinz_bus::pty::AdoptedTabPanes;
use yinz_bus::ClientId;
use yinz_bus::PaneId;
use yinz_types::data::WarpPlacement;
use yinz_types::data::WarpedPane;
use yinz_types::data::WarpedTab;
use yinz_types::input::command::RunCommand;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TerminalId;

pub(super) fn warp_pane(
    screen: &mut Screen,
    supports_companions: bool,
    supports_held: bool,
    supports_tracking: bool,
    pane_id: Option<PaneId>,
    to_session: SessionName,
    placement: WarpPlacement,
    client_id: ClientId,
    response_channel: yinz_bus::warp::PaneWarpReply,
    deadline: Instant,
) -> Result<()> {
    screen.warp_pane(
        supports_companions,
        supports_held,
        supports_tracking,
        pane_id,
        to_session,
        placement,
        client_id,
        response_channel,
        deadline,
    )?;
    Ok(())
}

pub(super) fn adopted_pane_ready(
    screen: &mut Screen,
    hold: Option<yinz_bus::warp::PaneWarpHold>,
    placed: crossbeam::channel::Sender<Result<()>>,
    terminal_id: TerminalId,
    payload: Box<WarpedPane>,
    placement: WarpPlacement,
    client_id: ClientId,
    cmd: RunCommand,
    buffered: Vec<u8>,
    child_exit_status: Option<Option<i32>>,
) -> Result<()> {
    let mut finish = Some(yinz_bus::warp::AdoptedPaneFinish {
        terminal_id,
        recovery: crate::warp::recovery_command(&payload),
        held: payload.held.is_some(),
        hold,
        placed,
    });
    // The pty thread has already told the caller this session owns
    // the terminal, so a placement failure is handled here and
    // reported through `placed`.
    screen
        .place_adopted_pane(
            terminal_id,
            *payload,
            placement,
            client_id,
            cmd,
            buffered,
            child_exit_status,
            &mut finish,
        )
        .non_fatal();
    // Failed before its pane was asked for, so there is none.
    if let Some(finish) = finish {
        screen.finish_adopted_pane(finish);
    }
    screen.log_and_report_session_state().non_fatal();
    screen.render()?;
    Ok(())
}

pub(super) fn warp_tab(
    screen: &mut Screen,
    supports_companions: bool,
    supports_held: bool,
    supports_tracking: bool,
    tab_id: Option<TabId>,
    client_id: ClientId,
    response_channel: crossbeam::channel::Sender<Result<Box<WarpedTab>, String>>,
    deadline: Instant,
) -> Result<()> {
    screen.warp_tab(
        supports_companions,
        supports_held,
        supports_tracking,
        tab_id,
        client_id,
        response_channel,
        deadline,
    )?;
    Ok(())
}

pub(super) fn adopted_tab_ready(
    screen: &mut Screen,
    is_rollback: bool,
    response_channel: Option<crossbeam::channel::Sender<Result<()>>>,
    hold: Option<yinz_bus::warp::PaneWarpHold>,
    placement: Option<yinz_types::data::WarpPlacement>,
    payload: WarpedTab,
    adopted: AdoptedTabPanes,
    client_id: ClientId,
) -> Result<()> {
    let result = screen.place_adopted_tab(
        payload,
        adopted,
        client_id,
        is_rollback,
        placement,
        response_channel.clone(),
        hold,
    );
    if let Err(error) = result {
        if let Some(response_channel) = response_channel {
            let _ = response_channel.send(Err(error));
        } else {
            return Err(error);
        }
    }
    screen.log_and_report_session_state().non_fatal();
    screen.render()?;
    Ok(())
}
