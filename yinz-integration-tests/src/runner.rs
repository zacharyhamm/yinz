use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::ClientScreen;
use yinz_cli::setup::Setup;
use yinz_client::os_input_output::SignalEvent;
use yinz_client::ClientInfo;
use yinz_types::input::actions::Action;
use yinz_types::input::options::Options;
use yinz_types::pane_size::Size;
use {yinz_cli::args::CliAction, yinz_cli::args::CliArgs};
use {yinz_types::data::LayoutInfo, yinz_types::data::SessionConnection};

use crate::client_screen::GridSnapshot;
use crate::fake_client_os_api::{FakeClientHandle, FakeClientOsApi};
use crate::fake_pty::FakePtyHandle;
use crate::fake_server_os_api::FakeServerOsApi;
use crate::{keys, test_env};

const GUEST_MODAL_TITLE: &str = "Nested Yinz session detected";

pub struct TestRunner {
    size: Size,
    extra_config_kdl: String,
    config_dir: Option<PathBuf>,
    layout: Option<LayoutInfo>,
    env: std::collections::HashMap<String, String>,
    stdout_tap: Option<crossbeam::channel::Sender<Vec<u8>>>,
    skip_concurrency_slot: bool,
}

impl TestRunner {
    pub fn new(size: Size) -> Self {
        TestRunner {
            size,
            extra_config_kdl: String::new(),
            config_dir: None,
            layout: None,
            env: std::collections::HashMap::new(),
            stdout_tap: None,
            skip_concurrency_slot: false,
        }
    }

    /// The config directory the session is told about, where `config.d/` and `plugins/` are
    /// looked for. Tests that persist configuration point this at a folder of their own.
    pub fn with_config_dir(mut self, config_dir: PathBuf) -> Self {
        self.config_dir = Some(config_dir);
        self
    }

    pub fn with_config(mut self, extra_config_kdl: &str) -> Self {
        self.extra_config_kdl.push('\n');
        self.extra_config_kdl.push_str(extra_config_kdl);
        self
    }

    pub fn with_layout(mut self, layout: LayoutInfo) -> Self {
        self.layout = Some(layout);
        self
    }

    pub fn with_stdout_tap(mut self, sender: crossbeam::channel::Sender<Vec<u8>>) -> Self {
        self.stdout_tap = Some(sender);
        self
    }

    pub fn skip_concurrency_slot(mut self) -> Self {
        self.skip_concurrency_slot = true;
        self
    }

    pub fn start(self) -> TestSession {
        test_env::init();
        let session_name = test_env::unique_session_name();
        let config_path = test_env::write_config(&session_name, &self.extra_config_kdl);
        let data_dir = test_env::init().join("data");

        let cli_args = CliArgs {
            config: Some(config_path),
            config_dir: self.config_dir.clone(),
            data_dir: Some(data_dir),
            ..Default::default()
        };
        let (config, default_layout_info, config_options, _, _) =
            Setup::from_cli_args(&cli_args).expect("failed to load harness config");

        let concurrency_slot = if self.skip_concurrency_slot {
            None
        } else {
            test_env::acquire_concurrency_slot()
        };

        let fake_server_os_api = FakeServerOsApi::default();
        let server_thread: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
        let server_spawner = in_process_server_spawner(fake_server_os_api.clone(), &server_thread);

        let (fake_client_os_api, fake_client_handle) =
            FakeClientOsApi::new_with_env(self.size, Some(server_spawner), self.env);
        if let Some(stdout_tap) = self.stdout_tap {
            fake_client_handle.client_screen.set_stdout_tap(stdout_tap);
        }
        let layout_info = self.layout.or(default_layout_info);
        let client_thread = spawn_client_thread(
            fake_client_os_api,
            cli_args.clone(),
            config.clone(),
            config_options.clone(),
            ClientInfo::New(session_name.clone().into(), layout_info, None),
            None,
        );

        TestSession {
            session_name,
            size: self.size,
            cli_args,
            config,
            config_options,
            fake_server_os_api,
            server_thread,
            main_client: TestClient {
                fake_client_handle,
                thread: Some(client_thread),
            },
            _concurrency_slot: concurrency_slot,
        }
    }
}

fn in_process_server_spawner(
    fake_server_os_api: FakeServerOsApi,
    server_thread: &Arc<Mutex<Option<JoinHandle<()>>>>,
) -> Box<dyn FnOnce(std::path::PathBuf) + Send> {
    let server_thread = server_thread.clone();
    Box::new(move |socket_path: std::path::PathBuf| {
        let install_panic_hook = false;
        let join_handle = std::thread::Builder::new()
            .name("in_process_yinz_server".to_string())
            .spawn(move || {
                yinz_server::start_server_impl(
                    Box::new(fake_server_os_api),
                    socket_path,
                    install_panic_hook,
                );
            })
            .unwrap();
        *server_thread.lock().unwrap() = Some(join_handle);
    })
}

fn new_pane_cli_action(
    command: &[&str],
    floating: bool,
    close_on_exit: bool,
    start_suspended: bool,
    block_until_exit: bool,
) -> CliAction {
    CliAction::NewPane {
        direction: None,
        command: command.iter().map(|part| part.to_string()).collect(),
        plugin: None,
        cwd: None,
        floating,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit,
        start_suspended,
        // Local. A pane on another machine needs a real ssh and a real machine
        // to be on, neither of which this in-process harness has — that is what
        // `yinz-server/tests/remote_pane_ssh.rs` is for.
        remote_host: None,

        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    }
}

fn referenced_contents_files(layout: &str) -> impl Iterator<Item = &str> {
    layout
        .match_indices("contents_file=\"")
        .filter_map(|(start, marker)| {
            let rest = &layout[start + marker.len()..];
            rest.find('"').map(|end| &rest[..end])
        })
}

fn spawn_client_thread(
    fake_client_os_api: FakeClientOsApi,
    cli_args: CliArgs,
    config: yinz_config::config::Config,
    config_options: Options,
    client_info: ClientInfo,
    pane_id_to_focus: Option<yinz_types::data::PaneId>,
) -> JoinHandle<yinz_client::ClientOutcome> {
    std::thread::Builder::new()
        .name("in_process_yinz_client".to_string())
        .spawn(move || {
            let tab_number_to_focus = None;
            let is_a_reconnect = false;
            let start_detached_and_exit = false;
            yinz_client::start_client(
                Box::new(fake_client_os_api),
                cli_args,
                config,
                config_options,
                client_info,
                tab_number_to_focus,
                pane_id_to_focus,
                is_a_reconnect,
                start_detached_and_exit,
            )
        })
        .unwrap()
}

pub struct TestClient {
    fake_client_handle: FakeClientHandle,
    thread: Option<JoinHandle<yinz_client::ClientOutcome>>,
}

#[derive(Clone)]
pub struct GuestResizer {
    size: Arc<Mutex<Size>>,
    signal_tx: crossbeam::channel::Sender<SignalEvent>,
}

impl GuestResizer {
    pub fn resize(&self, new_size: Size) -> bool {
        *self.size.lock().unwrap() = new_size;
        self.signal_tx.send(SignalEvent::Resize).is_ok()
    }
}

impl TestClient {
    pub fn send_stdin(&self, bytes: &[u8]) {
        self.fake_client_handle
            .stdin_tx
            .send(bytes.to_vec())
            .expect("client stdin closed");
    }

    pub fn resize(&self, new_size: Size) {
        *self.fake_client_handle.size.lock().unwrap() = new_size;
        let _ = self.fake_client_handle.signal_tx.send(SignalEvent::Resize);
    }

    pub fn wait_until(
        &self,
        what: &str,
        predicate: impl Fn(&GridSnapshot) -> bool,
    ) -> GridSnapshot {
        self.fake_client_handle
            .client_screen
            .wait_until(what, predicate)
    }

    pub fn wait_until_raw_output(&self, what: &str, predicate: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        self.fake_client_handle
            .client_screen
            .wait_until_raw_output(what, predicate)
    }

    pub fn snapshot(&self) -> GridSnapshot {
        self.fake_client_handle.client_screen.snapshot()
    }

    pub fn raw_bytes(&self) -> Vec<u8> {
        self.fake_client_handle.client_screen.raw_bytes()
    }

    pub fn received_server_messages(&self) -> Vec<String> {
        self.fake_client_handle.received_server_messages()
    }

    pub fn quit(mut self) {
        if self
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
        {
            self.fake_client_handle.quit_client();
        }
        self.join();
    }

    fn join(&mut self) -> Option<SessionConnection> {
        self.thread.take().and_then(join_client_thread_with_timeout)
    }
}

fn join_client_thread_with_timeout(
    join_handle: JoinHandle<yinz_client::ClientOutcome>,
) -> Option<SessionConnection> {
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("nested_client_join_watchdog".to_string())
        .spawn(move || {
            let result = join_handle.join();
            let _ = done_tx.send(result);
        })
        .unwrap();
    match done_rx.recv_timeout(CLIENT_JOIN_TIMEOUT) {
        Ok(Ok(outcome)) => {
            assert_eq!(outcome.exit_status, 0, "{}", outcome.exit_msg);
            outcome.reconnect_to_session
        },
        Ok(Err(_)) => panic!("client thread panicked"),
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("timed out waiting for client thread"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("client join watchdog disconnected")
        },
    }
}

fn join_server_thread_with_timeout(join_handle: JoinHandle<()>) {
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("nested_server_join_watchdog".to_string())
        .spawn(move || {
            let result = join_handle.join();
            let _ = done_tx.send(result);
        })
        .unwrap();
    match done_rx.recv_timeout(CLIENT_JOIN_TIMEOUT) {
        Ok(Ok(())) => {},
        Ok(Err(_)) => panic!("server thread panicked"),
        Err(mpsc::RecvTimeoutError::Timeout) => panic!(
            "timed out waiting for server thread\n{}",
            crate::test_env::log_tail(80)
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("server join watchdog disconnected")
        },
    }
}

const CLIENT_JOIN_TIMEOUT: Duration = Duration::from_secs(10);

pub struct TestSession {
    session_name: String,
    size: Size,
    cli_args: CliArgs,
    config: yinz_config::config::Config,
    config_options: Options,
    fake_server_os_api: FakeServerOsApi,
    server_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
    main_client: TestClient,
    _concurrency_slot: Option<test_env::ConcurrencySlot>,
}

pub struct CliClientHandle {
    thread: JoinHandle<i32>,
}

/// A `yinz events` client on this session. What it was sent is read off the fake client's
/// message trail, where an `EventUpdate` is recorded with its event type and its JSON.
pub struct EventsClientHandle {
    // Kept so the client thread is owned by something; it ends with the process, since the
    // harness never ends a session.
    _thread: JoinHandle<()>,
    client: FakeClientHandle,
}

impl EventsClientHandle {
    /// Every event type streamed so far, in order.
    pub fn streamed_event_types(&self) -> Vec<String> {
        self.streamed_events()
            .into_iter()
            .map(|(event_type, _)| event_type)
            .collect()
    }

    /// Every event streamed so far, in order, as its type and the JSON it was sent as - what a
    /// real `yinz events --json` prints, one event per line.
    pub fn streamed_events(&self) -> Vec<(String, String)> {
        self.client
            .received_server_messages()
            .iter()
            .filter_map(|message| message.strip_prefix("EventUpdate: "))
            .map(|event| match event.split_once(' ') {
                Some((event_type, event_json)) => (event_type.to_owned(), event_json.to_owned()),
                None => (event.to_owned(), String::new()),
            })
            .collect()
    }

    /// Block until an event of `event_type` has been streamed.
    pub fn wait_for_event(&self, event_type: &str) {
        self.wait_for_event_matching(event_type, |_| true);
    }

    /// Wait for a report describing the expected state, rather than merely its event type.
    pub fn wait_for_event_matching(
        &self,
        event_type: &str,
        predicate: impl Fn(&str) -> bool,
    ) -> String {
        let deadline = std::time::Instant::now() + crate::default_timeout();
        loop {
            let events = self.streamed_events();
            if let Some((_, event_json)) = events
                .iter()
                .find(|(kind, json)| kind == event_type && predicate(json))
            {
                return event_json.clone();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for a matching {event_type} event; streamed: {events:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl CliClientHandle {
    pub fn wait_for_exit(self) -> i32 {
        self.thread.join().expect("cli client thread panicked")
    }

    /// Whether the client has already returned. A test that asserts a client is *still*
    /// waiting asks this after observing the thing that must not have released it.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }
}

impl TestSession {
    /// Hold the PTY thread at its next metadata read until the test releases it.
    pub fn hold_next_metadata_collection(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        self.fake_server_os_api.hold_next_metadata_collection()
    }

    pub fn run_cli_action_async(&self, action: CliAction) -> CliClientHandle {
        CliClientHandle {
            thread: self.spawn_cli_client(action).0,
        }
    }
    pub fn session_name(&self) -> &str {
        &self.session_name
    }

    pub fn expect_pty_spawn(&self) -> FakePtyHandle {
        let terminal_id = self
            .fake_server_os_api
            .shared_ptys
            .wait_for("a pty spawn", |fake_pty_registry| {
                fake_pty_registry.spawn_queue.pop_front()
            });
        FakePtyHandle {
            terminal_id,
            shared_ptys: self.fake_server_os_api.shared_ptys.clone(),
        }
    }

    /// How many panes this session let go of, and took up, as *remote* panes.
    ///
    /// The only way a test can tell which road a warp took. The pane's state
    /// travels either way in this harness, so everything else about the pane
    /// that arrives reads the same whether it went by a supervisor socket or by
    /// a broker token — and a remote pane sent down the local road would be a
    /// silent divergence from the real backend, where the two are not
    /// interchangeable at all.
    pub fn remote_warp_counts(&self) -> (usize, usize) {
        self.fake_server_os_api.shared_ptys.remote_warp_counts()
    }

    /// The session names whose orphaned pane supervisors the server asked to
    /// have swept on its way out — one entry per `KillSession` exit, none for
    /// any other exit. `quit` joins the server thread, so the list read after it
    /// is final.
    pub fn set_orphan_sweep_error(&self, error: String) {
        self.fake_server_os_api.set_orphan_sweep_error(error);
    }

    pub fn orphan_sweeps(&self) -> Vec<String> {
        self.fake_server_os_api.orphan_sweeps()
    }

    /// A handle on the next pane this session took over from another one.
    ///
    /// Deliberately not `expect_pty_spawn`: nothing was spawned. The handle
    /// carries this session's terminal id, which is new, but everything it
    /// reaches — the stdin the pane has been sent, the sizes it has been told
    /// about, whether it ever exited — is the same fake process the other
    /// session had.
    pub fn expect_adopted_pty(&self) -> FakePtyHandle {
        let terminal_id = self.fake_server_os_api.shared_ptys.expect_adoption();
        FakePtyHandle {
            terminal_id,
            shared_ptys: self.fake_server_os_api.shared_ptys.clone(),
        }
    }

    pub fn send_stdin(&self, bytes: &[u8]) {
        self.main_client.send_stdin(bytes);
    }

    pub fn stdin_sender(&self) -> crossbeam::channel::Sender<Vec<u8>> {
        self.main_client.fake_client_handle.stdin_tx.clone()
    }

    pub fn resize(&self, new_size: Size) {
        self.main_client.resize(new_size);
    }

    pub fn resize_sender(&self) -> GuestResizer {
        GuestResizer {
            size: self.main_client.fake_client_handle.size.clone(),
            signal_tx: self.main_client.fake_client_handle.signal_tx.clone(),
        }
    }

    pub fn wait_until(
        &self,
        what: &str,
        predicate: impl Fn(&GridSnapshot) -> bool,
    ) -> GridSnapshot {
        self.main_client.wait_until(what, predicate)
    }

    pub fn wait_until_raw_output(&self, what: &str, predicate: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        self.main_client.wait_until_raw_output(what, predicate)
    }

    pub fn snapshot(&self) -> GridSnapshot {
        self.main_client.snapshot()
    }

    pub fn raw_bytes(&self) -> Vec<u8> {
        self.main_client.raw_bytes()
    }

    pub fn received_server_messages(&self) -> Vec<String> {
        self.main_client.received_server_messages()
    }

    pub fn wait_for_app_load(&self) -> GridSnapshot {
        self.main_client.wait_until("app to load", |grid_snapshot| {
            (grid_snapshot.status_bar_appears() || grid_snapshot.contains("Descend:"))
                && grid_snapshot.tab_bar_appears()
                && (grid_snapshot.cursor.is_some() || grid_snapshot.contains(GUEST_MODAL_TITLE))
        })
    }

    pub fn written_files(&self) -> std::collections::HashMap<String, String> {
        self.fake_server_os_api.written_files()
    }

    pub fn take_session_switch(&mut self) -> SessionConnection {
        self.main_client
            .join()
            .expect("client did not request a session switch")
    }

    pub fn attach_client(&self, size: Size) -> TestClient {
        self.attach_client_with_focus(size, None)
    }

    pub fn attach_client_with_focus(
        &self,
        size: Size,
        pane: Option<yinz_types::data::PaneId>,
    ) -> TestClient {
        let (fake_client_os_api, fake_client_handle) = FakeClientOsApi::new(size, None);
        let thread = spawn_client_thread(
            fake_client_os_api,
            self.cli_args.clone(),
            self.config.clone(),
            self.config_options.clone(),
            ClientInfo::Attach(
                self.session_name.clone().into(),
                self.config_options.clone(),
            ),
            pane,
        );
        TestClient {
            fake_client_handle,
            thread: Some(thread),
        }
    }

    pub fn attach_watcher(&self, size: Size) -> TestClient {
        let (fake_client_os_api, fake_client_handle) = FakeClientOsApi::new(size, None);
        let thread = spawn_client_thread(
            fake_client_os_api,
            self.cli_args.clone(),
            self.config.clone(),
            self.config_options.clone(),
            ClientInfo::Watch(
                self.session_name.clone().into(),
                self.config_options.clone(),
            ),
            None,
        );
        TestClient {
            fake_client_handle,
            thread: Some(thread),
        }
    }

    pub fn run_cli_action(&self, cli_action: CliAction) -> i32 {
        self.run_cli_action_reporting(cli_action).0
    }

    /// Run a cli action and return its exit code together with what it printed.
    ///
    /// Some actions answer in prose rather than in state — a warp that is refused
    /// says why, and the wording is the whole of what the user gets — so a test
    /// that only saw the exit code could not tell one refusal from another.
    pub fn run_cli_action_reporting(&self, cli_action: CliAction) -> (i32, String) {
        self.run_cli_action_reporting_with_stdin(cli_action, None)
    }

    /// `Some` supplies nonterminal stdin, including an empty stream that immediately reaches EOF.
    pub fn run_cli_action_reporting_with_stdin(
        &self,
        cli_action: CliAction,
        stdin: Option<&str>,
    ) -> (i32, String) {
        let (handle, client_screen) = self.spawn_cli_client_with_stdin(cli_action, stdin);
        let exit_code = handle.join().expect("cli client thread panicked");
        let reported = client_screen
            .server_messages()
            .lock()
            .unwrap()
            .iter()
            .filter_map(|message| message.split_once(": "))
            .filter_map(|(name, output)| match name {
                "Log" | "LogError" => Some(format!("{output}\n")),
                "CliPipeOutput" => Some(output.to_owned()),
                _ => None,
            })
            .collect::<String>();
        (exit_code, reported)
    }

    /// Run actions already built - what a CLI action turns into after it has read its own
    /// inputs, for the one CLI action a test cannot drive through `CliAction`: `agent-report
    /// --hook-json` reads the event off stdin, which an in-process client has no way to fake.
    pub fn run_actions_reporting(&self, actions: Vec<Action>) -> (i32, String) {
        let (handle, client_screen) = self.spawn_cli_client_with_actions(actions, None);
        let exit_code = handle.join().expect("cli client thread panicked");
        let reported = client_screen
            .server_messages()
            .lock()
            .unwrap()
            .iter()
            .filter_map(|message| message.split_once(": "))
            .filter_map(|(name, output)| match name {
                "Log" | "LogError" => Some(format!("{output}\n")),
                "CliPipeOutput" => Some(output.to_owned()),
                _ => None,
            })
            .collect::<String>();
        (exit_code, reported)
    }

    fn spawn_cli_client(&self, cli_action: CliAction) -> (JoinHandle<i32>, ClientScreen) {
        self.spawn_cli_client_with_stdin(cli_action, None)
    }

    fn spawn_cli_client_with_stdin(
        &self,
        cli_action: CliAction,
        stdin: Option<&str>,
    ) -> (JoinHandle<i32>, ClientScreen) {
        let actions = yinz_cli::actions::actions_from_cli(
            cli_action,
            Box::new(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))),
            Some(self.config.clone()),
        )
        .expect("failed to build cli actions");
        self.spawn_cli_client_with_actions(actions, stdin)
    }

    fn spawn_cli_client_with_actions(
        &self,
        actions: Vec<Action>,
        stdin: Option<&str>,
    ) -> (JoinHandle<i32>, ClientScreen) {
        let (fake_client_os_api, fake_client_handle) = FakeClientOsApi::new(self.size, None);
        let fake_client_os_api = fake_client_os_api.with_cli_stdin(stdin);
        let client_screen = fake_client_handle.client_screen.clone();
        let session_name = self.session_name.clone();
        let handle = std::thread::Builder::new()
            .name("in_process_cli_client".to_string())
            .spawn(move || {
                yinz_client::cli_client::start_cli_client(
                    Box::new(fake_client_os_api),
                    &yinz_types::internal_ids::SessionName::new(&session_name),
                    actions,
                    None,
                )
            })
            .unwrap();
        (handle, client_screen)
    }

    pub fn save_session(&self) {
        self.run_cli_action(CliAction::SaveSession);
    }

    pub fn wait_for_serialized_session(&self) {
        let session_dir = yinz_runtime::consts::session_info_folder_for_session(
            &yinz_types::internal_ids::SessionName::new(&self.session_name),
        );
        let deadline = std::time::Instant::now() + crate::default_timeout();
        loop {
            if let Ok(checkpoint) =
                yinz_runtime::session_checkpoint::SessionCheckpoint::read(&session_dir)
            {
                let layout_path = checkpoint.layout_path();
                if let Ok(layout) = std::fs::read_to_string(&layout_path) {
                    if referenced_contents_files(&layout)
                        .all(|name| layout_path.parent().unwrap().join(name).exists())
                    {
                        return;
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                panic!(
                    "timed out waiting for session serialization in {}",
                    session_dir.display()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    pub fn resurrect(&mut self, size: Size) {
        let layout_path = yinz_runtime::consts::session_layout_cache_file_name(
            &yinz_types::internal_ids::SessionName::new(&self.session_name),
        );
        {
            let checkpoint = yinz_runtime::session_checkpoint::SessionCheckpoint::read(
                layout_path.parent().unwrap(),
            )
            .unwrap();
            assert!(
                checkpoint.layout_path().exists(),
                "no serialized layout to resurrect from at {}",
                layout_path.display()
            );
        }
        let mut cli_args = self.cli_args.clone();
        if cli_args.config_dir.is_none() {
            cli_args.config_dir = cli_args
                .config
                .as_ref()
                .and_then(|config| config.parent().map(|parent| parent.to_path_buf()));
        }
        let fake_server_os_api = FakeServerOsApi::default();
        let server_thread: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
        let server_spawner = in_process_server_spawner(fake_server_os_api.clone(), &server_thread);
        let (fake_client_os_api, fake_client_handle) =
            FakeClientOsApi::new(size, Some(server_spawner));
        let client_thread = spawn_client_thread(
            fake_client_os_api,
            cli_args,
            self.config.clone(),
            self.config_options.clone(),
            ClientInfo::Resurrect(self.session_name.clone().into(), layout_path, false, None),
            None,
        );
        self.fake_server_os_api = fake_server_os_api;
        self.server_thread = server_thread;
        self.main_client = TestClient {
            fake_client_handle,
            thread: Some(client_thread),
        };
    }

    pub fn override_layout(&self, layout_name: &str) {
        self.run_cli_action(CliAction::OverrideLayout {
            layout: Some(PathBuf::from(layout_name)),
            layout_string: None,
            layout_dir: None,
            retain_existing_terminal_panes: false,
            retain_existing_plugin_panes: false,
            apply_only_to_active_tab: false,
        });
    }

    pub fn run_suspended_command(&self, command: &[&str]) {
        let start_suspended = true;
        self.run_cli_action(new_pane_cli_action(
            command,
            false,
            false,
            start_suspended,
            false,
        ));
    }

    pub fn run_blocking_command(&self, command: &[&str]) -> CliClientHandle {
        let floating = false;
        let close_on_exit = true;
        let block_until_exit = true;
        CliClientHandle {
            thread: self
                .spawn_cli_client(new_pane_cli_action(
                    command,
                    floating,
                    close_on_exit,
                    false,
                    block_until_exit,
                ))
                .0,
        }
    }

    /// `yinz run --block-until-exit -- <command>` as the CLI spells it, with no
    /// `--close-on-exit`: the pane holds when the command exits, so the exit reaches the hooks
    /// (`pane_exit`, and `agent_exit` for an agent pane), while the client still returns with
    /// the command's status. `run_blocking_command` is the same with the pane closing.
    pub fn run_blocking_command_holding_on_exit(&self, command: &[&str]) -> CliClientHandle {
        let floating = false;
        let close_on_exit = false;
        let block_until_exit = true;
        CliClientHandle {
            thread: self
                .spawn_cli_client(new_pane_cli_action(
                    command,
                    floating,
                    close_on_exit,
                    false,
                    block_until_exit,
                ))
                .0,
        }
    }

    /// `yinz run --blocking`: waits for the pane's command to exit, with no condition on the
    /// status. Distinct from `run_blocking_command`, whose `--block-until-exit` sets one.
    pub fn run_plain_blocking_command(&self, command: &[&str]) -> CliClientHandle {
        let mut action = new_pane_cli_action(command, false, true, false, false);
        if let CliAction::NewPane { blocking, .. } = &mut action {
            *blocking = true;
        }
        CliClientHandle {
            thread: self.spawn_cli_client(action).0,
        }
    }

    /// `yinz events [--type ...]`, streaming until the session ends.
    pub fn stream_events(&self, event_types: &[&str]) -> EventsClientHandle {
        let (fake_client_os_api, client) = FakeClientOsApi::new(self.size, None);
        let session_name = self.session_name.clone();
        let events_cli = yinz_cli::args::EventsCli {
            filters: Default::default(),
            text: false,
            event_type: event_types.iter().map(|t| t.to_string()).collect(),
        };
        let thread = std::thread::Builder::new()
            .name("in_process_events_client".to_string())
            .spawn(move || {
                yinz_client::cli_client::start_events_client(
                    Box::new(fake_client_os_api),
                    &yinz_types::internal_ids::SessionName::new(&session_name),
                    events_cli,
                )
            })
            .unwrap();
        EventsClientHandle {
            _thread: thread,
            client,
        }
    }

    pub fn run_blocking_floating_command(&self, command: &[&str]) -> CliClientHandle {
        let floating = true;
        let close_on_exit = true;
        let block_until_exit = true;
        CliClientHandle {
            thread: self
                .spawn_cli_client(new_pane_cli_action(
                    command,
                    floating,
                    close_on_exit,
                    false,
                    block_until_exit,
                ))
                .0,
        }
    }

    pub fn detach_main_client(&mut self) {
        let connected_clients_before_detach = self.fake_server_os_api.connected_client_count();
        self.send_stdin(&keys::ctrl('o'));
        self.send_stdin(&keys::key('d'));
        self.main_client.join();
        self.wait_for_server_to_release_a_client(connected_clients_before_detach);
    }

    fn wait_for_server_to_release_a_client(&self, connected_clients_before_detach: usize) {
        let deadline = std::time::Instant::now() + crate::default_timeout();
        loop {
            if self.fake_server_os_api.connected_client_count() < connected_clients_before_detach {
                return;
            }
            if std::time::Instant::now() >= deadline {
                panic!("timed out waiting for server to release the detached client");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    pub fn quit(&mut self) {
        if self.main_client.thread.is_some() {
            self.main_client.fake_client_handle.kill_session();
        } else {
            yinz_session::sessions::kill_session(&yinz_types::internal_ids::SessionName::new(
                &self.session_name,
            ))
            .expect("failed to deliver the integration-test server shutdown request");
        }
        self.wait_for_exit();
    }

    /// Join a session that is already exiting, for example after its final pane closes.
    pub fn wait_for_exit(&mut self) {
        self.main_client.join();
        if let Some(server_thread) = self.server_thread.lock().unwrap().take() {
            join_server_thread_with_timeout(server_thread);
        }
        test_env::assert_no_background_panics();
    }

    fn send_quit_without_joining(&self) {
        let _ = self
            .main_client
            .fake_client_handle
            .signal_tx
            .send(SignalEvent::Quit);
    }
}

impl Drop for TestSession {
    fn drop(&mut self) {
        if self.main_client.thread.is_some() {
            self.send_quit_without_joining();
        }
    }
}

pub fn normalized(grid_snapshot: &GridSnapshot) -> String {
    let text = replace_unique_session_name(&grid_snapshot.text);
    let text = strip_swap_layout_indication(&text);
    let text = strip_tip_indication(&text);
    let text = strip_status_bar_clock(&text);
    strip_trailing_whitespace(&text)
}

pub fn assert_same_rendered_grid(actual: &GridSnapshot, expected: &GridSnapshot, what: &str) {
    let actual = normalized(actual);
    let expected = normalized(expected);
    assert!(
        actual == expected,
        "{what}\n--- expected grid ---\n{expected}\n--- actual grid ---\n{actual}"
    );
}

/// The status bar's clock, which is the real wall clock and so different on every run.
///
/// Stripped rather than pinned because the alternative is a fake clock reaching all the way
/// into a wasm plugin's `SystemTime::now()`, and what these snapshots are about is the layout
/// around it. The trailing whitespace it leaves behind is taken off by
/// [`strip_trailing_whitespace`], which runs after this.
///
/// What tells the clock apart from a pane line that merely ends in `12:34` is that the bar
/// draws it *flush right*: `render_secondary_info` fits the block into `max_len - 1`, so
/// exactly one background column follows it, and then the edge of whatever the bar is drawn
/// in. That edge is the end of the line for a session's own bar, and the pane border for a
/// nested session's, which is drawn inside a pane and so partway up the grid - the border is
/// put back, along with the rest of the host's row beyond it. Rows reach here padded to the
/// full terminal width (`render_bytes` writes every cell, and [`strip_trailing_whitespace`]
/// runs after this), so a run of padding between a clock shape and the end of a line is what
/// says it is a pane's own text and not the bar's clock.
///
/// What comes *before* it is not constrained to a space - on a tight line the block abuts the
/// arrow separator - only to a non-digit, so a longer number ending in a clock shape is left
/// alone. The date half is optional because a narrow terminal sheds it and keeps the time.
fn strip_status_bar_clock(text: &str) -> String {
    regex::Regex::new(
        r"(?m)(^|[^0-9])\d{2}:\d{2}(?:  (?:Sun|Mon|Tue|Wed|Thu|Fri|Sat) \d{1,2} [A-Z][a-z]{2})?( ?(?:[│┃|] *)?)$",
    )
    .unwrap()
    .replace_all(text, "$1$2")
    .to_string()
}

fn strip_tip_indication(text: &str) -> String {
    regex::Regex::new(r" Tip: [^\n]*")
        .unwrap()
        .replace_all(text, "")
        .to_string()
}

fn replace_unique_session_name(text: &str) -> String {
    regex::Regex::new(r"Yinz \(test-\d+\)")
        .unwrap()
        .replace_all(text, "Yinz (test)")
        .to_string()
}

/// The swap layout the tab is on, which `override_layout` shows and then takes away again.
///
/// Anchored with `(?m)$` rather than a literal newline, as [`strip_status_bar_clock`] is: a bar
/// drawn on the terminal's *last* row - which is where a compact bar sits - has no newline after
/// it, and a trailing-newline anchor silently leaves the indicator in that one place it matters
/// most.
fn strip_swap_layout_indication(text: &str) -> String {
    let swap_layout_name =
        "BASE|VERTICAL|THREE-COLUMNS|HORIZONTAL|STACKED|STAGGERED|ENLARGED|SPREAD";
    let powerline_separated_or_plain = |prefix: &str| {
        regex::Regex::new(&format!(
            "(?m){} [\u{e0b0}]? ?(?:{}) ?[\u{e0b0}]?[ ]*$",
            prefix, swap_layout_name
        ))
        .unwrap()
    };
    let normal_mode = powerline_separated_or_plain("Alt <\\[\\]>");
    let tmux_mode_1 = powerline_separated_or_plain("Alt \\[\\|SPACE\\|Alt \\]");
    let tmux_mode_2 = powerline_separated_or_plain("Alt \\[\\|Alt \\]\\|SPACE");
    let tab_bar = regex::Regex::new(&format!(
        "(?m) {{2,}}[\u{e0b0}]? ?(?:{}) ?[\u{e0b0}]?[ ]*$",
        swap_layout_name
    ))
    .unwrap();
    let text = normal_mode.replace_all(text, "");
    let text = tmux_mode_1.replace_all(&text, "");
    let text = tmux_mode_2.replace_all(&text, "");
    tab_bar.replace_all(&text, "").to_string()
}

fn strip_trailing_whitespace(text: &str) -> String {
    regex::Regex::new(r"\s*\n")
        .unwrap()
        .replace_all(text, "\n")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_thread_panics_fail_session_shutdown() {
        const PROBE_ENV: &str = "YINZ_TEST_BACKGROUND_PANIC_PROBE";
        if std::env::var_os(PROBE_ENV).is_some() {
            let mut session = TestRunner::new(crate::TERMINAL_SIZE)
                .with_config("load_plugins clear-defaults=true {}")
                .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
                .start();
            session.expect_pty_spawn().output(b"ready");
            session.wait_until("probe session loaded", |grid| grid.contains("ready"));
            // Server teardown discards worker join errors. This must still fail the test
            // even when its visible behavior and outer server shutdown both succeed.
            let _ = std::thread::Builder::new()
                .name("fault_injected_worker".into())
                .spawn(|| panic!("injected worker invariant failure"))
                .unwrap()
                .join();
            session.quit();
            return;
        }

        // Isolate the intentional panic from this process's panic recorder, including when
        // bare cargo test runs other tests alongside this one.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runner::tests::background_thread_panics_fail_session_shutdown",
                "--nocapture",
            ])
            .env(PROBE_ENV, "1")
            .output()
            .unwrap();
        let diagnostic = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.status.success(),
            "worker panic passed:\n{diagnostic}"
        );
        assert!(diagnostic.contains("fault_injected_worker"), "{diagnostic}");
        assert!(
            diagnostic.contains("injected worker invariant failure"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("background thread panicked during integration test"),
            "{diagnostic}"
        );
    }

    /// A compact bar is drawn on the terminal's last row, which has no newline after it. An
    /// end-anchored strip has to reach it there too, or the indicator only ever survives into
    /// the snapshots of the one layout that puts the bar last.
    #[test]
    fn the_swap_layout_indicator_is_stripped_on_the_last_row_too() {
        let bar = " Yinz (test) NORMAL  [0] Tab #1";
        let indicator = format!("{bar}{}BASE ", " ".repeat(20));
        assert_eq!(
            strip_trailing_whitespace(&strip_swap_layout_indication(&format!("$ \n{indicator}\n"))),
            format!("$\n{bar}\n"),
        );
        assert_eq!(
            strip_trailing_whitespace(&strip_swap_layout_indication(&indicator)),
            bar,
            "the last row carries no newline to anchor against",
        );
    }

    /// Rows arrive here padded to the terminal width, so a clock shape in the middle of a
    /// pane's line is followed by the rest of that row. Only the bar's own clock is flush
    /// against the right edge, and only that one is the wall clock a snapshot cannot pin.
    #[test]
    fn only_a_flush_right_clock_is_stripped() {
        let width = 40;
        let pane_line = format!("{:width$}", "$ make -j 12:34");
        let bar_line = format!("{:>width$}", "<q> QUIT      14:32 ");
        let grid = format!("{pane_line}\n{bar_line}\n");

        let stripped = strip_status_bar_clock(&grid);
        assert!(
            stripped.contains("$ make -j 12:34"),
            "a pane's own line was rewritten: {stripped:?}"
        );
        assert!(
            !stripped.contains("14:32"),
            "the bar's clock survived: {stripped:?}"
        );
    }

    /// A nested session draws a second bar inside a pane, so its clock ends against the pane
    /// border and the host's own row runs on past it. The border and what follows are put back.
    #[test]
    fn a_nested_bars_clock_is_stripped_and_the_border_kept() {
        let stripped = strip_status_bar_clock(" <q> QUIT      14:32 │        \n");
        assert!(!stripped.contains("14:32"), "{stripped:?}");
        assert!(stripped.contains('│'), "{stripped:?}");
    }

    /// The date half is what a narrow terminal sheds first, so both shapes have to go.
    #[test]
    fn the_date_half_goes_with_the_time() {
        let stripped = strip_status_bar_clock(" <q> QUIT   14:32  Fri 5 Sep \n");
        assert_eq!(stripped, " <q> QUIT    \n");
    }

    /// A longer number that happens to end in a clock shape - a byte count, a duration - is
    /// not a clock.
    #[test]
    fn a_longer_number_ending_in_a_clock_shape_is_left_alone() {
        let line = " downloaded 112:34 \n";
        assert_eq!(strip_status_bar_clock(line), line);
    }
}

#[cfg(test)]
#[path = "unit/session_ownership_tests.rs"]
mod session_ownership_tests;
