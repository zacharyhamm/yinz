use super::*;
use crate::route::NotificationEnd;
use yinz_bus::pty::ReplacedPane;
use yinz_types::{data::PaneInfo, internal_ids::PluginId};

const FIRST: ClientId = ClientId::new(1);
const SECOND: ClientId = ClientId::new(2);
const UNKNOWN: ClientId = ClientId::new(99);
const FIRST_TAB: TabId = TabId::new(0);
const SECOND_TAB: TabId = TabId::new(1);

fn plugin() -> RunPluginOrAlias {
    yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:/path/to/fake/plugin",
        &None,
        None,
        None,
    )
    .unwrap()
}

fn two_client_screen() -> (
    MockScreen,
    Receiver<(PtyInstruction, ErrorContext)>,
    std::thread::JoinHandle<()>,
) {
    let size = Size {
        cols: 120,
        rows: 24,
    };
    let mut mock = MockScreen::new(size);
    mock.config.options.mirror_session = Some(false);
    let ptys = mock.pty_receiver.take().unwrap();
    let thread = mock.run(None, vec![]);
    mock.to_screen
        .send(ScreenInstruction::AddClient(SECOND, size, None, None))
        .unwrap();
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
            SECOND,
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
            SECOND_TAB,
            true,
            SECOND,
            None,
            None,
        ))
        .unwrap();
    assert_eq!(
        focused_pane(&mock, FIRST),
        (FIRST_TAB, PaneId::Terminal(TerminalId::new(0)))
    );
    assert_eq!(
        focused_pane(&mock, SECOND),
        (SECOND_TAB, PaneId::Terminal(TerminalId::new(1)))
    );
    ptys.try_iter().for_each(drop);
    (mock, ptys, thread)
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
        other => panic!("unexpected focused pane response: {other:?}"),
    }
}

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

fn pane_launches(client: ClientId, tab: Option<TabId>, no_focus: bool) -> Vec<ScreenInstruction> {
    vec![
        ScreenInstruction::NewTiledPluginPane(
            plugin(),
            Some("launch-title".into()),
            true,
            Some("/launch-cwd".into()),
            client,
            None,
            tab,
            no_focus,
        ),
        ScreenInstruction::NewFloatingPluginPane(
            plugin(),
            Some("launch-title".into()),
            true,
            Some("/launch-cwd".into()),
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                width: Some(PercentOrFixed::Fixed(40)),
                ..Default::default()
            })),
            client,
            None,
            tab,
            no_focus,
        ),
    ]
}

fn in_place_launches(client: ClientId, tab: Option<TabId>, pane: PaneId) -> Vec<ScreenInstruction> {
    vec![
        ScreenInstruction::NewInPlacePluginPane(
            plugin(),
            Some("replacement".into()),
            pane,
            true,
            true,
            client,
            None,
            tab,
            true,
        ),
        ScreenInstruction::LaunchPlugin(
            plugin(),
            false,
            true,
            true,
            Some(pane),
            true,
            Some("/launch-cwd".into()),
            client,
            None,
            tab,
            true,
            None,
        ),
        ScreenInstruction::LaunchOrFocusPlugin(
            plugin(),
            false,
            false,
            true,
            true,
            Some(pane),
            true,
            client,
            None,
            tab,
        ),
    ]
}

fn dispatch(
    mock: &MockScreen,
    ptys: &Receiver<(PtyInstruction, ErrorContext)>,
    instruction: ScreenInstruction,
) -> Option<PtyInstruction> {
    mock.to_screen.send(instruction).unwrap();
    // The reply follows the launch instruction, including failures that send nothing to PTY.
    pane_info(mock, PaneId::Terminal(TerminalId::new(0)));
    let mut launches = ptys
        .try_iter()
        .map(|(instruction, _)| instruction)
        .filter(|instruction| {
            matches!(
                instruction,
                PtyInstruction::FillPluginCwd(..) | PtyInstruction::ReloadPlugin { .. }
            )
        });
    let launch = launches.next();
    assert!(
        launches.next().is_none(),
        "one request must not launch twice"
    );
    launch
}

fn assert_rejected(
    mock: &MockScreen,
    ptys: &Receiver<(PtyInstruction, ErrorContext)>,
    mut instruction: ScreenInstruction,
    message: &str,
) {
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let completion = match &mut instruction {
        ScreenInstruction::NewTiledPluginPane(_, _, _, _, _, completion, _, _)
        | ScreenInstruction::NewFloatingPluginPane(_, _, _, _, _, _, completion, _, _)
        | ScreenInstruction::NewInPlacePluginPane(_, _, _, _, _, _, completion, _, _)
        | ScreenInstruction::LaunchPlugin(_, _, _, _, _, _, _, _, completion, _, _, _)
        | ScreenInstruction::LaunchOrFocusPlugin(_, _, _, _, _, _, _, _, completion, _)
        | ScreenInstruction::StartOrReloadPluginPane(_, _, _, completion) => completion,
        _ => panic!("unexpected launch instruction"),
    };
    *completion = Some(NotificationEnd::new(tx));
    assert!(dispatch(mock, ptys, instruction).is_none());
    let result = rx.try_recv().expect("failed launch must complete");
    assert_eq!(result.exit_status, Some(1));
    assert!(
        result
            .error_message
            .as_deref()
            .is_some_and(|error| error.contains(message)),
        "{result:?}"
    );
}

#[test]
fn tiled_and_floating_plugin_launches_follow_the_requesting_client_and_explicit_tab() {
    let (mut mock, ptys, thread) = two_client_screen();
    for no_focus in [false, true] {
        for (client, explicit, expected) in [
            (SECOND, None, SECOND_TAB),
            (SECOND, Some(FIRST_TAB), FIRST_TAB),
            (UNKNOWN, None, FIRST_TAB),
        ] {
            for instruction in pane_launches(client, explicit, no_focus) {
                let Some(PtyInstruction::FillPluginCwd(
                    floating,
                    in_place,
                    close,
                    title,
                    _,
                    tab,
                    replacement,
                    acting_client,
                    _,
                    skip_cache,
                    cwd,
                    focus,
                    coordinates,
                    _,
                    overlay,
                )) = dispatch(&mock, &ptys, instruction)
                else {
                    panic!("launch did not reach PTY");
                };
                assert_eq!(tab, expected);
                assert_eq!(acting_client, client);
                assert!(!in_place && !close && skip_cache);
                assert_eq!(title.as_deref(), Some("launch-title"));
                assert_eq!(cwd, Some(PathBuf::from("/launch-cwd")));
                assert_eq!(replacement, None);
                assert_eq!(focus, no_focus.then_some(false));
                assert_eq!(coordinates.is_some(), floating == Some(true));
                assert_eq!(overlay, None);
            }
        }
    }
    mock.teardown(vec![thread]);
}

#[test]
fn in_place_plugin_launch_metadata_and_placement_follow_the_replacement() {
    for explicit in [None, Some(FIRST_TAB), Some(TabId::new(99))] {
        for path in 0..3 {
            let (mut mock, ptys, thread) = two_client_screen();
            let target = PaneId::Terminal(TerminalId::new(1));
            let instruction = in_place_launches(FIRST, explicit, target).remove(path);
            let Some(PtyInstruction::FillPluginCwd(
                floating,
                in_place,
                close,
                title,
                run,
                tab,
                replacement,
                client,
                _,
                skip_cache,
                cwd,
                focus,
                coordinates,
                completion,
                overlay,
            )) = dispatch(&mock, &ptys, instruction)
            else {
                panic!("replacement did not reach PTY");
            };
            assert_eq!(tab, SECOND_TAB);
            assert_eq!(replacement, Some(target));
            assert_eq!(client, FIRST);
            assert!(in_place && close && skip_cache);
            assert_eq!(focus, (path != 2).then_some(false));
            let id = PluginId::new(42);
            mock.to_screen
                .send(ScreenInstruction::AddPlugin(
                    floating,
                    in_place,
                    ReplacedPane::closed_if(close),
                    run,
                    title,
                    Some(tab),
                    id,
                    replacement,
                    cwd,
                    false,
                    coordinates,
                    focus,
                    Some(client),
                    completion,
                    overlay,
                ))
                .unwrap();
            assert_eq!(
                focused_pane(&mock, SECOND),
                (SECOND_TAB, PaneId::Plugin(id))
            );
            assert!(pane_info(&mock, target).is_none());
            assert_eq!(focused_pane(&mock, FIRST).0, FIRST_TAB);
            mock.teardown(vec![thread]);
        }
    }
}

#[test]
fn plugin_launches_reject_missing_replacements_and_invalid_explicit_tabs() {
    let (mut mock, ptys, thread) = two_client_screen();
    for instruction in pane_launches(SECOND, Some(TabId::new(99)), false) {
        assert_rejected(&mock, &ptys, instruction, "Tab with id 99 not found");
    }
    for instruction in in_place_launches(
        SECOND,
        Some(FIRST_TAB),
        PaneId::Terminal(TerminalId::new(99)),
    ) {
        assert_rejected(&mock, &ptys, instruction, "not found");
    }
    mock.teardown(vec![thread]);
}

#[test]
fn detached_plugin_launches_require_an_explicit_destination_or_replacement() {
    let (mut mock, ptys, thread) = two_client_screen();
    for client in [FIRST, SECOND] {
        mock.to_screen
            .send(ScreenInstruction::RemoveClient(client))
            .unwrap();
    }
    for instruction in pane_launches(SECOND, Some(SECOND_TAB), true)
        .into_iter()
        .chain(in_place_launches(
            SECOND,
            Some(FIRST_TAB),
            PaneId::Terminal(TerminalId::new(1)),
        ))
    {
        assert!(
            matches!(dispatch(&mock, &ptys, instruction), Some(PtyInstruction::FillPluginCwd(_, _, _, _, _, tab, ..)) if tab == SECOND_TAB)
        );
    }
    for instruction in pane_launches(SECOND, None, false) {
        assert_rejected(&mock, &ptys, instruction, "No active tab");
    }
    mock.teardown(vec![thread]);
}

#[test]
fn start_or_reload_plugin_uses_the_acting_clients_tab_and_detached_first_tab() {
    let (mut mock, ptys, thread) = two_client_screen();
    for (client, expected) in [(SECOND, SECOND_TAB), (UNKNOWN, FIRST_TAB)] {
        let instruction = ScreenInstruction::StartOrReloadPluginPane(plugin(), None, client, None);
        assert!(
            matches!(dispatch(&mock, &ptys, instruction), Some(PtyInstruction::ReloadPlugin { tab_index, .. }) if tab_index == expected)
        );
    }
    for client in [FIRST, SECOND] {
        mock.to_screen
            .send(ScreenInstruction::RemoveClient(client))
            .unwrap();
    }
    // Removing tab zero proves that the detached fallback uses a real tab, not a default id.
    mock.to_screen
        .send(ScreenInstruction::CloseTabWithId(FIRST_TAB, None))
        .unwrap();
    let instruction = || ScreenInstruction::StartOrReloadPluginPane(plugin(), None, SECOND, None);
    assert!(
        matches!(dispatch(&mock, &ptys, instruction()), Some(PtyInstruction::ReloadPlugin { tab_index, .. }) if tab_index == SECOND_TAB)
    );
    mock.to_screen
        .send(ScreenInstruction::CloseTabWithId(SECOND_TAB, None))
        .unwrap();
    assert_rejected(&mock, &ptys, instruction(), "No active tab");
    for launch in pane_launches(SECOND, Some(SECOND_TAB), false) {
        assert_rejected(&mock, &ptys, launch, "not found");
    }
    mock.teardown(vec![thread]);
}
