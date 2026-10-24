//! Synchronous pane/tab metadata queries and CLI output formatting.
use super::completion::{send_error_to_client, send_output_to_client};
use yinz_bus::os_input_output::ServerOsApi;
use yinz_bus::pty::PtyInstruction;
use yinz_bus::screen::ScreenInstruction;
use yinz_bus::thread_bus::ThreadSenders;
use yinz_bus::{ClientId, PaneId};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{ListPanesResponse, ListTabsResponse, PaneListEntry, TabInfo};

pub(super) fn list_panes(
    show_tab: bool,
    show_command: bool,
    show_state: bool,
    show_geometry: bool,
    show_all: bool,
    output_json: bool,
    senders: &ThreadSenders,
    cli_client_id: Option<ClientId>,
    os_input: Option<&dyn ServerOsApi>,
    client_id: ClientId,
) -> Result<()> {
    let err_context = || format!("failed to route action for client {client_id}");
    let maybe_panes = request_panes_from_screen(senders, show_all).with_context(err_context)?;

    if let Some(mut pane_entries) = maybe_panes {
        if show_command || show_all || output_json {
            enrich_panes_with_pty_data(&mut pane_entries, senders).with_context(err_context)?;
        }

        let output_lines = if output_json {
            format_panes_as_json(&pane_entries)
        } else {
            format_panes_table(
                &pane_entries,
                show_tab || show_all,
                show_command || show_all,
                show_state || show_all,
                show_geometry || show_all,
            )
        };

        send_output_to_client(cli_client_id, os_input, output_lines);
    } else {
        send_error_to_client(cli_client_id, os_input, "Timeout listing panes");
    }
    Ok(())
}

pub(super) fn list_tabs(
    show_state: bool,
    show_dimensions: bool,
    show_panes: bool,
    show_layout: bool,
    show_all: bool,
    output_json: bool,
    senders: &ThreadSenders,
    client_id: ClientId,
    cli_client_id: Option<ClientId>,
    os_input: Option<&dyn ServerOsApi>,
) -> Result<()> {
    let err_context = || format!("failed to route action for client {client_id}");
    let maybe_tabs = request_tabs_from_screen(senders, client_id).with_context(err_context)?;

    if let Some(tab_infos) = maybe_tabs {
        let output_lines = if output_json {
            format_tabs_as_json(&tab_infos)
        } else {
            format_tabs_table(
                &tab_infos,
                show_state || show_all,
                show_dimensions || show_all,
                show_panes || show_all,
                show_layout || show_all,
            )
        };

        send_output_to_client(cli_client_id, os_input, output_lines);
    } else {
        send_error_to_client(cli_client_id, os_input, "Timeout listing tabs");
    }
    Ok(())
}

pub(super) fn current_tab_info(
    output_json: bool,
    senders: &ThreadSenders,
    client_id: ClientId,
    cli_client_id: Option<ClientId>,
    os_input: Option<&dyn ServerOsApi>,
) -> Result<()> {
    let err_context = || format!("failed to route action for client {client_id}");
    let maybe_tab_info =
        request_current_tab_info_from_screen(senders, client_id).with_context(err_context)?;

    match maybe_tab_info {
        Some(tab_info) => {
            let output_lines = if output_json {
                format_current_tab_info_as_json(&tab_info)
            } else {
                format_current_tab_info_plain(&tab_info)
            };
            send_output_to_client(cli_client_id, os_input, output_lines);
        },
        None => {
            send_error_to_client(
                cli_client_id,
                os_input,
                "No active tab found for current client",
            );
        },
    }
    Ok(())
}

// this should only be used for one-off startup instructions

fn request_panes_from_screen(
    senders: &ThreadSenders,
    show_all: bool,
) -> Result<Option<ListPanesResponse>> {
    use crossbeam::channel::{unbounded, RecvTimeoutError};
    use std::time::Duration;

    let (response_sender, response_receiver) = unbounded();
    senders.send_to_screen(ScreenInstruction::ListPanes {
        show_all,
        response_channel: response_sender,
    })?;

    match response_receiver.recv_timeout(Duration::from_secs(1)) {
        Ok(entries) => Ok(Some(entries)),
        Err(RecvTimeoutError::Timeout) => {
            log::error!("ListPanes timed out waiting for Screen response");
            Ok(None)
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!("ListPanes channel disconnected");
            Ok(None)
        },
    }
}

fn request_tabs_from_screen(
    senders: &ThreadSenders,
    client_id: ClientId,
) -> Result<Option<ListTabsResponse>> {
    use crossbeam::channel::{unbounded, RecvTimeoutError};
    use std::time::Duration;

    let (response_sender, response_receiver) = unbounded();
    senders.send_to_screen(ScreenInstruction::ListTabs {
        client_id,
        response_channel: response_sender,
    })?;

    match response_receiver.recv_timeout(Duration::from_secs(1)) {
        Ok(entries) => Ok(Some(entries)),
        Err(RecvTimeoutError::Timeout) => {
            log::error!("ListTabs timed out waiting for Screen response");
            Ok(None)
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!("ListTabs channel disconnected");
            Ok(None)
        },
    }
}

fn request_current_tab_info_from_screen(
    senders: &ThreadSenders,
    client_id: ClientId,
) -> Result<Option<TabInfo>> {
    use crossbeam::channel::{unbounded, RecvTimeoutError};
    use std::time::Duration;

    let (response_sender, response_receiver) = unbounded();
    senders.send_to_screen(ScreenInstruction::GetCurrentTabInfo {
        client_id,
        response_channel: response_sender,
    })?;

    match response_receiver.recv_timeout(Duration::from_secs(1)) {
        Ok(tab_info_opt) => Ok(tab_info_opt),
        Err(RecvTimeoutError::Timeout) => {
            log::error!("GetCurrentTabInfo timed out waiting for Screen response");
            Ok(None)
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!("GetCurrentTabInfo channel disconnected");
            Ok(None)
        },
    }
}

/// Fill in each terminal pane's command and cwd with one pty request for all of them, so a busy
/// pty thread costs one deadline rather than one per pane. On a timeout the fields stay empty.
///
/// The deadline grows with the pane count: the pty thread answers every pane in one pass, and a
/// configured `post_command_discovery_hook` runs a shell per pane with a foreground process. A
/// fixed deadline would drop the whole listing once enough panes were open, where each pane's
/// own request used to fit in its own budget.
fn enrich_panes_with_pty_data(
    pane_entries: &mut [PaneListEntry],
    senders: &ThreadSenders,
) -> Result<()> {
    use crossbeam::channel::{unbounded, RecvTimeoutError};
    use std::time::Duration;

    /// The least a listing waits for, however few panes it asks about.
    const MIN_TIMEOUT: Duration = Duration::from_secs(1);
    /// What one pane's command lookup was allowed when each pane had a request of its own.
    const PER_PANE_TIMEOUT: Duration = Duration::from_millis(100);

    let pane_ids: Vec<PaneId> = pane_entries
        .iter()
        .filter(|entry| !entry.pane_info.is_plugin())
        .map(|entry| entry.pane_info.id)
        .collect();
    if pane_ids.is_empty() {
        return Ok(());
    }
    let timeout = PER_PANE_TIMEOUT
        .saturating_mul(u32::try_from(pane_ids.len()).unwrap_or(u32::MAX))
        .max(MIN_TIMEOUT);

    let (response_sender, response_receiver) = unbounded();
    senders
        .send_to_pty(PtyInstruction::GetPanesMetadata {
            pane_ids,
            response_channel: response_sender,
        })
        .context("failed to ask the pty thread for pane metadata")?;

    let mut metadata = match response_receiver.recv_timeout(timeout) {
        Ok(metadata) => metadata,
        Err(RecvTimeoutError::Timeout) => {
            log::error!("GetPanesMetadata timed out waiting for Pty response");
            return Ok(());
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!("GetPanesMetadata channel disconnected");
            return Ok(());
        },
    };
    for entry in pane_entries.iter_mut() {
        if let Some(pane_metadata) = metadata.remove(&entry.pane_info.id) {
            entry.pane_command = pane_metadata
                .running_command
                .map(|command| command.join(" "));
            entry.pane_cwd = pane_metadata
                .cwd
                .map(|cwd| cwd.to_string_lossy().to_string());
        }
    }
    Ok(())
}

fn format_panes_as_json(pane_entries: &[PaneListEntry]) -> Vec<String> {
    vec![serde_json::to_string_pretty(pane_entries).unwrap_or_else(|_| "[]".to_string())]
}

fn format_panes_table(
    entries: &[PaneListEntry],
    show_tab: bool,
    show_command: bool,
    show_state: bool,
    show_geometry: bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(build_table_header(
        show_tab,
        show_command,
        show_state,
        show_geometry,
    ));

    for entry in entries {
        lines.push(build_table_row(
            entry,
            show_tab,
            show_command,
            show_state,
            show_geometry,
        ));
    }

    lines
}

fn build_table_header(
    show_tab: bool,
    show_command: bool,
    show_state: bool,
    show_geometry: bool,
) -> String {
    let mut header = Vec::new();

    if show_tab {
        header.push("TAB_ID");
        header.push("TAB_POS");
        header.push("TAB_NAME");
    }

    header.push("PANE_ID");
    header.push("TYPE");
    header.push("TITLE");

    if show_command {
        header.push("COMMAND");
        header.push("CWD");
    }

    if show_state {
        header.push("FOCUSED");
        header.push("FLOATING");
        header.push("EXITED");
    }

    if show_geometry {
        header.push("X");
        header.push("Y");
        header.push("ROWS");
        header.push("COLS");
    }

    header.join("  ")
}

fn build_table_row(
    entry: &PaneListEntry,
    show_tab: bool,
    show_command: bool,
    show_state: bool,
    show_geometry: bool,
) -> String {
    let mut row = Vec::new();

    if show_tab {
        row.push(entry.tab_id.to_string());
        row.push(entry.tab_position.to_string());
        row.push(entry.tab_name.clone());
    }

    row.push(format_pane_id(&entry.pane_info));
    row.push(format_pane_type(&entry.pane_info));
    row.push(entry.pane_info.title.clone());

    if show_command {
        row.push(extract_command(entry));
        row.push(extract_cwd(entry));
    }

    if show_state {
        row.push(entry.pane_info.is_focused.to_string());
        row.push(entry.pane_info.is_floating.to_string());
        row.push(entry.pane_info.exited.to_string());
    }

    if show_geometry {
        row.push(entry.pane_info.pane_x.to_string());
        row.push(entry.pane_info.pane_y.to_string());
        row.push(entry.pane_info.pane_rows.to_string());
        row.push(entry.pane_info.pane_columns.to_string());
    }

    row.join("  ")
}

fn format_pane_id(pane_info: &yinz_types::data::PaneInfo) -> String {
    pane_info.id.to_string()
}

fn format_pane_type(pane_info: &yinz_types::data::PaneInfo) -> String {
    if pane_info.is_plugin() {
        "plugin".to_string()
    } else {
        "terminal".to_string()
    }
}

fn extract_command(entry: &PaneListEntry) -> String {
    entry
        .pane_command
        .as_ref()
        .or(entry.pane_info.terminal_command.as_ref())
        .or(entry.pane_info.plugin_url.as_ref())
        .map(|s| s.as_str())
        .unwrap_or("-")
        .to_string()
}

fn extract_cwd(entry: &PaneListEntry) -> String {
    entry.pane_cwd.as_deref().unwrap_or("-").to_string()
}

fn format_tabs_as_json(tab_infos: &[TabInfo]) -> Vec<String> {
    vec![serde_json::to_string_pretty(tab_infos).unwrap_or_else(|_| "[]".to_string())]
}

fn format_tabs_table(
    tabs: &[TabInfo],
    show_state: bool,
    show_dimensions: bool,
    show_panes: bool,
    show_layout: bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(build_tabs_table_header(
        show_state,
        show_dimensions,
        show_panes,
        show_layout,
    ));

    for tab_info in tabs {
        lines.push(build_tabs_table_row(
            tab_info,
            show_state,
            show_dimensions,
            show_panes,
            show_layout,
        ));
    }

    lines
}

fn build_tabs_table_header(
    show_state: bool,
    show_dimensions: bool,
    show_panes: bool,
    show_layout: bool,
) -> String {
    let mut header = Vec::new();

    // Core fields (always shown)
    header.push("TAB_ID");
    header.push("POSITION");
    header.push("NAME");

    if show_state {
        header.push("ACTIVE");
        header.push("FULLSCREEN");
        header.push("SYNC_PANES");
        header.push("FLOATING_VIS");
    }

    if show_dimensions {
        header.push("VP_ROWS");
        header.push("VP_COLS");
        header.push("DA_ROWS");
        header.push("DA_COLS");
    }

    if show_panes {
        header.push("TILED_PANES");
        header.push("FLOAT_PANES");
        header.push("HIDDEN_PANES");
    }

    if show_layout {
        header.push("SWAP_LAYOUT");
        header.push("LAYOUT_DIRTY");
    }

    header.join("  ")
}

fn build_tabs_table_row(
    tab_info: &TabInfo,
    show_state: bool,
    show_dimensions: bool,
    show_panes: bool,
    show_layout: bool,
) -> String {
    let mut row = Vec::new();

    // Core fields
    row.push(tab_info.tab_id.to_string());
    row.push(tab_info.position.to_string());
    row.push(tab_info.name.clone());

    if show_state {
        row.push(tab_info.active.to_string());
        row.push(tab_info.is_fullscreen_active.to_string());
        row.push(tab_info.is_sync_panes_active.to_string());
        row.push(tab_info.are_floating_panes_visible.to_string());
    }

    if show_dimensions {
        row.push(tab_info.viewport_rows.to_string());
        row.push(tab_info.viewport_columns.to_string());
        row.push(tab_info.display_area_rows.to_string());
        row.push(tab_info.display_area_columns.to_string());
    }

    if show_panes {
        row.push(tab_info.selectable_tiled_panes_count.to_string());
        row.push(tab_info.selectable_floating_panes_count.to_string());
        row.push(tab_info.panes_to_hide.to_string());
    }

    if show_layout {
        row.push(
            tab_info
                .active_swap_layout_name
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        );
        row.push(tab_info.is_swap_layout_dirty.to_string());
    }

    row.join("  ")
}

fn format_current_tab_info_as_json(tab_info: &TabInfo) -> Vec<String> {
    vec![serde_json::to_string_pretty(tab_info).unwrap_or_else(|_| "{}".to_string())]
}

fn format_current_tab_info_plain(tab_info: &TabInfo) -> Vec<String> {
    vec![
        format!("name: {}", tab_info.name),
        format!("id: {}", tab_info.tab_id),
        format!("position: {}", tab_info.position),
    ]
}
