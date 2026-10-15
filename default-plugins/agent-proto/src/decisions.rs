//! Typed requests for [OpenRouter Decisions](https://openrouter.ai/openapi.json).
//!
//! Configuration is independent of chat: `decisions_url` and `decisions_model` in
//! plugin KDL override `OPENROUTER_DECISIONS_URL` and `OPENROUTER_DECISIONS_MODEL`.
//! Defaults are [`DEFAULT_URL`] and [`DEFAULT_MODEL`]. `decisions_api_key_env`
//! names the credential variable (default `OPENROUTER_API_KEY`). Blank settings
//! are absent; missing credentials return `Ok(None)`. URLs are complete HTTP(S)
//! endpoints, never rewritten. OpenAI credentials are never used as a fallback.
//!
//! The caller owns HTTP scheduling and permissions. For a plugin subscribed to
//! `WebRequestResult` with `WebAccess` permission, prepare a host command for the caller:
//!
//! ```no_run
//! use std::collections::BTreeMap;
//! use agent_proto::decisions::{self, Endpoint, Input, Question, Request};
//! use yinz_types::data::{HttpVerb, PluginCommand};
//!
//! # fn prepare(kdl: &BTreeMap<String, String>, env: &BTreeMap<String, String>) -> Result<Option<PluginCommand>, String> {
//! if let Some(endpoint) = Endpoint::resolve(kdl, env)? {
//!     let request = Request {
//!         state: Input::String("The checkout button is broken".into()),
//!         questions: BTreeMap::from([("is_bug".into(), Question::Noul {
//!             instructions: Input::String("Does this report a defect?".into()),
//!             criteria: None,
//!         })]),
//!         user: None,
//!         session_id: None,
//!     };
//!     let body = request.body(&endpoint.model)?;
//!     let headers = endpoint.headers();
//!     return Ok(Some(PluginCommand::WebRequest(endpoint.url, HttpVerb::Post, headers, body,
//!         BTreeMap::from([("request_id".into(), "decision-1".into())]), None, None)));
//! }
//! # Ok(None)
//! # }
//! // In the matching WebRequestResult handler, pass its status and body:
//! # fn receive(status: u16, body: &[u8]) -> Result<(), String> {
//! let response = decisions::parse_response(status, body)?;
//! if let Some(decisions::Answer::Noul { noul }) = response.answers.get("is_bug") {
//!     println!("Defect likelihood: {noul}");
//! }
//! # Ok(())
//! # }
//! ```

use crate::llm::{api_error_message, non_empty};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const DEFAULT_URL: &str = "https://openrouter.ai/api/alpha/decisions";
pub const DEFAULT_MODEL: &str = "typesafe/jev-1.13";
pub const DEFAULT_API_KEY_ENV: &str = "OPENROUTER_API_KEY";

#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    pub model: String,
    pub api_key: String,
}

impl Endpoint {
    /// Resolve configuration, rejecting invalid URLs even without credentials.
    pub fn resolve(
        kdl: &BTreeMap<String, String>,
        env: &BTreeMap<String, String>,
    ) -> Result<Option<Self>, String> {
        let url = non_empty(kdl, "decisions_url")
            .or_else(|| non_empty(env, "OPENROUTER_DECISIONS_URL"))
            .unwrap_or_else(|| DEFAULT_URL.into());
        let parsed = url::Url::parse(&url).map_err(|e| format!("invalid decisions_url: {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !url
                .split_once("://")
                .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(parsed.scheme()))
        {
            return Err("decisions_url must be an absolute HTTP(S) endpoint".into());
        }
        let model = non_empty(kdl, "decisions_model")
            .or_else(|| non_empty(env, "OPENROUTER_DECISIONS_MODEL"))
            .unwrap_or_else(|| DEFAULT_MODEL.into());
        let key_env =
            non_empty(kdl, "decisions_api_key_env").unwrap_or_else(|| DEFAULT_API_KEY_ENV.into());
        Ok(non_empty(env, &key_env).map(|api_key| Self {
            url,
            model,
            api_key,
        }))
    }

    pub fn headers(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("Content-Type".into(), "application/json".into()),
            ("Accept".into(), "application/json".into()),
            ("Authorization".into(), format!("Bearer {}", self.api_key)),
        ])
    }
}

/// State or guidance: scalar numbers, booleans and null are not valid inputs.
/// Nested JSON values inside objects and arrays are unrestricted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Input {
    String(String),
    Object(Map<String, Value>),
    Array(Vec<Value>),
}

/// If Noul criteria are supplied, both sides are required by the API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub when_true: Input,
    #[serde(rename = "false")]
    pub when_false: Input,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Choice {
        instructions: Input,
        /// A null criterion lets the option name supply its meaning.
        criteria: BTreeMap<String, Option<Input>>,
    },
    Score {
        instructions: Input,
        /// Ordered from lowest to highest; must contain at least one criterion.
        criteria: Vec<Input>,
    },
    Noul {
        instructions: Input,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
}

/// A request before attaching the resolved endpoint's model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub state: Input,
    pub questions: BTreeMap<String, Question>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl Request {
    /// Validate and serialize the body to send with an HTTP POST.
    pub fn body(&self, model: &str) -> Result<Vec<u8>, String> {
        if model.trim().is_empty() {
            return Err("a decisions request needs a model".into());
        }
        if self.questions.is_empty() {
            return Err("a decisions request needs at least one question".into());
        }
        for (name, question) in &self.questions {
            if name.trim().is_empty() {
                return Err("a decision question needs a name".into());
            }
            match question {
                Question::Choice { criteria, .. } if criteria.is_empty() => {
                    return Err(format!("question {name:?} needs choice criteria"));
                },
                Question::Score { criteria, .. } if criteria.is_empty() => {
                    return Err(format!("question {name:?} needs score criteria"));
                },
                _ => {},
            }
        }
        for (name, value) in [("user", &self.user), ("session_id", &self.session_id)] {
            if value.as_ref().is_some_and(|v| v.chars().count() > 256) {
                return Err(format!("{name} must be at most 256 characters"));
            }
        }
        #[derive(Serialize)]
        struct Body<'a> {
            model: &'a str,
            #[serde(flatten)]
            request: &'a Request,
        }
        serde_json::to_vec(&Body {
            model,
            request: self,
        })
        .map_err(|e| format!("could not serialize decisions request: {e}"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Choice {
        choice: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        probabilities: Option<BTreeMap<String, f64>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confidence: Option<f64>,
    },
    Score {
        score: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        probabilities: Option<BTreeMap<String, f64>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confidence: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        legend: Option<BTreeMap<String, Input>>,
    },
    Noul {
        noul: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// Decode a buffered HTTP result. Missing required fields are errors, not defaults.
pub fn parse_response(status: u16, body: &[u8]) -> Result<Response, String> {
    if !(200..300).contains(&status) {
        return Err(api_error_message(status, body));
    }
    serde_json::from_slice(body).map_err(|e| format!("invalid decisions response: {e}"))
}

#[cfg(test)]
#[path = "unit/decisions.rs"]
mod tests;
