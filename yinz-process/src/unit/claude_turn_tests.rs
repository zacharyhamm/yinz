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

fn task_fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/claude-task-executions.json")).unwrap()
}

fn task_launch(tasks: &mut tasks::Tasks, tool: &str, task: &str) -> Vec<HookEvent> {
    tasks.record(&json!({"sessionId":"conversation", "type":"assistant", "message":{"content":[{"type":"tool_use", "id":tool,"name":"Agent"}]}})).unwrap();
    tasks.record(&json!({"sessionId":"conversation", "type":"user", "message":{"content":[{"type":"tool_result","tool_use_id":tool}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":task}})).unwrap()
}

fn task_completion(task: &str, tool: Option<&str>, run: &str) -> Value {
    let causal = tool
        .map(|tool| format!("<tool-use-id>{tool}</tool-use-id>"))
        .unwrap_or_default();
    json!({"sessionId":"conversation", "type":"user", "origin":{"kind":"task-notification","producer":"session-task","runId":run}, "message":{"content":format!("<task-notification><task-id>{task}</task-id>{causal}<status>completed</status></task-notification>")}})
}

fn interactive_fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/claude-interactive-observation.json")).unwrap()
}

fn sidechain_completion(fixture: &Value) -> &Value {
    &fixture["worker_records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["record"]["joinedNoticeRunIds"].is_array())
        .unwrap()["record"]
}

#[test]
fn captured_system_banner_completion_survives_early_delivery_and_duplicate_replay() {
    let fixture = interactive_fixture();
    let completion = sidechain_completion(&fixture);
    assert!(completion["message"]["content"]
        .as_str()
        .unwrap()
        .starts_with("[SYSTEM NOTIFICATION - NOT USER INPUT]\n"));
    for completion_first in [false, true] {
        let mut tasks = tasks::Tasks::new("fixture_1");
        if completion_first {
            assert!(tasks.record(completion).unwrap().is_empty());
        }
        let records: Vec<_> = fixture["parent_records"]
            .as_array()
            .unwrap()
            .iter()
            .chain(fixture["worker_records"].as_array().unwrap())
            .map(|entry| &entry["record"])
            .collect();
        let events: Vec<_> = records
            .iter()
            .flat_map(|record| tasks.record(record).unwrap())
            .collect();
        assert_eq!(events.len(), 6);
        let resumed: Vec<_> = events
            .iter()
            .filter(|event| {
                event.claude_task_execution.as_ref().unwrap().tool_use_id == "fixture_7"
            })
            .collect();
        assert_eq!(resumed.len(), 2);
        assert_eq!(resumed[0].hook_event_name, "SubagentStart");
        assert_eq!(resumed[1].hook_event_name, "SubagentStop");
        assert_eq!(
            resumed[1]
                .claude_task_execution
                .as_ref()
                .unwrap()
                .run_id
                .as_deref(),
            Some("fixture_12")
        );
        assert!(!tasks.pending());
        for record in records {
            assert!(tasks.record(record).unwrap().is_empty());
        }
        assert!(!tasks.pending());
    }
}

#[test]
fn task_notifications_accept_only_bare_xml_or_the_captured_system_banner() {
    let fixture = interactive_fixture();
    let captured = sidechain_completion(&fixture);
    let text = captured["message"]["content"].as_str().unwrap();
    let (banner, _) = text.split_once("<task-notification>").unwrap();
    let xml = text.strip_prefix(banner).unwrap();
    for (body, accepted) in [
        (xml.to_owned(), true),
        (text.to_owned(), true),
        (format!(" \n{text}\n "), true),
        (format!("prefix\n{xml}"), false),
        (
            format!("[SYSTEM NOTIFICATION - NOT USER INPUT]\n{xml}"),
            false,
        ),
        (
            format!("{}{}", banner.replace("automated", "external"), xml),
            false,
        ),
        (format!("{banner}prefix\n{xml}"), false),
        (format!("{banner}{banner}{xml}"), false),
        (format!("{text}\nsuffix"), false),
        (format!("{text}\n{xml}"), false),
        (
            format!("{banner}<task-notification>{xml}</task-notification>"),
            false,
        ),
        (text.replace("</task-notification>", ""), false),
        (text.replace("<task-notification>", ""), false),
        (
            text.replace("<status>", "</task-notification><status>"),
            false,
        ),
        (text.replace("</task-id>", ""), false),
        (text.replace("</tool-use-id>", ""), false),
        (
            text.replace("<status>", "<status>completed</status><status>"),
            false,
        ),
    ] {
        for attachment in [false, true] {
            let mut tasks = tasks::Tasks::new("conversation");
            task_launch(&mut tasks, "fixture_7", "fixture_3");
            let mut record = captured.clone();
            record["sessionId"] = json!("conversation");
            record["message"]["content"] = json!(body);
            if attachment {
                record = json!({"sessionId":"conversation", "type":"attachment",
                    "attachment":{"type":"queued_command", "commandMode":"task-notification",
                        "origin":record["origin"], "runId":"fixture_12", "prompt":body}});
            }
            let events = tasks.record(&record).unwrap();
            assert_eq!(
                events.len(),
                usize::from(accepted),
                "{body}, attachment={attachment}"
            );
            assert_eq!(tasks.pending(), !accepted, "{body}");
            assert!(tasks.record(&record).unwrap().is_empty());
        }
    }
}

#[test]
fn captured_banner_does_not_bypass_completion_origin_or_execution_checks() {
    let fixture = interactive_fixture();
    let captured = sidechain_completion(&fixture);
    let text = captured["message"]["content"].as_str().unwrap();
    let mut invalid = vec![];
    for (pointer, value) in [
        ("/sessionId", json!("foreign")),
        ("/origin/kind", json!("user")),
        ("/origin/producer", json!("foreign")),
        ("/joinedNoticeRunIds", json!([])),
        ("/joinedNoticeRunIds", json!(["run1", "run2"])),
        ("/joinedNoticeRunIds/0", json!("")),
        ("/joinedNoticeRunIds/0", json!("bad/run")),
        ("/joinedNoticeRunIds/0", json!("r".repeat(129))),
    ] {
        let mut record = captured.clone();
        record["sessionId"] = json!("conversation");
        *record.pointer_mut(pointer).unwrap() = value;
        invalid.push(record);
    }
    for (from, to) in [
        ("completed", "running"),
        ("completed", "failed"),
        ("fixture_3", ""),
        ("fixture_3", "foreign"),
        ("fixture_3", "bad/task"),
        ("fixture_7", ""),
        ("fixture_7", "foreign"),
        ("fixture_7", "bad/tool"),
        (
            "<tool-use-id>",
            "<tool-use-id>fixture_7</tool-use-id><tool-use-id>",
        ),
    ] {
        let mut record = captured.clone();
        record["sessionId"] = json!("conversation");
        record["message"]["content"] = json!(text.replace(from, to));
        invalid.push(record);
    }
    for record in invalid {
        let mut tasks = tasks::Tasks::new("conversation");
        task_launch(&mut tasks, "fixture_7", "fixture_3");
        assert!(tasks.record(&record).unwrap().is_empty(), "{record}");
        assert!(tasks.pending(), "{record}");
    }
}

#[test]
fn helper_completion_requires_the_sole_initial_launch_and_retains_run_tombstones() {
    let mut tasks = tasks::Tasks::new("conversation");
    let completion = task_completion("helper", None, "run-1");
    assert!(tasks.record(&completion).unwrap().is_empty());
    let events = task_launch(&mut tasks, "launch-1", "helper");
    assert_eq!(events.len(), 2);
    assert!(!tasks.pending());
    task_launch(&mut tasks, "launch-2", "helper");
    assert!(tasks.record(&completion).unwrap().is_empty());
    assert!(tasks
        .record(&task_completion("helper", Some("launch-2"), "run-1"))
        .unwrap()
        .is_empty());
    assert!(tasks.pending());
    task_launch(&mut tasks, "launch-3", "helper");
    // Two unfinished executions are ambiguous; retain the notice until later evidence.
    assert!(tasks
        .record(&task_completion("helper", None, "run-3"))
        .unwrap()
        .is_empty());
    assert!(tasks.pending());
    let events = tasks
        .record(&task_completion("helper", Some("launch-2"), "run-2"))
        .unwrap();
    assert_eq!(events.len(), 1);
    assert!(tasks.pending());
    assert_eq!(
        tasks
            .record(&task_completion("helper", Some("launch-3"), "run-3"))
            .unwrap()
            .len(),
        1
    );
    assert!(!tasks.pending());
    assert!(tasks.record(&completion).unwrap().is_empty());
}

#[test]
fn unidentified_old_completion_cannot_finish_a_known_resume() {
    for (initial_observed, completion_first) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let mut tasks = tasks::Tasks::new("conversation");
        if initial_observed {
            task_launch(&mut tasks, "initial", "worker");
            tasks
                .record(&task_completion("worker", Some("initial"), "first-run"))
                .unwrap();
        }
        let older = task_completion("worker", None, "older-helper-run");
        if completion_first {
            assert!(tasks.record(&older).unwrap().is_empty());
        }
        tasks.record(&json!({"sessionId":"conversation", "type":"assistant", "message":{"content":[{"type":"tool_use", "id":"resume","name":"SendMessage"}]}})).unwrap();
        let events = tasks.record(&json!({"sessionId":"conversation", "type":"user", "message":{"content":[{"type":"tool_result","tool_use_id":"resume"}]},"toolUseResult":{"success":true,"revivedAgentId":"worker"}})).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].hook_event_name, "SubagentStart");
        assert!(tasks.record(&older).unwrap().is_empty());
        assert!(tasks.pending());
        assert!(tasks.record(&older).unwrap().is_empty());
        assert_eq!(
            tasks
                .record(&task_completion("worker", Some("resume"), "resumed-run"))
                .unwrap()
                .len(),
            1
        );
        assert!(!tasks.pending());
    }
}

#[test]
fn successful_active_worker_messages_do_not_create_executions() {
    let mut tasks = tasks::Tasks::new("conversation");
    tasks.record(&json!({"sessionId":"conversation", "isSidechain":true, "type":"assistant", "message":{"content":[{"type":"tool_use", "id":"message", "name":"SendMessage"}]}})).unwrap();
    assert!(tasks.record(&json!({"sessionId":"conversation", "isSidechain":true, "type":"user", "message":{"content":[{"type":"tool_result", "tool_use_id":"message"}]}, "toolUseResult":{"success":true}})).unwrap().is_empty());
    assert!(!tasks.pending());
}

#[test]
fn malformed_causal_completion_cannot_fall_back_to_a_newer_execution() {
    let mut tasks = tasks::Tasks::new("conversation");
    task_launch(&mut tasks, "newer", "worker");
    for tool in [
        "<tool-use-id>older",
        "<tool-use-id>older</tool-use-id><tool-use-id>other</tool-use-id>",
    ] {
        let mut completion = task_completion("worker", None, "old-run");
        completion["message"]["content"] = json!(format!("<task-notification><task-id>worker</task-id>{tool}<status>completed</status></task-notification>"));
        assert!(tasks.record(&completion).unwrap().is_empty());
        assert!(tasks.pending());
    }
}

#[test]
fn task_observer_lock_scopes_process_conversation_and_route_and_releases_on_exit() {
    let dir = tempfile::tempdir().unwrap();
    let mut observation = observation(&dir.path().join("root.jsonl"), None);
    let held = task_observer::lock(&observation, dir.path())
        .unwrap()
        .unwrap();
    assert!(task_observer::lock(&observation, dir.path())
        .unwrap()
        .is_none());
    observation.hook.prompt_id = Some("another-prompt".into());
    assert!(task_observer::lock(&observation, dir.path())
        .unwrap()
        .is_none());
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
        assert!(task_observer::lock(&other, dir.path()).unwrap().is_some());
    }
    drop(held);
    assert!(task_observer::lock(&observation, dir.path())
        .unwrap()
        .is_some());
}

#[test]
fn worker_trigger_initializes_a_missing_marker_but_cannot_replace_a_new_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversation");
    let worker = HookEvent {
        hook_event_name: "SubagentStart".into(),
        agent_id: Some("worker".into()),
        ..hook()
    };
    update_conversation_marker(&path, "claude", &worker).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "conversation");
    update_conversation_marker(
        &path,
        "claude",
        &HookEvent {
            session_id: "replacement".into(),
            hook_event_name: "SessionStart".into(),
            ..hook()
        },
    )
    .unwrap();
    update_conversation_marker(&path, "claude", &worker).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn task_observer_follows_new_sidechains_after_parent_idle_and_replays_after_report_failure() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/claude-interactive-observation.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("root.jsonl");
    let marker = dir.path().join("conversation");
    let session = fixture["parent_records"][0]["record"]["sessionId"]
        .as_str()
        .unwrap();
    std::fs::write(&marker, session).unwrap();
    let mut observation = observation(&path, Some(marker.clone()));
    observation.hook.session_id = session.into();
    let records = fixture["parent_records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["record"].to_string() + "\n")
        .collect::<String>();
    std::fs::write(&path, records).unwrap();
    // Parent completion cannot end the task observer, including with no current work.
    let mut root = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        root,
        "{}",
        json!({"type":"system","sessionId":session,"subtype":"turn_duration"})
    )
    .unwrap();
    let mut first = vec![];
    let failure = task_observer::follow(&observation, |event| {
        first.push(event.clone());
        let sidechains = path.with_extension("").join("subagents");
        std::fs::create_dir_all(&sidechains).unwrap();
        let records = fixture["worker_records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["record"].to_string() + "\n")
            .collect::<String>();
        std::fs::write(sidechains.join("agent-helper.jsonl"), records).unwrap();
        if first.len() == 6 {
            return Err(anyhow!("injected report failure"));
        }
        Ok(())
    });
    assert!(failure
        .unwrap_err()
        .to_string()
        .contains("injected report failure"));
    assert_eq!(
        first
            .iter()
            .filter(|event| event.hook_event_name == "SubagentStop")
            .count(),
        3
    );
    let mut replayed = vec![];
    task_observer::follow(&observation, |event| {
        replayed.push(event.clone());
        if replayed.len() == 6 {
            std::fs::write(&marker, "replacement").unwrap();
        }
        Ok(())
    })
    .unwrap();
    // File discovery order changes on restart; provider identities stay the same.
    let key = |event: &HookEvent| {
        let execution = event.claude_task_execution.as_ref().unwrap();
        (execution.tool_use_id.clone(), execution.run_id.clone())
    };
    first.sort_by_key(key);
    replayed.sort_by_key(key);
    assert_eq!(first, replayed);
}

fn tool_hook_fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/claude-tool-hook-ledger.json")).unwrap()
}

#[test]
fn captured_tool_hooks_join_nested_launches_but_success_alone_is_not_resumption() {
    let fixture = tool_hook_fixture();
    assert_eq!(fixture["provider"]["version"], "2.1.318");
    for (name, case) in fixture["cases"].as_object().unwrap() {
        assert_eq!(case["exit_code"], 0, "{name}");
        assert_eq!(case["process_timeout"], false, "{name}");
        let hooks: Vec<HookEvent> = case["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| serde_json::from_value(entry["payload"].clone()).unwrap())
            .collect();
        let runs = case["runs"].as_array().unwrap();
        for start in runs
            .iter()
            .map(|entry| &entry["record"])
            .filter(|run| run["subtype"] == "task_started")
        {
            let tool = start["tool_use_id"].as_str().unwrap();
            let pre = hooks
                .iter()
                .find(|hook| {
                    hook.hook_event_name == "PreToolUse"
                        && hook.tool_use_id.as_deref() == Some(tool)
                })
                .unwrap();
            let post = hooks
                .iter()
                .find(|hook| {
                    hook.hook_event_name == "PostToolUse"
                        && hook.tool_use_id.as_deref() == Some(tool)
                })
                .unwrap();
            assert_eq!(pre.session_id, start["session_id"]);
            assert_eq!(pre.session_id, post.session_id);
            assert_eq!(pre.agent_id, post.agent_id);
            assert_eq!(pre.tool_name, post.tool_name);
            let response = post.tool_response.as_ref().unwrap();
            if pre.tool_name.as_deref() == Some("Agent") {
                assert_eq!(response["isAsync"], true);
                assert_eq!(response["status"], "async_launched");
                assert_eq!(response["agentId"], start["task_id"]);
            } else {
                assert_eq!(pre.tool_name.as_deref(), Some("SendMessage"));
                assert_eq!(response["success"], true);
                assert_eq!(response["revivedAgentId"], start["task_id"]);
                // The nested caller has the same causal tool ID, but its response
                // lacks the field the current root transcript adapter requires.
                if pre.agent_id.is_some() {
                    assert!(response.get("resumedAgentId").is_none());
                } else {
                    assert_eq!(response["resumedAgentId"], start["task_id"]);
                }
            }
            assert!(runs.iter().any(|entry| {
                let end = &entry["record"];
                end["subtype"] == "task_notification"
                    && end["status"] == "completed"
                    && end["task_id"] == start["task_id"]
                    && end["tool_use_id"] == start["tool_use_id"]
                    && end["run_id"] == start["run_id"]
            }));
        }
        if name == "message-active" {
            assert_eq!(case["handback_gate_expired"], false);
            let message = hooks
                .iter()
                .find(|hook| {
                    hook.hook_event_name == "PostToolUse"
                        && hook.tool_name.as_deref() == Some("SendMessage")
                })
                .unwrap();
            let response = message.tool_response.as_ref().unwrap();
            assert_eq!(response["success"], true);
            assert!(response.get("revivedAgentId").is_none());
            assert!(response.get("resumedAgentId").is_none());
            assert_eq!(runs.len(), 2, "only the original execution ran");
            assert!(runs.iter().all(|entry| {
                entry["record"]["tool_use_id"].as_str() != message.tool_use_id.as_deref()
            }));
        }
    }
}

#[test]
fn captured_launch_barrier_failures_need_more_than_post_tool_hooks() {
    let fixture = tool_hook_fixture();
    for name in ["continue-error", "block-error", "block-timeout"] {
        let case = &fixture["cases"][name];
        let fault = &case["fault"];
        let tool = &fault["tool_use_id"];
        let hooks = case["hooks"].as_array().unwrap();
        let pre = hooks
            .iter()
            .map(|entry| &entry["payload"])
            .find(|hook| hook["hook_event_name"] == "PreToolUse" && &hook["tool_use_id"] == tool)
            .unwrap();
        assert_eq!(pre["agent_id"], fault["caller_agent_id"]);
        let post: Vec<_> = hooks
            .iter()
            .map(|entry| &entry["payload"])
            .filter(|hook| hook["hook_event_name"] != "PreToolUse" && &hook["tool_use_id"] == tool)
            .collect();
        let started = case["runs"].as_array().unwrap().iter().any(|entry| {
            entry["record"]["subtype"] == "task_started" && &entry["record"]["tool_use_id"] == tool
        });
        if name == "continue-error" {
            assert!(fault["onFailure"].is_null());
            assert!(
                started,
                "ordinary reporting failure does not prevent launch"
            );
            assert_eq!(post.len(), 1);
            assert_eq!(post[0]["hook_event_name"], "PostToolUse");
        } else {
            assert_eq!(fault["onFailure"], "block");
            assert!(!started);
            assert!(post.is_empty(), "no PostToolUseFailure on this denied call");
            let failures = case["failures"].as_array().unwrap();
            assert_eq!(failures.len(), 1);
            assert_eq!(&failures[0]["tool_use_id"], tool);
            assert_eq!(failures[0]["agent_id"], pre["agent_id"]);
            assert_eq!(failures[0]["is_error"], true);
            assert_eq!(failures[0]["classified_on_failure_block"], true);
            assert_eq!(failures[0]["classified_timeout"], name == "block-timeout");
        }
    }
}

#[test]
fn captured_nested_launch_response_is_lost_by_current_broadcast_pruning() {
    let fixture = tool_hook_fixture();
    let payload = fixture["cases"]["nested-resume"]["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["payload"])
        .find(|hook| hook["hook_event_name"] == "PostToolUse" && hook["tool_name"] == "SendMessage")
        .unwrap();
    let mut event: HookEvent = serde_json::from_value(payload.clone()).unwrap();
    let raw = serde_json::to_string(&event).unwrap();
    assert_eq!(event.broadcast_json(raw.clone()), raw);
    // Real tool responses also carry prose. Only this padding is synthetic; the
    // successful nested launch fields above come from the provider capture.
    event.tool_response.as_mut().unwrap()["message"] = "x"
        .repeat(yinz_types::agent::HOOK_BROADCAST_MAX_BYTES)
        .into();
    let raw = serde_json::to_string(&event).unwrap();
    let pruned: HookEvent = serde_json::from_str(&event.broadcast_json(raw)).unwrap();
    assert_eq!(pruned.tool_use_id, event.tool_use_id);
    assert_eq!(pruned.agent_id, event.agent_id);
    assert!(pruned.tool_response.is_none());
    assert!(pruned.claude_task_execution.is_none());
    // This characterizes a prerequisite, not a usable ledger: enrichment must
    // preserve validated launch evidence before any local or broker pruning.
}

#[test]
fn older_sanitized_capture_without_resumption_fields_does_not_invent_a_launch() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/claude-resumption-observation.json")).unwrap();
    for (case, data) in fixture["cases"].as_object().unwrap() {
        assert_eq!(data["exit_code"], 0);
        let runs = data["runs"].as_array().unwrap();
        let session = runs[0]["record"]["session_id"].as_str().unwrap();
        let mut tasks = tasks::Tasks::new(session);
        let mut observed = vec![];
        for entry in data["parent_records"].as_array().unwrap() {
            observed.extend(tasks.record(&entry["record"]).unwrap());
        }
        assert!(!tasks.pending());
        let missing: Vec<_> = runs
            .iter()
            .map(|entry| &entry["record"])
            .filter(|run| run["subtype"] == "task_started")
            .filter(|run| {
                !observed.iter().any(|event| {
                    event.claude_task_execution.as_ref().unwrap().tool_use_id == run["tool_use_id"]
                })
            })
            .collect();
        if case == "same-prompt" {
            assert!(missing.is_empty());
            assert_eq!(observed.len(), runs.len());
            assert_eq!(data["raw_payload_equality"]["SubagentStart"], true);
            assert_eq!(data["raw_payload_equality"]["SubagentStop"], true);
        } else {
            assert_eq!(case, "nested-resume");
            assert_eq!(missing.len(), 1);
            assert_eq!(observed.len() + 2, runs.len());
            let tool = missing[0]["tool_use_id"].as_str().unwrap();
            // This real SendMessage call occurs in the coordinator's sidechain. The
            // root observer sees its completed notification but cannot join its launch.
            assert!(data["worker_records"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| {
                    let record = &entry["record"];
                    record["isSidechain"] == true
                        && record["message"]["content"][0]["name"] == "SendMessage"
                        && record["message"]["content"][0]["id"] == tool
                }));
            assert!(data["parent_records"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| {
                    entry["record"]["attachment"]["prompt"]
                        .as_str()
                        .is_some_and(|text| {
                            text.contains(&format!("<tool-use-id>{tool}</tool-use-id>"))
                        })
                }));
            // This earlier projection retained only success=true in the nested
            // result. The newer interactive fixture carries revivedAgentId.
            for entry in data["worker_records"].as_array().unwrap() {
                assert!(tasks.record(&entry["record"]).unwrap().is_empty());
            }
            assert!(!tasks.pending());
            assert_eq!(observed.len() + 2, runs.len());
        }
    }
}

#[test]
fn captured_interactive_reporter_loss_does_not_block_nested_resumption() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/claude-interactive-observation.json")).unwrap();
    assert_eq!(fixture["provenance"]["mode"], "interactive");
    assert_eq!(fixture["provenance"]["result"]["exit_code"], 0);
    assert_eq!(fixture["provenance"]["result"]["timeout"], false);
    let hooks = fixture["hooks"].as_array().unwrap();
    let lost: Vec<_> = hooks
        .iter()
        .filter(|entry| entry["delivered"] == false)
        .collect();
    assert_eq!(lost.len(), 2);
    let pre = &lost[0]["payload"];
    let post = &lost[1]["payload"];
    assert_eq!(pre["hook_event_name"], "PreToolUse");
    assert_eq!(post["hook_event_name"], "PostToolUse");
    assert_eq!(pre["tool_name"], "SendMessage");
    assert_eq!(pre["tool_use_id"], post["tool_use_id"]);
    assert_eq!(pre["agent_id"], post["agent_id"]);
    assert!(pre["agent_id"].is_string());
    assert_eq!(post["tool_response"]["success"], true);
    assert!(post["tool_response"].get("resumedAgentId").is_none());
    let task = post["tool_response"]["revivedAgentId"].as_str().unwrap();
    let tool = post["tool_use_id"].as_str().unwrap();
    for event in ["SubagentStart", "SubagentStop"] {
        assert_eq!(
            hooks
                .iter()
                .filter(|entry| {
                    entry["payload"]["hook_event_name"] == event
                        && entry["payload"]["agent_id"] == task
                })
                .count(),
            2
        );
    }
    let worker_records = fixture["worker_records"].as_array().unwrap();
    for event in ["PreToolUse", "PostToolUse"] {
        assert!(worker_records.iter().any(|entry| {
            let attachment = &entry["record"]["attachment"];
            attachment["type"] == "hook_non_blocking_error"
                && attachment["hookEvent"] == event
                && attachment["toolUseID"] == tool
                && attachment["exitCode"] == 1
        }));
    }
    // Interactive completion was routed to the helper, with its run identity
    // alongside the joined notice. The root did not receive this as a user turn.
    assert!(worker_records.iter().any(|entry| {
        let record = &entry["record"];
        record["isSidechain"] == true
            && record["agentId"] == pre["agent_id"]
            && record["origin"]["kind"] == "task-notification"
            && record["joinedNoticeRunIds"][0].is_string()
            && record["message"]["content"].as_str().is_some_and(|text| {
                text.contains(&format!("<task-id>{task}</task-id>"))
                    && text.contains(&format!("<tool-use-id>{tool}</tool-use-id>"))
                    && text.contains("<status>completed</status>")
            })
    }));
    let mut tasks = tasks::Tasks::new(pre["session_id"].as_str().unwrap());
    let mut observed = vec![];
    for entry in fixture["parent_records"].as_array().unwrap() {
        observed.extend(tasks.record(&entry["record"]).unwrap());
    }
    assert!(observed
        .iter()
        .all(|event| { event.claude_task_execution.as_ref().unwrap().tool_use_id != tool }));
    for entry in worker_records {
        observed.extend(tasks.record(&entry["record"]).unwrap());
    }
    assert!(!tasks.pending());
    assert!(observed.iter().any(
        |event| event.claude_task_execution.as_ref().unwrap().tool_use_id == tool
            && event.hook_event_name == "SubagentStop"
    ));
    for entry in fixture["parent_records"]
        .as_array()
        .unwrap()
        .iter()
        .chain(worker_records)
    {
        assert!(tasks.record(&entry["record"]).unwrap().is_empty());
    }
    assert_eq!(fixture["registry"][0]["record"]["status"], "idle");
    // Neither registry Idle nor successful reconstruction of a finite capture
    // provides a provider revision/snapshot boundary for live observation.
}

#[test]
fn captured_transcript_replay_recovers_completion_before_launch() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/claude-resumption-observation.json")).unwrap();
    let case = &fixture["cases"]["same-prompt"];
    let records: Vec<_> = case["parent_records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["record"])
        .collect();
    let session = case["runs"][0]["record"]["session_id"].as_str().unwrap();
    let resume = records
        .iter()
        .find(|record| record["toolUseResult"]["resumedAgentId"].is_string())
        .unwrap();
    let tool = resume["message"]["content"][0]["tool_use_id"]
        .as_str()
        .unwrap();

    // Synthetic loss/delay of the captured resume result. A completed first run
    // plus an unmatched second completion leaves no pending execution, even
    // though the observer never reconstructed the second launch. This is a
    // best-effort source; replay can only recover the executions it observes.
    let mut tasks = tasks::Tasks::new(session);
    let mut partial = vec![];
    for record in &records {
        if record != resume {
            partial.extend(tasks.record(record).unwrap());
        }
    }
    assert_eq!(partial.len(), 2);
    assert!(!tasks.pending());
    assert!(partial
        .iter()
        .all(|event| { event.claude_task_execution.as_ref().unwrap().tool_use_id != tool }));

    // Late persistence joins the retained completion to its launch.
    let late = tasks.record(resume).unwrap();
    assert_eq!(late.len(), 2);
    assert!(late[0]
        .claude_task_execution
        .as_ref()
        .unwrap()
        .run_id
        .is_none());
    assert!(late[1]
        .claude_task_execution
        .as_ref()
        .unwrap()
        .run_id
        .is_some());
    assert!(!tasks.pending());

    // An independent replay can reconstruct both executions from this finite,
    // known-complete fixture. EOF itself does not prove that a live file is complete.
    let mut replay = tasks::Tasks::new(session);
    let mut recovered = vec![];
    for record in &records {
        recovered.extend(replay.record(record).unwrap());
    }
    assert_eq!(recovered.len(), 4);
    assert!(!replay.pending());
    assert!(recovered.iter().any(|event| {
        let execution = event.claude_task_execution.as_ref().unwrap();
        execution.tool_use_id == tool && execution.run_id.is_some()
    }));
    for record in &records {
        assert!(replay.record(record).unwrap().is_empty());
    }
}

#[test]
fn captured_task_runs_correlate_launch_resume_and_successful_handback_notifications() {
    let fixture = task_fixture();
    assert_eq!(fixture["provider"]["version"], "2.1.318");
    for (case, data) in fixture["cases"].as_object().unwrap() {
        let records = data["parent_records"].as_array().unwrap();
        let session = records[0]["record"]["sessionId"].as_str().unwrap();
        let mut tasks = tasks::Tasks::new(session);
        let mut events = vec![];
        for entry in records {
            events.extend(tasks.record(&entry["record"]).unwrap());
        }
        assert!(!tasks.pending(), "{case}");
        let mut turn = Turn::new(
            "claude",
            &HookEvent {
                session_id: session.into(),
                ..hook()
            },
        );
        turn.record(
            &json!({"type":"user", "sessionId":session, "promptId":"prompt", "uuid":"parent"}),
        )
        .unwrap();
        for entry in records {
            let mut record = entry["record"].clone();
            record["promptId"] = json!("prompt");
            turn.record(&record).unwrap();
        }
        assert_eq!(
            turn.background_tasks,
            [events[0].agent_id.clone().unwrap()],
            "each prompt retains its launched or resumed task: {case}"
        );
        let runs = data["runs"].as_array().unwrap();
        assert_eq!(events.len(), runs.len(), "{case}");
        for (event, run) in events.iter().zip(runs) {
            let run = &run["record"];
            assert_eq!(event.agent_id.as_deref(), run["task_id"].as_str(), "{case}");
            let execution = event.claude_task_execution.as_ref().unwrap();
            assert_eq!(execution.tool_use_id, run["tool_use_id"], "{case}");
            if event.hook_event_name == "SubagentStop" {
                assert_eq!(
                    execution.run_id.as_deref(),
                    run["run_id"].as_str(),
                    "{case}"
                );
            }
            let json = serde_json::to_string(event).unwrap();
            let (_, bounded) = event.broadcast(json);
            assert_eq!(
                serde_json::from_str::<HookEvent>(&bounded)
                    .unwrap()
                    .claude_task_execution,
                event.claude_task_execution
            );
        }
        // Multiple observers replay the same transcript; duplicates do not create work.
        for entry in records {
            assert!(tasks.record(&entry["record"]).unwrap().is_empty(), "{case}");
        }
    }
    let hooks = fixture["cases"]["same-prompt"]["hooks"].as_array().unwrap();
    for name in ["SubagentStart", "SubagentStop"] {
        let payloads: Vec<_> = hooks
            .iter()
            .map(|h| &h["payload"])
            .filter(|h| h["hook_event_name"] == name)
            .collect();
        assert_eq!(payloads.len(), 2);
        assert_eq!(
            payloads[0], payloads[1],
            "raw {name} cannot identify executions"
        );
    }
}

#[test]
fn captured_handback_success_is_distinct_from_a_denied_attempt_or_blocked_stop() {
    let fixture = task_fixture();
    let case = &fixture["cases"]["denied-handback"];
    let mut succeeded = 0;
    let mut denied = 0;
    for entry in case["worker_results"].as_array().unwrap() {
        let record = &entry["record"];
        for result in record["message"]["content"].as_array().unwrap() {
            if result["type"] != "tool_result" {
                continue;
            }
            if result["is_error"] == true {
                denied += 1;
                assert_ne!(record["toolEndsTurn"], true);
            } else if record["toolEndsTurn"] == true {
                let response: Value =
                    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
                assert_eq!(response["success"], true);
                assert!(case["hooks"].as_array().unwrap().iter().any(|hook| {
                    let hook = &hook["payload"];
                    hook["hook_event_name"] == "PostToolUse"
                        && hook["tool_name"] == "SubagentHandback"
                        && hook["tool_response"]["success"] == true
                        && hook["tool_use_id"] == result["tool_use_id"]
                }));
                succeeded += 1;
            }
        }
    }
    assert_eq!((succeeded, denied), (2, 1));
    let stops: Vec<_> = fixture["cases"]["blocked-plain"]["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|hook| hook["payload"]["hook_event_name"] == "SubagentStop")
        .collect();
    assert_eq!(stops.len(), 2);
    assert_eq!(stops[0]["payload"]["stop_hook_active"], false);
    assert_eq!(stops[1]["payload"]["stop_hook_active"], true);
}

#[test]
fn task_completions_require_provider_origin_identity_and_matching_launch() {
    let fixture = task_fixture();
    let records = fixture["cases"]["same-prompt"]["parent_records"]
        .as_array()
        .unwrap();
    for change in [
        json!({"sessionId":"foreign"}),
        json!({"attachment":{"type":"queued_command","commandMode":"task-notification"}}),
    ] {
        let mut tasks = tasks::Tasks::new("ID_0350");
        for entry in &records[..2] {
            tasks.record(&entry["record"]).unwrap();
        }
        let mut completion = records[2]["record"].clone();
        completion
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert!(tasks.record(&completion).unwrap().is_empty());
        assert!(tasks.pending());
    }
    for field in ["runId", "origin", "prompt"] {
        let mut tasks = tasks::Tasks::new("ID_0350");
        for entry in &records[..2] {
            tasks.record(&entry["record"]).unwrap();
        }
        let mut completion = records[2]["record"].clone();
        completion["attachment"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(tasks.record(&completion).unwrap().is_empty(), "{field}");
        assert!(tasks.pending());
    }
    for replacement in ["failed", "killed", "running"] {
        let mut tasks = tasks::Tasks::new("ID_0350");
        for entry in &records[..2] {
            tasks.record(&entry["record"]).unwrap();
        }
        let mut completion = records[2]["record"].clone();
        let text = completion["attachment"]["prompt"]
            .as_str()
            .unwrap()
            .replace("completed", replacement);
        completion["attachment"]["prompt"] = json!(text);
        assert!(tasks.record(&completion).unwrap().is_empty());
        assert!(tasks.pending());
    }
}

#[test]
fn parent_observer_finishes_independently_of_background_completion() {
    use std::io::Write;
    let fixture = task_fixture();
    let records = fixture["cases"]["same-prompt"]["parent_records"]
        .as_array()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript.jsonl");
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(
        file,
        "{}",
        json!({"type":"user", "sessionId":"ID_0350", "promptId":"prompt", "uuid":"parent"})
    )
    .unwrap();
    for entry in &records[..2] {
        writeln!(file, "{}", entry["record"]).unwrap();
    }
    writeln!(file, "{}", json!({"type":"system", "sessionId":"ID_0350", "subtype":"turn_duration", "parentUuid":"parent"})).unwrap();
    let mut observation = observation(&path, None);
    observation.hook.session_id = "ID_0350".into();
    let mut events = vec![];
    let turn = follow_with_api_reporting(&observation, None, |hook| {
        events.push(hook.clone());
        if hook.claude_turn_complete == Some(true) {
            writeln!(file, "{}", records[2]["record"]).unwrap();
        }
        Ok(())
    })
    .unwrap();
    assert!(turn.reported);
    assert_eq!(
        events
            .iter()
            .map(|e| e.hook_event_name.as_str())
            .collect::<Vec<_>>(),
        ["Stop"]
    );
}

#[test]
fn old_uncorrelated_tasks_do_not_keep_later_prompt_observers_alive() {
    use std::io::Write;
    let fixture = task_fixture();
    let records = fixture["cases"]["same-prompt"]["parent_records"]
        .as_array()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript.jsonl");
    let mut file = std::fs::File::create(&path).unwrap();
    for entry in &records[..2] {
        writeln!(file, "{}", entry["record"]).unwrap();
    }
    writeln!(
        file,
        "{}",
        json!({"type":"user", "sessionId":"ID_0350", "promptId":"prompt", "uuid":"parent"})
    )
    .unwrap();
    writeln!(file, "{}", json!({"type":"system", "sessionId":"ID_0350", "subtype":"turn_duration", "parentUuid":"parent"})).unwrap();
    let mut observation = observation(&path, None);
    observation.hook.session_id = "ID_0350".into();
    let mut events = vec![];
    follow_with_api_reporting(&observation, None, |hook| {
        events.push(hook.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].claude_turn_complete, Some(true));
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
        task_observer: false,
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
        task_observer: false,
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
        json!({"type":"user","parentUuid":"answer-two","uuid":"task-two","toolUseResult":{"backgroundTaskId":"second-task"}}),
        json!({"type":"user","promptId":"prompt","parentUuid":"answer-two","uuid":"task-one","toolUseResult":{"backgroundTaskId":"first-task"}}),
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
    assert_eq!(first.background_tasks, ["first-task"]);
    assert_eq!(second.background_tasks, ["second-task"]);
}
