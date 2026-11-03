use super::*;
use yinz_types::plugin_data::{DataValue, FetchRequest, Pred};

fn entry(content: &str, host: &str, session: &str) -> ClipboardHistoryEntry {
    ClipboardHistoryEntry {
        content: content.into(),
        copied_at_ms: 1234,
        host_id: host.into(),
        yinz_session_name: session.into(),
        destination: CopyDestination::System,
    }
}

#[test]
fn clipboard_history_preserves_content_and_duplicates_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let contents = [
        "".to_owned(),
        "héllo\nworld\0end".into(),
        "x".repeat(4 * 1024 * 1024),
    ];
    {
        let data = PluginDataStore::new(dir.path());
        for content in &contents {
            for _ in 0..2 {
                record(&data, entry(content, "host-a", "session-a")).unwrap();
            }
        }
    }
    let data = PluginDataStore::new(dir.path());
    let request = FetchRequest::entity("clipboard_history");
    assert_eq!(data.count(store::SHARED, &request).unwrap(), 6);
    // Fetch one row at a time so the read budget cannot truncate the large records.
    let mut ids = vec![];
    for (index, content) in contents.iter().flat_map(|text| [text, text]).enumerate() {
        let rows = data
            .fetch(
                store::SHARED,
                &request.clone().offset(index as u32).limit(1),
            )
            .unwrap();
        assert_eq!(
            rows.get(&rows.rows[0], "content"),
            Some(&DataValue::Blob(content.as_bytes().to_vec()))
        );
        assert_eq!(
            rows.get(&rows.rows[0], "copied_at_ms"),
            Some(&DataValue::Int(1234))
        );
        assert_eq!(
            rows.get(&rows.rows[0], "destination"),
            Some(&DataValue::Text("system".into()))
        );
        ids.push(rows.get(&rows.rows[0], "id").cloned().unwrap());
    }
    for pair in ids.windows(2) {
        assert_ne!(pair[0], pair[1]);
    }
}

#[test]
fn clipboard_history_scopes_concurrent_writers_by_host_and_session() {
    let dir = tempfile::tempdir().unwrap();
    std::thread::scope(|scope| {
        for (host, session) in [("a", "one"), ("a", "two"), ("b", "one")] {
            let path = dir.path();
            scope.spawn(move || {
                let data = PluginDataStore::new(path);
                for _ in 0..3 {
                    record(&data, entry("same text", host, session)).unwrap();
                }
            });
        }
    });
    let data = PluginDataStore::new(dir.path());
    for (host, session) in [("a", "one"), ("a", "two"), ("b", "one")] {
        let request = FetchRequest::entity("clipboard_history")
            .filter(Pred::eq("host_id", host).and(Pred::eq("yinz_session_name", session)));
        assert_eq!(data.count(store::SHARED, &request).unwrap(), 3);
    }
}

#[test]
fn clipboard_history_reports_storage_errors() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-directory");
    std::fs::write(&file, b"").unwrap();
    let data = PluginDataStore::new(&file);
    assert!(record(&data, entry("text", "host", "session")).is_err());
}

#[test]
fn clipboard_worker_drains_in_order_and_continues_after_a_storage_error() {
    let dir = tempfile::tempdir().unwrap();
    let data = PluginDataStore::new(dir.path());
    let (to_plugin, receiver) = channels::unbounded();
    let senders = ThreadSenders {
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        ..Default::default()
    };
    let (shutdown, mut finished) = tokio::sync::mpsc::channel(1);
    let writer = start_worker(
        Arc::new(PluginDataStore::new(dir.path())),
        senders,
        shutdown,
    );
    // Text attributes reject NUL, exercising a failed write before successful writes.
    writer
        .send(Message::Record(entry("rejected", "bad\0host", "session")))
        .unwrap();
    for content in ["first", "first", "last"] {
        writer
            .send(Message::Record(entry(content, "host", "session")))
            .unwrap();
    }
    drop(writer);
    yinz_bus::global_async_runtime::get_tokio_runtime().block_on(async {
        assert_eq!(
            tokio::time::timeout(yinz_test_support::default_timeout(), finished.recv())
                .await
                .expect("clipboard worker must drain and release its shutdown token"),
            None
        );
    });

    let rows = data
        .fetch(store::SHARED, &FetchRequest::entity("clipboard_history"))
        .unwrap();
    let contents: Vec<_> = rows
        .rows
        .iter()
        .map(|row| rows.get(row, "content"))
        .collect();
    assert_eq!(
        contents,
        vec![
            Some(&DataValue::Blob(b"first".to_vec())),
            Some(&DataValue::Blob(b"first".to_vec())),
            Some(&DataValue::Blob(b"last".to_vec())),
        ]
    );
    let refreshes: Vec<_> = receiver.try_iter().collect();
    assert_eq!(refreshes.len(), 3, "only committed entries refresh watches");
    for (instruction, _) in refreshes {
        assert!(
            matches!(instruction, PluginInstruction::RefreshDataWatches { namespace, entities }
            if namespace == store::SHARED && entities == ["clipboard_history".into()].into())
        );
    }
}

#[test]
fn clipboard_update_pause_flushes_preceding_writes_and_holds_following_writes() {
    let dir = tempfile::tempdir().unwrap();
    let data = PluginDataStore::new(dir.path());
    let (shutdown, mut finished) = tokio::sync::mpsc::channel(1);
    let writer = start_worker(
        Arc::new(PluginDataStore::new(dir.path())),
        ThreadSenders::default(),
        shutdown,
    );
    writer
        .send(Message::Record(entry("before", "host", "session")))
        .unwrap();
    let gate = crate::update_quiescence::PauseGate::default();
    let (reply, arrival) = std::sync::mpsc::channel();
    writer.send(Message::Pause(gate.clone(), reply)).unwrap();
    writer
        .send(Message::Record(entry("after", "host", "session")))
        .unwrap();
    arrival
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    assert_eq!(
        data.count(store::SHARED, &FetchRequest::entity("clipboard_history"))
            .unwrap(),
        1
    );
    gate.resume();
    drop(writer);
    yinz_bus::global_async_runtime::get_tokio_runtime().block_on(async {
        assert_eq!(
            tokio::time::timeout(yinz_test_support::default_timeout(), finished.recv())
                .await
                .unwrap(),
            None
        );
    });
    assert_eq!(
        data.count(store::SHARED, &FetchRequest::entity("clipboard_history"))
            .unwrap(),
        2
    );
}
