use super::*;
use std::{
    fs,
    io::{Seek, SeekFrom},
    os::unix::process::CommandExt,
    process::{Child, Stdio},
    sync::mpsc,
};
use yinz_ipc::{
    consts::ipc_connect,
    ipc::{
        ClientToServerMsg as ClientMsg, IpcRecv, IpcSenderWithContext,
        ServerToClientMsg as ServerMsg,
    },
};
use yinz_terminal::panes::grid::Grid;
use yinz_types::{
    input::{actions::Action, cli_assets::CliAssets},
    pane_size::Size,
};

#[cfg(test)]
#[path = "unit/session.rs"]
mod tests;

pub(super) fn environment(command: &mut Command, root: &Path) {
    // A whitelist also removes active-session identity, hooks, database URLs, and test overrides.
    let path = std::env::join_paths(std::iter::once(root.join("fixtures")).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap_or_default();
    command
        .env_clear()
        .env("PATH", path)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("YINZ_TEST_CACHE_ROOT", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("TMPDIR", root.join("tmp"))
        .env("YINZ_SOCKET_DIR", root.join("sockets"))
        .env("YINZ_SESSION_NAME", "plugin-test")
        .env("YINZ_DISABLE_PTY_HOST", "1")
        .env("PS1", "yinz-test> ")
        .env("SHELL", "/bin/sh")
        .env("TERM", "xterm-256color");
}

fn session_socket(root: &Path) -> PathBuf {
    root.join("sockets")
        .join(yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR.as_str())
        .join("plugin-test")
}

pub(super) struct OwnedSession {
    root: tempfile::TempDir,
    child: Child,
    stopped: bool,
}
impl OwnedSession {
    pub fn new() -> Result<Self> {
        // Short /tmp paths fit macOS's 104-byte Unix socket limit too.
        let root = tempfile::Builder::new()
            .prefix("ypt-")
            .tempdir_in("/tmp")
            .context("create integration test home")?;
        for dir in [
            "home", "config", "data", "cache", "runtime", "tmp", "sockets", "fixtures",
        ] {
            fs::create_dir(root.path().join(dir)).context("create test directory")?;
        }
        fs::create_dir_all(
            session_socket(root.path())
                .parent()
                .context("test socket directory")?,
        )
        .context("create test socket directory")?;
        let mut command = Command::new(binary()?);
        environment(&mut command, root.path());
        let child = command
            .arg("--plugin-test-server")
            .arg(session_socket(root.path()))
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(fs::File::create(root.path().join("server.log")).context("create server log")?)
            .spawn()
            .context("start disposable server")?;
        fs::write(root.path().join("server.pid"), child.id().to_string())
            .context("record owned server pid")?;
        Ok(Self {
            root,
            child,
            stopped: false,
        })
    }
    pub fn root(&self) -> &Path {
        self.root.path()
    }
    pub fn diagnostics(&self) -> String {
        let mut text = tail(&self.root.path().join("server.log"));
        append_bounded(&mut text, &tail(&self.root.path().join("diagnostics")));
        for path in log_files(&self.root.path().join("tmp")) {
            append_bounded(&mut text, &tail(&path));
        }
        append_bounded(
            &mut text,
            &format!("\nowned server pid: {}\n", self.child.id()),
        );
        text
    }
    pub fn shutdown(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        if let Some(status) = self.child.try_wait().context("reap test server")? {
            self.stopped = true;
            if !status.success() {
                bail!("test server exited with {status}");
            }
            return Ok(());
        }
        let delivery = ipc_connect(&session_socket(self.root.path()))
            .context("connect to owned test session for shutdown")
            .and_then(|stream| {
                IpcSenderWithContext::<ClientMsg>::new(stream)
                    .send_client_msg(ClientMsg::KillSession)
                    .context("stop owned test session")
            });
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if let Some(status) = self.child.try_wait().context("reap test server")? {
                self.stopped = true;
                if !status.success() {
                    bail!("test server exited with {status}; shutdown IPC: {delivery:?}");
                }
                return delivery;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(self.child.id() as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        self.child.kill().context("kill unresponsive test server")?;
        self.child.wait().context("reap unresponsive test server")?;
        self.stopped = true;
        let delivery = delivery
            .err()
            .map(|error| format!("; shutdown IPC: {error:#}"))
            .unwrap_or_default();
        bail!("test server did not shut down within five seconds{delivery}")
    }
}
impl Drop for OwnedSession {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn tail(path: &Path) -> String {
    let read = || -> std::io::Result<String> {
        let mut file = fs::File::open(path)?;
        let size = file.metadata()?.len();
        file.seek(SeekFrom::Start(size.saturating_sub(OUTPUT_LIMIT as u64)))?;
        let mut bytes = Vec::new();
        file.take(OUTPUT_LIMIT as u64).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    read().unwrap_or_default()
}

fn log_files(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![];
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                paths.extend(log_files(&path));
            } else if path.file_name().is_some_and(|name| name == "yinz.log") {
                paths.push(path);
            }
        }
    }
    paths
}

pub(super) struct Controller {
    root: PathBuf,
    grants: Vec<PermissionType>,
    sender: Option<IpcSenderWithContext<ClientMsg>>,
    messages: Option<mpsc::Receiver<ServerMsg>>,
    grid: Grid,
    parser: vte::Parser,
    events: Vec<Value>,
    size: Size,
    rendered: bool,
    agent_requests: BTreeMap<u64, mpsc::Receiver<std::result::Result<Value, String>>>,
    next_agent_request: u64,
    auto_grant_permissions: bool,
    plugin_responses:
        BTreeMap<String, std::result::Result<yinz_types::cli::PluginResponse, String>>,
}
impl Controller {
    pub fn new(root: PathBuf, grants: Vec<PermissionType>) -> Self {
        Self {
            root,
            grants,
            sender: None,
            messages: None,
            grid: Grid::new(
                24,
                80,
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                false,
                false,
                true,
                true,
                false,
            ),
            parser: vte::Parser::new(),
            events: vec![],
            size: Size { rows: 24, cols: 80 },
            rendered: false,
            agent_requests: BTreeMap::new(),
            next_agent_request: 0,
            auto_grant_permissions: true,
            plugin_responses: BTreeMap::new(),
        }
    }
    fn send(&mut self, message: ClientMsg) -> Result<()> {
        self.sender
            .as_mut()
            .context("call session.start() first")?
            .send_client_msg(message)
    }
    fn action(&mut self, action: Action) -> Result<()> {
        self.send(ClientMsg::Action {
            action,
            terminal_id: None,
            client_id: None,
            is_cli_client: false,
        })
    }
    fn plugin_lifecycle(
        &mut self,
        operation: yinz_types::cli::PluginOperation,
        location: String,
        configuration: Option<BTreeMap<String, String>>,
    ) -> Result<Value> {
        let fallback = if location.starts_with("plugin_") {
            None
        } else {
            Some(
                yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                    &location, &None, None, None,
                )
                .map_err(anyhow::Error::msg)?
                .get_run_plugin()
                .context("test plugin location")?,
            )
        };
        let id = uuid::Uuid::new_v4().to_string();
        self.send(ClientMsg::PluginRequest {
            request_id: id.clone(),
            request: yinz_types::cli::PluginRequest {
                operation,
                target: Some(location),
                configuration,
                fallback,
                candidates: vec![],
                caller: None,
            },
        })?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.collect(true)?;
            if let Some(result) = self.plugin_responses.remove(&id) {
                return serde_json::to_value(result.map_err(anyhow::Error::msg)?)
                    .context("test lifecycle response");
            }
            if Instant::now() >= deadline {
                bail!("plugin lifecycle response timed out");
            }
        }
    }
    fn collect(&mut self, wait: bool) -> Result<()> {
        let receiver = self
            .messages
            .as_ref()
            .context("call session.start() first")?;
        let first = if wait {
            receiver.recv_timeout(Duration::from_millis(10)).ok()
        } else {
            receiver.try_recv().ok()
        };
        for message in first.into_iter().chain(receiver.try_iter()).take(1000) {
            match message {
                ServerMsg::PluginResponse { request_id, result } => {
                    self.plugin_responses.insert(request_id, result);
                },
                ServerMsg::Render { content } => {
                    self.parser.advance(&mut self.grid, content.as_bytes());
                    self.rendered = true;
                },
                ServerMsg::EventUpdate {
                    event_type,
                    event_json,
                } => {
                    if self.events.len() == 1000 {
                        self.events.remove(0);
                    }
                    self.events.push(json!({"type":event_type, "event":serde_json::from_str::<Value>(&event_json).unwrap_or(Value::Null)}));
                },
                ServerMsg::Exit { exit_reason } => bail!("test session exited: {exit_reason}"),
                _ => {},
            }
        }
        Ok(())
    }
    fn location(&self, path: &str, loader: &PluginModuleLoader) -> Result<String> {
        if let Some(name) = path.strip_prefix("yinz:") {
            if yinz_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES.contains(&name)
                || yinz_config::consts::BUILTIN_PLUGIN_NAMES.contains(&name)
            {
                return Ok(path.to_owned());
            }
            bail!("integration test requires a shipped builtin: {path}");
        }
        let url = loader.resolve_local(path, loader.entrypoint().as_str(), false)?;
        loader.local_path(&url, false)?;
        Ok(url.to_string())
    }
    fn grant(&self, location: &str) -> Result<()> {
        if !self.auto_grant_permissions {
            return Ok(());
        }
        let pid =
            fs::read_to_string(self.root.join("server.pid")).context("read owned server pid")?;
        let path = self
            .root
            .join("cache")
            .join(pid)
            .join("cache/permissions.kdl");
        fs::create_dir_all(path.parent().context("permission cache directory")?)
            .context("create test permission cache")?;
        let mut cache = yinz_config::permission::PermissionCache::from_path_or_default(Some(path));
        let key = yinz_runtime::plugin_location::parse_plugin_location(location, None)
            .map_err(|error| anyhow!(error))?
            .to_string();
        cache.cache(key, self.grants.clone());
        cache.write_to_file().context("write isolated test grants")
    }
    fn actions(&mut self, text: &str) -> Result<()> {
        let doc: kdl::KdlDocument = text.parse().context("parse test action")?;
        for node in doc.nodes() {
            // These actions operate only on the owned session. Config, shell, remote-session,
            // and arbitrary plugin-loading actions must not bypass declared test grants.
            if ![
                "SwitchToMode",
                "CloseFocus",
                "NewPane",
                "MoveFocus",
                "Resize",
                "ToggleFloatingPanes",
                "ToggleFocusFullscreen",
                "GoToNextTab",
                "GoToPreviousTab",
                "NewTab",
                "CloseTab",
                "TogglePaneFrames",
                "NextPaneTab",
                "PreviousPaneTab",
                "FocusPaneTabs",
            ]
            .contains(&node.name().value())
            {
                bail!(
                    "unsupported test action {}; use the typed session controls",
                    node.name().value()
                );
            }
            if node.entries().iter().any(|entry| entry.name().is_some())
                || node.children().is_some()
            {
                bail!("test actions cannot supply configuration or commands");
            }
            let action = yinz_config::kdl::action::from_kdl((node, &Default::default()))
                .map_err(|error| anyhow!(error.to_string()))?;
            self.action(action)?;
        }
        Ok(())
    }
    pub fn diagnostics(&self, output: &str, pending: &str) {
        let mut text = format!("{output}\npending: {pending}\nrecent screen:\n");
        append_bounded(&mut text, &self.grid.dump_screen(false));
        let _ = fs::write(self.root.join("diagnostics"), text);
    }
    pub fn call(&mut self, op: &str, arg: Value, loader: &PluginModuleLoader) -> Result<Value> {
        match op {
            "fixture" => {
                if self.sender.is_some() {
                    bail!("create fixtures before session.start()");
                }
                let files: BTreeMap<String, String> =
                    serde_json::from_value(arg["files"].clone()).context("fixture files")?;
                let commands: BTreeMap<String, String> =
                    serde_json::from_value(arg["commands"].clone()).context("fixture commands")?;
                if !commands.is_empty() && !self.grants.contains(&PermissionType::RunCommands) {
                    bail!("executable fixtures require RunCommands");
                }
                if files
                    .iter()
                    .chain(commands.iter())
                    .map(|(name, text)| name.len() + text.len())
                    .sum::<usize>()
                    > 1_048_576
                {
                    bail!("fixtures exceed 1 MiB");
                }
                for (name, text) in files {
                    let path = Path::new(&name);
                    if name.is_empty()
                        || !path
                            .components()
                            .all(|part| matches!(part, std::path::Component::Normal(_)))
                    {
                        bail!("fixture path must stay inside the test home");
                    }
                    let path = self.root.join("home").join(path);
                    fs::create_dir_all(path.parent().context("fixture parent")?)
                        .context("create fixture directory")?;
                    fs::write(path, text).context("write fixture")?;
                }
                for (name, script) in commands {
                    if name.is_empty()
                        || !name
                            .bytes()
                            .all(|ch| ch.is_ascii_alphanumeric() || b"_-".contains(&ch))
                    {
                        bail!("fixture command requires a simple executable name");
                    }
                    use std::os::unix::fs::PermissionsExt;
                    let path = self.root.join("fixtures").join(name);
                    // Like write_warm_script in the Rust test support, pay macOS's
                    // first-exec validation cost before the fixture's observed operation.
                    fs::write(&path, format!("#!/bin/sh\n[ \"$YINZ_TEST_FIXTURE_WARMUP\" = 1 ] && exit 0\n{script}\n"))
                        .context("write fixture executable")?;
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                        .context("make fixture executable")?;
                    let mut warm = Command::new(&path);
                    environment(&mut warm, &self.root);
                    let status = warm
                        .env("YINZ_TEST_FIXTURE_WARMUP", "1")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .context("warm fixture executable")?;
                    if !status.success() {
                        bail!("fixture warmup failed: {status}");
                    }
                }
                return Ok(json!(self
                    .root
                    .join("home")
                    .canonicalize()
                    .context("canonical fixture home")?));
            },
            "start" => {
                self.auto_grant_permissions = arg["autoGrantPermissions"] != false;
                if self.sender.is_some() {
                    bail!("one session per integration test");
                }
                self.size = dimensions(&arg, self.size)?;
                self.grid.change_size(self.size.rows, self.size.cols);
                let mut config = "mouse_mode true\ndetachable_panes false\nsession_serialization false\ndefault_shell \"/bin/sh\"\n".to_owned();
                let entry = loader.local_path(loader.entrypoint(), false)?;
                config.push_str(&format!(
                    "user_plugin_dir {}\n",
                    quote(
                        &entry
                            .parent()
                            .context("test plugin directory")?
                            .display()
                            .to_string()
                    )
                ));
                config.push_str("hooks { ");
                for point in yinz_types::input::hooks::HookPoint::ALL {
                    let location = arg
                        .get("hooks")
                        .and_then(|hooks| hooks.get(point.as_str()))
                        .and_then(Value::as_str);
                    let location = location
                        .map(|path| self.location(path, loader))
                        .transpose()?
                        .unwrap_or_default();
                    if !location.is_empty() {
                        self.grant(&location)?;
                    }
                    config.push_str(&format!("{} {};", point.as_str(), quote(&location)));
                }
                config.push_str(" }\n");
                if let Some(hooks) = arg.get("hooks") {
                    for (point, path) in hooks.as_object().context("hooks must be an object")? {
                        let hook: yinz_types::input::hooks::HookPoint =
                            point.parse().map_err(|e| anyhow!("{e}"))?;
                        let _ = hook;
                        let location =
                            self.location(path.as_str().context("hook path")?, loader)?;
                        self.grant(&location)?;
                    }
                }
                config.push_str("load_plugins clear-defaults=true { ");
                if let Some(background) = arg.get("backgroundPlugins") {
                    for path in background
                        .as_array()
                        .context("backgroundPlugins must be an array")?
                    {
                        let location = self.location(
                            path.as_str()
                                .or_else(|| path["path"].as_str())
                                .context("background plugin path")?,
                            loader,
                        )?;
                        self.grant(&location)?;
                        let configuration: BTreeMap<String, String> = serde_json::from_value(
                            path.get("configuration").cloned().unwrap_or(json!({})),
                        )
                        .context("background plugin configuration")?;
                        let configuration = configuration
                            .iter()
                            .map(|(key, value)| format!("{} {};", quote(key), quote(value)))
                            .collect::<String>();
                        config.push_str(&format!("{} {{ {} }};", quote(&location), configuration));
                    }
                }
                config.push_str(" }\n");
                yinz_config::config::Config::from_kdl(&config, None)
                    .map_err(|error| anyhow!("invalid disposable session config: {error:?}"))?;
                fs::write(self.root.join("config.kdl"), config).context("write test config")?;
                fs::write(self.root.join("layout.kdl"), "layout { pane; }")
                    .context("write test layout")?;
                let deadline = Instant::now() + Duration::from_secs(10);
                let stream = loop {
                    match ipc_connect(&session_socket(&self.root)) {
                        Ok(stream) => break stream,
                        Err(error) if Instant::now() >= deadline => {
                            return Err(error).context("connect test server")
                        },
                        Err(_) => std::thread::sleep(Duration::from_millis(10)),
                    }
                };
                let sender = IpcSenderWithContext::<ClientMsg>::new(stream);
                let mut receiver = sender
                    .get_receiver::<ServerMsg>()
                    .context("set up test session replies")?;
                let (tx, rx) = mpsc::sync_channel(1000);
                std::thread::spawn(move || {
                    while let Some((message, _)) = receiver.recv_server_msg().msg() {
                        if tx.send(message).is_err() {
                            break;
                        }
                    }
                });
                self.sender = Some(sender);
                self.messages = Some(rx);
                self.send(ClientMsg::FirstClientConnected {
                    cli_assets: CliAssets {
                        config_file_path: Some(self.root.join("config.kdl")),
                        config_dir: Some(self.root.join("config")),
                        data_dir: Some(self.root.join("data/yinz")),
                        layout: Some(yinz_types::data::LayoutInfo::File(
                            self.root.join("layout.kdl").display().to_string(),
                            Default::default(),
                        )),
                        terminal_window_size: self.size,
                        cwd: Some(self.root.join("home")),
                        ..Default::default()
                    },
                })?;
                // Event subscribers use their own connection and join the same queue below later.
                let deadline = Instant::now() + Duration::from_secs(10);
                while !self.rendered {
                    self.collect(true)?;
                    if Instant::now() >= deadline {
                        bail!("test server did not render its initial layout");
                    }
                }
                self.send(ClientMsg::SubscribeToEvents {
                    options: Default::default(),
                    event_types: vec![],
                })?;
            },
            "load" => {
                let path = arg["path"].as_str().context("plugin path")?;
                let location = self.location(path, loader)?;
                if arg["autoGrantPermissions"] != false {
                    self.grant(&location)?;
                }
                let configuration: BTreeMap<String, String> =
                    serde_json::from_value(arg.get("configuration").cloned().unwrap_or(json!({})))
                        .context("plugin configuration")?;
                if arg["headless"] == true {
                    return self.plugin_lifecycle(
                        yinz_types::cli::PluginOperation::Start,
                        location,
                        Some(configuration),
                    );
                }
                let configuration = configuration
                    .iter()
                    .map(|(key, value)| format!("{} {};", quote(key), quote(value)))
                    .collect::<String>();
                let action = format!("LaunchPlugin {} {{ {} }}", quote(&location), configuration);
                let doc: kdl::KdlDocument = action.parse().context("plugin load action")?;
                for node in doc.nodes() {
                    self.action(
                        yinz_config::kdl::action::from_kdl((node, &Default::default()))
                            .map_err(|error| anyhow!(error.to_string()))?,
                    )?;
                }
            },
            "reload" | "stop" | "status" => {
                let path = arg["path"]
                    .as_str()
                    .or_else(|| arg.as_str())
                    .context("plugin path")?;
                let location = if path.starts_with("plugin_") {
                    path.to_owned()
                } else {
                    self.location(path, loader)?
                };
                let configuration = arg
                    .get("configuration")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .context("plugin configuration")?;
                let operation = match op {
                    "reload" => yinz_types::cli::PluginOperation::Reload,
                    "stop" => yinz_types::cli::PluginOperation::Stop,
                    _ => yinz_types::cli::PluginOperation::Inspect,
                };
                return self.plugin_lifecycle(operation, location, configuration);
            },
            "key" => {
                let key: yinz_types::keys::KeyWithModifier = arg
                    .as_str()
                    .context("key must be text")?
                    .parse()
                    .map_err(|error| anyhow!("{error}"))?;
                self.send(ClientMsg::Key {
                    raw_bytes: yinz_key_codec::keys::serialize_non_kitty(&key)
                        .context("key has no terminal encoding")?
                        .into_bytes(),
                    key,
                    is_kitty_keyboard_protocol: false,
                })?;
            },
            "text" => {
                for ch in arg.as_str().context("text must be a string")?.chars() {
                    self.send(ClientMsg::Key {
                        key: yinz_types::keys::KeyWithModifier::new(
                            yinz_types::keys::BareKey::Char(ch),
                        ),
                        raw_bytes: ch.to_string().into_bytes(),
                        is_kitty_keyboard_protocol: false,
                    })?;
                }
            },
            "mouse" => self.action(Action::MouseEvent {
                event: serde_json::from_value(arg).context("mouse event")?,
            })?,
            "resize" => {
                self.size = dimensions(&arg, self.size)?;
                self.grid.change_size(self.size.rows, self.size.cols);
                self.send(ClientMsg::TerminalResize {
                    new_size: self.size,
                })?;
            },
            "agent" => {
                use yinz_types::agent::control::AgentRequest;
                self.sender.as_ref().context("call session.start() first")?;
                if !self.grants.contains(&PermissionType::RunCommands) {
                    bail!("session.agent requires the declared RunCommands grant");
                }
                if self.agent_requests.len() >= 32 {
                    bail!("at most 32 test agent requests may be pending");
                }
                let request: AgentRequest =
                    serde_json::from_value(arg).context("test agent request")?;
                if let AgentRequest::Spawn { remote_host, .. } = &request {
                    if remote_host.is_some() {
                        bail!("test agent spawning is limited to the owned local session");
                    }
                }
                let timeout = match &request {
                    AgentRequest::Prompt { timeout_ms, .. }
                    | AgentRequest::Wait { timeout_ms, .. } => {
                        Duration::from_millis((*timeout_ms).min(3_600_000) + 2000)
                    },
                    _ => Duration::from_secs(30),
                };
                let socket = session_socket(&self.root);
                let (sender, receiver) = mpsc::channel();
                std::thread::spawn(move || {
                    let response = exchange(
                        &socket,
                        ClientMsg::AgentControl(request),
                        timeout,
                        |message| match message {
                            ServerMsg::AgentControl(response) => Some(
                                serde_json::to_value(response)
                                    .context("encode test agent response"),
                            ),
                            _ => None,
                        },
                    );
                    let _ = sender.send(response.map_err(|error| format!("{error:#}")));
                });
                let id = self.next_agent_request;
                self.next_agent_request += 1;
                self.agent_requests.insert(id, receiver);
                return Ok(json!(id));
            },
            "agent-poll" => {
                let id = arg.as_u64().context("test agent request id")?;
                let result = match self
                    .agent_requests
                    .get(&id)
                    .context("unknown test agent request")?
                    .try_recv()
                {
                    Ok(result) => result,
                    Err(mpsc::TryRecvError::Empty) => return Ok(Value::Null),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Err("test agent request disconnected".into())
                    },
                };
                self.agent_requests.remove(&id);
                return serde_json::to_value(result).context("encode test agent request result");
            },
            "report" => {
                use yinz_types::agent::{
                    hook_outcome,
                    report::{AgentReport, AgentReportState},
                    AgentEvent, HookEvent, HookOutcome,
                };
                use yinz_types::{internal_ids::ProcessId, pane_id::PaneId};
                self.sender.as_ref().context("call session.start() first")?;
                if !self.grants.contains(&PermissionType::RunCommands) {
                    bail!("session.report requires the declared RunCommands grant");
                }
                let pane: PaneId =
                    serde_json::from_value(arg["pane"].clone()).context("test report pane")?;
                let PaneId::Terminal(terminal) = pane else {
                    bail!("agent hooks require a terminal pane");
                };
                let kind = arg["kind"].as_str().context("test report agent kind")?;
                let hook: HookEvent =
                    serde_json::from_value(arg["hook"].clone()).context("test report hook")?;
                let state = match hook_outcome(kind, &hook) {
                    HookOutcome::Ended => AgentReportState::Clear,
                    HookOutcome::Reported(agent) => AgentReportState::Set(agent),
                    HookOutcome::Nothing => AgentReportState::Unchanged,
                };
                let report_id = uuid::Uuid::new_v4().to_string();
                let report = AgentReport {
                    report_id: report_id.clone(),
                    pane_id: pane,
                    state,
                    event: Some(AgentEvent::from_hook(
                        kind,
                        terminal,
                        &hook,
                        serde_json::to_string(&hook).context("encode test hook")?,
                    )),
                    trace: false,
                    process_id: ProcessId::new(std::process::id()),
                    kind: kind.into(),
                };
                yinz_runtime::agent::report::validate_report(&report)?;
                return exchange(
                    &session_socket(&self.root),
                    ClientMsg::AgentReport(report),
                    Duration::from_secs(5),
                    |message| match message {
                        ServerMsg::AgentReportAccepted {
                            report_id: received,
                        } if received == report_id => Some(Ok(Value::Null)),
                        _ => None,
                    },
                );
            },
            "action" => self.actions(arg.as_str().context("action must be KDL text")?)?,
            "screen" => {
                self.collect(false)?;
                return Ok(json!(self.grid.dump_screen(false)));
            },
            "events" => {
                self.collect(false)?;
                return Ok(json!(self.events));
            },
            "panes" => {
                self.collect(false)?;
                let panes = self
                    .events
                    .iter()
                    .rev()
                    .find_map(|event| event["event"].get("PaneUpdate"))
                    .and_then(|manifest| manifest["panes"].as_object());
                return Ok(json!(panes
                    .into_iter()
                    .flat_map(|panes| panes.values())
                    .filter_map(Value::as_array)
                    .flatten()
                    .collect::<Vec<_>>()));
            },
            "data" | "catalog" => {
                let store = yinz_storage::plugin_data_store::PluginDataStore::new(
                    &self.root.join("data/yinz"),
                );
                if op == "catalog" {
                    return serde_json::to_value(
                        store.catalog(None).map_err(|error| anyhow!(error))?,
                    )
                    .context("test data catalog");
                }
                let location =
                    self.location(arg["plugin"].as_str().context("data plugin")?, loader)?;
                let location =
                    yinz_runtime::plugin_location::parse_plugin_location(&location, None)
                        .map_err(|error| anyhow!(error))?;
                // Match production's existing private namespace serialization.
                let location = url::Url::from(&location);
                let namespace =
                    yinz_storage::plugin_data_store::private_namespace(location.as_ref());
                let request = yinz_types::plugin_data::FetchRequest::entity(
                    arg["entity"].as_str().context("data entity")?,
                );
                return serde_json::to_value(
                    store
                        .fetch(&namespace, &request)
                        .map_err(|error| anyhow!(error))?,
                )
                .context("read test plugin data");
            },
            "logs" => {
                let mut output = String::new();
                for path in log_files(&self.root.join("tmp")) {
                    append_bounded(&mut output, &tail(&path));
                }
                return Ok(json!(output));
            },
            "poll" => self.collect(true)?,
            "rpc" => {
                for permission in [
                    PermissionType::ReadCliPipes,
                    PermissionType::MessageAndLaunchOtherPlugins,
                ] {
                    if !self.grants.contains(&permission) {
                        bail!("session.rpc requires the declared {permission:?} grant");
                    }
                }
                let target =
                    self.location(arg["plugin"].as_str().context("RPC plugin")?, loader)?;
                let timeout_ms = arg["timeoutMs"]
                    .as_u64()
                    .context("RPC timeout must be an integer")?;
                if !(1..=3_600_000).contains(&timeout_ms) {
                    bail!("RPC timeout must be between 1 and 3600000 milliseconds");
                }
                let path = self.root.join("rpc.ts");
                if !path.exists() {
                    fs::write(&path, include_str!("rpc.ts")).context("install RPC test client")?;
                }
                let location = format!(
                    "file:{}",
                    path.canonicalize()
                        .context("resolve RPC test client")?
                        .display()
                );
                self.grant(&location)?;
                let payload = json!({"pluginUrl":target,"name":arg["name"],"payload":arg["payload"],"timeoutMs":timeout_ms}).to_string();
                let reply = self.pipe(
                    &json!({"name":"test-rpc","payload":payload}),
                    Some(location),
                    Duration::from_millis(timeout_ms + 1000),
                )?;
                return serde_json::from_str::<std::result::Result<Value, String>>(
                    reply.as_str().context("RPC reply must be text")?,
                )
                .context("decode test RPC reply")?
                .map_err(|error| anyhow!(error));
            },
            "pipe" => {
                let plugin = arg
                    .get("plugin")
                    .and_then(Value::as_str)
                    .map(|path| self.location(path, loader))
                    .transpose()?;
                return self.pipe(&arg, plugin, Duration::from_secs(5));
            },
            _ => bail!("unknown session control {op}"),
        }
        Ok(Value::Null)
    }
    fn pipe(&self, arg: &Value, plugin: Option<String>, timeout: Duration) -> Result<Value> {
        let stream = ipc_connect(&session_socket(&self.root)).context("connect test pipe")?;
        let mut sender = IpcSenderWithContext::<ClientMsg>::new(stream);
        let mut receiver = sender
            .get_receiver::<ServerMsg>()
            .context("set up test pipe reply")?;
        receiver
            .set_recv_timeout(Some(timeout))
            .context("bound pipe reply")?;
        sender.send_client_msg(ClientMsg::Action {
            action: Action::CliPipe {
                focus_plugin: false,
                pipe_id: "test-pipe".into(),
                name: Some(arg["name"].as_str().context("pipe name")?.into()),
                payload: arg
                    .get("payload")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                args: Some(
                    serde_json::from_value(arg.get("args").cloned().unwrap_or(json!({})))
                        .context("pipe args")?,
                ),
                plugin,
                configuration: None,
                launch_new: false,
                skip_cache: false,
                floating: None,
                in_place: None,
                cwd: None,
                pane_title: None,
            },
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        })?;
        let mut output = String::new();
        while let Some((message, _)) = receiver.recv_server_msg().msg() {
            match message {
                ServerMsg::CliPipeOutput { output: text, .. } => append_bounded(&mut output, &text),
                ServerMsg::UnblockCliPipeInput { .. } => return Ok(json!(output)),
                ServerMsg::Exit { .. } => break,
                _ => {},
            }
        }
        if output.is_empty() {
            bail!("pipe ended without a reply");
        }
        Ok(json!(output))
    }
}
/// Use the real control/report IPC routes; replies never bypass screen ownership checks.
fn exchange(
    socket: &Path,
    message: ClientMsg,
    timeout: Duration,
    reply: impl Fn(ServerMsg) -> Option<Result<Value>>,
) -> Result<Value> {
    let stream = ipc_connect(socket).context("connect test agent client")?;
    let mut sender = IpcSenderWithContext::<ClientMsg>::new(stream);
    let mut receiver = sender
        .get_receiver::<ServerMsg>()
        .context("set up test agent reply")?;
    sender
        .send_client_msg(message)
        .context("send test agent request")?;
    let deadline = Instant::now() + timeout;
    let result = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break Err(anyhow!("test agent request timed out"));
        }
        receiver
            .set_recv_timeout(Some(remaining))
            .context("bound test agent reply")?;
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerMsg::LogError { lines }, _) => {
                break Err(anyhow!("{}", lines.join("\n")))
            },
            IpcRecv::Msg(ServerMsg::Exit { exit_reason }, _) => {
                break Err(anyhow!("test agent session exited: {exit_reason}"))
            },
            IpcRecv::Msg(message, _) => {
                if let Some(result) = reply(message) {
                    break result;
                }
            },
            IpcRecv::Eof | IpcRecv::Idle | IpcRecv::Malformed => {
                break Err(anyhow!("test agent request ended without a reply"))
            },
        }
    };
    sender
        .send_client_msg(ClientMsg::ClientExited)
        .context("disconnect test agent client")
        .non_fatal();
    result
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("quote KDL string")
}
fn dimensions(value: &Value, default: Size) -> Result<Size> {
    let rows = value
        .get("rows")
        .map(|value| value.as_u64().context("rows must be an integer"))
        .transpose()?
        .unwrap_or(default.rows as u64);
    let cols = value
        .get("columns")
        .map(|value| value.as_u64().context("columns must be an integer"))
        .transpose()?
        .unwrap_or(default.cols as u64);
    if !(2..=500).contains(&rows) || !(2..=500).contains(&cols) {
        bail!("test dimensions must be between 2 and 500");
    }
    Ok(Size {
        rows: rows as usize,
        cols: cols as usize,
    })
}

pub(super) fn render_text(text: &str, rows: usize, cols: usize) -> Result<String> {
    if rows > 500 || cols > 500 || rows == 0 || cols == 0 {
        bail!("test render dimensions must be between 1 and 500");
    }
    let mut grid = Grid::new(
        rows,
        cols,
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        false,
        false,
        true,
        true,
        false,
    );
    vte::Parser::new().advance(&mut grid, text.as_bytes());
    Ok(grid.dump_screen(false))
}
