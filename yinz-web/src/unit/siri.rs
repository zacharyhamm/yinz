use super::*;
use crate::{
    auth::{Auth, AuthConfig, Clock, SystemClock},
    Backend, DispatchFuture, WebSettings,
};
use http_body_util::BodyExt;
use std::sync::{atomic::AtomicI64, Arc, Mutex};
use yinz_types::web::{Site, WebResponse};

#[derive(Default)]
struct Live {
    requests: Vec<(String, Value)>,
    on_readback: Option<fn(&mut Live, &str)>,
    agents: Vec<Value>,
    sends: Vec<Value>,
    replies: Vec<Value>,
    reply_on_send: bool,
    lose_send_response: bool,
    receipt: Value,
    lookup_unavailable: bool,
    launches: Vec<Value>,
    stale_run: Option<String>,
    launch_replaced: bool,
    launch_error: Option<(u16, &'static str)>,
}

impl Live {
    fn readback_requests(&self) -> &[(String, Value)] {
        let prompt = self
            .requests
            .iter()
            .rposition(|(path, _)| path == "/api/agent/prompt")
            .unwrap();
        &self.requests[prompt + 1..]
    }
}

#[derive(Default)]
struct Fixture(Mutex<Live>);

impl Backend for Fixture {
    fn dispatch(&self, req: WebRequest) -> DispatchFuture {
        let mut live = self.0.lock().unwrap();
        let body: Value = serde_json::from_str(req.body.as_deref().unwrap_or("null")).unwrap();
        live.requests.push((req.path.clone(), body.clone()));
        if !live.sends.is_empty()
            && let Some(update) = live.on_readback
        {
            update(&mut live, &req.path);
        }
        let mut status = 200;
        let value = match req.path.as_str() {
            "/api/agents" => json!({"agents":live.agents}),
            "/api/agent/assistant-reply" => {
                let filter = &body["filter"];
                assert_eq!(filter["main_only"], true);
                if live.stale_run.as_deref() != body["run"].as_str()
                    && live
                        .agents
                        .iter()
                        .any(|a| a["ref"] == body["agent"] && a["run"] == body["run"])
                {
                    let reply = live.replies.iter().rev().find(|r| {
                        r["run"] == body["run"]
                            && (filter["turn"].is_null() || filter["turn"] == r["turn"])
                            && r["revision"].as_i64().unwrap()
                                > filter["after"].as_i64().unwrap_or(0)
                    });
                    json!({"reply":reply})
                } else {
                    status = 409;
                    json!({"error":{"code":"target_changed"}})
                }
            },
            "/api/agent/prompt" => {
                live.sends.push(body.clone());
                if live.reply_on_send {
                    let id = &body["agent"]["session_id"];
                    live.replies.push(json!({"revision":2,"run":id,"text":"The requested tests passed.","turn":"our turn"}));
                    live.replies.push(json!({"revision":3,"run":id,"text":"A concurrent reply.","turn":"other turn"}));
                }
                if live.lose_send_response {
                    return Box::pin(async { None });
                }
                json!({"response":{"submission_id":body["submission_id"],"outcome":"submitted","turn_id":"our turn"}})
            },
            "/api/agent/submission-status" => {
                assert_eq!(body["agent"], live.sends.last().unwrap()["agent"]);
                assert_eq!(
                    body["submission_id"],
                    live.sends.last().unwrap()["submission_id"]
                );
                if live.lookup_unavailable {
                    return Box::pin(async { None });
                }
                let mut receipt = live.receipt.clone();
                receipt["submission_id"] = body["submission_id"].clone();
                json!({"response":receipt})
            },
            "/api/agent/launch-choices" => {
                json!({"projects":["swamp"],"providers":["claude"],"server_instance":"server"})
            },
            "/api/agent/launch" => {
                live.launches.push(body);
                if let Some((code, message)) = live.launch_error {
                    status = code;
                    json!({"error":{"code":message,"message":"Launch did not finish."}})
                } else {
                    json!({"pane_id":"1","session":"session-1"})
                }
            },
            "/api/agent/launch-status" if live.launch_replaced => {
                status = 409;
                json!({"error":{"code":"target_changed"}})
            },
            "/api/agent/launch-status" => json!(true),
            path => panic!("unexpected Siri operation {path}"),
        };
        Box::pin(async move {
            Some(WebReply::Response(WebResponse {
                status,
                headers: vec![],
                body: value.to_string(),
            }))
        })
    }
    fn session_name(&self) -> String {
        "server".into()
    }
    fn site(&self) -> Option<Arc<Site>> {
        None
    }
    fn activate(&self) {}
    fn quiesced(&self) -> bool {
        false
    }
}

fn row(i: usize) -> Value {
    let id = format!("agent-{i}");
    json!({"ref":{"id":id,"kind":"claude","session_id":id},"run":id,"pane_id":i.to_string(),
        "title":"Fix login timeout","project":"swamp","branch":format!("mobile-login-{i}"),
        "session":format!("session-{i}"),"current_session":i == 1,"state":"waiting","can_prompt":true})
}

struct Phone {
    _directory: tempfile::TempDir,
    backend: Arc<Fixture>,
    state: State,
    headers: HeaderMap,
    cursor: String,
}

fn sign_in(dir: &std::path::Path, verifier: &str) -> SessionInfo {
    let _lock = password::lock(&dir.join("web")).unwrap();
    sessions::Store::default()
        .issue(dir, verifier, "phone".into(), true, 1, SystemClock.now_ms())
        .unwrap()
        .1
}

impl Phone {
    fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }
    fn with_clock(clock: Arc<dyn Clock>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let verifier = password::tests::ready(dir.path());
        let info = sign_in(dir.path(), &verifier.verifier_id);
        let code = store::configure(dir.path(), &info.id, "listener", false)
            .unwrap()
            .unwrap();
        assert!(store::redeem(dir.path(), "another server", &code)
            .unwrap()
            .is_none());
        let token = store::redeem(dir.path(), "listener", &code)
            .unwrap()
            .unwrap();
        let backend = Arc::new(Fixture::default());
        backend.0.lock().unwrap().agents = (1..=7).map(row).collect();
        let state = State {
            settings: WebSettings::from_config(&Default::default(), dir.path()).unwrap(),
            auth: Arc::new(Auth::new(dir.path().into(), AuthConfig::default(), clock)),
            siri_commands: Default::default(),
            request_id: Default::default(),
            io_work: Arc::new(tokio::sync::Semaphore::new(4)),
            backend: backend.clone(),
            funnel_warning: Default::default(),
        };
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        Self {
            _directory: dir,
            backend,
            state,
            headers,
            cursor: "first".into(),
        }
    }
    async fn request(&self, id: &str, command: &str) -> Response<Body> {
        let body = json!({"request_id":id,"command":command}).to_string();
        handle(
            &self.state,
            "listener",
            "/api/siri/command",
            None,
            &self.headers,
            body.as_bytes(),
            i64::MAX,
        )
        .await
    }
    async fn send(&self, id: &str, command: &str) -> Value {
        let response = self.request(id, command).await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
    }
    async fn say(&mut self, command: &str) -> Value {
        let response = self.send(&self.cursor, command).await;
        self.cursor = response["request_id"].as_str().unwrap().into();
        response
    }
}

#[test]
fn siri_legacy_pairing_is_pruned_and_can_pair_again() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let verifier = password::tests::ready(data);
    let owner = sign_in(data, &verifier.verifier_id);
    let legacy_owner = sign_in(data, &verifier.verifier_id);
    let instance = "0123456789abcdef0123456789abcdef";
    let code = store::configure(data, &owner.id, instance, false)
        .unwrap()
        .unwrap();
    let token = store::redeem(data, instance, &code).unwrap().unwrap();
    let path = data.join("web/siri.json");
    let read_rows =
        || serde_json::from_slice::<Vec<Value>>(&std::fs::read(&path).unwrap()).unwrap();
    let bound = read_rows();
    assert_eq!(bound[0]["listener_instance"], instance);
    // Even a legacy session name identical to a binding ID cannot identify its issuer.
    let mut mixed = bound.clone();
    mixed.push(json!({
        "owner": legacy_owner.id, "server": instance,
        "code": [sessions::hash("legacy code"), SystemClock.now_ms() + 60_000],
        "token_hash": sessions::hash("legacy token"), "revision": 0, "dialogue": Dialogue::default()
    }));
    password::atomic_write(
        &data.join("web"),
        "siri.json",
        &serde_json::to_vec(&mixed).unwrap(),
    )
    .unwrap();
    assert!(store::redeem(data, instance, "legacy code")
        .unwrap()
        .is_none());
    assert_eq!(read_rows(), bound);
    assert!(store::load(data, instance, &sessions::hash("legacy token"))
        .unwrap()
        .is_none());
    let fresh = store::configure(data, &legacy_owner.id, instance, false)
        .unwrap()
        .unwrap();
    let repaired = store::redeem(data, instance, &fresh).unwrap().unwrap();
    for (token, expected) in [(token, owner), (repaired, legacy_owner)] {
        let phone = store::load(data, instance, &sessions::hash(&token))
            .unwrap()
            .unwrap();
        assert_eq!(phone.session.id, expected.id);
    }
}

#[tokio::test]
async fn siri_filters_paging_stable_ordinals_and_stale_conversations() {
    let mut phone = Phone::new();
    let selected = phone
        .say("Find the waiting Claude login agent in swamp on mobile login 3 in session session 3")
        .await;
    assert!(selected["speech"].as_str().unwrap().contains("Selected"));
    let list = phone.say("Find the waiting Claude agents in swamp").await;
    let speech = list["speech"].as_str().unwrap();
    assert!(speech.contains("5.") && !speech.contains("mobile-login-6"));
    assert!(phone.say("Show me that agent").await["path"]
        .as_str()
        .unwrap()
        .contains("agent-3"));
    phone.backend.0.lock().unwrap().agents.reverse();
    assert!(phone.say("Show me the second one").await["path"]
        .as_str()
        .unwrap()
        .contains("agent-2"));
    let more = phone.say("more").await;
    assert!(more["speech"].as_str().unwrap().contains("mobile-login-6"));
    phone
        .backend
        .0
        .lock()
        .unwrap()
        .agents
        .iter_mut()
        .find(|a| a["run"] == "agent-6")
        .unwrap()["ref"]["session_id"] = json!("replacement");
    assert!(
        phone.say("Tell the first one to run the tests").await["speech"]
            .as_str()
            .unwrap()
            .contains("select an agent again")
    );
    assert!(phone.backend.0.lock().unwrap().sends.is_empty());
}

#[tokio::test(start_paused = true)]
async fn siri_provider_constrains_search_and_descriptive_choice() {
    let mut phone = Phone::new();
    {
        let mut live = phone.backend.0.lock().unwrap();
        let mut claude = row(1);
        claude["title"] = json!("Fix cursor movement");
        live.agents = vec![claude];
    }
    let command = "Tell the Cursor agent to run tests";
    let missing = phone.say(command).await;
    assert!(missing["speech"]
        .as_str()
        .unwrap()
        .contains("No live agents matched"));
    assert!(phone.backend.0.lock().unwrap().sends.is_empty());

    let mut cursor = row(2);
    cursor["ref"]["kind"] = json!("cursor");
    phone.backend.0.lock().unwrap().agents.push(cursor.clone());
    phone.say(command).await;
    assert_eq!(phone.backend.0.lock().unwrap().sends.len(), 1);
    let list = phone.say("Tell the agent in swamp to run tests").await;
    assert!(list["speech"].as_str().unwrap().contains("Which one?"));
    phone.say("the Cursor agent").await;
    let live = phone.backend.0.lock().unwrap();
    assert_eq!(live.sends.len(), 2);
    for send in &live.sends {
        assert_eq!(send["agent"], cursor["ref"]);
        assert_eq!(send["text"], "run tests");
    }
}

#[test]
fn siri_matches_registered_provider_names_and_an_unavailable_opencode() {
    use yinz_types::agent::provider::{with_snapshot, AgentProvider};

    let custom = AgentProvider {
        kind: "cursor-tools".into(),
        executable: "cursor-tools".into(),
        ..Default::default()
    };
    with_snapshot(&[custom], || {
        let mut agent: Agent = serde_json::from_value(row(1)).unwrap();
        agent.title = "OpenCode cursor tools".into();
        for (target, kind) in [
            ("the Cursor Tools agent", "cursor-tools"),
            ("the OpenCode agent", "opencode"),
        ] {
            let matches = matching_target(target);
            agent.identity.kind = "cursor".into();
            assert!(!matches(&agent));
            agent.identity.kind = kind.into();
            assert!(matches(&agent));
        }
    });
}

#[tokio::test]
async fn siri_ambiguous_send_replays_once_and_reads_attributed_turn() {
    let mut phone = Phone::new();
    phone.backend.0.lock().unwrap().reply_on_send = true;
    let ask = phone
        .say("Tell the login agent in swamp to Run the Tests!")
        .await;
    assert!(ask["speech"].as_str().unwrap().contains("Which one?"));
    let id = phone.cursor.clone();
    let selection = "the one on mobile login 2";
    let reply = phone.say(selection).await;
    assert_eq!(reply["speech"], "The requested tests passed.");
    assert_eq!(phone.send(&id, selection).await, reply);
    assert_eq!(
        phone.say("Did it reply?").await["speech"],
        "The requested tests passed."
    );
    let live = phone.backend.0.lock().unwrap();
    assert_eq!(live.sends.len(), 1);
    assert_eq!(live.sends[0]["text"], "Run the Tests!");
    assert_eq!(live.sends[0]["agent"]["session_id"], "agent-2");
}

#[tokio::test(start_paused = true)]
async fn siri_readback_only_lists_agents_until_the_run_is_known() {
    for known_run in [true, false] {
        let mut phone = Phone::new();
        let after = if known_run { 8 } else { 0 };
        {
            let mut live = phone.backend.0.lock().unwrap();
            live.agents[0]["run"] = if known_run {
                json!("run-1")
            } else {
                Value::Null
            };
            if known_run {
                live.replies
                    .push(json!({"revision":8,"run":"run-1","text":"Previous reply"}));
            }
            live.on_readback = Some(|live, path| {
                let count = live
                    .readback_requests()
                    .iter()
                    .filter(|(p, _)| p == path)
                    .count();
                match (path, count) {
                    ("/api/agents", 2) => live.agents[0]["run"] = json!("run-1"),
                    ("/api/agent/assistant-reply", 5) => {
                        live.replies.push(json!({"revision":9,"run":"run-1","turn":"our turn","text":"Finished!"}));
                    },
                    _ => {},
                }
            });
        }

        let reply = phone
            .say("Tell the agent on mobile login 1 to run tests")
            .await;
        assert_eq!(reply["speech"], "Finished!");
        assert_eq!(phone.say("Did it reply?").await["speech"], "Finished!");

        let live = phone.backend.0.lock().unwrap();
        let mut requests = live.readback_requests().iter();
        if !known_run {
            for _ in 0..2 {
                assert_eq!(requests.next().unwrap().0, "/api/agents");
            }
        }
        let correlated = json!({"main_only":true,"after":after,"turn":"our turn"});
        let latest = json!({"main_only":true,"after":after});
        // Two misses, the attributed reply on the third poll, then the follow-up command.
        for filter in [
            &correlated,
            &latest,
            &correlated,
            &latest,
            &correlated,
            &correlated,
        ] {
            let (path, body) = requests.next().unwrap();
            assert_eq!(path, "/api/agent/assistant-reply");
            assert_eq!(
                body,
                &json!({"agent":row(1)["ref"],"run":"run-1","filter":filter})
            );
        }
        assert!(requests.next().is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn siri_readback_requests_reselection_when_the_run_changes() {
    let mut phone = Phone::new();
    {
        let mut live = phone.backend.0.lock().unwrap();
        live.on_readback = Some(|live, _| {
            live.agents[0]["run"] = json!("replacement");
        });
        live.replies.push(
            json!({"revision":1,"run":"replacement","turn":"our turn","text":"Replacement reply."}),
        );
    }
    assert_eq!(
        phone
            .say("Tell the agent on mobile login 1 to run tests")
            .await["speech"],
        CHANGED
    );
    let live = phone.backend.0.lock().unwrap();
    let [(path, body)] = live.readback_requests() else {
        panic!("expected a single guarded lookup")
    };
    assert_eq!(path, "/api/agent/assistant-reply");
    assert_eq!(
        body,
        &json!({"agent":row(1)["ref"],"run":"agent-1","filter":{"main_only":true,"after":0,"turn":"our turn"}})
    );
}

#[tokio::test(start_paused = true)]
async fn siri_follow_up_keeps_a_discovered_run_across_a_failed_lookup() {
    let mut phone = Phone::new();
    phone.backend.0.lock().unwrap().agents[0]["run"] = Value::Null;
    let sent = phone
        .say("Tell the agent on mobile login 1 to run tests")
        .await;
    assert!(sent["speech"].as_str().unwrap().contains("delivered"));
    {
        let mut live = phone.backend.0.lock().unwrap();
        live.agents[0]["run"] = json!("first-run");
        // The guarded endpoint returns an unreadable reply, so this command needs a retry.
        live.replies
            .push(json!({"revision":1,"run":"first-run","turn":"our turn","text":null}));
    }
    assert_eq!(
        phone.request(&phone.cursor, "Did it reply?").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let retry_start = {
        let mut live = phone.backend.0.lock().unwrap();
        live.agents[0]["run"] = json!("second-run");
        live.requests.len()
    };
    assert_eq!(phone.say("Did it reply?").await["speech"], CHANGED);
    let live = phone.backend.0.lock().unwrap();
    let [(path, body)] = &live.requests[retry_start..] else {
        panic!("expected one reply lookup on retry")
    };
    assert_eq!(path, "/api/agent/assistant-reply");
    assert_eq!(body["run"], "first-run");
}

#[tokio::test(start_paused = true)]
async fn siri_delayed_reply_and_uncertain_attribution() {
    let mut phone = Phone::new();
    let delivery = phone
        .say("Tell the agent on mobile login 1 to run tests")
        .await;
    assert!(delivery["speech"].as_str().unwrap().contains("delivered"));
    phone
        .backend
        .0
        .lock()
        .unwrap()
        .replies
        .push(json!({"revision":1,"run":"agent-1","text":"Concurrent update","turn":"other"}));
    let latest = phone.say("Did it reply?").await;
    assert!(latest["speech"]
        .as_str()
        .unwrap()
        .starts_with("The latest message"));
    phone
        .backend
        .0
        .lock()
        .unwrap()
        .replies
        .push(json!({"revision":2,"run":"agent-1","text":"Finished!","turn":"our turn"}));
    assert_eq!(phone.say("Did it reply?").await["speech"], "Finished!");
}

#[tokio::test]
async fn siri_listing_and_refinement_keep_the_right_conversation() {
    let mut phone = Phone::new();
    phone.say("Find agents on mobile login 1").await;
    phone.say("List agents on mobile login 2").await;
    assert!(phone.say("Show me that agent").await["path"]
        .as_str()
        .unwrap()
        .contains("agent-1"));
    phone.say("Find the missing agent").await;
    let found = phone.say("on mobile login 4").await;
    assert!(found["speech"].as_str().unwrap().contains("Selected"));
    assert!(phone.say("Show me that agent").await["path"]
        .as_str()
        .unwrap()
        .contains("agent-4"));
}

#[test]
fn siri_reading_keeps_paragraphs_and_unicode_intact() {
    let mut dialogue = Dialogue::default();
    let first = "雪".repeat(600);
    let second = "火".repeat(800);
    let spoken = reading(&mut dialogue, &format!("{first}\n\n{second}"));
    assert!(spoken.starts_with(&first));
    assert!(!spoken.contains('火'));
    assert_eq!(dialogue.reading, second);
    assert_eq!(
        reading(&mut dialogue, &second),
        format!("{} Say continue to hear more.", "火".repeat(700))
    );
    assert_eq!(dialogue.reading, "火".repeat(100));
    let exact = format!(" {}", "雪".repeat(699));
    assert_eq!(reading(&mut dialogue, &exact), exact);
    let full = "雪".repeat(700);
    assert_eq!(reading(&mut dialogue, &format!("{full}   ")), full);
    assert!(dialogue.reading.is_empty());
    assert!(matches!(parse("read latest reply"), Command::Choice(_)));
    assert!(matches!(parse("what did say?"), Command::Choice(_)));
}

#[test]
fn siri_reading_does_not_stop_at_an_early_paragraph_break() {
    let mut dialogue = Dialogue::default();
    let body = "word ".repeat(200);
    let spoken = reading(&mut dialogue, &format!("Done.\n\n{body}"));
    assert!(spoken.starts_with("Done.\n\nword word"), "{spoken}");
    assert!(spoken.ends_with(" Say continue to hear more."));
    assert!(!dialogue.reading.is_empty());
}

#[tokio::test]
async fn siri_continue_reads_every_word_in_bounded_chunks() {
    let mut phone = Phone::new();
    let words = (0..110).map(|i| format!("語句{i:04}")).collect::<Vec<_>>();
    let reply = format!("{}\n\n{}", words.join(" "), "火".repeat(800));
    phone
        .backend
        .0
        .lock()
        .unwrap()
        .replies
        .push(json!({"revision":1,"run":"agent-1","text":reply}));

    let mut command = "What did the agent on mobile login 1 say?";
    let chunks = [
        words[..100].join(" "),
        words[100..].join(" "),
        "火".repeat(700),
        "火".repeat(100),
    ];
    for (i, chunk) in chunks.iter().enumerate() {
        let response = phone.say(command).await;
        if i + 1 == chunks.len() {
            assert_eq!(response["speech"], *chunk);
            assert_eq!(response["follow_up"], "");
        } else {
            assert_eq!(
                response["speech"],
                format!("{chunk} Say continue to hear more.")
            );
            assert_eq!(response["follow_up"], "yes");
        }
        command = "continue";
    }
    assert_eq!(
        phone.say("continue").await["speech"],
        "There is nothing more to read."
    );
}

#[tokio::test]
async fn siri_recovers_persisted_sends_by_receipt_without_resending() {
    let command = "Tell the agent on mobile login 1 to run tests";
    for (receipt, expected) in [
        (
            json!({"outcome":"completed","acknowledged":true,"turn_id":"our turn"}),
            "The requested tests passed.",
        ),
        (
            json!({"outcome":"status_unavailable"}),
            "Delivery is uncertain",
        ),
        (json!({"outcome":"target_changed"}), "select an agent again"),
        (
            json!({"outcome":"rejected","prompt_delivery":"not_sent","message":"Agent declined."}),
            "Agent declined.",
        ),
    ] {
        let mut phone = Phone::new();
        {
            let mut live = phone.backend.0.lock().unwrap();
            live.lose_send_response = true;
            live.reply_on_send = true;
            live.lookup_unavailable = true;
            live.receipt = receipt;
        }
        assert_eq!(
            phone.request("first", command).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            phone.request("first", command).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        phone.backend.0.lock().unwrap().lookup_unavailable = false;
        let reply = phone.say(command).await;
        assert!(
            reply["speech"].as_str().unwrap().contains(expected),
            "{reply}"
        );
        assert_eq!(phone.send("first", command).await, reply);
        assert_eq!(phone.say("cancel").await["speech"], "Okay.");
        assert_eq!(phone.backend.0.lock().unwrap().sends.len(), 1);
    }
}

#[tokio::test]
async fn siri_limits_credential_lookups_before_store_access() {
    let clock = Arc::new(AtomicI64::new(SystemClock.now_ms()));
    let mut phone = Phone::with_clock(clock.clone());
    phone.say("Find agents on mobile login 1").await;
    clock.fetch_add(1000, Ordering::Release);
    let valid = phone
        .headers
        .insert(
            "authorization",
            "Bearer yzs1_00000000000000000000000000000000"
                .parse()
                .unwrap(),
        )
        .unwrap();
    for _ in 0..100 {
        assert_eq!(
            phone.request(&phone.cursor, "cancel").await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    // A lookup against this store would return 503.
    let path = phone.state.settings.data_dir.join("web/siri.json");
    let saved = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"invalid JSON").unwrap();
    let limited = phone.request(&phone.cursor, "cancel").await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers()["retry-after"], "1");
    std::fs::write(&path, saved).unwrap();

    phone.headers.insert("authorization", valid);
    clock.fetch_add(20, Ordering::Release);
    assert!(phone.say("Show me that agent").await["path"]
        .as_str()
        .unwrap()
        .contains("agent-1"));
}

#[tokio::test]
async fn siri_invalid_dictation_accepts_a_replacement_at_the_same_cursor() {
    let mut phone = Phone::new();
    let command = format!("Tell the agent on mobile login 1 to {}", "雪".repeat(2731));
    let response = phone.request(&phone.cursor, &command).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["error"]["code"], "invalid_dictation");
    assert!(body.get("request_id").is_none());
    assert!(phone.backend.0.lock().unwrap().requests.is_empty());
    assert!(
        phone.say("Select the agent on mobile login 2").await["speech"]
            .as_str()
            .unwrap()
            .starts_with("Selected")
    );
}

#[tokio::test]
async fn siri_successful_launch_abandons_unmatched_send() {
    for ready in [true, false] {
        let mut phone = Phone::new();
        if !ready {
            phone.backend.0.lock().unwrap().agents.remove(0);
        }
        let missing = phone.say("Tell the unicorn agent to run the tests").await;
        assert!(missing["speech"]
            .as_str()
            .unwrap()
            .contains("No live agents matched"));
        let launch = phone.say("Open a new Claude agent in swamp").await;
        if ready {
            assert_eq!(launch["speech"], "Agent launched and selected.");
        } else {
            assert!(launch["speech"].as_str().unwrap().contains("is starting"));
        }
        // A bare description used to resume the abandoned send here.
        phone.say("the one on mobile login 2").await;
        if !ready {
            let mut renamed = row(1);
            renamed["session"] = json!("renamed");
            phone.backend.0.lock().unwrap().agents.push(renamed);
            phone.backend.0.lock().unwrap().launch_replaced = true;
            assert_eq!(phone.say("Show me that agent").await["speech"], CHANGED);
            phone.backend.0.lock().unwrap().launch_replaced = false;
            assert!(phone.say("Show me that agent").await["path"]
                .as_str()
                .unwrap()
                .contains("agent-1"));
        }
        let selected = phone.say("Select the agent on mobile login 2").await;
        assert!(selected["speech"].as_str().unwrap().starts_with("Selected"));
        assert!(phone.backend.0.lock().unwrap().sends.is_empty());
    }
}

#[tokio::test]
async fn siri_launch_retries_pending_and_finishes_terminal_failure() {
    let command = "Launch a new Claude agent in swamp";
    let mut phone = Phone::new();
    phone.say("Tell the unicorn agent to run the tests").await;
    let launch_cursor = phone.cursor.clone();
    phone.backend.0.lock().unwrap().launch_error = Some((503, "unavailable"));
    assert_eq!(
        phone.request(&launch_cursor, command).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    phone.backend.0.lock().unwrap().launch_error = Some((502, "launch_failed"));
    let reply = phone.say(command).await;
    assert_eq!(reply["speech"], "Launch did not finish.");
    assert_eq!(phone.send(&launch_cursor, command).await, reply);
    phone.backend.0.lock().unwrap().reply_on_send = true;
    assert_eq!(
        phone.say("the one on mobile login 2").await["speech"],
        "The requested tests passed."
    );
    phone.backend.0.lock().unwrap().launch_error = Some((503, "unavailable"));
    assert_eq!(
        phone.request(&phone.cursor, command).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    phone.backend.0.lock().unwrap().launch_error = None;
    assert_eq!(
        phone.say(command).await["speech"],
        "Agent launched and selected."
    );
    let live = phone.backend.0.lock().unwrap();
    assert_eq!(live.launches.len(), 4);
    assert_eq!(live.launches[0], live.launches[1]);
    assert_eq!(live.launches[2], live.launches[3]);
    assert_ne!(
        live.launches[0]["submission_id"],
        live.launches[2]["submission_id"]
    );
}

#[tokio::test]
async fn siri_abandons_unknown_launch_after_replaying_the_same_receipt() {
    let mut phone = Phone::new();
    let command = "Launch a new Claude agent in swamp";
    phone.backend.0.lock().unwrap().launch_error = Some((503, "unavailable"));
    assert_eq!(
        phone.request("first", command).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    phone.backend.0.lock().unwrap().launch_error = Some((503, "launch_unknown"));
    let prompt = phone.say(command).await;
    assert_eq!(prompt["follow_up"], "yes");
    assert_eq!(phone.send("first", command).await, prompt);
    assert_eq!(phone.say("Check again.").await["speech"], CHECK_LAUNCH);
    assert!(phone.say("Cancel").await["speech"]
        .as_str()
        .unwrap()
        .contains("may still have launched"));
    assert!(
        phone.say("Select the agent on mobile login 2").await["speech"]
            .as_str()
            .unwrap()
            .starts_with("Selected")
    );
    let live = phone.backend.0.lock().unwrap();
    assert_eq!(live.launches.len(), 3);
    assert!(live.launches.windows(2).all(|pair| pair[0] == pair[1]));
}

#[tokio::test]
async fn siri_uncertain_send_retires_previous_reply() {
    let mut phone = Phone::new();
    phone.backend.0.lock().unwrap().reply_on_send = true;
    assert_eq!(
        phone
            .say("Tell the agent on mobile login 1 to run tests")
            .await["speech"],
        "The requested tests passed."
    );
    phone.backend.0.lock().unwrap().lose_send_response = true;
    phone.backend.0.lock().unwrap().receipt = json!({"outcome":"status_unavailable"});
    let next = "Tell the agent on mobile login 2 to run tests";
    assert_eq!(
        phone.request(&phone.cursor, next).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(phone.say(next).await["speech"]
        .as_str()
        .unwrap()
        .contains("uncertain"));
    assert_eq!(
        phone.say("Did it reply?").await["speech"],
        "There is no pending Siri message."
    );
}

#[test]
fn siri_undecodable_pairing_is_revoked_without_failing_others() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let verifier = password::tests::ready(data);
    let owner = sign_in(data, &verifier.verifier_id);
    let other = sign_in(data, &verifier.verifier_id);
    let code = store::configure(data, &owner.id, "listener", false)
        .unwrap()
        .unwrap();
    let token = store::redeem(data, "listener", &code).unwrap().unwrap();
    let path = data.join("web/siri.json");
    let mut rows: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut future = rows[0].clone();
    future["owner"] = json!(other.id);
    future["token_hash"] = json!(sessions::hash("future token"));
    future["dialogue"]["offset"] = json!("from another release");
    rows.push(future);
    password::atomic_write(
        &data.join("web"),
        "siri.json",
        &serde_json::to_vec(&rows).unwrap(),
    )
    .unwrap();
    let phone = store::load(data, "listener", &sessions::hash(&token))
        .unwrap()
        .unwrap();
    assert_eq!(phone.session.id, owner.id);
    assert!(
        store::load(data, "listener", &sessions::hash("future token"))
            .unwrap()
            .is_none()
    );
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(store::configure(data, &other.id, "listener", false)
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn siri_phrase_search_keeps_matches_when_another_candidate_changes() {
    let mut phone = Phone::new();
    {
        let mut live = phone.backend.0.lock().unwrap();
        live.stale_run = Some("agent-2".into());
        live.replies
            .push(json!({"revision":1,"run":"agent-1","text":"needle"}));
    }
    assert!(phone.say("List agents mentioned needle").await["speech"]
        .as_str()
        .unwrap()
        .contains("mobile-login-1"));
}

#[test]
fn siri_credential_reset_recovers_corrupt_pairing_store() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    password::tests::ready(data);
    for clear in [false, true] {
        password::atomic_write(&data.join("web"), "siri.json", b"{").unwrap();
        if clear {
            assert!(password::clear(data).unwrap());
            assert!(!data.join("web/siri.json").exists());
            password::tests::ready(data);
        } else {
            password::revoke_sessions(data).unwrap();
        }
        let password::PasswordState::Ready(v) = password::read_state(data) else {
            panic!("missing verifier")
        };
        let owner = sign_in(data, &v.verifier_id);
        let code = store::configure(data, &owner.id, "listener", false)
            .unwrap()
            .unwrap();
        assert!(store::redeem(data, "listener", &code).unwrap().is_some());
    }
}
