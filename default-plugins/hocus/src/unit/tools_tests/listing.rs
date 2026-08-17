use super::*;
use zellij_tile::prelude::{PaneInfo, PaneManifest, TabInfo};

fn tab(position: usize, tab_id: usize, name: &str) -> TabInfo {
    TabInfo {
        position,
        tab_id,
        name: name.to_owned(),
        active: position == 0,
        ..Default::default()
    }
}

fn pane(id: u32, title: &str) -> PaneInfo {
    PaneInfo {
        id,
        title: title.to_owned(),
        is_selectable: true,
        ..Default::default()
    }
}

fn global(id: u32, title: &str, placement: GlobalPanePlacement) -> PaneInfo {
    PaneInfo {
        global_placement: Some(placement),
        ..pane(id, title)
    }
}

fn session(tabs: Vec<TabInfo>, panes: PaneManifest) -> SessionInfo {
    SessionInfo {
        name: "work".to_owned(),
        tabs,
        panes,
        is_current_session: true,
        ..Default::default()
    }
}

/// The gap this whole change exists to close: a global pane is in no tab, so the tab
/// walk that builds the listing never saw it and the model was told the session had
/// panes it could not see — including, usually, hocus itself.
#[test]
fn a_global_pane_is_listed_with_the_edge_and_size_that_place_it() {
    let mut manifest = PaneManifest::default();
    manifest.panes.insert(0, vec![pane(3, "vim")]);
    manifest.global_panes = vec![
        global(
            5,
            "hocus",
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Right,
                size: PercentOrFixed::Percent(30),
                length: None,
            },
        ),
        PaneInfo {
            is_hidden: true,
            ..global(
                6,
                "status",
                GlobalPanePlacement {
                    edge: GlobalPaneEdge::Bottom,
                    size: PercentOrFixed::Fixed(3),
                    length: None,
                },
            )
        },
    ];

    let listing = session_listing(&session(vec![tab(0, 1, "edit")], manifest));
    let globals = listing["global_panes"].as_array().expect("global_panes");
    assert_eq!(globals.len(), 2);

    assert_eq!(globals[0]["pane_id"], 5);
    assert_eq!(globals[0]["title"], "hocus");
    // The words the listing prints are the words the tool accepts, so the model can
    // write the follow-up call straight from what it read.
    assert_eq!(globals[0]["edge"], "right");
    assert_eq!(globals[0]["size"], "30%");
    assert_eq!(globals[0]["hidden"], false);

    assert_eq!(globals[1]["edge"], "bottom");
    assert_eq!(
        globals[1]["size"], "3",
        "cells are written without a suffix"
    );
    assert_eq!(globals[1]["hidden"], true);

    // ...and they are not smuggled into a tab, which has no room for them.
    let panes = listing["tabs"][0]["panes"].as_array().unwrap();
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0]["pane_id"], 3);
}

/// `list_session` is called on nearly every turn, and most sessions have no global
/// pane at all. An always-present empty array would be paid for by all of them.
#[test]
fn a_session_without_global_panes_carries_no_such_key() {
    let mut manifest = PaneManifest::default();
    manifest.panes.insert(0, vec![pane(3, "vim")]);

    let listing = session_listing(&session(vec![tab(0, 1, "edit")], manifest));
    assert!(listing.get("global_panes").is_none());
    assert_eq!(listing["session"], "work");
}
