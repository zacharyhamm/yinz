#![cfg(unix)]
//! Where a spawned agent pane lands, seen from a real session.
//!
//! `Screen::agent_pane_target` is unit-tested against the method, and once through the screen
//! thread; what neither can say is that the tab bar shows the tab the agent was sent to and that
//! the keyboard followed it there. On the harness's 120x24 terminal the viewport is 120x21, and
//! one pane's share of it (1260 cells) already sits under the default 80x24 floor, so the first
//! agent spawned is enough.

use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, start_yinz, TestRunner, PROMPT, TERMINAL_SIZE,
};
use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
use yinz_types::agent::{
    control::{AgentOutcome, AgentPlacement, AgentRequest},
    AgentSpawnSpec,
};

#[test]
fn cli_agent_spawn_preserves_keyboard_focus_in_current_existing_and_new_tabs() {
    use yinz_types::internal_ids::TabId;
    let cases = [
        ("agent_pane_min_columns 0", "layout { tab name=\"original\" { pane; } }", 1, 0, AgentPlacement::Automatic, false),
        ("agent_pane_min_columns 50\nagent_pane_min_rows 20", "layout { tab name=\"original\" focus=true { pane; pane; }; tab name=\"spare\" { pane; }; }", 3, 1, AgentPlacement::Automatic, false),
        ("", "layout { tab name=\"original\" { pane; } }", 1, 1, AgentPlacement::Automatic, true),
        ("", "layout { tab name=\"original\" { pane; }; }", 1, 0, AgentPlacement::Tab(TabId::new(0)), false),
        ("", "layout { tab name=\"original\" focus=true { pane; }; tab name=\"spare\" { pane; }; }", 2, 1, AgentPlacement::Tab(TabId::new(1)), false),
        ("agent_pane_min_columns 0", "layout { tab name=\"original\" { pane; }; }", 1, 1, AgentPlacement::NewTab(None), true),
        ("agent_pane_min_columns 0", "layout { tab name=\"original\" { pane; }; }", 1, 1, AgentPlacement::NewTab(Some("review".into())), true),
    ];
    for (config, layout, pane_count, expected_tab, placement, created) in cases {
        let mut yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config("load_plugins clear-defaults=true {}\nmirror_session false")
            .with_config(config)
            .with_layout(yinz_integration_tests::LayoutInfo::Stringified(
                layout.into(),
            ))
            .start();
        let original = yinz.expect_pty_spawn();
        original.output(b"originalprompt$ ");
        let mut other_original = None;
        for _ in 1..pane_count {
            let pane = yinz.expect_pty_spawn();
            pane.output(b"otherprompt$ ");
            other_original = Some(pane);
        }
        yinz.wait_until("original pane is rendered", |snapshot| {
            snapshot.text.contains("originalprompt$")
        });
        yinz.run_cli_action(CliAction::FocusPaneId {
            pane_id: yinz_types::pane_id::PaneId::Terminal(original.terminal_id()).to_string(),
        });

        let other_pane = other_original.as_ref().unwrap_or(&original);
        let other_marker = if other_original.is_some() {
            "otherprompt$"
        } else {
            "originalprompt$"
        };
        let other_client = yinz.attach_client_with_focus(
            TERMINAL_SIZE,
            Some(yinz_types::pane_id::PaneId::Terminal(
                other_pane.terminal_id(),
            )),
        );
        other_client.wait_until("second client sees its original pane", |snapshot| {
            snapshot.text.contains(other_marker)
        });

        // A CLI connection has no attached screen, so placement uses the attached-client fallback.
        let socket = yinz_ipc::consts::ipc_connect(
            &yinz_runtime::consts::YINZ_SOCK_DIR.join(yinz.session_name()),
        )
        .unwrap();
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
        let mut receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
        receiver
            .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        let cwd = tempfile::tempdir().unwrap();
        sender
            .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Spawn {
                placement,
                spec: AgentSpawnSpec {
                    kind: Some("claude".into()),
                    allow_outside_worktree: Some(true),
                    ..Default::default()
                },
                cwd: Some(cwd.path().into()),
                project: None,
                remote_host: None,
            }))
            .unwrap();
        let IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) = receiver.recv_server_msg()
        else {
            panic!("expected agent spawn response");
        };
        assert_eq!(response.outcome, AgentOutcome::Ok, "{response:?}");
        let spawned = response.spawned.as_ref().unwrap();
        assert_eq!(spawned.tab_id, Some(TabId::new(expected_tab)));
        assert_eq!(
            spawned.created_tab_id,
            created.then_some(TabId::new(expected_tab))
        );
        let agent = yinz.expect_pty_spawn();
        agent.output(b"workerprompt$ ");
        let probe = b"originalkeyboard";
        yinz.send_stdin(probe);
        original.wait_for_stdin(
            "keyboard stays on the original pane after CLI spawn",
            |bytes| bytes.windows(probe.len()).any(|window| window == probe),
        );
        yinz.wait_until("original pane still visible after input", |snapshot| {
            snapshot.text.contains("originalkeyboard")
        });
        assert!(!agent
            .stdin_bytes()
            .windows(probe.len())
            .any(|window| window == probe));
        let other_probe = b"secondkeyboard";
        other_client.send_stdin(other_probe);
        other_pane.wait_for_stdin("second client's focus is preserved", |bytes| {
            bytes
                .windows(other_probe.len())
                .any(|window| window == other_probe)
        });
        assert!(!agent
            .stdin_bytes()
            .windows(other_probe.len())
            .any(|window| window == other_probe));
        let (code, listed) = yinz.run_cli_action_reporting(CliAction::ListPanes {
            tab: true,
            command: false,
            state: true,
            geometry: false,
            all: true,
            json: true,
        });
        assert_eq!(code, 0, "{listed}");
        let panes: serde_json::Value = serde_json::from_str(&listed).unwrap();
        let worker = panes
            .as_array()
            .unwrap()
            .iter()
            .find(|pane| pane["is_plugin"] == false && pane["id"] == agent.terminal_id().get())
            .unwrap();
        assert_eq!(worker["tab_id"], expected_tab, "{listed}");
        drop(sender);
        drop(receiver);
        other_client.quit();
        yinz.quit();
    }
}

fn new_pane_action(command: &[&str]) -> CliAction {
    CliAction::NewPane {
        direction: None,
        command: command.iter().map(|part| part.to_string()).collect(),
        remote_host: None,

        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
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
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    }
}

#[test]
fn an_agent_spawned_into_a_tab_too_small_to_share_gets_a_tab_of_its_own_and_the_keyboard() {
    let mut yinz = start_yinz();
    let shell = claim_first_terminal_and_wait_for_prompt(&yinz);
    // Something only the shell's tab shows, so its absence later proves that tab is off screen.
    shell.output(b"shellmarker");
    yinz.wait_until("the shell's marker is on screen", |grid_snapshot| {
        grid_snapshot.text.contains("shellmarker")
    });

    yinz.run_cli_action(new_pane_action(&["claude"]));
    let agent = yinz.expect_pty_spawn();
    agent.output(PROMPT);
    // The agent's title on the bar is not enough: until its pending layout lands,
    // the shell's tab bar already lists it while the client still sees the shell.
    // The shell's marker disappearing proves the client moved. A lone pane shows
    // its title in the tab bar; its prompt must also be visible.
    yinz.wait_until(
        "the client is on the agent's tab and sees its prompt",
        |grid_snapshot| {
            grid_snapshot.text.contains("claude")
                && !grid_snapshot.text.contains("shellmarker")
                && grid_snapshot.text.contains("$ ")
        },
    );

    let probe = b"agentprobe";
    yinz.send_stdin(probe);
    agent.wait_for_stdin("the probe reached the agent's pane", |stdin_bytes| {
        stdin_bytes
            .windows(probe.len())
            .any(|window| window == probe)
    });
    yinz.quit();
}

#[test]
fn a_zero_floor_keeps_the_agent_in_the_tab_it_was_asked_for() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("agent_pane_min_columns 0")
        .start();
    let _shell = claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.run_cli_action(new_pane_action(&["claude"]));
    let agent = yinz.expect_pty_spawn();
    agent.output(PROMPT);
    let screen = yinz.wait_until("the agent's prompt beside the shell's", |grid_snapshot| {
        grid_snapshot.text.matches("$").count() >= 2
    });
    assert!(
        !screen.text.contains("Tab #2"),
        "the rule is off, so no tab was made:\n{}",
        screen.text
    );
    yinz.quit();
}

#[test]
fn a_shell_is_not_an_agent_and_splits_the_tab_it_was_asked_for() {
    let mut yinz = start_yinz();
    let _shell = claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.run_cli_action(new_pane_action(&[]));
    let second_shell = yinz.expect_pty_spawn();
    second_shell.output(PROMPT);
    let screen = yinz.wait_until("two prompts in one tab", |grid_snapshot| {
        grid_snapshot.text.matches("$").count() >= 2
    });
    assert!(
        !screen.text.contains("Tab #2"),
        "a shell is placed where it was asked for:\n{}",
        screen.text
    );
    yinz.quit();
}

#[test]
fn cli_new_project_spawn_creates_an_agent_at_the_root_and_preserves_focus() {
    use agent_proto::{projects, store};
    use yinz_storage::data_access::DataAccess;
    use yinz_types::plugin_data::FetchRequest;

    let data = DataAccess::sqlite(&yinz_integration_tests::test_env::init().join("data"));
    for def in store::shared_defs() {
        data.plugin_data()
            .define_entity(store::SHARED, "test", &def, false)
            .unwrap();
    }
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\nagent_pane_min_columns 0")
        .start();
    let original = yinz.expect_pty_spawn();
    original.output(b"originalprompt$ ");
    yinz.wait_until("original pane is ready", |snapshot| {
        snapshot.contains("originalprompt$")
    });
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("new-app");
    let socket = yinz_ipc::consts::ipc_connect(
        &yinz_runtime::consts::YINZ_SOCK_DIR.join(yinz.session_name()),
    )
    .unwrap();
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
    let mut receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
    receiver
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(
            AgentRequest::SpawnNewProject {
                name: "new-app".into(),
                destination: root.clone(),
                spec: AgentSpawnSpec {
                    kind: Some("claude".into()),
                    ..Default::default()
                },
                placement: AgentPlacement::NewTab(Some("new-app".into())),
            },
        ))
        .unwrap();
    let response = loop {
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => break response,
            IpcRecv::Msg(_, _) => continue,
            _ => panic!("agent reply disconnected or timed out"),
        }
    };
    assert_eq!(response.outcome, AgentOutcome::Ok, "{response:?}");
    let spawned = response.spawned.unwrap();
    assert!(spawned.created_tab_id.is_some());
    assert_eq!(spawned.cwd, Some(root.clone()));
    let agent = yinz.expect_pty_spawn();
    assert_eq!(agent.run_command().cwd, Some(root.clone()));
    agent.output(b"workerprompt$ ");
    let probe = b"originalkeyboard";
    yinz.send_stdin(probe);
    original.wait_for_stdin("new project launch preserves keyboard focus", |bytes| {
        bytes.windows(probe.len()).any(|part| part == probe)
    });
    assert!(!agent
        .stdin_bytes()
        .windows(probe.len())
        .any(|part| part == probe));
    let rows = data
        .plugin_data()
        .fetch(store::SHARED, &FetchRequest::entity("project"))
        .unwrap();
    assert_eq!(
        projects::from_rows(&rows)["new-app"].root,
        root.to_str().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".git/HEAD")).unwrap(),
        "ref: refs/heads/main\n"
    );
    assert!(!root.join(".git/refs/heads/main").exists());
    drop(sender);
    drop(receiver);
    yinz.quit();
}
