use super::*;
use serde_json::json;

fn hook() -> HookEvent {
    HookEvent {
        session_id: "conversation".into(),
        prompt_id: Some("prompt".into()),
        ..Default::default()
    }
}

#[test]
fn a_blocking_stop_continues_until_the_matching_transcript_boundary() {
    let mut turn = Turn::new(&hook());
    for record in [
        json!({"type":"system","subtype":"turn_duration","uuid":"old"}),
        json!({"type":"user","sessionId":"conversation","promptId":"prompt","uuid":"user"}),
        json!({"type":"assistant","parentUuid":"user","uuid":"first","message":{"content":[{"type":"text","text":"FIRST"}]}}),
        json!({"type":"user","promptId":"prompt","parentUuid":"first","uuid":"blocking-hook"}),
        json!({"type":"assistant","parentUuid":"blocking-hook","uuid":"last","message":{"content":[{"type":"thinking","thinking":"private"},{"type":"text","text":"SECOND"}]}}),
        json!({"type":"system","subtype":"turn_duration","parentUuid":"worker","isSidechain":true}),
        json!({"type":"system","subtype":"turn_duration","parentUuid":"unrelated"}),
    ] {
        assert!(!turn.record(&record).unwrap());
    }
    assert!(turn
        .record(&json!({"type":"system","subtype":"turn_duration","parentUuid":"last"}))
        .unwrap());
    assert_eq!(turn.last_text.as_deref(), Some("SECOND"));
}

#[test]
fn replacement_prompts_and_conversations_never_confirm_the_previous_turn() {
    for replacement in [
        json!({"type":"user","promptId":"new"}),
        json!({"sessionId":"other","type":"system","subtype":"turn_duration"}),
    ] {
        let mut turn = Turn::new(&hook());
        turn.record(&json!({"type":"user","promptId":"prompt","uuid":"user"}))
            .unwrap();
        assert!(turn.record(&replacement).is_err());
    }
}

#[test]
fn missing_transcripts_and_exited_processes_are_unavailable() {
    let process = sysinfo::Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[process]), true);
    let started = system.process(process).unwrap().start_time();
    let observation = Observation {
        hook: HookEvent {
            transcript_path: Some("/nonexistent/yinz-claude.jsonl".into()),
            ..hook()
        },
        route: Route::Local {
            session: "test".into(),
            pane: "7".into(),
        },
        process: Some((std::process::id(), started)),
        conversation_file: None,
    };
    assert!(follow(&observation).is_err());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("turn.jsonl");
    std::fs::write(&path, "").unwrap();
    let observation = Observation {
        hook: HookEvent {
            transcript_path: Some(path.to_string_lossy().into_owned()),
            ..hook()
        },
        process: Some((std::process::id(), started + 1)),
        ..observation
    };
    assert!(follow(&observation).is_err());
}

#[test]
fn a_partial_transcript_record_is_not_completion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("turn.jsonl");
    let mut writer = std::fs::File::create(&path).unwrap();
    writer
        .write_all(b"{\"type\":\"system\",\"subtype\":\"turn_duration\"")
        .unwrap();
    let mut reader = BufReader::new(std::fs::File::open(&path).unwrap());
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).unwrap();
    assert_ne!(line.last(), Some(&b'\n'));
    writer.write_all(b",\"parentUuid\":\"user\"}\n").unwrap();
    reader.read_until(b'\n', &mut line).unwrap();
    let mut turn = Turn::new(&hook());
    turn.record(&json!({"type":"user","promptId":"prompt","uuid":"user"}))
        .unwrap();
    assert!(turn
        .record(&serde_json::from_slice(&line).unwrap())
        .unwrap());
}

fn observation(path: &std::path::Path, marker: Option<PathBuf>) -> Observation {
    let pid = sysinfo::Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    Observation {
        hook: HookEvent {
            transcript_path: Some(path.to_string_lossy().into_owned()),
            ..hook()
        },
        route: Route::Local {
            session: "test".into(),
            pane: "7".into(),
        },
        process: Some((
            std::process::id(),
            system.process(pid).unwrap().start_time(),
        )),
        conversation_file: marker,
    }
}

#[test]
fn an_observer_waits_for_initial_creation_and_complete_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("turn.jsonl");
    let observation = observation(&path, None);
    let (tx, rx) = std::sync::mpsc::channel();
    let observer = std::thread::spawn(move || tx.send(follow(&observation)).unwrap());
    // Timing is the behavior here: neither an absent file nor a partial record confirms a turn.
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    let mut writer = std::fs::File::create(&path).unwrap();
    writer.write_all(concat!(
        "{\"type\":\"user\",\"promptId\":\"prompt\",\"uuid\":\"user\"}\n",
        "{\"type\":\"assistant\",\"parentUuid\":\"user\",\"uuid\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"final\"}]}}\n",
        "{\"type\":\"system\",\"subtype\":\"turn_duration\",\"parentUuid\":\"assistant\"}"
    ).as_bytes()).unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    writer.write_all(b"\n").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .as_deref(),
        Some("final")
    );
    observer.join().unwrap();
}

#[test]
fn transcript_loss_and_conversation_replacement_stop_the_observer() {
    for replace in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("turn.jsonl");
        let marker = dir.path().join("conversation");
        std::fs::write(&marker, "conversation").unwrap();
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"promptId\":\"prompt\",\"uuid\":\"user\"}\n",
        )
        .unwrap();
        let observation = observation(&path, Some(marker.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        let observer = std::thread::spawn(move || tx.send(follow(&observation)).unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        if replace {
            std::fs::write(marker, "new-conversation").unwrap();
        } else {
            std::fs::remove_file(path).unwrap();
        }
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
        observer.join().unwrap();
    }
}
