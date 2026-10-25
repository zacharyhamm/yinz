//! Action completion waits and ordered reporting to CLI clients.
use std::time::Duration;
use tokio::sync::oneshot;
use yinz_bus::global_async_runtime::get_tokio_runtime;
use yinz_bus::os_input_output::ClientTransport;
use yinz_bus::route::ActionCompletionResult;
use yinz_bus::ClientId;
use yinz_ipc::ipc::{ExitReason, ServerToClientMsg};
use yinz_runtime::errors::prelude::*;

pub(super) const ACTION_COMPLETION_TIMEOUT: Duration = Duration::from_secs(1);

/// What a wait reports when the action owner never acknowledged completion.
fn unconfirmed() -> ActionCompletionResult {
    ActionCompletionResult {
        confirmed: false,
        agent_tab: None,
        exit_status: None,
        affected_pane_id: None,
        affected_tab_id: None,
        created_tab_id: None,
        error_message: None,
        stdout_message: None,
    }
}

pub fn wait_for_action_completion(
    receiver: oneshot::Receiver<ActionCompletionResult>,
    action_name: &str,
    wait_forever: bool,
) -> ActionCompletionResult {
    let runtime = get_tokio_runtime();
    if wait_forever {
        runtime.block_on(async {
            match receiver.await {
                Ok(result) => result,
                Err(e) => {
                    log::error!("Failed to wait for action {}: {}", action_name, e);
                    unconfirmed()
                },
            }
        })
    } else {
        match runtime
            .block_on(async { tokio::time::timeout(ACTION_COMPLETION_TIMEOUT, receiver).await })
        {
            Ok(Ok(result)) => result,
            // The sender was dropped rather than signalled. That is not a
            // failure and emphatically not a timeout: an action that does its
            // own waiting — every warp does, on a budget of its own — drops the
            // completion channel on purpose, because the thing worth waiting
            // for is not the one this channel reports. Logging it as a timeout
            // put an `Action WarpTab did not complete within 1s timeout` on top
            // of every warp, successful ones included, directly over the signal
            // anyone debugging a warp is looking for.
            Ok(Err(_)) => unconfirmed(),
            Err(_) => {
                log::error!(
                    "Action {} did not complete within {:?} timeout",
                    action_name,
                    ACTION_COMPLETION_TIMEOUT
                );
                unconfirmed()
            },
        }
    }
}

pub(super) fn report_action_completion(
    result: &mut ActionCompletionResult,
    action_name: &str,
    creates_pane: bool,
    creates_group: bool,
    cli_client_id: Option<ClientId>,
    os_input: Option<&dyn ClientTransport>,
) {
    if creates_pane && result.affected_pane_id.is_none() && result.error_message.is_none() {
        result.error_message = Some(format!("Action {action_name} completed without a pane ID"));
    }
    if let Some(error_message) = &result.error_message {
        send_error_to_client(cli_client_id, os_input, error_message);
    }
    // A group's first tab also produces tab/pane IDs; group commands return only the group ID.
    if creates_group {
        result.affected_tab_id = None;
        result.affected_pane_id = None;
    }
    // A CLI client stops at its first Log: include the headless plugin ID in that message.
    let mut pane_id_sent = false;
    if result.error_message.is_none()
        && let Some(stdout_message) = &result.stdout_message
    {
        let mut lines = vec![stdout_message.clone()];
        if let Some(pane_id) = result.affected_pane_id {
            lines.push(pane_id.to_string());
            pane_id_sent = true;
        }
        send_output_to_client(cli_client_id, os_input, lines);
    }
    // Return tab ID to CLI clients as plain text
    if let Some(tab_id) = result.affected_tab_id {
        send_output_to_client(cli_client_id, os_input, vec![tab_id.to_string()]);
    }
    // Return pane ID to CLI clients as plain text
    if !pane_id_sent && let Some(pane_id) = result.affected_pane_id {
        send_output_to_client(cli_client_id, os_input, vec![pane_id.to_string()]);
    }
    // A blocking pane's exit status, last: the client stops reading at `Exit`, so the ids above
    // have to be on the wire before it. Not sent alongside an error or a stdout message, which
    // already ended the client's wait.
    if result.error_message.is_none()
        && result.stdout_message.is_none()
        && let Some(exit_status) = result.exit_status
        && let Some(cli_client_id) = cli_client_id
        && let Some(os_input) = os_input
    {
        os_input
            .send_to_client(
                cli_client_id,
                ServerToClientMsg::Exit {
                    exit_reason: ExitReason::CustomExitStatus(exit_status),
                },
            )
            .with_context(|| format!("sending action exit status to client {cli_client_id}"))
            .non_fatal();
    }
}

pub(super) fn send_error_to_client(
    cli_client_id: Option<ClientId>,
    os_input: Option<&dyn ClientTransport>,
    error_message: &str,
) {
    if let Some(cli_client_id) = cli_client_id
        && let Some(os_input) = os_input
    {
        os_input
            .send_to_client(
                cli_client_id,
                ServerToClientMsg::LogError {
                    lines: vec![error_message.to_string()],
                },
            )
            .with_context(|| format!("sending action error to client {cli_client_id}"))
            .non_fatal();
    }
}

pub(super) fn send_output_to_client(
    cli_client_id: Option<ClientId>,
    os_input: Option<&dyn ClientTransport>,
    output_lines: Vec<String>,
) {
    if let Some(cli_client_id) = cli_client_id
        && let Some(os_input) = os_input
    {
        os_input
            .send_to_client(
                cli_client_id,
                ServerToClientMsg::Log {
                    lines: output_lines,
                },
            )
            .with_context(|| format!("sending action output to client {cli_client_id}"))
            .non_fatal();
    }
}
