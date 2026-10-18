use super::*;
use unicode_width::UnicodeWidthStr;
use yinz_types::internal_ids::{PluginId, TabId};

/// The project a plugin says each tab is on, as `SetTabProjects` carries it.
fn set_projects(screen: &mut Screen, projects: &[(usize, &str)]) {
    screen.tab_projects = projects
        .iter()
        .map(|(tab_id, project)| (TabId::new(*tab_id), (*project).to_owned()))
        .collect();
}

fn name_of(screen: &Screen, tab_id: TabId) -> String {
    let name = screen.plugin_tab_names().remove(&tab_id).expect("TEST");
    let client = ClientId::new(1);
    let tabs = screen.collect_tab_list(client).unwrap();
    assert_eq!(
        tabs.iter().find(|tab| tab.tab_id == tab_id).unwrap().name,
        name
    );
    for pane in screen
        .collect_pane_list(true)
        .unwrap()
        .iter()
        .filter(|pane| pane.tab_id == tab_id)
    {
        assert_eq!(pane.tab_name, name);
    }
    if screen.active_tab_ids.get(&client) == Some(&tab_id) {
        assert_eq!(
            screen.get_current_tab_info(client).unwrap().unwrap().name,
            name
        );
    }
    name
}

#[test]
fn project_changes_publish_group_names_for_manually_named_tabs() {
    let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
    let screen_thread = mock.run(None, vec![]);
    let instructions = Watched::new(vec![]);
    let receiver = mock.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(instructions, PluginInstruction::Exit, receiver);
    mock.to_screen
        .send(ScreenInstruction::SetCliEventSubscribersPresent(true))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::RenameTabWithId(
            TabId::new(0),
            b"notes".to_vec(),
            None,
        ))
        .unwrap();
    for project in ["x", "y"] {
        mock.to_screen
            .send(ScreenInstruction::SetProjectContext(
                vec![project.to_owned()],
                [(PaneId::Terminal(TerminalId::new(0)), project.to_owned())]
                    .into_iter()
                    .collect(),
            ))
            .unwrap();
        instructions.wait_until(
            "project change publishes the group even with an explicit tab title",
            yinz_test_support::default_timeout(),
            |log| {
                log.iter()
                    .any(|instruction| {
                        let PluginInstruction::Update(events) = instruction else {
                            return false;
                        };
                        events.iter().any(|(_, _, event)| {
                            let Event::TabUpdate(tabs) = event else {
                                return false;
                            };
                            tabs.iter().any(|tab| {
                                tab.name == "notes"
                                    && tab
                                        .group
                                        .as_ref()
                                        .is_some_and(|group| group.name == project)
                            })
                        })
                    })
                    .then_some(())
            },
        );
    }
    mock.teardown(vec![plugin_thread, screen_thread]);
}

#[test]
fn program_and_checkout_updates_wait_for_pane_creation_in_order() {
    let size = Size {
        cols: 100,
        rows: 24,
    };
    let mut mock = MockScreen::new(size);
    let screen_thread = mock.run(None, vec![]);
    let renders = Watched::new(vec![]);
    let receiver = mock.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(renders, ServerInstruction::KillSession, receiver);
    let id = TerminalId::new(9);
    let pane_id = PaneId::Terminal(id);
    for instruction in [
        ScreenInstruction::SetPaneTitlePrefix(pane_id, Some("yinz:topic".to_owned())),
        ScreenInstruction::UpdatePaneForegroundCommand(id, vec![], Some("/bin/zsh".to_owned())),
        ScreenInstruction::NewPane(
            pane_id,
            None,
            None,
            None,
            NewPanePlacement::default(),
            false,
            ClientTabIndexOrPaneId::ClientId(mock.main_client_id),
            None,
            false,
        ),
        ScreenInstruction::UpdatePaneForegroundCommand(
            id,
            vec!["/usr/bin/vim".to_owned()],
            Some("/usr/bin/vim".to_owned()),
        ),
        ScreenInstruction::PtyBytes(id, b"created".to_vec()),
    ] {
        mock.to_screen.send(instruction).unwrap();
    }
    wait_for_settled_render(
        &renders,
        size,
        "queued title reaches the new pane",
        |frame| frame.contains("yinz:topic: vim") && frame.contains("created"),
    );
    mock.teardown(vec![server_thread, screen_thread]);
}

#[test]
fn opening_a_scrollback_editor_by_pane_id_publishes_its_membership() {
    let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
    let screen_thread = mock.run(None, vec![]);
    let instructions = Watched::new(vec![]);
    let receiver = mock.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(instructions, PluginInstruction::Exit, receiver);
    mock.to_screen
        .send(ScreenInstruction::SetCliEventSubscribersPresent(true))
        .unwrap();
    mock.to_screen
        .send(ScreenInstruction::OpenInPlaceEditor(
            PaneId::Terminal(TerminalId::new(9)),
            ClientTabIndexOrPaneId::PaneId(PaneId::Terminal(TerminalId::new(0))),
        ))
        .unwrap();
    instructions.wait_until(
        "the editor and hidden source are published",
        yinz_test_support::default_timeout(),
        |log| {
            log.iter()
                .any(|instruction| {
                    let PluginInstruction::Update(events) = instruction else {
                        return false;
                    };
                    events.iter().any(|(_, _, event)| {
                        let Event::PaneUpdate(manifest) = event else {
                            return false;
                        };
                        let has_pane = |id, suppressed| {
                            manifest.panes.values().flatten().any(|pane| {
                                pane.id == PaneId::Terminal(TerminalId::new(id))
                                    && pane.is_suppressed == suppressed
                            })
                        };
                        has_pane(9, false) && has_pane(0, true)
                    })
                })
                .then_some(())
        },
    );
    mock.teardown(vec![plugin_thread, screen_thread]);
}

#[test]
fn a_project_names_a_tab_that_still_has_its_default_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    assert_eq!(name_of(&screen, TabId::new(0)), "Tab #1");

    set_projects(&mut screen, &[(0, "agent")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "agent");
}

#[test]
fn a_tab_the_user_named_keeps_that_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_tab_by_id_mut(TabId::new(0))
        .unwrap()
        .rename_by_user("notes".to_owned());

    set_projects(&mut screen, &[(0, "agent")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "notes");
}

/// Like `new_tab`, for a tab born with a name. Whose name it is is set apart from this - see
/// `Screen::tab_name_is_users` - so as it stands the name is a layout's.
fn new_named_tab(screen: &mut Screen, pid: u32, tab_id: TabId, name: &str) {
    let client_id = crate::ClientId::new(1);
    screen
        .new_tab(
            tab_id,
            (vec![], vec![]),
            Some(name.to_owned()),
            Some(client_id),
        )
        .expect("TEST");
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(crate::TerminalId::new(pid), None)],
            vec![],
            HashMap::new(),
            tab_id,
            true,
            client_id,
            None,
        )
        .expect("TEST");
}

/// A name a layout gave the tab is only a fallback: nobody chose it for the work now in the
/// tab. Laid-out tabs used to keep whatever they came with however the work in them changed.
#[test]
fn a_name_the_tab_was_born_with_gives_way_to_the_project() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_named_tab(&mut screen, 1, TabId::new(0), "editor");

    set_projects(&mut screen, &[(0, "agent")]);
    assert_eq!(name_of(&screen, TabId::new(0)), "agent");

    set_projects(&mut screen, &[]);
    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "editor",
        "the born name is what shows once nothing better is known",
    );
}

/// `new-tab --name logs`: the name was typed, so it is the user's as much as a rename is.
#[test]
fn a_name_typed_with_new_tab_holds_off_the_project() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_named_tab(&mut screen, 1, TabId::new(0), "logs");
    screen
        .get_tab_by_id_mut(TabId::new(0))
        .unwrap()
        .set_user_named(Screen::tab_name_is_users(Some("logs"), None));

    set_projects(&mut screen, &[(0, "agent")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "logs");
}

#[test]
fn whose_a_new_tabs_name_is() {
    let laid_out = TiledPaneLayout::default();
    let restored = TiledPaneLayout {
        user_named: true,
        ..Default::default()
    };
    assert!(Screen::tab_name_is_users(Some("logs"), None), "typed");
    assert!(
        !Screen::tab_name_is_users(Some("editor"), Some(&laid_out)),
        "a layout's own"
    );
    assert!(
        Screen::tab_name_is_users(Some("notes"), Some(&restored)),
        "a rename a resurrected session recorded, or a name a command filled in"
    );
    assert!(!Screen::tab_name_is_users(None, None), "no name at all");
    assert!(
        !Screen::tab_name_is_users(Some(""), None),
        "an empty name is none: the tab is \"Tab #N\""
    );
}

#[test]
fn a_lone_pane_lends_its_title_to_a_tab_born_named() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_named_tab(&mut screen, 1, TabId::new(0), "Tab #3");
    let tab = screen.get_tab_by_id_mut(TabId::new(0)).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.rename_pane_by_pane_id(
        PaneId::Terminal(crate::TerminalId::new(1)),
        "vim README".as_bytes().to_vec(),
    )
    .expect("TEST");

    assert_eq!(name_of(&screen, TabId::new(0)), "[1] vim README");
}

/// Keystrokes as rename mode delivers them, `\0` being the clear `TabNameInput 0` sends on entry.
fn type_in_rename_mode(screen: &mut Screen, keys: &[&str]) {
    let client_id = crate::ClientId::new(1);
    for key in keys {
        screen
            .update_active_tab_name(key.as_bytes().to_vec(), client_id)
            .expect("TEST");
    }
}

fn enter_mode(screen: &mut Screen, mode: InputMode) {
    screen
        .change_mode(mode, None, crate::ClientId::new(1))
        .expect("TEST");
}

#[test]
fn a_name_typed_in_rename_mode_holds_off_the_project() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    set_projects(&mut screen, &[(0, "agent")]);

    enter_mode(&mut screen, InputMode::RenameTab);
    type_in_rename_mode(&mut screen, &["\0", "n", "o"]);
    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "no",
        "the bar shows what is being typed, not the project",
    );
    enter_mode(&mut screen, InputMode::Normal);

    assert_eq!(name_of(&screen, TabId::new(0)), "no");
}

#[test]
fn an_empty_cli_tab_rename_restores_automatic_naming() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    set_projects(&mut screen, &[(0, "agent")]);

    type_in_rename_mode(&mut screen, &["\0", "notes"]);
    assert_eq!(name_of(&screen, TabId::new(0)), "notes");
    type_in_rename_mode(&mut screen, &["\0", ""]);
    assert_eq!(name_of(&screen, TabId::new(0)), "agent");

    enter_mode(&mut screen, InputMode::RenameTab);
    type_in_rename_mode(&mut screen, &["\0", ""]);
    assert_eq!(name_of(&screen, TabId::new(0)), "");
    type_in_rename_mode(&mut screen, &["draft", ""]);
    assert_eq!(name_of(&screen, TabId::new(0)), "draft");
}

#[test]
fn clearing_the_name_hands_the_tab_back_to_its_project() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_tab_by_id_mut(TabId::new(0))
        .unwrap()
        .rename_by_user("notes".to_owned());
    set_projects(&mut screen, &[(0, "agent")]);

    enter_mode(&mut screen, InputMode::RenameTab);
    type_in_rename_mode(&mut screen, &["\0"]);
    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "",
        "an emptied name is still being typed",
    );
    enter_mode(&mut screen, InputMode::Normal);
    assert_eq!(name_of(&screen, TabId::new(0)), "agent");

    set_projects(&mut screen, &[]);
    assert_eq!(name_of(&screen, TabId::new(0)), "Tab #1");
}

#[test]
fn undoing_a_rename_hands_the_tab_back_to_its_project() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    set_projects(&mut screen, &[(0, "agent")]);

    enter_mode(&mut screen, InputMode::RenameTab);
    type_in_rename_mode(&mut screen, &["\0", "x"]);
    screen
        .undo_active_rename_tab(crate::ClientId::new(1))
        .expect("TEST");
    enter_mode(&mut screen, InputMode::Normal);

    assert_eq!(name_of(&screen, TabId::new(0)), "agent");
}

/// A warped pane's default caption ("Pane #3") arrives as a caption, not a name, so a lone
/// warped pane lends its tab nothing and the project still names it.
#[test]
fn a_carried_default_caption_is_not_lent_to_the_tab() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .get_tab_by_id_mut(TabId::new(0))
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Titles);
    screen
        .with_pane(PaneId::Terminal(crate::TerminalId::new(1)), |pane| {
            pane.set_title("Pane #3".to_owned())
        })
        .expect("TEST");

    set_projects(&mut screen, &[(0, "agent")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "[1] agent");
}

fn adopt_tab(screen: &mut Screen, name: &str, user_named: bool) -> TabId {
    let tab_id = screen.get_new_tab_id();
    screen
        .place_adopted_tab(
            yinz_types::data::WarpedTab {
                name: Some(name.to_owned()),
                user_named,
                ..Default::default()
            },
            crate::pty::AdoptedTabPanes {
                tiled: vec![],
                floating: vec![],
            },
            crate::ClientId::new(1),
            None,
            None,
            None,
        )
        .expect("TEST");
    tab_id
}

#[test]
fn a_warped_in_tab_takes_the_project_unless_the_user_named_it() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    let arrived = adopt_tab(&mut screen, "Tab #3", false);
    let renamed = adopt_tab(&mut screen, "notes", true);

    set_projects(
        &mut screen,
        &[(arrived.get(), "agent"), (renamed.get(), "agent")],
    );

    assert_eq!(name_of(&screen, arrived), "agent");
    assert_eq!(
        name_of(&screen, renamed),
        "notes",
        "a rename the user made in the session it left still holds",
    );
}

/// The rename a warp carried is where the tab starts here, so there is nothing for an undo to
/// revert. It used to hand the tab to the project while leaving the name alone.
#[test]
fn undo_on_a_warped_in_tab_keeps_the_rename_it_carried() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    let renamed = adopt_tab(&mut screen, "notes", true);
    set_projects(&mut screen, &[(renamed.get(), "agent")]);

    let tab = screen.get_tab_by_id_mut(renamed).unwrap();
    tab.start_rename();
    assert!(
        !tab.undo_rename(),
        "an undo with nothing typed changes nothing"
    );

    assert_eq!(name_of(&screen, renamed), "notes");
}

#[test]
fn a_project_that_goes_away_gives_the_tab_its_own_name_back() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    set_projects(&mut screen, &[(0, "agent")]);
    assert_eq!(name_of(&screen, TabId::new(0)), "agent");

    set_projects(&mut screen, &[]);

    assert_eq!(name_of(&screen, TabId::new(0)), "Tab #1");
}

#[test]
fn two_tabs_on_one_project_are_numbered_by_position() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    new_tab(&mut screen, 2, TabId::new(1));
    new_tab(&mut screen, 3, TabId::new(2));

    set_projects(&mut screen, &[(0, "agent"), (1, "pocus"), (2, "agent")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "agent #1");
    assert_eq!(name_of(&screen, TabId::new(2)), "agent #2");
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
    set_projects(&mut screen, &[(0, "agent"), (1, "agent")]);
    assert_eq!(name_of(&screen, TabId::new(1)), "agent #2");

    screen.close_tab_by_id(TabId::new(0)).expect("TEST");

    assert_eq!(name_of(&screen, TabId::new(1)), "agent");
}

/// A lone pane's explicit title is more specific than its project name.
#[test]
fn a_lone_pane_title_outranks_the_project() {
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

    set_projects(&mut screen, &[(0, "agent")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "[1] vim README");
    set_projects(&mut screen, &[]);
    assert_eq!(name_of(&screen, TabId::new(0)), "[1] vim README");
}

#[test]
fn automatic_pane_titles_are_lent_without_becoming_explicit() {
    let mut screen = create_new_screen(
        Size {
            cols: 100,
            rows: 20,
        },
        true,
        true,
    );
    let tab_id = TabId::new(0);
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    new_tab(&mut screen, 1, tab_id);
    screen
        .get_tab_by_id_mut(tab_id)
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Titles);
    screen.with_pane(pane_id, |pane| {
        pane.set_title_prefix(Some("yinz:topic".to_owned()));
        assert!(!pane.has_explicit_title());
    });
    assert_eq!(name_of(&screen, tab_id), "[1] yinz:topic");
    screen.with_pane(pane_id, |pane| {
        pane.set_running_program(Some("/bin/zsh".to_owned()))
    });
    assert_eq!(name_of(&screen, tab_id), "[1] yinz:topic: zsh");
    screen
        .get_tab_by_id_mut(tab_id)
        .unwrap()
        .rename_by_user("my tab".to_owned());
    screen.with_pane(pane_id, |pane| {
        pane.set_running_program(Some("/usr/bin/vim".to_owned()))
    });
    assert_eq!(name_of(&screen, tab_id), "[1] my tab");
    screen
        .get_tab_by_id_mut(tab_id)
        .unwrap()
        .rename_by_user(String::new());
    assert_eq!(name_of(&screen, tab_id), "[1] yinz:topic: vim");
}

#[test]
fn a_lone_agent_panes_full_title_outranks_the_project() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    let tab_id = TabId::new(0);
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    new_tab(&mut screen, 1, tab_id);
    new_tab(&mut screen, 2, TabId::new(1));
    set_projects(&mut screen, &[(0, "agent"), (1, "agent")]);
    screen
        .get_tab_by_id_mut(tab_id)
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Titles);
    screen
        .with_pane(pane_id, |pane| {
            pane.handle_pty_bytes(b"\x1b]0;codex\x07".to_vec());
            pane.set_title_prefix(Some("agent:topic".to_owned()));
        })
        .expect("TEST");

    assert_eq!(name_of(&screen, tab_id), "[1] agent:topic: codex");
    assert_eq!(name_of(&screen, TabId::new(1)), "agent");

    screen
        .get_tab_by_id_mut(tab_id)
        .unwrap()
        .rename_by_user("notes".to_owned());
    assert_eq!(name_of(&screen, tab_id), "[1] notes");
    screen
        .get_tab_by_id_mut(tab_id)
        .unwrap()
        .rename_by_user(String::new());
    assert_eq!(name_of(&screen, tab_id), "[1] agent:topic: codex");

    let second_pane_id = PaneId::Terminal(TerminalId::new(3));
    screen
        .get_tab_by_id_mut(tab_id)
        .unwrap()
        .new_pane(
            second_pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(ClientId::new(1)),
            None,
        )
        .expect("TEST");
    assert_eq!(name_of(&screen, tab_id), "agent #1");
    assert_eq!(name_of(&screen, TabId::new(1)), "agent #2");
    screen
        .get_tab_by_id_mut(tab_id)
        .unwrap()
        .close_pane(second_pane_id, false, None);
    assert_eq!(name_of(&screen, tab_id), "[1] agent:topic: codex");
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

    set_projects(&mut screen, &[(0, "agent")]);

    assert_eq!(name_of(&screen, TabId::new(0)), "agent");
}

/// The tab a lone pane exits in still says so, whatever name it is wearing.
///
/// The marker used to be baked into the lent name, so a pane that set no title
/// of its own handed back "Tab #1 [ EXITED ] " — which is not the tab's own name
/// and so read as a loan, and vetoed the project. The tab went from "agent" to
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

    set_projects(&mut screen, &[(0, "agent")]);

    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "[1] agent [ EXIT CODE: 2 ] "
    );
}

/// A titled, retained exited pane keeps lending its title and exit marker.
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

    set_projects(&mut screen, &[(0, "agent")]);

    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "[1] cargo test [ EXITED ] "
    );
    set_projects(&mut screen, &[]);
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
        &[(0, "agent + pocus"), (1, "agent"), (2, "agent + pocus")],
    );

    assert_eq!(name_of(&screen, TabId::new(0)), "agent + pocus #1");
    assert_eq!(name_of(&screen, TabId::new(2)), "agent + pocus #2");
    assert_eq!(
        name_of(&screen, TabId::new(1)),
        "agent",
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

    set_projects(&mut screen, &[(0, "agent")]);
    assert_eq!(
        name_of(&screen, TabId::new(0)),
        "[1] agent",
        "a project's name"
    );

    screen
        .get_tab_by_id_mut(TabId::new(0))
        .unwrap()
        .rename_by_user("notes".to_owned());
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

#[test]
fn a_long_lent_pane_title_keeps_the_width_limit_and_exit_marker() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    let tab_id = TabId::new(0);
    let pane_id = PaneId::Terminal(TerminalId::new(1));
    new_tab(&mut screen, 1, tab_id);
    set_projects(&mut screen, &[(0, "agent")]);
    screen
        .get_tab_by_id_mut(tab_id)
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Titles);
    screen
        .with_pane(pane_id, |pane| {
            pane.rename("界".repeat(40).into_bytes());
            pane.set_title_prefix(Some("agent:topic".to_owned()));
        })
        .expect("TEST");

    for exited in [false, true] {
        if exited {
            screen.get_tab_by_id_mut(tab_id).unwrap().hold_pane(
                pane_id,
                Some(2),
                false,
                RunCommand::default(),
            );
        }
        let name = name_of(&screen, tab_id);
        assert!(name.starts_with("[1] agent:topic: "), "{name:?}");
        assert!(name.contains('…'), "{name:?}");
        assert!(name.width() <= 40, "{name:?} is {} columns", name.width());
        if exited {
            assert!(name.ends_with(" [ EXIT CODE: 2 ] "), "{name:?}");
        }
    }
}

/// A name the user typed is cut the same way. It was not before - only the lent title was.
#[test]
fn a_long_name_the_user_typed_is_cut_to_the_same_budget() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.get_tab_by_id_mut(TabId::new(0)).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.rename_by_user("n".repeat(60));

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

fn arrangement_group(identity: &str, pane_ids: &[u32]) -> yinz_types::data::ProjectPaneGroup {
    yinz_types::data::ProjectPaneGroup {
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
            .land_layout(
                layout,
                vec![],
                vec![],
                vec![],
                vec![],
                plugins,
                id,
                false,
                client,
                None,
                None,
                &mut super::super::thread_main::PendingEvents::default(),
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
            format!(r#"pane size={top} borderless=true {{ plugin location="yinz:tab-bar"; }}"#)
        };
        let footer = if bottom == 0 {
            String::new()
        } else {
            format!(
                r#"pane size={bottom} borderless=true {{ plugin location="yinz:status-bar"; }}"#
            )
        };
        *screen.default_layout = yinz_config::kdl::layout::from_kdl(
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
        std::fs::create_dir(directory.path().join(".yinz")).unwrap();
        std::fs::write(
            directory.path().join(".yinz/layout.kdl"),
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
    std::fs::create_dir(directory.path().join(".yinz")).unwrap();
    std::fs::write(
        directory.path().join(".yinz/layout.kdl"),
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
        let layout = yinz_config::project_layout::parse(source).unwrap();
        let expanded = crate::tab::arrangement_layout(layout, &size, 6, None).unwrap();
        assert_eq!(expanded.pane_count(), 6);
        assert_eq!(
            expanded
                .position_panes_in_space(
                    &yinz_types::pane_size::PaneGeom::from(&size),
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
    let layout = yinz_config::project_layout::parse("layout { pane; pane; pane; pane; }").unwrap();
    assert_eq!(
        layout
            .position_panes_in_space(
                &yinz_types::pane_size::PaneGeom::from(&size),
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
    std::fs::create_dir(directory.path().join(".yinz")).unwrap();
    std::fs::write(
        directory.path().join(".yinz/layout.kdl"),
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
    use yinz_types::geometry::SplitSize;
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
        std::fs::create_dir(directory.path().join(".yinz")).unwrap();
        // Valid syntax, impossible geometry: rejection happens during membership preflight.
        std::fs::write(
            directory.path().join(".yinz/layout.kdl"),
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
    std::fs::create_dir(directory.path().join(".yinz")).unwrap();
    std::fs::write(
        directory.path().join(".yinz/layout.kdl"),
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
        screen.render().unwrap();
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
    screen.render().unwrap();
    assert!(
        screen.tabs[&TabId::new(0)]
            .get_pane_info(pane(2))
            .unwrap()
            .is_floating
    );
    screen
        .arrange_project_panes(vec![arrangement_group("float", &[2])], ClientId::new(1))
        .unwrap();
    screen.render().unwrap();
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

#[test]
fn waiting_agent_bell_tracks_all_panes_and_moves_between_tabs() {
    use crate::screen::apply_pane_agent_report;
    use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
    let mut screen = create_new_screen(
        Size {
            cols: 120,
            rows: 30,
        },
        true,
        true,
    );
    let client = ClientId::new(1);
    let source = TabId::new(0);
    let destination = TabId::new(1);
    new_tab(&mut screen, 1, source);
    new_tab(&mut screen, 2, destination);
    let waiting = AgentInfo {
        kind: "codex".into(),
        state: AgentState::Waiting,
        source: AgentSource::Reported,
        detail: None,
        waiting_on: None,
        wait_needs_a_person: true,
        cwd: None,
        session_id: None,
        run_id: None,
    };
    let pane = |id| PaneId::Terminal(TerminalId::new(id));
    screen
        .tabs
        .get_mut(&source)
        .unwrap()
        .rename_by_user("界".repeat(40));
    for overlay in [false, true] {
        let id = if overlay { 4 } else { 3 };
        open_floating_pane(&mut screen, source, id, "waiting", overlay);
        apply_pane_agent_report(&mut screen, pane(id), Some(waiting.clone())).unwrap();
        assert!(name_of(&screen, source).starts_with("🔔 "));
        assert_eq!(name_of(&screen, source).matches('🔔').count(), 1);
        assert!(name_of(&screen, source).width() <= 40);
    }
    apply_pane_agent_report(&mut screen, pane(3), None).unwrap();
    assert!(
        name_of(&screen, source).starts_with("🔔 "),
        "the overlay still waits"
    );
    screen
        .tabs
        .get_mut(&source)
        .unwrap()
        .close_pane(pane(4), false, None);
    assert!(!name_of(&screen, source).contains('🔔'));
    apply_pane_agent_report(&mut screen, pane(3), Some(waiting.clone())).unwrap();
    screen
        .tabs
        .get_mut(&source)
        .unwrap()
        .suppress_pane(pane(3), None);
    assert!(
        name_of(&screen, source).starts_with("🔔 "),
        "suppressed panes still wait"
    );
    apply_pane_agent_report(&mut screen, pane(3), None).unwrap();
    assert!(!name_of(&screen, source).contains('🔔'));

    // Leave another pane behind so moving the waiter does not remove its source tab.
    open_floating_pane(&mut screen, source, 5, "another pane", false);
    apply_pane_agent_report(&mut screen, pane(1), Some(waiting)).unwrap();
    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![pane(1)], destination, true, client)
        .unwrap();
    assert!(outcome.succeeded());
    assert!(!name_of(&screen, source).contains('🔔'));
    assert!(name_of(&screen, destination).starts_with("🔔 "));
    screen
        .switch_active_tab(TabPosition::new(0), None, true, client)
        .unwrap();
    assert!(
        name_of(&screen, destination).starts_with("🔔 "),
        "switching tabs preserves the bell"
    );
    screen
        .tabs
        .get_mut(&destination)
        .unwrap()
        .close_pane(pane(1), false, None);
    assert!(!name_of(&screen, destination).contains('🔔'));
}

#[test]
fn session_snapshots_refresh_display_names_without_changing_layout_names() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_named_tab(&mut screen, 1, TabId::new(0), "editor");
    new_named_tab(&mut screen, 2, TabId::new(1), "shell");
    for project in [
        "yinz:topic",
        "a very long project and worktree label that needs shortening",
    ] {
        set_projects(&mut screen, &[(0, project), (1, project)]);
        screen.log_and_report_session_state().unwrap();
        let names = screen.plugin_tab_names();
        let session = &screen.peer_sessions_cache[&screen.session_name];
        for tab in &session.tabs {
            assert_eq!(tab.name, names[&tab.tab_id]);
            assert_eq!(tab.position, screen.tabs[&tab.tab_id].position);
        }
        let layout = screen.get_layout_metadata(None, None).to_layout_metadata();
        assert_eq!(
            layout
                .tabs
                .iter()
                .map(|tab| tab.name.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("editor"), Some("shell")]
        );
    }
}
