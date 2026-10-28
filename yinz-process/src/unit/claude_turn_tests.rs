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
            json!({"type":"user","parentUuid":"assistant","uuid":"tool-result","toolUseResult":{"backgroundTaskId":"background-task"}}),
        ] {
            assert!(!turn.record(&record).unwrap());
        }
        let mut interruption = interruption_record();
        interruption["message"]["content"] = content;
        assert!(turn.record(&interruption).unwrap());
        assert_eq!(turn.last_text.as_deref(), Some("Work so far."));
        assert_eq!(turn.background_tasks, ["background-task"]);
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
        json!({"isSidechain":true}),
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
        assert!(turn.record(&record).is_err(), "{change}");
    }
    let mut unseen = Turn::new("claude", &hook());
    assert!(!unseen.record(&interruption_record()).unwrap());
}

#[test]
fn background_launches_in_the_turn_are_reported_once_in_launch_order() {
    let mut turn = Turn::new("claude", &hook());
    for record in [
        json!({"type":"user","promptId":"prompt","uuid":"user"}),
        json!({"type":"assistant","parentUuid":"user","uuid":"bash","message":{"content":[{"type":"tool_use"}]}}),
        json!({"type":"user","parentUuid":"bash","uuid":"bash-result","toolUseResult":{"stdout":"","backgroundTaskId":"bj30txqod"}}),
        json!({"type":"assistant","parentUuid":"bash-result","uuid":"agent","message":{"content":[{"type":"tool_use"}]}}),
        json!({"type":"user","parentUuid":"agent","uuid":"agent-result","toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"a91f"}}),
        // A worker's own background launch and an unlinked one belong to other turns.
        json!({"type":"user","parentUuid":"agent-result","uuid":"worker","isSidechain":true,"toolUseResult":{"backgroundTaskId":"sidechain"}}),
        json!({"type":"user","parentUuid":"elsewhere","uuid":"stray","toolUseResult":{"backgroundTaskId":"stray"}}),
        // A finished subagent and a repeated report are not new background work.
        json!({"type":"user","parentUuid":"agent-result","uuid":"sync","toolUseResult":{"status":"completed","agentId":"sync"}}),
        json!({"type":"user","parentUuid":"sync","uuid":"again","toolUseResult":{"backgroundTaskId":"bj30txqod"}}),
        json!({"type":"assistant","parentUuid":"again","uuid":"last","message":{"content":[{"type":"text","text":"Started."}]}}),
    ] {
        assert!(!turn.record(&record).unwrap());
    }
    assert!(turn
        .record(&json!({"type":"system","subtype":"turn_duration","parentUuid":"last"}))
        .unwrap());
    assert_eq!(turn.background_tasks, ["bj30txqod", "a91f"]);
    assert_eq!(turn.last_text.as_deref(), Some("Started."));
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
    assert!(turn.background_tasks.is_empty());
    assert_eq!(turn.last_text.as_deref(), Some("BGDONE"));
}

#[test]
fn replacement_prompts_and_conversations_never_confirm_the_previous_turn() {
    for replacement in [
        json!({"type":"user","promptId":"new"}),
        json!({"sessionId":"other","type":"system","subtype":"turn_duration"}),
    ] {
        let mut turn = Turn::new("claude", &hook());
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
    };
    assert!(follow(&observation).is_err());
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
    }
}

#[test]
fn claude_idle_registry_requires_fresh_status_and_matching_identity() {
    let dir = tempfile::tempdir().unwrap();
    let status = ClaudeStatusObservation {
        path: dir.path().join("status.json"),
        after_ms: 100,
    };
    let idle = json!({"pid":123,"sessionId":"conversation","kind":"interactive",
        "status":"idle","statusUpdatedAt":101});
    assert!(!status.idle(123, "conversation"));
    for change in [
        json!({"pid":124}),
        json!({"sessionId":"other"}),
        json!({"kind":"background"}),
        json!({"status":"busy"}),
        json!({"status":"waiting"}),
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
        assert!(!status.idle(123, "conversation"), "{change}");
    }
    for malformed in ["{", &" ".repeat(64 * 1024 + 1)] {
        std::fs::write(&status.path, malformed).unwrap();
        assert!(!status.idle(123, "conversation"));
    }
    std::fs::write(&status.path, idle.to_string()).unwrap();
    assert!(status.idle(123, "conversation"));
}

#[test]
fn claude_early_interruption_uses_idle_registry_without_a_transcript_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("turn.jsonl");
    let mut observation = observation(&transcript, None);
    let status_path = dir.path().join("status.json");
    std::fs::write(
        &status_path,
        json!({"pid":std::process::id(),
        "sessionId":"conversation","kind":"interactive","status":"idle",
        "statusUpdatedAt":101})
        .to_string(),
    )
    .unwrap();
    observation.claude_status = Some(ClaudeStatusObservation {
        path: status_path,
        after_ms: 100,
    });
    // Early Esc restores the draft without writing an interruption or duration record.
    let prompt = json!({"type":"user","sessionId":"conversation",
        "promptId":"prompt","uuid":"user"});
    std::fs::write(&transcript, format!("{prompt}\n")).unwrap();
    let turn = follow(&observation).unwrap();
    assert!(turn.last_text.is_none());
    assert!(turn.background_tasks.is_empty());

    let next = json!({"type":"user","sessionId":"conversation",
        "promptId":"next","uuid":"new-user"});
    std::fs::write(&transcript, format!("{prompt}\n{next}\n")).unwrap();
    assert_eq!(
        follow(&observation).err().unwrap().to_string(),
        "Claude prompt replaced"
    );

    // A reused PID cannot be confirmed by a registry left by its previous process.
    std::fs::write(&transcript, format!("{prompt}\n")).unwrap();
    observation.process.as_mut().unwrap().1 += 1;
    assert_eq!(
        follow(&observation).err().unwrap().to_string(),
        "claude process exited"
    );
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
