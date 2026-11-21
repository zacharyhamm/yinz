//! Exercise screen serialization, server publication, the OS writer and the ordinary ANSI client.
use super::*;
use interprocess::local_socket::traits::Listener;
use yinz_ipc::ipc::{ClientToServerMsg, IpcReceiverWithContext};
use yinz_types::{
    data::{Event, NotificationDecision, NotificationDelivery},
    diagnostics::ErrorContext,
    internal_ids::TabId,
    pane_size::Size,
};

const SIZE: Size = Size { cols: 80, rows: 24 };
const WATCH_SIZE: Size = Size { cols: 51, rows: 12 };
const TERMINAL: TerminalId = TerminalId::new(1);

struct PublicationScreen {
    sender: SenderWithContext<ScreenInstruction>,
    output: channels::Receiver<(ServerInstruction, ErrorContext)>,
    plugins: channels::Receiver<(PluginInstruction, ErrorContext)>,
    thread: std::thread::JoinHandle<Result<()>>,
}

impl PublicationScreen {
    fn new(owners: envs::SessionState, client: ClientId) -> Self {
        let (tx, rx) = channels::unbounded();
        let sender = SenderWithContext::new(tx);
        let (tx, output) = channels::unbounded();
        let (plugin_tx, plugins) = channels::unbounded();
        let mut bus = Bus::new(
            vec![rx],
            Some(&sender),
            None,
            Some(&SenderWithContext::new(plugin_tx)),
            Some(&SenderWithContext::new(tx)),
            None,
            None,
            Some(Box::new(yinz_server_os::get_server_os_input().unwrap())),
        )
        .should_silently_fail();
        bus.senders.session = owners;
        let thread = std::thread::spawn(move || {
            screen_thread_main(
                None,
                bus,
                None,
                ClientAttributes {
                    size: SIZE,
                    ..Default::default()
                },
                Config::default(),
                false,
                None,
                None,
                None,
                Box::default(),
            )
        });
        let screen = Self {
            sender,
            output,
            plugins,
            thread,
        };
        screen.send(ScreenInstruction::RecomputeTabSize(client, SIZE));
        screen.send(ScreenInstruction::NewTab(
            None,
            None,
            Some(Default::default()),
            vec![],
            None,
            (None, None),
            None,
            false,
            true,
            client,
            None,
        ));
        screen.send(ScreenInstruction::ApplyLayout(
            Default::default(),
            vec![],
            vec![(TERMINAL, None)],
            vec![],
            vec![],
            HashMap::new(),
            TabId::new(0),
            true,
            client,
            None,
            None,
        ));
        screen
    }

    fn send(&self, instruction: ScreenInstruction) {
        self.sender.send(instruction).unwrap();
    }

    fn barrier(&self) {
        let (tx, rx) = channels::bounded(1);
        self.send(ScreenInstruction::ListNotifications {
            response_channel: tx,
        });
        rx.recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
    }

    fn publications(&self) -> Vec<ServerInstruction> {
        self.barrier();
        self.output
            .try_iter()
            .map(|(instruction, _)| instruction)
            .filter(|i| {
                matches!(
                    i,
                    ServerInstruction::Render(Some(_))
                        | ServerInstruction::RepaintComplete(_)
                        | ServerInstruction::ClipboardUpdate(_)
                        | ServerInstruction::DesktopNotification(_)
                )
            })
            .collect()
    }

    fn paint(&self, owner: ClientId, marker: &str) {
        self.send(ScreenInstruction::PtyBytes(
            TERMINAL,
            format!("\x1b[2J\x1b[H{marker}").into_bytes(),
        ));
        self.send(ScreenInstruction::RequestRepaint(owner));
        self.send(ScreenInstruction::RenderToClients);
    }

    fn side_effects(&self, marker: &str) {
        use base64::Engine;
        let clip = base64::engine::general_purpose::STANDARD.encode(marker);
        self.send(ScreenInstruction::PtyBytes(
            TERMINAL,
            format!("\x1b]52;c;{clip}\x07").into_bytes(),
        ));
        self.send(ScreenInstruction::ForwardDesktopNotifications {
            pane_id: TERMINAL,
            notifications: vec![yinz_terminal::panes::grid::PendingNotification::Osc9 {
                body: format!("{marker}-direct"),
            }],
        });
        let plugin = PluginId::new(52);
        let instance = Default::default();
        self.send(ScreenInstruction::NotificationPolicy(Some(plugin), None));
        self.send(ScreenInstruction::Notify(
            format!("{marker}-policy"),
            None,
            None,
            None,
        ));
        // Observe the policy request before returning the replacement decision.
        let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
        let id = loop {
            let (instruction, _) = self.plugins.recv_deadline(deadline).unwrap();
            if let PluginInstruction::Update(events) = instruction
                && let Some(id) = events.iter().find_map(|(_, _, event)| match event {
                    Event::NotificationRequested(n) if n.body == format!("{marker}-policy") => {
                        Some(n.id)
                    },
                    _ => None,
                })
            {
                break id;
            }
        };
        self.send(ScreenInstruction::BindNotificationRequest {
            id,
            plugin_id: plugin,
            instance_id: instance,
        });
        self.send(ScreenInstruction::ResolveNotification {
            id,
            plugin_id: plugin,
            instance_id: instance,
            decision: NotificationDecision::Replace {
                title: None,
                body: None,
                delivery: Some(NotificationDelivery::Desktop),
            },
        });
        self.send(ScreenInstruction::NotificationPolicy(None, None));
    }
}

#[test]
fn attachment_generation_held_screen_output_cannot_ready_ansi_replacement() {
    let state = Arc::new(RwLock::new(SessionState::new()));
    let data = Arc::new(RwLock::new(None));
    let mut os: Box<dyn ServerOsApi> = Box::new(yinz_server_os::get_server_os_input().unwrap());
    let allocate = |reuse: Option<ClientId>| {
        let mut state = state.write().unwrap();
        if let Some(previous) = reuse {
            state.next_client_id = Some(previous.get());
        }
        state.new_client().unwrap()
    };
    let old = allocate(None);
    let sibling = allocate(None);
    let watcher = allocate(None);
    let old_watcher = allocate(None);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("destination.sock");
    let listener = yinz_ipc::consts::ipc_bind(&path).unwrap();
    let connect = |os: &mut Box<dyn ServerOsApi>, owner| {
        let stream = yinz_ipc::consts::ipc_connect(&path).unwrap();
        let router = os.new_client(owner, listener.accept().unwrap()).unwrap();
        let reader = IpcReceiverWithContext::<ServerToClientMsg>::new(stream);
        reader
            .set_recv_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        (reader, router)
    };
    let _old_socket = connect(&mut os, old);
    let (mut sibling_reader, _sibling_socket) = connect(&mut os, sibling);
    let (mut watcher_reader, _watcher_socket) = connect(&mut os, watcher);
    let _old_watcher_socket = connect(&mut os, old_watcher);
    let screen = PublicationScreen::new(state.read().unwrap().attachments.clone(), old);
    screen.send(ScreenInstruction::AddClient(sibling, SIZE, None, None));
    screen.send(ScreenInstruction::SetFollowedClient(old));
    for id in [watcher, old_watcher] {
        screen.send(ScreenInstruction::AddWatcherClient(id, WATCH_SIZE));
    }
    screen.publications();
    screen.side_effects("OLD");
    screen.paint(old, "old-view");
    let held = screen.publications();
    let frame = |batch: &[ServerInstruction], owner| {
        batch
            .iter()
            .find_map(|instruction| match instruction {
                ServerInstruction::Render(Some(bytes)) => bytes.get(&owner).cloned(),
                _ => None,
            })
            .unwrap()
    };
    let old_bytes = frame(&held, old);
    let sibling_bytes = frame(&held, sibling);
    let watcher_bytes = frame(&held, watcher);
    assert!(old_bytes.contains("old-view") && sibling_bytes.contains("old-view"));
    assert!(watcher_bytes.contains("old-view"));
    assert_ne!(watcher_bytes, old_bytes);
    assert_eq!(frame(&held, old_watcher), watcher_bytes);
    let effects: Vec<_> = held
        .iter()
        .filter_map(|i| match i {
            ServerInstruction::DesktopNotification(batch)
            | ServerInstruction::ClipboardUpdate(batch) => Some(&batch[&old]),
            _ => None,
        })
        .collect();
    assert!(effects.iter().any(|b| b.contains("]52;")));
    for marker in ["OLD-direct", "OLD-policy"] {
        assert!(effects
            .iter()
            .any(|b| b.contains("]9;") && b.contains(marker)));
    }
    assert!(held
        .iter()
        .any(|i| matches!(i,ServerInstruction::RepaintComplete(client) if *client==old)));

    screen.send(ScreenInstruction::RemoveClient(sibling));
    screen.send(ScreenInstruction::RemoveClient(old));
    screen.send(ScreenInstruction::PtyBytes(
        TERMINAL,
        b"\r\nretained-source".to_vec(),
    ));
    screen.send(ScreenInstruction::RenderToClients);
    let retained = screen.publications();
    let retained_bytes = frame(&retained, watcher);
    assert!(retained_bytes.contains("retained-source"));

    let replacement = allocate(Some(old));
    let new_watcher = allocate(Some(old_watcher));
    let (mut reused_reader, _reused_socket) = connect(&mut os, new_watcher);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let preparing =
        std::thread::spawn(move || yinz_client::exercise_test_destination(&path, deadline));
    let mut router = os
        .new_client(replacement, listener.accept().unwrap())
        .unwrap();
    router
        .set_recv_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    assert!(matches!(
        router.recv_client_msg().msg(),
        Some((ClientToServerMsg::AttachClient { .. }, _))
    ));

    let mut pending = HashMap::from([(replacement, std::time::Instant::now())]);
    let publish = |batch: &[ServerInstruction],
                   os: &mut Box<dyn ServerOsApi>,
                   pending: &mut HashMap<_, _>| {
        for i in batch {
            publish_screen_output(i.clone(), os, &state, &data, pending).unwrap();
        }
    };
    publish(&held, &mut os, &mut pending);
    publish(&retained, &mut os, &mut pending);
    assert!(pending.contains_key(&replacement));
    for owner in [new_watcher, replacement] {
        os.send_to_client(owner, ServerToClientMsg::ConfigFileUpdated)
            .unwrap();
    }
    assert!(matches!(
        reused_reader.recv_server_msg().msg(),
        Some((ServerToClientMsg::ConfigFileUpdated, _))
    ));
    let read_frame = |reader: &mut IpcReceiverWithContext<ServerToClientMsg>| loop {
        if let ServerToClientMsg::Render { content } = reader.recv_server_msg().msg().unwrap().0 {
            break content;
        }
    };
    assert_eq!(read_frame(&mut sibling_reader), sibling_bytes);
    assert_eq!(read_frame(&mut watcher_reader), watcher_bytes);
    assert_eq!(read_frame(&mut watcher_reader), retained_bytes);

    screen.send(ScreenInstruction::RemoveWatcherClient(old_watcher));
    screen.send(ScreenInstruction::AddWatcherClient(new_watcher, WATCH_SIZE));
    screen.send(ScreenInstruction::AddClient(replacement, SIZE, None, None));
    screen.send(ScreenInstruction::SetClientHostTerminalEnv(
        replacement,
        BTreeMap::from([("TERM".into(), "xterm-kitty".into())]),
    ));
    screen.paint(replacement, "new-view");
    let startup = screen.publications();
    for i in startup
        .iter()
        .filter(|i| matches!(i, ServerInstruction::Render(Some(_))))
    {
        publish(std::slice::from_ref(i), &mut os, &mut pending);
    }
    assert!(pending.is_empty());
    assert!(
        matches!(router.recv_client_msg().msg(), Some((ClientToServerMsg::Action {action: Action::WriteChars {chars},..},_)) if chars=="committed")
    );
    assert!(matches!(
        router.recv_client_msg().msg(),
        Some((ClientToServerMsg::RequestRepaint, _))
    ));
    publish(&held, &mut os, &mut pending);
    os.send_to_client(replacement, ServerToClientMsg::ConfigFileUpdated)
        .unwrap();
    screen.side_effects("NEW");
    screen.paint(replacement, "restored-view");
    publish(&screen.publications(), &mut os, &mut pending);
    let display = String::from_utf8(preparing.join().unwrap().unwrap()).unwrap();
    for marker in [
        "new-view",
        "restored-view",
        "]99;",
        "NEW-direct",
        "NEW-policy",
        "TkVX",
    ] {
        assert!(display.contains(marker), "missing {marker}: {display:?}");
    }
    for marker in [
        "old-view",
        "retained-source",
        "OLD-direct",
        "OLD-policy",
        "T0xE",
    ] {
        assert!(!display.contains(marker));
    }
    screen.send(ScreenInstruction::Exit);
    screen.thread.join().unwrap().unwrap();
}
