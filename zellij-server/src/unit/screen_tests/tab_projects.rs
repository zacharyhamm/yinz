use super::*;

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
    tab.rename_pane_by_pane_id(PaneId::Terminal(1), "vim README".as_bytes().to_vec())
        .expect("TEST");

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, 0), "vim README");
}

/// A second pane means no title is being lent, so the project names the tab.
#[test]
fn a_tab_with_two_panes_takes_the_project_name() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 20 }, true, true);
    new_tab(&mut screen, 1, 0);
    // Only a tab that draws pane titles has one to lend.
    let tab = screen.get_tab_by_id_mut(0).unwrap();
    tab.set_pane_frames(PaneFrameStyle::Titles);
    tab.rename_pane_by_pane_id(PaneId::Terminal(1), "vim README".as_bytes().to_vec())
        .expect("TEST");
    screen
        .get_tab_by_id_mut(0)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .expect("TEST");

    set_projects(&mut screen, &[(0, "hocus")]);

    assert_eq!(name_of(&screen, 0), "hocus");
}
