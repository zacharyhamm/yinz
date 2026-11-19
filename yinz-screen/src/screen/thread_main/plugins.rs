use super::*;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::PathBuf;
use yinz_bus::loading_indication::LoadingIndication;
use yinz_bus::plugins::DumpSessionLayoutResponse;
use yinz_bus::plugins::PluginId;
use yinz_bus::plugins::PluginRenderAsset;
use yinz_bus::pty::ReplacedPane;
use yinz_bus::route::NotificationEnd;
use yinz_bus::ClientId;
use yinz_bus::PaneId;
use yinz_types::data::EventType;
use yinz_types::data::NewPanePlacement;
use yinz_types::data::OverlayPlacement;
use yinz_types::data::RegexHighlight;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::input::layout::RunPluginOrAlias;
use yinz_types::internal_ids::TabId;

pub(super) fn plugin_bytes(
    state: &mut EventLoopState,
    screen: &mut Screen,
    plugin_render_assets: Vec<PluginRenderAsset>,
) -> Result<()> {
    for plugin_render_asset in screen.handle_plugin_render_assets(plugin_render_assets)? {
        let plugin_id = plugin_render_asset.plugin_id;
        state
            .pending
            .waiting_for_pane
            .entry(PaneId::Plugin(plugin_id))
            .or_default()
            .push(ScreenInstruction::PluginBytes(vec![plugin_render_asset]));
    }
    Ok(())
}

pub(super) fn dump_layout_to_plugin(
    screen: &mut Screen,
    plugin_id: PluginId,
    tab_index: Option<TabId>,
    response_channel: crossbeam::channel::Sender<DumpSessionLayoutResponse>,
    trace: yinz_bus::plugins::LayoutQueryTrace,
) -> Result<()> {
    trace.mark("screen metadata");
    let err_context = || "Failed to dump layout".to_string();
    let session_layout_metadata =
        screen.get_layout_metadata(Some(screen.core.default_shell.clone()), tab_index);
    trace.mark("pty queued");
    screen
        .core
        .bus
        .senders
        .send_to_pty(PtyInstruction::DumpLayoutToPlugin {
            session_layout_metadata,
            plugin_id,
            response_channel,
            trace,
        })
        .with_context(err_context)
        .non_fatal();
    Ok(())
}

pub(super) fn list_clients_to_plugin(
    screen: &mut Screen,
    plugin_id: PluginId,
    client_id: ClientId,
    generation: Option<u64>,
) -> Result<()> {
    let err_context = || "Failed to dump layout".to_string();
    let session_layout_metadata =
        screen.get_layout_metadata(Some(screen.core.default_shell.clone()), None);
    screen
        .core
        .bus
        .senders
        .send_to_pty(PtyInstruction::ListClientsToPlugin(
            session_layout_metadata,
            plugin_id,
            client_id,
            generation,
        ))
        .with_context(err_context)
        .non_fatal();
    Ok(())
}

pub(super) fn show_plugin_cursor(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pid: PluginId,
    client_id: ClientId,
    cursor_position: Option<(usize, usize)>,
) -> Result<()> {
    if !screen.show_plugin_cursor(pid, client_id, cursor_position) {
        state
            .pending
            .waiting_for_pane
            .entry(PaneId::Plugin(pid))
            .or_default()
            .push(ScreenInstruction::ShowPluginCursor(
                pid,
                client_id,
                cursor_position,
            ));
    }
    screen.render()?;
    Ok(())
}

pub(super) fn new_tiled_plugin_pane(
    screen: &mut Screen,
    run_plugin: RunPluginOrAlias,
    pane_title: Option<String>,
    skip_cache: bool,
    cwd: Option<PathBuf>,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
    explicit_tab_id: Option<TabId>,
    no_focus: bool,
) -> Result<()> {
    plugin_launch::new_tiled_plugin_pane(
        screen,
        run_plugin,
        pane_title,
        skip_cache,
        cwd,
        client_id,
        completion_tx,
        explicit_tab_id,
        no_focus,
    )?;
    Ok(())
}

pub(super) fn new_floating_plugin_pane(
    screen: &mut Screen,
    run_plugin: RunPluginOrAlias,
    pane_title: Option<String>,
    skip_cache: bool,
    cwd: Option<PathBuf>,
    placement: NewPanePlacement,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
    explicit_tab_id: Option<TabId>,
    no_focus: bool,
) -> Result<()> {
    screen.new_floating_plugin_pane(
        run_plugin,
        pane_title,
        skip_cache,
        cwd,
        placement,
        client_id,
        completion_tx,
        explicit_tab_id,
        no_focus,
    )?;
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
    completion_tx: Option<NotificationEnd>,
    explicit_tab_id: Option<TabId>,
    no_focus: bool,
) -> Result<()> {
    plugin_launch::new_in_place_plugin_pane(
        screen,
        run_plugin,
        pane_title,
        pane_id_to_replace,
        skip_cache,
        close_replaced_pane,
        client_id,
        completion_tx,
        explicit_tab_id,
        no_focus,
    )?;
    Ok(())
}

pub(super) fn start_or_reload_plugin_pane(
    screen: &mut Screen,
    run_plugin: RunPluginOrAlias,
    pane_title: Option<String>,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    plugin_launch::start_or_reload_plugin_pane(
        screen,
        run_plugin,
        pane_title,
        client_id,
        completion_tx,
    )?;
    Ok(())
}

pub(super) fn add_plugin(
    state: &mut EventLoopState,
    screen: &mut Screen,
    should_float: Option<bool>,
    should_be_in_place: bool,
    replaced: ReplacedPane,
    run_plugin_or_alias: RunPluginOrAlias,
    pane_title: Option<String>,
    tab_index: Option<TabId>,
    plugin_id: PluginId,
    pane_id_to_replace: Option<PaneId>,
    cwd: Option<PathBuf>,
    start_suppressed: bool,
    floating_pane_coordinates: Option<FloatingPaneCoordinates>,
    should_focus_plugin: Option<bool>,
    client_id: Option<ClientId>,
    completion_tx: Option<NotificationEnd>,
    stated_overlay_placement: Option<OverlayPlacement>,
) -> Result<()> {
    if screen.add_plugin(
        should_float,
        should_be_in_place,
        replaced,
        run_plugin_or_alias,
        pane_title,
        tab_index,
        plugin_id,
        pane_id_to_replace,
        cwd,
        start_suppressed,
        floating_pane_coordinates,
        should_focus_plugin,
        client_id,
        completion_tx,
        stated_overlay_placement,
        &mut state.pending,
    )? {
        screen.log_and_report_session_state()?;
    }
    Ok(())
}

pub(super) fn update_plugin_loading_stage(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pid: PluginId,
    loading_indication: LoadingIndication,
) -> Result<()> {
    let found_plugin = screen.update_plugin_loading_stage(pid, loading_indication.clone());
    if !found_plugin {
        state
            .pending
            .waiting_for_pane
            .entry(PaneId::Plugin(pid))
            .or_default()
            .push(ScreenInstruction::UpdatePluginLoadingStage(
                pid,
                loading_indication,
            ));
    }
    screen.render()?;
    Ok(())
}

pub(super) fn start_plugin_loading_indication(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pid: PluginId,
    loading_indication: LoadingIndication,
) -> Result<()> {
    if !screen.start_plugin_loading_indication(pid, loading_indication.clone()) {
        state
            .pending
            .waiting_for_pane
            .entry(PaneId::Plugin(pid))
            .or_default()
            .push(ScreenInstruction::StartPluginLoadingIndication(
                pid,
                loading_indication,
            ));
    }
    screen.render()?;
    Ok(())
}

pub(super) fn start_client_plugin_loading_indication(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pid: PluginId,
    client_id: ClientId,
    loading_indication: LoadingIndication,
) -> Result<()> {
    if !screen.start_client_plugin_loading_indication(pid, client_id, loading_indication.clone()) {
        state
            .pending
            .waiting_for_pane
            .entry(PaneId::Plugin(pid))
            .or_default()
            .push(ScreenInstruction::StartClientPluginLoadingIndication(
                pid,
                client_id,
                loading_indication,
            ));
    }
    screen.render()?;
    Ok(())
}
// No replay queue for a plugin whose pane does not exist yet, unlike the loading
// *indication* above: with the offset derived from elapsed time there is nothing to
// replay. The plugin stays registered and the next frame after its pane appears simply
// draws the right number of dots

pub(super) fn start_plugin_loading_animation(screen: &mut Screen, pid: PluginId) -> Result<()> {
    // A plugin that runs headless or is gone never shows a load, and may never
    // hear the matching stop: a reload that drops its declaration from a layout
    // slot is unloaded without finishing, and this start, relayed through the
    // background jobs thread, can land after the unload.
    if screen.plugin_has_no_pane(pid) {
        return Ok(());
    }
    screen.core.animating_plugin_loads.insert(pid);
    screen.sync_animation_clock(Instant::now())?;
    screen.render()?;
    Ok(())
}

pub(super) fn launch_or_focus_plugin(
    screen: &mut Screen,
    run_plugin: RunPluginOrAlias,
    should_float: bool,
    move_to_focused_tab: bool,
    should_open_in_place: bool,
    close_replaced_pane: bool,
    pane_id_to_replace: Option<PaneId>,
    skip_cache: bool,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
    explicit_tab_id: Option<TabId>,
) -> Result<()> {
    screen.launch_or_focus_plugin(
        run_plugin,
        should_float,
        move_to_focused_tab,
        should_open_in_place,
        close_replaced_pane,
        pane_id_to_replace,
        skip_cache,
        client_id,
        completion_tx,
        explicit_tab_id,
    )?;
    Ok(())
}

pub(super) fn launch_plugin(
    screen: &mut Screen,
    run_plugin: RunPluginOrAlias,
    should_float: bool,
    should_open_in_place: bool,
    close_replaced_pane: bool,
    pane_id_to_replace: Option<PaneId>,
    skip_cache: bool,
    cwd: Option<PathBuf>,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
    explicit_tab_id: Option<TabId>,
    no_focus: bool,
    overlay_placement: Option<OverlayPlacement>,
) -> Result<()> {
    screen.launch_plugin(
        run_plugin,
        should_float,
        should_open_in_place,
        close_replaced_pane,
        pane_id_to_replace,
        skip_cache,
        cwd,
        client_id,
        completion_tx,
        explicit_tab_id,
        no_focus,
        overlay_placement,
    )?;
    Ok(())
}

pub(super) fn open_plugin_pane_in_place_of_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    plugin_url: String,
    configuration: BTreeMap<String, String>,
    cwd: PathBuf,
    client_id: ClientId,
    completion: NotificationEnd,
) -> Result<()> {
    screen.open_plugin_pane_in_place_of_pane_id(
        pane_id,
        plugin_url,
        configuration,
        cwd,
        client_id,
        completion,
    );
    Ok(())
}

pub(super) fn set_plugin_regex_highlights(
    screen: &mut Screen,
    pane_id: PaneId,
    plugin_id: PluginId,
    highlights: Vec<RegexHighlight>,
) -> Result<()> {
    let style = screen.core.style;
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.set_plugin_regex_highlights_for_pane(pane_id, plugin_id, highlights, &style);
    }
    screen.render()?;
    Ok(())
}

pub(super) fn update_background_plugin_subscriptions(
    screen: &mut Screen,
    plugin_id: PluginId,
    client_id: ClientId,
    subscriptions: HashSet<EventType>,
) -> Result<()> {
    let previous = screen
        .core
        .background_plugin_subscriptions
        .get(&(plugin_id, client_id));
    let needs_state = [EventType::PaneUpdate, EventType::TabUpdate]
        .iter()
        .any(|event| {
            subscriptions.contains(event)
                && previous.is_none_or(|previous| !previous.contains(event))
        });
    if subscriptions.is_empty() {
        screen
            .core
            .background_plugin_subscriptions
            .remove(&(plugin_id, client_id));
    } else {
        screen
            .core
            .background_plugin_subscriptions
            .insert((plugin_id, client_id), subscriptions);
    }
    // The plugin may subscribe after startup's last layout report. Give it
    // the current panes and tabs even when the session is otherwise quiet.
    if needs_state {
        screen.log_and_report_session_state()?;
    }
    Ok(())
}
