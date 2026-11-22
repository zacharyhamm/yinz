//! Voice commands compose the signed-in web app's live list, archive and guarded actions.
//! The shortcut can submit only a dictated command and the cursor for its next exchange.
mod dialogue;
mod store;

use crate::{
    auth::{password, sessions, SessionInfo},
    policy,
    service::{error, file_work, response, unauthenticated, Body, State},
};
use dialogue::*;
use http::{HeaderMap, Response, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::atomic::Ordering, time::Duration};
use yinz_types::web::{WebAuth, WebMethod, WebReply, WebRequest, MAX_RESPONSE_BODY};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    code: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dictation {
    request_id: String,
    command: String,
}

fn json_response(value: Value) -> Response<Body> {
    response(StatusCode::OK, value.to_string(), "application/json")
}
fn unavailable() -> Response<Body> {
    error(StatusCode::SERVICE_UNAVAILABLE, "siri_unavailable")
}
/// Unwrap a pairing-store operation, logging a store failure the client only sees as 503.
fn stored<T>(what: &str, result: std::result::Result<anyhow::Result<T>, ()>) -> Option<T> {
    match result {
        Ok(Ok(value)) => Some(value),
        Ok(Err(error)) => {
            log::warn!("Siri {what}: {error:#}");
            None
        },
        Err(()) => None,
    }
}

pub(crate) async fn handle(
    state: &State,
    listener_instance: &str,
    path: &str,
    session: Option<&SessionInfo>,
    headers: &HeaderMap,
    bytes: &[u8],
    deadline_ms: i64,
) -> Response<Body> {
    let dir = state.settings.data_dir.clone();
    let instance = listener_instance.to_owned();
    match path {
        "/api/siri/code" | "/api/siri/revoke" => {
            let Some(owner) = session.map(|s| s.id.clone()) else {
                return unauthenticated();
            };
            let revoke = path.ends_with("/revoke");
            let result = file_work(state, move || {
                store::configure(&dir, &owner, &instance, revoke)
            })
            .await;
            match stored("pairing setup", result) {
                Some(code) => json_response(json!({"code":code})),
                None => unavailable(),
            }
        },
        "/api/siri/pair" => {
            if let Err(limit) = state.auth.public() {
                return super::service::auth_response(crate::auth::AuthResponse::limited(limit));
            }
            let Ok(pair) = serde_json::from_slice::<Pair>(bytes) else {
                return error(StatusCode::BAD_REQUEST, "invalid_pairing");
            };
            let result = file_work(state, move || store::redeem(&dir, &instance, &pair.code)).await;
            match stored("pairing", result) {
                Some(Some(token)) => json_response(json!({"token":token,"request_id":"first"})),
                Some(None) => unauthenticated(),
                None => unavailable(),
            }
        },
        "/api/siri/command" => {
            let Some(token) = policy::single(headers, "authorization")
                .ok()
                .flatten()
                .and_then(|h| h.strip_prefix("Bearer "))
                .filter(|t| t.len() == 37 && t.starts_with("yzs1_"))
            else {
                return unauthenticated();
            };
            if let Err(limit) = state.auth.public() {
                return super::service::auth_response(crate::auth::AuthResponse::limited(limit));
            }
            let hash = sessions::hash(token);
            // A retry waits for the first request to publish its receipt. The durable
            // revision also fences requests racing revocation or a listener restart.
            // The hash is lowercase hex: bucket by the digit's value, not its ASCII byte.
            let bucket = usize::from(u8::from_str_radix(&hash[..1], 16).unwrap_or(0))
                % state.siri_commands.len();
            let _command = state.siri_commands[bucket].lock().await;
            let key = hash.clone();
            let result = file_work(state, move || store::load(&dir, &instance, &key)).await;
            let phone = match stored("credential lookup", result) {
                Some(Some(phone)) => phone,
                Some(None) => return unauthenticated(),
                None => return unavailable(),
            };
            if let Err(limit) = state.auth.touch(&phone.session) {
                return super::service::auth_response(crate::auth::AuthResponse::limited(limit));
            }
            let Ok(command) = serde_json::from_slice::<Dictation>(bytes) else {
                return error(StatusCode::BAD_REQUEST, "invalid_dictation");
            };
            if command.command.trim().is_empty()
                || command.command.len() > 8192
                || command.request_id.len() > 64
            {
                return error(StatusCode::BAD_REQUEST, "invalid_dictation");
            }
            let mut ctx = Context {
                state,
                listener_instance,
                hash,
                phone,
                deadline_ms,
            };
            ctx.command(command).await
        },
        _ => error(StatusCode::NOT_FOUND, "not_found"),
    }
}

enum Failure {
    Spoken(String),
    TargetChanged,
    UnknownLaunch,
    Retry,
}
type Result<T> = std::result::Result<T, Failure>;
fn spoken(text: &str) -> Failure {
    Failure::Spoken(text.into())
}
const CHECK_LAUNCH: &str = "The launch outcome is uncertain. Say “check again” or “cancel”.";
const CHANGED: &str = "That conversation ended or changed. Please select an agent again.";
/// How long Siri waits for the answer that refines an unmatched search.
const REFINEMENT_WINDOW_MS: i64 = 120_000;
struct Context<'a> {
    state: &'a State,
    listener_instance: &'a str,
    hash: String,
    phone: store::Phone,
    deadline_ms: i64,
}
impl Context<'_> {
    async fn save(&mut self) -> Result<()> {
        let dir = self.state.settings.data_dir.clone();
        let instance = self.listener_instance.to_owned();
        let hash = self.hash.clone();
        let rev = self.phone.revision;
        let dialogue = self.phone.dialogue.clone();
        let result = file_work(self.state, move || {
            store::save(&dir, &instance, &hash, rev, dialogue)
        })
        .await;
        stored("dialogue checkpoint", result).ok_or(Failure::Retry)?;
        self.phone.revision += 1;
        Ok(())
    }

    async fn api(
        &self,
        path: &str,
        query: Vec<(String, String)>,
        body: Option<Value>,
    ) -> Result<Value> {
        let request = WebRequest {
            id: self.state.request_id.fetch_add(1, Ordering::Relaxed),
            method: if body.is_some() {
                WebMethod::Post
            } else {
                WebMethod::Get
            },
            path: path.into(),
            segments: path
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            query,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.map(|v| v.to_string()),
            auth: WebAuth {
                session: self.phone.session.id.clone(),
                label: self.phone.session.label.clone(),
            },
            deadline_ms: self.deadline_ms,
        };
        let Some(WebReply::Response(reply)) = self.state.backend.dispatch(request).await else {
            return Err(Failure::Retry);
        };
        if reply.body.len() > MAX_RESPONSE_BODY {
            return Err(Failure::Retry);
        }
        let body: Value = serde_json::from_str(&reply.body).map_err(|_| Failure::Retry)?;
        match reply.status {
            200 => Ok(body),
            502 if path == "/api/agent/launch" && body["error"]["code"] == "launch_failed" => {
                Err(spoken(
                    body["error"]["message"]
                        .as_str()
                        .unwrap_or("The agent launch failed."),
                ))
            },
            503 if path == "/api/agent/launch" && body["error"]["code"] == "launch_unknown" => {
                Err(Failure::UnknownLaunch)
            },
            409 if body["error"]["code"] == "target_changed" => Err(Failure::TargetChanged),
            400 | 403 | 404 | 409 | 422 => Err(spoken(
                body["error"]["message"]
                    .as_str()
                    .unwrap_or("Yinz could not complete that request."),
            )),
            _ => Err(Failure::Retry),
        }
    }
    async fn agents(&self) -> Result<Vec<Agent>> {
        let rows = self.api("/api/agents", vec![], None).await?;
        let agents: Vec<Agent> =
            serde_json::from_value(rows["agents"].clone()).map_err(|_| Failure::Retry)?;
        Ok(agents
            .into_iter()
            .filter(|a| {
                a.can_prompt
                    && a.state.as_deref() != Some("exited")
                    && !a.identity.session_id.is_empty()
            })
            .collect())
    }
    async fn current(&self, agent: &Agent) -> Result<Agent> {
        let mut current = self
            .agents()
            .await?
            .into_iter()
            .find(|a| {
                a.identity == agent.identity
                    && a.session == agent.session
                    && (agent.run.is_none() || agent.run == a.run)
            })
            .ok_or_else(|| spoken(CHANGED))?;
        current.hit.clone_from(&agent.hit);
        Ok(current)
    }
    async fn reply(&self, agent: &Agent, mut filter: Value) -> Result<Option<Reply>> {
        let Some(run) = &agent.run else {
            return Ok(None);
        };
        filter["main_only"] = json!(true);
        let response = self
            .api(
                "/api/agent/assistant-reply",
                vec![],
                Some(json!({"agent":agent.identity,"run":run,"filter":filter})),
            )
            .await?;
        serde_json::from_value(response["reply"].clone()).map_err(|_| Failure::Retry)
    }
    async fn command(&mut self, cmd: Dictation) -> Response<Body> {
        let d = &self.phone.dialogue;
        if let Some((id, text, reply)) = &d.last
            && *id == cmd.request_id
            && *text == cmd.command
        {
            return json_response(reply.clone());
        }
        let cursor = d.next.as_deref().unwrap_or("first");
        if cmd.request_id != cursor
            || d.prepared
                .as_ref()
                .is_some_and(|p| p.utterance != cmd.command)
        {
            // Nothing ran. Hand the phone the live cursor and any in-flight command so
            // its next run finishes that command or dictates anew instead of replaying this one.
            let pending = d.prepared.as_ref().map_or("", |p| p.utterance.as_str());
            return response(
                StatusCode::CONFLICT,
                json!({"error":{"code":"stale_siri_request"},"cursor":cursor,"pending":pending})
                    .to_string(),
                "application/json",
            );
        }
        let result = if let Some(prepared) = &d.prepared {
            match prepared.action.clone() {
                Mutation::Send {
                    id, agent, after, ..
                } => self.recover_send(&id, agent, after).await,
                action => self.mutate(action).await,
            }
        } else {
            self.interpret(&cmd.command).await
        };
        let mut reply = match result {
            Ok(answer) => answer,
            Err(Failure::TargetChanged) => answer(CHANGED, false),
            Err(Failure::Spoken(speech)) => answer(speech, false),
            Err(Failure::Retry | Failure::UnknownLaunch) => return unavailable(),
        };
        let Ok(next) = password::random_id() else {
            return unavailable();
        };
        reply["request_id"] = json!(next);
        self.phone.dialogue.next = Some(next);
        self.phone.dialogue.prepared = None;
        self.phone.dialogue.last = Some((cmd.request_id, cmd.command, reply.clone()));
        if self.save().await.is_err() {
            return unavailable();
        }
        json_response(reply)
    }

    async fn interpret(&mut self, input: &str) -> Result<Value> {
        // An unmatched search can be refined only by the immediate, prompt answer to it.
        // Any other command, or a late reply, abandons it, including a pending send's text.
        let refinement = self
            .phone
            .dialogue
            .refinement
            .take()
            .filter(|_| self.state.auth.now_ms() <= self.phone.dialogue.refinement_until_ms);
        if let Some(body) = self.phone.dialogue.uncertain_launch.take() {
            if matches!(parse(input), Command::Cancel) {
                return Ok(answer(
                    "Stopped checking. The agent may still have launched.",
                    false,
                ));
            }
            if normalize(input) == "check again" {
                return self.prepare(input, Mutation::Launch(body)).await;
            }
            self.phone.dialogue.uncertain_launch = Some(body);
            return Ok(answer(CHECK_LAUNCH, true));
        }
        match parse(input) {
            Command::Cancel => {
                self.phone.dialogue.choice = None;
                self.phone.dialogue.reading.clear();
                Ok(answer("Okay.", false))
            },
            Command::More => {
                let d = &mut self.phone.dialogue;
                if d.offset + 5 >= d.results.len() {
                    return Ok(answer("There are no more matches.", false));
                }
                d.offset += 5;
                Ok(answer(choices(d), true))
            },
            Command::Continue => {
                let text = std::mem::take(&mut self.phone.dialogue.reading);
                if text.is_empty() {
                    return Ok(answer("There is nothing more to read.", false));
                }
                Ok(self.read(&text))
            },
            Command::Pending => self.check_pending().await,
            Command::Launch(text) => self.launch(input, text).await,
            Command::Find(action, target, phrase) => {
                self.search(input, action, target, phrase).await
            },
            Command::Target(action, target) => {
                if matches!(action, Action::Match)
                    && self.phone.dialogue.choice.is_some()
                    && page(&self.phone.dialogue).iter().any(|a| a.hit.is_some())
                {
                    self.phone.dialogue.choice = Some(action);
                    return Ok(answer(choices(&self.phone.dialogue), true));
                }
                self.phone.dialogue.choice = None;
                if let Some(index) = ordinal(target) {
                    let agent = page(&self.phone.dialogue)
                        .get(index)
                        .cloned()
                        .ok_or_else(|| spoken("Choose a number from the last list."))?;
                    return self.act(input, action, agent).await;
                }
                if pronoun(target) {
                    if self.phone.dialogue.launched.is_some() {
                        self.bind_launch().await?;
                    }
                    let agent = self
                        .phone
                        .dialogue
                        .selected
                        .clone()
                        .ok_or_else(|| spoken("Which agent? Find an agent first."))?;
                    return self.act(input, action, agent).await;
                }
                self.search(input, action, target, None).await
            },
            Command::Choice(text) => {
                if let Some((action, phrase)) = refinement {
                    return self.search(input, action, text, phrase.as_deref()).await;
                }
                let action = self.phone.dialogue.choice.clone().unwrap_or(Action::Select);
                let rows = page(&self.phone.dialogue);
                if rows.is_empty() {
                    return Ok(answer(
                        "Try find agents, tell an agent to do something, or open a new agent.",
                        false,
                    ));
                }
                let matches: Vec<_> = match ordinal(text) {
                    Some(i) => rows.get(i).into_iter().cloned().collect(),
                    None => {
                        let matches = matching_target(text);
                        rows.iter().filter(|a| matches(a)).cloned().collect()
                    },
                };
                if let [agent] = matches.as_slice() {
                    self.act(input, action, agent.clone()).await
                } else {
                    Ok(answer(
                        format!(
                            "Please choose from this list. {}",
                            choices(&self.phone.dialogue)
                        ),
                        true,
                    ))
                }
            },
        }
    }
    async fn search(
        &mut self,
        input: &str,
        action: Action,
        target: &str,
        phrase: Option<&str>,
    ) -> Result<Value> {
        let mut rows = Vec::new();
        if phrase.is_some_and(|q| q.trim().is_empty()) {
            return Ok(answer("What did the agent mention?", true));
        }
        for mut agent in self
            .agents()
            .await?
            .into_iter()
            .filter(matching_target(target))
        {
            if let Some(q) = phrase {
                let reply = match self.reply(&agent, json!({"query":q})).await {
                    Ok(Some(reply)) => reply,
                    Ok(None) | Err(Failure::TargetChanged) => continue,
                    Err(error) => return Err(error),
                };
                agent.hit = Some(Hit {
                    revision: reply.revision,
                    excerpt: excerpt(&reply.text, q),
                });
            }
            rows.push(agent);
        }
        let d = &mut self.phone.dialogue;
        d.refinement = rows
            .is_empty()
            .then(|| (action.clone(), phrase.map(str::to_owned)));
        d.refinement_until_ms = self.state.auth.now_ms() + REFINEMENT_WINDOW_MS;
        d.results = rows;
        d.offset = 0;
        d.choice = None;
        match d.results.as_slice() {
            [] => Ok(answer(
                format!(
                    "No live agents matched {target}{}. Try another title, project, or branch.",
                    phrase
                        .map(|s| format!(" mentioning {s}"))
                        .unwrap_or_default()
                ),
                true,
            )),
            [only] if matches!(action, Action::List) => Ok(answer(
                format!(
                    "One match. {}, {}, {}.",
                    only.title, only.branch, only.session
                ),
                false,
            )),
            [only] => {
                let agent = only.clone();
                self.act(input, action, agent).await
            },
            _ => {
                d.choice = Some(if matches!(action, Action::List) {
                    Action::Select
                } else {
                    action
                });
                Ok(answer(choices(d), true))
            },
        }
    }
    fn read(&mut self, text: &str) -> Value {
        let speech = reading(&mut self.phone.dialogue, text);
        answer(speech, !self.phone.dialogue.reading.is_empty())
    }
    async fn act(&mut self, input: &str, action: Action, agent: Agent) -> Result<Value> {
        let agent = self.current(&agent).await?;
        let d = &mut self.phone.dialogue;
        d.choice = None;
        d.launched = None;
        d.refinement = None;
        d.selected = Some(agent.clone());
        match action {
            Action::List | Action::Select => Ok(answer(
                format!(
                    "Selected {}. {}",
                    agent.title,
                    agent.hit.as_ref().map(|h| h.excerpt.as_str()).unwrap_or("")
                ),
                false,
            )),
            Action::Latest | Action::Match => {
                let filter = if matches!(action, Action::Match) {
                    let hit = agent
                        .hit
                        .as_ref()
                        .ok_or_else(|| spoken("Find an agent by something it mentioned first."))?;
                    json!({"revision":hit.revision})
                } else {
                    json!({})
                };
                let reply = self
                    .reply(&agent, filter)
                    .await?
                    .ok_or_else(|| spoken("There is no recorded reply for this request."))?;
                Ok(self.read(&reply.text))
            },
            Action::Show => {
                let encode = |s: &str| {
                    url::form_urlencoded::byte_serialize(s.as_bytes())
                        .collect::<String>()
                        .replace('+', "%20")
                };
                Ok(
                    json!({"speech":format!("Opening {}.",agent.title),"follow_up":"",
                    "path":format!("/#/a/{}?run={}&tab=timeline",encode(&agent.identity.id),encode(agent.run.as_deref().unwrap_or("")))}),
                )
            },
            Action::Send(text) => {
                let after = self
                    .reply(&agent, json!({}))
                    .await?
                    .map_or(0, |r| r.revision);
                let id = password::random_id().map_err(|_| Failure::Retry)?;
                self.prepare(
                    input,
                    Mutation::Send {
                        id,
                        agent,
                        text,
                        after,
                    },
                )
                .await
            },
        }
    }
    async fn recover_send(&mut self, id: &str, agent: Agent, after: i64) -> Result<Value> {
        self.phone.dialogue.pending = None;
        let result = self
            .api(
                "/api/agent/submission-status",
                vec![],
                Some(json!({"agent":agent.identity,"submission_id":id})),
            )
            .await?;
        self.finish_send(id, agent, after, &result["response"], true)
            .await
    }
    async fn finish_send(
        &mut self,
        id: &str,
        agent: Agent,
        after: i64,
        r: &Value,
        recovering: bool,
    ) -> Result<Value> {
        if r["submission_id"] != id {
            return Err(if recovering {
                spoken("Delivery is uncertain. Check the agent’s timeline.")
            } else {
                Failure::Retry
            });
        }
        match r["outcome"].as_str() {
            Some("submitted" | "command_sent") => {},
            _ if recovering && r["acknowledged"] == true => {},
            Some(
                "target_changed" | "target_closed" | "not_agent" | "target_not_found"
                | "target_exited",
            ) => return Err(spoken(CHANGED)),
            Some("rejected") if r["prompt_delivery"] != "started" => {
                return Err(spoken(
                    r["message"]
                        .as_str()
                        .unwrap_or("The agent declined the message."),
                ))
            },
            _ if r["prompt_delivery"] == "not_sent" => {
                return Err(spoken("Yinz could not deliver the message."))
            },
            _ => return Err(spoken("Delivery is uncertain. Check the agent’s timeline.")),
        }
        self.phone.dialogue.pending = Some(Pending {
            agent,
            after,
            turn: r["turn_id"].as_str().map(str::to_owned),
        });
        let prepared = self
            .phone
            .dialogue
            .prepared
            .as_mut()
            .ok_or(Failure::Retry)?;
        prepared.action = Mutation::Readback;
        self.save().await?;
        self.wait_for_reply().await
    }
    async fn prepare(&mut self, input: &str, mut action: Mutation) -> Result<Value> {
        if let Mutation::Send { agent, .. } = &mut action {
            // Verify the target before the checkpoint. A failure here has sent nothing,
            // so a retry interprets again instead of recovering an unsent submission.
            *agent = self.current(agent).await?;
            self.phone.dialogue.pending = None;
        }
        self.phone.dialogue.prepared = Some(Prepared {
            utterance: input.into(),
            action: action.clone(),
        });
        self.save().await?;
        self.mutate(action).await
    }
    async fn mutate(&mut self, action: Mutation) -> Result<Value> {
        match action {
            Mutation::Send {
                id,
                agent,
                text,
                after,
            } => {
                // `prepare` verified this target before persisting the submission.
                let result = self
                    .api(
                        "/api/agent/prompt",
                        vec![],
                        Some(json!({"agent":agent.identity,"submission_id":id,"text":text})),
                    )
                    .await?;
                self.finish_send(&id, agent, after, &result["response"], false)
                    .await
            },
            Mutation::Readback => self.wait_for_reply().await,
            Mutation::Launch(body) => {
                let result = match self
                    .api("/api/agent/launch", vec![], Some(body.clone()))
                    .await
                {
                    Err(Failure::UnknownLaunch) => {
                        self.phone.dialogue.uncertain_launch = Some(body);
                        return Ok(answer(CHECK_LAUNCH, true));
                    },
                    result => result?,
                };
                self.phone.dialogue.selected = None;
                self.phone.dialogue.choice = None;
                self.phone.dialogue.refinement = None;
                self.phone.dialogue.launched = Some(Launch {
                    id: field(&body, "submission_id")?,
                    kind: field(&body, "kind")?,
                    pane: field(&result, "pane_id")?,
                });
                match self.bind_launch().await {
                    Ok(()) => Ok(answer("Agent launched and selected.", false)),
                    Err(Failure::Retry) => Ok(answer(
                        "The agent launched. Say “tell it” once it is ready.",
                        false,
                    )),
                    Err(error) => Err(error),
                }
            },
        }
    }
    async fn launch(&mut self, input: &str, text: &str) -> Result<Value> {
        let normalized = normalize(text);
        let (_, rest) = normalized
            .split_once("a new ")
            .ok_or_else(|| spoken("Name a provider and project."))?;
        let (provider, location) = rest
            .split_once(" agent in ")
            .ok_or_else(|| spoken("Name a provider and project."))?;
        // "in a swamp worktree" drops the article, but a project may itself start with one.
        let article = ["a ", "the "]
            .into_iter()
            .find_map(|a| location.strip_prefix(a).map(|rest| (a, rest)));
        let location = article.map_or(location, |(_, rest)| rest);
        let (project, branch) = location
            .split_once(" on ")
            .map_or((location, None), |(p, b)| {
                (p, Some(b.trim_start_matches("branch ")))
            });
        let project = project.strip_suffix(" worktree").unwrap_or(project);
        let choices = self.api("/api/agent/launch-choices", vec![], None).await?;
        let exact = |key: &str, name: &str| -> Result<String> {
            let rows: Vec<_> = choices[key]
                .as_array()
                .ok_or(Failure::Retry)?
                .iter()
                .filter_map(Value::as_str)
                .filter(|s| normalize(s) == name)
                .collect();
            match rows.as_slice() {
                [s] => Ok((*s).into()),
                _ => Err(spoken(&format!(
                    "I couldn’t identify {name} among the available {key}."
                ))),
            }
        };
        let kind = exact("providers", provider)?;
        let project = match (exact("projects", project), article) {
            (Err(error), Some((article, _))) => {
                exact("projects", &format!("{article}{project}")).map_err(|_| error)?
            },
            (found, _) => found?,
        };
        let nonce = password::random_id().map_err(|_| Failure::Retry)?;
        let checkout = if let Some(branch) = branch {
            let c = self
                .api(
                    "/api/agent/launch-choices",
                    vec![("project".into(), project.clone())],
                    None,
                )
                .await?;
            let found: Vec<_> = c["worktrees"]
                .as_array()
                .ok_or(Failure::Retry)?
                .iter()
                .filter(|w| w["branch"].as_str().is_some_and(|s| normalize(s) == branch))
                .collect();
            let [w] = found.as_slice() else {
                return Err(spoken("Please choose a unique existing branch."));
            };
            json!({"type":"existing","path":field(w,"path")?})
        } else {
            json!({"type":"new","branch":format!("siri-{}-{}",chrono::Utc::now().format("%Y%m%d"),&nonce[..8])})
        };
        let body = json!({"submission_id":format!("{}.{}",field(&choices,"server_instance")?,nonce),"kind":kind,"project":project,"checkout":checkout});
        self.prepare(input, Mutation::Launch(body)).await
    }
    async fn bind_launch(&mut self) -> Result<()> {
        let l = self
            .phone
            .dialogue
            .launched
            .as_ref()
            .ok_or(Failure::Retry)?;
        let agent = self
            .agents()
            .await?
            .into_iter()
            .find(|a| a.current_session && a.pane_id == l.pane && a.identity.kind == l.kind)
            .ok_or_else(|| {
                spoken("The agent launched and is starting. If it asks for workspace approval, confirm in its pane, then try again.")
            })?;
        self.api(
            "/api/agent/launch-status",
            vec![],
            Some(json!({"submission_id":l.id,"agent":agent.identity})),
        )
        .await?;
        self.phone.dialogue.selected = Some(agent);
        self.phone.dialogue.launched = None;
        Ok(())
    }
    async fn wait_for_reply(&mut self) -> Result<Value> {
        let mut latest = None;
        let wait = async {
            loop {
                self.discover_pending_run().await?;
                if let Some((message, correlated)) = self.pending_reply().await? {
                    if correlated
                        || self
                            .phone
                            .dialogue
                            .pending
                            .as_ref()
                            .is_some_and(|p| p.turn.is_none())
                    {
                        return Ok(message);
                    }
                    latest = Some(message);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        };
        match tokio::time::timeout(Duration::from_secs(10), wait).await {
            Ok(Ok(message)) => Ok(self.read(&message)),
            Ok(Err(error @ (Failure::Spoken(_) | Failure::TargetChanged))) => Err(error),
            _ => match latest {
                Some(message) => Ok(self.read(&message)),
                None => Ok(answer(
                    "Message delivered. Ask “did it reply?” in a moment.",
                    false,
                )),
            },
        }
    }
    async fn discover_pending_run(&mut self) -> Result<bool> {
        if let Some(pending) = &self.phone.dialogue.pending
            && pending.agent.run.is_none()
        {
            let agent = self.current(&pending.agent).await?;
            if agent.run.is_none() {
                return Ok(false);
            }
            self.phone.dialogue.pending = Some(Pending {
                agent,
                after: pending.after,
                turn: pending.turn.clone(),
            });
            return Ok(true);
        }
        Ok(false)
    }
    async fn pending_reply(&self) -> Result<Option<(String, bool)>> {
        let p = self
            .phone
            .dialogue
            .pending
            .as_ref()
            .ok_or_else(|| spoken("There is no pending Siri message."))?;
        let agent = &p.agent;
        if let Some(turn) = &p.turn
            && let Some(r) = self
                .reply(agent, json!({"after":p.after,"turn":turn}))
                .await?
        {
            return Ok(Some((r.text, true)));
        }
        Ok(self.reply(agent, json!({"after":p.after})).await?.map(|r| {
            (
                format!("The latest message from {} is: {}", agent.title, r.text),
                false,
            )
        }))
    }
    async fn check_pending(&mut self) -> Result<Value> {
        if self.discover_pending_run().await? {
            // A failed reply lookup can be retried at this cursor. Keep it bound to this run.
            self.save().await?;
        }
        match self.pending_reply().await? {
            Some((text, _)) => Ok(self.read(&text)),
            None => Ok(answer("There is no new reply yet.", false)),
        }
    }
}
#[derive(Deserialize)]
struct Reply {
    revision: i64,
    text: String,
}
fn answer(speech: impl Into<String>, listen: bool) -> Value {
    json!({"speech":speech.into(),"follow_up":if listen { "yes" } else { "" }})
}
fn field(v: &Value, key: &str) -> Result<String> {
    v[key].as_str().map(str::to_string).ok_or(Failure::Retry)
}
fn excerpt(text: &str, query: &str) -> String {
    let pattern = regex::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build();
    let index = pattern
        .ok()
        .and_then(|r| r.find(text).map(|m| m.start()))
        .unwrap_or(0);
    let start = text[..index]
        .char_indices()
        .rev()
        .nth(40)
        .map_or(0, |(i, _)| i);
    text[start..]
        .chars()
        .take(130)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[path = "unit/siri.rs"]
mod tests;
