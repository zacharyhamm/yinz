use super::*;
use unicode_width::UnicodeWidthStr;
use zellij_types::internal_ids::{PluginId, TabId};

/// The project a plugin says each tab is on, as `SetTabProjects` carries it.
fn set_projects(screen: &mut Screen, projects: &[(usize, &str)]) {
    screen.tab_projects = projects
        .iter()
        .map(|(tab_id, project)| (TabId::new(*tab_id), (*project).to_owned()))
        .collect();
}

fn name_of(screen: &Screen, tab_id: TabId) -> String {
    screen.plugin_tab_names().remove(&tab_id).expect("TEST")
}

#[test]
fn a_project_names_a_tab_that_still_has_its_default_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    assert_eq!(name_of(&screen, TabId::new(0)), "Tab #1");

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "hocus");
}

#[test]
fn a_tab_the_user_named_keeps_that_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.get_tab_by_id_mut(TabId::new(0)).unwrap().name = "notes".to_owned();

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "notes");
}

#[test]
fn a_project_that_goes_away_gives_the_tab_its_own_name_back() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    set_projects(&mut screen, &[(0, "hocus")]);
    assert_eq!(name_of(&screen, TabId::new(0)), "hocus");

    set_projects(&mut screen, &[]);

    assert_eq!(name_of(&screen, TabId::new(0)), "Tab #1");
}

#[test]
fn two_tabs_on_one_project_are_numbered_by_position() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    new_tab(&mut screen, 3, TabId::new(2));

    set_projects(&mut screen, &[(0, "hocus"), (1, "pocus"), (2, "hocus")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "hocus #1");
    assert_eq!(name_of(&screen, TabId::new(2)), "hocus #2");
    assert_eq!(
        name_of(&screen, TabId::new(1)),
        "pocus",
        "a project only one tab is on is not numbered"
    );
}

#[test]
fn the_last_tab_on_a_project_goes_back_to_the_plain_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    set_projects(&mut screen, &[(0, "hocus"), (1, "hocus")]);
    assert_eq!(name_of(&screen, TabId::new(1)), "hocus #2");

    screen.close_tab_by_id(TabId::new(0)).expect("TEST");

    assert_eq!(name_of(&screen, TabId::new(1)), "hocus");
}

/// The tab's own title is the more specific answer, and it moves with the work.
#[test]
fn a_lone_pane_lending_its_title_outranks_the_project() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    // Only a tab that draws pane titles has one to lend.
    let tab = screen.get_tab_by_id_mut(TabId::new(0)).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.rename_pane_by_pane_id(
        PaneId::Terminal(crate::TerminalId::new(1)),
        "vim README".as_bytes().to_vec(),
    )
    .expect("TEST");

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "[1] vim README");
}

/// A second pane means no title is being lent, so the project names the tab.
#[test]
fn a_tab_with_two_panes_takes_the_project_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    // Only a tab that draws pane titles has one to lend.
    let tab = screen.get_tab_by_id_mut(TabId::new(0)).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.rename_pane_by_pane_id(
        PaneId::Terminal(crate::TerminalId::new(1)),
        "vim README".as_bytes().to_vec(),
    )
    .expect("TEST");
    screen
        .get_tab_by_id_mut(TabId::new(0))
        .unwrap()
        .new_pane(
            PaneId::Terminal(crate::TerminalId::new(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .expect("TEST");

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "hocus");
}

/// The tab a lone pane exits in still says so, whatever name it is wearing.
///
/// The marker used to be baked into the lent name, so a pane that set no title
/// of its own handed back "Tab #1 [ EXITED ] " — which is not the tab's own name
/// and so read as a loan, and vetoed the project. The tab went from "hocus" to
/// "Tab #1 [ EXITED ]" the moment its shell stopped.
#[test]
fn a_pane_that_exits_without_a_title_keeps_the_projects_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.get_tab_by_id_mut(TabId::new(0)).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.hold_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        Some(2),
        false,
        RunCommand::default(),
    );

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "[1] hocus [ EXIT CODE: 2 ] "
    );
}

/// And a pane that *did* set a title keeps lending it, marker and all.
#[test]
fn a_pane_that_exits_with_a_title_still_lends_it() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.get_tab_by_id_mut(TabId::new(0)).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.rename_pane_by_pane_id(
        PaneId::Terminal(crate::TerminalId::new(1)),
        "cargo test".as_bytes().to_vec(),
    )
    .expect("TEST");
    tab.hold_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        None,
        false,
        RunCommand::default(),
    );

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "[1] cargo test [ EXITED ] "
    );
}

/// A tab in two projects is named after both, and the "#1"/"#2" a collision
/// earns is worked out on the whole combined label rather than on either half.
#[test]
fn two_tabs_sharing_a_combined_label_are_numbered_by_position() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    new_tab(&mut screen, 3, TabId::new(2));

    set_projects(
        &mut screen,
        &[(0, "hocus + pocus"), (1, "hocus"), (2, "hocus + pocus")],
    );

    assert_eq!(name_of(&screen, TabId::new(0)), "hocus + pocus #1");
    assert_eq!(name_of(&screen, TabId::new(2)), "hocus + pocus #2");
    assert_eq!(
        name_of(&screen, TabId::new(1)),
        "hocus",
        "a tab on one of the two projects collides with neither"
    );
}

/// A tab down to one pane wears that pane's id, the way the pane's own frame does.
///
/// Not a name and not in competition with one: it leads whichever of the three
/// names won, so a tab the user named and a tab on a project both keep saying
/// which pane `--pane-id` would reach.
#[test]
fn a_lone_pane_lends_its_id_to_a_tab_however_the_tab_is_named() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_tab_by_id_mut(TabId::new(0))
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Titles);

    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "[1] Tab #1",
        "the tab's own name"
    );

    set_projects(&mut screen, &[(0, "hocus")]);
    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "[1] hocus",
        "a project's name"
    );

    screen.get_tab_by_id_mut(TabId::new(0)).unwrap().name = "notes".to_owned();
    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "[1] notes",
        "a name the user typed"
    );
}

/// The id says which pane, so a tab holding more than one says nothing.
#[test]
fn a_tab_with_two_panes_wears_no_pane_id() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.get_tab_by_id_mut(TabId::new(0)).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(crate::ClientId::new(1)),
        None,
    )
    .expect("TEST");

    assert_eq!(name_of(&screen, TabId::new(0)), "Tab #1");
}

/// Everything decorating a name is charged to the budget, so the bars have nothing left
/// to cut off the end - which is where the exit marker is.
///
/// It used to be only the lent title that paid: a long project name plus the id in front
/// went past the 40 columns the bars allow, and the bar took the cut out of the tail.
#[test]
fn a_long_project_name_is_cut_rather_than_the_exit_marker() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.get_tab_by_id_mut(TabId::new(0)).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.hold_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        Some(2),
        false,
        RunCommand::default(),
    );

    set_projects(&mut screen, &[(0, &"p".repeat(60))]);

    let name = name_of(&screen, TabId::new(0));
    assert!(name.starts_with("[1] "), "the id survives: {:?}", name);
    assert!(
        name.ends_with(" [ EXIT CODE: 2 ] "),
        "and so does the whole marker: {:?}",
        name
    );
    assert!(name.width() <= 40, "{:?} is {} columns", name, name.width());
}

/// A name the user typed is cut the same way. It was not before - only the lent title was.
#[test]
fn a_long_name_the_user_typed_is_cut_to_the_same_budget() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.get_tab_by_id_mut(TabId::new(0)).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.name = "n".repeat(60);

    let name = name_of(&screen, TabId::new(0));
    assert!(name.starts_with("[1] n"), "{:?}", name);
    assert!(name.ends_with('…'), "the cut is visible: {:?}", name);
    assert_eq!(name.width(), 40);
}

/// The "#1"/"#2" a collision earns is a decoration too, and charged rather than cut: two
/// tabs on the same long project have nothing but that to tell them apart.
#[test]
fn a_collision_ordinal_survives_a_long_project_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));

    let project = "p".repeat(60);
    set_projects(&mut screen, &[(0, &project), (1, &project)]);

    let first = name_of(&screen, TabId::new(0));
    let second = name_of(&screen, TabId::new(1));
    assert!(first.ends_with(" #1"), "{:?}", first);
    assert!(second.ends_with(" #2"), "{:?}", second);
    assert_eq!(first.width(), 40);
    assert_eq!(second.width(), 40);
}

fn arrangement_group(identity: &str, pane_ids: &[u32]) -> zellij_types::data::ProjectPaneGroup {
    zellij_types::data::ProjectPaneGroup {
        identity: identity.into(),
        name: "same-name".into(),
        configuration_root: None,
        pane_ids: pane_ids.to_vec(),
    }
}

fn capture_arrangement_loads(screen: &mut Screen) -> Receiver<(PluginInstruction, ErrorContext)> {
    let (sender, receiver) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    receiver
}

fn apply_arrangement_loads(
    screen: &mut Screen,
    receiver: &Receiver<(PluginInstruction, ErrorContext)>,
) {
    let loads: Vec<_> = receiver
        .try_iter()
        .filter_map(|(instruction, _)| {
            if let PluginInstruction::NewTab(
                _,
                _,
                Some(layout),
                floating,
                id,
                _,
                _,
                focus,
                client,
                _,
            ) = instruction
            {
                assert_eq!(floating, Some(vec![]));
                assert!(!focus);
                Some((layout, id, client))
            } else {
                None
            }
        })
        .collect();
    for (layout, id, client) in loads {
        let mut plugins: HashMap<RunPluginOrAlias, Vec<PluginId>> = HashMap::new();
        for (index, run) in layout.extract_run_instructions().into_iter().enumerate() {
            let Some(Run::Plugin(plugin)) = run else {
                panic!("arrangement must not spawn terminals")
            };
            plugins
                .entry(plugin)
                .or_default()
                .push(PluginId::new((id.get() * 100 + index) as u32));
        }
        screen
            .apply_layout(
                layout,
                vec![],
                vec![],
                vec![],
                plugins,
                id,
                false,
                client,
                None,
            )
            .unwrap();
    }
}

#[test]
fn arrangement_loads_new_tab_chrome_without_spawning_template_terminals() {
    for (top, bottom) in [(1, 2), (2, 3), (0, 0)] {
        let mut screen = create_new_screen(
            Size {
                cols: 120,
                rows: 40,
            },
            true,
            true,
        );
        let receiver = capture_arrangement_loads(&mut screen);
        let bars = if top == 0 {
            String::new()
        } else {
            format!(r#"pane size={top} borderless=true {{ plugin location="zellij:tab-bar"; }}"#)
        };
        let footer = if bottom == 0 {
            String::new()
        } else {
            format!(
                r#"pane size={bottom} borderless=true {{ plugin location="zellij:status-bar"; }}"#
            )
        };
        *screen.default_layout = zellij_config::kdl::layout::from_kdl(
            &format!(
                r#"layout {{ new_tab_template {{
                {bars}
                pane {{ pane command="must-not-start"; pane; }}
                {footer}
                floating_panes {{ pane command="must-not-float"; }}
            }}; }}"#
            ),
            None,
            None,
            None,
        )
        .unwrap();
        arrangement_test_tab(
            &mut screen,
            0,
            &[1, 2, 3, 4],
            TiledPaneLayout {
                children: vec![TiledPaneLayout::default(); 4],
                ..Default::default()
            },
        );
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join(".zellij")).unwrap();
        std::fs::write(
            directory.path().join(".zellij/layout.kdl"),
            "layout { pane split_direction=\"vertical\" { pane; pane; }; }",
        )
        .unwrap();
        let mut b = arrangement_group("b", &[2, 4]);
        b.configuration_root = Some(directory.path().display().to_string());
        let focused = screen.get_active_pane_id(&ClientId::new(1));
        let groups = vec![
            arrangement_group("a", &[1]),
            b,
            arrangement_group("c", &[3]),
        ];
        screen
            .arrange_project_panes(groups.clone(), ClientId::new(1))
            .unwrap();
        assert!(screen.pending_arrangement.is_some());
        assert_eq!(screen.tabs[&TabId::new(0)].arrangement_pane_ids().len(), 4);
        // A second request must neither move panes nor allocate more tabs.
        screen
            .arrange_project_panes(groups.clone(), ClientId::new(1))
            .unwrap();
        assert_eq!(screen.tabs.len(), 3);
        apply_arrangement_loads(&mut screen, &receiver);
        assert!(screen.pending_arrangement.is_none());
        assert_eq!(screen.get_active_pane_id(&ClientId::new(1)), focused);
        for id in [1, 2] {
            let tab = &screen.tabs[&TabId::new(id)];
            assert_eq!(
                tab.get_tiled_pane_ids().len(),
                (if top == 0 { 0 } else { 2 }) + if id == 1 { 2 } else { 1 }
            );
            let terminal = tab
                .get_pane_with_id(PaneId::Terminal(TerminalId::new(id as u32 + 1)))
                .unwrap();
            let geom = terminal.position_and_size();
            assert_eq!(geom.cols.as_usize(), if id == 1 { 60 } else { 120 });
            assert_eq!(geom.y, top);
            assert_eq!(geom.rows.as_usize(), 40 - top - bottom);
            assert_eq!(tab.tiled_and_floating_pane_ids(), tab.get_tiled_pane_ids());
        }
        let before: Vec<_> = screen
            .tabs
            .values()
            .map(|tab| tab.get_tiled_pane_ids())
            .collect();
        screen
            .arrange_project_panes(groups, ClientId::new(1))
            .unwrap();
        assert!(screen.pending_arrangement.is_none());
        assert_eq!(
            before,
            screen
                .tabs
                .values()
                .map(|tab| tab.get_tiled_pane_ids())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn arrangement_cancels_when_a_destination_closes_during_loading() {
    let mut screen = create_new_screen(
        Size {
            cols: 120,
            rows: 40,
        },
        true,
        true,
    );
    let receiver = capture_arrangement_loads(&mut screen);
    arrangement_test_tab(
        &mut screen,
        0,
        &[1, 2],
        TiledPaneLayout {
            children: vec![TiledPaneLayout::default(); 2],
            ..Default::default()
        },
    );
    screen
        .arrange_project_panes(
            vec![arrangement_group("a", &[1]), arrangement_group("b", &[2])],
            ClientId::new(1),
        )
        .unwrap();
    screen.close_tab_by_id(TabId::new(1)).unwrap();
    apply_arrangement_loads(&mut screen, &receiver);
    assert!(screen.pending_arrangement.is_none());
    assert_eq!(screen.tabs[&TabId::new(0)].arrangement_pane_ids().len(), 2);
}

#[test]
fn arrangement_consolidates_without_changing_ids_focus_or_explicit_names() {
    let mut screen = create_new_screen(
        Size {
            cols: 120,
            rows: 40,
        },
        true,
        true,
    );
    for id in 0..4 {
        new_tab(&mut screen, id + 1, TabId::new(id as usize));
    }
    screen.tabs.get_mut(&TabId::new(0)).unwrap().name = "keep this".into();
    let groups = vec![
        arrangement_group("repo-a", &[3, 1]),
        arrangement_group("repo-b", &[4, 2]),
    ];
    screen
        .arrange_project_panes(groups.clone(), ClientId::new(1))
        .unwrap();
    assert_eq!(screen.tabs.len(), 2);
    assert_eq!(screen.tabs[&TabId::new(0)].name, "keep this");
    assert_eq!(
        screen.get_active_pane_id(&ClientId::new(1)),
        Some(PaneId::Terminal(TerminalId::new(4)))
    );
    let state = |screen: &Screen| {
        screen
            .tabs
            .values()
            .map(|tab| {
                let mut panes: Vec<_> = tab
                    .get_tiled_pane_ids()
                    .iter()
                    .map(|id| (*id, tab.get_pane_with_id(*id).unwrap().position_and_size()))
                    .collect();
                panes.sort_by_key(|(_, geom)| geom.logical_position);
                (tab.id, panes)
            })
            .collect::<Vec<_>>()
    };
    let before = state(&screen);
    assert_eq!(
        before[0].1.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![
            PaneId::Terminal(TerminalId::new(3)),
            PaneId::Terminal(TerminalId::new(1))
        ]
    );
    screen
        .arrange_project_panes(groups, ClientId::new(1))
        .unwrap();
    assert_eq!(state(&screen), before);
}

#[test]
fn arrangement_rejects_duplicate_ids_and_executable_layouts_before_movement() {
    let mut screen = create_new_screen(
        Size {
            cols: 120,
            rows: 40,
        },
        true,
        true,
    );
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .arrange_project_panes(
            vec![arrangement_group("repo", &[1, 1, 2])],
            ClientId::new(1),
        )
        .unwrap();
    assert_eq!(screen.tabs.len(), 2);
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".zellij")).unwrap();
    std::fs::write(
        directory.path().join(".zellij/layout.kdl"),
        "layout { pane command=\"sh\"; }",
    )
    .unwrap();
    let mut group = arrangement_group("repo", &[1, 2]);
    group.configuration_root = Some(directory.path().display().to_string());
    screen
        .arrange_project_panes(vec![group], ClientId::new(1))
        .unwrap();
    assert_eq!(screen.tabs.len(), 2);
    assert!(screen.tabs[&TabId::new(0)].has_pane_with_pid(&PaneId::Terminal(TerminalId::new(1))));
    assert!(screen.tabs[&TabId::new(1)].has_pane_with_pid(&PaneId::Terminal(TerminalId::new(2))));
}

#[test]
fn arrangement_keeps_floating_geometry_and_agent_records() {
    let mut screen = create_new_screen(
        Size {
            cols: 120,
            rows: 40,
        },
        true,
        true,
    );
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    new_tab(&mut screen, 3, TabId::new(2));
    let floating = PaneId::Terminal(TerminalId::new(3));
    // Give the source a second tiled pane so floating one leaves a live tab.
    screen
        .break_multiple_panes_to_tab_with_id(
            vec![PaneId::Terminal(TerminalId::new(2))],
            TabId::new(2),
            false,
            ClientId::new(1),
        )
        .unwrap();
    let tab = screen.tabs.get_mut(&TabId::new(2)).unwrap();
    tab.toggle_pane_embed_or_floating_for_pane_id(floating, Some(ClientId::new(1)))
        .unwrap();
    let agent =
        serde_json::from_value(serde_json::json!({"kind":"codex", "state":"Busy", "cwd":"/work"}))
            .unwrap();
    tab.set_pane_agent(floating, Some(agent));
    let before = tab.get_pane_info(floating).unwrap();
    screen
        .arrange_project_panes(
            vec![arrangement_group("repo", &[1, 2, 3])],
            ClientId::new(1),
        )
        .unwrap();
    let tab = screen
        .tabs
        .values()
        .find(|tab| tab.has_pane_with_pid(&floating))
        .unwrap();
    let after = tab.get_pane_info(floating).unwrap();
    assert!(after.is_floating);
    assert_eq!(
        (
            after.pane_x,
            after.pane_y,
            after.pane_columns,
            after.pane_rows
        ),
        (
            before.pane_x,
            before.pane_y,
            before.pane_columns,
            before.pane_rows
        )
    );
    assert_eq!(after.agent, before.agent);
}

#[test]
fn arrangement_expands_layout_slots_and_skips_impossible_geometry() {
    let size = Size {
        cols: 120,
        rows: 40,
    };
    for source in [
        "layout { pane; }",
        "layout { pane stacked=true { pane; pane; }; }",
    ] {
        let layout = zellij_config::project_layout::parse(source).unwrap();
        let expanded = crate::tab::arrangement_layout(layout, &size, 6, None).unwrap();
        assert_eq!(expanded.pane_count(), 6);
        assert_eq!(
            expanded
                .position_panes_in_space(
                    &zellij_types::pane_size::PaneGeom::from(&size),
                    Some(6),
                    false,
                    false
                )
                .unwrap()
                .len(),
            6
        );
    }
    assert!(crate::tab::arrangement_layout(
        TiledPaneLayout::default(),
        &Size { cols: 5, rows: 2 },
        4,
        None
    )
    .is_err());
    let layout =
        zellij_config::project_layout::parse("layout { pane; pane; pane; pane; }").unwrap();
    assert_eq!(
        layout
            .position_panes_in_space(
                &zellij_types::pane_size::PaneGeom::from(&size),
                Some(2),
                false,
                false
            )
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn arrangement_manifest_preserves_nested_layout_traversal_order() {
    let mut screen = create_new_screen(
        Size {
            cols: 120,
            rows: 40,
        },
        true,
        true,
    );
    for id in 0..3 {
        new_tab(&mut screen, id + 1, TabId::new(id as usize));
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".zellij")).unwrap();
    std::fs::write(
        directory.path().join(".zellij/layout.kdl"),
        "layout { pane split_direction=\"vertical\" { pane { pane; pane; }; pane; }; }",
    )
    .unwrap();
    let mut group = arrangement_group("repo", &[3, 1, 2]);
    group.configuration_root = Some(directory.path().display().to_string());
    screen
        .arrange_project_panes(vec![group], ClientId::new(1))
        .unwrap();
    let ids: Vec<_> = screen.tabs[&TabId::new(0)]
        .pane_infos()
        .into_iter()
        .map(|info| info.id)
        .collect();
    assert_eq!(
        ids,
        vec![
            PaneId::Terminal(TerminalId::new(3)),
            PaneId::Terminal(TerminalId::new(1)),
            PaneId::Terminal(TerminalId::new(2))
        ]
    );
}

fn arrangement_test_tab(screen: &mut Screen, tab: usize, ids: &[u32], layout: TiledPaneLayout) {
    let tab = TabId::new(tab);
    screen
        .new_tab(tab, (vec![], vec![]), None, Some(ClientId::new(1)))
        .unwrap();
    screen
        .apply_layout(
            layout,
            vec![],
            ids.iter().map(|id| (TerminalId::new(*id), None)).collect(),
            vec![],
            HashMap::new(),
            tab,
            true,
            ClientId::new(1),
            None,
        )
        .unwrap();
}

#[test]
fn arrangement_preserves_boundary_bars_on_repeated_application() {
    use zellij_types::geometry::SplitSize;
    let mut screen = create_new_screen(Size { cols: 20, rows: 13 }, true, true);
    let bar = |height| TiledPaneLayout {
        split_size: Some(SplitSize::Fixed(height)),
        ..Default::default()
    };
    arrangement_test_tab(
        &mut screen,
        0,
        &[10, 1, 11],
        TiledPaneLayout {
            children: vec![bar(1), TiledPaneLayout::default(), bar(2)],
            ..Default::default()
        },
    );
    let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    for id in [10, 11] {
        tab.set_pane_selectable(PaneId::Terminal(TerminalId::new(id)), false);
    }
    let bars: Vec<_> = [10, 11]
        .map(|id| {
            let id = PaneId::Terminal(TerminalId::new(id));
            (id, tab.get_pane_with_id(id).unwrap().position_and_size())
        })
        .into();
    let viewport = tab.get_viewport();
    assert_eq!((viewport.y, viewport.rows), (1, 10));
    new_tab(&mut screen, 2, TabId::new(1));
    for _ in 0..2 {
        screen
            .arrange_project_panes(vec![arrangement_group("repo", &[1, 2])], ClientId::new(1))
            .unwrap();
        let tab = &screen.tabs[&TabId::new(0)];
        assert_eq!(tab.get_viewport(), viewport);
        for (id, geom) in &bars {
            assert_eq!(
                tab.get_pane_with_id(*id).unwrap().position_and_size(),
                *geom
            );
        }
        assert_eq!(tab.arrangement_pane_ids().len(), 2);
        for id in tab.arrangement_pane_ids() {
            let geom = tab.get_pane_with_id(id).unwrap().position_and_size();
            assert!(geom.y >= 1 && geom.y + geom.rows.as_usize() <= 11);
        }
    }
}

#[test]
fn arrangement_counts_final_membership_and_rechecks_rejected_projects() {
    for (cols, reject_second) in [(12, false), (22, true), (12, true)] {
        let mut screen = create_new_screen(Size { cols, rows: 5 }, true, true);
        let layout = TiledPaneLayout {
            children_split_direction: SplitDirection::Vertical,
            children: vec![TiledPaneLayout::default(); 2],
            ..Default::default()
        };
        arrangement_test_tab(&mut screen, 0, &[1, 2], layout.clone());
        arrangement_test_tab(&mut screen, 1, &[3, 4], layout);
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join(".zellij")).unwrap();
        // Valid syntax, impossible geometry: rejection happens during membership preflight.
        std::fs::write(
            directory.path().join(".zellij/layout.kdl"),
            "layout { pane size=1; pane; }",
        )
        .unwrap();
        let mut second = arrangement_group("b", &[4, 2]);
        if reject_second {
            second.configuration_root = Some(directory.path().display().to_string());
        }
        screen
            .arrange_project_panes(
                vec![arrangement_group("a", &[1, 3]), second],
                ClientId::new(1),
            )
            .unwrap();
        let ids = |tab| {
            let mut ids = screen.tabs[&TabId::new(tab)].get_tiled_pane_ids();
            ids.sort();
            ids
        };
        let expected = if !reject_second {
            (vec![1, 3], vec![2, 4])
        } else if cols == 22 {
            (vec![1, 2, 3], vec![4])
        } else {
            (vec![1, 2], vec![3, 4])
        };
        for (tab, expected) in [expected.0, expected.1].into_iter().enumerate() {
            assert_eq!(
                ids(tab),
                expected
                    .into_iter()
                    .map(|id| PaneId::Terminal(TerminalId::new(id)))
                    .collect::<Vec<_>>(),
                "cols={cols}, reject={reject_second}"
            );
        }
        assert_eq!(screen.tabs.len(), 2);
    }
}

#[test]
fn arrangement_splits_original_lists_by_project_and_preserves_separate_stacks() {
    let mut screen = create_new_screen(
        Size {
            cols: 160,
            rows: 60,
        },
        true,
        true,
    );
    *screen.stacked_pane_list.borrow_mut() = true;
    let pane = |id| PaneId::Terminal(TerminalId::new(id));
    for (tab_id, ids) in [(0, vec![1, 2, 3, 4, 5, 6, 7]), (1, vec![8, 9])] {
        new_tab(&mut screen, ids[0], TabId::new(tab_id));
        let tab = screen.tabs.get_mut(&TabId::new(tab_id)).unwrap();
        for id in &ids[1..] {
            tab.new_pane(
                pane(*id),
                None,
                None,
                false,
                true,
                NewPanePlacement::Stacked {
                    pane_id_to_stack_under: None,
                    borderless: None,
                },
                Some(ClientId::new(1)),
                None,
            )
            .unwrap();
        }
    }
    screen
        .focus_pane_with_id(pane(2), false, false, ClientId::new(1))
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".zellij")).unwrap();
    std::fs::write(
        directory.path().join(".zellij/layout.kdl"),
        "layout { pane size=1; pane; }",
    )
    .unwrap();
    let mut a = arrangement_group("a", &[3, 1, 9, 8]);
    a.configuration_root = Some(directory.path().display().to_string());
    let groups = vec![
        a,
        arrangement_group("b", &[4, 2]),
        arrangement_group("c", &[5]),
    ];
    let receiver = capture_arrangement_loads(&mut screen);
    let mut previous = None;
    for _ in 0..2 {
        screen
            .arrange_project_panes(groups.clone(), ClientId::new(1))
            .unwrap();
        apply_arrangement_loads(&mut screen, &receiver);
        screen.render(None).unwrap();
        assert_eq!(screen.tabs.len(), 3);
        assert_eq!(screen.get_active_pane_id(&ClientId::new(1)), Some(pane(2)));
        let destination = |id| {
            screen
                .tabs
                .values()
                .find(|tab| tab.has_pane_with_pid(&pane(id)))
                .unwrap()
        };
        assert_eq!(destination(1).id, destination(8).id);
        assert_ne!(destination(1).id, destination(2).id);
        assert_ne!(destination(2).id, destination(5).id);
        assert_eq!(destination(6).id, TabId::new(0));
        assert_eq!(destination(7).id, TabId::new(0));
        assert!(!destination(5).pane_is_stack_list_member(&pane(5)));
        let a_stacks = destination(1).arrangement_stacks();
        for (members, visible) in [(vec![3, 1], 3), (vec![9, 8], 9)] {
            assert!(a_stacks.iter().any(|stack| stack.members
                == members.iter().map(|id| pane(*id)).collect::<Vec<_>>()
                && stack.visible == pane(visible)));
        }
        assert_eq!(
            destination(2)
                .arrangement_stacks()
                .iter()
                .find(|stack| stack.members.contains(&pane(2)))
                .unwrap()
                .visible,
            pane(2)
        );
        for tab in screen.tabs.values() {
            let tiled = tab.get_tiled_pane_ids();
            for (i, id) in tiled.iter().enumerate() {
                let a = tab.get_pane_with_id(*id).unwrap().position_and_size();
                for other in &tiled[i + 1..] {
                    let b = tab.get_pane_with_id(*other).unwrap().position_and_size();
                    assert!(
                        a.x + a.cols.as_usize() <= b.x
                            || b.x + b.cols.as_usize() <= a.x
                            || a.y + a.rows.as_usize() <= b.y
                            || b.y + b.rows.as_usize() <= a.y
                    );
                }
            }
            for stack in tab.arrangement_stacks() {
                for id in stack.members {
                    let info = tab.get_pane_info(id).unwrap();
                    assert!(info.is_stack_list_member);
                    assert_eq!(info.is_suppressed, id != stack.visible);
                }
            }
        }
        let state: Vec<_> = screen
            .tabs
            .values()
            .flat_map(|tab| {
                let mut panes = tab.pane_infos();
                panes.sort_by_key(|info| info.id);
                panes.into_iter().map(move |info| (tab.id, info))
            })
            .collect();
        if let Some(previous) = &previous {
            assert_eq!(&state, previous);
        }
        previous = Some(state);
    }
}

#[test]
fn arrangement_stack_header_preflight_leaves_a_rejected_stack_untouched() {
    let mut screen = screen_with_a_stack_list();
    // A layout slot fits a terminal but not a three-member list and its headers.
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .resize_whole_tab(Size { cols: 80, rows: 5 })
        .unwrap();
    let before = screen.tabs[&TabId::new(0)].pane_infos();
    screen
        .arrange_project_panes(vec![arrangement_group("a", &[1, 2, 3])], ClientId::new(1))
        .unwrap();
    assert_eq!(screen.tabs[&TabId::new(0)].pane_infos(), before);
}

#[test]
fn arrangement_render_keeps_unknown_floats_and_floating_only_destinations_alive() {
    let mut screen = create_new_screen(
        Size {
            cols: 120,
            rows: 40,
        },
        true,
        true,
    );
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(tx));
    let pane = |id| PaneId::Terminal(TerminalId::new(id));
    arrangement_test_tab(
        &mut screen,
        0,
        &[1, 2],
        TiledPaneLayout {
            children: vec![TiledPaneLayout::default(); 2],
            ..Default::default()
        },
    );
    new_tab(&mut screen, 3, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .toggle_pane_embed_or_floating_for_pane_id(pane(2), Some(ClientId::new(1)))
        .unwrap();
    // Reserve the destination so the source is left with only its unknown float.
    screen
        .arrangement_projects
        .insert(TabId::new(1), "known".into());
    screen
        .arrange_project_panes(vec![arrangement_group("known", &[1, 3])], ClientId::new(1))
        .unwrap();
    screen.render(None).unwrap();
    assert!(
        screen.tabs[&TabId::new(0)]
            .get_pane_info(pane(2))
            .unwrap()
            .is_floating
    );
    screen
        .arrange_project_panes(vec![arrangement_group("float", &[2])], ClientId::new(1))
        .unwrap();
    screen.render(None).unwrap();
    assert_eq!(screen.tabs.len(), 2);
    assert!(
        screen.tabs[&TabId::new(0)]
            .get_pane_info(pane(2))
            .unwrap()
            .is_floating
    );
    assert!(rx.try_iter().all(|(instruction, _)| !matches!(
        instruction,
        PtyInstruction::ClosePane(_, _) | PtyInstruction::CloseTab(_)
    )));
}

#[test]
fn arrangement_selects_the_first_unknown_member_when_the_visible_member_leaves() {
    let mut screen = screen_with_a_stack_list();
    let pane = |id| PaneId::Terminal(TerminalId::new(id));
    screen
        .focus_pane_with_id(pane(2), false, false, ClientId::new(1))
        .unwrap();
    screen
        .arrange_project_panes(vec![arrangement_group("a", &[2, 9])], ClientId::new(1))
        .unwrap();
    let source = &screen.tabs[&TabId::new(0)];
    let stacks = source.arrangement_stacks();
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].members, vec![pane(1), pane(3)]);
    assert_eq!(stacks[0].visible, pane(1));
    assert_eq!(screen.get_active_pane_id(&ClientId::new(1)), Some(pane(2)));
}
