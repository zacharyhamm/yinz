//! Private native palette queries. Values are literal; free text is fuzzy matched by the widget.
use std::collections::BTreeMap;

pub const LIVE_FIELDS: &[&str] = &[
    "state",
    "provider",
    "source",
    "project",
    "session",
    "host",
    "target",
    "title",
    "directory",
    "pane",
    "agent-session",
    "run",
    "detail",
    "waiting",
];
pub const ARCHIVE_FIELDS: &[&str] = &["provider", "project", "session", "host", "run", "since"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentQuery {
    pub text: String,
    pub filters: BTreeMap<String, String>,
}

impl AgentQuery {
    pub fn parse(input: &str, fields: &[&str]) -> Result<Self, String> {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut quoted = false;
        let mut escaped = false;
        // Only a bare key and colon introduce a filter; quotes/escapes make text literal.
        let mut unquoted_key = true;
        let mut separator = None;
        for c in input.chars() {
            if escaped {
                word.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
                unquoted_key = false;
            } else if c == '"' {
                quoted = !quoted;
                unquoted_key = false;
            } else if c.is_whitespace() && !quoted {
                if !word.is_empty() {
                    words.push((std::mem::take(&mut word), separator.take()));
                }
                unquoted_key = true;
            } else {
                if c == ':' && unquoted_key {
                    separator = Some(word.len());
                    unquoted_key = false;
                }
                word.push(c);
            }
        }
        if quoted || escaped {
            return Err("Unclosed quote or escape in filter".into());
        }
        if !word.is_empty() {
            words.push((word, separator));
        }
        let mut query = Self::default();
        let mut text = Vec::new();
        for (word, separator) in words {
            if let Some((key, value)) = separator
                .map(|index| (&word[..index], &word[index + 1..]))
                .filter(|(key, _)| fields.contains(key))
            {
                if value.is_empty() {
                    return Err(format!("{key} requires a value"));
                }
                if key == "state" && !["busy", "idle", "waiting", "exited"].contains(&value) {
                    return Err("state must be busy, idle, waiting, or exited".into());
                }
                if key == "source"
                    && !["spawned", "registered", "reported", "detected"].contains(&value)
                {
                    return Err("source must be spawned, registered, reported, or detected".into());
                }
                if key == "since" && value.parse::<i64>().is_err() {
                    return Err("since requires epoch milliseconds".into());
                }
                if query.filters.insert(key.into(), value.into()).is_some() {
                    return Err(format!("Duplicate {key} filter"));
                }
            } else {
                text.push(word);
            }
        }
        query.text = text.join(" ");
        Ok(query)
    }

    pub fn matches(&self, metadata: &BTreeMap<String, String>) -> bool {
        self.filters.iter().all(|(key, value)| {
            metadata.get(key).is_some_and(|actual| {
                if ["title", "directory", "detail", "waiting"].contains(&key.as_str()) {
                    actual.to_lowercase().contains(&value.to_lowercase())
                } else {
                    actual.eq_ignore_ascii_case(value)
                }
            })
        })
    }

    pub fn tokens(&self) -> String {
        std::iter::once(&self.text)
            .filter(|text| !text.is_empty())
            .map(|text| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")))
            .chain(
                self.filters.iter().map(|(k, v)| {
                    format!("{k}:\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
                }),
            )
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
#[path = "unit/palette_query.rs"]
mod tests;
