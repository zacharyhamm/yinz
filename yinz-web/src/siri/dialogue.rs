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
    #[serde(default)]
    pub current_session: bool,
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

pub(super) fn matching_target(target: &str) -> impl Fn(&Agent) -> bool {
    let target = normalize(target);
    // OpenCode conversations can outlive the plugin that offers new launches.
    let offered = yinz_runtime::agent::launch::offered();
    let mut providers: Vec<_> = offered.iter().map(|p| normalize(p)).collect();
    providers.push("opencode".into());
    providers.retain(|name| !name.is_empty());
    providers.sort_by_key(|name| std::cmp::Reverse(name.len()));
    move |agent| matches(agent, &target, &providers)
}

/// Consume a whole normalized name, including names that span several spoken words.
fn after_name<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(name)?;
    if rest.is_empty() {
        Some(rest)
    } else {
        rest.strip_prefix(' ')
    }
}

fn matches(agent: &Agent, mut target: &str, providers: &[String]) -> bool {
    let title = normalize(&agent.title);
    let kind = normalize(&agent.identity.kind);
    let state = normalize(agent.state.as_deref().unwrap_or(""));
    let project = normalize(&agent.project);
    let session = normalize(&agent.session);
    let branch = normalize(&agent.branch);
    while !target.is_empty() {
        let (word, mut rest) = target.split_once(' ').unwrap_or((target, ""));
        let locations: &[&str] = match word {
            "in" if let Some(name) = after_name(rest, "session") => {
                rest = name;
                &[&session]
            },
            "in" if let Some(name) = after_name(rest, "project") => {
                rest = name;
                &[&project]
            },
            "in" => &[&project, &session],
            "session" => &[&session],
            "project" => &[&project],
            "provider" => &[&kind],
            "status" => &[&state],
            "on" if let Some(name) = after_name(rest, "branch") => {
                rest = name;
                &[&branch]
            },
            "on" | "branch" => &[&branch],
            "from" => &[&session, &project],
            _ => &[],
        };
        if !locations.is_empty() {
            // "in the swamp project" puts an article before the name and a noun after it.
            let article = ["the ", "a "]
                .into_iter()
                .find_map(|article| rest.strip_prefix(article));
            let Some(remaining) = locations
                .iter()
                .filter(|name| !name.is_empty())
                .flat_map(|name| {
                    [Some(rest), article]
                        .into_iter()
                        .flatten()
                        .filter_map(move |text| after_name(text, name))
                })
                .min_by_key(|s| s.len())
            else {
                return false;
            };
            if let Some(after_noun) = ["project", "session", "branch", "worktree"]
                .into_iter()
                .find_map(|noun| after_name(remaining, noun))
                && matches(agent, after_noun, providers)
            {
                return true;
            }
            target = remaining;
            continue;
        }
        if let Some((provider, remaining)) = providers
            .iter()
            .find_map(|name| after_name(target, name).map(|remaining| (name, remaining)))
        {
            if provider != &kind {
                return false;
            }
            target = remaining;
            continue;
        }
        target = rest;
        match word {
            "a" | "an" | "all" | "the" | "that" | "agent" | "agents" | "one" | "it" | "and"
            | "which" | "who" => {},
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
    #[serde(default)]
    pub refinement_until_ms: i64,
    pub reading: String,
    pub pending: Option<Pending>,
    pub uncertain_launch: Option<Value>,
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
    pub pane: String,
    pub kind: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Prepared {
    pub utterance: String,
    pub action: Mutation,
    /// When the command was checkpointed, so a recovery that keeps failing can be abandoned.
    #[serde(default)]
    pub since_ms: i64,
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
    const CONTENT_LIMIT: usize = 700;
    let mut paragraph = None;
    let mut whitespace = None;
    for (count, (offset, ch)) in text.char_indices().enumerate() {
        if ch.is_whitespace() {
            whitespace = Some(offset);
            if text[offset..].starts_with("\n\n") {
                paragraph = Some((offset, count));
            }
        }
        if count == CONTENT_LIMIT {
            // Prefer a paragraph break unless it would leave only a short opening line.
            let paragraph = paragraph
                .filter(|&(_, at)| at >= CONTENT_LIMIT / 2)
                .map(|(offset, _)| offset);
            let boundary = paragraph.or(whitespace).unwrap_or(offset);
            let chunk = text[..boundary].trim_end();
            let remaining = text[boundary..].trim_start();
            if chunk.is_empty() {
                return reading(d, remaining);
            }
            d.reading = remaining.into();
            return if remaining.is_empty() {
                chunk.to_owned()
            } else {
                format!("{chunk} Say continue to hear more.")
            };
        }
    }
    d.reading.clear();
    text.to_owned()
}
