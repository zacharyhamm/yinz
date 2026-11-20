use super::*;
use unicode_width::UnicodeWidthStr;
use yinz_types::internal_ids::TabId;

/// The project a plugin says each tab is on, as `SetTabProjects` carries it.
fn set_projects(screen: &mut Screen, projects: &[(usize, &str)]) {
    screen.core.tab_projects = projects
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
    if screen.core.active_tab_ids.get(&client) == Some(&tab_id) {
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
                Default::default(),
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

/// A key or `UpdateTabName` carrying bytes that are not UTF-8 (a Latin-1 terminal, a stray
/// high byte) used to fail `update_active_tab_name`, and the `?` chain up to
/// `screen_thread_main` ended the session. The bytes are decoded lossily instead.
#[test]
fn non_utf8_input_in_rename_mode_is_decoded_lossily() {
    let client_id = crate::ClientId::new(1);
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));

    enter_mode(&mut screen, InputMode::RenameTab);
    type_in_rename_mode(&mut screen, &["\0", "a"]);
    let handled = screen
        .write_character(
            None,
            vec![b'c', b'a', b'f', 0xe9],
            false,
            client_id,
            &HashMap::new(),
            None,
        )
        .expect("a non-UTF-8 key in rename mode must not fail the screen thread");
    assert!(handled);
    screen
        .update_active_tab_name(vec![0xff, b'!'], client_id)
        .expect("a non-UTF-8 UpdateTabName must not fail the screen thread");
    assert_eq!(name_of(&screen, TabId::new(0)), "acaf\u{FFFD}\u{FFFD}!");

    enter_mode(&mut screen, InputMode::Normal);
    assert_eq!(name_of(&screen, TabId::new(0)), "acaf\u{FFFD}\u{FFFD}!");
}

/// The pane-rename counterpart: `RenamePane` keys and `UpdatePaneName` reach
/// `Tab::update_active_pane_name`, which decodes lossily too.
#[test]
fn non_utf8_input_in_pane_rename_mode_is_decoded_lossily() {
    let client_id = crate::ClientId::new(1);
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, TabId::new(0));

    enter_mode(&mut screen, InputMode::RenamePane);
    let tab = screen.get_tab_by_id_mut(TabId::new(0)).expect("TEST");
    tab.update_active_pane_name(b"\0".to_vec(), client_id)
        .expect("TEST");
    tab.update_active_pane_name(vec![b'v', 0xc3, b'm'], client_id)
        .expect("a non-UTF-8 pane name must not fail the rename");
    let title = screen
        .with_pane(PaneId::Terminal(crate::TerminalId::new(1)), |pane| {
            pane.custom_title()
        })
        .expect("TEST");
    assert_eq!(title.as_deref(), Some("v\u{FFFD}m"));
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
            false,
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
        .core
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
        .core
        .tabs
        .get_mut(&source)
        .unwrap()
        .close_pane(pane(4), false, None);
    assert!(!name_of(&screen, source).contains('🔔'));
    apply_pane_agent_report(&mut screen, pane(3), Some(waiting.clone())).unwrap();
    screen
        .core
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
        .core
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
        let session = &screen.core.peer_sessions_cache[&screen.core.session_name];
        for tab in &session.tabs {
            assert_eq!(tab.name, names[&tab.tab_id]);
            assert_eq!(tab.position, screen.core.tabs[&tab.tab_id].position);
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
