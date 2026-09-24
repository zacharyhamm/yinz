use super::*;

#[test]
fn frames_split_anywhere_are_reassembled() {
    let stream = b"data: {\"type\":\"ping\"}\n\ndata: {\"type\":\"issue_updated\",\"issueNumber\":7}\n\ndata: {\"type\":\"comment_added\",\"issueNumber\":7,\"commentId\":\"c-1\"}\n\n";
    for split in 0..stream.len() {
        let mut parser = Parser::new();
        let mut events = parser.push(&stream[..split]);
        events.extend(parser.push(&stream[split..]));
        assert_eq!(
            events,
            vec![
                StreamEvent::IssueUpdated { issue_number: 7 },
                StreamEvent::CommentAdded {
                    issue_number: 7,
                    comment_id: "c-1".into(),
                },
            ],
            "split at {split}"
        );
    }
}

#[test]
fn every_known_event_type_decodes() {
    let mut parser = Parser::new();
    let events = parser.push(
        concat!(
            "data: {\"type\":\"issue_created\",\"issueNumber\":1}\r\n\r\n",
            "data: {\"type\":\"issue_deleted\",\"issueNumber\":2}\n\n",
            "data: {\"type\":\"comment_updated\",\"issueNumber\":3,\"commentId\":\"a\"}\n\n",
            "data: {\"type\":\"comment_deleted\",\"issueNumber\":4,\"commentId\":\"b\"}\n\n",
            "data: {\"type\":\"lifecycle_added\",\"issueNumber\":5,\"entryId\":\"e\"}\n\n",
            "data: {\"type\":\"issue_starred\",\"issueNumber\":6}\n\n",
        )
        .as_bytes(),
    );
    assert_eq!(
        events,
        vec![
            StreamEvent::IssueCreated { issue_number: 1 },
            StreamEvent::IssueDeleted { issue_number: 2 },
            StreamEvent::CommentUpdated {
                issue_number: 3,
                comment_id: "a".into(),
            },
            StreamEvent::CommentDeleted {
                issue_number: 4,
                comment_id: "b".into(),
            },
            StreamEvent::LifecycleAdded {
                issue_number: 5,
                entry_id: "e".into(),
            },
            StreamEvent::Other {
                kind: "issue_starred".into(),
                issue_number: Some(6),
            },
        ]
    );
    assert_eq!(events[5].issue_number(), Some(6));
}

#[test]
fn comments_garbage_and_incomplete_events_are_dropped() {
    let mut parser = Parser::new();
    let events = parser.push(
        b": keep-alive\n\ndata: not json\n\ndata: {\"type\":\"comment_added\",\"issueNumber\":1}\n\nevent: x\n\n",
    );
    assert!(events.is_empty(), "{events:?}");
    // A frame still waiting for its blank line stays pending.
    assert!(parser
        .push(b"data: {\"type\":\"issue_updated\",\"issueNumber\":9}\n")
        .is_empty());
    assert_eq!(
        parser.push(b"\n"),
        vec![StreamEvent::IssueUpdated { issue_number: 9 }]
    );
}
