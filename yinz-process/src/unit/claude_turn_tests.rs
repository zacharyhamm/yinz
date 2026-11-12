use super::*;
use serde_json::json;

fn follow(observation: &Observation) -> Result<Turn> {
    follow_with_api_reporting(observation, None, |_| Ok(()))
}

fn follow_with_api(observation: &Observation, api: Option<AgyApi>) -> Result<Turn> {
    follow_with_api_reporting(observation, api, |_| Ok(()))
}

fn hook() -> HookEvent {
    HookEvent {
        session_id: "conversation".into(),
        prompt_id: Some("prompt".into()),
        ..Default::default()
    }
}

#[test]
fn recovery_observer_lock_scopes_process_conversation_and_route_and_releases_on_exit() {
    let dir = tempfile::tempdir().unwrap();
    let mut observation = observation(&dir.path().join("root.jsonl"), None);
    let held = recovery::lock(&observation, dir.path()).unwrap().unwrap();
    assert!(recovery::lock(&observation, dir.path()).unwrap().is_none());
    observation.hook.prompt_id = Some("another-prompt".into());
    assert!(recovery::lock(&observation, dir.path()).unwrap().is_none());
    for changed in ["process", "conversation", "route"] {
        let mut other: Observation =
            serde_json::from_value(serde_json::to_value(&observation).unwrap()).unwrap();
        match changed {
            "process" => other.process.as_mut().unwrap().1 += 1,
            "conversation" => other.hook.session_id = "replacement".into(),
            _ => {
                other.route = Route::Local {
                    session: "different".into(),
                    pane: "7".into(),
                }
            },
        }
        assert!(recovery::lock(&other, dir.path()).unwrap().is_some());
    }
    drop(held);
    assert!(recovery::lock(&observation, dir.path()).unwrap().is_some());
}

#[test]
fn a_blocking_stop_continues_until_the_matching_transcript_boundary() {
    let mut turn = Turn::new("claude", &hook());
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
fn an_interruption_ends_the_turn_without_a_duration_record() {
    for content in [
        json!([{"type":"text","text":"[Request interrupted by user]"}]),
        json!("[Request interrupted by user]"),
    ] {
        let mut turn = Turn::new("claude", &hook());
        for record in [
            json!({"type":"user","sessionId":"conversation","promptId":"prompt","uuid":"user"}),
            json!({"type":"assistant","parentUuid":"user","uuid":"assistant","message":{"content":[{"type":"text","text":"Work so far."}]}}),
            json!({"type":"user","parentUuid":"assistant","uuid":"tool-result","toolUseResult":{}}),
        ] {
            assert!(!turn.record(&record).unwrap());
        }
        let mut interruption = interruption_record();
        interruption["message"]["content"] = content;
        assert!(turn.record(&interruption).unwrap());
        assert_eq!(turn.last_text.as_deref(), Some("Work so far."));
    }
}

// Minimal sanitized shape of the interruption record, which has no promptId.
fn interruption_record() -> Value {
    json!({
        "type": "user",
        "sessionId": "conversation",
        "isSidechain": false,
        "uuid": "interruption",
        "parentUuid": "tool-result",
        "interruptedMessageId": "interrupted-message",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": "[Request interrupted by user]"}]
        }
    })
}

#[test]
fn interruption_requires_metadata_exact_text_and_an_existing_parent_in_the_turn() {
    for change in [
        json!({"type":"assistant"}),
        json!({"parentUuid":"unrelated"}),
        json!({"parentUuid":null}),
        json!({"promptId":"prompt","parentUuid":"unrelated"}),
        json!({"promptId":"prompt","parentUuid":"interruption"}),
        json!({"interruptedMessageId":null}),
        json!({"interruptedMessageId":""}),
        json!({"interruptedMessageId":42}),
        json!({"message":{"content":"Quoted: [Request interrupted by user]"}}),
        json!({"message":{"content":[{"type":"text","text":"Quoted: [Request interrupted by user]"}]}}),
        json!({"message":{"content":[{"type":"thinking","text":"[Request interrupted by user]"}]}}),
        json!({"message":{"content":[]}}),
        json!({"message":null}),
    ] {
        let mut turn = Turn::new("claude", &hook());
        turn.record(&json!({"type":"user","promptId":"prompt","uuid":"tool-result"}))
            .unwrap();
        let mut record = interruption_record();
        record
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert!(!turn.record(&record).unwrap(), "{change}");
    }
    // An ordinary initial prompt quoting the exact marker cannot end itself.
    let mut turn = Turn::new("claude", &hook());
    assert!(!turn
        .record(&json!({
            "type":"user", "promptId":"prompt", "uuid":"user",
            "message":{"content":"[Request interrupted by user]"}
        }))
        .unwrap());
    let mut quoted = interruption_record();
    quoted
        .as_object_mut()
        .unwrap()
        .remove("interruptedMessageId");
    quoted["parentUuid"] = json!("user");
    assert!(!turn.record(&quoted).unwrap());
}

#[test]
fn interruption_cannot_confirm_a_different_conversation_or_prompt() {
    for change in [json!({"sessionId":"other"}), json!({"promptId":"other"})] {
        let mut turn = Turn::new("claude", &hook());
        turn.record(&json!({"type":"user","promptId":"prompt","uuid":"tool-result"}))
            .unwrap();
        let mut record = interruption_record();
        record
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        if change.get("sessionId").is_some() {
            assert!(turn.record(&record).is_err(), "{change}");
        } else {
            assert!(!turn.record(&record).unwrap(), "{change}");
        }
    }
    let mut unseen = Turn::new("claude", &hook());
    assert!(!unseen.record(&interruption_record()).unwrap());
}

#[test]
fn a_task_notification_turn_is_followed_by_its_own_prompt_id() {
    let mut turn = Turn::new(
        "claude",
        &HookEvent {
            prompt_id: Some("notification".into()),
            ..hook()
        },
    );
    for record in [
        json!({"type":"user","promptId":"prompt","uuid":"earlier"}),
        json!({"type":"user","promptId":"notification","uuid":"user","origin":{"kind":"task-notification"},"message":{"content":"<task-notification>\n<task-id>bj30txqod</task-id>\n</task-notification>"}}),
        json!({"type":"assistant","parentUuid":"user","uuid":"last","message":{"content":[{"type":"text","text":"BGDONE"}]}}),
    ] {
        assert!(!turn.record(&record).unwrap());
    }
    assert!(turn
        .record(&json!({"type":"system","subtype":"turn_duration","parentUuid":"last"}))
        .unwrap());
    assert_eq!(turn.last_text.as_deref(), Some("BGDONE"));
}

#[test]
fn replacement_conversations_never_confirm_the_previous_turn() {
    let mut turn = Turn::new("claude", &hook());
    turn.record(&json!({"type":"user","promptId":"prompt","uuid":"user"}))
        .unwrap();
    assert!(turn
        .record(&json!({"sessionId":"other","type":"system","subtype":"turn_duration"}))
        .is_err());
}

#[test]
fn missing_transcripts_and_exited_processes_are_unavailable() {
    let process = sysinfo::Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[process]), true);
    let started = system.process(process).unwrap().start_time();
    let observation = Observation {
        kind: "claude".into(),
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
        claude_status: None,
        recovery: false,
        permissions: false,
    };
    assert!(follow_with_limits(
        &observation,
        None,
        |_| Ok(()),
        Duration::ZERO,
        Duration::from_millis(10)
    )
    .is_err());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("turn.jsonl");
    std::fs::write(&path, "").unwrap();
    let observation = Observation {
        kind: "claude".into(),
        hook: HookEvent {
            transcript_path: Some(path.to_string_lossy().into_owned()),
            ..hook()
        },
        process: Some((std::process::id(), started + 1)),
        ..observation
    };
    assert!(follow_with_limits(
        &observation,
        None,
        |_| Ok(()),
        Duration::ZERO,
        Duration::from_millis(10)
    )
    .is_err());
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
    let mut turn = Turn::new("claude", &hook());
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
        kind: "claude".into(),
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
        claude_status: None,
        recovery: false,
        permissions: false,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

fn registry(status: &str, updated: u64) -> String {
    json!({"pid":std::process::id(),"sessionId":"conversation","kind":"interactive",
        "status":status,"statusUpdatedAt":updated})
    .to_string()
}

#[test]
fn claude_registry_ends_a_turn_only_with_fresh_idle_or_shell_and_matching_identity() {
    let dir = tempfile::tempdir().unwrap();
    let status = ClaudeStatusObservation {
        path: dir.path().join("status.json"),
        after_ms: 100,
    };
    let idle = json!({"pid":123,"sessionId":"conversation","kind":"interactive",
        "status":"idle","statusUpdatedAt":101});
    assert!(!status.turn_over(123, "conversation"));
    for change in [
        json!({"pid":124}),
        json!({"sessionId":"other"}),
        json!({"kind":"background"}),
        json!({"status":"busy"}),
        json!({"status":"waiting"}),
        json!({"status":null}),
        json!({"statusUpdatedAt":99,"updatedAt":102}),
        json!({"statusUpdatedAt":100}),
        json!({"statusUpdatedAt":null}),
        json!({"statusUpdatedAt":"101"}),
    ] {
        let mut record = idle.clone();
        record
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        std::fs::write(&status.path, record.to_string()).unwrap();
        assert!(!status.turn_over(123, "conversation"), "{change}");
    }
    for malformed in ["{", &" ".repeat(64 * 1024 + 1)] {
        std::fs::write(&status.path, malformed).unwrap();
        assert!(!status.turn_over(123, "conversation"));
    }
    for over in ["idle", "shell"] {
        let mut record = idle.clone();
        record["status"] = json!(over);
        std::fs::write(&status.path, record.to_string()).unwrap();
        assert!(status.turn_over(123, "conversation"), "{over}");
    }
}

#[test]
fn claude_early_interruption_uses_the_registry_without_a_transcript_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("turn.jsonl");
    let mut observation = observation(&transcript, None);
    let status_path = dir.path().join("status.json");
    observation.claude_status = Some(ClaudeStatusObservation {
        path: status_path.clone(),
        after_ms: now_ms(),
    });
    std::fs::write(&status_path, registry("idle", now_ms() + 1)).unwrap();
    // Early Esc restores the draft without writing an interruption or duration record.
    let prompt = json!({"type":"user","sessionId":"conversation",
        "promptId":"prompt","uuid":"user"});
    std::fs::write(&transcript, format!("{prompt}\n")).unwrap();
    let mut reports = vec![];
    let turn = follow_with_api_reporting(&observation, None, |hook| {
        reports.push(hook.clone());
        Ok(())
    })
    .unwrap();
    assert!(turn.last_text.is_none());
    assert!(!turn.background);
    assert_eq!(reports[0].claude_background, Some(false));

    // A turn that leaves only background Bash running also ends, as background work.
    std::fs::write(&status_path, registry("shell", now_ms() + 1)).unwrap();
    let mut reports = vec![];
    let turn = follow_with_api_reporting(&observation, None, |hook| {
        reports.push(hook.clone());
        Ok(())
    })
    .unwrap();
    assert!(turn.background);
    assert_eq!(reports[0].claude_turn_complete, Some(true));
    assert_eq!(reports[0].claude_background, Some(true));

    let next = json!({"type":"user","sessionId":"conversation",
        "promptId":"next","uuid":"new-user"});
    std::fs::write(&transcript, format!("{prompt}\n{next}\n")).unwrap();
    // Conversation idle can also settle an earlier prompt with no separate boundary.
    let turn = follow(&observation).unwrap();
    assert!(turn.last_text.is_none());

    // A reused PID cannot be confirmed by a registry left by its previous process.
    std::fs::write(&transcript, format!("{prompt}\n")).unwrap();
    observation.process.as_mut().unwrap().1 += 1;
    assert_eq!(
        follow(&observation).err().unwrap().to_string(),
        "claude process exited"
    );
}

#[test]
fn a_transcript_ended_turn_reports_registry_background_work() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("turn.jsonl");
    let status_path = dir.path().join("status.json");
    let mut observation = observation(&transcript, None);
    let started = observation.process.unwrap().1 * 1000;
    observation.claude_status = Some(ClaudeStatusObservation {
        path: status_path.clone(),
        after_ms: now_ms(),
    });
    std::fs::write(
        &transcript,
        [
            json!({"type":"user","sessionId":"conversation","promptId":"prompt","uuid":"user"}),
            json!({"type":"system","sessionId":"conversation","subtype":"turn_duration","parentUuid":"user"}),
        ]
        .map(|record| record.to_string() + "\n")
        .concat(),
    )
    .unwrap();
    let background = |observation: &Observation| {
        let mut reports = vec![];
        let turn = follow_with_api_reporting(observation, None, |hook| {
            reports.push(hook.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(turn.background, reports[0].claude_background == Some(true));
        reports[0].claude_background
    };
    // Background agents keep the busy record written before this prompt; it outlasts the
    // settle period. Records from before this process started, or of other kinds, are unknown.
    std::fs::write(&status_path, registry("busy", started + 1)).unwrap();
    let settling = Instant::now();
    assert_eq!(background(&observation), Some(true));
    assert!(settling.elapsed() >= BACKGROUND_SETTLE);
    for (status, expected) in [
        ("shell", Some(true)),
        ("waiting", Some(true)),
        ("idle", Some(false)),
        ("unknown", None),
    ] {
        std::fs::write(&status_path, registry(status, started + 1)).unwrap();
        assert_eq!(background(&observation), expected, "{status}");
    }
    std::fs::write(&status_path, registry("busy", started.saturating_sub(1001))).unwrap();
    assert_eq!(background(&observation), None);
    std::fs::remove_file(&status_path).unwrap();
    assert_eq!(background(&observation), None);
    observation.claude_status = None;
    assert_eq!(background(&observation), None);

    // A foreground turn can still read busy briefly after its transcript ends.
    observation.claude_status = Some(ClaudeStatusObservation {
        path: status_path.clone(),
        after_ms: now_ms(),
    });
    std::fs::write(&status_path, registry("busy", now_ms())).unwrap();
    let writer = {
        let status_path = status_path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            std::fs::write(&status_path, registry("idle", now_ms())).unwrap();
        })
    };
    assert_eq!(background(&observation), Some(false));
    writer.join().unwrap();
}

#[test]
fn background_watch_reports_idle_for_the_turn_once_the_registry_is_idle() {
    let dir = tempfile::tempdir().unwrap();
    let status_path = dir.path().join("status.json");
    let marker = dir.path().join("conversation");
    std::fs::write(&marker, "conversation").unwrap();
    let mut observation = observation(&dir.path().join("turn.jsonl"), Some(marker.clone()));
    observation.claude_status = Some(ClaudeStatusObservation {
        path: status_path.clone(),
        after_ms: now_ms(),
    });
    observation.hook.hook_event_name = "UserPromptSubmit".into();
    observation.hook.prompt = Some("review".into());
    std::fs::write(&status_path, registry("shell", now_ms())).unwrap();
    let writer = {
        let status_path = status_path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::fs::write(&status_path, registry("busy", now_ms())).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            std::fs::write(&status_path, registry("idle", now_ms())).unwrap();
        })
    };
    let watching = Instant::now();
    let mut reports = vec![];
    watch_background(&observation, |hook| {
        reports.push(hook.clone());
        Ok(())
    })
    .unwrap();
    writer.join().unwrap();
    assert!(watching.elapsed() >= Duration::from_millis(600));
    assert_eq!(reports.len(), 1);
    assert_eq!(
        reports[0].hook_event_name,
        yinz_types::agent::CLAUDE_BACKGROUND_IDLE
    );
    assert_eq!(reports[0].prompt_id.as_deref(), Some("prompt"));
    assert_eq!(reports[0].claude_background, Some(false));
    assert!(reports[0].prompt.is_none());

    // Lost identity or signal must never clear background work.
    std::fs::write(&status_path, registry("busy", now_ms())).unwrap();
    std::fs::write(&marker, "replacement").unwrap();
    let mut reports = vec![];
    watch_background(&observation, |hook| {
        reports.push(hook.clone());
        Ok(())
    })
    .unwrap_err();
    assert_eq!(reports.len(), 0);
    std::fs::write(&marker, "conversation").unwrap();
    std::fs::remove_file(&status_path).unwrap();
    watch_background(&observation, |hook| {
        reports.push(hook.clone());
        Ok(())
    })
    .unwrap_err();
    assert_eq!(reports.len(), 0);
}

#[test]
fn conversation_markers_reject_symlink_directories_before_writing_or_removing() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("marker"), "conversation").unwrap();
    let link = dir.path().join("observations");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    for event in ["SessionStart", "SessionEnd"] {
        let hook = HookEvent {
            hook_event_name: event.into(),
            session_id: "conversation".into(),
            ..Default::default()
        };
        assert!(update_conversation_marker(&link.join("marker"), "claude", &hook).is_err());
        assert_eq!(
            std::fs::read_to_string(outside.join("marker")).unwrap(),
            "conversation"
        );
    }
}

#[test]
fn conversation_markers_are_removed_on_session_end_without_a_prompt_id() {
    for kind in ["agy", "claude"] {
        for event in ["UserPromptSubmit", "SessionStart"] {
            let dir = tempfile::tempdir().unwrap();
            let marker = dir.path().join("observations/marker");
            let session = "conversation:with\"quotes\nand:colons";
            let hook = HookEvent {
                hook_event_name: event.into(),
                session_id: session.into(),
                prompt_id: (event == "UserPromptSubmit").then(|| format!("{session}:1")),
                ..Default::default()
            };
            update_conversation_marker(&marker, kind, &hook).unwrap();
            let contents = std::fs::read_to_string(&marker).unwrap();
            if kind == "agy" {
                assert_eq!(
                    serde_json::from_str::<(String, Option<String>)>(&contents).unwrap(),
                    (hook.session_id.clone(), hook.prompt_id.clone())
                );
            } else {
                assert_eq!(contents, session);
            }
            let end = HookEvent {
                hook_event_name: "SessionEnd".into(),
                prompt_id: None,
                ..hook
            };
            update_conversation_marker(&marker, kind, &end).unwrap();
            assert!(!marker.exists());
            update_conversation_marker(&marker, kind, &end).unwrap();
            assert!(!marker.exists());
        }
    }
}

#[test]
fn session_end_preserves_other_sessions_including_shared_prefixes() {
    for kind in ["agy", "claude"] {
        for (current, ended) in [
            ("other", "conversation"),
            ("conversation:branch", "conversation"),
            ("conversation", "conversation:branch"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let marker = dir.path().join("marker");
            let hook = HookEvent {
                hook_event_name: "UserPromptSubmit".into(),
                session_id: current.into(),
                prompt_id: Some(format!("{current}:1")),
                ..Default::default()
            };
            update_conversation_marker(&marker, kind, &hook).unwrap();
            let contents = std::fs::read_to_string(&marker).unwrap();
            update_conversation_marker(
                &marker,
                kind,
                &HookEvent {
                    hook_event_name: "SessionEnd".into(),
                    session_id: ended.into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(std::fs::read_to_string(&marker).unwrap(), contents);
        }
    }
}

#[test]
fn agy_session_end_only_removes_exact_legacy_session_markers() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let end = HookEvent {
        hook_event_name: "SessionEnd".into(),
        session_id: "conversation".into(),
        ..Default::default()
    };
    for (contents, removed) in [
        ("conversation:", true),
        ("conversation:conversation:1", true),
        ("conversation:conversation:123", true),
        ("other:", false),
        ("other:other:1", false),
        ("conversation:branch:", false),
        ("conversation:branch:conversation:branch:1", false),
        ("conversation:conversation:", false),
        ("conversation:conversation:step", false),
        ("conversation:conversation:1:extra", false),
    ] {
        std::fs::write(&marker, contents).unwrap();
        update_conversation_marker(&marker, "agy", &end).unwrap();
        assert_eq!(!marker.exists(), removed, "{contents}");
        update_conversation_marker(&marker, "agy", &end).unwrap();
        if !removed {
            assert_eq!(std::fs::read_to_string(&marker).unwrap(), contents);
        }
    }
    for event in ["SessionStart", "UserPromptSubmit"] {
        std::fs::write(&marker, "conversation:conversation:1").unwrap();
        let hook = HookEvent {
            hook_event_name: event.into(),
            prompt_id: (event == "UserPromptSubmit").then(|| "conversation:2".into()),
            ..end.clone()
        };
        update_conversation_marker(&marker, "agy", &hook).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&std::fs::read_to_string(&marker).unwrap()).unwrap(),
            json!([hook.session_id, hook.prompt_id])
        );
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
            .last_text
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

#[test]
fn an_agy_model_response_never_confirms_completion() {
    let hook = HookEvent {
        session_id: "agy-conv".into(),
        turn_id: Some("1".into()),
        prompt: Some("fix auth".into()),
        ..Default::default()
    };
    let mut turn = Turn::new("agy", &hook);

    // 1. Initial user request
    let user_step = json!({
        "step_index": 0,
        "type": "USER_INPUT",
        "status": "DONE",
        "content": "<USER_REQUEST>\nfix auth\n</USER_REQUEST>"
    });
    assert!(!turn.record(&user_step).unwrap());

    // 2. Intermediate model response with tool calls is not completion
    let tool_step = json!({
        "step_index": 1,
        "type": "PLANNER_RESPONSE",
        "status": "DONE",
        "tool_calls": [{"name": "run_command", "args": {"CommandLine": "cargo check"}}]
    });
    assert!(!turn.record(&tool_step).unwrap());

    // 3. Tool execution output is not completion
    let generic_step = json!({
        "step_index": 2,
        "type": "GENERIC",
        "status": "DONE",
        "content": "Finished dev target"
    });
    assert!(!turn.record(&generic_step).unwrap());

    // A finished model response is text evidence, never turn completion.
    let final_step = json!({
        "step_index": 3,
        "type": "PLANNER_RESPONSE",
        "status": "DONE",
        "content": "I fixed the authentication bug."
    });
    assert!(!turn.record(&final_step).unwrap());
    assert_eq!(
        turn.last_text.as_deref(),
        Some("I fixed the authentication bug.")
    );
}

#[test]
fn an_agy_turn_observer_detects_prompt_replacement() {
    let hook = HookEvent {
        session_id: "agy-conv".into(),
        turn_id: Some("1".into()),
        prompt: Some("initial prompt".into()),
        ..Default::default()
    };
    let mut turn = Turn::new("agy", &hook);

    let prompt1 = json!({
        "step_index": 0,
        "type": "USER_INPUT",
        "status": "DONE",
        "content": "<USER_REQUEST>\ninitial prompt\n</USER_REQUEST>"
    });
    assert!(!turn.record(&prompt1).unwrap());

    // A competing prompt in the same conversation fails the previous turn observation
    let prompt2 = json!({
        "step_index": 1,
        "type": "USER_INPUT",
        "status": "DONE",
        "content": "<USER_REQUEST>\nreplacement prompt\n</USER_REQUEST>"
    });
    assert!(turn.record(&prompt2).is_err());
}

#[test]
fn an_agy_turn_observer_detects_error_status() {
    let hook = HookEvent {
        session_id: "agy-conv".into(),
        turn_id: Some("1".into()),
        prompt: Some("do task".into()),
        ..Default::default()
    };
    let mut turn = Turn::new("agy", &hook);

    let prompt = json!({
        "step_index": 0,
        "type": "USER_INPUT",
        "status": "DONE",
        "content": "<USER_REQUEST>\ndo task\n</USER_REQUEST>"
    });
    assert!(!turn.record(&prompt).unwrap());

    let error_step = json!({
        "step_index": 1,
        "type": "PLANNER_RESPONSE",
        "status": "ERROR",
        "content": "API error"
    });
    assert!(turn.record(&error_step).is_err());
}

fn agy_response(status: &str) -> Value {
    json!({"status":status,"numTotalSteps":2,"trajectory":{"cascadeId":"conversation","steps":[
        {"type":"CORTEX_STEP_TYPE_USER_INPUT","status":"CORTEX_STEP_STATUS_DONE"},
        {"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","status":"CORTEX_STEP_STATUS_DONE"}
    ]}})
}

#[test]
fn agy_api_requires_matching_identity_and_execution_evidence() {
    let hook = HookEvent {
        turn_id: Some("1".into()),
        ..hook()
    };
    let mut turn = Turn::new("agy", &hook);
    let idle = agy_response("CASCADE_RUN_STATUS_IDLE");
    assert!(!AgyApi::trajectory_idle(&turn, &idle).unwrap());
    turn.record(&json!({"step_index":0,"type":"USER_INPUT","content":"same text"}))
        .unwrap();
    assert!(AgyApi::trajectory_idle(&turn, &idle).unwrap());
    assert!(!AgyApi::trajectory_idle(&turn, &agy_response("CASCADE_RUN_STATUS_RUNNING")).unwrap());
    for bad in [
        json!({}),
        {
            let mut v = idle.clone();
            v["status"] = "unknown".into();
            v
        },
        {
            let mut v = idle.clone();
            v["trajectory"]["cascadeId"] = "replacement".into();
            v
        },
        {
            let mut v = idle.clone();
            v["numTotalSteps"] = 3.into();
            v
        },
        {
            let mut v = idle.clone();
            v["trajectory"]["steps"][1]["type"] = "CORTEX_STEP_TYPE_USER_INPUT".into();
            v
        },
    ] {
        assert!(AgyApi::trajectory_idle(&turn, &bad).is_err(), "{bad}");
    }
    let unstarted = json!({"status":"CASCADE_RUN_STATUS_IDLE","numTotalSteps":1,
        "trajectory":{"cascadeId":"conversation","steps":[{"type":"CORTEX_STEP_TYPE_USER_INPUT"}]}});
    assert!(!AgyApi::trajectory_idle(&turn, &unstarted).unwrap());
    assert!(turn
        .record(&json!({"step_index":2,"type":"USER_INPUT","content":"same text"}))
        .is_err());
    let mut next = Turn::new(
        "agy",
        &HookEvent {
            turn_id: Some("3".into()),
            ..hook
        },
    );
    next.record(&json!({"step_index":0,"type":"USER_INPUT","content":"same text"}))
        .unwrap();
    assert!(!next.found);
    next.record(&json!({"step_index":2,"type":"USER_INPUT","content":"same text"}))
        .unwrap();
    assert!(next.found);
}

#[test]
fn agy_observer_waits_for_api_idle_after_model_completion() {
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = AgyApi::new(
        &listener.local_addr().unwrap().to_string(),
        "fixture-token".into(),
    )
    .unwrap();
    let idle = Arc::new(AtomicBool::new(false));
    let server_idle = idle.clone();
    let (queried_tx, queried_rx) = std::sync::mpsc::channel();
    let (idle_query_tx, idle_query_rx) = std::sync::mpsc::channel();
    let (idle_reply_tx, idle_reply_rx) = std::sync::mpsc::channel();
    let mut idle_responses = 0;
    let server = std::thread::spawn(move || loop {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            headers.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        assert!(headers.starts_with(
            "POST /exa.language_server_pb.LanguageServerService/GetCascadeTrajectory "
        ));
        assert!(headers
            .to_lowercase()
            .contains("x-codeium-csrf-token: fixture-token"));
        let length: usize = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .unwrap()
            .1
            .trim()
            .parse()
            .unwrap();
        let mut request = vec![0; length];
        reader.read_exact(&mut request).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request).unwrap(),
            json!({"cascadeId": "conversation"})
        );
        let done = server_idle.load(Ordering::SeqCst);
        if done {
            idle_responses += 1;
            if idle_responses == 1 {
                idle_query_tx.send(()).unwrap();
                idle_reply_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        }
        let body = agy_response(if done {
            "CASCADE_RUN_STATUS_IDLE"
        } else {
            "CASCADE_RUN_STATUS_RUNNING"
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        queried_tx.send(()).unwrap();
        if idle_responses == 2 {
            break;
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript.jsonl");
    std::fs::write(&path, concat!(
        "{\"step_index\":0,\"type\":\"USER_INPUT\",\"content\":\"same text\"}\n",
        "{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"content\":\"FIRST\"}\n"
    )).unwrap();
    let mut observation = observation(&path, None);
    observation.kind = "agy".into();
    observation.hook.turn_id = Some("1".into());
    observation.hook.prompt_id = Some("conversation:1".into());
    let (tx, rx) = std::sync::mpsc::channel();
    let observer =
        std::thread::spawn(move || tx.send(follow_with_api(&observation, Some(api))).unwrap());
    queried_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // The timed absence of completion is the behavior under test, even with model DONE.
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    idle.store(true, Ordering::SeqCst);
    idle_query_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // A transcript change during Idle requires another query after the observer reads it.
    let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        writer,
        "{}",
        json!({"step_index":2,"type":"PLANNER_RESPONSE","status":"DONE","content":"SECOND"})
    )
    .unwrap();
    idle_reply_tx.send(()).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .last_text
            .as_deref(),
        Some("SECOND")
    );
    observer.join().unwrap();
    server.join().unwrap();
}

#[test]
fn agy_unavailable_api_and_invalid_responses_fail_closed() {
    use std::net::TcpListener;
    let turn = Turn::new(
        "agy",
        &HookEvent {
            turn_id: Some("1".into()),
            ..hook()
        },
    );
    assert!(AgyApi::new("192.0.2.1:1234", "secret".into()).is_err());
    assert!(AgyApi::new("127.0.0.1:1234", String::new()).is_err());
    for (status, body) in [
        ("200 OK", "not JSON"),
        ("200 OK", "{}"),
        ("302 Found", ""),
        ("503 Unavailable", ""),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api =
            AgyApi::new(&listener.local_addr().unwrap().to_string(), "secret".into()).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
            }
            write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        assert!(api.idle(&turn).is_err());
        server.join().unwrap();
        assert!(api.idle(&turn).is_err());
    }
}

#[test]
fn agy_missing_identity_process_exit_replacement_and_session_end_are_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript.jsonl");
    let marker = dir.path().join("marker");
    std::fs::write(&path, "").unwrap();
    let mut observation = observation(&path, Some(marker.clone()));
    observation.kind = "agy".into();
    observation.hook.hook_event_name = "UserPromptSubmit".into();
    observation.hook.prompt_id = Some("conversation:1".into());
    observation.hook.turn_id = Some("1".into());
    for (session, prompt) in [("other", "other:1"), ("conversation", "conversation:3")] {
        let replacement = HookEvent {
            session_id: session.into(),
            prompt_id: Some(prompt.into()),
            ..observation.hook.clone()
        };
        update_conversation_marker(&marker, "agy", &replacement).unwrap();
        assert_eq!(
            follow_with_api(&observation, None)
                .err()
                .unwrap()
                .to_string(),
            "agy conversation replaced"
        );
    }
    update_conversation_marker(&marker, "agy", &observation.hook).unwrap();
    update_conversation_marker(
        &marker,
        "agy",
        &HookEvent {
            hook_event_name: "SessionEnd".into(),
            prompt_id: None,
            ..observation.hook.clone()
        },
    )
    .unwrap();
    assert_eq!(
        follow_with_api(&observation, None)
            .err()
            .unwrap()
            .to_string(),
        "agy conversation ended"
    );
    let mut missing = observation.hook.clone();
    missing.prompt_id = None;
    let original = std::mem::replace(&mut observation.hook, missing);
    observation.conversation_file = None;
    assert!(follow_with_api(&observation, None).is_err());
    observation.hook = original;
    observation.process.as_mut().unwrap().1 += 1;
    assert!(follow_with_api(&observation, None).is_err());
}

#[test]
fn overlapping_claude_prompts_keep_their_own_results_and_boundaries() {
    let mut first = Turn::new("claude", &hook());
    let mut second = Turn::new(
        "claude",
        &HookEvent {
            prompt_id: Some("next".into()),
            ..hook()
        },
    );
    for record in [
        json!({"type":"user","promptId":"prompt","uuid":"one"}),
        json!({"type":"assistant","parentUuid":"one","uuid":"answer-one","message":{"content":[{"type":"text","text":"FIRST"}]}}),
        // The newer prompt shares ancestry with the first, but has its own ownership.
        json!({"type":"user","promptId":"next","parentUuid":"answer-one","uuid":"two"}),
        json!({"type":"assistant","parentUuid":"two","uuid":"answer-two","message":{"content":[{"type":"text","text":"SECOND"}]}}),
        json!({"type":"user","parentUuid":"answer-two","uuid":"task-two","toolUseResult":{}}),
        json!({"type":"user","promptId":"prompt","parentUuid":"answer-two","uuid":"task-one","toolUseResult":{}}),
    ] {
        assert!(!first.record(&record).unwrap());
        assert!(!second.record(&record).unwrap());
    }
    let first_end = json!({"type":"system","subtype":"turn_duration","parentUuid":"task-one"});
    assert!(first.record(&first_end).unwrap());
    assert!(!second.record(&first_end).unwrap());
    let second_end = json!({"type":"system","subtype":"turn_duration","parentUuid":"task-two"});
    assert!(!first.record(&second_end).unwrap());
    assert!(second.record(&second_end).unwrap());
    assert_eq!(first.last_text.as_deref(), Some("FIRST"));
    assert_eq!(second.last_text.as_deref(), Some("SECOND"));
}

fn queued_fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/claude-missed-queued-prompt.json")).unwrap()
}

#[test]
fn queued_prompt_recovery_replays_once_and_confirms_the_correct_response() {
    let fixture = queued_fixture();
    let records = fixture["transcript"].as_array().unwrap();
    let mut prompts = prompts::Prompts::new(&hook());
    let recovered: Vec<_> = records
        .iter()
        .filter_map(|r| prompts.record(r).unwrap())
        .collect();
    assert_eq!(recovered.len(), 1);
    let recovered = &recovered[0];
    assert_eq!(recovered.prompt_id.as_deref(), Some("queued"));
    assert_eq!(recovered.claude_recovered_after.as_deref(), Some("prompt"));
    assert!(recovered.prompt.is_none());
    assert!(recovered.prompt_sha256.is_none());
    assert!(recovered.turn_id.is_none());
    for record in records {
        assert!(prompts.record(record).unwrap().is_none());
    }
    let mut turn = Turn::new("claude", recovered);
    let completed: Vec<_> = records.iter().map(|r| turn.record(r).unwrap()).collect();
    assert_eq!(completed.iter().filter(|done| **done).count(), 1);
    assert!(completed.last().unwrap());
    assert_eq!(turn.last_text.as_deref(), Some("Second task finished."));
}

#[test]
fn queued_prompt_recovery_requires_anchor_identity_and_explicit_human_metadata() {
    let fixture = queued_fixture();
    let anchor = &fixture["transcript"][0];
    let queued = &fixture["transcript"][3];
    for patch in [
        json!({"sessionId":"other"}),
        json!({"sessionId":null}),
        json!({"isSidechain":true}),
        json!({"isSidechain":null}),
        json!({"type":"attachment"}),
        json!({"isMeta":true}),
        json!({"origin":{"kind":"task-notification"}}),
        json!({"origin":null}),
        json!({"turnOrigin":"task-notification"}),
        json!({"promptSource":"direct"}),
        json!({"promptId":""}),
        json!({"promptId":"  "}),
        json!({"promptId":42}),
        json!({"promptId":"bad\nidentity"}),
        json!({"promptId":"x".repeat(1025)}),
        json!({"toolUseResult":{}}),
        json!({"message":{"content":[{"type":"tool_result","tool_use_id":"tool"}]}}),
    ] {
        let mut prompts = prompts::Prompts::new(&hook());
        assert!(prompts.record(anchor).unwrap().is_none());
        let mut record = queued.clone();
        record
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(prompts.record(&record).unwrap().is_none(), "{patch}");
    }
    let mut prompts = prompts::Prompts::new(&hook());
    assert!(prompts.record(queued).unwrap().is_none());
    assert!(prompts.record(anchor).unwrap().is_none());
    assert!(
        prompts.record(queued).unwrap().is_none(),
        "records before anchor cannot be replayed into acceptance"
    );
    let mut later = queued.clone();
    later["promptId"] = json!("later");
    assert_eq!(
        prompts
            .record(&later)
            .unwrap()
            .unwrap()
            .claude_recovered_after
            .as_deref(),
        Some("prompt")
    );
}

#[test]
fn queued_prompt_recovery_tracks_intervening_native_prompts() {
    let fixture = queued_fixture();
    let mut prompts = prompts::Prompts::new(&hook());
    prompts.record(&fixture["transcript"][0]).unwrap();
    let mut native = fixture["transcript"][0].clone();
    native["promptId"] = json!("native");
    assert!(prompts.record(&native).unwrap().is_none());
    let recovered = prompts.record(&fixture["transcript"][3]).unwrap().unwrap();
    assert_eq!(recovered.claude_recovered_after.as_deref(), Some("native"));
}

#[test]
fn queued_prompt_observer_inherits_process_route_and_marker_after_acceptance() {
    let fixture = queued_fixture();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("root.jsonl");
    let marker = dir.path().join("conversation");
    std::fs::write(&marker, "conversation").unwrap();
    std::fs::write(
        &path,
        fixture["transcript"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.to_string() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    let original = observation(&path, Some(marker.clone()));
    let reports = std::cell::RefCell::new(Vec::new());
    recovery::follow_reporting(
        &original,
        |hook| {
            reports.borrow_mut().push(hook.clone());
            Ok(())
        },
        |recovered| {
            assert_eq!(
                reports.borrow().len(),
                1,
                "acceptance precedes starting completion observation"
            );
            assert_eq!(recovered.process, original.process);
            assert_eq!(recovered.conversation_file, original.conversation_file);
            assert_eq!(
                serde_json::to_value(&recovered.route).unwrap(),
                serde_json::to_value(&original.route).unwrap()
            );
            assert!(!recovered.recovery);
            follow_with_api_reporting(recovered, None, |hook| {
                reports.borrow_mut().push(hook.clone());
                Ok(())
            })
            .unwrap();
            std::fs::write(&marker, "replacement").unwrap();
            Ok(Command::new("/usr/bin/true").spawn().unwrap())
        },
    )
    .unwrap();
    let reports = reports.into_inner();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[1].claude_turn_complete, Some(true));
    assert_eq!(
        reports[1].last_assistant_message.as_deref(),
        Some("Second task finished.")
    );
    // A reporter child must not initialize a marker or spawn more observers.
    start("claude", &reports[0], original.route).unwrap();
}

#[test]
fn queued_prompt_idle_freshness_uses_transcript_time_before_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("turn.jsonl");
    let mut original = observation(&path, None);
    original.claude_status = Some(ClaudeStatusObservation {
        path: dir.path().join("status.json"),
        after_ms: 300, // Recovery starts after the registry already published Idle.
    });
    let prompt = json!({"type":"user", "sessionId":"conversation", "promptId":"queued",
        "uuid":"queued-user", "timestamp":"1970-01-01T00:00:00.100Z"});
    let recovered_hook = HookEvent {
        prompt_id: Some("queued".into()),
        ..original.hook.clone()
    };
    let recovered = recovery::recovered_observation(&original, recovered_hook.clone(), &prompt);
    let status = recovered.claude_status.as_ref().unwrap();
    assert_eq!(status.after_ms, 100);
    let idle = json!({"pid":std::process::id(),"sessionId":"conversation","kind":"interactive",
        "status":"idle", "statusUpdatedAt":200});
    std::fs::write(&status.path, idle.to_string()).unwrap();
    std::fs::write(&path, format!("{prompt}\n")).unwrap();
    assert!(!original
        .claude_status
        .as_ref()
        .unwrap()
        .turn_over(std::process::id(), "conversation"));
    follow(&recovered).unwrap();
    for change in [
        json!({"statusUpdatedAt":99}),
        json!({"statusUpdatedAt":100}),
        json!({"pid":0}),
        json!({"sessionId":"other"}),
    ] {
        let mut stale = idle.clone();
        stale
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        std::fs::write(&status.path, stale.to_string()).unwrap();
        assert!(
            !status.turn_over(std::process::id(), "conversation"),
            "{change}"
        );
    }
    std::fs::write(&status.path, idle.to_string()).unwrap();
    for timestamp in [
        Value::Null,
        json!(100),
        json!("invalid"),
        json!("1969-12-31T23:59:59Z"),
    ] {
        let mut invalid = prompt.clone();
        invalid["timestamp"] = timestamp;
        let recovered =
            recovery::recovered_observation(&original, recovered_hook.clone(), &invalid);
        assert!(recovered.claude_status.is_none());
        let boundary =
            json!({"type":"system","subtype":"turn_duration","parentUuid":"queued-user"});
        std::fs::write(&path, format!("{invalid}\n{boundary}\n")).unwrap();
        follow(&recovered).unwrap(); // Transcript completion remains available.
    }
}

// Observe termination without consuming the exit status: the task observer must reap it.
fn exited_observer_child(child: &Child) {
    let mut status = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
    // SAFETY: status points to writable siginfo storage and the PID belongs to our child.
    assert_eq!(
        unsafe {
            libc::waitid(
                libc::P_PID,
                child.id(),
                status.as_mut_ptr(),
                libc::WEXITED | libc::WNOWAIT,
            )
        },
        0
    );
}

fn assert_observer_child_reaped(pid: u32) {
    // SAFETY: no status output is requested; this only checks our child's waitability.
    assert_eq!(
        unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
fn recovery_observer_reaps_multiple_completion_children_while_still_running() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("turn.jsonl");
    let marker = dir.path().join("conversation");
    std::fs::write(&marker, "conversation").unwrap();
    let fixture = queued_fixture();
    let mut records = vec![fixture["transcript"][0].clone()];
    let queued = fixture["transcript"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["promptSource"] == "queued")
        .unwrap();
    for id in ["one", "two"] {
        let mut prompt = queued.clone();
        prompt["promptId"] = json!(id);
        records.push(prompt);
    }
    // Each sweep reads at most 1024 records. The third prompt therefore runs after
    // a reap sweep, regardless of scheduling; waitid below proves both children exited.
    records.resize(1024, json!({"type":"system"}));
    let mut third = queued.clone();
    third["promptId"] = json!("three");
    records.push(third);
    std::fs::write(
        &path,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let original = observation(&path, Some(marker.clone()));
    let mut pids = vec![];
    recovery::follow_reporting(
        &original,
        |_| Ok(()),
        |_| {
            if pids.len() == 2 {
                for pid in &pids {
                    assert_observer_child_reaped(*pid);
                }
                std::fs::write(&marker, "replacement").unwrap();
            }
            let child = Command::new("/usr/bin/true").spawn().unwrap();
            exited_observer_child(&child);
            pids.push(child.id());
            Ok(child)
        },
    )
    .unwrap();
    assert_eq!(pids.len(), 3);
    for pid in pids {
        assert_observer_child_reaped(pid);
    }
}

#[test]
fn recovery_observer_leaves_live_children_running_on_shutdown_and_failure() {
    for failure in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("turn.jsonl");
        let marker = dir.path().join("conversation");
        std::fs::write(&marker, "conversation").unwrap();
        let fixture = queued_fixture();
        let queued = fixture["transcript"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["promptSource"] == "queued")
            .unwrap();
        let mut second = queued.clone();
        second["promptId"] = json!("second");
        std::fs::write(
            &path,
            format!("{}\n{queued}\n{second}\n", fixture["transcript"][0]),
        )
        .unwrap();
        let original = observation(&path, Some(marker.clone()));
        let mut pids = vec![];
        let result = recovery::follow_reporting(
            &original,
            |_| Ok(()),
            |_| {
                if !pids.is_empty() && failure {
                    return Err(anyhow!("injected spawn failure"));
                }
                // Not stdin-driven: the observer drops its handle on exit, and a closed
                // pipe would end the child on its own before liveness is checked.
                let child = Command::new("sleep")
                    .arg("600")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .spawn()
                    .unwrap();
                pids.push(child.id());
                std::fs::write(&marker, "replacement").unwrap();
                Ok(child)
            },
        );
        assert_eq!(result.is_err(), failure);
        assert_eq!(pids.len(), if failure { 1 } else { 2 });
        // A completion observer may be watching background work for hours; the
        // recovery observer's exit must not cut off its final report.
        for pid in pids {
            let pid = pid as libc::pid_t;
            // SAFETY: pid is this test's unreaped child; no status output is requested.
            assert_eq!(
                unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
                0,
                "completion observer {pid} survives the recovery observer"
            );
            // SAFETY: pid is this test's live, unreaped child.
            assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
            // SAFETY: pid is this test's child; no status output is requested.
            assert_eq!(unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) }, pid);
            assert_observer_child_reaped(pid as u32);
        }
    }
}

#[test]
fn completion_observer_initialization_failure_kills_and_reaps_child() {
    let dir = tempfile::tempdir().unwrap();
    let original = observation(&dir.path().join("turn.jsonl"), None);
    for missing_input in [false, true] {
        let mut child = Command::new(if missing_input {
            "/usr/bin/cat"
        } else {
            "/usr/bin/true"
        })
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
        let pid = child.id();
        let _input = if missing_input {
            child.stdin.take()
        } else {
            exited_observer_child(&child);
            None
        };
        assert!(initialize_observer(child, &original).is_err());
        assert_observer_child_reaped(pid);
    }
}

#[test]
fn root_notification_mapping_requires_structural_origin_and_exact_turn() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/claude-notification-settlement.json")).unwrap();
    let row = fixture["accepted"].clone();
    assert_eq!(
        notifications::accepted(&row, "conversation", "prompt")
            .unwrap()
            .task,
        "task"
    );
    for patch in [
        json!({"origin":null}),
        json!({"isSidechain":true}),
        json!({"sessionId":"foreign"}),
        json!({"promptId":"other"}),
        json!({"message":{"content":"ordinary prose <task-notification><task-id>task</task-id></task-notification>"}}),
    ] {
        let mut invalid = row.clone();
        invalid
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(notifications::accepted(&invalid, "conversation", "prompt").is_none());
    }
}

#[test]
fn notification_follow_reports_acceptance_and_only_ancestral_answer_completion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("root.jsonl");
    let rows = [
        json!({"type":"user", "isSidechain":false,"sessionId":"conversation","promptId":"prompt","uuid":"accepted",
            "origin":{"kind":"task-notification","producer":"session-task","runId":"run"},
            "message":{"content":"<task-notification><task-id>task</task-id><tool-use-id>tool</tool-use-id></task-notification>"}}),
        json!({"type":"assistant","uuid":"answer","parentUuid":"accepted","message":{"content":[{"type":"text","text":"FINAL"}]}}),
        json!({"type":"system","subtype":"turn_duration","parentUuid":"answer"}),
    ];
    std::fs::write(
        &path,
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let observation = observation(&path, None);
    let mut reports = Vec::new();
    let turn = follow_with_api_reporting(&observation, None, |h| {
        reports.push(h.clone());
        Ok(())
    })
    .unwrap();
    assert!(turn.answer_boundary);
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].hook_event_name, "NotificationAccepted");
    assert_eq!(reports[1].claude_turn_complete, Some(true));
    assert_eq!(reports[1].last_assistant_message.as_deref(), Some("FINAL"));
    assert_eq!(
        reports[0].claude_notifications,
        reports[1].claude_notifications
    );
}

#[test]
fn a_registry_only_notice_end_is_unavailable_answer_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("root.jsonl");
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/claude-notification-settlement.json")).unwrap();
    std::fs::write(&path, format!("{}\n", fixture["accepted"])).unwrap();
    let mut observation = observation(&path, None);
    let status = dir.path().join("status.json");
    observation.claude_status = Some(ClaudeStatusObservation {
        path: status.clone(),
        after_ms: now_ms(),
    });
    std::fs::write(status, registry("idle", now_ms() + 1)).unwrap();
    let mut reports = Vec::new();
    follow_with_api_reporting(&observation, None, |h| {
        reports.push(h.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[1].claude_turn_complete, Some(false));
    assert!(
        reports[1]
            .claude_notifications
            .as_ref()
            .unwrap()
            .unavailable
    );
}

#[test]
fn claude_permission_registry_requires_positive_reason_and_guarded_identity() {
    use yinz_types::agent::claude_permissions::Status;
    let dir = tempfile::tempdir().unwrap();
    let reader = ClaudeStatusObservation {
        path: dir.path().join("record.json"),
        after_ms: 0,
    };
    let base = json!({"pid":123,"sessionId":"conversation","kind":"interactive","status":"waiting","waitingFor":"permission prompt","statusUpdatedAt":456_100});
    for (change, expected) in [
        (json!({}), Some(Status::Active)),
        (json!({"status":"busy"}), Some(Status::Inactive)),
        (json!({"status":"idle"}), Some(Status::Inactive)),
        (json!({"waitingFor":"input needed"}), Some(Status::Inactive)),
        (json!({"waitingFor":"dialog open"}), Some(Status::Inactive)),
        (
            json!({"waitingFor":"worker request"}),
            Some(Status::Inactive),
        ),
        (
            json!({"waitingFor":"sandbox request"}),
            Some(Status::Inactive),
        ),
        (json!({"waitingFor":"new reason"}), None),
        (json!({"waitingFor":null}), None),
        (json!({"status":"future"}), None),
        (json!({"pid":124}), None),
        (json!({"kind":"worker"}), None),
        (json!({"sessionId":"other"}), None),
        (json!({"statusUpdatedAt":454_000}), None),
    ] {
        let mut record = base.clone();
        record
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        std::fs::write(&reader.path, record.to_string()).unwrap();
        assert_eq!(
            reader
                .permission((123, 456), "conversation")
                .map(|s| s.status),
            expected,
            "{change}"
        );
    }
    for malformed in ["{".into(), " ".repeat(65_537)] {
        std::fs::write(&reader.path, malformed).unwrap();
        assert!(reader.permission((123, 456), "conversation").is_none());
    }
    std::fs::remove_file(&reader.path).unwrap();
    // Opening a FIFO must never block a synchronous hook.
    nix::unistd::mkfifo(&reader.path, nix::sys::stat::Mode::S_IRUSR).unwrap();
    assert!(reader.permission((123, 456), "conversation").is_none());
}

#[test]
fn claude_permission_watch_retries_release_without_new_trigger_and_deduplicates() {
    use yinz_types::agent::{
        claude_notifications::Source,
        claude_permissions::{Sample, Status},
    };
    let mut watch = permissions::Watch::default();
    let mut sample = Sample {
        source: Source { pid: 1, started: 2 },
        updated_ms: 3_000,
        observed_ms: 500_000,
        status: Status::Inactive,
    };
    assert_eq!(
        watch.next(Some(sample.clone()), Duration::ZERO),
        Some(sample.clone())
    );
    watch.accepted(sample.clone(), Duration::ZERO);
    // Slow automatic allow/deny remains inactive even beyond a candidate grace period.
    for ms in [100, 500, 1_000, 3_000] {
        assert!(watch
            .next(Some(sample.clone()), Duration::from_millis(ms))
            .is_none());
    }
    sample.status = Status::Active;
    sample.updated_ms += 1;
    let active = watch
        .next(Some(sample.clone()), Duration::from_secs(4))
        .unwrap();
    watch.accepted(active.clone(), Duration::from_secs(4));
    assert!(watch
        .next(Some(active.clone()), Duration::from_millis(4_100))
        .is_none());
    assert_eq!(
        watch.next(Some(active.clone()), Duration::from_millis(4_500)),
        Some(active.clone())
    );
    sample.status = Status::Inactive;
    sample.updated_ms += 1;
    // First delivery fails: do not acknowledge it into the watch. It must retry itself.
    assert_eq!(
        watch.next(Some(sample.clone()), Duration::from_secs(5)),
        Some(sample.clone())
    );
    assert_eq!(
        watch.next(Some(sample.clone()), Duration::from_millis(5_100)),
        Some(sample.clone())
    );
    watch.accepted(sample.clone(), Duration::from_millis(5_100));
    assert!(watch.next(None, Duration::from_secs(6)).is_none());
    assert!(watch.next(None, Duration::from_millis(7_999)).is_none());
    let lost = watch.next(None, Duration::from_secs(8)).unwrap();
    assert_eq!(lost.status, Status::Unavailable);
    watch.accepted(lost, Duration::from_secs(8));
    assert_eq!(
        watch.next(Some(sample.clone()), Duration::from_secs(9)),
        Some(sample)
    );
}

#[test]
fn claude_permission_observer_lock_has_independent_namespace_and_releases() {
    let dir = tempfile::tempdir().unwrap();
    let mut observation = observation(&dir.path().join("missing-transcript"), None);
    observation.process = Some((1, 2));
    let lock = permissions::lock(&observation, dir.path())
        .unwrap()
        .unwrap();
    assert!(permissions::lock(&observation, dir.path())
        .unwrap()
        .is_none());
    assert!(recovery::lock(&observation, dir.path()).unwrap().is_some());
    drop(lock);
    assert!(permissions::lock(&observation, dir.path())
        .unwrap()
        .is_some());
}

#[test]
fn issue85_missing_transcript_then_lost_confirmation_ack_recovers_original_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delayed.jsonl");
    let observation = observation(&path, None);
    let mut unavailable = 0;
    let mut confirmations = Vec::new();
    let turn = follow_with_limits(
        &observation, None,
        |hook| {
            if hook.claude_turn_complete == Some(false) {
                unavailable += 1;
                std::fs::write(&path, [
                    json!({"type":"user","promptId":"prompt","uuid":"accepted"}),
                    json!({"type":"assistant","parentUuid":"accepted","uuid":"answer","message":{"content":[{"type":"text","text":"DONE"}]}}),
                    json!({"type":"system","subtype":"turn_duration","parentUuid":"answer"}),
                ].iter().map(|row| format!("{row}\n")).collect::<String>()).unwrap();
                return Ok(());
            }
            confirmations.push(hook.clone());
            if confirmations.len() == 1 {
                // Simulate delivery followed by a lost acknowledgement.
                return Err(anyhow!("injected acknowledgement loss"));
            }
            Ok(())
        },
        Duration::ZERO, Duration::from_secs(5),
    ).unwrap();
    assert_eq!(unavailable, 1);
    assert_eq!(confirmations.len(), 2);
    assert_eq!(confirmations[0], confirmations[1]);
    assert_eq!(confirmations[1].prompt_id.as_deref(), Some("prompt"));
    assert_eq!(confirmations[1].claude_answered, Some(true));
    assert_eq!(turn.last_text.as_deref(), Some("DONE"));
    assert!(turn.reported);
}

#[test]
fn issue85_completion_delivery_retry_has_an_absolute_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("turn.jsonl");
    std::fs::write(&path, [
        json!({"type":"user","promptId":"prompt","uuid":"accepted"}),
        json!({"type":"assistant","parentUuid":"accepted","uuid":"answer","message":{"content":[]}}),
        json!({"type":"system","subtype":"turn_duration","parentUuid":"answer"}),
    ].iter().map(|row| format!("{row}\n")).collect::<String>()).unwrap();
    let error = follow_with_limits(
        &observation(&path, None),
        None,
        |_| Err(anyhow!("injected delivery failure")),
        Duration::ZERO,
        Duration::from_millis(10),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("deadline"));
}

#[test]
fn issue85_linked_interruption_has_terminal_provenance_without_an_answer() {
    let mut turn = Turn::new("claude", &hook());
    turn.record(&json!({"type":"user","promptId":"prompt","uuid":"tool-result"}))
        .unwrap();
    assert!(turn.record(&interruption_record()).unwrap());
    assert!(turn.interrupted);
    assert!(!turn.answer_boundary);
}

#[test]
fn restored_claude_status_requires_matching_process_conversation_and_known_state() {
    use yinz_types::agent::AgentState;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.json");
    let reader = ClaudeStatusObservation {
        path: path.clone(),
        after_ms: 0,
    };
    assert_eq!(reader.current((7, 100), "conversation"), None);
    for (pid, session, timestamp, accepted) in [
        (7, "conversation", 101000, true),
        (8, "conversation", 101000, false),
        (7, "replacement", 101000, false),
        (7, "conversation", 98000, false),
    ] {
        std::fs::write(&path, json!({"pid":pid,"sessionId":session,"kind":"interactive","statusUpdatedAt":timestamp,"status":"idle"}).to_string()).unwrap();
        assert_eq!(reader.current((7, 100), "conversation").is_some(), accepted);
    }
    std::fs::write(&path, "partial rewrite").unwrap();
    assert_eq!(reader.current((7, 100), "conversation"), None);
    for (status, state) in [
        ("idle", Some(AgentState::Idle)),
        ("busy", Some(AgentState::Busy)),
        ("shell", Some(AgentState::Busy)),
        ("waiting", None),
        ("unknown", None),
    ] {
        assert_eq!(restored_state(Some(status)), state);
    }
    assert_eq!(restored_state(None), None);
}

#[test]
fn restored_claude_observer_discovers_only_the_adopted_process_tree() {
    // A renamed system utility gives the real process backend a Claude-shaped
    // process without invoking Claude or changing its configuration directory.
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("claude");
    std::fs::copy("/bin/sleep", &executable).unwrap();
    let child = RestoredClaudeChild(Command::new(executable).arg("60").spawn().unwrap());
    let called = std::cell::Cell::new(false);
    observe_restored(child.0.id(), "missing-conversation", |state| {
        assert_eq!(state, None);
        called.set(true);
        false
    });
    assert!(called.get());
    observe_restored(u32::MAX, "missing-conversation", |_| {
        panic!("an unrelated process must not be observed")
    });
}

#[test]
fn restored_claude_observer_recognizes_argv_zero() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("different-name");
    std::fs::copy("/bin/sleep", &executable).unwrap();
    let child = RestoredClaudeChild(
        Command::new(executable)
            .arg0("/opt/claude")
            .arg("60")
            .env("CLAUDE_CONFIG_DIR", directory.path())
            .spawn()
            .unwrap(),
    );
    let called = std::cell::Cell::new(false);
    observe_restored(child.0.id(), "missing-conversation", |state| {
        assert_eq!(state, None);
        called.set(true);
        false
    });
    assert!(called.get());
}

#[test]
fn restored_claude_observer_rejects_two_actual_processes() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("claude");
    std::fs::copy("/bin/sleep", &executable).unwrap();
    let _first = RestoredClaudeChild(Command::new(&executable).arg("60").spawn().unwrap());
    let _second = RestoredClaudeChild(Command::new(&executable).arg("60").spawn().unwrap());
    observe_restored(std::process::id(), "missing-conversation", |_| {
        panic!("two Claude processes in the same tree are ambiguous")
    });
}

struct RestoredClaudeChild(Child);

impl Drop for RestoredClaudeChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn restored_claude_thread_fixture() {
    if std::env::var_os("YINZ_TEST_RESTORED_CLAUDE_THREAD").is_none() {
        return;
    }
    // The harness names its test thread. Give it Claude's name before creating
    // an unnamed worker, which inherits that name just as Claude's workers do.
    nix::sys::prctl::set_name(c"claude").unwrap();
    std::thread::spawn(|| {
        let tid = nix::unistd::gettid();
        assert_eq!(
            std::fs::read_to_string(format!("/proc/self/task/{tid}/comm")).unwrap(),
            "claude\n"
        );
        println!("RESTORED_CLAUDE_THREAD_READY");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    })
    .join()
    .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn restored_claude_observer_ignores_inherited_thread_names() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("claude");
    std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &executable).unwrap();
    let mut child = RestoredClaudeChild(
        Command::new(executable)
            .args([
                "--exact",
                "claude_turn::tests::restored_claude_thread_fixture",
                "--nocapture",
            ])
            .env("YINZ_TEST_RESTORED_CLAUDE_THREAD", "1")
            .env("CLAUDE_CONFIG_DIR", directory.path())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (ready, received) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line.unwrap().contains("RESTORED_CLAUDE_THREAD_READY") {
                ready.send(()).unwrap();
                return;
            }
        }
    });
    received.recv_timeout(Duration::from_secs(10)).unwrap();
    reader.join().unwrap();
    assert_eq!(
        std::fs::read_to_string(format!("/proc/{}/comm", child.0.id())).unwrap(),
        "claude\n"
    );
    let called = std::cell::Cell::new(false);
    observe_restored(child.0.id(), "missing-conversation", |state| {
        assert_eq!(state, None);
        called.set(true);
        false
    });
    assert!(called.get());
}

#[test]
fn restored_claude_registry_uses_the_observed_process_environment() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("claude");
    std::fs::copy("/bin/sleep", &executable).unwrap();
    for source in ["config", "home", "empty-config", "missing"] {
        let home = directory.path().join(source);
        let config = home.join("custom-config");
        let mut command = Command::new(&executable);
        command.arg("60").env_clear();
        if source != "missing" {
            command.env("HOME", &home);
        }
        match source {
            "config" => {
                command.env("CLAUDE_CONFIG_DIR", &config);
            },
            "empty-config" => {
                command.env("CLAUDE_CONFIG_DIR", "");
            },
            _ => {},
        }
        let child = RestoredClaudeChild(command.spawn().unwrap());
        let root = if source == "config" {
            config
        } else {
            home.join(".claude")
        };
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        std::fs::write(
            root.join("sessions").join(format!("{}.json", child.0.id())),
            json!({"pid":child.0.id(),"sessionId":"conversation","kind":"interactive",
                "statusUpdatedAt":now,"status":"idle"})
            .to_string(),
        )
        .unwrap();
        if source == "config" {
            // A conflicting HOME registry must not override CLAUDE_CONFIG_DIR.
            let fallback = home.join(".claude/sessions");
            std::fs::create_dir_all(&fallback).unwrap();
            std::fs::write(fallback.join(format!("{}.json", child.0.id())), "invalid").unwrap();
        }
        let called = std::cell::Cell::new(false);
        let pid = child.0.id();
        let child = std::cell::RefCell::new(child);
        observe_restored(pid, "conversation", |state| {
            assert_eq!(state, Some(yinz_types::agent::AgentState::Idle), "{source}");
            called.set(true);
            // The next sample must stop on process death, despite the retained record.
            let mut child = child.borrow_mut();
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            true
        });
        assert_eq!(called.get(), source != "missing", "{source}");
    }
}
