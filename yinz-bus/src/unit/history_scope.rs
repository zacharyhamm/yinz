use super::{HistoryAction, HistoryScope};
use yinz_types::agent::history::{Cursor, Range};
#[test]
fn history_scopes_keep_search_worker_tools_and_chronological_cursors_independent() {
    let worker = HistoryAction::Conversation {
        id: "run".into(),
        worker: Some("one".into()),
        before: None,
    };
    let earlier = HistoryAction::Conversation {
        id: "run".into(),
        worker: Some("one".into()),
        before: Some(Cursor {
            created_at_ms: Some(1),
            revision: 5,
        }),
    };
    assert_eq!(worker.scope(), earlier.scope());
    let search = HistoryAction::Run {
        id: "run".into(),
        from: None,
        query: "worker:one".into(),
    };
    assert_ne!(worker.scope(), search.scope());
    assert!(matches!(search.scope(), Some((_, HistoryScope::Search(_)))));
    let tools = HistoryAction::Tools {
        id: "run".into(),
        range: Range {
            start: None,
            end: None,
            worker: Some("one".into()),
        },
        before: None,
    };
    assert_ne!(worker.scope(), tools.scope());
    assert_eq!(
        HistoryAction::Run {
            id: "run".into(),
            from: Some(500),
            query: String::new()
        }
        .scope(),
        HistoryAction::Conversation {
            id: "run".into(),
            worker: None,
            before: None
        }
        .scope()
    );
}
