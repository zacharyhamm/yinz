//! Which layouts carry an agent to resume: a saved session's, and only for an agent still running.

use super::*;
use yinz_types::agent::AgentInfo;
use yinz_types::internal_ids::TabId;
use yinz_types::session_manifest::PaneLayoutManifest;

const TAB: TabId = TabId::new(0);
const PANE: PaneId = PaneId::Terminal(TerminalId::new(1));

fn screen_with_agent(state: &str) -> Screen {
    let mut screen = create_new_screen(
        Size {
            cols: 120,
            rows: 40,
        },
        true,
        true,
    );
    new_tab(&mut screen, 1, TAB);
    let agent: AgentInfo = serde_json::from_value(serde_json::json!({
        "kind": "claude", "state": state, "session_id": "conversation"
    }))
    .unwrap();
    screen
        .tabs
        .get_mut(&TAB)
        .unwrap()
        .set_pane_agent(PANE, Some(agent));
    screen
}

fn serialized(metadata: crate::session_layout_metadata::SessionLayoutMetadata) -> String {
    yinz_session::session_serialization::serialize_session_layout(metadata.into())
        .unwrap()
        .0
}

#[test]
fn only_a_saved_session_carries_a_running_agent() {
    let screen = screen_with_agent("Busy");
    assert!(serialized(screen.session_save_layout_metadata(None)).contains("conversation"));
    assert!(!serialized(screen.get_layout_metadata(None, None)).contains("conversation"));
    assert!(!serialized(screen.get_layout_metadata(None, Some(TAB))).contains("conversation"));
}

#[test]
fn saved_session_leaves_an_exited_agent_behind() {
    let screen = screen_with_agent("Exited");
    assert!(!serialized(screen.session_save_layout_metadata(None)).contains("conversation"));
}

#[test]
fn warp_manifest_gains_no_record_for_a_live_agent() {
    let screen = screen_with_agent("Busy");
    let (tiled, _, _) = screen.tab_panes_for_layout(
        TAB,
        &screen.tabs[&TAB],
        false,
        crate::screen::LayoutPurpose::Warp,
    );
    let (_, pane) = tiled.into_iter().find(|(id, _)| *id == PANE).unwrap();
    let manifest: PaneLayoutManifest = pane.into();
    assert!(!matches!(
        manifest.run,
        Some(Run::Command(RunCommand {
            resume_agent: Some(_),
            ..
        }))
    ));
}
