use super::*;
use yinz_types::agent::{AgentInfo, AgentState};
use yinz_types::web::WebResponse;

fn response(status: u16) -> WebReply {
    WebReply::Response(WebResponse {
        status,
        headers: vec![],
        body: String::new(),
    })
}

#[test]
fn web_reply_slot_first_send_wins_across_clones() {
    let (slot, mut receiver) = ReplySlot::new();
    let clone = slot.clone();
    assert!(!slot.is_closed());
    assert!(clone.send(response(200)));
    assert!(
        slot.is_closed(),
        "an answered slot is closed on every clone"
    );
    assert!(!slot.send(response(500)), "a second answer is refused");
    assert_eq!(receiver.try_recv().expect("first reply"), response(200));
}

#[test]
fn web_reply_slot_reports_a_dropped_receiver() {
    let (slot, receiver) = ReplySlot::new();
    drop(receiver);
    assert!(slot.is_closed());
    assert!(!slot.send(response(200)));
}

#[test]
fn web_reply_slot_dropped_without_answer_closes_the_receiver() {
    let (slot, mut receiver) = ReplySlot::new();
    let clone = slot.clone();
    drop(slot);
    assert!(receiver.try_recv().is_err(), "a live clone keeps it open");
    drop(clone);
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    ));
}

fn live(kind: &str, session_id: Option<&str>) -> WebLiveAgent {
    let pane = PaneInfo {
        agent: Some(AgentInfo {
            kind: kind.into(),
            state: AgentState::Idle,
            source: yinz_types::agent::AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: session_id.map(str::to_owned),
            run_id: None,
        }),
        ..Default::default()
    };
    WebLiveAgent {
        id: "live-agent:0::main:7".into(),
        session: SessionName::new("main"),
        current_session: true,
        pane,
        metadata: BTreeMap::new(),
        run: None,
        label_source: AgentLabelSource {
            id: "live-agent:0::main:7".into(),
            host: None,
            cwd: None,
            run: None,
        },
        provider: kind.into(),
        activity: String::new(),
        host: None,
        pane_label: "7".into(),
        glyph: "○",
        status: "idle",
        focused: false,
        selectable: true,
        suppressed: false,
        usage_unavailable: None,
        details: String::new(),
        header: WebLiveHeader::default(),
    }
}

#[test]
fn web_live_agent_matches_only_a_complete_identical_ref() {
    let agent = live("claude", Some("abc"));
    let reference = WebAgentRef {
        id: agent.id.clone(),
        kind: "claude".into(),
        session_id: "abc".into(),
    };
    assert!(agent.matches(&reference));
    for changed in [
        WebAgentRef {
            id: "live-agent:0::main:8".into(),
            ..reference.clone()
        },
        WebAgentRef {
            kind: "codex".into(),
            ..reference.clone()
        },
        WebAgentRef {
            session_id: "other".into(),
            ..reference.clone()
        },
    ] {
        assert!(!agent.matches(&changed), "{changed:?}");
    }
    let empty = live("claude", Some(""));
    assert!(!empty.matches(&WebAgentRef {
        session_id: String::new(),
        ..reference.clone()
    }));
    assert!(!live("claude", None).matches(&reference));
}

#[test]
fn web_agent_ref_round_trips_through_json() {
    let reference: WebAgentRef =
        serde_json::from_str(r#"{"id":"live-agent:x","kind":"claude","session_id":"s"}"#)
            .expect("decode");
    assert_eq!(reference.kind, "claude");
    assert_eq!(
        serde_json::to_value(&reference).expect("encode")["session_id"],
        "s"
    );
}
