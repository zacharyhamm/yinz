use super::*;
use crate::{
    auth::{Auth, AuthConfig, Clock, SystemClock},
    Backend, DispatchFuture, WebSettings,
};
use http_body_util::BodyExt;
use std::sync::{Arc, Mutex};
use yinz_types::web::{Site, WebResponse};

#[derive(Default)]
struct Live {
    agents: Vec<Value>,
    sends: Vec<Value>,
    replies: Vec<Value>,
    reply_on_send: bool,
}

#[derive(Default)]
struct Fixture(Mutex<Live>);

impl Backend for Fixture {
    fn dispatch(&self, req: WebRequest) -> DispatchFuture {
        let mut live = self.0.lock().unwrap();
        let body: Value = serde_json::from_str(req.body.as_deref().unwrap_or("null")).unwrap();
        let value = match req.path.as_str() {
            "/api/agents" => json!({"agents":live.agents}),
            "/api/agent/assistant-reply" => {
                let filter = &body["filter"];
                let reply = live.replies.iter().rev().find(|r| {
                    r["run"] == body["run"]
                        && (filter["turn"].is_null() || filter["turn"] == r["turn"])
                        && r["revision"].as_i64().unwrap() > filter["after"].as_i64().unwrap_or(0)
                });
                json!({"reply":reply})
            },
            "/api/agent/prompt" => {
                live.sends.push(body.clone());
                if live.reply_on_send {
                    let id = &body["agent"]["session_id"];
                    live.replies.push(json!({"revision":2,"run":id,"text":"The requested tests passed.","turn":"our turn"}));
                    live.replies.push(json!({"revision":3,"run":id,"text":"A concurrent reply.","turn":"other turn"}));
                }
                json!({"response":{"submission_id":body["submission_id"],"outcome":"submitted","turn_id":"our turn"}})
            },
            path => panic!("unexpected Siri operation {path}"),
        };
        Box::pin(async move {
            Some(WebReply::Response(WebResponse {
                status: 200,
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
        "session":format!("session-{i}"),"state":"waiting","can_prompt":true})
}

struct Phone {
    _directory: tempfile::TempDir,
    backend: Arc<Fixture>,
    state: State,
    headers: HeaderMap,
    cursor: String,
}
impl Phone {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let verifier = password::tests::ready(dir.path());
        let info = {
            let _lock = password::lock(&dir.path().join("web")).unwrap();
            sessions::Store::default()
                .issue(
                    dir.path(),
                    &verifier.verifier_id,
                    "phone".into(),
                    true,
                    1,
                    SystemClock.now_ms(),
                )
                .unwrap()
                .1
        };
        let code = store::configure(dir.path(), &info.id, "server", false)
            .unwrap()
            .unwrap();
        assert!(store::redeem(dir.path(), "another server", &code)
            .unwrap()
            .is_none());
        let token = store::redeem(dir.path(), "server", &code).unwrap().unwrap();
        let backend = Arc::new(Fixture::default());
        backend.0.lock().unwrap().agents = (1..=7).map(row).collect();
        let state = State {
            settings: WebSettings::from_config(&Default::default(), dir.path()).unwrap(),
            auth: Arc::new(Auth::new(
                dir.path().into(),
                AuthConfig::default(),
                Arc::new(SystemClock),
            )),
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
    async fn send(&self, id: &str, command: &str) -> Value {
        let body = json!({"request_id":id,"command":command}).to_string();
        let response = handle(
            &self.state,
            "/api/siri/command",
            None,
            &self.headers,
            body.as_bytes(),
            i64::MAX,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
    }
    async fn say(&mut self, command: &str) -> Value {
        let response = self.send(&self.cursor, command).await;
        self.cursor = response["request_id"].as_str().unwrap().into();
        response
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
    let second = "火".repeat(200);
    let spoken = reading(&mut dialogue, &format!("{first}\n\n{second}"));
    assert!(spoken.starts_with(&first));
    assert!(!spoken.contains('火'));
    assert_eq!(dialogue.reading, second);
    assert!(matches!(parse("read latest reply"), Command::Choice(_)));
    assert!(matches!(parse("what did say?"), Command::Choice(_)));
}
