//! Script host acceptance harness shared by Scheme tests and retained runtime comparisons.
use super::*;
use yinz_types::internal_ids::{PluginId, TabId};
const CLIENT: ClientId = ClientId::new(1);
const PLUGIN: PluginId = PluginId::new(0);
const SIZE: Size = Size { cols: 80, rows: 10 };

// CustomMessage is ungated; InputReceived would require ReadApplicationState.
pub(super) fn release_event() -> Event {
    Event::CustomMessage("acceptance-release".into(), String::new())
}

pub(super) fn run_plugin(path: &std::path::Path) -> RunPluginOrAlias {
    RunPluginOrAlias::RunPlugin(RunPlugin {
        location: RunPluginLocation::File(path.to_owned()),
        configuration: PluginUserConfiguration::new(BTreeMap::from([(
            "greeting".into(),
            "configured".into(),
        )])),
        ..Default::default()
    })
}

pub(super) fn load(sender: &SenderWithContext<PluginInstruction>, plugin: RunPluginOrAlias) {
    sender
        .send(PluginInstruction::Load(
            Some(false),
            false,
            false,
            Some("deno acceptance".into()),
            plugin,
            Some(TabId::new(1)),
            None,
            CLIENT,
            SIZE,
            None,
            None,
            false,
            None,
            None,
            None,
            None,
        ))
        .unwrap();
}

pub(super) struct Harness {
    pub(super) sender: SenderWithContext<PluginInstruction>,
    pub(super) log: Watched<Vec<ScreenInstruction>>,
    pub(super) server_log: Watched<Vec<ServerInstruction>>,
    pub(super) directory: tempfile::TempDir,
    pub(super) path: PathBuf,
    pub(super) teardown: Option<Box<dyn FnOnce()>>,
    pub(super) logger: Option<std::thread::JoinHandle<()>>,
    pub(super) server_logger: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    pub(super) fn new(extension: &str, source: &str, status: PermissionStatus) -> Self {
        Self::with_permission_reply(extension, source, Some(status))
    }

    pub(super) fn with_permission_reply(
        extension: &str,
        source: &str,
        status: Option<PermissionStatus>,
    ) -> Self {
        Self::with_event_sender(extension, source, status, None)
    }

    pub(super) fn with_event_sender(
        extension: &str,
        source: &str,
        status: Option<PermissionStatus>,
        event_sender: Option<SenderWithContext<PluginInstruction>>,
    ) -> Self {
        Self::with_services(extension, source, status, event_sender, None)
    }

    pub(super) fn with_services(
        extension: &str,
        source: &str,
        status: Option<PermissionStatus>,
        event_sender: Option<SenderWithContext<PluginInstruction>>,
        data_access: Option<DataAccess>,
    ) -> Self {
        let directory = tempdir().unwrap();
        let path = directory.path().join(format!("acceptance.{extension}"));
        std::fs::write(&path, source).unwrap();
        let (sender, server_receiver, receiver, teardown) = create_plugin_thread_with_services(
            Some(directory.path().to_owned()),
            None,
            None,
            Default::default(),
            event_sender.clone(),
            data_access,
            Some(directory.path().to_owned()),
        );
        let server_log = Watched::new(vec![]);
        let server_logger = log_until_disconnect(&server_log, server_receiver);
        let log = Watched::new(vec![]);
        let logger = match status {
            Some(status) => permissions_and_log_until_disconnect(
                &log,
                receiver,
                directory.path().join("permissions.kdl"),
                // An intercepted plugin bus must also carry permission answers:
                // otherwise they can overtake CachePluginEvents on that bus and
                // leave all subsequent events cached after the grant.
                event_sender.unwrap_or_else(|| sender.clone()),
                CLIENT,
                status,
            ),
            None => log_until_disconnect(&log, receiver),
        };
        sender.send(PluginInstruction::AddClient(CLIENT)).unwrap();
        // Callers may write imported dependencies before starting the load.
        Self {
            sender,
            log,
            server_log,
            server_logger: Some(server_logger),
            directory,
            path,
            teardown: Some(teardown),
            logger: Some(logger),
        }
    }

    pub(super) fn load(&self) {
        load(&self.sender, run_plugin(&self.path));
    }

    pub(super) fn load_shared(&self) {
        load(
            &self.sender,
            RunPluginOrAlias::RunPlugin(RunPlugin {
                location: RunPluginLocation::File(self.path.clone()),
                configuration: PluginUserConfiguration::new(BTreeMap::from([(
                    "shared".into(),
                    "true".into(),
                )])),
                ..Default::default()
            }),
        );
    }

    pub(super) fn event(&self, event: Event) {
        self.sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(CLIENT),
                event,
            )]))
            .unwrap();
    }

    pub(super) fn reload(&self) {
        self.sender
            .send(PluginInstruction::Reload(
                Some(false),
                Some("deno acceptance".into()),
                run_plugin(&self.path),
                TabId::new(1),
                SIZE,
                None,
            ))
            .unwrap();
    }

    pub(super) fn rendered(&self, text: &str) -> String {
        wait_for_render_containing(&self.log, text)
    }

    pub(super) fn finish(mut self) -> Vec<ScreenInstruction> {
        self.stop();
        let log = self.log.lock().clone();
        log
    }

    fn stop(&mut self) {
        if let Some(teardown) = self.teardown.take() {
            teardown();
        }
        if let Some(logger) = self.server_logger.take() {
            logger.join().unwrap();
        }
        if let Some(logger) = self.logger.take() {
            logger.join().unwrap();
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // Preserve Watched's timeout and frame diagnostics if the runtime is stuck.
            // nextest isolates each test in a process; request shutdown without joining
            // a worker that may be the reason the wait failed. Successful tests still join.
            let _ = self.sender.send(PluginInstruction::Exit);
        } else {
            self.stop();
        }
    }
}

impl Harness {
    pub(super) fn permission_requests(&self, count: usize) -> Vec<Vec<PermissionType>> {
        self.log
            .wait_until("the next permission prompt", default_timeout(), |log| {
                let requests: Vec<_> = log
                    .iter()
                    .filter_map(|instruction| match instruction {
                        ScreenInstruction::RequestPluginPermissions(_, request) => {
                            Some(request.permissions.clone())
                        },
                        _ => None,
                    })
                    .collect();
                (requests.len() >= count).then_some(requests)
            })
    }

    pub(super) fn answer_permissions(
        &self,
        _permissions: Vec<PermissionType>,
        status: PermissionStatus,
    ) {
        self.sender
            .send(PluginInstruction::PermissionAnswer {
                request_id: self
                    .log
                    .lock()
                    .iter()
                    .rev()
                    .find_map(|instruction| match instruction {
                        ScreenInstruction::RequestPluginPermissions(_, request) => {
                            Some(request.request_id)
                        },
                        _ => None,
                    })
                    .unwrap(),
                status,
                cache_path: Some(self.directory.path().join("permissions.kdl")),
            })
            .unwrap();
    }
}
