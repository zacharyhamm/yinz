//! Resolving plugin launch destinations and scheduling creation on the PTY thread.

use crate::panes::PaneId;
use crate::pty::PtyInstruction;
use crate::route::NotificationEnd;
use crate::screen::Screen;
use crate::ClientId;
use std::path::PathBuf;
use yinz_runtime::errors::prelude::*;
use yinz_types::input::layout::RunPluginOrAlias;
use yinz_types::internal_ids::TabId;
use yinz_types::pane_size::Size;

pub(super) fn new_tiled_plugin_pane(
    screen: &mut Screen,
    run_plugin: RunPluginOrAlias,
    pane_title: Option<String>,
    skip_cache: bool,
    cwd: Option<PathBuf>,
    client_id: ClientId,
    mut completion_tx: Option<NotificationEnd>,
    explicit_tab_id: Option<TabId>,
    no_focus: bool,
) -> Result<()> {
    if explicit_tab_id.is_none()
        && !no_focus
        && screen
            .launch_overlay_tab(
                &run_plugin,
                cwd.clone(),
                skip_cache,
                client_id,
                &mut completion_tx,
            )
            .context("failed to launch plugin in an overlay tab")?
    {
        return Ok(());
    }
    let Some(tab_index) =
        screen.tab_for_plugin_launch(client_id, explicit_tab_id, None, &mut completion_tx)
    else {
        return Ok(());
    };
    let size = Size::default();
    let should_float = Some(false);
    let should_be_opened_in_place = false;
    let should_focus_plugin = if no_focus { Some(false) } else { None };
    screen
        .bus
        .senders
        .send_to_pty(PtyInstruction::FillPluginCwd(
            should_float,
            should_be_opened_in_place,
            false, // close_replaced_pane
            pane_title,
            run_plugin,
            tab_index,
            None,
            client_id,
            size,
            skip_cache,
            cwd,
            should_focus_plugin,
            None,
            completion_tx,
            None,
        ))
        .context("failed to schedule tiled plugin pane")?;
    Ok(())
}

pub(super) fn new_in_place_plugin_pane(
    screen: &mut Screen,
    run_plugin: RunPluginOrAlias,
    pane_title: Option<String>,
    pane_id_to_replace: PaneId,
    skip_cache: bool,
    close_replaced_pane: bool,
    client_id: ClientId,
    mut completion_tx: Option<NotificationEnd>,
    explicit_tab_id: Option<TabId>,
    no_focus: bool,
) -> Result<()> {
    let Some(tab_index) = screen.tab_for_plugin_launch(
        client_id,
        explicit_tab_id,
        Some(pane_id_to_replace),
        &mut completion_tx,
    ) else {
        return Ok(());
    };
    let size = Size::default();
    let should_float = None;
    let should_be_in_place = true;
    let should_focus_plugin = if no_focus { Some(false) } else { None };
    screen
        .bus
        .senders
        .send_to_pty(PtyInstruction::FillPluginCwd(
            should_float,
            should_be_in_place,
            close_replaced_pane,
            pane_title,
            run_plugin,
            tab_index,
            Some(pane_id_to_replace),
            client_id,
            size,
            skip_cache,
            None,
            should_focus_plugin,
            None,
            completion_tx,
            None,
        ))
        .context("failed to schedule in-place plugin pane")?;
    Ok(())
}

pub(super) fn start_or_reload_plugin_pane(
    screen: &mut Screen,
    run_plugin: RunPluginOrAlias,
    pane_title: Option<String>,
    client_id: ClientId,
    mut completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    let detached_tab = if screen.has_no_client_to_act_for(client_id) {
        screen.tabs.keys().next().copied()
    } else {
        None
    };
    let Some(tab_index) =
        screen.tab_for_plugin_launch(client_id, detached_tab, None, &mut completion_tx)
    else {
        return Ok(());
    };
    let size = Size::default();
    let should_float = Some(false);

    screen
        .bus
        .senders
        // ClosePane queues unloading on PTY; reopening must follow that unload.
        .send_to_pty(PtyInstruction::ReloadPlugin {
            should_float,
            pane_title,
            run_plugin,
            tab_index,
            size,
            completion_tx,
        })
        .context("failed to schedule plugin reload")?;
    Ok(())
}
