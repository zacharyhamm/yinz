use super::*;
use unicode_width::UnicodeWidthStr;

/// The project a plugin says each tab is on, as `SetTabProjects` carries it.
fn set_projects(screen: &mut Screen, projects: &[(usize, &str)]) {
    screen.tab_projects = projects
        .iter()
        .map(|(tab_id, project)| (*tab_id, (*project).to_owned()))
        .collect();
}

fn name_of(screen: &Screen, tab_id: usize) -> String {
    screen.plugin_tab_names().remove(&tab_id).expect("TEST")
}

#[test]
fn a_project_names_a_tab_that_still_has_its_default_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    assert_eq!(name_of(&screen, 0), "Tab #1");

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, 0), "hocus");
}

#[test]
fn a_tab_the_user_named_keeps_that_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    screen.get_tab_by_id_mut(0).unwrap().name = "notes".to_owned();

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, 0), "notes");
}

#[test]
fn a_project_that_goes_away_gives_the_tab_its_own_name_back() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    set_projects(&mut screen, &[(0, "hocus")]);
    assert_eq!(name_of(&screen, 0), "hocus");

    set_projects(&mut screen, &[]);

    assert_eq!(name_of(&screen, 0), "Tab #1");
}

#[test]
fn two_tabs_on_one_project_are_numbered_by_position() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);

    set_projects(&mut screen, &[(0, "hocus"), (1, "pocus"), (2, "hocus")]);

    assert_eq!(name_of(&screen, 0), "hocus #1");
    assert_eq!(name_of(&screen, 2), "hocus #2");
    assert_eq!(
        name_of(&screen, 1),
        "pocus",
        "a project only one tab is on is not numbered"
    );
}

#[test]
fn the_last_tab_on_a_project_goes_back_to_the_plain_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    set_projects(&mut screen, &[(0, "hocus"), (1, "hocus")]);
    assert_eq!(name_of(&screen, 1), "hocus #2");

    screen.close_tab_by_id(0).expect("TEST");

    assert_eq!(name_of(&screen, 1), "hocus");
}

/// The tab's own title is the more specific answer, and it moves with the work.
#[test]
fn a_lone_pane_lending_its_title_outranks_the_project() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    // Only a tab that draws pane titles has one to lend.
    let tab = screen.get_tab_by_id_mut(0).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.rename_pane_by_pane_id(
        PaneId::Terminal(crate::TerminalId(1)),
        "vim README".as_bytes().to_vec(),
    )
    .expect("TEST");

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, 0), "[1] vim README");
}

/// A second pane means no title is being lent, so the project names the tab.
#[test]
fn a_tab_with_two_panes_takes_the_project_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    // Only a tab that draws pane titles has one to lend.
    let tab = screen.get_tab_by_id_mut(0).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.rename_pane_by_pane_id(
        PaneId::Terminal(crate::TerminalId(1)),
        "vim README".as_bytes().to_vec(),
    )
    .expect("TEST");
    screen
        .get_tab_by_id_mut(0)
        .unwrap()
        .new_pane(
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId(1)),
            None,
        )
        .expect("TEST");

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, 0), "hocus");
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
    new_tab(&mut screen, 1, 0);
    let tab = screen.get_tab_by_id_mut(0).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.hold_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        Some(2),
        false,
        RunCommand::default(),
    );

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, 0), "[1] hocus [ EXIT CODE: 2 ] ");
}

/// And a pane that *did* set a title keeps lending it, marker and all.
#[test]
fn a_pane_that_exits_with_a_title_still_lends_it() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    let tab = screen.get_tab_by_id_mut(0).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.rename_pane_by_pane_id(
        PaneId::Terminal(crate::TerminalId(1)),
        "cargo test".as_bytes().to_vec(),
    )
    .expect("TEST");
    tab.hold_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        None,
        false,
        RunCommand::default(),
    );

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, 0), "[1] cargo test [ EXITED ] ");
}

/// A tab in two projects is named after both, and the "#1"/"#2" a collision
/// earns is worked out on the whole combined label rather than on either half.
#[test]
fn two_tabs_sharing_a_combined_label_are_numbered_by_position() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);

    set_projects(
        &mut screen,
        &[(0, "hocus + pocus"), (1, "hocus"), (2, "hocus + pocus")],
    );

    assert_eq!(name_of(&screen, 0), "hocus + pocus #1");
    assert_eq!(name_of(&screen, 2), "hocus + pocus #2");
    assert_eq!(
        name_of(&screen, 1),
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
    new_tab(&mut screen, 1, 0);
    screen
        .get_tab_by_id_mut(0)
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Titles);

    assert_eq!(name_of(&screen, 0), "[1] Tab #1", "the tab's own name");

    set_projects(&mut screen, &[(0, "hocus")]);
    assert_eq!(name_of(&screen, 0), "[1] hocus", "a project's name");

    screen.get_tab_by_id_mut(0).unwrap().name = "notes".to_owned();
    assert_eq!(name_of(&screen, 0), "[1] notes", "a name the user typed");
}

/// The id says which pane, so a tab holding more than one says nothing.
#[test]
fn a_tab_with_two_panes_wears_no_pane_id() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    let tab = screen.get_tab_by_id_mut(0).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(crate::ClientId(1)),
        None,
    )
    .expect("TEST");

    assert_eq!(name_of(&screen, 0), "Tab #1");
}

/// Everything decorating a name is charged to the budget, so the bars have nothing left
/// to cut off the end - which is where the exit marker is.
///
/// It used to be only the lent title that paid: a long project name plus the id in front
/// went past the 40 columns the bars allow, and the bar took the cut out of the tail.
#[test]
fn a_long_project_name_is_cut_rather_than_the_exit_marker() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    let tab = screen.get_tab_by_id_mut(0).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.hold_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        Some(2),
        false,
        RunCommand::default(),
    );

    set_projects(&mut screen, &[(0, &"p".repeat(60))]);

    let name = name_of(&screen, 0);
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
    new_tab(&mut screen, 1, 0);
    let tab = screen.get_tab_by_id_mut(0).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.name = "n".repeat(60);

    let name = name_of(&screen, 0);
    assert!(name.starts_with("[1] n"), "{:?}", name);
    assert!(name.ends_with('…'), "the cut is visible: {:?}", name);
    assert_eq!(name.width(), 40);
}

/// The "#1"/"#2" a collision earns is a decoration too, and charged rather than cut: two
/// tabs on the same long project have nothing but that to tell them apart.
#[test]
fn a_collision_ordinal_survives_a_long_project_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    let project = "p".repeat(60);
    set_projects(&mut screen, &[(0, &project), (1, &project)]);

    let first = name_of(&screen, 0);
    let second = name_of(&screen, 1);
    assert!(first.ends_with(" #1"), "{:?}", first);
    assert!(second.ends_with(" #2"), "{:?}", second);
    assert_eq!(first.width(), 40);
    assert_eq!(second.width(), 40);
}
