use super::*;
use yinz_bus::command_palette::AgentAction;
use yinz_types::data::OverlayPlacement;
use yinz_types::internal_ids::TabNumber;
use yinz_types::keys::{BareKey, KeyWithModifier};

const CLIENT: ClientId = ClientId::new(1);
const SIZE: Size = Size { cols: 80, rows: 24 };
const SHELL: PaneId = PaneId::Terminal(TerminalId::new(1));
const MOTHER: PaneId = PaneId::Terminal(TerminalId::new(2));

fn screen() -> Screen {
    let mut screen = create_non_mirrored_screen(SIZE);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
}

/// An overlay terminal on the client's tab, as the plugin host places a mother: unfocused.
fn overlay(screen: &mut Screen, pane: PaneId) {
    screen
        .get_active_tab_mut(CLIENT)
        .unwrap()
        .new_pane(
            pane,
            None,
            None,
            false,
            false,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(CLIENT),
            None,
        )
        .unwrap();
}

fn screen_with_mother() -> Screen {
    let mut screen = screen();
    overlay(&mut screen, MOTHER);
    screen.mother_spawned(MOTHER).unwrap();
    screen
}

#[test]
fn failed_mother_supervisor_requires_resume_and_recovers_in_the_same_pane() {
    use crate::screen::agent_control::Reply;
    use std::sync::mpsc;
    use std::time::Duration;
    use yinz_types::agent::{control::AgentOutcome, mother::MotherRequest};

    let mut screen = screen_with_mother();
    screen.session_name = format!("mother-recovery-{}", uuid::Uuid::new_v4()).into();
    let work = yinz_config::project_config::bubblewrap::mother_directory(
        &yinz_runtime::consts::YINZ_CACHE_DIR,
        screen.session_name.as_str(),
    )
    .unwrap();
    let memory = work.parent().unwrap().join("supervision.json");
    std::fs::create_dir(&memory).unwrap();
    let (instructions, received) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(instructions));
    screen.sync_mother_supervisor();
    let generation = screen
        .mother_supervisor
        .as_ref()
        .unwrap()
        .generation
        .clone();
    let (reply, response) = mpsc::channel();
    screen.mother_command(MotherRequest::Inbox, Reply::Host(reply));
    let result = response.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(result.outcome, AgentOutcome::Rejected);
    assert!(result.message.unwrap().contains("resume"));
    let (instruction, _) = received.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        matches!(instruction, ScreenInstruction::MotherStatus(_, status) if status.state == "degraded")
    );
    for _ in 0..3 {
        screen.sync_mother_supervisor();
        assert_eq!(
            screen.mother_supervisor.as_ref().unwrap().generation,
            generation
        );
        assert_eq!(screen.mother_status.as_ref().unwrap().state, "degraded");
    }

    // A failed explicit retry still replies instead of losing the queued command.
    let (reply, response) = mpsc::channel();
    screen.mother_command(MotherRequest::Resume, Reply::Host(reply));
    assert_eq!(
        response
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .outcome,
        AgentOutcome::Rejected
    );
    let generation = screen
        .mother_supervisor
        .as_ref()
        .unwrap()
        .generation
        .clone();
    std::fs::remove_dir(&memory).unwrap();
    let (reply, response) = mpsc::channel();
    screen.mother_command(MotherRequest::Resume, Reply::Host(reply));
    assert_eq!(
        response
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .outcome,
        AgentOutcome::Ok
    );
    assert_ne!(
        screen.mother_supervisor.as_ref().unwrap().generation,
        generation
    );
    assert_eq!(screen.live_mother(), Some(MOTHER));
    let (reply, response) = mpsc::channel();
    screen.mother_command(
        MotherRequest::Checkpoint {
            expect_rev: Some(0),
            value: Some(serde_json::json!({"recovered":true})),
        },
        Reply::Host(reply),
    );
    assert_eq!(
        response
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .outcome,
        AgentOutcome::Ok
    );
    screen.mother_supervisor = None;
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&memory).unwrap()).unwrap();
    assert_eq!(saved["checkpoint"]["recovered"], true);
    std::fs::remove_dir_all(work.parent().unwrap()).unwrap();
}

#[test]
fn mother_overview_only_needs_input_while_waiting_on_a_person() {
    use crate::screen::agent_control::Reply;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    use yinz_types::agent::{control::AgentOutcome, mother::MotherRequest, AgentState};

    for (state, wait_needs_a_person, expected) in [
        (AgentState::Busy, true, false),
        (AgentState::Idle, true, false),
        (AgentState::Waiting, false, false),
        (AgentState::Waiting, true, true),
    ] {
        let mut screen = screen_with_mother();
        screen.session_name = format!("mother-attention-{}", uuid::Uuid::new_v4()).into();
        screen.get_active_tab_mut(CLIENT).unwrap().set_pane_agent(
            MOTHER,
            Some(yinz_types::agent::AgentInfo {
                kind: "codex".into(),
                state,
                source: yinz_types::agent::AgentSource::Reported,
                detail: None,
                waiting_on: None,
                wait_needs_a_person,
                cwd: None,
                session_id: Some("mother-conversation".into()),
                run_id: None,
            }),
        );
        let (instructions, received) = channels::unbounded();
        screen.bus.senders.to_screen = Some(SenderWithContext::new(instructions));
        screen.sync_mother_supervisor();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (instruction, _) = received
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if let ScreenInstruction::AgentControlReply(request, reply) = instruction {
                screen.agent_control_with_reply(request, Reply::Host(reply));
                break;
            }
        }
        let overview = loop {
            assert!(
                Instant::now() < deadline,
                "Mother identity was not observed"
            );
            let (reply, response) = mpsc::channel();
            screen.mother_command(MotherRequest::Overview, Reply::Host(reply));
            let response = response
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            assert_eq!(response.outcome, AgentOutcome::Ok, "{:?}", response.message);
            let overview: serde_json::Value =
                serde_json::from_str(&response.output.unwrap()).unwrap();
            if overview["mother"]["conversation_id"] == "mother-conversation" {
                break overview;
            }
        };
        let work = yinz_config::project_config::bubblewrap::mother_directory(
            &yinz_runtime::consts::YINZ_CACHE_DIR,
            screen.session_name.as_str(),
        )
        .unwrap();
        screen.mother_supervisor = None;
        std::fs::remove_dir_all(work.parent().unwrap()).unwrap();
        assert_eq!(overview["mother"]["state"], serde_json::json!(state));
        assert_eq!(overview["mother"]["needs_input"], expected, "{state}");
    }
}

#[test]
fn renamed_mother_reconnects_and_reloads_its_checkpoint() {
    use crate::screen::agent_control::Reply;
    use interprocess::local_socket::prelude::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext};
    use yinz_types::agent::{control::AgentOutcome, mother::MotherRequest};

    let mut screen = screen_with_mother();
    let old = format!("mother-old-{}", uuid::Uuid::new_v4());
    let new = format!("mother-new-{}", uuid::Uuid::new_v4());
    screen.session_name = old.clone().into();
    screen.get_active_tab_mut(CLIENT).unwrap().set_pane_agent(
        SHELL,
        Some(yinz_types::agent::AgentInfo {
            kind: "codex".into(),
            state: yinz_types::agent::AgentState::Idle,
            source: yinz_types::agent::AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: Some("worker-conversation".into()),
            run_id: None,
        }),
    );
    let os = FakeInputOutput::default();
    let messages = os.server_to_client_messages.clone();
    screen.bus.os_input = Some(Box::new(os));
    let socket_dir = &*yinz_runtime::consts::YINZ_SOCK_DIR;
    std::fs::create_dir_all(socket_dir).unwrap();
    let listener = yinz_ipc::consts::ipc_bind(&socket_dir.join(&old)).unwrap();
    let (connections, accepted) = mpsc::channel();
    let acceptor = std::thread::spawn(move || {
        for _ in 0..5 {
            if connections.send(listener.accept().unwrap()).is_err() {
                return;
            }
        }
    });
    let connect = || {
        let sender = IpcSenderWithContext::<yinz_ipc::ipc::ServerToClientMsg>::new(
            accepted.recv_timeout(Duration::from_secs(10)).unwrap(),
        );
        let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
        receiver
            .set_recv_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let IpcRecv::Msg(ClientToServerMsg::SubscribeToEvents { options, .. }, _) =
            receiver.recv_client_msg()
        else {
            panic!("expected a lifecycle subscription");
        };
        (sender, receiver, options)
    };
    let command = |screen: &mut Screen, request| {
        let (reply, response) = mpsc::channel();
        screen.mother_command(request, Reply::Host(reply));
        let response = response.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(response.outcome, AgentOutcome::Ok, "{:?}", response.message);
        serde_json::from_str::<serde_json::Value>(&response.output.unwrap()).unwrap()
    };
    let snapshot =
        |screen: &mut Screen, sender: &mut IpcSenderWithContext<ServerToClientMsg>, options| {
            let watcher = ClientId::new(99);
            screen.subscribe_agent_events(watcher, options);
            let events = messages.lock().unwrap().remove(&watcher).unwrap();
            let facts: Vec<serde_json::Value> = events
                .iter()
                .filter_map(|event| match event {
                    ServerToClientMsg::EventUpdate { event_json, .. } => {
                        Some(serde_json::from_str(event_json).unwrap())
                    },
                    _ => None,
                })
                .collect();
            assert_eq!(facts.first().unwrap()["event_type"], "snapshot_begin");
            assert!(facts.iter().any(|fact| fact["event_type"] == "snapshot"));
            let ready = facts.last().unwrap();
            assert_eq!(ready["event_type"], "ready");
            let cursor = ready["cursor"].as_str().unwrap().to_owned();
            for event in events {
                sender.send_server_msg(event).unwrap();
            }
            cursor
        };
    let (instructions, received) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(instructions));
    screen.sync_mother_supervisor();
    let mut initial = connect();
    assert!(initial.2.cursor.is_none());
    command(&mut screen, MotherRequest::Pause);
    let overview = command(&mut screen, MotherRequest::Overview);
    assert_eq!(overview["mother"]["pane_id"], MOTHER.to_string());
    assert_eq!(overview["mother"]["yinz_session_name"], old);
    assert_eq!(
        overview["mother"]["host_id"],
        serde_json::json!(yinz_runtime::host_identity::host_id())
    );
    assert!(overview["mother"]["conversation_id"].is_null());
    command(
        &mut screen,
        MotherRequest::Checkpoint {
            expect_rev: Some(0),
            value: Some(serde_json::json!({"assignment":"preserved"})),
        },
    );
    let old_cursor = snapshot(&mut screen, &mut initial.0, initial.2.clone());
    let ready = || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (instruction, _) = received
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if matches!(instruction, ScreenInstruction::MotherStatus(_, status) if status.state == "paused")
            {
                break;
            }
        }
    };
    ready();
    assert_eq!(
        command(&mut screen, MotherRequest::Inbox)["cursor"],
        old_cursor
    );
    let generation = screen
        .mother_supervisor
        .as_ref()
        .unwrap()
        .generation
        .clone();
    screen.rename_session(old.clone().into(), CLIENT).unwrap();
    assert_eq!(
        screen.mother_supervisor.as_ref().unwrap().generation,
        generation
    );
    screen.rename_session(new.clone().into(), CLIENT).unwrap();
    assert_eq!(screen.session_name.as_str(), new);
    assert_ne!(
        screen.mother_supervisor.as_ref().unwrap().generation,
        generation
    );
    assert!(!socket_dir.join(&old).exists());
    let mut renamed = connect();
    assert!(renamed.2.cursor.is_none());
    let work = yinz_config::project_config::bubblewrap::mother_directory(
        &yinz_runtime::consts::YINZ_CACHE_DIR,
        &new,
    )
    .unwrap();
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(work.parent().unwrap().join("supervision.json")).unwrap(),
    )
    .unwrap();
    assert!(
        saved["cursor"].is_null(),
        "reset must be durable before connecting"
    );
    let new_cursor = snapshot(&mut screen, &mut renamed.0, renamed.2.clone());
    ready();
    assert_ne!(old_cursor, new_cursor);
    assert_eq!(
        command(&mut screen, MotherRequest::Inbox)["cursor"],
        new_cursor
    );
    command(
        &mut screen,
        MotherRequest::Configure {
            panes: vec![],
            projects: vec![],
            kinds: vec!["codex".into()],
            lifecycle_types: vec!["turn_failed".into()],
        },
    );
    let configured = connect();
    assert_eq!(configured.2.kinds, vec!["codex"]);
    assert!(configured.2.lifecycle_types.is_empty());
    drop(configured);
    let reconnected = connect();
    assert_eq!(
        command(
            &mut screen,
            MotherRequest::Checkpoint {
                expect_rev: None,
                value: None
            }
        )["value"]["assignment"],
        "preserved"
    );
    screen.mother_supervisor = None;
    screen.sync_mother_supervisor();
    let relaunched = connect();
    assert_eq!(
        command(
            &mut screen,
            MotherRequest::Checkpoint {
                expect_rev: None,
                value: None
            }
        )["revision"],
        1
    );
    screen.mother_supervisor = None;
    drop((initial, renamed, reconnected, relaunched));
    acceptor.join().unwrap();
    std::fs::remove_file(socket_dir.join(&new)).unwrap();
    for session in [old, new] {
        let work = yinz_config::project_config::bubblewrap::mother_directory(
            &yinz_runtime::consts::YINZ_CACHE_DIR,
            &session,
        )
        .unwrap();
        std::fs::remove_dir_all(work.parent().unwrap()).unwrap();
    }
}

fn mother_tab(screen: &Screen) -> TabId {
    screen.tab_containing_overlay(MOTHER).unwrap().id
}

fn hidden(screen: &Screen) -> bool {
    screen
        .tab_containing_overlay(MOTHER)
        .unwrap()
        .overlay_carry_for(MOTHER)
        .hidden
}

fn reported_mother(screen: &Screen) -> Option<TerminalId> {
    screen.peer_sessions_cache[&screen.session_name].mother
}

fn hold(screen: &mut Screen, pane: PaneId) {
    screen.tab_containing_pane_mut(pane).unwrap().hold_pane(
        pane,
        Some(0),
        false,
        yinz_types::input::command::RunCommand {
            command: "/usr/bin/true".into(),
            ..Default::default()
        },
    );
    screen.log_and_report_session_state().unwrap();
}

fn palette_key(screen: &mut Screen, key: BareKey) {
    assert!(screen.command_palette_handles_input(
        &Some(KeyWithModifier::new(key)),
        &[],
        false,
        CLIENT
    ));
}

#[test]
fn toggle_without_a_mother_opens_the_palette_at_the_agents_to_launch() {
    use yinz_types::agent::provider::{self, AgentProvider};
    provider::register(
        98982,
        AgentProvider {
            kind: "mother-test-choice".into(),
            executable: "mother-test-choice".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let mut screen = screen();
    let (plugin_sender, plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_sender));
    let renders = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);

    screen.toggle_mother(CLIENT).unwrap();
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(frame.contains("Launch mother"), "{frame}");
    for kind in ["claude", "codex", "pi"] {
        assert!(frame.contains(kind), "{frame}");
    }
    for kind in ["muse", "agy", "cursor", "mother-test-choice"] {
        assert!(!frame.contains(kind), "{frame}");
    }
    assert!(!frame.contains("New pane"), "{frame}");
    provider::unregister_owner(98982);

    assert!(screen.command_palette_handles_input(&None, b"codex", false, CLIENT));
    palette_key(&mut screen, BareKey::Enter);
    let request = plugins
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            PluginInstruction::PaletteAgent(request) => Some(*request),
            _ => None,
        })
        .expect("the palette asked the plugin host to launch");
    assert!(matches!(&request.action, AgentAction::Mother { kind } if kind == "codex"));
    // A launch in flight cannot be submitted twice.
    palette_key(&mut screen, BareKey::Enter);
    assert!(!plugins
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
    assert!(screen.mother.is_none());
}

#[test]
fn toggle_focuses_then_hides_then_shows_mother_again() {
    let mut screen = screen_with_mother();
    assert_eq!(reported_mother(&screen), Some(TerminalId::new(2)));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(SHELL));

    // Visible but without the keyboard: the toggle hands it over.
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert!(!hidden(&screen));

    screen.toggle_mother(CLIENT).unwrap();
    assert!(hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(SHELL));
    assert_eq!(reported_mother(&screen), Some(TerminalId::new(2)));

    screen.toggle_mother(CLIENT).unwrap();
    assert!(!hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert!(!screen.command_palettes.contains_key(&CLIENT));
}

#[test]
fn a_visible_mother_follows_tab_switches_and_keeps_the_keyboard_it_had() {
    let mut screen = screen_with_mother();

    // Shown without focus: it comes along, and the new tab's own pane keeps the keyboard.
    new_tab(&mut screen, 3, TabId::new(1));
    assert_eq!(mother_tab(&screen), TabId::new(1));
    assert!(!hidden(&screen));
    assert_eq!(
        screen.get_active_pane_id(&CLIENT),
        Some(PaneId::Terminal(TerminalId::new(3)))
    );

    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(0));
    assert!(!hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn a_hidden_mother_stays_behind_until_a_toggle_fetches_it() {
    let mut screen = screen_with_mother();
    screen.toggle_mother(CLIENT).unwrap();
    screen.toggle_mother(CLIENT).unwrap();
    assert!(hidden(&screen));

    new_tab(&mut screen, 3, TabId::new(1));
    assert_eq!(mother_tab(&screen), TabId::new(0));
    assert!(hidden(&screen));

    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(1));
    assert!(!hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert_eq!(
        screen.active_tab_ids.get(&CLIENT),
        Some(&TabId::new(1)),
        "fetching mother must not move the client"
    );
}

#[test]
fn mother_survives_a_new_conversation_and_ends_with_its_process() {
    let mut screen = screen_with_mother();
    // `/clear` and a replaced conversation end the agent's lifecycle, not the pane.
    screen.invalidate_agent_lifecycle(MOTHER, "conversation changed");
    screen.log_and_report_session_state().unwrap();
    assert_eq!(reported_mother(&screen), Some(TerminalId::new(2)));
    assert_eq!(screen.live_mother(), Some(MOTHER));

    hold(&mut screen, MOTHER);
    assert_eq!(screen.live_mother(), None);
    assert_eq!(reported_mother(&screen), None);

    // What it left on screen no longer follows the client.
    new_tab(&mut screen, 3, TabId::new(1));
    assert_eq!(mother_tab(&screen), TabId::new(0));

    // The next toggle fetches it to be read or re-run, and offers a new mother over it.
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(1));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert!(screen.command_palettes.contains_key(&CLIENT));
    // Turning the offer down leaves it there, and asking again offers again instead of hiding.
    screen.close_command_palette(CLIENT);
    assert_eq!(screen.mother, Some(TerminalId::new(2)));
    screen.toggle_mother(CLIENT).unwrap();
    assert!(!hidden(&screen));
    assert!(screen.command_palettes.contains_key(&CLIENT));
    assert_eq!(reported_mother(&screen), None);
}

#[test]
fn an_exited_mother_is_not_listed_as_mother_among_the_live_agents() {
    let mut screen = screen_with_mother();
    let agent = yinz_types::agent::AgentInfo {
        kind: "claude".into(),
        state: yinz_types::agent::AgentState::Idle,
        source: yinz_types::agent::AgentSource::Reported,
        detail: None,
        waiting_on: None,
        wait_needs_a_person: false,
        cwd: None,
        session_id: None,
        run_id: None,
    };
    let listed_as_mother = |screen: &Screen| {
        screen
            .live_palette_agents()
            .iter()
            .find(|agent| agent.pane.id == MOTHER)
            .map(|agent| agent.metadata["mother"].clone())
    };
    assert!(screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .set_pane_agent(MOTHER, Some(agent.clone())));
    assert_eq!(listed_as_mother(&screen).as_deref(), Some("true"));

    // Held for a rerun: the pane and its record remain, mother does not.
    screen.tab_containing_pane_mut(MOTHER).unwrap().hold_pane(
        MOTHER,
        Some(0),
        true,
        yinz_types::input::command::RunCommand {
            command: "/usr/bin/true".into(),
            ..Default::default()
        },
    );
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .set_pane_agent(MOTHER, Some(agent));
    assert_eq!(screen.live_mother(), None);
    assert_eq!(listed_as_mother(&screen).as_deref(), Some("false"));
}

#[test]
fn mother_is_forgotten_when_its_pane_closes() {
    let mut screen = screen_with_mother();
    let (pty_sender, _pty) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    screen.close_pane_explicitly(MOTHER, None);
    // Forgotten with the pane, not by whichever report next finds it missing.
    assert!(screen.mother.is_none());
    screen.log_and_report_session_state().unwrap();
    assert_eq!(reported_mother(&screen), None);
}

#[test]
fn a_second_launch_never_replaces_a_live_mother() {
    let mut screen = screen_with_mother();
    let (pty_sender, _pty) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    let late = PaneId::Terminal(TerminalId::new(3));
    drop(place_mother(&mut screen, 3, false));
    assert_eq!(screen.mother, Some(TerminalId::new(2)));
    assert!(screen.tab_containing_pane(late).is_none());

    // An exited mother is replaced, and what it left on screen goes with it.
    hold(&mut screen, MOTHER);
    overlay(&mut screen, late);
    screen.mother_spawned(late).unwrap();
    assert_eq!(screen.mother, Some(TerminalId::new(3)));
    assert!(screen.tab_containing_pane(MOTHER).is_none());
}

#[test]
fn the_palette_offers_launch_or_toggle_and_refuses_a_second_mother() {
    let mut screen = screen();
    let (plugin_sender, plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_sender));
    let renders = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);

    screen.toggle_command_palette(CLIENT);
    assert!(screen.command_palette_handles_input(&None, b"mother", false, CLIENT));
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(frame.contains("Launch mother…"), "{frame}");
    assert!(!frame.contains("Toggle mother"), "{frame}");
    // The picker's candidates were listed before mother appeared; choosing one is refused.
    palette_key(&mut screen, BareKey::Enter);
    overlay(&mut screen, MOTHER);
    screen.mother_spawned(MOTHER).unwrap();
    palette_key(&mut screen, BareKey::Enter);
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(frame.contains("Mother is already running"), "{frame}");
    assert!(!plugins
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PluginInstruction::PaletteAgent(_))));
    screen.close_command_palette(CLIENT);

    let (screen_sender, actions) = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(screen_sender));
    screen.toggle_command_palette(CLIENT);
    assert!(screen.command_palette_handles_input(&None, b"mother", false, CLIENT));
    let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
    assert!(frame.contains("Toggle mother"), "{frame}");
    assert!(!frame.contains("Launch mother"), "{frame}");
    palette_key(&mut screen, BareKey::Enter);
    assert!(actions.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::ActivateCommandPaletteAction {
            action: Action::ToggleMother,
            ..
        }
    )));
}

const OTHER: ClientId = ClientId::new(2);

#[test]
fn another_client_cannot_carry_or_hide_a_focused_mother() {
    for stand_in in [None, Some(PaneId::Terminal(TerminalId::new(4)))] {
        let mut screen = screen_with_mother();
        screen.add_client(OTHER).unwrap();
        screen.toggle_mother(OTHER).unwrap();
        if let Some(stand_in) = stand_in {
            screen
                .tab_containing_pane_mut(MOTHER)
                .unwrap()
                .replace_pane_with_editor_pane(stand_in, MOTHER)
                .unwrap();
        }
        let visible = stand_in.unwrap_or(MOTHER);
        assert_eq!(screen.get_active_pane_id(&OTHER), Some(visible));

        // Sharing focus is allowed, hiding the other client's input target is not.
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(screen.get_active_pane_id(&OTHER), Some(visible));
        assert!(
            !screen
                .tab_containing_overlay(visible)
                .unwrap()
                .overlay_carry_for(visible)
                .hidden
        );

        // From another tab the toggle cannot take it either, so it goes to it.
        new_tab(&mut screen, 3, TabId::new(1));
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(
            screen.tab_containing_overlay(visible).unwrap().id,
            TabId::new(0)
        );
        assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(0));
        assert_eq!(screen.get_active_pane_id(&OTHER), Some(visible));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));

        let (sender, writes) = channels::unbounded();
        screen
            .tab_containing_overlay_mut(visible)
            .unwrap()
            .senders
            .to_pty_writer = Some(SenderWithContext::new(sender));
        screen
            .write_character(
                None,
                b"still typing".to_vec(),
                false,
                OTHER,
                &HashMap::new(),
                None,
            )
            .unwrap();
        let PaneId::Terminal(terminal) = visible else {
            unreachable!()
        };
        assert!(writes.try_iter().any(|(instruction, _)| matches!(instruction,
            PtyWriteInstruction::Write(bytes, id, _) if id == terminal && bytes == b"still typing"
        )));

        // Leaving again does not take it along, until the other client lets go of it.
        screen.go_to_tab(TabNumber::new(2), CLIENT).unwrap();
        assert_eq!(
            screen.tab_containing_overlay(visible).unwrap().id,
            TabId::new(0)
        );
        screen
            .focus_pane_with_id(SHELL, false, false, OTHER)
            .unwrap();
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(
            screen.tab_containing_overlay(visible).unwrap().id,
            TabId::new(1)
        );
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
    }
}

#[test]
fn clients_sharing_mother_let_go_one_at_a_time_and_the_last_one_hides_it() {
    let mut screen = screen_with_mother();
    screen.add_client(OTHER).unwrap();
    screen.toggle_mother(OTHER).unwrap();
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));

    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(SHELL));
    assert_eq!(screen.get_active_pane_id(&OTHER), Some(MOTHER));
    assert!(!hidden(&screen));

    screen.toggle_mother(OTHER).unwrap();
    assert!(hidden(&screen));
    screen.toggle_mother(CLIENT).unwrap();
    assert!(!hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn a_disconnected_client_does_not_pin_mother_to_its_tab() {
    let mut screen = screen_with_mother();
    screen.add_client(OTHER).unwrap();
    screen.toggle_mother(OTHER).unwrap();
    new_tab(&mut screen, 3, TabId::new(1));
    assert_eq!(mother_tab(&screen), TabId::new(0));
    screen.remove_client(OTHER).unwrap();
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(1));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn mirrored_clients_leaving_together_do_not_block_mother() {
    let mut screen = create_new_screen(SIZE, false, false);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 3, TabId::new(1));
    screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
    overlay(&mut screen, MOTHER);
    screen.mother_spawned(MOTHER).unwrap();
    screen.add_client(OTHER).unwrap();
    screen.toggle_mother(CLIENT).unwrap();
    screen.toggle_mother(OTHER).unwrap();
    screen.go_to_tab(TabNumber::new(2), CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(1));
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
    assert_eq!(screen.active_tab_ids[&OTHER], TabId::new(1));
}

#[test]
fn invalid_mother_focus_is_non_fatal_and_later_toggles_work() {
    let mut screen = screen_with_mother();
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .set_pane_selectable(MOTHER, false);
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(SHELL));
    assert_eq!(mother_tab(&screen), TabId::new(0));
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .set_pane_selectable(MOTHER, true);
    screen.active_tab_ids.insert(CLIENT, TabId::new(99));
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(0));
    screen.active_tab_ids.remove(&CLIENT);
    screen.toggle_mother(CLIENT).unwrap();
    screen.active_tab_ids.insert(CLIENT, TabId::new(0));
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn mothers_replacements_toggle_travel_and_restore_as_overlays() {
    let editor = PaneId::Terminal(TerminalId::new(4));
    let plugin = PaneId::Plugin(yinz_types::internal_ids::PluginId::new(5));
    for replacements in [vec![editor], vec![plugin], vec![editor, plugin]] {
        let mut screen = screen_with_mother();
        let (sender, _plugins) = channels::unbounded();
        screen
            .tab_containing_pane_mut(MOTHER)
            .unwrap()
            .senders
            .to_plugin = Some(SenderWithContext::new(sender));
        screen.toggle_mother(CLIENT).unwrap();
        let mut previous = MOTHER;
        for replacement in &replacements {
            let tab = screen.tab_containing_pane_mut(previous).unwrap();
            if *replacement == editor {
                tab.replace_pane_with_editor_pane(*replacement, previous)
                    .unwrap();
            } else {
                tab.suppress_pane_and_replace_with_pid(
                    previous,
                    *replacement,
                    false,
                    None,
                    None,
                    None,
                )
                .unwrap();
            }
            previous = *replacement;
        }
        let visible = previous;
        assert_eq!(screen.live_mother(), Some(MOTHER));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
        screen.toggle_mother(CLIENT).unwrap();
        assert!(
            screen
                .tab_containing_overlay(visible)
                .unwrap()
                .overlay_carry_for(visible)
                .hidden
        );
        new_tab(&mut screen, 3, TabId::new(1));
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
        screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
        assert_eq!(
            screen.tab_containing_overlay(visible).unwrap().id,
            TabId::new(0)
        );
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
        let tab = screen.tab_containing_pane(MOTHER).unwrap();
        assert!(!tab.pane_id_is_floating(&MOTHER));
        assert_eq!(tab.get_suppressed_panes().count(), replacements.len());
        for replacement in replacements.iter().rev() {
            screen
                .tab_containing_pane_mut(*replacement)
                .unwrap()
                .close_pane(*replacement, false, None);
        }
        assert_eq!(mother_tab(&screen), TabId::new(0));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
        assert_eq!(
            screen
                .tab_containing_pane(MOTHER)
                .unwrap()
                .get_suppressed_panes()
                .count(),
            0
        );
        screen.toggle_mother(CLIENT).unwrap();
        assert!(hidden(&screen));
        screen.toggle_mother(CLIENT).unwrap();
        screen.go_to_tab(TabNumber::new(2), CLIENT).unwrap();
        assert_eq!(mother_tab(&screen), TabId::new(1));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    }
}

#[test]
fn a_toggle_puts_a_parked_mother_back_on_the_surface() {
    let mut screen = screen_with_mother();
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .suppress_pane(MOTHER, Some(CLIENT));
    screen.toggle_mother(CLIENT).unwrap();
    let tab = screen.tab_containing_pane(MOTHER).unwrap();
    assert_eq!(tab.get_suppressed_panes().count(), 0);
    assert!(tab.pane_is_overlay(MOTHER));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert_eq!(screen.live_mother(), Some(MOTHER));
}

#[test]
fn a_toggle_restores_a_mother_that_is_no_longer_an_overlay() {
    for float in [true, false] {
        let mut screen = screen_with_mother();
        // Parked and then fetched by an ordinary focus request, mother comes back as a
        // floating or tiled pane.
        screen
            .tab_containing_pane_mut(MOTHER)
            .unwrap()
            .suppress_pane(MOTHER, Some(CLIENT));
        screen
            .focus_pane_with_id(MOTHER, float, false, CLIENT)
            .unwrap();
        let tab = screen.tab_containing_pane(MOTHER).unwrap();
        assert!(!tab.pane_is_overlay(MOTHER));
        assert_eq!(tab.pane_id_is_floating(&MOTHER), float);
        new_tab(&mut screen, 3, TabId::new(1));

        // The toggle brings it to the client as the overlay it was launched as.
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
        assert_eq!(mother_tab(&screen), TabId::new(1));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
        assert_eq!(screen.live_mother(), Some(MOTHER));
        screen.toggle_mother(CLIENT).unwrap();
        assert!(hidden(&screen));
        assert_eq!(
            screen.get_active_pane_id(&CLIENT),
            Some(PaneId::Terminal(TerminalId::new(3)))
        );
    }
}

#[test]
fn mother_stays_with_a_client_still_looking_at_it() {
    let mut screen = screen_with_mother();
    screen.add_client(OTHER).unwrap();
    assert_eq!(screen.get_active_pane_id(&OTHER), Some(SHELL));
    // The other client has not focused it, and would still lose sight of it.
    new_tab(&mut screen, 3, TabId::new(1));
    assert_eq!(screen.active_tab_ids[&OTHER], TabId::new(0));
    assert_eq!(mother_tab(&screen), TabId::new(0));
    assert!(!hidden(&screen));

    // Asking for it outright still fetches it.
    screen.toggle_mother(CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(1));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn repeated_toggles_do_not_stack_the_launch_picker_or_replace_another() {
    let mut screen = screen();
    screen.toggle_mother(CLIENT).unwrap();
    screen.toggle_mother(CLIENT).unwrap();
    // One step back to the palette's root, and one more to leave it.
    palette_key(&mut screen, BareKey::Esc);
    assert!(screen.command_palettes.contains_key(&CLIENT));
    palette_key(&mut screen, BareKey::Esc);
    assert!(!screen.command_palettes.contains_key(&CLIENT));

    // A palette already in another picker keeps it.
    screen.toggle_command_palette(CLIENT);
    assert!(screen.command_palette_handles_input(&None, b"change theme", false, CLIENT));
    palette_key(&mut screen, BareKey::Enter);
    screen.toggle_mother(CLIENT).unwrap();
    palette_key(&mut screen, BareKey::Esc);
    palette_key(&mut screen, BareKey::Esc);
    assert!(!screen.command_palettes.contains_key(&CLIENT));
}

#[test]
fn toggling_an_unselectable_mother_keeps_the_screen_thread_running() {
    let mut mock = MockScreen::new(SIZE);
    let received = Watched::new(vec![]);
    let server = mock.server_receiver.take().unwrap();
    let logger = log_actions_in_thread!(received, ServerInstruction::KillSession, server);
    let screen_thread = mock.run(None, vec![]);
    let client = mock.main_client_id;
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let mut completion = crate::route::NotificationEnd::new(tx);
    completion.is_mother = true;
    for instruction in [
        ScreenInstruction::NewPane(
            MOTHER,
            None,
            None,
            None,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            false,
            ClientTabIndexOrPaneId::ClientId(client),
            Some(completion),
            false,
        ),
        ScreenInstruction::SetSelectable(MOTHER, false),
        ScreenInstruction::ToggleMother(client, None),
        ScreenInstruction::SetSelectable(MOTHER, true),
        ScreenInstruction::ToggleMother(client, None),
    ] {
        mock.to_screen.send(instruction).unwrap();
    }
    let (tx, rx) = crossbeam::channel::bounded(1);
    mock.to_screen
        .send(ScreenInstruction::GetFocusedPaneInfo {
            client_id: client,
            response_channel: tx,
        })
        .unwrap();
    let response = rx
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    mock.teardown(vec![screen_thread, logger]);
    assert!(matches!(
        response,
        yinz_types::data::GetFocusedPaneInfoResponse::Ok {
            pane_id: MOTHER,
            ..
        }
    ));
}

fn assert_input_goes_to(screen: &mut Screen, client: ClientId, pane: PaneId) {
    let (sender, writes) = channels::unbounded();
    screen
        .tab_containing_pane_mut(pane)
        .unwrap()
        .senders
        .to_pty_writer = Some(SenderWithContext::new(sender));
    screen
        .write_character(
            None,
            b"input".to_vec(),
            false,
            client,
            &HashMap::new(),
            None,
        )
        .unwrap();
    assert!(writes.try_iter().any(|(instruction, _)| matches!(instruction,
        PtyWriteInstruction::Write(bytes, id, _) if PaneId::Terminal(id) == pane && bytes == b"input"
    )));
}

#[test]
fn mirrored_transitions_restore_each_clients_mother_focus_after_layout_and_edge_focus() {
    for stand_in in [false, true] {
        for other_focused in [false, true] {
            for transition in ["ordinary", "directional", "layout"] {
                let mut screen = create_new_screen(SIZE, false, false);
                new_tab(&mut screen, 1, TabId::new(0));
                if transition != "layout" {
                    new_tab(&mut screen, 3, TabId::new(1));
                    screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
                }
                overlay(&mut screen, MOTHER);
                screen.mother_spawned(MOTHER).unwrap();
                screen.add_client(OTHER).unwrap();
                screen.toggle_mother(CLIENT).unwrap();
                if other_focused {
                    screen.toggle_mother(OTHER).unwrap();
                }
                let visible = if stand_in {
                    let editor = PaneId::Terminal(TerminalId::new(4));
                    screen
                        .tab_containing_pane_mut(MOTHER)
                        .unwrap()
                        .replace_pane_with_editor_pane(editor, MOTHER)
                        .unwrap();
                    editor
                } else {
                    MOTHER
                };
                match transition {
                    "layout" => new_tab(&mut screen, 3, TabId::new(1)),
                    "directional" => screen
                        .switch_active_tab(
                            yinz_types::internal_ids::TabPosition::new(1),
                            Some(Direction::Left),
                            true,
                            CLIENT,
                        )
                        .unwrap(),
                    _ => screen.go_to_tab(TabNumber::new(2), CLIENT).unwrap(),
                }
                assert_eq!(
                    screen.tab_containing_overlay(visible).unwrap().id,
                    TabId::new(1)
                );
                let destination = PaneId::Terminal(TerminalId::new(3));
                for (client, expected) in [
                    (CLIENT, visible),
                    (OTHER, if other_focused { visible } else { destination }),
                ] {
                    assert_eq!(screen.active_tab_ids[&client], TabId::new(1));
                    assert_eq!(
                        screen.get_active_pane_id(&client),
                        Some(expected),
                        "{transition} {stand_in} {other_focused}"
                    );
                    assert_input_goes_to(&mut screen, client, expected);
                }
            }
        }
    }
}

#[test]
fn generic_focus_fetches_hidden_mother_and_its_replacement_without_toggling_or_switching_tabs() {
    for stand_in in [false, true] {
        let mut screen = screen_with_mother();
        screen.toggle_mother(CLIENT).unwrap();
        let visible = if stand_in {
            let editor = PaneId::Terminal(TerminalId::new(4));
            screen
                .tab_containing_pane_mut(MOTHER)
                .unwrap()
                .replace_pane_with_editor_pane(editor, MOTHER)
                .unwrap();
            editor
        } else {
            MOTHER
        };
        screen.toggle_mother(CLIENT).unwrap();
        new_tab(&mut screen, 3, TabId::new(1));
        for requested in [MOTHER, visible, MOTHER] {
            screen
                .focus_pane_with_id(requested, true, true, CLIENT)
                .unwrap();
            assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
            assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
            assert!(
                !screen
                    .tab_containing_overlay(visible)
                    .unwrap()
                    .overlay_carry_for(visible)
                    .hidden
            );
            assert_input_goes_to(&mut screen, CLIENT, visible);
        }
        assert_eq!(
            screen
                .tab_containing_pane(MOTHER)
                .unwrap()
                .get_suppressed_panes()
                .count(),
            usize::from(stand_in)
        );
    }
}

#[test]
fn generic_mother_focus_goes_to_a_shared_mother_and_fetches_a_parked_one() {
    let mut screen = screen_with_mother();
    screen.add_client(OTHER).unwrap();
    screen.toggle_mother(OTHER).unwrap();
    new_tab(&mut screen, 3, TabId::new(1));
    // Another client is typing into it, so the request goes to mother instead of taking it.
    screen
        .focus_pane_with_id(MOTHER, true, false, CLIENT)
        .unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(0));
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(0));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert_input_goes_to(&mut screen, OTHER, MOTHER);
    screen.go_to_tab(TabNumber::new(2), CLIENT).unwrap();
    assert_eq!(mother_tab(&screen), TabId::new(0));
    screen.active_tab_ids.insert(CLIENT, TabId::new(99));
    screen
        .focus_pane_with_id(MOTHER, true, false, CLIENT)
        .unwrap();
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(99));
    assert_eq!(mother_tab(&screen), TabId::new(0));
    screen.active_tab_ids.insert(CLIENT, TabId::new(1));
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .suppress_pane(MOTHER, Some(OTHER));
    // Parked with nothing standing in for it, mother answers to the caller's flags.
    screen
        .focus_pane_with_id(MOTHER, true, true, CLIENT)
        .unwrap();
    assert!(screen
        .tab_containing_pane(MOTHER)
        .unwrap()
        .has_non_suppressed_pane_with_pid(&MOTHER));
    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(0));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn saved_mother_matches_reported_live_hidden_replaced_held_and_missing_metadata() {
    for state in ["live", "hidden", "replaced", "held", "exited", "missing"] {
        let mut screen = screen_with_mother();
        screen.mother_status = Some(yinz_types::agent::mother::MotherStatus {
            state: "watching".into(),
            ..Default::default()
        });
        match state {
            "hidden" => {
                screen.toggle_mother(CLIENT).unwrap();
                screen.toggle_mother(CLIENT).unwrap();
            },
            "replaced" => screen
                .tab_containing_pane_mut(MOTHER)
                .unwrap()
                .replace_pane_with_editor_pane(PaneId::Terminal(TerminalId::new(4)), MOTHER)
                .unwrap(),
            "held" | "exited" => {
                screen.tab_containing_pane_mut(MOTHER).unwrap().hold_pane(
                    MOTHER,
                    Some(0),
                    state == "held",
                    RunCommand {
                        command: "/usr/bin/true".into(),
                        ..Default::default()
                    },
                );
            },
            "missing" => {
                screen.mother = Some(TerminalId::new(99));
            },
            _ => {},
        }
        let expected =
            matches!(state, "live" | "hidden" | "replaced").then_some(TerminalId::new(2));
        let (sender, messages) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
        screen.save_session(None).unwrap();
        let saved = messages
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PtyInstruction::SaveSessionToDisk { session_info, .. } => Some(session_info),
                _ => None,
            })
            .unwrap();
        assert_eq!(saved.mother, expected, "{state}");
        assert_eq!(
            saved.mother_supervision.is_some(),
            expected.is_some(),
            "{state}"
        );
        assert_eq!(
            serde_json::to_value(&saved).unwrap()["mother"].is_null(),
            expected.is_none()
        );
        screen.log_and_report_session_state().unwrap();
        assert_eq!(saved.mother, reported_mother(&screen), "{state}");
        assert_eq!(
            screen.peer_sessions_cache[&screen.session_name].mother_supervision,
            saved.mother_supervision,
            "{state}"
        );
        if matches!(state, "held" | "exited") {
            assert_eq!(screen.mother, Some(TerminalId::new(2)));
        }
    }
}

fn place_mother(
    screen: &mut Screen,
    id: u32,
    set_blocking: bool,
) -> tokio::sync::oneshot::Receiver<crate::route::ActionCompletionResult> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut completion = crate::route::NotificationEnd::new(tx);
    completion.set_affected_pane_id(PaneId::Terminal(TerminalId::new(id)));
    completion.is_mother = true;
    completion.wait_for_agent_start = true;
    screen
        .place_new_pane(
            PaneId::Terminal(TerminalId::new(id)),
            None,
            None,
            None,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            false,
            ClientTabIndexOrPaneId::ClientIdNoFocus(CLIENT),
            Some(completion),
            set_blocking,
            &mut Default::default(),
        )
        .unwrap();
    rx
}

#[test]
fn mother_registration_survives_late_placement_and_refuses_duplicate_arrivals() {
    let mut screen = screen();
    let (sender, messages) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
    // The launcher gave up before its pane arrived; registration belongs to the screen.
    drop(place_mother(&mut screen, 2, false));
    assert_eq!(screen.live_mother(), Some(MOTHER));

    let late = PaneId::Terminal(TerminalId::new(3));
    let reply = place_mother(&mut screen, 3, false).blocking_recv().unwrap();
    assert_eq!(
        reply.error_message.as_deref(),
        Some("Mother is already running")
    );
    // The pane was never shown, so the launcher is not told about one.
    assert_eq!(reply.affected_pane_id, None);
    assert_eq!(screen.live_mother(), Some(MOTHER));
    assert!(screen.tab_containing_pane(late).is_none());
    assert_eq!(
        messages
            .try_iter()
            .filter(|(instruction, _)| matches!(instruction,
                PtyInstruction::ClosePane(pane, _) if *pane == late))
            .count(),
        1
    );
}

#[test]
fn a_blocking_mother_launch_is_still_registered() {
    let mut screen = screen();
    let _reply = place_mother(&mut screen, 2, true);
    assert_eq!(screen.live_mother(), Some(MOTHER));
}

#[test]
fn mother_placement_registers_retained_panes_on_error_but_never_missing_panes() {
    for retained in [false, true] {
        let mut screen = screen();
        let (sender, _messages) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
        if retained {
            let tab = screen.get_active_tab_mut(CLIENT).unwrap();
            tab.senders.should_silently_fail = false;
            tab.senders.to_pty_writer = None;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut completion = crate::route::NotificationEnd::new(tx);
        completion.is_mother = true;
        completion.wait_for_agent_start = true;
        let target = if retained {
            TabId::new(0)
        } else {
            TabId::new(99)
        };
        let result = screen.place_new_pane(
            MOTHER,
            None,
            None,
            None,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            false,
            ClientTabIndexOrPaneId::TabIndexNoFocus(target),
            Some(completion),
            false,
            &mut Default::default(),
        );
        assert_eq!(result.is_err(), retained);
        let reply = rx.blocking_recv().unwrap();
        assert!(reply.error_message.is_some());
        assert_eq!(reply.affected_pane_id, Some(MOTHER));
        assert_eq!(screen.live_mother(), retained.then_some(MOTHER));
        assert_eq!(screen.tab_containing_pane(MOTHER).is_some(), retained);
    }
}

#[test]
fn mother_is_registered_before_pending_layout_completes_placement() {
    let mut screen = screen();
    let tab = TabId::new(1);
    screen
        .new_tab(tab, (vec![], vec![]), None, Some(CLIENT))
        .unwrap();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let mut completion = crate::route::NotificationEnd::new(tx);
    completion.is_mother = true;
    completion.wait_for_agent_start = true;
    screen
        .place_new_pane(
            MOTHER,
            None,
            None,
            None,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            false,
            ClientTabIndexOrPaneId::TabIndexNoFocus(tab),
            Some(completion),
            false,
            &mut Default::default(),
        )
        .unwrap();
    assert_eq!(screen.live_mother(), Some(MOTHER));
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    // The caller gives up before the layout arrives; registration already belongs to the screen.
    drop(rx);
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(3), None)],
            vec![],
            HashMap::new(),
            tab,
            false,
            CLIENT,
            None,
        )
        .unwrap();
    assert_eq!(screen.live_mother(), Some(MOTHER));
    assert_eq!(mother_tab(&screen), tab);
}

#[test]
fn explicit_focus_hides_mother_and_replacements_unless_another_client_uses_them() {
    for another_tab in [false, true] {
        for stand_in in [false, true] {
            for shared in [false, true] {
                let mut screen = screen_with_mother();
                new_tab(&mut screen, 3, TabId::new(1));
                screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
                screen.toggle_mother(CLIENT).unwrap();
                let visible = if stand_in {
                    let editor = PaneId::Terminal(TerminalId::new(4));
                    screen
                        .tab_containing_pane_mut(MOTHER)
                        .unwrap()
                        .replace_pane_with_editor_pane(editor, MOTHER)
                        .unwrap();
                    editor
                } else {
                    MOTHER
                };
                if shared {
                    screen.add_client(OTHER).unwrap();
                    screen
                        .focus_pane_with_id(visible, false, false, OTHER)
                        .unwrap();
                }
                let requested = if another_tab {
                    PaneId::Terminal(TerminalId::new(3))
                } else {
                    SHELL
                };
                screen
                    .focus_pane_with_id(requested, false, false, CLIENT)
                    .unwrap();
                assert_eq!(screen.get_active_pane_id(&CLIENT), Some(requested));
                assert_input_goes_to(&mut screen, CLIENT, requested);
                assert_eq!(
                    screen
                        .tab_containing_overlay(visible)
                        .unwrap()
                        .overlay_carry_for(visible)
                        .hidden,
                    !shared
                );
                assert_eq!(
                    screen.tab_containing_overlay(visible).unwrap().id,
                    TabId::new(0)
                );
                if shared {
                    assert_input_goes_to(&mut screen, OTHER, visible);
                }
                screen.toggle_mother(CLIENT).unwrap();
                assert_eq!(screen.get_active_pane_id(&CLIENT), Some(visible));
                assert_input_goes_to(&mut screen, CLIENT, visible);
            }
        }
    }
}

#[test]
fn invalid_explicit_focus_leaves_mother_visible_and_focused() {
    let mut screen = screen_with_mother();
    screen.toggle_mother(CLIENT).unwrap();
    screen
        .focus_pane_with_id(PaneId::Terminal(TerminalId::new(99)), false, false, CLIENT)
        .unwrap();
    assert!(!hidden(&screen));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
    assert_input_goes_to(&mut screen, CLIENT, MOTHER);
}

#[test]
fn notification_and_tiled_agent_launch_focus_hide_mother_before_switching_tabs() {
    use yinz_bus::command_palette::{AgentReply, Instruction};
    use yinz_types::agent::control::{AgentOutcome, AgentResponse};
    use yinz_types::agent::SpawnedAgent;
    use yinz_types::internal_ids::NotificationId;

    for launch in [false, true] {
        let mut screen = screen_with_mother();
        new_tab(&mut screen, 3, TabId::new(1));
        screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
        screen.toggle_mother(CLIENT).unwrap();
        let target = PaneId::Terminal(TerminalId::new(3));
        if launch {
            let (sender, requests) = channels::unbounded();
            screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
            screen.toggle_command_palette(CLIENT);
            screen.command_palette_handles_input(&None, b"claude current directory", false, CLIENT);
            palette_key(&mut screen, BareKey::Enter);
            let request = requests
                .try_iter()
                .find_map(|(instruction, _)| match instruction {
                    PluginInstruction::PaletteAgent(request) => Some(request),
                    _ => None,
                })
                .unwrap();
            screen.plugin_palette_instruction(Instruction::AgentReply {
                client: CLIENT,
                opening: request.opening,
                step: request.step,
                reply: Ok(AgentReply::Launched(AgentResponse {
                    outcome: AgentOutcome::Ok,
                    spawned: Some(SpawnedAgent {
                        pane_id: target,
                        tab_id: Some(TabId::new(1)),
                        ..Default::default()
                    }),
                    ..Default::default()
                })),
            });
            assert!(!screen.command_palettes.contains_key(&CLIENT));
        } else {
            screen.activate_notification(
                NotificationId::new(0),
                Some(target),
                None,
                CLIENT,
                std::time::Instant::now(),
            );
        }
        assert!(hidden(&screen));
        assert_eq!(mother_tab(&screen), TabId::new(0));
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(target));
        assert_input_goes_to(&mut screen, CLIENT, target);
        screen.toggle_mother(CLIENT).unwrap();
        assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
        assert_input_goes_to(&mut screen, CLIENT, MOTHER);
    }
}

#[test]
fn palette_pane_focus_hides_mother_before_switching_tabs() {
    let mut screen = screen_with_mother();
    new_tab(&mut screen, 3, TabId::new(1));
    screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
    screen.toggle_mother(CLIENT).unwrap();
    let target = PaneId::Terminal(TerminalId::new(3));
    screen.toggle_command_palette(CLIENT);
    // Session-provider navigation uses the same host focus request as other plugins.
    screen
        .focus_pane_by_request(target, true, false, CLIENT, None, &mut Default::default())
        .unwrap();
    assert!(hidden(&screen));
    assert_eq!(mother_tab(&screen), TabId::new(0));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(target));
    assert_input_goes_to(&mut screen, CLIENT, target);
    screen.toggle_mother(CLIENT).unwrap();
    assert_input_goes_to(&mut screen, CLIENT, MOTHER);
}

#[test]
fn mother_reclamation_in_place_preserves_pending_respawn() {
    let mut screen = screen_with_mother();
    let respawn = (
        RunCommand {
            command: "/usr/bin/true".into(),
            ..Default::default()
        },
        true,
    );
    let tab = screen.tab_containing_pane_mut(MOTHER).unwrap();
    tab.suppress_pane(MOTHER, Some(CLIENT));
    tab.restore_pending_respawn(MOTHER, respawn.clone());
    screen.toggle_mother(CLIENT).unwrap();
    let tab = screen.tab_containing_pane_mut(MOTHER).unwrap();
    assert_eq!(tab.id, TabId::new(0));
    assert!(tab.pane_is_overlay(MOTHER));
    assert_eq!(tab.take_pending_respawn(MOTHER), Some(respawn));
    assert_eq!(screen.get_active_pane_id(&CLIENT), Some(MOTHER));
}

#[test]
fn mother_transfer_refusal_restores_original_surface_and_respawn() {
    let mut screen = screen_with_mother();
    let respawn = (
        RunCommand {
            command: "/usr/bin/true".into(),
            ..Default::default()
        },
        true,
    );
    screen
        .tab_containing_pane_mut(MOTHER)
        .unwrap()
        .restore_pending_respawn(MOTHER, respawn.clone());
    let pane = screen
        .extract_pane_for_transfer(TabId::new(0), MOTHER)
        .unwrap();
    let outcome = screen.accept_transferred_panes(
        vec![(pane, crate::tab::ExtractedPanePlacement::Tiled)],
        TabId::new(99),
        false,
    );
    assert_eq!(outcome.returned_to_source, vec![MOTHER]);
    assert!(outcome.closed.is_empty());
    let tab = screen.tab_containing_pane_mut(MOTHER).unwrap();
    assert!(tab.pane_is_overlay(MOTHER));
    assert_eq!(tab.take_pending_respawn(MOTHER), Some(respawn));
}

#[test]
fn mother_candidates_exist_only_without_a_live_mother() {
    let mut screen = screen();
    let renders = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(SIZE);
    for live in [false, true, false] {
        if live {
            overlay(&mut screen, MOTHER);
            screen.mother_spawned(MOTHER).unwrap();
        } else if screen.mother.is_some() {
            hold(&mut screen, MOTHER);
        }
        screen.toggle_command_palette(CLIENT);
        screen.command_palette_handles_input(&None, b"mother", false, CLIENT);
        let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
        assert_eq!(frame.contains("Launch mother"), !live, "{frame}");
        if !live {
            palette_key(&mut screen, BareKey::Enter);
            let frame = render_frame_into(&mut screen, &renders, CLIENT, &mut grid);
            assert!(
                frame.contains("codex") && frame.contains("claude"),
                "{frame}"
            );
        }
        screen.close_command_palette(CLIENT);
    }
}

#[test]
fn independent_transitions_leave_hidden_exited_or_watched_mother_behind() {
    for state in ["hidden", "exited", "watched"] {
        for transition in ["ordinary", "directional", "layout"] {
            let mut screen = screen_with_mother();
            if transition != "layout" {
                new_tab(&mut screen, 3, TabId::new(1));
                screen.go_to_tab(TabNumber::new(1), CLIENT).unwrap();
                screen.toggle_mother(CLIENT).unwrap();
            }
            match state {
                "hidden" => {
                    screen
                        .tab_containing_pane_mut(MOTHER)
                        .unwrap()
                        .hide_overlay_pane(MOTHER);
                },
                "exited" => hold(&mut screen, MOTHER),
                _ => screen.add_client(OTHER).unwrap(),
            }
            match transition {
                "layout" => new_tab(&mut screen, 3, TabId::new(1)),
                "directional" => screen
                    .switch_active_tab(
                        yinz_types::internal_ids::TabPosition::new(1),
                        Some(Direction::Left),
                        true,
                        CLIENT,
                    )
                    .unwrap(),
                _ => screen.go_to_tab(TabNumber::new(2), CLIENT).unwrap(),
            }
            assert_eq!(mother_tab(&screen), TabId::new(0), "{state} {transition}");
            assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(1));
            assert_input_goes_to(&mut screen, CLIENT, PaneId::Terminal(TerminalId::new(3)));
        }
    }
}

#[test]
fn a_failed_mother_transfer_insertion_closes_the_homeless_process() {
    let mut screen = screen_with_mother();
    let (sender, messages) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(sender));
    let pane = screen
        .extract_pane_for_transfer(TabId::new(0), MOTHER)
        .unwrap();
    let mut placement = OverlayPlacement::mother();
    placement.background_opacity = Some(101);
    let outcome = screen.accept_transferred_panes(
        vec![(
            pane,
            crate::tab::ExtractedPanePlacement::Overlay(placement, Default::default()),
        )],
        TabId::new(0),
        false,
    );
    assert_eq!(outcome.closed, vec![MOTHER]);
    assert!(outcome.moved.is_empty());
    assert!(screen.tab_containing_pane(MOTHER).is_none());
    assert!(screen.mother.is_none());
    assert!(messages.try_iter().any(
        |(instruction, _)| matches!(instruction, PtyInstruction::ClosePane(id, _) if id == MOTHER)
    ));
}

#[test]
fn mother_carries_loaded_overview_behind_an_in_place_replacement() {
    use yinz_types::internal_ids::PluginId;
    let mut screen = screen_with_mother();
    screen.toggle_mother(CLIENT).unwrap();
    let tab = screen.tab_containing_pane_mut(MOTHER).unwrap();
    let (sender, _receiver) = channels::unbounded();
    tab.senders.to_plugin = Some(SenderWithContext::new(sender));
    let id = tab.pane_tabs_of(MOTHER).unwrap();
    tab.select_pane_tab(id, 1, CLIENT).unwrap();
    let plugin = PluginId::new(42);
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:mother-overview",
        &None,
        None,
        None,
    )
    .unwrap();
    tab.pane_tab_loaded(id, 1, 1, Ok((plugin, run))).unwrap();
    tab.select_pane_tab(id, 0, CLIENT).unwrap();
    let editor = PaneId::Terminal(TerminalId::new(4));
    tab.replace_pane_with_editor_pane(editor, MOTHER).unwrap();
    new_tab(&mut screen, 3, TabId::new(1));
    screen.toggle_mother(CLIENT).unwrap();
    let tab = screen.tab_containing_pane_mut(MOTHER).unwrap();
    assert!(tab.pane_tabs_info(id).is_some());
    assert!(tab.get_pane_with_id(PaneId::Plugin(plugin)).is_some());
    tab.close_pane(editor, false, None);
    tab.select_pane_tab(id, 1, CLIENT).unwrap();
    assert_eq!(
        tab.pane_tabs_info(id).unwrap().pane_id,
        PaneId::Plugin(plugin)
    );
    tab.close_pane_tabs(id).unwrap();
    assert!(tab.get_pane_with_id(MOTHER).is_none());
    assert!(tab.get_pane_with_id(PaneId::Plugin(plugin)).is_none());
}

#[test]
fn rename_session_rejects_unlisted_socket_cache_and_lookup_errors_without_mutation() {
    use std::os::unix::net::UnixListener;
    use yinz_runtime::consts::{session_info_folder_for_session, YINZ_SOCK_DIR};
    use yinz_types::internal_ids::SessionName;
    let mut screen = screen_with_mother();
    let old: SessionName = format!("rename-old-{}", uuid::Uuid::new_v4()).into();
    screen.session_name = old.clone();
    std::fs::create_dir_all(&*YINZ_SOCK_DIR).unwrap();
    let old_socket = YINZ_SOCK_DIR.join(&old);
    let _old = UnixListener::bind(&old_socket).unwrap();
    let cache = session_info_folder_for_session(&old);
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("sentinel"), "old").unwrap();
    let target: SessionName = format!("rename-target-{}", uuid::Uuid::new_v4()).into();
    let target_socket = YINZ_SOCK_DIR.join(&target);
    let target_cache = session_info_folder_for_session(&target);
    let mother = screen.mother;
    let _target = UnixListener::bind(&target_socket).unwrap();
    screen.rename_session(target.clone(), CLIENT).unwrap();
    assert_eq!(screen.session_name, old);
    assert!(target_socket.exists());
    std::fs::remove_file(&target_socket).unwrap();
    std::fs::create_dir_all(&target_cache).unwrap();
    std::fs::write(target_cache.join("sentinel"), "target").unwrap();
    screen.rename_session(target.clone(), CLIENT).unwrap();
    assert_eq!(screen.session_name, old);
    assert_eq!(
        std::fs::read_to_string(target_cache.join("sentinel")).unwrap(),
        "target"
    );
    std::fs::remove_dir_all(&target_cache).unwrap();
    // A self-referential link produces ELOOP instead of an absent path.
    std::os::unix::fs::symlink(&target_socket, &target_socket).unwrap();
    screen.rename_session(target.clone(), CLIENT).unwrap();
    assert_eq!(screen.session_name, old);
    std::fs::remove_file(&target_socket).unwrap();
    std::os::unix::fs::symlink(&target_cache, &target_cache).unwrap();
    screen.rename_session(target, CLIENT).unwrap();
    assert_eq!(screen.session_name, old);
    screen.rename_session(old.clone(), CLIENT).unwrap();
    assert_eq!(screen.mother, mother);
    assert_eq!(
        std::fs::read_to_string(cache.join("sentinel")).unwrap(),
        "old"
    );
    assert!(old_socket.exists());
    std::fs::remove_file(target_cache).unwrap();
    std::fs::remove_file(old_socket).unwrap();
    std::fs::remove_dir_all(cache).unwrap();
}
