use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Identity {
    pub id: String,
    pub kind: String,
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Agent {
    #[serde(rename = "ref")]
    pub identity: Identity,
    pub title: String,
    pub project: String,
    pub branch: String,
    pub session: String,
    pub state: Option<String>,
    pub run: Option<String>,
    pub pane_id: String,
    pub can_prompt: bool,
    #[serde(default)]
    pub hit: Option<Hit>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Hit {
    pub revision: i64,
    pub excerpt: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) enum Action {
    List,
    Select,
    Latest,
    Match,
    Show,
    Send(String),
}

pub(super) enum Command<'a> {
    Find(Action, &'a str, Option<&'a str>),
    Target(Action, &'a str),
    Launch(&'a str),
    Choice(&'a str),
    More,
    Continue,
    Pending,
    Cancel,
}

pub(super) fn parse(input: &str) -> Command<'_> {
    let input = input.trim();
    let text = input.trim_end_matches(['.', '?', '!']).trim();
    // ASCII folding preserves byte offsets into the original dictated message.
    let low = text.to_ascii_lowercase();
    if matches!(low.as_str(), "more" | "show more" | "next five") {
        return Command::More;
    }
    if matches!(low.as_str(), "continue" | "keep reading") {
        return Command::Continue;
    }
    if matches!(low.as_str(), "cancel" | "stop" | "never mind") {
        return Command::Cancel;
    }
    if matches!(low.as_str(), "did it reply" | "has it replied") {
        return Command::Pending;
    }
    if let Some(rest) = low.strip_prefix("tell ")
        && let Some((target, _)) = rest.split_once(" to ")
    {
        let end = 5 + target.len();
        return Command::Target(Action::Send(input[end + 4..].trim().into()), &text[5..end]);
    }
    if low.starts_with("open a new ")
        || low.starts_with("launch a new ")
        || low.starts_with("start a new ")
    {
        return Command::Launch(text);
    }
    for prefix in ["find ", "list ", "search for "] {
        if let Some(rest) = low.strip_prefix(prefix) {
            let action = if prefix == "list " {
                Action::List
            } else {
                Action::Select
            };
            for marker in [" mentioned ", " said "] {
                if let Some((target, phrase)) = rest.split_once(marker) {
                    return Command::Find(
                        action,
                        &text[prefix.len()..prefix.len() + target.len()],
                        Some(text[text.len() - phrase.len()..].trim_matches(['“', '”', '"'])),
                    );
                }
            }
            return Command::Find(action, &text[prefix.len()..], None);
        }
    }
    if low == "read that match" {
        return Command::Target(Action::Match, "it");
    }
    if text.len() > 13 && low.starts_with("what did ") && low.ends_with(" say") {
        return Command::Target(Action::Latest, &text[9..text.len() - 4]);
    }
    if text.len() > 18 && low.starts_with("read ") && low.ends_with(" latest reply") {
        return Command::Target(Action::Latest, &text[5..text.len() - 13]);
    }
    if low.starts_with("show me ") {
        return Command::Target(Action::Show, &text[8..]);
    }
    if low.starts_with("select ") {
        return Command::Target(Action::Select, &text[7..]);
    }
    Command::Choice(text)
}

pub(super) fn normalize(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn pronoun(s: &str) -> bool {
    matches!(
        normalize(s).as_str(),
        "it" | "its" | "that agent" | "this agent" | "the agent" | "that one"
    )
}

pub(super) fn ordinal(s: &str) -> Option<usize> {
    let normalized = normalize(s);
    let choice = normalized.strip_prefix("the ").unwrap_or(&normalized);
    match choice.strip_suffix(" one").unwrap_or(choice) {
        "one" | "first" | "1" => Some(0),
        "two" | "second" | "2" => Some(1),
        "three" | "third" | "3" => Some(2),
        "four" | "fourth" | "4" => Some(3),
        "five" | "fifth" | "5" => Some(4),
        _ => None,
    }
}

/// Consume explicit location constraints in order. A spoken branch or project can span words.
pub(super) fn matches(agent: &Agent, target: &str) -> bool {
    let target = normalize(target);
    let words: Vec<_> = target.split_whitespace().collect();
    let title = normalize(&agent.title);
    let kind = normalize(&agent.identity.kind);
    let state = normalize(agent.state.as_deref().unwrap_or(""));
    let project = normalize(&agent.project);
    let session = normalize(&agent.session);
    let branch = normalize(&agent.branch);
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        i += 1;
        let locations: &[&str] = match word {
            "in" if words.get(i) == Some(&"session") => {
                i += 1;
                &[&session]
            },
            "in" if words.get(i) == Some(&"project") => {
                i += 1;
                &[&project]
            },
            "in" => &[&project, &session],
            "session" => &[&session],
            "project" => &[&project],
            "provider" => &[&kind],
            "status" => &[&state],
            "on" if words.get(i) == Some(&"branch") => {
                i += 1;
                &[&branch]
            },
            "on" | "branch" => &[&branch],
            "from" => &[&session, &project],
            _ => &[],
        };
        if !locations.is_empty() {
            let consumed = locations
                .iter()
                .map(|s| s.split_whitespace().collect::<Vec<_>>())
                .filter(|name| !name.is_empty() && words[i..].starts_with(name))
                .map(|name| name.len())
                .max();
            let Some(count) = consumed else {
                return false;
            };
            i += count;
            continue;
        }
        match word {
            "a" | "an" | "all" | "the" | "that" | "agent" | "agents" | "one" | "it" | "and"
            | "which" | "who" => {},
            "claude" | "codex" | "pi" | "muse" | "agy" | "opencode" if word != kind => {
                return false
            },
            "waiting" | "busy" | "idle" | "running"
                if word != state && !(word == "running" && state == "busy") =>
            {
                return false
            },
            _ if word == kind || word == state || (word == "running" && state == "busy") => {},
            _ if title.split_whitespace().any(|t| t == word) => {},
            _ => return false,
        }
    }
    true
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Dialogue {
    pub selected: Option<Agent>,
    pub results: Vec<Agent>,
    pub offset: usize,
    pub choice: Option<Action>,
    pub refinement: Option<(Action, Option<String>)>,
    pub reading: String,
    pub pending: Option<Pending>,
    pub launched: Option<Launch>,
    pub prepared: Option<Prepared>,
    pub next: Option<String>,
    pub last: Option<(String, String, Value)>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Pending {
    pub agent: Agent,
    pub after: i64,
    pub turn: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Launch {
    pub id: String,
    pub session: String,
    pub pane: String,
    pub kind: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Prepared {
    pub utterance: String,
    pub action: Mutation,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) enum Mutation {
    Send {
        id: String,
        agent: Agent,
        text: String,
        after: i64,
    },
    Launch(Value),
    Readback,
}

pub(super) fn page(d: &Dialogue) -> &[Agent] {
    &d.results[d.offset..(d.offset + 5).min(d.results.len())]
}

pub(super) fn choices(d: &Dialogue) -> String {
    let rows = page(d)
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let detail = [&a.project, &a.branch, &a.session]
                .into_iter()
                .filter(|s| !s.is_empty())
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            let hit = a
                .hit
                .as_ref()
                .map(|h| format!(". {}", h.excerpt))
                .unwrap_or_default();
            format!("{}. {}, {}{hit}.", i + 1, a.title, detail)
        })
        .collect::<Vec<_>>()
        .join(" ");
    let more = if d.offset + 5 < d.results.len() {
        " You can also say more."
    } else {
        ""
    };
    format!("{} matches. {rows} Which one?{more}", d.results.len())
}

pub(super) fn reading(d: &mut Dialogue, text: &str) -> String {
    let end = text.char_indices().nth(700).map_or(text.len(), |(i, _)| i);
    if end == text.len() {
        d.reading.clear();
        return text.to_owned();
    }
    let boundary = text[..end]
        .rfind("\n\n")
        .filter(|i| *i > 0)
        .or_else(|| text[end..].find("\n\n").map(|i| end + i))
        .or_else(|| text[..end].rfind(char::is_whitespace).filter(|i| *i > 0))
        .unwrap_or(end);
    d.reading = text[boundary..].trim_start().into();
    format!("{} Say continue to hear more.", text[..boundary].trim_end())
}
