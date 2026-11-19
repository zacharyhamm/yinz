use super::*;
use std::path::PathBuf;
use yinz_bus::plugins::PluginId;
use yinz_bus::route::NotificationEnd;
use yinz_bus::ClientId;
use yinz_bus::PaneId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::pane_size::Size;

pub(super) fn render_to_clients(state: &mut EventLoopState, screen: &mut Screen) -> Result<()> {
    // render_blocker.can_render() returning true means that either all pending plugins
    // (only those waiting for a new tab layout to be applied!) have been rendered or
    // that a 100ms timeout has been reached (more info in the RenderBlocker comment)
    let tracing_attach = !state.attach_traces_waiting_for_first_render.is_empty();
    if screen.core.render_blocker.can_render() {
        let render_started_at = Instant::now();
        if tracing_attach {
            log::info!(
                "attach trace: screen first render begin clients={:?}",
                state
                    .attach_traces_waiting_for_first_render
                    .keys()
                    .copied()
                    .collect::<Vec<_>>()
            );
        }
        let result = if tracing_attach {
            screen.render_to_clients_traced(true)
        } else {
            screen.render_to_clients()
        };
        if tracing_attach {
            log::info!(
                "attach trace: screen first render finished after {:?} ({})",
                render_started_at.elapsed(),
                if result.is_ok() { "ok" } else { "error" }
            );
            if result.is_ok() {
                state.attach_traces_waiting_for_first_render.clear();
            }
        }
        result?;
    } else {
        if !state.attach_traces_waiting_for_first_render.is_empty() {
            log::info!(
                "attach trace: screen first render deferred by render blocker clients={:?}",
                state
                    .attach_traces_waiting_for_first_render
                    .keys()
                    .copied()
                    .collect::<Vec<_>>()
            );
        }
        screen.render()?;
    }
    Ok(())
}

pub(super) fn list_clients_metadata(
    screen: &mut Screen,
    default_shell: Option<PathBuf>,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    let err_context = || "Failed to dump layout".to_string();
    let session_layout_metadata = screen.get_layout_metadata(default_shell, None);
    screen
        .core
        .bus
        .senders
        .send_to_plugin(PluginInstruction::ListClientsMetadata(
            session_layout_metadata,
            client_id,
            completion_tx,
        ))
        .with_context(err_context)?;
    Ok(())
}

pub(super) fn report_client_timezones(
    screen: &mut Screen,
    plugin_id: PluginId,
    client_id: ClientId,
    instance: yinz_bus::plugins::PluginInstanceId,
    generation: Option<u64>,
) -> Result<()> {
    let mut clients: Vec<_> = screen
        .core
        .connected_clients
        .borrow()
        .iter()
        .copied()
        .collect();
    clients.sort();
    let events = clients
        .into_iter()
        .map(|id| screen.client_timezone_event(id))
        .collect();
    screen
        .core
        .bus
        .senders
        .send_to_plugin(PluginInstruction::ClientTimezoneSnapshot {
            plugin_id,
            client_id,
            instance,
            generation,
            events,
        })
        .context("reply with timezone subscription snapshot")
        .non_fatal();
    Ok(())
}

pub(super) fn set_structured_render_support(
    screen: &mut Screen,
    client_id: ClientId,
    supported: bool,
) -> Result<()> {
    screen.legacy_render_support(client_id, supported);
    screen.render()?;
    Ok(())
}

pub(super) fn add_client(
    state: &mut EventLoopState,
    screen: &mut Screen,
    client_id: ClientId,
    client_size: Size,
    tab_number_to_focus: Option<TabNumber>,
    pane_id_to_focus: Option<PaneId>,
) -> Result<()> {
    screen.attach_client(
        client_id,
        client_size,
        tab_number_to_focus,
        pane_id_to_focus,
        &mut state.attach_traces_waiting_for_first_render,
        &mut state.pending,
    )?;
    Ok(())
}

pub(super) fn remove_client(
    state: &mut EventLoopState,
    screen: &mut Screen,
    client_id: ClientId,
) -> Result<()> {
    screen.detach_agent_controls(client_id);
    screen.agents.remove_subscriber(client_id);
    state
        .attach_traces_waiting_for_first_render
        .remove(&client_id);
    // A connection that never attached — a session probe, a one-shot
    // CLI action client, a hook's `yinz action agent-report` — has
    // no screen state, and its departure must not cost the session a
    // reflow and a full session-state broadcast. At several such
    // connections per hook event, those broadcasts were most of the
    // plugin threads' load while an agent worked.
    let was_attached = screen.is_client_attached(client_id);
    screen.core.pending_client_repaints.remove(&client_id);
    screen.remove_client(client_id).non_fatal();
    state.keybind_intercepts.remove(&client_id);
    if !screen.accepts_plugin_output(client_id) {
        state.pending.remove_client_plugin_output(client_id);
    }
    if was_attached {
        // the departing client may have been the one constraining the
        // screen size; resize_to_clients also reports session state.
        screen.resize_to_clients().non_fatal();
        screen.render().non_fatal();
    }
    Ok(())
}

pub(super) fn remove_watcher_client(
    state: &mut EventLoopState,
    screen: &mut Screen,
    client_id: ClientId,
) -> Result<()> {
    let departed = screen.departed_follow_source();
    screen.remove_watcher_client(client_id);
    if screen.core.followed_client_id.is_none()
        && let Some((source, _)) = departed
    {
        state.pending.remove_client_plugin_output(source);
    }
    Ok(())
}

pub(super) fn set_followed_client(
    state: &mut EventLoopState,
    screen: &mut Screen,
    client_id: ClientId,
) -> Result<()> {
    if let Some(departed) = screen
        .core
        .followed_client_id
        .filter(|id| *id != client_id && !screen.core.connected_clients.borrow().contains(id))
    {
        state.pending.remove_client_plugin_output(departed);
    }
    screen
        .set_followed_client(client_id)
        .context("failed to set followed client")?;
    Ok(())
}
