use super::{screen_thread_main, Screen, ScreenInstruction};
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::{PaneFactory, PaneId};
use crate::pty_host::client::{ReleasedPane, ReleasedPtyHost, ReleasedRemotePane};
use crate::{
    channels::SenderWithContext,
    route::{actions_leave_clipboard_message, route_action},
    thread_bus::Bus,
    ClientId, ServerInstruction, ThreadSenders,
};
use yinz_bus::os_input_output::{
    ClientTransport, RemotePanes, ServerEnvironment, TerminalIo, TerminalProcesses,
    TerminalSupervision,
};
use yinz_test_support::Watched;
use yinz_types::internal_ids::NotificationId;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;
use yinz_types::internal_ids::TerminalId;

use insta::assert_snapshot;
use std::path::PathBuf;
use yinz_cli::args::CliAction;
use yinz_config::config::Config;
use yinz_config::screen_config::ScreenConfig;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::WarpedPane;
use yinz_types::diagnostics::ErrorContext;
use yinz_types::geometry::Resize;
use yinz_types::geometry::{PercentOrFixed, SplitDirection};
use yinz_types::input::actions::Action;
use yinz_types::input::context_menu::ContextMenu;
use yinz_types::input::keybinds::Keybinds;
use yinz_types::style::{Style, Styling};
use {
    yinz_types::data::Event, yinz_types::data::EventType, yinz_types::data::Mouse,
    yinz_types::data::OverlayPlacement,
};
use {yinz_types::input::command::RunCommand, yinz_types::input::command::TerminalAction};
use {
    yinz_types::input::layout::FloatingPaneLayout, yinz_types::input::layout::Layout,
    yinz_types::input::layout::PluginAlias, yinz_types::input::layout::PluginUserConfiguration,
    yinz_types::input::layout::Run, yinz_types::input::layout::RunPlugin,
    yinz_types::input::layout::RunPluginLocation, yinz_types::input::layout::RunPluginOrAlias,
    yinz_types::input::layout::TiledPaneLayout,
};

use yinz_types::mouse::MouseEvent;
use {
    yinz_types::input::options::NestedSessionHandling, yinz_types::input::options::Options,
    yinz_types::input::options::PaneFrameStyle,
};

use crate::background_jobs::BackgroundJob;
use crate::os_input_output::{AsyncReader, NullAsyncReader, RenderBacklog, RenderReady};
use crate::pty_writer::PtyWriteInstruction;
use crate::{
    plugins::{PluginInstruction, PluginRenderAsset},
    pty::{ClientTabIndexOrPaneId, PtyInstruction},
};
use std::collections::{BTreeMap, HashSet};
use std::env::set_var;
use std::sync::{Arc, Mutex};
use yinz_ipc::ipc::IpcReceiverWithContext;
use yinz_types::pane_size::{Size, SizeInPixels};
use yinz_types::position::Position;

use interprocess::local_socket::Stream as LocalSocketStream;
use yinz_ipc::ipc::PixelDimensions;
use yinz_runtime::channels::{self, ChannelWithContext, Receiver, Shutdown};
use yinz_types::geometry::{Direction, FloatingPaneCoordinates};
use yinz_types::style::Palette;
use {
    yinz_ipc::ipc::ClientAttributes, yinz_ipc::ipc::ClientToServerMsg,
    yinz_ipc::ipc::ServerToClientMsg, yinz_types::data::InputMode, yinz_types::data::ModeInfo,
    yinz_types::data::NewPanePlacement,
};

use crate::panes::grid::Grid;
use crate::panes::link_handler::LinkHandler;
use crate::panes::sixel::SixelImageStore;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use yinz_ipc::ipc::ExitReason;
use yinz_types::data::PaneContents;

fn take_snapshot_and_cursor_coordinates(
    ansi_instructions: &str,
    grid: &mut Grid,
) -> (Option<(usize, usize)>, String) {
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(grid, ansi_instructions.as_bytes());
    let coords = grid
        .cursor_coordinates()
        .and_then(|(x, y, visible)| if visible { Some((x, y)) } else { None });
    (coords, format!("{:?}", grid))
}

fn grid_for_render_snapshot(screen_size: Size) -> Grid {
    Grid::new(
        screen_size.rows,
        screen_size.cols,
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(Some(SizeInPixels {
            width: 8,
            height: 21,
        }))),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    )
}

fn take_snapshots_and_cursor_coordinates_from_render_events<'a>(
    all_events: impl Iterator<Item = &'a ServerInstruction>,
    screen_size: Size,
) -> Vec<(Option<(usize, usize)>, String)> {
    let mut grid = grid_for_render_snapshot(screen_size);
    let snapshots: Vec<(Option<(usize, usize)>, String)> = all_events
        .filter_map(|server_instruction| {
            match server_instruction {
                ServerInstruction::Render(output) => {
                    if let Some(output) = output {
                        // note this only takes a snapshot of the first client!
                        let raw_snapshot = output.get(&ClientId::new(1)).unwrap();
                        let snapshot =
                            take_snapshot_and_cursor_coordinates(raw_snapshot, &mut grid);
                        Some(snapshot)
                    } else {
                        None
                    }
                },
                _ => None,
            }
        })
        .collect();
    snapshots
}

/// Blocks until the screen has rendered a frame satisfying `is_settled`, and returns it.
///
/// This is the replacement for `sleep(...)` followed by `snapshots.last()`. Renders are emitted
/// asynchronously and coalesce, so *which* frame is last — and how many arrive at all — depends on
/// timing. Asserting on the last one therefore asserts on the scheduler. Waiting for a frame whose
/// **content** shows the state under test is stable regardless of how the renders were batched.
///
/// The grid is cumulative, so every frame is replayed from the beginning each time. That is
/// quadratic in the number of renders and entirely fine at test sizes.
fn wait_for_settled_render(
    received_server_instructions: &Watched<Vec<ServerInstruction>>,
    screen_size: Size,
    what: &str,
    is_settled: impl Fn(&str) -> bool,
) -> String {
    wait_for_render_after(
        received_server_instructions,
        screen_size,
        0,
        what,
        |_, frame| is_settled(frame),
    )
}

fn wait_for_render_after(
    received_server_instructions: &Watched<Vec<ServerInstruction>>,
    screen_size: Size,
    checkpoint: usize,
    what: &str,
    is_settled: impl Fn(Option<(usize, usize)>, &str) -> bool,
) -> String {
    received_server_instructions.wait_until_described(
        what,
        yinz_test_support::default_timeout(),
        |events| {
            // Replay all deltas to reconstruct the grid; only accept new frames.
            let old_frames = events
                .iter()
                .take(checkpoint)
                .filter(|event| matches!(event, ServerInstruction::Render(Some(_))))
                .count();
            take_snapshots_and_cursor_coordinates_from_render_events(events.iter(), screen_size)
                .into_iter()
                .skip(old_frames)
                .find(|(cursor, snapshot)| is_settled(*cursor, snapshot))
                .map(|(_, snapshot)| snapshot)
        },
        |events| {
            let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
                events.iter(),
                screen_size,
            );
            match snapshots.last() {
                Some((_, last)) => format!(
                    "{} instruction(s), {} render(s); last render was:\n{last}",
                    events.len(),
                    snapshots.len()
                ),
                None => format!("{} instruction(s) and no renders at all", events.len()),
            }
        },
    )
}

fn send_cli_action_to_server(
    session_metadata: &MockSession,
    cli_action: CliAction,
    client_id: ClientId,
) {
    let get_current_dir = || PathBuf::from(".");
    let actions =
        yinz_cli::actions::actions_from_cli(cli_action, Box::new(get_current_dir), None).unwrap();
    let senders = session_metadata.senders.clone();
    let default_shell = None;
    let default_mode = session_metadata.default_mode();
    let leaves_clipboard_message = actions_leave_clipboard_message(&actions);
    for action in actions {
        route_action(
            action,
            client_id,
            None,
            None,
            senders.clone(),
            default_shell.clone(),
            None,
            default_mode,
            None,
            leaves_clipboard_message,
            true, // is_client_input
        )
        .unwrap();
    }
}

fn route_arbitrary_action_to_server(
    session_metadata: &MockSession,
    action: Action,
    client_id: ClientId,
) {
    let senders = session_metadata.senders.clone();
    let default_mode = session_metadata.default_mode();
    route_action(
        action,
        client_id,
        None,
        None,
        senders,
        None,
        None,
        default_mode,
        None,
        false,
        true, // is_client_input
    )
    .unwrap();
}

/// Everything the screen wrote to each terminal's stdin, keyed by terminal id.
type TtyStdinBytes = Arc<Mutex<BTreeMap<TerminalId, Vec<u8>>>>;
type ServerReceiver = Receiver<(ServerInstruction, ErrorContext)>;
type PluginReceiver = Receiver<(PluginInstruction, ErrorContext)>;
/// Every message the fake os api sent to each client, shared with the `FakeInputOutput` that
/// records them.
type CapturedClientMessages = Arc<Mutex<HashMap<ClientId, Vec<ServerToClientMsg>>>>;
/// What the fake os api reports as each client's render backlog.
type RenderBacklogs = Arc<Mutex<Vec<(ClientId, RenderBacklog)>>>;
type RenderReadyCallbacks = Arc<Mutex<HashMap<ClientId, RenderReady>>>;

#[derive(Clone, Default)]
struct FakeInputOutput {
    fake_filesystem: Arc<Mutex<HashMap<String, String>>>,
    server_to_client_messages: CapturedClientMessages,
    tty_stdin_bytes: TtyStdinBytes,
    render_backlog: RenderBacklogs,
    render_ready: RenderReadyCallbacks,
}

impl TerminalIo for FakeInputOutput {
    fn set_terminal_size_using_terminal_id(
        &self,
        _terminal_id: TerminalId,
        _cols: u16,
        _rows: u16,
        _width_in_pixels: Option<u16>,
        _height_in_pixels: Option<u16>,
    ) -> Result<()> {
        // noop
        Ok(())
    }
    fn write_to_tty_stdin(&self, id: TerminalId, buf: &[u8]) -> Result<usize> {
        self.tty_stdin_bytes
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn tcdrain(&self, _id: TerminalId) -> Result<()> {
        unimplemented!()
    }
}

impl TerminalProcesses for FakeInputOutput {
    fn spawn_terminal(
        &self,
        _file_to_open: TerminalAction,
        _quit_db: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        _default_editor: Option<PathBuf>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        Ok((crate::TerminalId::new(42), Box::new(NullAsyncReader), None))
    }
    fn kill(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn force_kill(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn re_run_command_in_terminal(
        &self,
        _terminal_id: TerminalId,
        _run_command: RunCommand,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        unimplemented!()
    }
    fn clear_terminal_id(&self, _terminal_id: TerminalId) -> Result<()> {
        unimplemented!()
    }
    fn send_sigint(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn send_sigcont(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
}

impl TerminalSupervision for FakeInputOutput {}

impl RemotePanes for FakeInputOutput {}

impl ClientTransport for FakeInputOutput {
    fn send_to_client(&self, client_id: ClientId, msg: ServerToClientMsg) -> Result<()> {
        self.server_to_client_messages
            .lock()
            .unwrap()
            .entry(client_id)
            .or_default()
            .push(msg);
        Ok(())
    }
    fn new_client(
        &mut self,
        _client_id: ClientId,
        _stream: LocalSocketStream,
    ) -> Result<IpcReceiverWithContext<ClientToServerMsg>> {
        unimplemented!()
    }
    fn remove_client(&mut self, _client_id: ClientId) -> Result<()> {
        unimplemented!()
    }
    fn client_render_backlog_and_notify(
        &self,
        client_id: ClientId,
        threshold: usize,
        ready: &RenderReady,
    ) -> Option<RenderBacklog> {
        let backlog = self
            .render_backlog
            .lock()
            .ok()?
            .iter()
            .find(|(id, _)| *id == client_id)
            .map(|(_, backlog)| *backlog);
        if backlog.is_some_and(|b| b.queued_renders >= threshold) {
            self.render_ready
                .lock()
                .unwrap()
                .insert(client_id, ready.clone());
        }
        backlog
    }
}

impl ServerEnvironment for FakeInputOutput {
    fn load_palette(&self) -> Palette {
        unimplemented!()
    }
    fn get_cwd(&self, _pid: ProcessId) -> Option<PathBuf> {
        unimplemented!()
    }
    fn write_to_file(&mut self, contents: String, filename: Option<String>) -> Result<()> {
        if let Some(filename) = filename {
            self.fake_filesystem
                .lock()
                .unwrap()
                .insert(filename, contents);
        }
        Ok(())
    }
}

fn create_new_screen(
    size: Size,
    advanced_mouse_actions: bool,
    mouse_hover_effects: bool,
) -> Screen {
    create_new_screen_with_kitty_graphics(size, advanced_mouse_actions, mouse_hover_effects, true)
}

fn create_new_screen_with_kitty_graphics(
    size: Size,
    advanced_mouse_actions: bool,
    mouse_hover_effects: bool,
    support_kitty_graphics_protocol: bool,
) -> Screen {
    let (screen, _tty_stdin_bytes, _server_receiver) = create_new_screen_with_capture(
        size,
        advanced_mouse_actions,
        mouse_hover_effects,
        support_kitty_graphics_protocol,
        true,
    );
    screen
}

/// Like `create_new_screen_with_kitty_graphics`, but also hands back what the screen writes to
/// each pane's stdin and what it sends to the server thread.
/// The screen configuration the direct constructors use: the config defaults, with no pane
/// frames (the snapshots are drawn without them), no configured shell, the stacked pane list
/// off, and agent placement off - a test that wants it sets the floor.
fn test_screen_config() -> ScreenConfig {
    let mut screen_config = ScreenConfig::from_config(&Config::default());
    screen_config.pane_frame_style = PaneFrameStyle::None;
    screen_config.stacked_pane_list = false;
    screen_config.agent_pane_min_size = Size { rows: 0, cols: 0 };
    screen_config
}

fn create_new_screen_with_capture(
    size: Size,
    advanced_mouse_actions: bool,
    mouse_hover_effects: bool,
    support_kitty_graphics_protocol: bool,
    session_is_mirrored: bool,
) -> (Screen, TtyStdinBytes, ServerReceiver) {
    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    let fake_os_input = FakeInputOutput::default();
    let tty_stdin_bytes = fake_os_input.tty_stdin_bytes.clone();
    bus.os_input = Some(Box::new(fake_os_input));
    let (to_server, server_receiver): ChannelWithContext<ServerInstruction> = channels::unbounded();
    bus.senders.to_server = Some(SenderWithContext::new(to_server));
    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let max_panes = None;
    let mode_info = ModeInfo {
        session_name: Some("yinz-test".into()),
        ..Default::default()
    };
    let default_layout = Box::new(Layout::default());

    let debug = false;
    let mut screen_config = test_screen_config();
    screen_config.advanced_mouse_actions = advanced_mouse_actions;
    screen_config.mouse_hover_effects = mouse_hover_effects;
    screen_config.support_kitty_graphics_protocol = support_kitty_graphics_protocol;
    screen_config.session_is_mirrored = session_is_mirrored;
    let screen = Screen::new(
        bus,
        &client_attributes,
        max_panes,
        mode_info,
        debug,
        default_layout,
        screen_config,
    );
    (
        seed_first_client_size(screen, size),
        tty_stdin_bytes,
        server_receiver,
    )
}

fn seed_first_client_size(mut screen: Screen, size: Size) -> Screen {
    screen
        .set_client_size(crate::ClientId::new(1), size)
        .unwrap();
    screen
}

/// The part of the server's session the screen tests drive: the senders every thread shares and
/// the config paths the screen thread starts with. The server's own session also owns the thread
/// handles and the per-client configuration, neither of which a screen test has: its
/// configuration is always the default one, so every client's default mode is `Normal`.
///
/// Dropping one tells every worker to exit through `ThreadSenders::exit_threads`, as dropping
/// the server's session does, and tests lean on that: a clone going out of scope at the end of a
/// test is what stops the threads it started.
#[derive(Clone)]
struct MockSession {
    shutdown: Shutdown,
    senders: ThreadSenders,
    config_file_path: Option<PathBuf>,
    config_dir: Option<PathBuf>,
}

impl MockSession {
    /// What the server resolves a client's default mode to under the default configuration.
    fn default_mode(&self) -> InputMode {
        InputMode::Normal
    }

    /// The server's `propagate_configuration_changes` under the default configuration: no plugin
    /// has registered default keybinds, so each config reaches the threads as is, through the
    /// same `ThreadSenders::reconfigure_threads` the server sends with.
    pub fn propagate_configuration_changes(
        &self,
        config_changes: Vec<(ClientId, Config)>,
        config_was_written_to_disk: bool,
    ) {
        self.senders.reconfigure_threads(
            config_changes,
            self.config_dir.as_deref(),
            config_was_written_to_disk,
        );
    }
}

impl Drop for MockSession {
    fn drop(&mut self) {
        self.shutdown.begin();
        self.senders.exit_threads(|| {}, || {});
    }
}

struct MockScreen {
    pub main_client_id: ClientId,
    pub pty_receiver: Option<Receiver<(PtyInstruction, ErrorContext)>>,
    pub pty_writer_receiver: Option<Receiver<(PtyWriteInstruction, ErrorContext)>>,
    #[allow(dead_code)]
    pub background_jobs_receiver: Option<Receiver<(BackgroundJob, ErrorContext)>>,
    pub received_background_jobs: Watched<Vec<BackgroundJob>>,
    pub screen_receiver: Option<Receiver<(ScreenInstruction, ErrorContext)>>,
    pub server_receiver: Option<Receiver<(ServerInstruction, ErrorContext)>>,
    pub plugin_receiver: Option<Receiver<(PluginInstruction, ErrorContext)>>,
    pub to_screen: SenderWithContext<ScreenInstruction>,
    pub to_pty: SenderWithContext<PtyInstruction>,
    pub to_plugin: SenderWithContext<PluginInstruction>,
    pub to_server: SenderWithContext<ServerInstruction>,
    pub to_pty_writer: SenderWithContext<PtyWriteInstruction>,
    pub to_background_jobs: SenderWithContext<BackgroundJob>,
    pub os_input: FakeInputOutput,
    pub client_attributes: ClientAttributes,
    #[allow(dead_code)]
    pub config_options: Options,
    pub session_metadata: MockSession,
    pub config: Config,
    default_layout: Box<Layout>,
    advanced_mouse_actions: bool,
    last_opened_tab_index: Option<TabId>,
}

impl MockScreen {
    pub fn run(
        &mut self,
        initial_layout: Option<TiledPaneLayout>,
        initial_floating_panes_layout: Vec<FloatingPaneLayout>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
        let mut config = self.config.clone();
        config.options.advanced_mouse_actions = Some(self.advanced_mouse_actions);
        // Off unless a test turns it on: a `MockScreen` has no plugin thread to apply a new
        // tab's layout, so an agent pane sent to a tab of its own would sit in a pending tab
        // for the rest of the test, its held command and bytes buffered and never replayed.
        config.options.agent_pane_min_columns.get_or_insert(0);
        config.options.agent_pane_min_rows.get_or_insert(0);
        let client_attributes = self.client_attributes.clone();
        let default_layout = self.default_layout.clone();
        let screen_bus = Bus::new(
            vec![self.screen_receiver.take().unwrap()],
            Some(&self.to_screen.clone()),
            Some(&self.to_pty.clone()),
            Some(&self.to_plugin.clone()),
            Some(&self.to_server.clone()),
            Some(&self.to_pty_writer.clone()),
            Some(&self.to_background_jobs.clone()),
            Some(Box::new(self.os_input.clone())),
        )
        .should_silently_fail();
        let debug = false;
        let config_file_path = self.session_metadata.config_file_path.clone();
        let config_dir = self.session_metadata.config_dir.clone();
        let screen_thread = std::thread::Builder::new()
            .name("screen_thread".to_string())
            .spawn(move || {
                // FIXME: Audit that the environment access only happens in single-threaded code.
                unsafe { set_var("YINZ_SESSION_NAME", "yinz-test") };
                screen_thread_main(
                    screen_bus,
                    None,
                    client_attributes,
                    config,
                    debug,
                    config_file_path,
                    config_dir,
                    None,
                    default_layout,
                )
                .expect("TEST")
            })
            .unwrap();
        let pane_layout = initial_layout.unwrap_or_default();
        let pane_count = pane_layout.extract_run_instructions().len();
        let floating_pane_count = initial_floating_panes_layout.len();
        let mut pane_ids = vec![];
        let mut floating_pane_ids = vec![];
        let mut plugin_ids = HashMap::new();
        plugin_ids.insert(
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                "file:/path/to/fake/plugin",
                &None,
                None,
                None,
            )
            .unwrap(),
            vec![yinz_types::internal_ids::PluginId::new(1)],
        );
        for i in 0..pane_count {
            pane_ids.push((TerminalId::new(i as u32), None));
        }
        for i in 0..floating_pane_count {
            floating_pane_ids.push((TerminalId::new(i as u32), None));
        }
        let default_shell = None;
        let tab_name = None;
        let tab_index = TabId::new(self.last_opened_tab_index.map(|l| l.get() + 1).unwrap_or(0));
        let should_change_focus_to_new_tab = true;
        std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async
                                                                   // render
        let _ = self.to_screen.send(ScreenInstruction::NewTab(
            None,
            default_shell,
            Some(pane_layout.clone()),
            initial_floating_panes_layout.clone(),
            tab_name,
            (Some(vec![]), Some(vec![])), // swap layouts (None would fall back to Screen::default_layout)
            None,                         // initial_panes
            false,
            should_change_focus_to_new_tab,
            self.main_client_id,
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            pane_layout,
            initial_floating_panes_layout,
            pane_ids,
            floating_pane_ids,
            vec![],
            plugin_ids,
            tab_index,
            true,
            self.main_client_id,
            None,
            None,
        ));
        self.last_opened_tab_index = Some(tab_index);
        std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
        screen_thread
    }
    // same as the above function, but starts a plugin with a plugin alias
    pub fn run_with_alias(
        &mut self,
        initial_layout: Option<TiledPaneLayout>,
        initial_floating_panes_layout: Vec<FloatingPaneLayout>,
    ) -> std::thread::JoinHandle<()> {
        let config = self.config.clone();
        let client_attributes = self.client_attributes.clone();
        let screen_bus = Bus::new(
            vec![self.screen_receiver.take().unwrap()],
            Some(&self.to_screen.clone()),
            Some(&self.to_pty.clone()),
            Some(&self.to_plugin.clone()),
            Some(&self.to_server.clone()),
            Some(&self.to_pty_writer.clone()),
            Some(&self.to_background_jobs.clone()),
            Some(Box::new(self.os_input.clone())),
        )
        .should_silently_fail();
        let debug = false;
        let config_file_path = self.session_metadata.config_file_path.clone();
        let config_dir = self.session_metadata.config_dir.clone();
        let screen_thread = std::thread::Builder::new()
            .name("screen_thread".to_string())
            .spawn(move || {
                // FIXME: Audit that the environment access only happens in single-threaded code.
                unsafe { set_var("YINZ_SESSION_NAME", "yinz-test") };
                screen_thread_main(
                    screen_bus,
                    None,
                    client_attributes,
                    config,
                    debug,
                    config_file_path,
                    config_dir,
                    None,
                    Box::default(),
                )
                .expect("TEST")
            })
            .unwrap();
        let pane_layout = initial_layout.unwrap_or_default();
        let pane_count = pane_layout.extract_run_instructions().len();
        let floating_pane_count = initial_floating_panes_layout.len();
        let mut pane_ids = vec![];
        let mut floating_pane_ids = vec![];
        let mut plugin_ids = HashMap::new();
        plugin_ids.insert(
            RunPluginOrAlias::Alias(PluginAlias {
                name: "fixture_plugin_for_tests".to_owned(),
                configuration: Some(Default::default()),
                run_plugin: Some(RunPlugin {
                    location: yinz_runtime::plugin_location::parse_plugin_location(
                        "file:/path/to/fake/plugin",
                        None,
                    )
                    .unwrap(),
                    configuration: PluginUserConfiguration::default(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            vec![yinz_types::internal_ids::PluginId::new(1)],
        );
        for i in 0..pane_count {
            pane_ids.push((TerminalId::new(i as u32), None));
        }
        for i in 0..floating_pane_count {
            floating_pane_ids.push((TerminalId::new(i as u32), None));
        }
        let default_shell = None;
        let tab_name = None;
        let tab_index = TabId::new(self.last_opened_tab_index.map(|l| l.get() + 1).unwrap_or(0));
        let should_change_focus_to_new_tab = true;
        let _ = self.to_screen.send(ScreenInstruction::NewTab(
            None,
            default_shell,
            Some(pane_layout.clone()),
            initial_floating_panes_layout.clone(),
            tab_name,
            (Some(vec![]), Some(vec![])), // swap layouts (None would fall back to Screen::default_layout)
            None,                         // initial_panes
            false,
            should_change_focus_to_new_tab,
            self.main_client_id,
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            pane_layout,
            initial_floating_panes_layout,
            pane_ids,
            floating_pane_ids,
            vec![],
            plugin_ids,
            tab_index,
            true,
            self.main_client_id,
            None,
            None,
        ));
        self.last_opened_tab_index = Some(tab_index);
        screen_thread
    }
    pub fn new_tab(&mut self, tab_layout: TiledPaneLayout) {
        let pane_count = tab_layout.extract_run_instructions().len();
        let mut pane_ids = vec![];
        let plugin_ids = HashMap::new();
        let default_shell = None;
        let tab_name = None;
        let tab_index = TabId::new(self.last_opened_tab_index.map(|l| l.get() + 1).unwrap_or(0));
        for i in 0..pane_count {
            pane_ids.push((TerminalId::new(i as u32), None));
        }
        let should_change_focus_to_new_tab = true;
        let _ = self.to_screen.send(ScreenInstruction::NewTab(
            None,
            default_shell,
            Some(tab_layout.clone()),
            vec![], // floating_panes_layout
            tab_name,
            (Some(vec![]), Some(vec![])), // swap layouts (None would fall back to Screen::default_layout)
            None,                         // initial_panes
            false,
            should_change_focus_to_new_tab,
            self.main_client_id,
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            tab_layout,
            vec![], // floating_panes_layout
            pane_ids,
            vec![], // floating panes ids
            vec![], // adopted agents
            plugin_ids,
            TabId::new(0),
            true,
            self.main_client_id,
            None,
            None,
        ));
        self.last_opened_tab_index = Some(tab_index);
    }
    pub fn new_tab_with_plugins(
        &mut self,
        plugin_pane_ids: Vec<yinz_types::internal_ids::PluginId>,
    ) {
        // Build a layout where each child is a plugin pane
        let fake_plugin_url = "file:/path/to/fake/plugin";
        let run_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            fake_plugin_url,
            &None,
            None,
            None,
        )
        .unwrap();
        let tab_layout = TiledPaneLayout {
            children_split_direction: SplitDirection::Vertical,
            children: plugin_pane_ids
                .iter()
                .map(|_| TiledPaneLayout {
                    run: Some(Run::Plugin(run_plugin.clone())),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let pane_ids = vec![]; // no terminal panes
        let mut plugin_ids = HashMap::new();
        plugin_ids.insert(run_plugin, plugin_pane_ids);
        let default_shell = None;
        let tab_name = None;
        let tab_index = TabId::new(self.last_opened_tab_index.map(|l| l.get() + 1).unwrap_or(0));
        let should_change_focus_to_new_tab = true;
        let _ = self.to_screen.send(ScreenInstruction::NewTab(
            None,
            default_shell,
            Some(tab_layout.clone()),
            vec![], // floating_panes_layout
            tab_name,
            (Some(vec![]), Some(vec![])), // swap layouts (None would fall back to Screen::default_layout)
            None,                         // initial_panes
            false,
            should_change_focus_to_new_tab,
            self.main_client_id,
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            tab_layout,
            vec![], // floating_panes_layout
            pane_ids,
            vec![], // floating panes ids
            vec![], // adopted agents
            plugin_ids,
            tab_index,
            true,
            self.main_client_id,
            None,
            None,
        ));
        self.last_opened_tab_index = Some(tab_index);
    }
    /// Shuts the fake session down and re-raises any panic from the threads it joins.
    ///
    /// Discarding the join results - as this used to - means a panicked screen thread shows up as
    /// a test failing on a wrong or missing snapshot, which sends the reader looking at the
    /// assertion instead of at the panic that caused it. Re-raising puts the original panic, with
    /// its own message and location, back in front of them.
    ///
    /// The Exits go out as the server's session sends them, through
    /// `ThreadSenders::exit_threads`, and are still allowed to fail: a thread that has already
    /// exited has already done what the Exit message would have asked of it, and its `join` below
    /// is the real check.
    pub fn teardown(&mut self, threads: Vec<std::thread::JoinHandle<()>>) {
        self.session_metadata.senders.exit_threads(|| {}, || {});
        let _ = self.to_server.send(ServerInstruction::KillSession);
        for thread in threads {
            if let Err(panic) = thread.join() {
                std::panic::resume_unwind(panic);
            }
        }
    }
    pub fn clone_session_metadata(&self) -> MockSession {
        self.session_metadata.clone()
    }
}

impl MockScreen {
    pub fn new(size: Size) -> Self {
        let (to_server, server_receiver): ChannelWithContext<ServerInstruction> =
            channels::bounded(50);
        let to_server = SenderWithContext::new(to_server);

        let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> =
            channels::unbounded();
        let to_screen = SenderWithContext::new(to_screen);

        let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> =
            channels::unbounded();
        let to_plugin = SenderWithContext::new(to_plugin);
        let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
        let to_pty = SenderWithContext::new(to_pty);

        let (to_pty_writer, pty_writer_receiver): ChannelWithContext<PtyWriteInstruction> =
            channels::unbounded();
        let to_pty_writer = SenderWithContext::new(to_pty_writer);

        let (to_background_jobs, background_jobs_receiver): ChannelWithContext<BackgroundJob> =
            channels::unbounded();
        let to_background_jobs = SenderWithContext::new(to_background_jobs);

        let client_attributes = ClientAttributes {
            size,
            ..Default::default()
        };

        let session_metadata = MockSession {
            shutdown: Default::default(),
            senders: ThreadSenders {
                to_screen: Some(to_screen.clone()),
                to_pty: Some(to_pty.clone()),
                to_plugin: Some(to_plugin.clone()),
                to_pty_writer: Some(to_pty_writer.clone()),
                to_background_jobs: Some(to_background_jobs.clone()),
                to_server: Some(to_server.clone()),
                session: Default::default(),
                session_checkpoint: Default::default(),
                session_ownership: None,
                should_silently_fail: true,
            },
            config_file_path: None,
            config_dir: None,
        };

        let os_input = FakeInputOutput::default();
        let config_options = Options::default();
        let mut config = Config::default();
        config.options.pane_frame_style = Some(PaneFrameStyle::Full);
        config.options.stacked_pane_list = Some(false);
        let main_client_id = ClientId::new(1);

        let _ = to_screen.send(ScreenInstruction::RecomputeTabSize(main_client_id, size));

        let received_background_jobs = Watched::new(vec![]);
        std::thread::Builder::new()
            .name("background_jobs_thread".to_string())
            .spawn({
                let to_screen = to_screen.clone();
                let received_background_jobs = received_background_jobs.clone();
                move || loop {
                    let (event, _err_ctx) = background_jobs_receiver
                        .recv()
                        .expect("failed to receive event on channel");
                    received_background_jobs.lock().push(event.clone());
                    match event {
                        BackgroundJob::RenderToClients => {
                            let _ = to_screen.send(ScreenInstruction::RenderToClients);
                        },
                        BackgroundJob::Exit => {
                            break;
                        },
                        _ => {},
                    }
                }
            })
            .unwrap();
        MockScreen {
            main_client_id,
            pty_receiver: Some(pty_receiver),
            pty_writer_receiver: Some(pty_writer_receiver),
            background_jobs_receiver: None,
            received_background_jobs,
            screen_receiver: Some(screen_receiver),
            server_receiver: Some(server_receiver),
            plugin_receiver: Some(plugin_receiver),
            to_screen,
            to_pty,
            to_plugin,
            to_server,
            to_pty_writer,
            to_background_jobs,
            os_input,
            client_attributes,
            config_options,
            session_metadata,
            last_opened_tab_index: None,
            config,
            default_layout: Box::new(Layout::default()),
            advanced_mouse_actions: true,
        }
    }
    pub fn set_advanced_hover_effects(&mut self, advanced_mouse_actions: bool) {
        self.advanced_mouse_actions = advanced_mouse_actions;
    }
    pub fn drop_all_pty_messages(&mut self) {
        let pty_receiver = self.pty_receiver.take();
        std::thread::Builder::new()
            .name("pty_thread".to_string())
            .spawn({
                move || {
                    if let Some(pty_receiver) = pty_receiver {
                        loop {
                            let (event, _err_ctx) = pty_receiver
                                .recv()
                                .expect("failed to receive event on channel");
                            match event {
                                PtyInstruction::Exit => {
                                    break;
                                },
                                _ => {
                                    // here the event will be dropped - we do this so that the completion_tx will drop and release the
                                    // test actions
                                },
                            }
                        }
                    }
                }
            })
            .unwrap();
    }
}

macro_rules! log_actions_in_thread {
    ( $arc_mutex_log:expr_2021, $exit_event:path, $receiver:expr_2021 ) => {
        std::thread::Builder::new()
            .name("pty_writer_thread".to_string())
            .spawn({
                let log = $arc_mutex_log.clone();
                move || loop {
                    let (event, _err_ctx) = $receiver
                        .recv()
                        .expect("failed to receive event on channel");
                    match event {
                        $exit_event => {
                            log.lock().push(event.clone());
                            break;
                        },
                        _ => {
                            log.lock().push(event.clone());
                        },
                    }
                }
            })
            .unwrap()
    };
}

fn new_tab(screen: &mut Screen, pid: u32, tab_index: TabId) {
    let client_id = ClientId::new(1);
    let new_terminal_ids = vec![(TerminalId::new(pid), None)];
    let new_plugin_ids = HashMap::new();
    screen
        .new_tab(tab_index, (vec![], vec![]), None, Some(client_id))
        .expect("TEST");
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![], // floating panes layout
            new_terminal_ids,
            vec![], // new floating terminal ids
            new_plugin_ids,
            tab_index,
            true,
            client_id,
            None,
        )
        .expect("TEST");
}

fn create_fixed_size_screen() -> Screen {
    create_new_screen(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
        true,
    )
}

fn create_new_screen_with_message_capture(size: Size) -> (Screen, CapturedClientMessages) {
    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    let fake_os_input = FakeInputOutput::default();
    let messages = fake_os_input.server_to_client_messages.clone();
    bus.os_input = Some(Box::new(fake_os_input));
    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let max_panes = None;
    let mode_info = ModeInfo {
        session_name: Some("yinz-test".into()),
        ..Default::default()
    };
    let default_layout = Box::new(Layout::default());
    let debug = false;
    let mut screen_config = test_screen_config();
    screen_config.session_is_mirrored = true;
    let screen = Screen::new(
        bus,
        &client_attributes,
        max_panes,
        mode_info,
        debug,
        default_layout,
        screen_config,
    );
    (seed_first_client_size(screen, size), messages)
}

// =====================================================================
// Host-reply forwarding (CSI 2031)
//
// These tests exercise the token-lifecycle API on `Screen` directly —
// no route.rs, no thread spawn, no client. The harness plugs real
// `to_server` / `to_pty_writer` channels into the Bus so the forward
// dispatch (→ `ServerInstruction::ForwardQueryToHost`) and the reply
// write (→ `PtyWriteInstruction::Write`) can be asserted.
// =====================================================================

struct ForwardCapture {
    server_rx: Receiver<(ServerInstruction, ErrorContext)>,
    pty_writer_rx: Receiver<(PtyWriteInstruction, ErrorContext)>,
}

impl ForwardCapture {
    /// Drain every pending `ServerInstruction::ForwardQueryToHost` and
    /// return them as `(token, query_bytes)` pairs. Other variants are
    /// dropped — the forward path only ever emits this one.
    fn drain_forward_queries(&self) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.server_rx.try_recv() {
            if let ServerInstruction::ForwardQueryToHost(token, bytes, _) = instr {
                out.push((token, bytes));
            }
        }
        out
    }

    fn drain_forward_queries_with_async(&self) -> Vec<(u32, Vec<u8>, bool)> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.server_rx.try_recv() {
            if let ServerInstruction::ForwardQueryToHost(token, bytes, resolve_async) = instr {
                out.push((token, bytes, resolve_async));
            }
        }
        out
    }

    /// Drain every pending `PtyWriteInstruction::Write`, returning
    /// `(bytes, terminal_id)` — the two fields the reply path sets.
    fn drain_pty_writes(&self) -> Vec<(Vec<u8>, TerminalId)> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.pty_writer_rx.try_recv() {
            if let PtyWriteInstruction::Write(bytes, terminal_id, _) = instr {
                out.push((bytes, terminal_id));
            }
        }
        out
    }

    /// Drain every pending `ServerInstruction::KeyPassthroughChanged`,
    /// returning the `notify_guest` flag for each. Other variants are
    /// dropped.
    fn drain_key_passthrough_notify_flags(&self) -> Vec<bool> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.server_rx.try_recv() {
            if let ServerInstruction::KeyPassthroughChanged(_, _, _, _, _, notify_guest) = instr {
                out.push(notify_guest);
            }
        }
        out
    }
}

fn create_new_screen_with_forward_capture(size: Size) -> (Screen, ForwardCapture) {
    let (server_tx, server_rx) = channels::unbounded::<(ServerInstruction, ErrorContext)>();
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();

    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    let fake_os_input = FakeInputOutput::default();
    bus.os_input = Some(Box::new(fake_os_input));

    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let max_panes = None;
    let mode_info = ModeInfo {
        session_name: Some("yinz-test".into()),
        ..Default::default()
    };
    let default_layout = Box::new(Layout::default());
    let debug = false;
    let mut screen_config = test_screen_config();
    screen_config.session_is_mirrored = true;
    let screen = Screen::new(
        bus,
        &client_attributes,
        max_panes,
        mode_info,
        debug,
        default_layout,
        screen_config,
    );
    (
        seed_first_client_size(screen, size),
        ForwardCapture {
            server_rx,
            pty_writer_rx,
        },
    )
}

// Convenience constructors for the forwarding tests — all callers
// want a fresh `HostQuery` value with the default terminator.
use crate::host_query::{HostQuery, OscTerminator};

fn bg_query() -> HostQuery {
    HostQuery::DefaultBackground {
        terminator: OscTerminator::St,
    }
}

fn fg_query() -> HostQuery {
    HostQuery::DefaultForeground {
        terminator: OscTerminator::St,
    }
}

fn fg_query_bel() -> HostQuery {
    HostQuery::DefaultForeground {
        terminator: OscTerminator::Bel,
    }
}

fn palette_query(index: u8) -> HostQuery {
    HostQuery::PaletteRegister {
        index,
        terminator: OscTerminator::St,
    }
}

fn clipboard_query() -> HostQuery {
    HostQuery::ClipboardContent {
        selection: 'c',
        terminator: OscTerminator::St,
    }
}

// =====================================================================
// (CSI 2031 / DSR 997) (dark/light theme changes)
// =====================================================================

struct ThemeCapture {
    plugin_rx: Receiver<(PluginInstruction, ErrorContext)>,
    pty_writer_rx: Receiver<(PtyWriteInstruction, ErrorContext)>,
}

impl ThemeCapture {
    fn drain_plugin_updates(
        &self,
    ) -> Vec<(
        Option<yinz_types::internal_ids::PluginId>,
        Option<ClientId>,
        Event,
    )> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.plugin_rx.try_recv() {
            if let PluginInstruction::Update(updates) = instr {
                out.extend(updates);
            }
        }
        out
    }

    fn drain_plugin_events(&self) -> Vec<Event> {
        self.drain_plugin_updates()
            .into_iter()
            .map(|(_, _, event)| event)
            .collect()
    }
    fn drain_pty_writes(&self) -> Vec<(Vec<u8>, TerminalId)> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.pty_writer_rx.try_recv() {
            if let PtyWriteInstruction::Write(bytes, terminal_id, _) = instr {
                out.push((bytes, terminal_id));
            }
        }
        out
    }
}

fn create_new_screen_with_theme_capture(size: Size) -> (Screen, ThemeCapture) {
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();

    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    let fake_os_input = FakeInputOutput::default();
    bus.os_input = Some(Box::new(fake_os_input));

    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let mode_info = ModeInfo {
        session_name: Some("yinz-test".into()),
        ..Default::default()
    };
    let default_layout = Box::new(Layout::default());
    let mut screen_config = test_screen_config();
    screen_config.session_is_mirrored = true;
    let screen = Screen::new(
        bus,
        &client_attributes,
        None,
        mode_info,
        false,
        default_layout,
        screen_config,
    );
    (
        seed_first_client_size(screen, size),
        ThemeCapture {
            plugin_rx,
            pty_writer_rx,
        },
    )
}

// =====================================================================
// Pause-on-forward state machine (pane-level)
//
// These tests exercise the per-pane forward-pause flag and the
// always-buffered PTY-input queue that preserves query/reply ordering
// when an app interleaves a sync-replied query (DA1, DSR, DECQRM) with
// a host-forwarded query (OSC 10/11/4, CSI 14t/16t).
//
// Single-buffer model:
//   - `handle_pty_bytes` always appends to `pending_pty_input`.
//   - When `forward_paused` is false, processing immediately drains
//     the queue byte-by-byte until either the queue empties or Grid
//     produces a forward-bound query.
//   - Once Tab arms the pause, subsequent calls just append; the
//     queue grows and waits.
//   - On resume, Tab clears the pause and calls handle_pty_bytes
//     with an empty slice; that triggers a fresh process pass over
//     the queued bytes.
// =====================================================================

use crate::panes::TerminalPane;
use crate::tab::Pane;
use yinz_types::pane_size::PaneGeom;

fn new_terminal_pane_for_pause_test(pid: u32) -> TerminalPane {
    let mut geom = PaneGeom::default();
    geom.cols.set_inner(20);
    geom.rows.set_inner(10);
    TerminalPane::new(
        crate::TerminalId::new(pid),
        geom,
        Style::default(),
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(Some(SizeInPixels {
            width: 8,
            height: 16,
        }))),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        None,
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    )
}

/// A screen whose background-job sends can be read back, for the tests that care about what was
/// asked of the shared animation clock rather than about what was painted.
fn screen_with_background_jobs(size: Size) -> (Screen, Receiver<(BackgroundJob, ErrorContext)>) {
    let mut screen = create_non_mirrored_screen(size);
    let (to_background_jobs, background_jobs_receiver): ChannelWithContext<BackgroundJob> =
        channels::unbounded();
    screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(to_background_jobs));
    (screen, background_jobs_receiver)
}

/// Every background job sent so far, draining the channel.
fn drain_background_jobs(receiver: &Receiver<(BackgroundJob, ErrorContext)>) -> Vec<BackgroundJob> {
    receiver.try_iter().map(|(job, _)| job).collect()
}

fn create_non_mirrored_screen(size: Size) -> Screen {
    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    let fake_os_input = FakeInputOutput::default();
    bus.os_input = Some(Box::new(fake_os_input));
    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let mode_info = ModeInfo {
        session_name: Some("yinz-test".into()),
        ..Default::default()
    };
    let screen_config = test_screen_config();
    let screen = Screen::new(
        bus,
        &client_attributes,
        None,
        mode_info,
        false,
        Box::default(),
        screen_config,
    );
    seed_first_client_size(screen, size)
}

fn decode_nested_frame(bytes: &[u8]) -> Option<yinz_ipc::nested_session::NestedSessionMessage> {
    use {
        yinz_ipc::nested_session::decode_base64, yinz_ipc::nested_session::decode_payload,
        yinz_ipc::nested_session::NESTED_FRAME_HEADER,
        yinz_ipc::nested_session::NESTED_FRAME_TERMINATOR,
    };
    let encoded_payload = bytes
        .strip_prefix(NESTED_FRAME_HEADER)?
        .strip_suffix(NESTED_FRAME_TERMINATOR)?;
    decode_payload(&decode_base64(encoded_payload)?)
}

fn guest_announce_message() -> yinz_ipc::nested_session::NestedSessionMessage {
    yinz_ipc::nested_session::NestedSessionMessage::Announce {
        session_name: "guest-session".to_owned().into(),
        capabilities: vec![yinz_ipc::nested_session::NestedSessionCapability::NestedControl],
    }
}

/// Open a floating pane in `tab_index` and give it something to say. `overlay` places it
/// against the whole surface, as an overlay plugin places itself.
fn open_floating_pane(
    screen: &mut Screen,
    tab_index: TabId,
    pane_id: u32,
    contents: &str,
    overlay: bool,
) {
    let placement = if overlay {
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(2)),
                y: Some(PercentOrFixed::Fixed(2)),
                width: Some(PercentOrFixed::Percent(60)),
                height: Some(PercentOrFixed::Fixed(4)),
                ..Default::default()
            },
        )))
    } else {
        NewPanePlacement::Floating(None)
    };
    let tab = screen.tabs.get_mut(&tab_index).expect("TEST");
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(pane_id)),
        None,
        None,
        false,
        true,
        placement,
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");
    tab.handle_pty_bytes(
        crate::TerminalId::new(pane_id),
        contents.as_bytes().to_vec(),
    )
    .expect("TEST");
}

/// Like `render_frame_into`, but also reports where the frame left the terminal's cursor.
///
/// Read off the replayed grid rather than out of the render instructions, because that is the only
/// faithful way to ask: `serialize_chunks` writes an absolute goto per character chunk, so where
/// the cursor ends up is a property of the whole frame and not just of the post-VTE instructions
/// `Tab::render_cursor` adds.
/// Where the active pane thinks its cursor is, in absolute surface coordinates. What the frame
/// then does with it - place it, or leave it hidden because something is drawn over that cell - is
/// what the tests using this are about.
fn pane_cursor(screen: &mut Screen, client_id: ClientId) -> (usize, usize) {
    screen
        .get_active_tab_mut(client_id)
        .expect("TEST")
        .get_active_terminal_cursor_position(client_id)
        .map(|(x, y, _)| (x, y))
        .expect("the active pane has a cursor")
}

/// Give a directly-constructed `Screen` somewhere to render to, and hand back the other end.
fn screen_wired_to_server(
    screen: &mut Screen,
) -> channels::Receiver<(ServerInstruction, ErrorContext)> {
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    server_receiver
}

fn render_frame_and_cursor_into(
    screen: &mut Screen,
    receiver: &channels::Receiver<(ServerInstruction, ErrorContext)>,
    client_id: ClientId,
    grid: &mut Grid,
) -> (Option<(usize, usize)>, String) {
    let snapshot = render_frame_into(screen, receiver, client_id, grid);
    let cursor = grid
        .cursor_coordinates()
        .and_then(|(x, y, visible)| if visible { Some((x, y)) } else { None });
    (cursor, snapshot)
}

fn render_frame_into(
    screen: &mut Screen,
    receiver: &channels::Receiver<(ServerInstruction, ErrorContext)>,
    client_id: ClientId,
    grid: &mut Grid,
) -> String {
    screen.render_to_clients().expect("TEST");
    let mut snapshot = String::new();
    while let Ok((instruction, _)) = receiver.try_recv() {
        if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction
            && let Some(client_output) = rendered_clients.remove(&client_id)
        {
            snapshot = take_snapshot_and_cursor_coordinates(&client_output, grid).1;
        }
    }
    snapshot
}

// Regression tests for: leaving Scroll mode used to unconditionally snap the active pane's
// scroll position back to the bottom (via an unconditional `clear_active_terminal_scroll` call
// in `Screen::change_mode`), which meant switching focus away from a scrolled pane and back
// (MoveFocus always leaves Scroll mode first) lost the scroll position. The fix drops that
// unconditional reset and instead keeps the client's mode in sync with whether its focused pane
// is scrolled: focusing a scrolled pane enters Scroll mode, focusing an unscrolled one returns to
// the session's actual default mode (Normal, or Locked under unlock-first).

fn fill_pane_zero(mock_screen: &MockScreen) {
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        crate::TerminalId::new(0),
        pane_contents.as_bytes().to_vec(),
    ));
}

fn wait_for_mode_after(
    log: &Watched<Vec<ServerInstruction>>,
    checkpoint: usize,
    client_id: ClientId,
    mode: InputMode,
) {
    log.wait_until(
        "mode change after focus action",
        yinz_test_support::default_timeout(),
        |events| {
            events.iter().skip(checkpoint).any(|event| {
                matches!(event, ServerInstruction::ChangeMode(cid, observed, _) if *cid == client_id && *observed == mode)
            }).then_some(())
        },
    );
}

fn assert_focus_change_syncs_scroll_mode(default_mode: InputMode) {
    let size = Size { cols: 80, rows: 10 };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.config.options.default_mode = Some(default_mode);
    let client_id = mock_screen.main_client_id;
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    // Fill and scroll up the left pane, then return to the default mode; its scroll position
    // survives (see scroll_position_is_preserved_across_move_focus above), so the left pane is
    // left scrolled while the client is back in its default mode.
    fill_pane_zero(&mock_screen);
    wait_for_settled_render(
        &received_server_instructions,
        size,
        "pane filled",
        |frame| frame.contains("something 19"),
    );
    let _ = mock_screen.to_screen.send(ScreenInstruction::ChangeMode(
        InputMode::Scroll,
        None,
        client_id,
        None,
    ));
    let scroll_up = CliAction::ScrollUp { pane_id: None };
    for _ in 0..4 {
        send_cli_action_to_server(&session_metadata, scroll_up.clone(), client_id);
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::ChangeMode(
        default_mode,
        None,
        client_id,
        None,
    ));
    wait_for_settled_render(
        &received_server_instructions,
        size,
        "left pane scrolled",
        |frame| frame.contains("SCROLL:  4/13") && !frame.contains("something 19"),
    );
    let checkpoint = received_server_instructions.lock().len();

    // From the default mode, focus the unscrolled (right) pane: nothing to sync.
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Right,
        },
        client_id,
    );
    wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "focus on right pane",
        |cursor, _| cursor.is_some_and(|(x, _)| x > 40),
    );
    let checkpoint = received_server_instructions.lock().len();
    // Focus back to the scrolled (left) pane: the client should be switched to Scroll.
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Left,
        },
        client_id,
    );
    wait_for_mode_after(
        &received_server_instructions,
        checkpoint,
        client_id,
        InputMode::Scroll,
    );

    // The mock has no server loop to round-trip ServerInstruction::ChangeMode back into the
    // screen, so apply the mode the server would have set to advance mode_info to Scroll.
    let _ = mock_screen.to_screen.send(ScreenInstruction::ChangeMode(
        InputMode::Scroll,
        None,
        client_id,
        None,
    ));
    let checkpoint = received_server_instructions.lock().len();
    // Focus the unscrolled (right) pane: the client should return to its default mode.
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Right,
        },
        client_id,
    );
    wait_for_mode_after(
        &received_server_instructions,
        checkpoint,
        client_id,
        default_mode,
    );

    mock_screen.teardown(vec![server_thread, screen_thread]);
}

// ---------------------------------------------------------------------------
// Where an adopted pane lands
// ---------------------------------------------------------------------------
//
// A pane arriving from another session is placed by `NewPane`, which reads
// `ClientTabIndexOrPaneId::ClientId` as "wherever the client is".

/// Adopt a pane with the given placement and return the `NewPane` it asks for.
fn adopted_pane_placement_target(
    screen: &mut Screen,
    placement: yinz_types::data::WarpPlacement,
    client_id: ClientId,
) -> ClientTabIndexOrPaneId {
    let (_, _, target) = adopted_pane_new_pane(screen, placement, client_id, RunCommand::default());
    target
}

/// Adopt a pane running `cmd` with the given placement and return what the `NewPane` it
/// asks for says: the run it was invoked with, its placement, and its target.
fn adopted_pane_new_pane(
    screen: &mut Screen,
    placement: yinz_types::data::WarpPlacement,
    client_id: ClientId,
    cmd: RunCommand,
) -> (Option<Run>, NewPanePlacement, ClientTabIndexOrPaneId) {
    use yinz_types::data::WarpedPane;

    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    screen
        .place_adopted_pane(
            crate::TerminalId::new(50),
            WarpedPane::default(),
            placement,
            client_id,
            cmd,
            vec![],
            None,
            &mut None,
        )
        .expect("TEST");
    screen_receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            ScreenInstruction::NewPane(_, _, _, invoked_with, placement, _, target, _, _) => {
                Some((invoked_with, placement, target))
            },
            _ => None,
        })
        .expect("the adopted pane should have asked for a pane")
}

/// A tab where terminal 1's scrollback is being edited by terminal 9, so 1 is
/// suppressed behind the editor, plus a second tab so the session is never down
/// to its last pane.
fn screen_with_a_scrollback_editor_open() -> Screen {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .replace_pane_with_editor_pane(
            PaneId::Terminal(crate::TerminalId::new(9)),
            PaneId::Terminal(crate::TerminalId::new(1)),
        )
        .expect("TEST");
    screen
}

/// The same shape, but the stand-in is an ordinary pane opened over terminal 1
/// rather than an editor of it - a pane with its own process and its own reason
/// to exist.
fn screen_with_a_pane_replaced_in_place() -> Screen {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .expect("TEST")
        .suppress_pane_and_replace_with_pid(
            PaneId::Terminal(crate::TerminalId::new(1)),
            PaneId::Terminal(crate::TerminalId::new(9)),
            yinz_bus::pty::ReplacedPane::Suppress,
            None,
            None,
            None,
        )
        .expect("TEST");
    screen
}

fn warp_restart_fixture() -> RunCommand {
    RunCommand {
        command: "/usr/bin/printf".into(),
        args: vec!["warp test".into(), "argument with spaces".into()],
        cwd: Some("/tmp/warp workdir".into()),
        ..Default::default()
    }
}

fn released_for_warp(pane: ReleasedPane) -> yinz_bus::warp::ReleasedPaneForWarp {
    let run_command = match &pane {
        ReleasedPane::Held(held) => held.command.clone(),
        _ => warp_restart_fixture(),
    };
    yinz_bus::warp::ReleasedPaneForWarp { pane, run_command }
}

/// The same, for a pane whose process is on another machine.
///
/// What a remote release hands over is a broker and a token rather than a
/// socket, and `received` is the byte count the replay has to resume from —
/// nonzero here so a path that dropped it could not pass by accident.
fn released_remote_pane_for_warp(
    screen: &mut Screen,
    terminal_id: yinz_types::internal_ids::TerminalId,
) -> Box<WarpedPane> {
    screen
        .take_released_pane_for_warp(
            terminal_id,
            Ok(released_for_warp(ReleasedPane::Remote(
                ReleasedRemotePane {
                    host: "build-box".to_owned(),
                    pane_token: format!("token-{terminal_id}"),
                    received: 4096,
                    cmd: RunCommand {
                        command: PathBuf::from("/usr/bin/tail"),
                        args: vec!["-f".to_owned(), "/var/log/build".to_owned()],
                        target: yinz_types::command::PaneTarget::Ssh {
                            host: "build-box".to_owned(),
                        },
                        ..Default::default()
                    },
                },
            ))),
        )
        .expect("the pane should have been taken out of its tab")
}

// ---------------------------------------------------------------------------
// Warping a tab that holds a stack list
// ---------------------------------------------------------------------------
//
// A stack list keeps all but its visible member in `suppressed_panes`, the same
// place a scrollback editor parks the pane it is editing. Refusing every tab
// with a hidden terminal in it therefore refused every tab with a stack in it,
// which is most of them once the mode is on. The members are ordinary panes with
// ordinary processes and they already have a place in the layout the tab travels
// as - so what has to be told apart is not "hidden" but *how* it is hidden.

/// A session whose first tab holds a three-member stack list, plus a second tab
/// so the session is never down to its last pane.
///
/// Terminal 1 is the tab's original pane; 2 and 3 are stacked onto it, leaving 3
/// visible and 1 and 2 parked in `suppressed_panes` under their own ids.
fn screen_with_a_stack_list() -> Screen {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    // terminal 9, so that a member left behind in this tab cannot be mistaken
    // for a pane that was always here
    new_tab(&mut screen, 9, TabId::new(1));
    *screen.stacked_pane_list.borrow_mut() = true;
    let tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    for pane_id in [
        PaneId::Terminal(crate::TerminalId::new(2)),
        PaneId::Terminal(crate::TerminalId::new(3)),
    ] {
        tab.new_pane(
            pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: None,
                borderless: None,
            },
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
    }
    assert!(
        tab.has_stack_lists(),
        "the fixture should have formed a stack list",
    );
    screen
}

/// A stack list whose member 3 has a scrollback editor (terminal 8) open on it,
/// with the list then focused elsewhere - so the editor is the member, and it is
/// hidden, and it is standing in for pane 3 all at once.
fn screen_with_an_editor_on_a_hidden_stack_list_member() -> Screen {
    let mut screen = screen_with_a_stack_list();
    let tab = screen.tabs.get_mut(&TabId::new(0)).expect("TEST");
    tab.replace_pane_with_editor_pane(
        PaneId::Terminal(crate::TerminalId::new(8)),
        PaneId::Terminal(crate::TerminalId::new(3)),
    )
    .expect("TEST");
    tab.focus_hidden_stack_list_member(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );
    screen
}

// ---------------------------------------------------------------------------
// Moving panes between tabs when tab ids and tab positions have diverged
//
// A tab has a stable `id` (the `Screen::tabs` key) and a display `position` that
// recompacts every time a tab closes. In a session that has never closed a tab the two
// are equal, which is why a function that confuses them can look correct for a long
// time. These tests build the state a real session reaches the moment any tab closes.
// ---------------------------------------------------------------------------

/// Four tabs, the first two closed: ids {2, 3} at positions {0, 1}.
///
/// Each surviving tab holds one pane, with a pid that identifies it. Panes 1 and 2 died
/// with tabs 0 and 1.
fn screen_with_diverged_tab_ids() -> Screen {
    let mut screen = create_new_screen(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
        true,
    );
    for tab_id in 0..4 {
        new_tab(&mut screen, tab_id as u32 + 1, TabId::new(tab_id));
    }
    screen.close_tab_by_id(TabId::new(0)).expect("TEST");
    screen.close_tab_by_id(TabId::new(1)).expect("TEST");

    assert_eq!(
        screen.tabs.get(&TabId::new(2)).map(|t| t.position),
        Some(TabPosition::new(0)),
        "Pre-condition: tab id 2 sits at position 0"
    );
    assert_eq!(
        screen.tabs.get(&TabId::new(3)).map(|t| t.position),
        Some(TabPosition::new(1)),
        "Pre-condition: tab id 3 sits at position 1"
    );
    screen
}

/// Adds a pane to a tab by id, so a test does not depend on which tab is focused.
fn add_pane_to_tab(screen: &mut Screen, tab_id: TabId, pane_id: PaneId, floating: bool) {
    let placement = if floating {
        NewPanePlacement::Floating(None)
    } else {
        NewPanePlacement::default()
    };
    screen
        .tabs
        .get_mut(&tab_id)
        .expect("TEST")
        .new_pane(
            pane_id,
            None,
            None,
            false,
            false,
            placement,
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");
}

#[test]
fn notification_history_keeps_dismissed_toasts() {
    let mut mock_screen = MockScreen::new(Size { cols: 80, rows: 24 });
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    mock_screen
        .to_screen
        .send(ScreenInstruction::Notify(
            "first".to_owned(),
            None,
            None,
            None,
        ))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::Notify(
            "second".to_owned(),
            None,
            None,
            None,
        ))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::DismissNotification(NotificationId::new(
            0,
        )))
        .unwrap();

    let (response_tx, response_rx) = crossbeam::channel::bounded(1);
    mock_screen
        .to_screen
        .send(ScreenInstruction::ListNotifications {
            response_channel: response_tx,
        })
        .unwrap();
    let history = response_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("screen returns notification history");
    assert_eq!(
        history
            .iter()
            .map(|notification| (notification.id, notification.message.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (NotificationId::new(0), "first"),
            (NotificationId::new(1), "second")
        ]
    );
    assert!(history.iter().all(
        |notification| notification.received_at_unix_ms > yinz_types::units::UnixMillis::new(0)
    ));

    mock_screen.teardown(vec![screen_thread]);
}

mod agent_control;
mod agent_placement;
mod animation_clock;
mod background_permissions;
mod break_and_move_panes;
mod break_panes_to_tab;
mod capabilities;
mod cli_actions;
mod cli_layouts_and_rename;
mod cli_pane_and_tab_id;
mod clients;
#[cfg(unix)]
mod clipboard;
mod command_palette;
mod command_respawn;
mod context_menu;
mod desktop_notifications;
mod floating_panes;
mod hooks;
mod host_focus;
mod host_query;
mod key_intercepts;
mod layout_watcher;
mod lent_slots;
mod mouse;
mod nested_sessions;
mod new_pane_placement;
mod notification_activation;
mod notification_animation;
mod notification_browser;
mod notification_policy;
mod osc7;
mod overlays;
mod override_layout;
mod pane_animations;
mod pane_state;
mod paneless_plugins;
mod plugin_broadcasts;
mod plugin_launch;
mod quit;
mod reconfigure;
mod render_pacing;
mod reveal_pane;
mod same_worktree;
mod scroll_and_warp;
mod sizing;
mod subscribers;
mod tab_groups;
mod tab_projects;
mod tabs;
mod theme;
mod waiting_pulse;
