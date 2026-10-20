//! The screen thread snapshots state and installs the subscription in one instruction.
use serde_json::json;
use std::time::Duration;
use yinz_cli::args::Wait;
use yinz_ipc::ipc::{ClientToServerMsg, ServerToClientMsg};
use yinz_runtime::errors::prelude::*;
use yinz_types::{data::Event, internal_ids::SessionName, pane_id::PaneId};

pub fn wait(session: &SessionName, request: Wait) -> Result<serde_json::Value> {
    let (pane, timeout, message) = match &request {
        Wait::PaneWait {
            pane_id,
            contains: Some(_),
            timeout,
            ..
        } => (
            *pane_id,
            *timeout,
            ClientToServerMsg::SubscribeToPaneRenders {
                pane_ids: vec![*pane_id],
                scrollback: None,
                ansi: false,
            },
        ),
        Wait::PaneWait {
            pane_id, timeout, ..
        } => (
            *pane_id,
            *timeout,
            ClientToServerMsg::SubscribeToEvents {
                options: Default::default(),
                event_types: vec!["pane_update".into(), "command_pane_exited".into()],
            },
        ),
    };
    crate::store_client::exchange(session, Duration::from_secs(timeout), message, |message| {
        observe(&request, pane, message)
    })
}

fn observe(
    request: &Wait,
    target: PaneId,
    message: ServerToClientMsg,
) -> Option<Result<serde_json::Value>> {
    let closed = || match request {
        Wait::PaneWait { closed: true, .. } => Ok(json!({"pane_id":target,"closed":true})),
        _ => Err(anyhow!(
            "target pane disappeared before the requested condition"
        )),
    };
    match message {
        ServerToClientMsg::PaneRenderUpdate {
            pane_id, viewport, ..
        } if pane_id == target => {
            if let Wait::PaneWait {
                contains: Some(literal),
                ..
            } = request
                && viewport.join("\n").contains(literal)
            {
                return Some(Ok(json!({"pane_id":target,"viewport":viewport})));
            }
        },
        ServerToClientMsg::SubscribedPaneClosed { pane_id } if pane_id == target => {
            return Some(closed())
        },
        ServerToClientMsg::EventUpdate { event_json, .. } => {
            let event = match serde_json::from_str::<Event>(&event_json) {
                Ok(event) => event,
                Err(error) => return Some(Err(anyhow!("invalid wait event: {error}"))),
            };
            match event {
                Event::PaneUpdate(manifest) => {
                    let Some(pane) = manifest
                        .panes
                        .values()
                        .flatten()
                        .find(|pane| pane.id == target)
                    else {
                        return Some(closed());
                    };
                    let satisfied = match request {
                        Wait::PaneWait { exited: true, .. } => pane.exited,
                        _ => false,
                    };
                    if satisfied {
                        return Some(serde_json::to_value(pane).context("encode observed pane"));
                    }
                },
                Event::CommandPaneExited(id, status, _)
                    if target == PaneId::Terminal(id)
                        && matches!(request, Wait::PaneWait { exited: true, .. }) =>
                {
                    return Some(Ok(
                        json!({"pane_id":target,"exited":true,"exit_status":status}),
                    ))
                },
                _ => {},
            }
        },
        _ => {},
    }
    None
}

#[cfg(test)]
#[path = "unit/wait_client.rs"]
mod tests;
