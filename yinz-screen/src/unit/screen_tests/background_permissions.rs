use super::*;
use crate::PluginId;
use yinz_types::data::{PaneInfo, PermissionStatus, PermissionType, PluginPermission};

const PLUGIN: PluginId = PluginId::new(42);
const CLIENT: ClientId = ClientId::new(1);
const SIZE: Size = Size {
    cols: 100,
    rows: 30,
};

fn request(mock: &MockScreen) {
    mock.to_screen
        .send(ScreenInstruction::RequestPluginPermissions(
            PLUGIN,
            PluginPermission::new(
                "background-permission-fixture".into(),
                vec![PermissionType::ReadApplicationState],
            ),
        ))
        .unwrap();
}

fn attach(mock: &MockScreen, tab: TabId) {
    mock.to_screen
        .send(ScreenInstruction::AddPlugin(
            None,
            false,
            false,
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                "file:/background-permission-fixture",
                &None,
                None,
                None,
            )
            .unwrap(),
            None,
            Some(tab),
            PLUGIN,
            None,
            None,
            true,
            None,
            Some(false),
            Some(CLIENT),
            None,
            None,
        ))
        .unwrap();
}

// Also a dispatcher barrier: every instruction sent before this query has been handled.
fn pane_info(mock: &MockScreen, pane_id: PaneId) -> Option<PaneInfo> {
    let (tx, rx) = crossbeam::channel::bounded(1);
    mock.to_screen
        .send(ScreenInstruction::GetPaneInfo {
            pane_id,
            response_channel: tx,
        })
        .unwrap();
    rx.recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
}

fn start() -> (
    MockScreen,
    Watched<Vec<ServerInstruction>>,
    Vec<std::thread::JoinHandle<()>>,
) {
    let mut mock = MockScreen::new(SIZE);
    let renders = Watched::new(vec![]);
    let receiver = mock.server_receiver.take().unwrap();
    let logger = log_actions_in_thread!(renders, ServerInstruction::KillSession, receiver);
    let screen = mock.run(None, vec![]);
    (mock, renders, vec![screen, logger])
}

fn assert_prompt(renders: &Watched<Vec<ServerInstruction>>) {
    wait_for_settled_render(
        renders,
        SIZE,
        "background permission prompt without input",
        |frame| frame.contains("Allow? (y/n)"),
    );
}

fn focused_pane(mock: &MockScreen, client_id: ClientId) -> (TabId, PaneId) {
    let (tx, rx) = crossbeam::channel::bounded(1);
    mock.to_screen
        .send(ScreenInstruction::GetFocusedPaneInfo {
            client_id,
            response_channel: tx,
        })
        .unwrap();
    match rx
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
    {
        yinz_types::data::GetFocusedPaneInfoResponse::Ok { tab_id, pane_id } => (tab_id, pane_id),
        response => panic!("unexpected focus response: {response:?}"),
    }
}

#[test]
fn background_permission_clients_and_inactive_tab() {
    let (mut mock, renders, threads) = start();
    mock.to_screen
        .send(ScreenInstruction::NewTab(
            None,
            None,
            Some(TiledPaneLayout::default()),
            vec![],
            None,
            (Some(vec![]), Some(vec![])),
            None,
            false,
            true,
            CLIENT,
            None,
        ))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::ApplyLayout(
            TiledPaneLayout::default(),
            vec![],
            vec![(TerminalId::new(1), None)],
            vec![],
            vec![],
            HashMap::new(),
            TabId::new(1),
            true,
            CLIENT,
            None,
            None,
        ))
        .unwrap();
    attach(&mock, TabId::new(1));
    mock.to_screen
        .send(ScreenInstruction::GoToTabWithId(
            TabId::new(0),
            Some(CLIENT),
            None,
        ))
        .unwrap();
    let other = ClientId::new(2);
    mock.to_screen
        .send(ScreenInstruction::AddClient(
            other,
            SIZE,
            Some(yinz_types::internal_ids::TabNumber::new(2)),
            None,
        ))
        .unwrap();
    request(&mock);
    assert_eq!(
        focused_pane(&mock, CLIENT),
        (TabId::new(0), PaneId::Terminal(TerminalId::new(0))),
        "a background request must not switch an unrelated client away from its tab"
    );
    assert_eq!(
        focused_pane(&mock, other),
        (TabId::new(1), PaneId::Plugin(PLUGIN))
    );
    mock.to_screen
        .send(ScreenInstruction::GoToTabWithId(
            TabId::new(1),
            Some(CLIENT),
            None,
        ))
        .unwrap();
    assert_prompt(&renders);
    assert_eq!(
        focused_pane(&mock, CLIENT),
        (TabId::new(1), PaneId::Plugin(PLUGIN))
    );
    let arriving = ClientId::new(3);
    mock.to_screen
        .send(ScreenInstruction::AddClient(
            arriving,
            SIZE,
            Some(yinz_types::internal_ids::TabNumber::new(2)),
            None,
        ))
        .unwrap();
    assert_eq!(
        focused_pane(&mock, arriving),
        (TabId::new(1), PaneId::Plugin(PLUGIN))
    );
    mock.teardown(threads);
}

#[test]
fn background_permission_survives_initial_layout() {
    for hide_floating_panes in [false, true] {
        let (mut mock, renders, threads) = start();
        let layout = TiledPaneLayout {
            hide_floating_panes,
            ..Default::default()
        };
        mock.to_screen
            .send(ScreenInstruction::NewTab(
                None,
                None,
                Some(layout.clone()),
                vec![],
                None,
                (Some(vec![]), Some(vec![])),
                None,
                false,
                true,
                CLIENT,
                None,
            ))
            .unwrap();
        attach(&mock, TabId::new(1));
        request(&mock);
        assert!(
            pane_info(&mock, PaneId::Plugin(PLUGIN))
                .unwrap()
                .is_suppressed,
            "permission presentation must wait until the initial layout settles"
        );
        mock.to_screen
            .send(ScreenInstruction::ApplyLayout(
                layout,
                vec![],
                vec![(TerminalId::new(1), None)],
                vec![],
                vec![],
                HashMap::new(),
                TabId::new(1),
                true,
                CLIENT,
                None,
                None,
            ))
            .unwrap();
        assert_prompt(&renders);
        assert_eq!(focused_pane(&mock, CLIENT).1, PaneId::Plugin(PLUGIN));
        mock.teardown(threads);
    }
}

#[test]
fn background_permission_replays_on_pane_attachment() {
    let (mut mock, renders, threads) = start();
    request(&mock);
    assert!(pane_info(&mock, PaneId::Plugin(PLUGIN)).is_none());
    attach(&mock, TabId::new(0));
    assert_prompt(&renders);
    assert_eq!(focused_pane(&mock, CLIENT).1, PaneId::Plugin(PLUGIN));
    mock.teardown(threads);
}

#[test]
fn background_permission_schedules_render_when_idle() {
    let (mut mock, renders, threads) = start();
    attach(&mock, TabId::new(0));
    assert!(
        pane_info(&mock, PaneId::Plugin(PLUGIN))
            .unwrap()
            .is_suppressed
    );
    // Fence the background queue, then the screen queue. All preceding render jobs have
    // returned to the dispatcher before the permission request; there is no animation clock
    // in this mock to hide a missing render request.
    mock.to_background_jobs
        .send(BackgroundJob::WatchCommandPaletteSessions(true))
        .unwrap();
    mock.received_background_jobs.wait_until(
        "background queue drained",
        yinz_test_support::default_timeout(),
        |jobs| {
            jobs.iter()
                .any(|job| matches!(job, BackgroundJob::WatchCommandPaletteSessions(true)))
                .then_some(())
        },
    );
    pane_info(&mock, PaneId::Plugin(PLUGIN));
    let checkpoint = mock.received_background_jobs.lock().len();
    request(&mock);
    mock.received_background_jobs.wait_until(
        "permission schedules a render",
        yinz_test_support::default_timeout(),
        |jobs| {
            jobs[checkpoint..]
                .iter()
                .any(|job| matches!(job, BackgroundJob::RenderToClients))
                .then_some(())
        },
    );
    assert_prompt(&renders);
    mock.teardown(threads);
}

#[test]
fn background_permission_restores_placement() {
    let (mut mock, renders, mut threads) = start();
    let answers = Watched::new(vec![]);
    let receiver = mock.plugin_receiver.take().unwrap();
    threads.push(log_actions_in_thread!(
        answers,
        PluginInstruction::Exit,
        receiver
    ));
    attach(&mock, TabId::new(0));
    for (key, expected) in [
        (b'y', PermissionStatus::Granted),
        (b'n', PermissionStatus::Denied),
    ] {
        let checkpoint = renders.lock().len();
        request(&mock);
        wait_for_render_after(
            &renders,
            SIZE,
            checkpoint,
            "fresh permission prompt",
            |_, frame| frame.contains("Allow? (y/n)"),
        );
        mock.to_screen
            .send(ScreenInstruction::WriteCharacter(
                None,
                vec![key],
                false,
                CLIENT,
                None,
            ))
            .unwrap();
        answers.wait_until("permission answer reaches host", yinz_test_support::default_timeout(), |events| {
            events.iter().any(|event| matches!(event,
                PluginInstruction::PermissionRequestResult(id, Some(client), permissions, status, None)
                if *id == PLUGIN && *client == CLIENT && permissions == &[PermissionType::ReadApplicationState] && *status == expected
            )).then_some(())
        });
        assert!(
            pane_info(&mock, PaneId::Plugin(PLUGIN))
                .unwrap()
                .is_suppressed
        );
        assert_eq!(
            focused_pane(&mock, CLIENT).1,
            PaneId::Terminal(TerminalId::new(0))
        );
        wait_for_render_after(
            &renders,
            SIZE,
            checkpoint,
            "answered prompt clears",
            |_, frame| !frame.contains("Allow? (y/n)"),
        );
    }
    mock.teardown(threads);
}
