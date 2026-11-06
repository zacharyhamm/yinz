//! Recover only explicit queued human prompts after this observer's anchor.
use super::*;

pub(super) struct Prompts {
    hook: HookEvent,
    previous: Option<String>,
    seen: HashSet<String>,
}

impl Prompts {
    pub(super) fn new(hook: &HookEvent) -> Self {
        Self {
            hook: hook.clone(),
            previous: None,
            seen: HashSet::new(),
        }
    }

    pub(super) fn record(&mut self, record: &Value) -> Result<Option<HookEvent>> {
        if record["type"] != "user" {
            return Ok(None);
        }
        let queued = record["promptSource"] == "queued";
        let reason = if self.hook.session_id.is_empty()
            || record["sessionId"].as_str() != Some(&self.hook.session_id)
        {
            Some("conversation_mismatch")
        } else if record["isSidechain"] != false {
            Some("sidechain_or_unknown")
        } else if record.get("toolUseResult").is_some()
            || record["message"]["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["type"] == "tool_result"))
        {
            Some("tool_result")
        } else if record["isMeta"] == true
            || record["origin"]["kind"] == "task-notification"
            || record["turnOrigin"] == "task-notification"
        {
            Some("notification_or_meta")
        } else {
            None
        };
        let prompt = record["promptId"].as_str().filter(|id| {
            !id.trim().is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control)
        });
        let reason = reason.or_else(|| prompt.is_none().then_some("invalid_identity"));
        if let Some(reason) = reason {
            if queued {
                self.diagnostic(prompt, reason);
            }
            return Ok(None);
        }
        let prompt = prompt.context("validated Claude prompt identity missing")?;
        if self.seen.contains(prompt) {
            return Ok(None);
        }
        // Fail closed instead of forgetting replay tombstones in a long conversation.
        if self.seen.len() >= 4096 {
            return Err(anyhow!("Claude prompt observation exceeds 4096 prompts"));
        }
        self.seen.insert(prompt.into());
        if self.previous.is_none() {
            if self.hook.prompt_id.as_deref() == Some(prompt) && !self.hook.is_worker() {
                self.previous = Some(prompt.into());
            } else if queued {
                self.diagnostic(Some(prompt), "before_anchor");
            }
            return Ok(None);
        }
        let predecessor = self.previous.clone();
        if !queued {
            self.previous = Some(prompt.into());
            return Ok(None);
        }
        if record["origin"]["kind"] != "human" || record["turnOrigin"] != "human" {
            self.diagnostic(Some(prompt), "not_explicit_human");
            self.previous = Some(prompt.into());
            return Ok(None);
        }
        self.diagnostic(Some(prompt), "recovered");
        self.previous = Some(prompt.into());
        Ok(Some(HookEvent {
            hook_event_name: "UserPromptSubmit".into(),
            session_id: self.hook.session_id.clone(),
            prompt_id: Some(prompt.into()),
            transcript_path: self.hook.transcript_path.clone(),
            cwd: self.hook.cwd.clone(),
            claude_recovered_after: predecessor,
            // Neither text nor a fingerprint can prove which pending submission this
            // consumed. Do not inherit correlation fields from the anchor's hook.
            ..Default::default()
        }))
    }

    fn diagnostic(&self, prompt: Option<&str>, reason: &str) {
        let bounded = |id: &str| id.chars().take(128).collect::<String>();
        log::debug!(
            "Claude transcript recovery conversation={:?} prompt={:?} predecessor={:?} reason={reason}",
            bounded(&self.hook.session_id), bounded(prompt.unwrap_or_default()),
            bounded(self.previous.as_deref().unwrap_or_default()),
        );
    }
}
