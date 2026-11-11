//! Pure web syntax and error boundaries; host authorization remains authoritative.
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use steel::{
    rvals::{Result, SteelVal},
    steel_vm::{engine::Engine, register_fn::RegisterFn},
};

const MAX_DIAGNOSTICS: usize = 64;
const MAX_MESSAGE: usize = 2048;
const MAX_DIAGNOSTIC: usize = 16 * 1024;
const TRUNCATED: &str = "\n[web diagnostics truncated or dropped]\n";

#[derive(Default)]
pub(crate) struct Diagnostics {
    errors: Vec<steel::SteelErr>,
    dropped: bool,
}

/// Copy only a bounded message and the failing span. No guest values or trace graphs survive.
/// The primary span is sufficient for file/line diagnosis; optional call stacks are omitted.
fn diagnostic_error(value: &SteelVal) -> steel::SteelErr {
    if let SteelVal::Custom(custom) = value {
        let guard = custom.read();
        if let Some(error) = steel::rvals::as_underlying_type::<steel::SteelErr>(guard.as_ref()) {
            // Steel exposes Display but keeps message() private. Cap writes while formatting
            // rather than allocating the original (potentially very large) message first.
            let mut message = BoundedMessage::default();
            let _ = std::fmt::write(&mut message, format_args!("{error}"));
            let mut bounded = steel::SteelErr::new(error.kind(), message.finish());
            if let Some(span) = error.span() {
                bounded = bounded.with_span(span);
            }
            return bounded;
        }
    }
    let message = match value {
        SteelVal::StringV(text) => text.as_str(),
        _ => "unavailable web error diagnostic",
    };
    err(clipped(message, MAX_MESSAGE))
}

#[derive(Default)]
struct BoundedMessage {
    text: String,
    truncated: bool,
}
impl std::fmt::Write for BoundedMessage {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let remaining = MAX_MESSAGE.saturating_sub(self.text.len());
        let mut end = text.len().min(remaining);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&text[..end]);
        self.truncated |= end != text.len();
        Ok(())
    }
}
impl BoundedMessage {
    fn finish(self) -> String {
        if self.truncated {
            clipped(&(self.text + TRUNCATED), MAX_MESSAGE)
        } else {
            self.text
        }
    }
}

fn clipped(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit.saturating_sub(TRUNCATED.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATED}", &text[..end])
}

impl Diagnostics {
    pub(crate) fn drain(
        &mut self,
        mut output: String,
        format: impl Fn(steel::SteelErr) -> String,
        limit: usize,
    ) -> String {
        let mut dropped = std::mem::take(&mut self.dropped);
        for error in std::mem::take(&mut self.errors) {
            // Put the useful message before any optional source excerpt is clipped.
            let message = error.to_string();
            let diagnostic = clipped(
                &format!("web handler: {message}\n{}\n", format(error)),
                MAX_DIAGNOSTIC,
            );
            if output.len().saturating_add(diagnostic.len())
                <= limit.saturating_sub(TRUNCATED.len())
            {
                output.push_str(&diagnostic);
            } else {
                dropped = true;
            }
        }
        if dropped {
            output = clipped(&output, limit.saturating_sub(TRUNCATED.len()));
            output.push_str(TRUNCATED);
        }
        output
    }
}
fn err(e: impl std::fmt::Display) -> steel::rerrs::SteelErr {
    crate::scheme_error(e)
}
fn validate(value: SteelVal) -> Result<SteelVal> {
    let response = crate::from_scheme(&value).map_err(err)?;
    let status = response["status"]
        .as_u64()
        .filter(|n| (200..=599).contains(n))
        .ok_or_else(|| err("invalid HTTP status"))?;
    let body = response["body"]
        .as_str()
        .ok_or_else(|| err("response body must be text"))?;
    if body.len() > 4 * 1024 * 1024 || (status == 304 && !body.is_empty()) {
        return Err(err("invalid HTTP body size"));
    }
    let headers = response["headers"]
        .as_object()
        .ok_or_else(|| err("headers must be a hash"))?;
    let mut normalized = serde_json::Map::new();
    for (name, value) in headers {
        let name = name.parse::<http::header::HeaderName>().map_err(err)?;
        let text = value
            .as_str()
            .ok_or_else(|| err("header value must be text"))?;
        http::HeaderValue::from_str(text).map_err(err)?;
        let name = name.as_str();
        if !(matches!(
            name,
            "content-type" | "etag" | "cache-control" | "vary" | "retry-after" | "location"
        ) || (name.starts_with("x-yinz-") && name.len() > 7))
        {
            return Err(err("disallowed header"));
        }
        if name == "location"
            && (!(300..400).contains(&status)
                || !crate::html::root_url(text)
                || text.parse::<http::Uri>().is_err())
        {
            return Err(err("unsafe redirect"));
        }
        if normalized.insert(name.to_owned(), value.clone()).is_some() {
            return Err(err("duplicate HTTP header"));
        }
    }
    if !body.is_empty() && !normalized.contains_key("content-type") {
        return Err(err("response body needs content-type"));
    }
    crate::to_scheme(&json!({"status":status,"headers":normalized,"body":body})).map_err(err)
}
pub(crate) fn register(engine: &mut Engine, diagnostics: Arc<Mutex<Diagnostics>>) {
    engine.register_fn("%web-diagnostic", move |value: SteelVal| {
        // Best effort only: diagnostic saturation must never fail a handled request.
        let mut records = diagnostics.lock().unwrap_or_else(|e| e.into_inner());
        if records.errors.len() < MAX_DIAGNOSTICS {
            records.errors.push(diagnostic_error(&value));
        } else {
            records.dropped = true;
        }
    });
    engine.register_fn(
        "yinz-web-agent-app",
        |_configuration: SteelVal| -> Result<SteelVal> {
            Err(err("standard web agent app is not installed"))
        },
    );
    engine.register_fn("%web-validate-response", validate);
    engine.register_fn("%web-raise", |message: String| -> Result<SteelVal> {
        Err(err(message))
    });
    engine.register_fn("%web-body-json", |request: SteelVal| -> Result<SteelVal> {
        let req = crate::from_scheme(&request).map_err(err)?;
        let media = req["headers"]["content-type"]
            .as_str()
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        let parsed = if media.eq_ignore_ascii_case("application/json") {
            req["body"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
        } else {
            None
        };
        match parsed {
            Some(value) => crate::to_scheme(&value).map_err(err),
            None => Err(err(
                "{\"yinz_web_error\":1,\"code\":\"bad_json\",\"status\":400}",
            )),
        }
    });
    engine.register_fn(
        "%web-public-path",
        |path: String, prefix: bool| -> Result<String> {
            if !path.starts_with('/')
                || path == "/api"
                || path.starts_with("/api/")
                || path.contains("//")
                || !path.bytes().all(|b| {
                    b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || b"/_-".contains(&b)
                        || (!prefix && b == b'.')
                })
                || (prefix && (path.len() <= 1 || !path.ends_with('/')))
                || (path != "/"
                    && path[1..]
                        .trim_end_matches('/')
                        .split('/')
                        .any(|s| matches!(s, "" | "." | "..")))
            {
                return Err(err("invalid public path"));
            }
            Ok(path)
        },
    );
    engine.register_fn("%web-split-path", |path: String| -> Result<SteelVal> {
        if !path.starts_with('/')
            || path.contains("//")
            || path.contains(['?', '#', '\\'])
            || path.chars().any(char::is_control)
        {
            return Err(err("invalid route path"));
        }
        let trimmed = path.strip_suffix('/').unwrap_or(&path);
        crate::to_scheme(&json!(trimmed
            .strip_prefix('/')
            .unwrap_or_default()
            .split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()))
        .map_err(err)
    });
    engine.register_fn(
        "%web-error-response",
        |failure: String| -> Result<SteelVal> {
            let expected = serde_json::from_str::<Value>(&failure)
                .ok()
                .filter(|v| v == &json!({"yinz_web_error":1,"code":"bad_json","status":400}));
            crate::to_scheme(&if expected.is_some() {
                json!({"status":400,"code":"bad_json"})
            } else {
                json!({"status":500,"code":"internal"})
            })
            .map_err(err)
        },
    );
}
