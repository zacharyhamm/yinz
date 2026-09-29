use super::*;
use yinz_types::{
    data::{PaneInfo, PaneManifest},
    internal_ids::{TabPosition, TerminalId},
};

#[test]
fn waits_check_initial_and_later_observations_and_do_not_confuse_disappearance_with_exit() {
    let pane_id = PaneId::Terminal(TerminalId::new(4));
    let mut request = Wait::PaneWait {
        pane_id,
        contains: Some("ready\nnext".into()),
        exited: false,
        closed: false,
        timeout: 30,
    };
    for initial in [true, false] {
        let render = |lines: &[&str]| ServerToClientMsg::PaneRenderUpdate {
            pane_id,
            viewport: lines.iter().map(|line| (*line).into()).collect(),
            scrollback: None,
            is_initial: initial,
        };
        assert!(observe(&request, pane_id, render(&["waiting"])).is_none());
        assert!(observe(&request, pane_id, render(&["ready", "next"]))
            .unwrap()
            .is_ok());
    }
    assert!(observe(
        &request,
        pane_id,
        ServerToClientMsg::SubscribedPaneClosed { pane_id }
    )
    .unwrap()
    .is_err());
    request = Wait::PaneWait {
        pane_id,
        contains: None,
        exited: true,
        closed: false,
        timeout: 30,
    };
    let update = |panes| ServerToClientMsg::EventUpdate {
        event_type: "pane_update".into(),
        event_json: serde_json::to_string(&Event::PaneUpdate(PaneManifest {
            panes: [(TabPosition::new(0), panes)].into(),
        }))
        .unwrap(),
    };
    assert!(observe(
        &request,
        pane_id,
        update(vec![PaneInfo {
            id: pane_id,
            ..Default::default()
        }])
    )
    .is_none());
    let observed = observe(
        &request,
        pane_id,
        update(vec![PaneInfo {
            id: pane_id,
            exited: true,
            exit_status: Some(17),
            ..Default::default()
        }]),
    )
    .unwrap()
    .unwrap();
    assert_eq!(observed["exit_status"], 17);
    assert!(observe(&request, pane_id, update(vec![])).unwrap().is_err());
    request = Wait::PaneWait {
        pane_id,
        contains: None,
        exited: false,
        closed: true,
        timeout: 30,
    };
    assert!(observe(&request, pane_id, update(vec![])).unwrap().is_ok());
}
