use super::*;
use yinz_tile::prelude::TabId;
use yinz_tile::prelude::TabPosition;
use yinz_tile::prelude::{PaneInfo, PaneManifest, TabInfo};

fn tab(position: usize, tab_id: TabId, name: &str) -> TabInfo {
    TabInfo {
        position: TabPosition::new(position),
        tab_id,
        name: name.to_owned(),
        active: position == 0,
        ..Default::default()
    }
}

fn pane(id: u32, title: &str) -> PaneInfo {
    PaneInfo {
        id: yinz_tile::prelude::PaneId::from_legacy(id, false),
        title: title.to_owned(),
        is_selectable: true,
        ..Default::default()
    }
}

fn session(tabs: Vec<TabInfo>, panes: PaneManifest) -> SessionInfo {
    SessionInfo {
        name: "work".to_owned().into(),
        tabs,
        panes,
        is_current_session: true,
        ..Default::default()
    }
}

/// The listing is the session's tabs and their panes, and nothing else.
#[test]
fn a_listing_is_the_session_name_and_its_tabs() {
    let mut manifest = PaneManifest::default();
    manifest
        .panes
        .insert(TabPosition::new(0), vec![pane(3, "vim")]);

    let listing = session_listing(&session(vec![tab(0, TabId::new(1), "edit")], manifest));
    assert_eq!(listing["session"], "work");
    let panes = listing["tabs"][0]["panes"].as_array().unwrap();
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0]["pane_id"], 3);
}
