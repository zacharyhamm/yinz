use super::history::{
    cap_rows, labelled_blocks, run_header, wrap_width, RunFacts, CAPPED_MARKER, ENTRY_HEAD_BLOCKS,
    ENTRY_HEAD_BYTES, HEADER_LABEL_WIDTH, LABEL_STYLE,
};
use super::presentation::{awaiting_confirmation, compact_kind, PREVIEW_CHARS, PREVIEW_LINES};
use super::summaries::{SUMMARY_MIN_CHARS, SUMMARY_TEXT_CHARS, SUMMARY_TITLE_CHARS};
use super::*;
use crate::markdown::{MdLine, MdStyle};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::sync::Arc;
use unicode_width::UnicodeWidthStr;
use yinz_tile::prelude::HostId;
use yinz_types::internal_ids::DataRevision;
use yinz_types::plugin_data::REV_ABSENT;

/// The machine most of these tests are on. Named, rather than empty, so that a helper which
/// forgot to write `host_id` fails a test instead of matching the pre-migration rows.
const HOST: &str = "host-a";

/// An owner on [`HOST`]. The cross-machine cases spell `Owner::new` out.
fn owner(yinz_session_name: &str) -> rows::Owner {
    rows::Owner::new(HostId::new(HOST), yinz_session_name.into())
}

#[test]
fn a_registered_record_names_the_kind_and_nothing_it_does_not_know() {
    let record = registered_agent("reviewer", Some("/work".to_owned()));
    assert_eq!(record.kind, "reviewer");
    assert_eq!(record.source, AgentSource::Registered);
    assert_eq!(record.state, AgentState::Idle);
    assert_eq!(record.cwd.as_deref(), Some("/work"));
    assert!(record.detail.is_none() && record.session_id.is_none());
    assert!(
        HookFeed::from_agent(&record).is_none(),
        "a person's word about the kind is not a hook feed"
    );
}

/// A feed read off a record clears `waiting_on` on every phase that is not a wait, the
/// way a feed built from events does. A row that says `idle` beside a question, or a
/// `list_json` item carrying `"state":"idle"` and a `waiting_on` together, is a record
/// disagreeing with itself in front of whoever reads it.
#[test]
fn a_feed_off_a_record_only_waits_on_something_while_it_is_waiting() {
    let record = |state, waiting_on: Option<&str>| AgentInfo {
        kind: "claude".to_owned(),
        state,
        source: AgentSource::Reported,
        detail: None,
        waiting_on: waiting_on.map(str::to_owned),
        wait_needs_a_person: state == AgentState::Waiting,
        cwd: None,
        session_id: None,
        run_id: None,
    };

    let waiting = HookFeed::from_agent(&record(AgentState::Waiting, Some("Bash rm")))
        .expect("a wait is a feed");
    assert_eq!(waiting.waiting_on.as_deref(), Some("Bash rm"));

    let idle = HookFeed::from_agent(&record(AgentState::Idle, Some("Bash rm")))
        .expect("an idle agent is a feed too");
    assert_eq!(idle.phase, HookPhase::Idle);
    assert_eq!(idle.waiting_on, None);
}

fn pane(pane_id: u32, title: &str, command: Option<&str>) -> AgentPane {
    AgentPane {
        pane_id,
        title: title.to_owned(),
        command: command.map(str::to_owned),
        cwd: None,
        host: None,
        bubblewrap: None,
        status: AgentStatus::Running,
        tab: None,
        detected: None,
        agent: None,
        last_hook_at_ms: None,
    }
}

/// A pane that has been seen running `kind` and is now running something else —
/// an agent that shelled out.
fn latched(pane_id: u32, kind: &str, command: &str) -> AgentPane {
    AgentPane {
        detected: Some(kind.to_owned()),
        ..pane(pane_id, "zsh", Some(command))
    }
}

fn manual(pairs: &[(u32, &str)]) -> BTreeMap<u32, String> {
    pairs
        .iter()
        .map(|(id, kind)| (*id, (*kind).to_owned()))
        .collect()
}

fn owners(pairs: &[(u32, &str)]) -> BTreeMap<u32, String> {
    manual(pairs)
}

fn branches(pairs: &[(u32, &str)]) -> BTreeMap<u32, String> {
    manual(pairs)
}

fn project(name: &str) -> StripRow {
    StripRow::Project(name.to_owned())
}

fn branch(name: &str) -> StripRow {
    StripRow::Branch(format!("  {name}"))
}

/// The host a compact line names after its provider code, as `strip_rows` carries it.
fn host_of(text: &str) -> Option<String> {
    let (_, host) = text.split_whitespace().nth(1)?.split_once('@')?;
    Some(host.to_owned())
}

/// Full provider identity stays separate from compact list text.
fn agent(pane_id: u32, kind: &str, text: &str) -> StripRow {
    StripRow::Agent {
        pane_id,
        kind: kind.into(),
        text: text.into(),
        waiting: false,
        host: host_of(text),
    }
}

/// An agent row the model said is blocked on a confirmation.
fn waiting(pane_id: u32, kind: &str, text: &str) -> StripRow {
    StripRow::Agent {
        pane_id,
        kind: kind.into(),
        text: text.into(),
        waiting: true,
        host: host_of(text),
    }
}

/// A session where nothing reports for itself.
fn no_hooks() -> HookFeeds {
    HookFeeds::new()
}

/// One pane's feed, in the phase named.
fn feeds(pairs: &[(u32, HookPhase)]) -> HookFeeds {
    pairs
        .iter()
        .map(|(pane_id, phase)| {
            (
                *pane_id,
                HookFeed {
                    phase: *phase,
                    detail: None,
                    waiting_on: None,
                    session_id: "s1".to_owned(),
                    cwd: None,
                },
            )
        })
        .collect()
}

fn summaries(pairs: &[(u32, bool)]) -> Summaries {
    pairs
        .iter()
        .map(|(pane_id, awaiting)| {
            (
                *pane_id,
                Summary {
                    text: "did a thing".to_owned(),
                    title: "a thing".to_owned(),
                    awaiting: *awaiting,
                    hash: 1,
                    created_at_ms: None,
                },
            )
        })
        .collect()
}

/// The case that made the dash rule earn its place: an installer that puts a
/// versioned binary on the path, which no alias table could have anticipated.
#[test]
fn detects_a_versioned_binary_name() {
    assert_eq!(
        detect_kind(
            "zsh",
            Some("/Users/x/.local/bin/muse-bin-0.1.0-R708.1 --resume"),
        )
        .as_deref(),
        Some("muse"),
    );
}

#[test]
fn detects_exact_command_aliases() {
    for (command, kind) in [
        ("muse", "muse"),
        ("muse-cli", "muse"),
        ("codex", "codex"),
        ("codex-cli", "codex"),
        ("claude", "claude"),
        ("claude-code", "claude"),
        ("cursor", "cursor"),
        ("cursor-agent", "cursor"),
        ("qwen", "qwen"),
        ("qwen-code", "qwen"),
        ("gemini", "gemini"),
        ("gemini-cli", "gemini"),
        ("aider", "aider"),
        ("cline", "cline"),
        ("pi", "pi"),
        ("pi-agent", "pi"),
    ] {
        assert_eq!(
            detect_kind("shell", Some(command)).as_deref(),
            Some(kind),
            "{command}"
        );
    }
}

#[test]
fn detects_the_program_behind_a_path_and_its_arguments() {
    assert_eq!(
        detect_kind("shell", Some("/usr/local/bin/Codex --resume")).as_deref(),
        Some("codex"),
    );
}

#[test]
fn detects_a_kind_named_as_a_whole_word_in_the_title() {
    assert_eq!(
        detect_kind("aider — src/main.rs", None).as_deref(),
        Some("aider")
    );
    assert_eq!(detect_kind("CLAUDE", None).as_deref(), Some("claude"));
}

/// The whole reason detection is token-based rather than substring-based.
#[test]
fn does_not_detect_a_kind_inside_a_longer_word() {
    assert_eq!(detect_kind("pip install", Some("zsh")).as_deref(), None);
    assert_eq!(detect_kind("piano", None).as_deref(), None);
    assert_eq!(detect_kind("shell", Some("pipenv run pi")).as_deref(), None);
}

#[test]
fn detects_an_agent_behind_the_interpreter_that_runs_it() {
    assert_eq!(
        detect_kind("shell", Some("node /opt/bin/gemini")).as_deref(),
        Some("gemini"),
    );
    assert_eq!(
        detect_kind("shell", Some("python3 -m aider")).as_deref(),
        Some("aider")
    );
    assert_eq!(
        detect_kind("shell", Some("node /opt/lib/claude.js")).as_deref(),
        Some("claude"),
    );
}

/// The guard that keeps the interpreter case from swallowing every command line
/// that happens to say an agent's name.
#[test]
fn does_not_look_past_the_first_word_of_anything_else() {
    assert_eq!(
        detect_kind("shell", Some("git commit -m ask-claude")).as_deref(),
        None
    );
    assert_eq!(detect_kind("shell", Some("zsh")).as_deref(), None);
    assert_eq!(detect_kind("shell", Some("man codex")).as_deref(), None);
}

#[test]
fn rejects_a_kind_that_is_empty_too_long_or_not_a_plain_word() {
    assert!(valid_kind("reviewer"));
    assert!(valid_kind("agent_2-b"));
    assert!(!valid_kind(""));
    assert!(!valid_kind(&"a".repeat(33)));
    assert!(!valid_kind("Reviewer"));
    assert!(!valid_kind("code review"));
}

#[test]
fn a_manual_kind_wins_over_detection_and_survives_a_title_change() {
    let manual = manual(&[(3, "reviewer")]);
    assert_eq!(
        classify(&pane(3, "codex", Some("codex")), &manual),
        Some(("reviewer".to_owned(), "manual")),
    );
    assert_eq!(
        classify(&pane(3, "new title", Some("codex")), &manual),
        Some(("reviewer".to_owned(), "manual")),
    );
    assert_eq!(
        classify(&pane(4, "codex", Some("codex")), &manual),
        Some(("codex".to_owned(), "auto")),
    );
    assert_eq!(classify(&pane(5, "zsh", Some("zsh")), &manual), None);
}

/// The flicker the latch exists for: an agent that shells out is still an
/// agent, and the pane must not leave every list until the tool it ran
/// finishes.
#[test]
fn a_pane_that_shelled_out_keeps_the_kind_it_latched() {
    let manual = manual(&[]);
    assert_eq!(
        classify(&latched(3, "codex", "git status"), &manual),
        Some(("codex".to_owned(), "auto")),
    );
    // Even when what it is running is a shell of its own.
    assert_eq!(
        classify(&latched(3, "codex", "bash -c rg"), &manual),
        Some(("codex".to_owned(), "auto")),
    );
}

/// What a pane is doing now outranks what it was doing: a pane that has
/// become a different agent is reported as the one it is, and a manual kind
/// still beats both.
#[test]
fn the_latch_is_the_last_word_and_not_the_first() {
    let running = AgentPane {
        detected: Some("codex".to_owned()),
        ..pane(3, "zsh", Some("claude"))
    };
    assert_eq!(
        classify(&running, &manual(&[])),
        Some(("claude".to_owned(), "auto"))
    );
    assert_eq!(
        classify(&running, &manual(&[(3, "reviewer")])),
        Some(("reviewer".to_owned(), "manual")),
    );
}

/// Nothing rewrites a pane title when the program that set it exits, so a
/// pane that ran an agent and went back to its prompt used to stay an agent
/// for as long as it stayed open.
#[test]
fn a_shell_prompt_outranks_the_title_the_last_program_left() {
    assert_eq!(detect_kind("claude", Some("zsh")).as_deref(), None);
    assert_eq!(
        detect_kind("claude", Some("/usr/bin/zsh -l")).as_deref(),
        None
    );
    assert_eq!(detect_kind("claude", Some("-zsh")).as_deref(), None);
    // Still an agent while it is actually running one, and still detected by
    // title when nothing has said what it runs.
    assert_eq!(
        detect_kind("claude", Some("claude")).as_deref(),
        Some("claude")
    );
    assert_eq!(detect_kind("claude", None).as_deref(), Some("claude"));
    // A command that is neither a shell nor an agent leaves the title alone:
    // a wrapper we do not recognise must not hide a real agent.
    assert_eq!(
        detect_kind("claude", Some("mise exec")).as_deref(),
        Some("claude")
    );
}

/// opencode is detected like every other kind, which is what puts a pane a person
/// started by hand into the agents strip. From its command first - a path or a
/// bare binary, flags and all - then from its title when nothing has said what it runs,
/// and the latch survives a shell-out the way any other agent's does.
#[test]
fn an_opencode_pane_is_detected_from_its_command_and_its_title() {
    yinz_types::agent::provider::register(
        0,
        yinz_types::agent::provider::AgentProvider {
            kind: "opencode".into(),
            executable: "opencode".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        detect_kind("shell", Some("/home/u/.opencode/bin/opencode --prompt fix")).as_deref(),
        Some("opencode"),
    );
    assert_eq!(
        classify(
            &pane(3, "shell", Some("opencode --auto --prompt go")),
            &manual(&[]),
        ),
        Some(("opencode".to_owned(), "auto")),
    );
    assert_eq!(
        classify(&pane(4, "opencode — src/main.rs", None), &manual(&[])),
        Some(("opencode".to_owned(), "auto")),
    );
    assert_eq!(
        classify(&latched(5, "opencode", "git status"), &manual(&[])),
        Some(("opencode".to_owned(), "auto")),
    );
    // Back at a prompt, the title it left behind is outlived like any other agent's.
    assert_eq!(detect_kind("opencode", Some("zsh")).as_deref(), None);
}

#[test]
fn is_shell_reads_whole_program_names_only() {
    assert!(is_shell("zsh"));
    assert!(is_shell("/bin/bash --login"));
    assert!(is_shell("-zsh"));
    assert!(!is_shell("shellcheck"));
    assert!(!is_shell("codex"));
    assert!(!is_shell(""));
}

#[test]
fn preview_keeps_the_last_lines_without_the_trailing_blanks() {
    let text = (1..=30).fold(String::new(), |mut all, line| {
        all.push_str(&format!("line {line}\n"));
        all
    }) + "\n\n\n";
    let preview = preview(&text);
    assert!(preview.starts_with("line 11\n"));
    assert!(preview.ends_with("line 30"));
    assert_eq!(preview.lines().count(), PREVIEW_LINES);
}

/// Same convention as `ui::wrap`: the control character goes, whatever followed it
/// stays. A pane read is already rendered text, so this is a belt-and-braces pass.
#[test]
fn preview_drops_control_characters_and_stops_at_the_ceiling() {
    assert_eq!(preview("re\u{7}ad\ty"), "ready");
    assert!(!preview("\x1b[0mready").contains('\x1b'));
    assert_eq!(preview(&"x".repeat(900)).len(), PREVIEW_CHARS);
}

#[test]
fn observe_counts_unchanged_polls_and_resets_on_new_content() {
    let mut activity = Activity::new();
    observe(&mut activity, 3, Sample::Text("working"));
    assert_eq!(activity[&3].1, 0);
    observe(&mut activity, 3, Sample::Text("working"));
    observe(&mut activity, 3, Sample::Text("working"));
    assert_eq!(activity[&3].1, 2);
    observe(&mut activity, 3, Sample::Text("done"));
    assert_eq!(activity[&3].1, 0);
}

/// What the daemon debounces on. Each poll of a stretch of stillness reports its own
/// count, so a caller can pick the length of stillness it acts at and hit it exactly
/// once — and a new screen restarts the count from one.
#[test]
fn observe_counts_the_polls_of_each_stretch_of_stillness() {
    let mut activity = Activity::new();
    // A first sighting cannot tell "working" from "sitting still", so it is not idle.
    assert_eq!(
        observe(&mut activity, 3, Sample::Text("working")),
        Observed::Busy
    );
    for polls in 1..=4 {
        assert_eq!(
            observe(&mut activity, 3, Sample::Text("working")),
            Observed::Idle { polls }
        );
    }

    // Back to work, then still again: a second stretch counts from the start.
    assert_eq!(
        observe(&mut activity, 3, Sample::Text("more")),
        Observed::Busy
    );
    assert_eq!(
        observe(&mut activity, 3, Sample::Text("more")),
        Observed::Idle { polls: 1 }
    );
}

/// The whole point of hashing words rather than text: a pane resized to a new width
/// holds the same session, and used to look like a screenful of new work.
#[test]
fn rewrapping_the_same_output_is_not_a_change() {
    let narrow = "the agent finished\nthe refactor and is\nwaiting for you";
    let wide = "the agent finished the refactor\nand is waiting for you";
    assert_eq!(fingerprint(narrow), fingerprint(wide));

    let mut activity = Activity::new();
    assert_eq!(
        observe(&mut activity, 3, Sample::Text(narrow)),
        Observed::Busy
    );
    assert_eq!(
        observe(&mut activity, 3, Sample::Text(wide)),
        Observed::Idle { polls: 1 },
        "a rewrap is not the agent doing something"
    );
}

/// The other side of it: normalising must not go so far that a working agent looks
/// still. Anything with a digit or a letter in it counts.
#[test]
fn a_ticking_counter_is_still_a_change() {
    let mut activity = Activity::new();
    observe(&mut activity, 3, Sample::Text("thinking (12s)"));
    assert_eq!(
        observe(&mut activity, 3, Sample::Text("thinking (13s)")),
        Observed::Busy
    );
}

/// Spinner frames and rules are not the session changing.
#[test]
fn punctuation_only_tokens_are_not_content() {
    assert_eq!(
        fingerprint("* working on it\n----------"),
        fingerprint("- working on it\n===")
    );
}

/// A reading that says nothing must neither restart the stretch of stillness nor
/// disturb the hash an in-flight summary is stamped against.
#[test]
fn a_sample_we_cannot_use_keeps_the_hash_and_counts_the_pause() {
    let mut activity = Activity::new();
    observe(&mut activity, 3, Sample::Text("waiting for you"));
    let hash = activity_hash(&activity, 3).expect("recorded");
    assert_eq!(
        observe(&mut activity, 3, Sample::Unusable),
        Observed::Idle { polls: 1 }
    );
    assert_eq!(
        observe(&mut activity, 3, Sample::Unusable),
        Observed::Idle { polls: 2 }
    );
    assert_eq!(activity_hash(&activity, 3), Some(hash));
}

/// Before any usable reading there is nothing to be still about.
#[test]
fn the_first_reading_of_a_pane_cannot_be_an_unusable_one() {
    let mut activity = Activity::new();
    assert_eq!(observe(&mut activity, 3, Sample::Unusable), Observed::Busy);
    assert_eq!(activity_hash(&activity, 3), None);
}

#[test]
fn activity_hash_is_the_hash_the_poll_recorded() {
    let mut activity = Activity::new();
    assert_eq!(activity_hash(&activity, 3), None);
    observe(&mut activity, 3, Sample::Text("working"));
    let first = activity_hash(&activity, 3).expect("recorded");
    observe(&mut activity, 3, Sample::Text("working"));
    assert_eq!(
        activity_hash(&activity, 3),
        Some(first),
        "same text, same hash"
    );
    observe(&mut activity, 3, Sample::Text("done"));
    assert_ne!(activity_hash(&activity, 3), Some(first));
}

/// One trimmer for the preview and the summary, so they cannot disagree about what a
/// pane's tail is.
#[test]
fn tail_is_the_body_preview_is_built_from() {
    let text = (1..=200).fold(String::new(), |mut all, line| {
        all.push_str(&format!("line {line}\n"));
        all
    });
    assert_eq!(preview(&text), tail(&text, PREVIEW_LINES, PREVIEW_CHARS));

    let big = tail(&text, SUMMARY_LINES, SUMMARY_CHARS);
    assert_eq!(big.lines().count(), SUMMARY_LINES);
    assert!(big.starts_with("line 81\n"), "{big:.20}");
    assert!(big.ends_with("line 200"));
}

#[test]
fn only_a_pane_with_something_on_it_is_worth_asking_about() {
    assert!(!worth_summarizing(""));
    assert!(!worth_summarizing("   \n \n "));
    assert!(!worth_summarizing("$ "));
    assert!(worth_summarizing(&"x".repeat(SUMMARY_MIN_CHARS)));
}

/// The request the daemon sends. `stream` must be false - the daemon reads a buffered
/// body and has no SSE parser - and neither `temperature` nor `max_tokens` may appear,
/// since providers reject them inconsistently.
#[test]
fn a_summary_request_is_buffered_and_carries_no_contested_knobs() {
    let body = summary_request_body(
        "gpt-4o-mini",
        "claude",
        Some("/work/agent"),
        Some("topic"),
        "waiting for you",
    );
    let value: Value = serde_json::from_slice(&body).expect("valid json");
    assert_eq!(value["model"], "gpt-4o-mini");
    assert_eq!(value["stream"], false);
    assert!(value.get("temperature").is_none());
    assert!(value.get("max_tokens").is_none());
    assert_eq!(value["messages"][0]["role"], "system");
    let user = value["messages"][1]["content"]
        .as_str()
        .expect("a user turn");
    assert!(user.contains("Agent: claude."), "{user}");
    assert!(user.contains("Folder: /work/agent."), "{user}");
    assert!(user.contains("Branch: topic."), "{user}");
    assert!(user.contains("waiting for you"), "{user}");

    // A pane with no folder simply says less, rather than saying "None".
    let bare = summary_request_body("m", "codex", None, None, "hi");
    let value: Value = serde_json::from_slice(&bare).expect("valid json");
    let user = value["messages"][1]["content"].as_str().unwrap();
    assert!(!user.contains("Folder"), "{user}");
    assert!(!user.contains("Branch"), "{user}");
}

/// A completion whose content is `content`, as the provider would frame it.
fn completion(content: &str) -> Vec<u8> {
    json!({ "choices": [{ "message": { "content": content } }] })
        .to_string()
        .into_bytes()
}

#[test]
fn parse_summary_reads_a_clean_object() {
    let summary = parse_summary(
        &completion(r#"{"summary":"Ran the tests, all green.","awaiting":false}"#),
        7,
    )
    .expect("parsed");
    assert_eq!(summary.text, "Ran the tests, all green.");
    assert!(!summary.awaiting);
    assert_eq!(summary.hash, 7, "stamped with the pane it was made from");
}

/// Every provider decorates differently, and a model told "no markdown" still
/// sometimes writes a fence or a sentence of preamble.
#[test]
fn parse_summary_digs_the_object_out_of_whatever_wraps_it() {
    for content in [
        "```json\n{\"summary\":\"Waiting.\",\"awaiting\":true}\n```",
        "Sure! {\"summary\":\"Waiting.\",\"awaiting\":true} Hope that helps.",
        "```\n{\"summary\": \"Waiting.\", \"awaiting\": true}\n```",
    ] {
        let summary = parse_summary(&completion(content), 1).expect(content);
        assert_eq!(summary.text, "Waiting.", "{content}");
        assert!(summary.awaiting, "{content}");
    }
}

/// A `}` inside the summary text must not end the object early, or the parse fails on
/// exactly the answers that talk about code.
#[test]
fn parse_summary_survives_braces_inside_the_summary() {
    let summary = parse_summary(
        &completion(r#"{"summary":"Closed the `if {` block \" here","awaiting":false}"#),
        1,
    )
    .expect("parsed");
    assert_eq!(summary.text, r#"Closed the `if {` block " here"#);
}

/// Absent means false: a model that forgot the key has not claimed the agent is
/// blocked, and a spurious `!` is worse than a missing one.
#[test]
fn a_missing_awaiting_is_not_a_claim_that_the_agent_is_blocked() {
    let summary = parse_summary(&completion(r#"{"summary":"Done."}"#), 1).expect("parsed");
    assert!(!summary.awaiting);
    // Nor is a non-boolean one.
    let summary =
        parse_summary(&completion(r#"{"summary":"Done.","awaiting":"yes"}"#), 1).expect("parsed");
    assert!(!summary.awaiting);
}

#[test]
fn parse_summary_refuses_what_it_cannot_use() {
    assert!(parse_summary(b"not json at all", 1).is_none());
    assert!(parse_summary(&completion("no object here"), 1).is_none());
    assert!(parse_summary(&completion(r#"{"awaiting":true}"#), 1).is_none());
    assert!(parse_summary(&completion(r#"{"summary":"   "}"#), 1).is_none());
    assert!(parse_summary(&completion(r#"{"summary":42}"#), 1).is_none());
    // An unterminated object is not an object.
    assert!(parse_summary(&completion(r#"{"summary":"hi""#), 1).is_none());
    // A well-formed error response has no content to read.
    assert!(parse_summary(br#"{"error":{"message":"nope"}}"#, 1).is_none());
}

#[test]
fn parse_summary_reads_the_title_the_pane_is_named_after() {
    let summary = parse_summary(
        &completion(r#"{"summary":"Ran the tests.","awaiting":false,"title":"running the tests"}"#),
        1,
    )
    .expect("parsed");
    assert_eq!(summary.title, "running the tests");
    assert_eq!(rename_title(&summary), Some("running the tests"));
}

/// A title is a nicety, a summary is the point. A model that answered only the two keys
/// it used to be asked for still gets its summary drawn - it simply does not rename the
/// pane.
#[test]
fn a_summary_without_a_title_is_still_a_summary() {
    for content in [
        r#"{"summary":"Done.","awaiting":false}"#,
        r#"{"summary":"Done.","title":"   "}"#,
        r#"{"summary":"Done.","title":42}"#,
    ] {
        let summary = parse_summary(&completion(content), 1).expect(content);
        assert_eq!(summary.text, "Done.", "{content}");
        assert_eq!(rename_title(&summary), None, "{content}");
    }
}

/// The title travels the same wrapped-in-whatever path the summary does.
#[test]
fn parse_summary_digs_the_title_out_of_a_fence() {
    let summary = parse_summary(
        &completion("```json\n{\"summary\":\"Waiting.\",\"title\":\"fixing resize\"}\n```"),
        1,
    )
    .expect("parsed");
    assert_eq!(summary.title, "fixing resize");
}

#[test]
fn a_long_title_is_cut_to_something_a_pane_frame_could_hold() {
    let long = "x".repeat(SUMMARY_TITLE_CHARS * 2);
    let summary = parse_summary(
        &completion(&json!({"summary": "Done.", "title": long}).to_string()),
        1,
    )
    .expect("parsed");
    assert_eq!(summary.title.chars().count(), SUMMARY_TITLE_CHARS);
}

#[test]
fn a_long_summary_is_cut_to_something_a_row_could_hold() {
    let long = "x".repeat(SUMMARY_TEXT_CHARS * 2);
    let summary = parse_summary(&completion(&json!({"summary": long}).to_string()), 1);
    assert_eq!(
        summary.expect("parsed").text.chars().count(),
        SUMMARY_TEXT_CHARS
    );
}

/// One summary, as `summary_toast` is asked to frame it.
fn toast_summary(text: &str, awaiting: bool) -> Summary {
    Summary {
        text: text.to_owned(),
        title: "running the tests".to_owned(),
        awaiting,
        hash: 1,
        created_at_ms: None,
    }
}

/// The headline is the whole point of the toast: the summary text on its own says
/// nothing about which of several agents produced it.
#[test]
fn a_toast_names_the_pane_above_what_it_said() {
    let toast = summary_toast(3, Some("claude"), &toast_summary("Ran the tests.", false));
    assert_eq!(toast, "agent idle · 3 · claude\nRan the tests.");
}

/// The same claim the strip's `!` makes, in a word.
#[test]
fn a_toast_for_an_agent_blocked_on_the_user_says_so() {
    let toast = summary_toast(7, Some("codex"), &toast_summary("Shall I push?", true));
    assert!(
        toast.starts_with("agent waiting · 7 · codex\n"),
        "got {toast:?}"
    );
}

/// A pane whose kind would not classify still gets a toast - the id names it.
#[test]
fn a_toast_without_a_kind_still_names_the_pane() {
    let toast = summary_toast(9, None, &toast_summary("Done.", false));
    assert_eq!(toast, "agent idle · 9\nDone.");
}

/// The renderer wraps and ellipsizes to the room a toast has; doing it here as well
/// would mean two places disagreeing about how much that is.
#[test]
fn a_toast_carries_the_summary_whole() {
    let long = "x".repeat(SUMMARY_TEXT_CHARS);
    let toast = summary_toast(3, Some("claude"), &toast_summary(&long, false));
    assert!(
        toast.ends_with(&long),
        "the text is passed through untouched"
    );
}

/// The fallback that makes summaries work with no API key at all. Before the hook
/// feed there was nothing to fall back *to* — a screen tail is not a summary of
/// anything, and showing one raw would be worse than showing nothing. A final message
/// is prose already, so its opening is an honest answer.
#[test]
fn a_summary_can_be_made_from_the_agents_own_words_with_no_model() {
    let summary = local_summary(
        "I ran the test suite and it passes. Two snapshots were regenerated along \
         the way; both diffs are whitespace only.",
        42,
    )
    .expect("long enough to be worth drawing");
    assert!(summary
        .text
        .starts_with("I ran the test suite and it passes."));
    assert_eq!(summary.hash, 42);
    // Naming a pane in four words is a judgement, and this path has nobody to make
    // it. An empty title is `rename_title`'s "leave the pane's name alone".
    assert_eq!(summary.title, "");
    assert_eq!(rename_title(&summary), None);
    // Decided by the feed, never here.
    assert!(!summary.awaiting);
}

/// A summary that opens with three lines of somebody else's Rust has said nothing,
/// so a fenced block is dropped whole rather than unwrapped. The markers around the
/// prose go too: they are structure for a renderer this box does not have.
#[test]
fn a_local_summary_keeps_the_sentences_and_drops_the_markdown() {
    let summary = local_summary(
        "## Done\n\n\
         The **flaky** test is fixed. It was waiting on a `sleep` rather than on the \
         condition it cared about.\n\n\
         ```rust\nfn main() { panic!() }\n```\n\n\
         - regenerated one snapshot\n\
         - left the other alone\n",
        0,
    )
    .expect("long enough");
    assert_eq!(
        summary.text,
        "Done The flaky test is fixed. It was waiting on a sleep rather than on the \
         condition it cared about. regenerated one snapshot left the other alone"
    );
    assert!(!summary.text.contains("panic!"));
    assert!(!summary.text.contains('`') && !summary.text.contains('#'));
}

/// The same floor a screen has: below it there is nothing to describe, and a box
/// holding three words is worse than no box.
#[test]
fn a_reply_too_short_to_be_a_summary_is_not_one() {
    assert_eq!(local_summary("Done.", 0), None);
    assert_eq!(local_summary("", 0), None);
    assert_eq!(local_summary("```\nfn main() {}\n```", 0), None);
    assert!(local_summary(&"x".repeat(SUMMARY_MIN_CHARS), 0).is_some());
}

/// A model told to be brief occasionally is not, and neither is an agent. The cut is
/// the same one the model's own answer is held to.
#[test]
fn a_long_reply_is_cut_to_what_a_summary_holds() {
    let summary = local_summary(&"word ".repeat(400), 0).expect("long enough");
    assert!(summary.text.chars().count() <= SUMMARY_TEXT_CHARS);
    assert!(summary.text.ends_with('…'));
}

/// What the model is shown for a run: the history the agent reported as it worked, and
/// the question it was answering. A description of an answer written without sight of
/// the question is half a description.
#[test]
fn a_run_summary_asks_about_the_history_and_the_request_behind_it() {
    let body = run_summary_request_body(
        "gpt-4o-mini",
        "claude",
        Some("/work"),
        Some("main"),
        Some("fix the flaky test"),
        "Fixed it; it was waiting on a sleep.",
    );
    let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
    assert_eq!(body["model"], "gpt-4o-mini");
    assert_eq!(body["stream"], false);
    let user = body["messages"][1]["content"].as_str().expect("a string");
    assert!(user.starts_with("Agent: claude. Folder: /work. Branch: main."));
    assert!(user.contains("fix the flaky test"));
    assert!(user.contains("Fixed it; it was waiting on a sleep."));

    // The system prompt asks for two keys and does not ask about `awaiting`: the feed
    // already knows, and a second opinion would only be a disagreement to resolve.
    let system = body["messages"][0]["content"].as_str().expect("a string");
    assert!(system.contains("\"summary\"") && system.contains("\"title\""));
    assert!(!system.contains("awaiting"));

    // A run with no prompt behind it - a resumed session, a hook-driven
    // continuation - is summarised without one rather than against a stale question.
    let body = run_summary_request_body("m", "claude", None, None, None, "Done.");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
    let user = body["messages"][1]["content"].as_str().expect("a string");
    assert_eq!(user, "Agent: claude.\n\n--- run history ---\nDone.");
}

#[test]
fn run_history_keeps_only_meaningful_hook_events() {
    let event = |name: &str| crate::wire::HookEvent {
        hook_event_name: name.to_owned(),
        ..Default::default()
    };
    assert_eq!(run_entry(&event("PostToolUse")), None);
    assert_eq!(
        run_entry(&event("PreToolUse")),
        None,
        "unnamed tools are noise"
    );

    let prompt = crate::wire::HookEvent {
        prompt: Some("build the history".to_owned()),
        ..event("UserPromptSubmit")
    };
    assert_eq!(
        run_entry(&prompt),
        Some((RunEntryKind::Prompt, "build the history".to_owned()))
    );
    let pasted = crate::wire::HookEvent {
        prompt: Some(
            "Follow the instructions in the pasted text: \n\n<pasted_content id=\"8c9e\">\nbuild\nthe history\n</pasted_content id=\"8c9e\">\n"
                .to_owned(),
        ),
        ..event("UserPromptSubmit")
    };
    assert_eq!(
        run_entry(&pasted),
        Some((RunEntryKind::Prompt, "build\nthe history".to_owned()))
    );
    let plan = crate::wire::HookEvent {
        tool_name: Some("ExitPlanMode".to_owned()),
        tool_input: Some(json!({"plan": "store prompt, plan, steps, result"})),
        ..event("PreToolUse")
    };
    assert_eq!(
        run_entry(&plan),
        Some((
            RunEntryKind::Plan,
            "store prompt, plan, steps, result".to_owned()
        ))
    );
    let ask = crate::wire::HookEvent {
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(json!({"questions": [{"question": "Which library?"}]})),
        ..event("PreToolUse")
    };
    assert_eq!(
        run_entry(&ask),
        Some((RunEntryKind::Question, "Which library?".to_owned()))
    );
    let codex_ask = crate::wire::HookEvent {
        tool_name: Some("request_user_input".to_owned()),
        tool_input: Some(json!({"questions": [{"question": "Which history?"}]})),
        ..event("PreToolUse")
    };
    assert_eq!(
        run_entry(&codex_ask),
        Some((RunEntryKind::Question, "Which history?".to_owned()))
    );
    let result = crate::wire::HookEvent {
        last_assistant_message: Some("Implemented and tested.".to_owned()),
        ..event("Stop")
    };
    assert_eq!(
        run_entry(&result),
        Some((RunEntryKind::Result, "Implemented and tested.".to_owned()))
    );
    assert_eq!(
        run_entry(&event("StopFailure")),
        Some((RunEntryKind::Result, "Turn failed".to_owned()))
    );
    assert_eq!(
        run_entry(&crate::wire::HookEvent {
            tool_name: Some("Bash".to_owned()),
            ..event("PreToolUse")
        }),
        None
    );
    assert_eq!(run_entry(&event("PermissionRequest")), None);
}

/// Nothing a hook event says is cut on its way into the history.
///
/// The regression the split was made for: a prompt used to stop at 600 characters and a
/// question at 80, both silently, because the whole timeline was one blob that grew with
/// every event. It is rows now, and the panel wraps and scrolls.
#[test]
fn a_retained_entry_keeps_the_whole_of_what_the_event_said() {
    let long = "word ".repeat(4000);
    let prompt = crate::wire::HookEvent {
        hook_event_name: "UserPromptSubmit".to_owned(),
        prompt: Some(long.clone()),
        ..Default::default()
    };
    assert_eq!(
        run_entry(&prompt),
        Some((RunEntryKind::Prompt, long.trim().to_owned()))
    );

    let plan = crate::wire::HookEvent {
        hook_event_name: "PreToolUse".to_owned(),
        tool_name: Some("ExitPlanMode".to_owned()),
        tool_input: Some(json!({ "plan": long.clone() })),
        ..Default::default()
    };
    assert_eq!(
        run_entry(&plan),
        Some((RunEntryKind::Plan, long.trim().to_owned()))
    );

    let ask = crate::wire::HookEvent {
        hook_event_name: "PreToolUse".to_owned(),
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(json!({
            "questions": [
                {
                    "header": "Caps",
                    "question": "Which of these should the history keep whole?",
                    "options": [{ "label": "all of them" }, { "label": "none of them" }],
                },
                { "question": "And the second question?" },
            ],
        })),
        ..Default::default()
    };
    let (kind, text) = run_entry(&ask).expect("a question");
    assert_eq!(kind, RunEntryKind::Question);
    assert!(
        text.contains("And the second question?"),
        "every question, not only the first: {text}"
    );
    assert!(text.contains("\n- all of them"), "and its options: {text}");
    assert!(
        text.chars().count() > WAITING_DETAIL_CHARS,
        "which is more than a toast's line: {text}"
    );
}

/// The two spellings of a kind are each other's inverse, so a row round trips.
#[test]
fn every_kind_survives_its_label() {
    for kind in [
        RunEntryKind::Prompt,
        RunEntryKind::Plan,
        RunEntryKind::Question,
        RunEntryKind::Step,
        RunEntryKind::Result,
        RunEntryKind::Message,
        RunEntryKind::Summary,
    ] {
        assert_eq!(RunEntryKind::from_label(kind.label()), Some(kind));
    }
    assert_eq!(RunEntryKind::from_label("something-newer"), None);
}

/// The whole timeline comes back, oldest first, and the caller windows it. The
/// windowing itself now lives in the agents pane, which is the only thing that knows
/// how many rows the history half has.
#[test]
fn a_run_history_is_the_whole_timeline_oldest_first() {
    let mut run = AgentRun {
        id: "claude:session-1".to_owned(),
        pane_id: 7,
        yinz_session_name: "yinz".to_owned().into(),
        host_id: HostId::new(HOST),
        origin_run_id: None,
        previous_run_id: None,
        session_id: "session-1".to_owned(),
        kind: "claude".to_owned(),
        cwd: None,
        host: None,
        bubblewrap: None,
        started_at_ms: 10,
        ended_at_ms: None,
        outcome: RunOutcome::Active,
        revision: 6,
        summarized_revision: 0,
        summary: None,
        entries: ["oldest", "second", "third", "fourth", "fifth", "newest"]
            .into_iter()
            .enumerate()
            .map(|(index, text)| RunEntry {
                tool: None,
                revision: index as u64 + 1,
                event_id: None,
                kind: RunEntryKind::Question,
                text: text.to_owned(),
                created_at_ms: None,
            })
            .collect(),
        omitted_entries: 0,
    };

    let history = run_history(&run, 40, 100, 0);
    assert_eq!(history.label, "claude:session-1");
    assert_eq!(history.entries.len(), 6);
    assert!(history.entries[0].text.contains("oldest"));
    assert!(history.entries[5].text.contains("newest"));

    run.entries.truncate(1);
    let short = run_history(&run, 40, 100, 0);
    assert_eq!(short.entries.len(), 1);
    assert!(short.entries[0].text.contains("oldest"));

    // Too narrow to wrap into anything but a column of single words: better nothing,
    // and the half says so instead.
    assert!(run_history(&run, HISTORY_MIN_COLS + 1, 100, 0).is_empty());
}

/// The two rows above the timeline, which are what the divider's eight characters of
/// session id cannot be: the whole id, and the day and minute the run began.
#[test]
fn the_header_states_the_start_and_the_whole_session_id() {
    let mut run = a_pane_run();
    run.session_id = "9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f".to_owned();
    run.id = format!("claude:{}", run.session_id);
    // 2026-09-04 18:32:07 UTC, read an hour east of it.
    run.started_at_ms = 1_788_546_727_000;

    let header = run_history(&run, 60, run.started_at_ms, 3600).header;
    let rows: Vec<&str> = header.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(
        rows,
        vec![
            "2026-09-04 19:32:07 +01:00",
            "session 9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f",
        ]
    );
    let local =
        ParsedRun::new(&run).lines(60, run.started_at_ms, 0, Some("Sep 4, 2026, 1:32 PM CDT"));
    assert_eq!(local.header[0].text, "Sep 4, 2026, 1:32 PM CDT");
    // The divider still says the short form: the header is what the whole id is for.
    assert_eq!(run_history(&run, 60, 0, 0).label, "claude:9f3c1a7e");
}

/// The fallback id names the machine, and has to.
///
/// A pane id means nothing outside the server that handed it out, so two servers sharing a
/// store in this machine's data dir
/// can reach for the same `kind:pane:5:<epoch>`. The row is written with an unguarded `Put`
/// keyed on it, so a collision would not fail: it would interleave two agents' timelines
/// under one uid and say nothing.
#[test]
fn two_machines_minting_a_fallback_run_at_once_do_not_collide() {
    let here = run_id("claude", 5, "", 1_764_618_000_000, &HostId::new("host-a"));
    let there = run_id("claude", 5, "", 1_764_618_000_000, &HostId::new("host-b"));
    assert_ne!(here, there);
    assert_eq!(here, "claude:pane:5:1764618000000:host-a");

    // An agent with a session id of its own is host-free, and stays that way: the
    // derivation has to reach the same answer on the far side of a warp.
    assert_eq!(
        run_id("claude", 5, "s1", 1_764_618_000_000, &HostId::new("host-a")),
        run_id("claude", 99, "s1", 0, &HostId::new("host-b")),
    );
    assert_eq!(
        run_id("claude", 5, "s1", 0, &HostId::new("host-a")),
        session_run_id("claude", "s1"),
    );

    // A caller that does not know its machine yet keeps the old shape, which is no worse
    // than what it replaced — and it is the shape every run recorded before this has.
    assert_eq!(
        run_id("claude", 5, "", 1_764_618_000_000, &HostId::new("")),
        "claude:pane:5:1764618000000",
    );
}

/// A run the agent gave no session id is keyed by pane, start time and machine (`run_id`),
/// and calling that a session would be a claim it cannot make.
#[test]
fn a_run_with_no_session_id_of_its_own_is_headed_by_its_run_id() {
    let mut run = a_pane_run();
    run.session_id = String::new();
    run.id = run_id("claude", 3, "", 10, &HostId::new("host-a"));

    let header = run_history(&run, 60, 100, 0).header;
    assert_eq!(header[1].text, "run     claude:pane:3:10:host-a");
}

/// Cut ids are useless ids, so a half too narrow for one wraps it instead - under the
/// value column, so the field still reads as one thing.
#[test]
fn a_session_id_wider_than_the_half_wraps_rather_than_being_cut() {
    let mut run = a_pane_run();
    run.session_id = "9f3c1a7e-4b21-4d8e-9c02-1a2b3c4d5e6f".to_owned();

    // Wide enough for the timestamp on one row (26 columns of value), too narrow for a
    // 36-character uuid: the two fields are wrapped independently.
    let header = run_history(&run, 40, 100, 0).header;
    assert_eq!(header.len(), 3, "{header:?}");
    let id: String = header[1..]
        .iter()
        .map(|line| &line.text[HEADER_LABEL_WIDTH..])
        .collect();
    assert_eq!(id, run.session_id);
    for row in &header[2..] {
        assert!(
            row.text.starts_with(&" ".repeat(HEADER_LABEL_WIDTH)),
            "a continuation row sits under the value column: {:?}",
            row.text
        );
    }
}

/// Nothing pins a header over an idle agent's summary: there is no run, so neither of the
/// two facts is in hand.
#[test]
fn a_summary_with_no_run_behind_it_has_no_header() {
    let summary = Summary {
        text: "did a thing".to_owned(),
        title: String::new(),
        awaiting: false,
        hash: 0,
        created_at_ms: Some(50),
    };
    assert!(summary_history(3, "claude", &summary, false, 40, 100)
        .header
        .is_empty());
}

/// A summary somebody asked for is drawn wherever it was made, on a run that is still
/// going and on one that has ended. The timeline keeps it beside the original messages.
#[test]
fn a_summary_somebody_asked_for_is_drawn_on_the_timeline_whatever_the_run_is_doing() {
    let mut run = AgentRun {
        id: "claude:session-1".to_owned(),
        pane_id: 7,
        yinz_session_name: "yinz".to_owned().into(),
        host_id: HostId::new(HOST),
        origin_run_id: None,
        previous_run_id: None,
        session_id: "session-1".to_owned(),
        kind: "claude".to_owned(),
        cwd: None,
        host: None,
        bubblewrap: None,
        started_at_ms: 10,
        ended_at_ms: None,
        outcome: RunOutcome::Active,
        revision: 1,
        summarized_revision: 1,
        summary: Some(Summary {
            text: "Rewrote the retry backoff.".to_owned(),
            title: String::new(),
            awaiting: false,
            hash: 0,
            created_at_ms: None,
        }),
        entries: vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Summary,
            text: "Rewrote the retry backoff.".to_owned(),
            created_at_ms: None,
        }],
        omitted_entries: 0,
    };

    let live = run_history(&run, 60, 100, 0);
    assert!(
        live.entries
            .iter()
            .any(|line| line.text.contains("summary: Rewrote the retry backoff.")),
        "filed on the timeline"
    );
    assert_eq!(
        live.entries
            .iter()
            .filter(|line| line.text.contains("Rewrote the retry backoff."))
            .count(),
        1,
        "once: the rolling summary says the same thing, and is dropped rather than              repeated under the entry"
    );

    run.outcome = RunOutcome::Completed;
    assert!(
        run_history(&run, 60, 100, 0)
            .entries
            .iter()
            .any(|line| line.text.contains("summary:")),
        "and it survives the run ending, unlike a result the summary absorbed"
    );
}

/// The kind travels through the shared store as `summary`, which is what an older build
/// reading the same file would have to understand. Asserted on the wire spelling rather
/// than on a round trip alone, because the spelling is the compatibility surface.
#[test]
fn a_filed_summary_round_trips_as_its_own_kind() {
    let entry = RunEntry {
        tool: None,
        revision: 4,
        event_id: None,
        kind: RunEntryKind::Summary,
        text: "Rewrote the retry backoff.".to_owned(),
        created_at_ms: None,
    };

    let json = serde_json::to_string(&entry).expect("encoded");
    assert!(json.contains(r#""kind":"summary""#), "{json}");
    let back: RunEntry = serde_json::from_str(&json).expect("parsed");
    assert_eq!(back, entry);
    assert_eq!(RunEntryKind::Summary.label(), "summary");
}

/// Two presses against an agent that has not moved produce the same sentence twice, and
/// two identical paragraphs one under the other read as a bug rather than as a history.
/// Only the newest is compared: a run that comes back round to the same state after
/// doing something else really was in that state twice.
#[test]
fn filing_a_summary_skips_one_that_repeats_the_newest() {
    let mut run = AgentRun {
        id: "claude:session-1".to_owned(),
        pane_id: 7,
        yinz_session_name: "yinz".to_owned().into(),
        host_id: HostId::new(HOST),
        origin_run_id: None,
        previous_run_id: None,
        session_id: "session-1".to_owned(),
        kind: "claude".to_owned(),
        cwd: None,
        host: None,
        bubblewrap: None,
        started_at_ms: 10,
        ended_at_ms: None,
        outcome: RunOutcome::Active,
        revision: 0,
        summarized_revision: 0,
        summary: None,
        entries: Vec::new(),
        omitted_entries: 0,
    };

    assert!(push_summary_entry(
        &mut run,
        "Rewrote the retry backoff.",
        100
    ));
    assert_eq!(
        run.revision, 1,
        "a filed summary is a revision like any other"
    );
    assert!(!push_summary_entry(
        &mut run,
        "Rewrote the retry backoff.",
        100
    ));
    assert!(
        !push_summary_entry(&mut run, "  Rewrote the retry backoff.  ", 100),
        "trimmed before comparing, so whitespace is not a new sentence"
    );
    assert_eq!(run.entries.len(), 1);
    assert_eq!(run.revision, 1, "and a skipped one costs no revision");

    assert!(push_summary_entry(&mut run, "Then rewrote the tests.", 100));
    assert!(
        push_summary_entry(&mut run, "Rewrote the retry backoff.", 100),
        "the run came back round to the same state, which really did happen twice"
    );
    assert_eq!(run.entries.len(), 3);

    assert!(
        !push_summary_entry(&mut run, "   ", 100),
        "and nothing at all is never filed"
    );
}

#[test]
fn active_and_archived_runs_keep_results_visible_after_summarization() {
    let mut run = AgentRun {
        id: "claude:session-1".to_owned(),
        pane_id: 7,
        yinz_session_name: "yinz".to_owned().into(),
        host_id: HostId::new(HOST),
        origin_run_id: None,
        previous_run_id: None,
        session_id: "session-1".to_owned(),
        kind: "claude".to_owned(),
        cwd: None,
        host: None,
        bubblewrap: None,
        started_at_ms: 10,
        ended_at_ms: None,
        outcome: RunOutcome::Active,
        revision: 1,
        summarized_revision: 0,
        summary: None,
        entries: vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Result,
            text: "Implemented and tested.".to_owned(),
            created_at_ms: None,
        }],
        omitted_entries: 0,
    };

    assert!(
        !run_history(&run, 60, 100, 0).entries.is_empty(),
        "a completed response stays visible during the run"
    );

    run.outcome = RunOutcome::Completed;
    let archived = run_history(&run, 60, 100, 0);
    assert_eq!(archived.label, "claude:session-1");
    assert!(archived
        .entries
        .iter()
        .any(|line| line.text.contains("result: Implemented and tested.")));

    run.summarized_revision = run.revision;
    run.summary = Some(Summary {
        text: "The implementation is complete.".to_owned(),
        title: String::new(),
        awaiting: false,
        hash: 0,
        created_at_ms: None,
    });
    let summarized = run_history(&run, 60, 100, 0);
    assert!(summarized
        .entries
        .iter()
        .any(|line| line.text.contains("result:")));
    assert!(
        summarized
            .entries
            .iter()
            // Past the age gutter, which is blank on an entry with no recorded time.
            .any(|line| line
                .text
                .trim_start()
                .starts_with("summary: The implementation")),
        "{:?}",
        summarized.entries
    );
}

#[test]
fn an_agent_run_round_trips_through_the_shared_store() {
    use crate::store::{self, Backend};
    use yinz_tile::prelude::{FetchRequest, Mutation, Pred};

    let local = store::Local::shared("agent-service");
    local.define_all(&store::shared_defs()).unwrap();
    let legacy = |uid: &str, active: bool| {
        Mutation::put(rows::RUN_ENTITY)
            .uid(uid)
            .set("pane_id", 7_u32)
            .set("session_id", uid)
            .set("kind", "claude")
            .set("active", active)
            .set("started_at_ms", 1_i64)
            .set("payload", "{}")
            .build()
    };
    local
        .apply(vec![
            legacy("legacy-active", true),
            legacy("legacy-archive", false),
        ])
        .unwrap();
    let mut run = AgentRun {
        id: "claude:session-1".to_owned(),
        pane_id: 7,
        yinz_session_name: "yinz".to_owned().into(),
        host_id: HostId::new(HOST),
        origin_run_id: None,
        previous_run_id: None,
        session_id: "session-1".to_owned(),
        kind: "claude".to_owned(),
        cwd: Some("/work".to_owned()),
        host: None,
        bubblewrap: None,
        started_at_ms: 10,
        ended_at_ms: None,
        outcome: RunOutcome::Active,
        revision: 1,
        summarized_revision: 0,
        summary: None,
        entries: vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text: "implement history".to_owned(),
            created_at_ms: None,
        }],
        omitted_entries: 0,
    };
    local.apply(rows::run_mutations(&run, REV_ABSENT)).unwrap();
    let mut foreign = run.clone();
    foreign.id = "claude:session-2".to_owned();
    foreign.session_id = "session-2".to_owned();
    foreign.yinz_session_name = "another-yinz".to_owned().into();
    local
        .apply(rows::run_mutations(&foreign, REV_ABSENT))
        .unwrap();
    let active = local
        .fetch(&rows::active_runs_request(&owner("yinz")))
        .unwrap();
    // The metadata comes back from the run's own row; the timeline is rows of its own, and
    // arrives only once it is asked for.
    let mut restored = rows::runs_from_rows(&active);
    assert_eq!(
        restored.iter().map(|run| &run.id).collect::<Vec<_>>(),
        vec![&run.id]
    );
    assert!(restored[0].entries.is_empty());
    assert_eq!(
        rows::hydrate_entries(&local, &mut restored),
        std::collections::BTreeSet::from([run.id.clone()])
    );
    assert_eq!(restored, vec![run.clone()]);

    run.ended_at_ms = Some(20);
    run.outcome = RunOutcome::Completed;
    local
        .apply(rows::run_mutations(&run, DataRevision::new(1)))
        .unwrap();
    let completed = local
        .fetch(
            &FetchRequest::entity(rows::RUN_ENTITY)
                .filter(Pred::eq("active", false))
                .filter(Pred::not_null("header"))
                .select(["header"]),
        )
        .unwrap();
    assert!(!completed.columns.iter().any(|column| column == "payload"));
    assert_eq!(rows::run_headers_from_rows(&completed), vec![run.header()]);

    let detail = local
        .fetch(
            &FetchRequest::entity(rows::RUN_ENTITY)
                .filter(Pred::eq("uid", run.id.clone()))
                .select(["payload"]),
        )
        .unwrap();
    let mut detail = rows::runs_from_rows(&detail);
    rows::hydrate_entries(&local, &mut detail);
    assert_eq!(detail, vec![run]);
}

#[test]
fn stored_legacy_run_outcomes_are_normalized_before_resume() {
    use crate::store::{self, Backend};
    use yinz_tile::prelude::{FetchRequest, Mutation, Pred};
    use RunOutcome::{Active, Closed, Completed, Replaced};

    let local = store::Local::shared("agent-service");
    local.define_all(&store::shared_defs()).unwrap();
    for (index, (outcome, ended, expected)) in [
        (None, None, Some(Active)),
        (None, Some(Value::Null), Some(Active)),
        (None, Some(json!(20)), Some(Completed)),
        (Some(json!("active")), Some(json!(20)), Some(Active)),
        (Some(json!("completed")), Some(json!(20)), Some(Completed)),
        (Some(json!("closed")), Some(json!(20)), Some(Closed)),
        (Some(json!("replaced")), Some(json!(20)), Some(Replaced)),
        (Some(Value::Null), Some(json!(20)), None),
        (Some(json!("unknown")), Some(json!(20)), None),
        (None, Some(json!("invalid")), None),
    ]
    .into_iter()
    .enumerate()
    {
        let run = a_run_with_entries(&format!("claude:legacy-{index}"), "yinz", 7, &["asked"]);
        let mut payload = serde_json::to_value(&run).unwrap();
        let mut header = serde_json::to_value(run.header()).unwrap();
        for value in [&mut payload, &mut header] {
            let object = value.as_object_mut().unwrap();
            object.remove("outcome");
            object.remove("ended_at_ms");
            if let Some(outcome) = &outcome {
                object.insert("outcome".into(), outcome.clone());
            }
            if let Some(ended) = &ended {
                object.insert("ended_at_ms".into(), ended.clone());
            }
        }
        local
            .apply(vec![Mutation::put(rows::RUN_ENTITY)
                .uid(run.id.clone())
                .set("pane_id", run.pane_id)
                .set("session_id", run.session_id.clone())
                .set("kind", run.kind.clone())
                .set("active", ended.as_ref().is_none_or(Value::is_null))
                .set("started_at_ms", run.started_at_ms)
                .set("payload", payload.to_string())
                .set("header", header.to_string())
                .build()])
            .unwrap();
        let fetched = local
            .fetch(
                &FetchRequest::entity(rows::RUN_ENTITY)
                    .filter(Pred::eq("uid", run.id.clone()))
                    .select(["payload", "header"]),
            )
            .unwrap();
        let runs = rows::runs_from_rows(&fetched);
        let headers = rows::run_headers_from_rows(&fetched);
        assert_eq!(
            runs.first().map(|run| run.outcome),
            expected,
            "case {index}"
        );
        assert_eq!(
            headers.first().map(|header| header.outcome),
            expected,
            "case {index}"
        );
        if let Some(header) = headers.first() {
            assert_eq!(runs[0].header(), *header);
            assert_eq!(runs[0].entries, run.entries);
            assert_eq!(
                crate::launch::resume_directory(header, &run.host_id).is_ok(),
                expected != Some(Active),
                "case {index}"
            );
        }
    }
}

/// A run written before the timeline had rows of its own keeps the entries in its payload.
///
/// The whole of the backward compatibility: there is no migration and no backfill, so a
/// hydration that finds no rows for a run must leave what the payload carried alone.
#[test]
fn a_run_stored_before_the_split_keeps_the_entries_in_its_payload() {
    use crate::store::{self, Backend};
    use yinz_tile::prelude::Mutation;

    let local = store::Local::shared("agent-service");
    local.define_all(&store::shared_defs()).unwrap();
    let run = a_run_with_entries("claude:session-1", "yinz", 7, &["asked", "answered"]);
    // The old shape: the whole run, timeline included, inside `payload`.
    local
        .apply(vec![Mutation::put(rows::RUN_ENTITY)
            .uid(run.id.clone())
            .set("pane_id", run.pane_id)
            .set("session_id", run.session_id.clone())
            .set("yinz_session_name", run.yinz_session_name.clone())
            .set("host_id", run.host_id.clone())
            .set("kind", run.kind.clone())
            .set("active", true)
            .set("started_at_ms", run.started_at_ms)
            .set("payload", serde_json::to_string(&run).unwrap())
            .build()])
        .unwrap();
    let mut restored = rows::runs_from_rows(
        &local
            .fetch(&rows::active_runs_request(&owner("yinz")))
            .unwrap(),
    );
    assert_eq!(
        restored,
        vec![run.clone()],
        "the payload still carries them"
    );
    assert!(
        rows::hydrate_entries(&local, &mut restored).is_empty(),
        "nothing was hydrated, so the caller knows they are not yet rows"
    );
    assert_eq!(restored, vec![run], "and they were not taken away");
}

/// What the panel is sent is bounded by bytes, not only by rows.
///
/// An entry keeps the whole of what the agent said, so five hundred of them is not a size:
/// only [`rows::RUN_DETAIL_BYTES`] makes the payload the panel is handed a bounded one.
#[test]
fn a_run_detail_is_cut_to_its_byte_budget_from_the_newest_end() {
    use crate::store::{self, Backend};

    let local = store::Local::shared("agent-service");
    local.define_all(&store::shared_defs()).unwrap();
    // Ten entries of a tenth of the budget each, so half of them fit and the oldest half
    // is what goes.
    let each = rows::RUN_DETAIL_BYTES / 10;
    let texts: Vec<String> = (0..10).map(|n| format!("{n}").repeat(each)).collect();
    let run = a_run_with_entries(
        "claude:session-1",
        "yinz",
        7,
        &texts.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    local.apply(rows::run_mutations(&run, REV_ABSENT)).unwrap();

    let mut detail = AgentRun {
        entries: Vec::new(),
        ..run.clone()
    };
    rows::hydrate_newest_entries(&local, &mut detail);
    let spent: usize = detail.entries.iter().map(|entry| entry.text.len()).sum();
    assert!(
        spent <= rows::RUN_DETAIL_BYTES,
        "the payload is within the budget, got {spent}"
    );
    assert_eq!(
        detail.entries,
        run.entries[run.entries.len() - detail.entries.len()..],
        "and what it kept is the newest end, in order"
    );
    assert!(!detail.entries.is_empty());
    assert_eq!(
        detail.omitted_entries,
        run.entries.len() - detail.entries.len(),
        "and it counts what it left out"
    );

    // One entry over the budget on its own is still sent: an empty history is worse than
    // a large one.
    let huge = "x".repeat(rows::RUN_DETAIL_BYTES * 2);
    let big = a_run_with_entries("claude:session-2", "yinz", 8, &[huge.as_str()]);
    local.apply(rows::run_mutations(&big, REV_ABSENT)).unwrap();
    let mut detail = AgentRun {
        entries: Vec::new(),
        ..big.clone()
    };
    rows::hydrate_newest_entries(&local, &mut detail);
    assert_eq!(detail.entries, big.entries);
    assert_eq!(detail.omitted_entries, 0);
}

/// A run longer than the panel is handed says so at the top of its timeline, rather than
/// letting the oldest event it was sent pass for the start of the run.
#[test]
fn a_run_cut_to_its_newest_entries_opens_with_a_notice() {
    use crate::store::{self, Backend};

    let local = store::Local::shared("agent-service");
    local.define_all(&store::shared_defs()).unwrap();
    let texts: Vec<String> = (0..rows::RUN_DETAIL_ENTRIES + 5)
        .map(|n| format!("entry {n}"))
        .collect();
    let run = a_run_with_entries(
        "claude:session-1",
        "yinz",
        7,
        &texts.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    local.apply(rows::run_mutations(&run, REV_ABSENT)).unwrap();
    let mut detail = AgentRun {
        entries: Vec::new(),
        ..run.clone()
    };
    rows::hydrate_newest_entries(&local, &mut detail);
    assert_eq!(detail.entries.len(), rows::RUN_DETAIL_ENTRIES as usize);
    assert!(detail.omitted_entries > 0, "the row limit cut it");
    // The count rides to the panel with the run, and is left off a run that has nothing
    // to say about it.
    let sent: AgentRun = serde_json::from_str(&serde_json::to_string(&detail).unwrap()).unwrap();
    assert_eq!(sent.omitted_entries, detail.omitted_entries);
    assert!(!serde_json::to_string(&run)
        .unwrap()
        .contains("omitted_entries"));

    let history = run_history(&detail, 60, 100, 0);
    assert_eq!(
        history.entries[0].text.trim(),
        "older events not loaded · / searches them"
    );
    assert!(history.entries[0]
        .styles
        .iter()
        .all(|style| style.dim || *style == MdStyle::default()));
    // It belongs to no event: the first one starts after it, and the rest still tile.
    let mut at = 1;
    for event in &history.events {
        assert_eq!(event.start, at, "{:?}", history.events);
        at = event.end();
    }
    assert_eq!(at, history.entries.len());

    // A half too narrow for all of it keeps the part that says what happened, and one too
    // narrow for that says it shorter - a whole phrase at every width, never one cut off
    // mid-word, and still the one row.
    for width in HISTORY_MIN_COLS + 2..=60 {
        let history = run_history(&detail, width, 100, 0);
        let notice = &history.entries[0].text;
        assert!(
            [
                "older not loaded",
                "older events not loaded",
                "older events not loaded · / searches them",
            ]
            .contains(&notice.trim()),
            "{width}: {notice:?}"
        );
        assert!(notice.width() <= width - 2, "{width}: {notice:?}");
        assert_eq!(history.events[0].start, 1, "{width}");
    }

    // A run that was sent whole has no notice.
    let whole = run_history(&run, 60, 100, 0);
    assert!(!whole.entries[0].text.contains("not loaded"));
    assert_eq!(whole.events[0].start, 0);
}

/// A timeline longer than one fetch page still comes back whole.
#[test]
fn a_long_timeline_is_paged_back_in_full() {
    use crate::store::{self, Backend};

    let local = store::Local::shared("agent-service");
    local.define_all(&store::shared_defs()).unwrap();
    let texts: Vec<String> = (0..yinz_tile::prelude::MAX_FETCH_ROWS + 20)
        .map(|n| format!("entry {n}"))
        .collect();
    let run = a_run_with_entries(
        "claude:session-1",
        "yinz",
        7,
        &texts.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    local.apply(rows::run_mutations(&run, REV_ABSENT)).unwrap();
    let mut restored = rows::runs_from_rows(
        &local
            .fetch(&rows::active_runs_request(&owner("yinz")))
            .unwrap(),
    );
    rows::hydrate_entries(&local, &mut restored);
    assert_eq!(restored, vec![run]);
}

#[test]
fn lineage_round_trips_in_payload_and_header_and_defaults_to_absent() {
    use crate::store::{self, Backend};
    let local = store::Local::shared("agent-service");
    local.define_all(&store::shared_defs()).unwrap();
    let mut run = a_run_with_entries("claude:C", "here", 7, &["continue"]);
    for linked in [false, true] {
        if linked {
            run.origin_run_id = Some("claude:A".into());
            run.previous_run_id = Some("claude:B".into());
        }
        let json = serde_json::to_value(&run).unwrap();
        assert_eq!(json.get("origin_run_id").is_some(), linked);
        assert_eq!(serde_json::from_value::<AgentRun>(json).unwrap(), run);
        let header = serde_json::to_value(run.header()).unwrap();
        assert_eq!(header.get("previous_run_id").is_some(), linked);
        assert_eq!(
            serde_json::from_value::<AgentRunHeader>(header).unwrap(),
            run.header()
        );
        local
            .apply(rows::run_mutations(
                &run,
                DataRevision::new(i64::from(linked)),
            ))
            .unwrap();
        assert_eq!(stored_run(&local, &run.id), run);
        let fetched = local
            .fetch(&rows::run_by_id_request(&run.id).select(["header"]))
            .unwrap();
        assert_eq!(rows::run_headers_from_rows(&fetched), vec![run.header()]);
        let adopted = stored_run(&local, &run.id).adopted_by(9, &owner("there"));
        assert_eq!(adopted.origin_run_id, run.origin_run_id);
        assert_eq!(adopted.previous_run_id, run.previous_run_id);
    }
}

#[test]
fn header_wraps_the_session_id_without_showing_lineage() {
    let mut run = a_pane_run();
    run.session_id = "cccccccc-cccc-cccc-cccc-cccccccccccc".into();
    run.origin_run_id = Some("claude:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into());
    run.previous_run_id = Some("claude:bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into());
    for width in [20, 32, 60] {
        let header = run_header(&RunFacts::of(&run), width, 0, None);
        assert!(header.iter().all(|row| row.text.chars().count() <= width));
        let session: String = header
            .iter()
            .skip_while(|row| !row.text.starts_with("session"))
            .enumerate()
            .map(|(i, row)| {
                if i == 0 {
                    row.text["session".len()..].trim()
                } else {
                    row.text.trim()
                }
            })
            .collect();
        assert_eq!(session, run.session_id);
        assert!(!header.iter().any(|row| row.text.contains("origin")
            || row.text.contains("previous")
            || row.text.contains("aaaaaaaa")
            || row.text.contains("bbbbbbbb")));
    }
}

fn a_run_with_entries(id: &str, session: &str, pane_id: u32, entries: &[&str]) -> AgentRun {
    AgentRun {
        id: id.to_owned(),
        pane_id,
        yinz_session_name: session.to_owned().into(),
        host_id: HostId::new(HOST),
        origin_run_id: None,
        previous_run_id: None,
        session_id: "session-1".to_owned(),
        kind: "claude".to_owned(),
        cwd: Some("/work".to_owned()),
        host: None,
        bubblewrap: None,
        started_at_ms: 10,
        ended_at_ms: None,
        outcome: RunOutcome::Active,
        revision: entries.len() as u64,
        summarized_revision: 0,
        summary: None,
        entries: entries
            .iter()
            .enumerate()
            .map(|(index, text)| RunEntry {
                tool: None,
                revision: index as u64 + 1,
                event_id: None,
                kind: RunEntryKind::Prompt,
                text: (*text).to_owned(),
                created_at_ms: None,
            })
            .collect(),
        omitted_entries: 0,
    }
}

/// One stored run, timeline and all — the two fetches a reader of a whole run makes.
fn stored_run(local: &impl crate::store::Backend, id: &str) -> AgentRun {
    let rows = local.fetch(&rows::run_by_id_request(id)).unwrap();
    let mut runs = rows::runs_from_rows(&rows);
    rows::hydrate_entries(local, &mut runs);
    runs.into_iter().next().expect("the run is stored")
}

#[test]
fn run_revisions_request_reads_every_listed_run_in_one_fetch() {
    use crate::store::{self, Backend};
    use yinz_tile::prelude::DataValue;

    let local = store::Local::shared("agent-service");
    local.define_all(&store::shared_defs()).unwrap();
    for id in ["claude:one", "claude:two", "claude:unlisted"] {
        let run = a_run_with_entries(id, "here", 7, &["prompt"]);
        local.apply(rows::run_mutations(&run, REV_ABSENT)).unwrap();
    }
    let fetched = local
        .fetch(&rows::run_revisions_request([
            "claude:one".to_owned(),
            "claude:two".to_owned(),
            "claude:missing".to_owned(),
        ]))
        .unwrap();
    let mut seen: Vec<_> = fetched
        .rows
        .iter()
        .map(|row| {
            let Some(DataValue::Text(uid)) = fetched.get(row, "uid") else {
                panic!("each row names its run")
            };
            assert!(matches!(fetched.get(row, "rev"), Some(DataValue::Int(_))));
            uid.clone()
        })
        .collect();
    seen.sort();
    assert_eq!(seen, ["claude:one", "claude:two"]);
}

#[test]
fn run_writes_reject_stale_metadata_and_roll_back_its_entries() {
    use crate::store::{self, Backend};
    use yinz_tile::prelude::DataValue;

    let first = store::Local::shared("agent-service");
    first.define_all(&store::shared_defs()).unwrap();
    let second = first.joined(store::SHARED, "agent-service");
    let run = a_run_with_entries("claude:concurrent", "here", 7, &["original"]);
    first.apply(rows::run_mutations(&run, REV_ABSENT)).unwrap();
    assert!(second.apply(rows::run_mutations(&run, REV_ABSENT)).is_err());
    let fetched = second.fetch(&rows::run_by_id_request(&run.id)).unwrap();
    let Some(DataValue::Int(revision)) = fetched.get(&fetched.rows[0], "rev") else {
        panic!("the original read carries its database revision")
    };
    let expected = DataRevision::new(*revision);
    let mut winner = stored_run(&first, &run.id);
    let mut stale = stored_run(&second, &run.id);
    winner.cwd = Some("/new/location".into());
    first.apply(rows::run_mutations(&winner, expected)).unwrap();
    stale.entries[0].text = "stale replacement".into();
    stale.entries.push(RunEntry {
        revision: 2,
        ..stale.entries[0].clone()
    });
    for mutations in [
        rows::run_mutations(&stale, expected),
        vec![rows::run_owned_mutation(&stale, &owner("here"), expected)],
    ] {
        assert!(matches!(
            second.apply(mutations),
            Err(yinz_tile::prelude::WriteError::Conflict(_))
        ));
        assert_eq!(stored_run(&first, &run.id), winner);
    }
}

/// A warped pane's run continues where it left off.
///
/// The session that lost the pane cannot tell a warp from a `kill -9`, so it closes the
/// run. The session that gained it looks the same row up by id — which is the agent's own
/// session id, unchanged by the move — and reopens it. Nothing about the run is rebuilt:
/// the prompts, plans and results are the same work by the same agent.
#[test]
fn a_closed_run_is_reopened_by_the_session_that_adopted_its_pane() {
    use crate::store::{self, Backend};

    let losing = store::Local::shared("agent-service");
    losing.define_all(&store::shared_defs()).unwrap();
    let gaining = losing.joined(store::SHARED, "agent-service");

    let mut run = a_run_with_entries("claude:session-1", "here", 7, &["ship it"]);
    losing.apply(rows::run_mutations(&run, REV_ABSENT)).unwrap();
    // The pane leaves the manifest, so the losing session releases the run.
    run.ended_at_ms = Some(20);
    run.outcome = RunOutcome::Closed;
    losing
        .apply(vec![rows::run_owned_mutation(
            &run,
            &owner("here"),
            DataRevision::new(1),
        )])
        .unwrap();
    assert_eq!(
        stored_run(&losing, "claude:session-1").outcome,
        RunOutcome::Closed
    );

    // The gaining session finds it by the id the record carried, and claims it.
    let adopted = stored_run(&gaining, "claude:session-1").adopted_by(2, &owner("there"));
    assert_eq!(adopted.entries.len(), 1, "the history came with it");
    assert_eq!(adopted.started_at_ms, 10, "and so did when it began");
    assert_eq!(adopted.outcome, RunOutcome::Active);
    gaining
        .apply(rows::run_mutations(&adopted, DataRevision::new(2)))
        .unwrap();

    let stored = stored_run(&losing, "claude:session-1");
    assert_eq!(stored.pane_id, 2);
    assert_eq!(stored.yinz_session_name, "there".into());
    assert_eq!(stored.ended_at_ms, None);
}

/// The race the guard exists for, in the order that used to lose the history.
///
/// A session notices a pane is gone on its next manifest, which can be well after the
/// session that took it has adopted the run and added to it. An unconditional write of the
/// loser's closed copy would put the row back to where it was before the move, throwing
/// away everything the agent has done since. The predicate settles it inside the
/// transaction: the row is no longer ours, so the write matches nothing.
#[test]
fn a_release_after_the_run_moved_on_does_not_overwrite_it() {
    use crate::store::{self, Backend};

    let losing = store::Local::shared("agent-service");
    losing.define_all(&store::shared_defs()).unwrap();
    let gaining = losing.joined(store::SHARED, "agent-service");

    let run = a_run_with_entries("claude:session-1", "here", 7, &["ship it"]);
    losing.apply(rows::run_mutations(&run, REV_ABSENT)).unwrap();

    // The gaining session gets there first, and the agent keeps working.
    let mut adopted = stored_run(&gaining, "claude:session-1").adopted_by(2, &owner("there"));
    adopted.revision += 1;
    adopted.entries.push(RunEntry {
        tool: None,
        revision: adopted.revision,
        event_id: None,
        kind: RunEntryKind::Result,
        text: "shipped".to_owned(),
        created_at_ms: None,
    });
    gaining
        .apply(rows::run_mutations(&adopted, DataRevision::new(1)))
        .unwrap();

    // Only now does the losing session notice the pane is gone.
    let mut released = run.clone();
    released.ended_at_ms = Some(20);
    released.outcome = RunOutcome::Closed;
    losing
        .apply(vec![rows::run_owned_mutation(
            &released,
            &owner("here"),
            DataRevision::new(1),
        )])
        .unwrap_err();

    let stored = stored_run(&losing, "claude:session-1");
    assert_eq!(stored, adopted, "the loser's last word was dropped whole");
    assert_eq!(
        stored.entries.len(),
        2,
        "including what arrived after the move"
    );
    assert_eq!(stored.ended_at_ms, None, "and the run is still open");
}

/// The same guard, doing nothing when it should do nothing: a run whose pane really did
/// close is still ours, so releasing it closes it.
#[test]
fn a_release_of_a_run_nobody_took_closes_it() {
    use crate::store::{self, Backend};

    let local = store::Local::shared("agent-service");
    local.define_all(&store::shared_defs()).unwrap();
    let mut run = a_run_with_entries("claude:session-1", "here", 7, &["ship it"]);
    local.apply(rows::run_mutations(&run, REV_ABSENT)).unwrap();

    run.ended_at_ms = Some(20);
    run.outcome = RunOutcome::Closed;
    local
        .apply(vec![rows::run_owned_mutation(
            &run,
            &owner("here"),
            DataRevision::new(1),
        )])
        .unwrap();

    let stored = stored_run(&local, "claude:session-1");
    assert_eq!(stored.outcome, RunOutcome::Closed);
    assert_eq!(stored.ended_at_ms, Some(20));
    // The write that closed it rewrote the payload without a timeline, which is what the
    // entry rows are for: closing a run does not cost it its history.
    assert_eq!(
        stored
            .entries
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>(),
        vec!["ship it"],
    );
    assert!(
        local
            .fetch(&rows::active_runs_request(&owner("here")))
            .unwrap()
            .rows
            .is_empty(),
        "and it is out of the live set"
    );
}

/// Which id a pane's record points at, and when it points at nothing.
#[test]
fn a_records_stored_run_is_named_or_derived_or_neither() {
    let record = |session_id: Option<&str>, run_id: Option<&str>| AgentInfo {
        kind: "claude".to_owned(),
        state: AgentState::Busy,
        source: AgentSource::Reported,
        detail: None,
        waiting_on: None,
        wait_needs_a_person: false,
        cwd: None,
        session_id: session_id.map(str::to_owned),
        run_id: run_id.map(str::to_owned),
    };
    // Named outright: the only road open to an agent with no session id of its own.
    assert_eq!(
        stored_run_id(&record(None, Some("mine:7"))),
        Some("mine:7".to_owned())
    );
    // Derived, and the derivation mentions neither the pane nor the machine - which is
    // what lets the far side of a warp reach the same answer.
    assert_eq!(
        stored_run_id(&record(Some("s1"), None)),
        Some("claude:s1".to_owned())
    );
    // A name wins over a derivation: it is what a plugin actually wrote down.
    assert_eq!(
        stored_run_id(&record(Some("s1"), Some("mine:7"))),
        Some("mine:7".to_owned())
    );
    // Neither. A run keyed by pane id and a timestamp cannot be found from here.
    assert_eq!(stored_run_id(&record(None, None)), None);
    assert_eq!(stored_run_id(&record(Some("  "), Some(" "))), None);
}

/// Two sessions, one store file, one pane id. These rows used to be uid'd by the pane id
/// alone, so each session's write landed on the other's row and each session's prune
/// deleted the other's rows.
#[test]
fn per_pane_rows_belong_to_one_session() {
    use crate::store::{self, Backend};

    let here = store::Local::shared("agent");
    here.define_all(&store::shared_defs()).unwrap();
    let there = here.joined(store::SHARED, "agent");

    here.apply(vec![rows::latch_mutation(&owner("here"), 5, "reviewer")])
        .unwrap();
    there
        .apply(vec![rows::latch_mutation(&owner("there"), 5, "builder")])
        .unwrap();
    let latches_of = |backend: &dyn Backend, session: &str| {
        let rows = backend
            .fetch(&rows::owned_rows_request(
                rows::LATCH_ENTITY,
                &owner(session),
            ))
            .unwrap();
        rows::latches_from_rows(&rows)
    };
    assert_eq!(
        latches_of(&here, "here"),
        BTreeMap::from([(5, "reviewer".to_owned())])
    );
    assert_eq!(
        latches_of(&there, "there"),
        BTreeMap::from([(5, "builder".to_owned())])
    );

    // And a prune reaches only its own.
    here.apply(vec![rows::latch_delete_mutation(&owner("here"), [5])])
        .unwrap();
    assert!(latches_of(&here, "here").is_empty());
    assert_eq!(
        latches_of(&there, "there"),
        BTreeMap::from([(5, "builder".to_owned())]),
        "the other session's pane 5 is a different pane"
    );
}

/// The same collision one size up: two host identities in one store, one session name, one pane
/// id. Session names are not unique between machines — `main` is the obvious case — so
/// scoping by session alone put two machines' pane 5 back into the row the session key had
/// just separated, and each machine's prune deleted the other's.
#[test]
fn per_pane_rows_belong_to_one_machine_as_well_as_one_session() {
    use crate::store::{self, Backend};

    let laptop = store::Local::shared("agent");
    laptop.define_all(&store::shared_defs()).unwrap();
    let build_box = laptop.joined(store::SHARED, "agent");

    // Both sessions are called `main`, which is what makes this a collision rather than a
    // coincidence.
    let on_laptop = rows::Owner::new(HostId::new("host-laptop"), "main".into());
    let on_build_box = rows::Owner::new(HostId::new("host-build-box"), "main".into());

    laptop
        .apply(vec![rows::latch_mutation(&on_laptop, 5, "reviewer")])
        .unwrap();
    build_box
        .apply(vec![rows::latch_mutation(&on_build_box, 5, "builder")])
        .unwrap();

    let latches_of = |backend: &dyn Backend, owner: &rows::Owner| {
        let rows = backend
            .fetch(&rows::owned_rows_request(rows::LATCH_ENTITY, owner))
            .unwrap();
        rows::latches_from_rows(&rows)
    };
    assert_eq!(
        latches_of(&laptop, &on_laptop),
        BTreeMap::from([(5, "reviewer".to_owned())])
    );
    assert_eq!(
        latches_of(&build_box, &on_build_box),
        BTreeMap::from([(5, "builder".to_owned())]),
        "the other machine's pane 5 is a different pane"
    );

    // And a prune reaches only its own machine's.
    laptop
        .apply(vec![rows::latch_delete_mutation(&on_laptop, [5])])
        .unwrap();
    assert!(latches_of(&laptop, &on_laptop).is_empty());
    assert_eq!(
        latches_of(&build_box, &on_build_box),
        BTreeMap::from([(5, "builder".to_owned())]),
        "one machine's prune deleted another machine's row"
    );
}

/// The ownership guard, one size up. A run released by one session is written under a
/// predicate naming its owner, so a session that has since taken the pane keeps its copy.
/// With the owner as a bare session name, `main` on one machine passed the guard held by
/// `main` on another — worse than no guard, because it looks like one.
#[test]
fn a_guarded_run_write_does_not_match_another_machines_session_of_the_same_name() {
    use crate::store::{self, Backend};

    let laptop = store::Local::shared("agent-service");
    laptop.define_all(&store::shared_defs()).unwrap();
    let build_box = laptop.joined(store::SHARED, "agent-service");

    let on_laptop = rows::Owner::new(HostId::new("host-laptop"), "main".into());
    let on_build_box = rows::Owner::new(HostId::new("host-build-box"), "main".into());

    // The build box owns the run, and has said something recent about it.
    let mut run = a_run_with_entries("claude:session-1", "main", 7, &["answered"]);
    run.host_id = on_build_box.host_id.clone();
    build_box
        .apply(rows::run_mutations(&run, REV_ABSENT))
        .unwrap();

    // The laptop's session, also called `main`, closes a run it believes it lost.
    let mut stale = run.clone();
    stale.host_id = on_laptop.host_id.clone();
    stale.ended_at_ms = Some(99);
    stale.outcome = RunOutcome::Completed;
    laptop
        .apply(vec![rows::run_owned_mutation(
            &stale,
            &on_laptop,
            DataRevision::new(1),
        )])
        .unwrap_err();

    let live = rows::runs_from_rows(
        &build_box
            .fetch(&rows::active_runs_request(&on_build_box))
            .unwrap(),
    );
    assert_eq!(
        live.len(),
        1,
        "the other machine's write closed a run it does not own"
    );
    assert_eq!(live[0].outcome, RunOutcome::Active);
}

/// The v1 → v2 migration is one the store performs unasked, and it had better be: every
/// agent declares these entities on load and gives up on the store entirely if a
/// declaration is refused. Dropping `unique` from `pane_id` and adding a column with a
/// default are both non-destructive, but that is a property of the engine rather than of
/// this crate, so it is checked here rather than assumed.
#[test]
fn the_pane_id_only_entities_migrate_rather_than_refusing() {
    use crate::store::{self, Backend};
    use yinz_tile::prelude::{AttrDef, EntityDef, FetchRequest, Mutation};

    let local = store::Local::shared("agent");
    // The definitions as they were, and a row under the uid they used.
    local
        .define(
            &EntityDef::new(rows::LATCH_ENTITY, 1)
                .attr(AttrDef::int("pane_id").unique().required())
                .attr(AttrDef::text("kind").required()),
        )
        .expect("the old definition still declares");
    local
        .apply(vec![Mutation::put(rows::LATCH_ENTITY)
            .uid("5")
            .set("pane_id", 5_u32)
            .set("kind", "reviewer")
            .build()])
        .expect("an old row");

    local
        .define_all(&store::shared_defs())
        .expect("the new definition migrates rather than being refused");

    // Two sessions can now hold pane 5, which is what the unique index forbade.
    local
        .apply(vec![
            rows::latch_mutation(&owner("here"), 5, "reviewer"),
            rows::latch_mutation(&owner("there"), 5, "builder"),
        ])
        .expect("one pane id, two sessions");

    // And the row from before, which no predicate a current row matches can reach, is
    // dropped by the one-time cleanup.
    local
        .apply(rows::stale_unowned_delete_mutations())
        .expect("cleaned up");
    let all = local
        .fetch(&FetchRequest::entity(rows::LATCH_ENTITY))
        .unwrap();
    assert_eq!(
        all.rows.len(),
        2,
        "only the two session-scoped rows are left"
    );
}

/// The `agent_run` half of the same migration, and the one that could not be cleaned up
/// afterwards: a run is history, so an unreachable row is data lost rather than litter.
///
/// The store fills the new `host_id` in with the declaration's default, which is empty, so
/// every run recorded before this branch names no machine. A restore that demanded this
/// machine's id found none of its own open runs — and the guarded close names the host too,
/// so it could not have closed them either. They would have sat `active` forever while a
/// fresh run started beside each of them for the same agent.
#[test]
fn a_run_written_before_the_host_column_is_restored_and_then_claimed() {
    use crate::store::{self, Backend};
    use yinz_tile::prelude::{AttrDef, EntityDef, Mutation};

    let local = store::Local::shared("agent-service");
    // The definition as it was, without a host, and a run stored under it. Its payload has
    // no `host_id` field at all, which is what a run serialized by that version looks like.
    local
        .define(
            &EntityDef::new(rows::RUN_ENTITY, 2)
                .attr(AttrDef::int("pane_id").indexed().required())
                .attr(AttrDef::text("session_id").indexed().required().default(""))
                .attr(AttrDef::text("yinz_session_name").indexed())
                .attr(AttrDef::text("kind").indexed().required())
                .attr(
                    AttrDef::boolean("active")
                        .indexed()
                        .required()
                        .default(true),
                )
                .attr(AttrDef::int("started_at_ms").required())
                .attr(AttrDef::json("header"))
                .attr(AttrDef::json("payload").required()),
        )
        .expect("the old definition still declares");
    let mut before = a_run_with_entries("claude:session-1", "yinz", 7, &[]);
    before.host_id = HostId::default();
    local
        .apply(vec![Mutation::put(rows::RUN_ENTITY)
            .uid(before.id.clone())
            .set("pane_id", before.pane_id)
            .set("session_id", before.session_id.clone())
            .set("yinz_session_name", before.yinz_session_name.clone())
            .set("kind", before.kind.clone())
            .set("active", true)
            .set("started_at_ms", before.started_at_ms)
            .set("payload", serde_json::to_string(&before).unwrap())
            .build()])
        .expect("a run from before the column");

    local
        .define_all(&store::shared_defs())
        .expect("the new definition migrates rather than being refused");

    // The restore finds it, where a strict host check would have found nothing.
    let mut restored = rows::runs_from_rows(
        &local
            .fetch(&rows::active_runs_request(&owner("yinz")))
            .unwrap(),
    );
    assert_eq!(restored, vec![before.clone()], "the run was stranded");
    assert!(restored[0].host_id.is_empty(), "and it still names no host");

    // Claiming costs no write of its own: the stamp rides out with the next persist.
    rows::claim_unowned_runs(&mut restored, &owner("yinz"));
    assert_eq!(restored[0].host_id, HostId::new(HOST));
    local
        .apply(rows::run_mutations(&restored[0], DataRevision::new(1)))
        .unwrap();

    // Which narrows the row. Another machine running a session of the same name could reach
    // it while it named no host, and cannot once it does.
    let elsewhere = rows::Owner::new(HostId::new("host-b"), "yinz".into());
    assert!(
        rows::runs_from_rows(&local.fetch(&rows::active_runs_request(&elsewhere)).unwrap())
            .is_empty(),
        "the claimed run is not the other machine's"
    );
    assert_eq!(
        rows::runs_from_rows(
            &local
                .fetch(&rows::active_runs_request(&owner("yinz")))
                .unwrap()
        )
        .len(),
        1,
        "and is still ours"
    );
}

/// The other end of the same run's life. A close is written under a guard, so a guard this
/// session could never satisfy would leave a pre-migration run open for good.
#[test]
fn a_guarded_close_can_close_a_run_that_predates_the_host_column() {
    use crate::store::{self, Backend};

    let local = store::Local::shared("agent-service");
    local.define_all(&store::shared_defs()).unwrap();
    let mut run = a_run_with_entries("claude:session-1", "yinz", 7, &["asked"]);
    run.host_id = HostId::default();
    local.apply(rows::run_mutations(&run, REV_ABSENT)).unwrap();

    // The daemon restores it, claims it in memory, and closes it when the pane goes.
    run.host_id = HostId::new(HOST);
    run.ended_at_ms = Some(99);
    run.outcome = RunOutcome::Closed;
    local
        .apply(vec![rows::run_owned_mutation(
            &run,
            &owner("yinz"),
            DataRevision::new(1),
        )])
        .unwrap();

    assert!(
        rows::runs_from_rows(
            &local
                .fetch(&rows::active_runs_request(&owner("yinz")))
                .unwrap()
        )
        .is_empty(),
        "the run could not be closed"
    );
    let closed = rows::runs_from_rows(&local.fetch(&rows::run_by_id_request(&run.id)).unwrap());
    assert_eq!(closed[0].outcome, RunOutcome::Closed);
    assert_eq!(
        closed[0].host_id,
        HostId::new(HOST),
        "the close is also what claimed the row"
    );
}

/// And the case that must *not* be adopted, so the two predicates stay distinct. A latch or
/// a summary that names no machine names a pane in a server that is gone, and is dropped
/// rather than taken over — a stale latch is worth less than a wrong one.
#[test]
fn a_latch_that_predates_the_host_column_is_not_adopted() {
    use crate::store::{self, Backend};
    use yinz_tile::prelude::{FetchRequest, Mutation};

    let local = store::Local::shared("agent");
    local.define_all(&store::shared_defs()).unwrap();
    local
        .apply(vec![Mutation::put(rows::LATCH_ENTITY)
            .uid("yinz:5")
            .set("pane_id", 5_u32)
            .set("yinz_session_name", "yinz")
            .set("host_id", "")
            .set("kind", "reviewer")
            .build()])
        .expect("a latch from before the column");

    let mine = local
        .fetch(&rows::owned_rows_request(
            rows::LATCH_ENTITY,
            &owner("yinz"),
        ))
        .unwrap();
    assert!(
        rows::latches_from_rows(&mine).is_empty(),
        "a latch naming no machine is not this machine's to restore"
    );
    local.apply(rows::stale_unowned_delete_mutations()).unwrap();
    assert!(local
        .fetch(&FetchRequest::entity(rows::LATCH_ENTITY))
        .unwrap()
        .is_empty());
}

/// The interruption the feed exists to make in time. Shaped like `summary_toast`, so
/// a user who has seen one recognises the other.
#[test]
fn a_waiting_toast_names_the_pane_and_what_it_is_asking() {
    let feed = |waiting_on: Option<&str>| HookFeed {
        phase: HookPhase::Waiting,
        detail: None,
        waiting_on: waiting_on.map(str::to_owned),
        session_id: "s1".to_owned(),
        cwd: None,
    };
    assert_eq!(
        waiting_toast(7, Some("claude◆"), &feed(Some("Bash cargo"))),
        "agent waiting · 7 · claude◆\nBash cargo"
    );
    // A wait we could not name is still worth interrupting for: the pane id is what
    // the user acts on, and the toast focuses that pane when it is clicked.
    assert_eq!(
        waiting_toast(7, Some("claude"), &feed(None)),
        "agent waiting · 7 · claude\nwaiting for you to answer"
    );
    // A pane whose classification failed keeps the id and loses the segment.
    assert_eq!(
        waiting_toast(9, None, &feed(Some("approve the plan"))),
        "agent waiting · 9\napprove the plan"
    );
}

#[test]
fn compact_provider_names() {
    for (kind, expected) in [
        ("codex", "CDX"),
        ("claude", "CLA"),
        ("opencode", "OPC"),
        ("gemini", "GEM"),
        ("cursor", "CUR"),
        ("qwen", "QWN"),
        ("aider", "AID"),
        ("cline", "CLI"),
        ("pi", "PIA"),
        ("muse", "MUS"),
        ("reviewer", "REV"),
        ("xy", "XY "),
        ("é", "É  "),
        ("", "   "),
    ] {
        assert_eq!(compact_kind(kind), expected);
    }
}

#[test]
fn compact_rows_only_change_with_identity_or_state() {
    let mut pane = pane(12, "first title", Some("codex"));
    pane.host = Some("box".into());
    let mut hooks = feeds(&[(12, HookPhase::Busy)]);
    let render = |pane: &AgentPane, hooks: &HookFeeds, activity: &Activity| {
        strip_rows(
            std::slice::from_ref(pane),
            &manual(&[]),
            activity,
            &owners(&[]),
            &branches(&[]),
            &Summaries::new(),
            hooks,
        )[2]
        .clone()
    };
    let before = render(&pane, &hooks, &Activity::new());
    assert_eq!(before, agent(12, "codex◆", "12 CDX@box ●"));
    pane.title = "a new title".into();
    pane.tab = Some((TabId::new(9), "another tab".into()));
    hooks.get_mut(&12).unwrap().detail = Some("Bash cargo test".into());
    assert_eq!(render(&pane, &hooks, &Activity::new()), before);
    for (status, activity, phase, expected) in [
        (AgentStatus::Running, None, None, '?'),
        (AgentStatus::Running, Some((1, 0)), None, '●'),
        (AgentStatus::Running, Some((1, 1)), None, '○'),
        (
            AgentStatus::Running,
            Some((1, 99)),
            Some(HookPhase::Idle),
            '○',
        ),
        (AgentStatus::Running, None, Some(HookPhase::Waiting), '!'),
        (AgentStatus::Held, None, Some(HookPhase::Waiting), '■'),
        (AgentStatus::Exited, None, Some(HookPhase::Busy), '×'),
    ] {
        pane.status = status;
        let hooks = phase.map(|phase| feeds(&[(12, phase)])).unwrap_or_default();
        let activity = activity
            .map(|value| Activity::from([(12, value)]))
            .unwrap_or_default();
        let StripRow::Agent { text, .. } = render(&pane, &hooks, &activity) else {
            panic!("agent row")
        };
        assert_eq!(text, format!("12 CDX@box {expected}"));
    }
}

#[test]
fn strip_rows_flag_only_the_agents_waiting_on_an_answer() {
    let panes = vec![
        pane(3, "codex", Some("codex")),
        pane(4, "claude", Some("claude")),
        pane(5, "aider", Some("aider")),
    ];
    let rows = strip_rows(
        &panes,
        &manual(&[]),
        // All three idle, so nothing but the answers decides who is flagged.
        &Activity::from([(3, (1, 2)), (4, (1, 2)), (5, (1, 2))]),
        &owners(&[]),
        &branches(&[]),
        // Pane 4 is blocked; pane 3 has a summary but is merely finished; pane 5 has
        // never gone idle and so has no summary at all.
        &summaries(&[(3, false), (4, true)]),
        &no_hooks(),
    );

    assert_eq!(
        rows,
        vec![
            project("no project"),
            branch("no branch"),
            // `waiting` rather than the `idle` its screen is: the row says one thing.
            waiting(4, "claude", "4 CLA !"),
            agent(3, "codex", "3 CDX ○"),
            agent(5, "aider", "5 AID ○"),
        ],
    );
}

#[test]
fn hook_markers_stay_in_details_but_not_compact_rows() {
    let mut remote = pane(5, "claude", Some("claude"));
    remote.host = Some("box".to_owned());
    let panes = vec![
        pane(3, "codex", Some("codex")),
        pane(4, "claude", Some("claude")),
        remote,
    ];
    let rows = strip_rows(
        &panes,
        &manual(&[]),
        &Activity::from([(3, (1, 2)), (4, (1, 2)), (5, (1, 2))]),
        &owners(&[]),
        &branches(&[]),
        &summaries(&[]),
        // The two claudes report; the codex beside them cannot.
        &feeds(&[(4, HookPhase::Idle), (5, HookPhase::Busy)]),
    );

    assert_eq!(
        rows,
        vec![
            project("no project"),
            branch("no branch"),
            agent(5, "claude◆", "5 CLA@box ●"),
            agent(3, "codex", "3 CDX ○"),
            agent(4, "claude◆", "4 CLA ○"),
        ],
    );

    // Details retain the full name and reported-state marker.
    assert_eq!(
        rows.iter()
            .filter_map(|row| match row {
                StripRow::Agent { pane_id, kind, .. } => Some((*pane_id, kind.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(5, "claude◆"), (3, "codex"), (4, "claude◆")],
    );
}

/// The marker cannot be confused for the one the agents pane looks for. That pane
/// colours the `!` in a box header by *searching* the line for the character, so a
/// marker sharing it would light up the wrong column — see `render_summary` there.
#[test]
fn the_hook_marker_is_not_the_waiting_marker() {
    assert!(!HOOK_MARKER.contains('!'));
    assert_eq!(kind_cell("claude", None, false), "claude");
    assert_eq!(kind_cell("claude", None, true), "claude◆");
    assert_eq!(kind_cell("claude", Some("box"), true), "claude◆@box");
    assert_eq!(kind_cell("claude", Some("box"), false), "claude@box");
}

/// A pane that has settled on a screen the stored summary was never made for — the
/// hash it was made for belongs to whatever the pane showed before. Showing the marker
/// here would tell the user (and the model) the summary describes the current screen
/// when it does not.
#[test]
fn strip_rows_does_not_flag_a_pane_whose_summary_predates_its_current_screen() {
    let panes = vec![pane(4, "claude", Some("claude"))];
    let rows = strip_rows(
        &panes,
        &manual(&[]),
        // Idle on hash 2; the summary below was made for hash 1.
        &Activity::from([(4, (2, 3))]),
        &owners(&[]),
        &branches(&[]),
        &summaries(&[(4, true)]),
        &no_hooks(),
    );

    assert_eq!(
        rows,
        vec![
            project("no project"),
            branch("no branch"),
            agent(4, "claude", "4 CLA ○")
        ],
    );
}

#[test]
fn list_json_tells_the_model_what_the_strip_draws() {
    let json = list_json(
        &[
            pane(3, "codex", Some("codex")),
            pane(4, "aider", Some("aider")),
        ],
        &manual(&[]),
        &Activity::from([(3, (1, 2)), (4, (1, 2))]),
        &owners(&[]),
        &branches(&[]),
        &summaries(&[(3, true)]),
        &no_hooks(),
        |_| String::new(),
    );
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value[0]["summary"], "did a thing");
    assert_eq!(value[0]["awaiting_confirmation"], true);
    // An agent that has never gone idle says nothing rather than saying "false".
    assert!(value[1].get("summary").is_none());
    assert!(value[1].get("awaiting_confirmation").is_none());
}

/// `awaiting_confirmation` must not disagree with `waiting_marker` — a summary made for
/// a screen the pane has since moved past does not describe the pane's current state.
#[test]
fn list_json_does_not_claim_a_stale_summary_is_awaiting_confirmation() {
    let json = list_json(
        &[pane(3, "codex", Some("codex"))],
        &manual(&[]),
        // Idle on hash 2; the summary below was made for hash 1.
        &Activity::from([(3, (2, 2))]),
        &owners(&[]),
        &branches(&[]),
        &summaries(&[(3, true)]),
        &no_hooks(),
        |_| String::new(),
    );
    let value: Value = serde_json::from_str(&json).unwrap();
    // The stale text is still surfaced (it is the last thing known), but not as an
    // answer to the pane's current screen.
    assert_eq!(value[0]["summary"], "did a thing");
    assert_eq!(value[0]["awaiting_confirmation"], false);
}

#[test]
fn status_cell_grades_a_running_agent_by_how_long_its_pane_sat_still() {
    let cell = |polls| status_cell(AgentStatus::Running, Some(&(1, polls)), None);
    assert_eq!(status_cell(AgentStatus::Running, None, None), "running");
    assert_eq!(cell(0), "busy");
    assert_eq!(cell(11), "idle");
    assert_eq!(cell(12), "idle 1m");
    assert_eq!(cell(36), "idle 3m");
    // A pane that is not running has a better word than any guess.
    assert_eq!(status_cell(AgentStatus::Held, Some(&(1, 40)), None), "held");
}

/// The whole point of the feed: a pane whose agent reports for itself is read off
/// that report, not off how long its picture has held still. A `Stop` names the
/// pane idle on the poll it arrives, where the heuristic would have called the
/// same reading `busy` and then spent three more polls settling.
#[test]
fn a_reporting_agent_is_read_off_its_own_report_not_off_its_screen() {
    let feed = |phase| HookFeed {
        phase,
        detail: None,
        waiting_on: None,
        session_id: "s1".to_owned(),
        cwd: None,
    };
    let just_moved = Some(&(1u64, 0u32));

    assert_eq!(
        status_cell(AgentStatus::Running, just_moved, None),
        "busy",
        "without a feed, a screen that just moved is a working agent"
    );
    assert_eq!(
        status_cell(
            AgentStatus::Running,
            just_moved,
            Some(&feed(HookPhase::Idle))
        ),
        "idle",
        "with one, the same reading is the start of an idle stretch"
    );
    assert_eq!(
        status_cell(
            AgentStatus::Running,
            just_moved,
            Some(&feed(HookPhase::Waiting))
        ),
        "waiting"
    );
    // The idle clock is still `activity`'s, so one pane's minutes read like
    // every other pane's.
    assert_eq!(
        status_cell(
            AgentStatus::Running,
            Some(&(1, 36)),
            Some(&feed(HookPhase::Idle))
        ),
        "idle 3m"
    );
    // A pane that is not running has a better word than any report about it.
    assert_eq!(
        status_cell(AgentStatus::Held, just_moved, Some(&feed(HookPhase::Busy))),
        "held"
    );
}

/// The tool hint rides in the status cell rather than in a variant of its own.
#[test]
fn a_running_tool_is_named_in_the_status_cell() {
    let busy = |detail: Option<&str>| {
        status_cell(
            AgentStatus::Running,
            Some(&(1, 0)),
            Some(&HookFeed {
                phase: HookPhase::Busy,
                detail: detail.map(str::to_owned),
                waiting_on: None,
                session_id: "s1".to_owned(),
                cwd: None,
            }),
        )
    };
    assert_eq!(busy(None), "busy");
    assert_eq!(busy(Some("Bash cargo")), "busy · Bash cargo");
}

/// One hook event carrying nothing but its name.
fn event(name: &str) -> crate::wire::HookEvent {
    crate::wire::HookEvent {
        hook_event_name: name.to_owned(),
        ..Default::default()
    }
}

/// One `Notification` of the kind named.
fn notification(kind: &str) -> crate::wire::HookEvent {
    crate::wire::HookEvent {
        hook_event_name: "Notification".to_owned(),
        notification_type: Some(kind.to_owned()),
        ..Default::default()
    }
}

/// Every event we wire, and the states they name. The three that say nothing are
/// as important as the ones that do: a notification about a quota reset must not
/// draw a `!` beside a pane that is working.
#[test]
fn hook_events_map_to_the_states_they_name() {
    for source in ["startup", "resume", "fork", "clear"] {
        assert_eq!(
            hook_phase(
                "claude",
                &crate::wire::HookEvent {
                    source: Some(source.to_owned()),
                    ..event("SessionStart")
                }
            ),
            Some(HookPhase::Idle)
        );
    }
    for source in ["compact", "something_new"] {
        assert_eq!(
            hook_phase(
                "claude",
                &crate::wire::HookEvent {
                    source: Some(source.to_owned()),
                    ..event("SessionStart")
                }
            ),
            None
        );
    }
    assert_eq!(hook_phase("claude", &event("SessionStart")), None);
    assert_eq!(hook_phase("claude", &event("Stop")), Some(HookPhase::Busy));
    assert_eq!(
        hook_phase("claude", &event("UserPromptSubmit")),
        Some(HookPhase::Busy)
    );
    assert_eq!(
        hook_phase("claude", &event("PreToolUse")),
        Some(HookPhase::Busy)
    );
    assert_eq!(
        hook_phase("claude", &event("PostToolUse")),
        Some(HookPhase::Busy)
    );
    assert_eq!(
        hook_phase("claude", &event("PermissionRequest")),
        Some(HookPhase::Busy)
    );
    assert_eq!(
        hook_phase("claude", &notification("permission_prompt")),
        Some(HookPhase::Waiting)
    );
    assert_eq!(
        hook_phase("claude", &notification("agent_needs_input")),
        Some(HookPhase::Waiting)
    );
    assert_eq!(
        hook_phase("claude", &notification("worker_permission_prompt")),
        None
    );

    // A finished turn nobody has answered yet is idleness, not a question.
    assert_eq!(hook_phase("claude", &notification("idle_prompt")), None);
    assert_eq!(
        hook_phase("claude", &notification("quota_auto_resume_fired")),
        None
    );
    // Ending a session is not a state to be in - callers check HOOK_SESSION_END.
    assert_eq!(hook_phase("claude", &event(HOOK_SESSION_END)), None);
    // A version that grows an event we have never heard of costs us nothing.
    assert_eq!(
        hook_phase("claude", &event("SomethingNewInAYearsTime")),
        None
    );
}

/// A turn that dies on an API error has ended as surely as one that finished, and
/// `StopFailure` is the only thing that says so. Without it the feed sits on `busy`
/// until the user types again — and nothing here expires on a clock, so "until the
/// user types again" means exactly that.
#[test]
fn a_turn_that_failed_ends_the_agents_work_like_one_that_finished() {
    let failed = crate::wire::HookEvent {
        hook_event_name: "StopFailure".to_owned(),
        last_assistant_message: Some("I was about to run the tests.".to_owned()),
        ..Default::default()
    };
    assert_eq!(hook_phase("claude", &failed), Some(HookPhase::Idle));
}

/// The two tools that are themselves a question. Every other tool starting is work;
/// these do nothing but put a prompt up and wait, so a `PreToolUse` naming one is a
/// wait rather than the busy every other `PreToolUse` is.
#[test]
fn a_tool_that_asks_the_user_something_is_a_wait_not_work() {
    let pre = |tool: &str| crate::wire::HookEvent {
        hook_event_name: "PreToolUse".to_owned(),
        tool_name: Some(tool.to_owned()),
        ..Default::default()
    };
    assert_eq!(
        hook_phase("claude", &pre("AskUserQuestion")),
        Some(HookPhase::Waiting)
    );
    assert_eq!(
        hook_phase("claude", &pre("ExitPlanMode")),
        Some(HookPhase::Waiting)
    );
    assert_eq!(hook_phase("claude", &pre("Bash")), Some(HookPhase::Busy));

    // And the answer coming back is ordinary work again, which is what clears the
    // wait without either tool needing a case of its own on the way out.
    let post = crate::wire::HookEvent {
        hook_event_name: "PostToolUse".to_owned(),
        tool_name: Some("AskUserQuestion".to_owned()),
        ..Default::default()
    };
    assert_eq!(hook_phase("claude", &post), Some(HookPhase::Busy));
}

/// The events wired since the first eight, each in the state it names.
#[test]
fn the_rest_of_the_lifecycle_names_the_state_it_is_in() {
    for name in [
        "PostToolUseFailure",
        "PermissionDenied",
        "ElicitationResult",
    ] {
        assert_eq!(
            hook_phase("claude", &event(name)),
            Some(HookPhase::Busy),
            "{name} is the agent working"
        );
    }
    assert_eq!(
        hook_phase("claude", &event("Elicitation")),
        Some(HookPhase::Waiting)
    );
    for name in ["SubagentStart", "SubagentStop", "PreCompact", "PostCompact"] {
        assert_eq!(hook_phase("claude", &event(name)), None);
    }
    // A *background* session that finished, which is the pane's agent still working —
    // Claude Code raises this one from its background-job monitor, on the foreground
    // session's payload. The pane's own turn ending is `Stop`, which arrives earlier and
    // carries the words to describe it. See `agent::WAITING_NOTIFICATIONS`.
    assert_eq!(hook_phase("claude", &notification("agent_completed")), None);
}

/// One `PermissionRequest` for a tool, under a mode.
fn request(tool: Option<&str>, mode: Option<&str>) -> crate::wire::HookEvent {
    crate::wire::HookEvent {
        hook_event_name: "PermissionRequest".to_owned(),
        tool_name: tool.map(str::to_owned),
        permission_mode: mode.map(str::to_owned),
        ..Default::default()
    }
}

/// Manual approval waits; automatic approval is work on both reporting paths.
#[test]
fn permission_phases_distinguish_manual_and_automatic_approval() {
    assert!(waiting_needs_a_person(&request(Some("Bash"), None)));
    assert_eq!(
        hook_phase("codex", &request(Some("Bash"), None)),
        Some(HookPhase::Waiting)
    );
    assert!(!waiting_needs_a_person(&request(
        Some("Bash"),
        Some("bypassPermissions")
    )));
    assert_eq!(
        hook_phase("claude", &request(Some("Bash"), Some("bypassPermissions"))),
        Some(HookPhase::Busy)
    );
}

/// What the user is actually being asked, from each of the four events that can
/// say. This is what the toast carries, so it is worth pinning exactly.
#[test]
fn a_wait_says_what_it_is_waiting_for() {
    let ask = crate::wire::HookEvent {
        hook_event_name: "PreToolUse".to_owned(),
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(json!({
            "questions": [{
                "question": "Which library should we use for date formatting?",
                "header": "Library",
                "options": [{"label": "chrono", "description": "the usual one"}],
            }],
        })),
        ..Default::default()
    };
    assert_eq!(
        waiting_detail(&ask).as_deref(),
        Some("Which library should we use for date formatting?")
    );

    let plan = crate::wire::HookEvent {
        hook_event_name: "PreToolUse".to_owned(),
        tool_name: Some("ExitPlanMode".to_owned()),
        ..Default::default()
    };
    assert_eq!(waiting_detail(&plan).as_deref(), Some("approve the plan"));

    let permission = crate::wire::HookEvent {
        hook_event_name: "PermissionRequest".to_owned(),
        tool_name: Some("Bash".to_owned()),
        tool_input: Some(json!({"command": "cargo x test -p agent-proto"})),
        ..Default::default()
    };
    assert_eq!(waiting_detail(&permission).as_deref(), Some("Bash cargo"));

    let elicitation = crate::wire::HookEvent {
        hook_event_name: "Elicitation".to_owned(),
        message: Some("Pick a repository".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        waiting_detail(&elicitation).as_deref(),
        Some("Pick a repository")
    );

    // Nothing to say about an event that is not a wait.
    assert_eq!(waiting_detail(&event("Stop")), None);
    assert_eq!(waiting_detail(&event("PostToolUse")), None);
}

/// `tool_input` has no schema we control. A Claude Code that reshapes
/// `AskUserQuestion`'s arguments must cost us the question's *text*, not the fact
/// that a question was asked — and must never cost us a panic.
#[test]
fn a_question_we_cannot_read_still_says_a_question_was_asked() {
    let ask = |input: serde_json::Value| crate::wire::HookEvent {
        hook_event_name: "PreToolUse".to_owned(),
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(input),
        ..Default::default()
    };
    for input in [
        json!({}),
        json!({"questions": []}),
        json!({"questions": "not an array"}),
        json!({"questions": [{}]}),
        json!({"questions": [{"question": ""}]}),
        json!({"questions": [{"question": 7}]}),
    ] {
        assert_eq!(
            waiting_detail(&ask(input.clone())).as_deref(),
            Some("a question"),
            "{input}"
        );
    }
    // The header is the fallback while there is still one to read.
    assert_eq!(
        waiting_detail(&ask(json!({"questions": [{"header": "Library"}]}))).as_deref(),
        Some("Library")
    );
}

/// The other half of the `tool_input` argument: text the agent's own input can
/// steer, going into a toast. Newlines collapse, control characters go, and a long
/// question is cut rather than allowed to fill the box.
#[test]
fn a_wait_detail_is_one_clean_line_of_bounded_length() {
    let ask = |question: &str| crate::wire::HookEvent {
        hook_event_name: "PreToolUse".to_owned(),
        tool_name: Some("AskUserQuestion".to_owned()),
        tool_input: Some(json!({"questions": [{"question": question}]})),
        ..Default::default()
    };

    let detail =
        waiting_detail(&ask("  which\n\n\tone\x1b[2J  do   you\rwant?  ")).expect("a question");
    assert_eq!(detail, "which one[2J do you want?");
    assert!(!detail.contains('\n') && !detail.contains('\t'));

    // The cap counts the ellipsis: what it bounds is the width of the finished line,
    // not the length of what was kept before it was marked as cut.
    let long = waiting_detail(&ask(&"word ".repeat(80))).expect("a question");
    assert!(long.chars().count() <= WAITING_DETAIL_CHARS, "{long}");
    assert!(long.ends_with('…'));
    assert!(long.starts_with("word word"), "{long}");
}

/// The prompt names the pane the moment it is submitted, so what a person typed over
/// several lines has to become one line a frame can hold.
#[test]
fn a_prompt_title_is_one_bounded_line_of_what_was_asked_for() {
    assert_eq!(
        prompt_title("  fix the flaky\n\treconnect\x1b[2J test  ").as_deref(),
        Some("fix the flaky reconnect[2J test")
    );

    // Cut to the same budget a waiting detail has, ellipsis included in it.
    let long = prompt_title(&"word ".repeat(80)).expect("a title");
    assert!(long.chars().count() <= WAITING_DETAIL_CHARS, "{long}");
    assert!(long.ends_with('…'), "{long}");

    // Nothing to name the pane after leaves its name alone, the same answer
    // `rename_title` gives an empty title.
    assert_eq!(prompt_title("   \n\t ").as_deref(), None);
    assert_eq!(prompt_title("").as_deref(), None);
}

/// Whatever the toast is given, the row gets the short form: a sidebar cell that
/// carried the whole of a question would push every tab name off the end of the
/// line beside it.
#[test]
fn a_waiting_row_carries_the_question_cut_to_the_sidebars_budget() {
    let waiting = |waiting_on: Option<&str>| {
        status_cell(
            AgentStatus::Running,
            Some(&(1, 0)),
            Some(&HookFeed {
                phase: HookPhase::Waiting,
                detail: None,
                waiting_on: waiting_on.map(str::to_owned),
                session_id: "s1".to_owned(),
                cwd: None,
            }),
        )
    };
    assert_eq!(waiting(None), "waiting");
    assert_eq!(waiting(Some("Bash cargo")), "waiting · Bash cargo");

    let long = waiting(Some("Which library should we use for date formatting?"));
    assert!(long.starts_with("waiting · Which library"), "{long}");
    assert!(long.ends_with('…'), "{long}");
    assert_eq!(
        long.chars().count(),
        "waiting · ".chars().count() + HOOK_DETAIL_CHARS
    );
}

/// `tool_input` is free-form text an agent's own input can steer, so the little
/// that is taken from it is cut short and stripped of anything that could move a
/// cursor or break a row.
#[test]
fn a_tool_hint_is_short_and_holds_no_control_characters() {
    let pre = |tool: &str, input: serde_json::Value| crate::wire::HookEvent {
        hook_event_name: "PreToolUse".to_owned(),
        tool_name: Some(tool.to_owned()),
        tool_input: Some(input),
        ..Default::default()
    };

    assert_eq!(
        hook_detail(&pre(
            "Bash",
            json!({"command": "cargo x test -p agent-proto"})
        )),
        Some("Bash cargo".to_owned()),
        "the program, not the command line"
    );
    assert_eq!(
        hook_detail(&pre(
            "Edit",
            json!({"file_path": "/work/agent/src/main.rs"})
        )),
        Some("Edit main.rs".to_owned())
    );
    assert_eq!(
        hook_detail(&pre("WebFetch", json!({"url": "https://example/x"}))),
        Some("WebFetch".to_owned()),
        "a tool with no obvious argument is named on its own"
    );

    let nasty = hook_detail(&pre(
        "Bash",
        json!({"command": "\u{1b}[2Jrm-rf-and-a-very-long-argument-indeed"}),
    ))
    .expect("a hint");
    assert!(
        !nasty.chars().any(char::is_control),
        "no escape sequence survives: {nasty:?}"
    );
    assert!(nasty.chars().count() <= 5 + HOOK_DETAIL_CHARS, "{nasty:?}");

    let exact = "abcdefghijkl mnopqrst uvwx";
    assert_eq!(
        clean_hint(exact),
        Some("abcdefghijklmnopqrstuvwx".to_owned())
    );
    assert_eq!(
        clean_hint(&format!("{exact}0")),
        Some("abcdefghijklmnopqrstuvw…".to_owned())
    );

    // Only a tool *starting* has a hint; anything else clears it.
    assert_eq!(
        hook_detail(&crate::wire::HookEvent {
            hook_event_name: "PostToolUse".to_owned(),
            tool_name: Some("Bash".to_owned()),
            ..Default::default()
        }),
        None
    );
}

/// The `!` marker, and the two rules behind it. A summary describes one screen and
/// outlives it, so it is believed only while the pane still shows that screen. A
/// feed has no screen behind it - it is the state - so it is believed outright.
#[test]
fn the_waiting_marker_believes_a_feed_outright_and_a_summary_only_while_it_is_fresh() {
    let panes = vec![pane(3, "claude", Some("claude"))];
    let owners = owners(&[]);
    let branches = branches(&[]);

    // Waiting, with a screen that is still moving and no summary anywhere.
    let rows = strip_rows(
        &panes,
        &manual(&[]),
        &Activity::from([(3, (9, 0))]),
        &owners,
        &branches,
        &Summaries::new(),
        &feeds(&[(3, HookPhase::Waiting)]),
    );
    assert!(
        matches!(&rows[2], StripRow::Agent { text, .. } if text.ends_with(" !")),
        "{rows:?}"
    );

    // The same pane working: no marker, whatever a stale summary once said.
    let rows = strip_rows(
        &panes,
        &manual(&[]),
        &Activity::from([(3, (1, 4))]),
        &owners,
        &branches,
        &summaries(&[(3, true)]),
        &feeds(&[(3, HookPhase::Busy)]),
    );
    assert!(
        matches!(&rows[2], StripRow::Agent { text, .. } if !text.ends_with(" !")),
        "a feed outranks the summary beside it: {rows:?}"
    );
}

/// A pane can be blocked on the user before anything has been written about it,
/// which the summary path could never say - it only ever answered where a summary
/// existed, and gated that on the screen not having moved since.
#[test]
fn list_json_reports_a_waiting_agent_that_has_no_summary_yet() {
    let json = list_json(
        &[pane(3, "claude", Some("claude"))],
        &manual(&[]),
        &Activity::from([(3, (9, 0))]),
        &owners(&[]),
        &branches(&[]),
        &Summaries::new(),
        &feeds(&[(3, HookPhase::Waiting)]),
        |_| String::new(),
    );
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value[0]["awaiting_confirmation"], true);
    assert!(value[0].get("summary").is_none());
}

#[test]
fn automatic_permissions_have_no_waiting_marker() {
    for kind in ["claude", "codex"] {
        let event = crate::wire::HookEvent {
            permission_mode: Some("auto".to_owned()),
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(json!({"command": "git status"})),
            ..event("PermissionRequest")
        };
        let crate::agent::HookOutcome::Reported(record) = crate::agent::hook_outcome(kind, &event)
        else {
            panic!("permission request must report a record");
        };
        let hooks = HookFeeds::from([(3, HookFeed::from_agent(&record).unwrap())]);
        assert_eq!(hook_phase(kind, &event), Some(HookPhase::Busy));
        assert_eq!(
            status_cell(AgentStatus::Running, None, hooks.get(&3)),
            "busy · Bash git"
        );
        assert!(!awaiting_confirmation(
            &Summaries::new(),
            &Activity::new(),
            &hooks,
            AgentStatus::Running,
            3
        ));
        let json = list_json(
            &[pane(3, kind, Some(kind))],
            &manual(&[]),
            &Activity::new(),
            &owners(&[]),
            &branches(&[]),
            &Summaries::new(),
            &hooks,
            |_| String::new(),
        );
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value[0]["state"], "busy · Bash git");
        assert_eq!(value[0]["awaiting_confirmation"], false);
    }
}

#[test]
fn a_non_running_pane_does_not_wait_on_a_stale_feed() {
    let mut held = pane(3, "claude", Some("claude"));
    held.status = AgentStatus::Held;
    let panes = [held];
    let hooks = feeds(&[(3, HookPhase::Waiting)]);
    let rows = strip_rows(
        &panes,
        &manual(&[]),
        &Activity::new(),
        &owners(&[]),
        &branches(&[]),
        &Summaries::new(),
        &hooks,
    );
    assert!(matches!(&rows[2], StripRow::Agent { text, .. } if !text.ends_with(" !")));

    let json = list_json(
        &panes,
        &manual(&[]),
        &Activity::new(),
        &owners(&[]),
        &branches(&[]),
        &Summaries::new(),
        &hooks,
        |_| String::new(),
    );
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value[0]["state"], "held");
    assert_eq!(value[0]["awaiting_confirmation"], false);
}

#[test]
fn mention_context_resolves_ids_and_kinds_and_skips_what_it_cannot() {
    let tabbed = AgentPane {
        tab: Some((TabId::new(9), "work".to_owned())),
        ..pane(3, "codex", Some("codex"))
    };
    let panes = [tabbed, pane(5, "zsh", Some("zsh"))];
    let kinds = manual(&[(5, "reviewer")]);
    let activity = Activity::from([(3, (1, 24)), (5, (1, 0))]);

    assert_eq!(
        mention_context(
            "ask @codex and @5, please",
            &panes,
            &kinds,
            &activity,
            &no_hooks()
        ),
        Some(
            "[@codex is agent pane 3 (codex, idle 2m) on tab \"work\"]\n\
             [@5 is agent pane 5 (reviewer, busy)]"
                .to_owned()
        ),
    );
    // An id mention, a duplicate, and punctuation after the body.
    assert_eq!(
        mention_context("@3 @codex again", &panes, &kinds, &activity, &no_hooks(),),
        Some("[@3 is agent pane 3 (codex, idle 2m) on tab \"work\"]".to_owned()),
    );
    // Unresolved kinds, non-agent panes, embedded @ and bare @ say nothing.
    assert_eq!(
        mention_context("@aider @7 a@b @", &panes, &kinds, &activity, &no_hooks(),),
        None
    );
    assert_eq!(
        mention_context("no mentions here", &panes, &kinds, &activity, &no_hooks(),),
        None
    );
}

#[test]
fn mention_candidates_are_each_kind_once_then_every_pane_id() {
    let panes = [
        pane(3, "codex", Some("codex")),
        pane(4, "codex", Some("codex")),
        pane(5, "zsh", Some("zsh")),
    ];
    assert_eq!(
        mention_candidates(&panes, &manual(&[(5, "reviewer")])),
        vec!["codex", "reviewer", "3", "4", "5"],
    );
}

#[test]
fn strip_rows_omit_tab_titles() {
    let tabbed = AgentPane {
        tab: Some((TabId::new(9), "work".to_owned())),
        ..pane(3, "codex", Some("codex"))
    };
    let rows = strip_rows(
        &[tabbed, pane(5, "aider", Some("aider"))],
        &manual(&[]),
        &Activity::new(),
        &owners(&[]),
        &branches(&[]),
        &Summaries::new(),
        &no_hooks(),
    );
    // Nothing to group by, and the headers say so rather than going missing:
    // a row with no header above it would read as belonging to the one above
    // that.
    assert_eq!(
        rows,
        vec![
            project("no project"),
            branch("no branch"),
            agent(3, "codex", "3 CDX ?"),
            agent(5, "aider", "5 AID ?"),
        ],
    );
}

#[test]
fn list_json_carries_the_tab_when_the_pane_has_one() {
    let tabbed = AgentPane {
        tab: Some((TabId::new(9), "work".to_owned())),
        ..pane(3, "codex", Some("codex"))
    };
    let json = list_json(
        &[tabbed],
        &manual(&[]),
        &Activity::new(),
        &owners(&[]),
        &branches(&[]),
        &Summaries::new(),
        &no_hooks(),
        |_| String::new(),
    );
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value[0]["tab_id"], 9);
    assert_eq!(value[0]["tab"], "work");
}

#[test]
fn list_json_reads_each_agent_once_and_no_other_pane() {
    let panes = vec![
        pane(3, "codex", Some("codex")),
        pane(4, "zsh", Some("zsh")),
        AgentPane {
            status: AgentStatus::Held,
            ..pane(5, "shell", Some("zsh"))
        },
    ];
    let mut reads = Vec::new();
    // Pane 5 has activity too, but it is held, so no idle_seconds for it.
    let activity = Activity::from([(3, (1, 24)), (5, (1, 2))]);
    let json = list_json(
        &panes,
        &manual(&[(5, "reviewer")]),
        &activity,
        &owners(&[]),
        &branches(&[]),
        &Summaries::new(),
        &no_hooks(),
        |pane_id| {
            reads.push(pane_id);
            "ready".to_owned()
        },
    );

    assert_eq!(reads, vec![3, 5]);
    assert_eq!(
        serde_json::from_str::<Value>(&json).unwrap(),
        serde_json::json!([
            {"idle_seconds":120,"kind":"codex","pane_id":3,"preview":"ready","source":"auto","status":"running","title":"codex"},
            {"kind":"reviewer","pane_id":5,"preview":"ready","source":"manual","status":"held","title":"shell"}
        ]),
    );
}

fn in_folder(pane_id: u32, kind: &str, cwd: &str, tab_id: TabId) -> AgentPane {
    AgentPane {
        cwd: Some(cwd.to_owned()),
        tab: Some((tab_id, format!("tab {tab_id}"))),
        ..pane(pane_id, kind, Some(kind))
    }
}

#[test]
fn list_json_carries_the_folder_the_project_and_the_branch() {
    let pane = in_folder(3, "codex", "/work/agent/src", TabId::new(9));
    let json = list_json(
        &[pane],
        &manual(&[]),
        &Activity::new(),
        &owners(&[(3, "agent")]),
        &branches(&[(3, "topic")]),
        &Summaries::new(),
        &no_hooks(),
        |_| String::new(),
    );

    assert_eq!(
        serde_json::from_str::<Value>(&json).unwrap(),
        serde_json::json!([{"branch":"topic","cwd":"/work/agent/src","kind":"codex","pane_id":3,"preview":"","project":"agent","source":"auto","status":"running","tab":"tab 9","tab_id":9,"title":"codex"}]),
    );
}

#[test]
fn strip_rows_group_by_activity_before_project_and_follow_status_changes() {
    let panes: Vec<_> = (1..=6).map(|id| pane(id, "codex", Some("codex"))).collect();
    let owners = owners(&[(1, "alpha"), (2, "alpha"), (3, "zeta"), (4, "zeta")]);
    let branches = branches(&[(1, "main"), (2, "main"), (3, "topic"), (4, "topic")]);
    let activity = Activity::from([
        (1, (1, 2)),
        (2, (1, 0)),
        (3, (1, 2)),
        (4, (1, 2)),
        (5, (1, 0)),
    ]);
    let mut hooks = feeds(&[(3, HookPhase::Waiting), (4, HookPhase::Busy)]);
    let rows_for = |hooks: &HookFeeds| {
        strip_rows(
            &panes,
            &manual(&[]),
            &activity,
            &owners,
            &branches,
            &Summaries::new(),
            hooks,
        )
    };
    assert_eq!(
        rows_for(&hooks),
        vec![
            project("zeta"),
            branch("topic"),
            waiting(3, "codex◆", "3 CDX !"),
            project("alpha"),
            branch("main"),
            agent(2, "codex", "2 CDX ●"),
            project("zeta"),
            branch("topic"),
            agent(4, "codex◆", "4 CDX ●"),
            project("no project"),
            branch("no branch"),
            agent(5, "codex", "5 CDX ●"),
            project("alpha"),
            branch("main"),
            agent(1, "codex", "1 CDX ○"),
            project("no project"),
            branch("no branch"),
            agent(6, "codex", "6 CDX ?"),
        ]
    );
    hooks = feeds(&[(3, HookPhase::Idle), (4, HookPhase::Waiting)]);
    let ids: Vec<_> = rows_for(&hooks)
        .into_iter()
        .filter_map(|row| match row {
            StripRow::Agent { pane_id, .. } => Some(pane_id),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec![4, 2, 5, 1, 3, 6]);
}

#[test]
fn strip_rows_sort_by_last_hook_within_activity_groups_before_project() {
    let mut panes: Vec<_> = (1..=7).map(|id| pane(id, "codex", Some("codex"))).collect();
    for (pane, timestamp) in panes.iter_mut().zip([
        Some(10),
        Some(100),
        Some(200),
        Some(30),
        Some(20),
        Some(0),
        None,
    ]) {
        pane.last_hook_at_ms = timestamp;
    }
    let hooks = feeds(&[
        (1, HookPhase::Waiting),
        (2, HookPhase::Busy),
        (3, HookPhase::Idle),
        (4, HookPhase::Waiting),
        (5, HookPhase::Waiting),
        (6, HookPhase::Waiting),
        (7, HookPhase::Waiting),
    ]);
    let ids = |panes: &[AgentPane]| {
        strip_rows(
            panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[(1, "alpha"), (4, "zeta")]),
            &BTreeMap::new(),
            &Summaries::new(),
            &hooks,
        )
        .into_iter()
        .filter_map(|row| match row {
            StripRow::Agent { pane_id, .. } => Some(pane_id),
            _ => None,
        })
        .collect::<Vec<_>>()
    };
    assert_eq!(ids(&panes), vec![4, 5, 1, 6, 7, 2, 3]);
    // Another hook moves the agent even when its phase has not changed.
    panes[0].last_hook_at_ms = Some(40);
    assert_eq!(ids(&panes), vec![1, 4, 5, 6, 7, 2, 3]);
}

#[test]
fn strip_rows_break_equal_hook_timestamps_by_project_branch_and_pane() {
    let mut panes: Vec<_> = (1..=6)
        .rev()
        .map(|id| pane(id, "codex", Some("codex")))
        .collect();
    for pane in &mut panes {
        pane.last_hook_at_ms = Some(100);
    }
    assert_eq!(
        strip_rows(
            &panes,
            &manual(&[]),
            &Activity::new(),
            &owners(&[
                (1, "zeta"),
                (2, "alpha"),
                (3, "alpha"),
                (4, "alpha"),
                (6, "alpha")
            ]),
            &branches(&[(1, "main"), (2, "zeta"), (3, "alpha"), (4, "alpha")]),
            &Summaries::new(),
            &no_hooks(),
        ),
        vec![
            project("alpha"),
            branch("alpha"),
            agent(3, "codex", "3 CDX ?"),
            agent(4, "codex", "4 CDX ?"),
            branch("zeta"),
            agent(2, "codex", "2 CDX ?"),
            branch("no branch"),
            agent(6, "codex", "6 CDX ?"),
            project("zeta"),
            branch("main"),
            agent(1, "codex", "1 CDX ?"),
            project("no project"),
            branch("no branch"),
            agent(5, "codex", "5 CDX ?"),
        ]
    );
}

#[test]
fn strip_rows_group_agents_under_their_project() {
    let panes = vec![
        in_folder(3, "codex", "/work/pocus", TabId::new(1)),
        in_folder(4, "aider", "/elsewhere", TabId::new(1)),
        in_folder(5, "codex", "/work/agent", TabId::new(2)),
    ];
    let rows = strip_rows(
        &panes,
        &manual(&[]),
        &Activity::new(),
        &owners(&[(3, "pocus"), (5, "agent")]),
        &branches(&[(3, "main"), (5, "main")]),
        &Summaries::new(),
        &no_hooks(),
    );

    // Projects in name order with the unowned agent last, and no row
    // repeating what the headers above it already say.
    assert_eq!(
        rows,
        vec![
            project("agent"),
            branch("main"),
            agent(5, "codex", "5 CDX ?"),
            project("pocus"),
            branch("main"),
            agent(3, "codex", "3 CDX ?"),
            project("no project"),
            branch("no branch"),
            agent(4, "aider", "4 AID ?"),
        ],
    );
}

/// The case the branch header exists for: one project, two worktrees. The
/// project is named once and the agents of each checkout sit together.
#[test]
fn strip_rows_group_the_worktrees_of_one_project_by_branch() {
    let panes = vec![
        in_folder(3, "codex", "/work/agent", TabId::new(1)),
        in_folder(4, "claude", "/work/trees/topic", TabId::new(2)),
        in_folder(5, "aider", "/work/agent", TabId::new(1)),
    ];
    let rows = strip_rows(
        &panes,
        &manual(&[]),
        &Activity::new(),
        &owners(&[(3, "agent"), (4, "agent"), (5, "agent")]),
        &branches(&[(3, "main"), (4, "topic"), (5, "main")]),
        &Summaries::new(),
        &no_hooks(),
    );

    assert_eq!(
        rows,
        vec![
            project("agent"),
            branch("main"),
            agent(3, "codex", "3 CDX ?"),
            agent(5, "aider", "5 AID ?"),
            branch("topic"),
            agent(4, "claude", "4 CLA ?"),
        ],
    );
}

/// An agent on another machine has to say so on its own line. Which machine
/// you are about to send input to is not decoration — two agents in the same
/// folder on different hosts are doing different work.
#[test]
fn strip_rows_name_the_host_of_a_remote_agent() {
    let mut remote = pane(3, "", Some("claude"));
    remote.host = Some("build-box".to_owned());
    let local = pane(4, "", Some("claude"));

    let rows = strip_rows(
        &[remote, local],
        &manual(&[]),
        &Activity::new(),
        &owners(&[]),
        &branches(&[]),
        &summaries(&[]),
        &no_hooks(),
    );

    assert_eq!(
        rows,
        vec![
            project("no project"),
            branch("no branch"),
            agent(3, "claude", "3 CLA@build-box ?"),
            agent(4, "claude", "4 CLA ?"),
        ],
    );
}

/// A checkout nobody named still has a branch, and an agent in one is still
/// worth telling apart from an agent in the next.
#[test]
fn strip_rows_group_by_branch_without_a_project() {
    let panes = vec![
        in_folder(3, "codex", "/work/one", TabId::new(1)),
        in_folder(4, "codex", "/work/two", TabId::new(2)),
    ];
    let rows = strip_rows(
        &panes,
        &manual(&[]),
        &Activity::new(),
        &owners(&[]),
        &branches(&[(3, "main"), (4, "topic")]),
        &Summaries::new(),
        &no_hooks(),
    );

    assert_eq!(
        rows,
        vec![
            project("no project"),
            branch("main"),
            agent(3, "codex", "3 CDX ?"),
            branch("topic"),
            agent(4, "codex", "4 CDX ?"),
        ],
    );
}

/// An agent whose folder has not been asked about yet has no branch to
/// group under, and waits under the header that says so rather than joining
/// whichever branch happens to be nearby. The shape matters: this is what
/// the picker draws for the round trip between a pane appearing in a
/// worktree and Git answering about it, and it must not read as a claim.
#[test]
fn an_agent_whose_branch_is_not_known_yet_waits_under_its_own_header() {
    let panes = vec![
        in_folder(3, "codex", "/work/agent", TabId::new(1)),
        in_folder(
            4,
            "claude",
            "/work/agent/.claude/worktrees/topic",
            TabId::new(2),
        ),
    ];
    let rows = strip_rows(
        &panes,
        &manual(&[]),
        &Activity::new(),
        &owners(&[(3, "agent"), (4, "agent")]),
        // Pane 4's folder is in the project, but no probe has answered for
        // it yet.
        &branches(&[(3, "main")]),
        &Summaries::new(),
        &no_hooks(),
    );

    assert_eq!(
        rows,
        vec![
            project("agent"),
            branch("main"),
            agent(3, "codex", "3 CDX ?"),
            branch("no branch"),
            agent(4, "claude", "4 CLA ?"),
        ],
    );
}

#[test]
fn tab_projects_agree_only_when_one_project_owns_the_tab() {
    let untabbed = AgentPane {
        cwd: Some("/work/agent".to_owned()),
        ..pane(9, "codex", Some("codex"))
    };
    let panes = vec![
        // Tab 1 agrees, even though pane 4 has no known project.
        in_folder(3, "codex", "/work/agent", TabId::new(1)),
        in_folder(4, "aider", "/elsewhere", TabId::new(1)),
        // Tab 2 has two projects, so it agrees on nothing.
        in_folder(5, "codex", "/work/agent", TabId::new(2)),
        in_folder(6, "aider", "/work/pocus", TabId::new(2)),
        // Tab 3 has a shell, not an agent, and pane 9 is in no tab at all.
        in_folder(7, "zsh", "/work/agent", TabId::new(3)),
        untabbed,
    ];
    let owners = owners(&[
        (3, "agent"),
        (5, "agent"),
        (6, "pocus"),
        (7, "agent"),
        (9, "agent"),
    ]);

    assert_eq!(
        tab_projects(&panes, &manual(&[]), &owners),
        BTreeMap::from([(TabId::new(1), "agent".to_owned())]),
    );
}

#[test]
fn tab_project_labels_name_a_tab_after_every_project_in_it() {
    let untabbed = AgentPane {
        cwd: Some("/work/agent".to_owned()),
        ..pane(9, "codex", Some("codex"))
    };
    let panes = vec![
        // One project, named once however many agents are on it.
        in_folder(3, "codex", "/work/agent", TabId::new(1)),
        in_folder(4, "aider", "/work/agent", TabId::new(1)),
        // Two projects, sorted rather than left in pane order.
        in_folder(5, "codex", "/work/pocus", TabId::new(2)),
        in_folder(6, "aider", "/work/agent", TabId::new(2)),
        // A shell names its tab as readily as an agent does — which is the
        // whole point, since the ordinary session has no agent in it. Pane 9
        // is in no tab at all, and still names nothing.
        in_folder(7, "zsh", "/work/agent", TabId::new(3)),
        untabbed,
    ];
    let owners = owners(&[
        (3, "agent"),
        (4, "agent"),
        (5, "pocus"),
        (6, "agent"),
        (7, "agent"),
        (9, "agent"),
    ]);

    assert_eq!(
        tab_project_labels(
            &panes,
            &owners
                .iter()
                .map(|(id, name)| (*id, (name.clone(), None)))
                .collect()
        ),
        BTreeMap::from([
            (TabId::new(1), vec![("agent".to_owned(), None)]),
            (
                TabId::new(2),
                vec![("agent".to_owned(), None), ("pocus".to_owned(), None)]
            ),
            (TabId::new(3), vec![("agent".to_owned(), None)]),
        ]),
    );
}

/// The other half of the split from `tab_projects`, which counts only agents
/// because an assignment says whose *work* a tab is doing. A name says where
/// the tab is, and a shell answers that.
#[test]
fn only_the_label_counts_a_shell_the_assignment_does_not() {
    let panes = vec![in_folder(7, "zsh", "/work/agent", TabId::new(3))];
    let owners = owners(&[(7, "agent")]);

    assert_eq!(
        tab_project_labels(
            &panes,
            &owners
                .iter()
                .map(|(id, name)| (*id, (name.clone(), None)))
                .collect()
        ),
        BTreeMap::from([(TabId::new(3), vec![("agent".to_owned(), None)])]),
    );
    assert!(tab_projects(&panes, &manual(&[]), &owners).is_empty());
}

#[test]
fn all_projects_that_fit_are_named() {
    let named = |names: &[&str]| -> Vec<(String, Option<String>)> {
        names
            .iter()
            .map(|name| ((*name).to_owned(), None))
            .collect()
    };

    assert_eq!(tab_project_label(&named(&[])), None);
    assert_eq!(tab_project_label(&named(&["agent"])).unwrap(), "agent");
    assert_eq!(
        tab_project_label(&named(&["agent", "pocus"])).unwrap(),
        "agent + pocus"
    );
    assert_eq!(
        tab_project_label(&named(&["agent", "pocus", "focus", "locus"])).unwrap(),
        "agent + pocus + focus + locus"
    );
}

#[test]
fn branch_labels_are_sorted_and_deduplicated() {
    let panes = (1..=5)
        .map(|id| in_folder(id, "zsh", "/work", TabId::new(1)))
        .collect::<Vec<_>>();
    let owners = BTreeMap::from([
        (1, ("project".into(), Some("b".into()))),
        (2, ("project".into(), Some("a".into()))),
        (3, ("project".into(), Some("b".into()))),
        (4, ("other".into(), Some("main".into()))),
        (5, ("project".into(), None)),
    ]);
    let labels = tab_project_labels(&panes, &owners);
    assert_eq!(labels[&TabId::new(1)].len(), 4);
    assert_eq!(
        tab_project_label(&labels[&TabId::new(1)]).unwrap(),
        "other:main + project + a + b"
    );
    assert_eq!(tab_project_labels(panes.iter().rev(), &owners), labels);
}

#[test]
fn branch_summary_fits_complete_entries_and_reserves_the_count() {
    let cases = [
        (vec![("project", Some("a"))], "project:a".to_owned()),
        (
            vec![
                ("project", Some("a")),
                ("project", Some("b")),
                ("project", Some("c")),
            ],
            "project:a + b + c".into(),
        ),
        (
            vec![("project", Some("a")), ("other", Some("b"))],
            "project:a + other:b".into(),
        ),
        (
            vec![
                ("1234567890123456789012345678901234567", None),
                ("ab", None),
            ],
            "1234567890123456789012345678901234567 +1".into(),
        ),
        (
            vec![("123456789012345678901234567890123456", None), ("a", None)],
            "123456789012345678901234567890123456 + a".into(),
        ),
        (
            vec![("1234567890123456789012345678901234567890", None)],
            "1234567890123456789012345678901234567890".into(),
        ),
        (
            vec![("12345678901234567890123456789012345678901", None)],
            "123456789012345678901234567890123456789…".into(),
        ),
        (
            vec![
                ("12345678901234567890123456789012345678901", None),
                ("b", None),
                ("c", None),
            ],
            "123456789012345678901234567890123456… +2".into(),
        ),
        (
            vec![("日本語", Some("á")), ("日本語", Some("界"))],
            "日本語:á + 界".into(),
        ),
        (
            vec![
                ("日本語日本語日本語日本語日本語日本語日本語", None),
                ("z", None),
            ],
            format!("{}… +1", "日本語".repeat(6)),
        ),
    ];
    for (entries, expected) in cases {
        let entries = entries
            .into_iter()
            .map(|(p, b)| (p.to_owned(), b.map(str::to_owned)))
            .collect::<Vec<_>>();
        let label = tab_project_label(&entries).unwrap();
        assert_eq!(label, expected);
        assert!(label.width() <= 40);
    }
    let entries = vec![("x".repeat(50), None); 12];
    assert_eq!(
        tab_project_label(&entries).unwrap(),
        format!("{}… +11", "x".repeat(35))
    );
}

#[test]
fn a_tab_whose_agents_have_no_project_is_left_out() {
    let panes = vec![in_folder(3, "codex", "/elsewhere", TabId::new(1))];

    assert!(tab_project_labels(&panes, &BTreeMap::new()).is_empty());
}

#[test]
fn a_tab_that_two_projects_disagree_over_stays_unassigned() {
    // Pane 7 agrees with pane 5, but pane 6 already broke the tab. Order
    // must not be able to heal it.
    let panes = vec![
        in_folder(5, "codex", "/work/agent", TabId::new(1)),
        in_folder(6, "aider", "/work/pocus", TabId::new(1)),
        in_folder(7, "codex", "/work/agent", TabId::new(1)),
    ];
    let owners = owners(&[(5, "agent"), (6, "pocus"), (7, "agent")]);

    assert!(tab_projects(&panes, &manual(&[]), &owners).is_empty());
}

// -- the idle summary in the history half --------------------------------

/// History rows as they read, for the assertions that are about the words rather than
/// about how they are painted.
fn texts(lines: &[MdLine]) -> Vec<&str> {
    lines.iter().map(|line| line.text.as_str()).collect()
}

fn a_summary(text: &str, awaiting: bool) -> Summary {
    Summary {
        text: text.to_owned(),
        title: "running the tests".to_owned(),
        awaiting,
        hash: 1,
        created_at_ms: None,
    }
}

/// The model's words, wrapped to the half and nothing else: no frame, no ceiling. The
/// box this replaced cut at six rows because it had six rows to give; the half scrolls.
#[test]
fn an_idle_summary_is_the_model_s_words_wrapped_to_the_half() {
    let history = summary_history(
        7,
        "claude",
        &a_summary("Ran the tests, all green.", false),
        false,
        30,
        0,
    );
    assert_eq!(history.label, "7 · claude");
    assert_eq!(texts(&history.entries), ["Ran the tests, all green."]);

    let long = "word ".repeat(200);
    let long = summary_history(7, "claude", &a_summary(&long, false), false, 30, 0);
    assert!(
        long.entries.len() > 6,
        "every line of it, for the half to window, past the box's old ceiling of {}",
        long.entries.len()
    );
    // Uncapped, unlike a run's events: there is one of these and nothing beside it
    // competing for the room. It is still one event, so a cursor can open it in full.
    assert!(long.entries.len() > MAX_ENTRY_ROWS);
    assert_eq!(long.events.len(), 1);
    assert_eq!(long.events[0].start, 0);
    assert_eq!(long.events[0].len, long.entries.len());
    assert_eq!(long.events[0].kind, "summary");
}

/// One event gets four rows of the timeline and no more, the last of them saying how much
/// was left out. Before the cap a single talkative result filled the half and pushed
/// everything either side of it out of view, which is the opposite of what a timeline is
/// for.
#[test]
fn a_long_event_is_capped_at_four_rows_that_name_what_is_missing() {
    let mut run = a_pane_run();
    run.entries = vec![RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: RunEntryKind::Prompt,
        // Ten lines of it at this width, against the four the cap allows.
        text: "word ".repeat(50),
        created_at_ms: None,
    }];

    let history = run_history(&run, 30, 100, 0);
    assert_eq!(history.entries.len(), MAX_ENTRY_ROWS);
    // Three rows kept, and the marker speaks for everything from the fourth on.
    assert_eq!(
        history.entries.last().expect("four rows").text.trim(),
        CAPPED_MARKER
    );

    // The event keeps the whole of what it said, which is what makes the cap honest: the
    // pane opens this rather than the three rows it drew.
    assert_eq!(history.events.len(), 1);
    assert_eq!(&*history.events[0].text, run.entries[0].text);
    assert_eq!(history.events[0].kind, "prompt");
    assert_eq!(history.events[0].len, MAX_ENTRY_ROWS);
}

/// A run shaped like a real one: every kind the timeline draws, a harness prompt that is
/// drawn as its digest, an entry too long for the cap, entries with and without stamps, and
/// a rolling summary.
fn a_mixed_run() -> AgentRun {
    let mut run = a_pane_run();
    let entry = |revision: u64, kind: RunEntryKind, text: String, created_at_ms| RunEntry {
        tool: None,
        revision,
        event_id: None,
        kind,
        text,
        created_at_ms,
    };
    run.entries = vec![
        entry(
            1,
            RunEntryKind::Prompt,
            "fix the **flaky** test".to_owned(),
            Some(0),
        ),
        entry(2, RunEntryKind::Prompt, task_notification(3), Some(60_000)),
        entry(
            3,
            RunEntryKind::Plan,
            "- one\n- two\n\n# Then\n\ndone".to_owned(),
            None,
        ),
        entry(
            4,
            RunEntryKind::Message,
            "word ".repeat(80),
            Some(3_600_000),
        ),
        entry(
            5,
            RunEntryKind::Summary,
            "Fixed it.".to_owned(),
            Some(7_200_000),
        ),
    ];
    run.summary = Some(a_summary("Fixed it, and ran the suite.", false));
    run
}

/// Keeping the parse is only allowed to change what the half costs, never what it draws:
/// one `ParsedRun` walked through several widths draws exactly what a fresh `run_history`
/// draws at each of them, and a later minute re-reads every age.
#[test]
fn a_kept_parse_draws_what_a_fresh_one_draws_at_every_width() {
    let run = a_mixed_run();
    let mut parsed = ParsedRun::new(&run);
    for (width, now) in [(80, 100), (30, 100), (57, 90_000_000), (80, 100)] {
        assert_eq!(
            parsed.lines(width, now, 0, None),
            run_history(&run, width, now, 0),
            "width {width} at {now}"
        );
    }
    // Resizing and the clock re-wrap and re-stamp; they never parse.
    assert_eq!(parsed.parses, 6);
}

/// A run that grew by an entry costs the parse of that entry, and draws what the whole
/// run parsed afresh draws.
#[test]
fn a_grown_run_parses_only_what_is_new() {
    let mut run = a_mixed_run();
    let mut parsed = ParsedRun::new(&run);
    let _ = parsed.lines(60, 100, 0, None);
    let before = parsed.parses;

    run.entries.push(RunEntry {
        tool: None,
        revision: 6,
        event_id: None,
        kind: RunEntryKind::Question,
        text: "Shall I push?".to_owned(),
        created_at_ms: Some(8_000_000),
    });
    parsed.update(&run);
    assert_eq!(
        parsed.lines(60, 100, 0, None),
        run_history(&run, 60, 100, 0)
    );
    assert_eq!(parsed.parses - before, 1, "only the question");
}

/// A revision that comes back a different length is parsed again rather than drawn as what
/// it used to say - and so is a rolling summary that changed. Only the length is read of a
/// recorded entry (see `ParsedEvent::is`), so the rewrite here is deliberately not the
/// length it was.
#[test]
fn an_entry_that_changed_is_parsed_again() {
    let mut run = a_mixed_run();
    let mut parsed = ParsedRun::new(&run);
    let _ = parsed.lines(60, 100, 0, None);
    let before = parsed.parses;

    run.entries[0].text = "fix the *other* test".to_owned();
    assert_ne!(
        run.entries[0].text.len(),
        a_mixed_run().entries[0].text.len()
    );
    run.summary = Some(a_summary("Something else entirely.", true));
    parsed.update(&run);
    assert_eq!(
        parsed.lines(60, 100, 0, None),
        run_history(&run, 60, 100, 0)
    );
    assert_eq!(parsed.parses - before, 2);

    // And one the run dropped is dropped here too.
    run.entries.remove(1);
    parsed.update(&run);
    assert_eq!(
        parsed.lines(60, 100, 0, None),
        run_history(&run, 60, 100, 0)
    );
}

/// The rolling summary is rewritten in place, so it is recognised by what it says and not
/// by its length: one that came back the same size saying something else is parsed again.
#[test]
fn a_summary_rewritten_to_the_same_length_is_parsed_again() {
    let mut run = a_mixed_run();
    let mut parsed = ParsedRun::new(&run);
    let _ = parsed.lines(60, 100, 0, None);
    let before = parsed.parses;

    let old = run.summary.as_ref().expect("a summary").text.clone();
    let new = "Broke it, and ran the suite.";
    assert_eq!(old.len(), new.len());
    run.summary = Some(a_summary(new, false));
    parsed.update(&run);
    assert_eq!(
        parsed.lines(60, 100, 0, None),
        run_history(&run, 60, 100, 0)
    );
    assert_eq!(parsed.parses - before, 1, "only the summary");
}

/// Entries are recognised by revision, and revisions are only unique within a run, so a
/// parse handed another run starts over: its revision 1 is not this one's, however alike
/// the two look from outside.
#[test]
fn another_run_is_parsed_afresh_however_alike_its_revisions_are() {
    let run = a_mixed_run();
    let mut parsed = ParsedRun::new(&run);
    let _ = parsed.lines(60, 100, 0, None);

    let mut other = run.clone();
    other.id = "claude:session-2".to_owned();
    other.entries[0].text = "fix the **stale** test".to_owned();
    assert_eq!(other.entries[0].text.len(), run.entries[0].text.len());
    parsed.update(&other);
    assert_eq!(
        parsed.lines(60, 100, 0, None),
        run_history(&other, 60, 100, 0)
    );
    assert!(parsed
        .lines(60, 100, 0, None)
        .entries
        .iter()
        .any(|line| line.text.contains("stale")));
}

/// A stamp is read by the gutter and nothing else, so an entry whose stamp was corrected
/// keeps its parse - and is still drawn with the age the new stamp gives it.
#[test]
fn a_restamped_entry_is_not_parsed_again() {
    let mut run = a_mixed_run();
    let mut parsed = ParsedRun::new(&run);
    let _ = parsed.lines(60, 90_000_000, 0, None);
    let before = parsed.parses;

    // A new age, a stamp where there was none - which widens the gutter the rows were
    // wrapped beside - and the same for the rolling summary.
    run.entries[0].created_at_ms = Some(89_000_000);
    run.entries[2].created_at_ms = Some(60_000);
    run.summary.as_mut().expect("a summary").created_at_ms = Some(80_000_000);
    parsed.update(&run);
    assert_eq!(
        parsed.lines(60, 90_000_000, 0, None),
        run_history(&run, 60, 90_000_000, 0)
    );
    assert_eq!(parsed.parses, before);
}

/// A minute on, the timeline restamped in place is the one drawn afresh at that minute -
/// and one of another shape than the parse would draw is refused and left as it was.
#[test]
fn a_restamped_timeline_is_the_one_drawn_at_the_new_minute() {
    let run = a_mixed_run();
    let mut parsed = ParsedRun::new(&run);
    let mut lines = parsed.lines(60, 100, 0, None);
    for now in [90_000_000, 90_060_000, 200_000_000, 200_000_000] {
        assert!(parsed.restamp(&mut lines, now), "at {now}");
        assert_eq!(lines, run_history(&run, 60, now, 0), "at {now}");
    }

    let mut other = HistoryLines::default();
    assert!(!parsed.restamp(&mut other, 5));
    assert_eq!(other, HistoryLines::default());
}

/// The timeline's parse is handed to a reader only where it is the whole of the event's
/// text - and only for that very event, not another holding the same words.
#[test]
fn only_an_event_parsed_whole_shares_its_parse() {
    let run = a_mixed_run();
    let mut parsed = ParsedRun::new(&run);
    let lines = parsed.lines(60, 100, 0, None);
    let options = crate::markdown::Options::compact();

    let blocks = parsed
        .whole_blocks(&lines.events[0].text)
        .expect("a short prompt is parsed whole");
    assert_eq!(
        crate::markdown::wrap(&blocks, 40, options),
        crate::markdown::render(&run.entries[0].text, 40, options)
    );
    assert!(
        parsed.whole_blocks(&lines.events[1].text).is_none(),
        "a prompt drawn as its digest"
    );
    assert!(
        parsed
            .whole_blocks(&Arc::from(&*lines.events[0].text))
            .is_none(),
        "the same words, somewhere else"
    );

    let mut long = a_pane_run();
    long.entries = vec![RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: RunEntryKind::Message,
        text: "word ".repeat(ENTRY_HEAD_BYTES),
        created_at_ms: None,
    }];
    let mut parsed = ParsedRun::new(&long);
    let lines = parsed.lines(60, 100, 0, None);
    assert!(
        parsed.whole_blocks(&lines.events[0].text).is_none(),
        "an event parsed only in part"
    );
}

/// An entry that is one enormous block - a paragraph, a code fence, a table - is parsed
/// and wrapped only as far as the timeline draws it, and is still capped and still opens
/// whole. The cap used to stop only between blocks, so an entry like these was wrapped in
/// full on every resize step and then cut to four rows.
#[test]
fn one_enormous_block_is_parsed_only_as_far_as_its_head() {
    let table = std::iter::once("| id | what |\n|---|---|".to_owned())
        .chain((0..4_000).map(|n| format!("| {n} | {} |", "cell ".repeat(4))))
        .collect::<Vec<_>>()
        .join("\n");
    let fence = format!(
        "```\n{}\n```",
        (0..5_000)
            .map(|n| format!("let line_{n} = {n};"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    for (shape, text) in [
        ("paragraph", "word ".repeat(20_000)),
        ("fence", fence),
        ("table", table),
    ] {
        assert!(text.len() >= 100_000, "{shape} is {} bytes", text.len());
        let mut run = a_pane_run();
        run.entries = vec![RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Message,
            text: text.clone(),
            created_at_ms: None,
        }];

        let parsed = ParsedRun::new(&run);
        assert!(
            parsed.parsed_chars() <= ENTRY_HEAD_BYTES,
            "{shape} kept {} characters",
            parsed.parsed_chars()
        );

        let history = run_history(&run, 60, 100, 0);
        assert_eq!(history.entries.len(), MAX_ENTRY_ROWS, "{shape}");
        assert_eq!(
            history.entries.last().expect("four rows").text.trim(),
            CAPPED_MARKER,
            "{shape}"
        );
        assert_eq!(&*history.events[0].text, text, "{shape} opens whole");
    }
}

/// An event parsed only in part says there is more even when its head fits: what was
/// parsed is not all it said. Whole, the same rows are left alone.
#[test]
fn a_partly_parsed_event_is_capped_however_short_its_head() {
    let rows = vec![MdLine::plain("one"), MdLine::plain("two")];
    let whole = cap_rows(rows.clone(), 2, true);
    assert_eq!(whole, rows);

    let part = cap_rows(rows, 2, false);
    assert_eq!(part.len(), 3);
    assert_eq!(part[..2], whole[..]);
    assert_eq!(part[2].text.trim(), CAPPED_MARKER);
}

/// A head is only ever a saving: on text short enough to be parsed whole, the timeline is
/// the one the whole parse draws, at every width.
#[test]
fn a_parsed_head_draws_what_the_whole_parse_draws() {
    let run = a_mixed_run();
    for width in [20, 30, 57, 80] {
        let Some(inner) = wrap_width(width) else {
            continue;
        };
        for entry in &run.entries {
            let whole = crate::markdown::parse(&entry.text);
            let (head, complete) =
                crate::markdown::parse_head(&entry.text, ENTRY_HEAD_BLOCKS, ENTRY_HEAD_BYTES);
            let draw = |blocks: &[crate::markdown::MdBlock], complete| {
                cap_rows(
                    labelled_blocks(blocks, "message", 3, inner, LABEL_STYLE).0,
                    3,
                    complete,
                )
            };
            assert_eq!(
                draw(&head, complete),
                draw(&whole, true),
                "{:?} at {width}",
                entry.text
            );
        }
    }
}

/// An event that fits is left alone: the marker costs a row, so spending one to say "one
/// more line" would show less than saying nothing.
#[test]
fn an_event_that_fits_the_cap_grows_no_marker() {
    let mut run = a_pane_run();
    run.entries = vec![RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: RunEntryKind::Prompt,
        // Four list items fill the cap without a label row.
        text: "- one\n- two\n- three\n- four".to_owned(),
        created_at_ms: None,
    }];
    let history = run_history(&run, 40, 100, 0);
    assert_eq!(history.entries.len(), MAX_ENTRY_ROWS);
    assert!(
        !history.entries.last().unwrap().text.contains('…'),
        "{:?}",
        texts(&history.entries)
    );
}

/// The spans tile the rows: every row of the half belongs to exactly one event, so a
/// cursor on an event and a window over the lines can be read against each other.
#[test]
fn the_event_spans_tile_the_rows_they_were_drawn_from() {
    let mut run = a_pane_run();
    run.summary = Some(a_summary("Ran the tests, all green.", false));
    run.entries = vec![
        RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text: "word ".repeat(50),
            created_at_ms: Some(40),
        },
        RunEntry {
            tool: None,
            revision: 2,
            event_id: None,
            kind: RunEntryKind::Question,
            text: "Ship it?".to_owned(),
            created_at_ms: None,
        },
    ];

    let history = run_history(&run, 40, 100, 0);
    assert_eq!(
        history.events.len(),
        3,
        "two entries and the rolling summary"
    );
    let mut at = 0;
    for event in &history.events {
        assert_eq!(event.start, at, "no gap and no overlap");
        assert!(event.len > 0 && event.len <= MAX_ENTRY_ROWS);
        at = event.end();
    }
    assert_eq!(at, history.entries.len(), "and nothing left over");

    // The stamp travels with the event, so a reader's divider can go on ticking.
    assert_eq!(history.events[0].created_at_ms, Some(40));
    assert_eq!(&*history.events[1].text, "Ship it?");
    assert_eq!(history.events[2].kind, "summary");
    assert_eq!(&*history.events[2].text, "Ran the tests, all green.");
}

/// Everything in the half is markdown by the time it gets here - a prompt is what the user
/// typed, a result is a model's last message - so it is drawn as markdown rather than as
/// the asterisks and hashes it is spelled with.
#[test]
fn a_run_s_history_is_rendered_as_markdown() {
    let mut run = a_pane_run();
    run.summary = Some(a_summary("Ran the **tests**, all green.", false));
    run.entries = vec![RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: RunEntryKind::Prompt,
        text: "Do it:\n\n- one\n- two".to_owned(),
        created_at_ms: None,
    }];

    let history = run_history(&run, 40, 100, 0);
    assert_eq!(
        texts(&history.entries),
        [
            "                                Do it:",
            "                                • one",
            "                                • two",
            "    summary: Ran the tests, all green.",
        ],
        "the list is a list, the label rides on the prose above it, and the summary is the              newest row rather than a block above them - no asterisks in any of it"
    );

    // and the emphasis landed on the word rather than on the whole line
    let summary = history.entries.last().unwrap();
    let emphasised: String = summary
        .text
        .chars()
        .zip(&summary.styles)
        .filter(|(_, style)| style.bold)
        .map(|(ch, _)| ch)
        .collect();
    assert_eq!(
        emphasised, "summary: tests",
        "the model's emphasis, and the label that marks the row as a summary"
    );
    assert!(
        summary.styles[AGE_WIDTH..AGE_WIDTH + 8]
            .iter()
            .all(|style| !style.dim),
        "the summary's label is the one piece of furniture that is not dimmed"
    );
}

/// Outgoing lists keep their markers and align together without a label row.
#[test]
fn outgoing_lists_keep_their_markers() {
    let mut run = a_pane_run();
    run.summary = None;
    run.entries = vec![
        RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text: "1. one\n2. two".to_owned(),
            created_at_ms: None,
        },
        RunEntry {
            tool: None,
            revision: 2,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text: "- one\n- two".to_owned(),
            created_at_ms: None,
        },
    ];

    assert_eq!(
        texts(&run_history(&run, 40, 100, 0).entries),
        [
            "                                1. one", // the numbering kept, and its column with it
            "                                2. two",
            "                                 • one",
            "                                 • two",
        ],
    );
}

/// A summary opening with a marker gives the label a row of its own, the same as any other
/// entry: `summary: - ran the tests` is not a bullet, and gluing the label onto the marker
/// would take the column its continuation rows are indented to.
#[test]
fn a_summary_that_opens_with_a_marker_gives_the_label_a_row() {
    let mut run = a_pane_run();
    run.summary = Some(a_summary("- ran the tests", false));
    let history = run_history(&run, 40, 100, 0);
    assert_eq!(
        texts(&history.entries),
        ["    summary:", "    • ran the tests"]
    );

    run.summary = Some(a_summary("ran the tests", false));
    let history = run_history(&run, 40, 100, 0);
    assert_eq!(texts(&history.entries), ["    summary: ran the tests"]);
}

/// The rolling summary is the newest row of the timeline rather than a block above it, and
/// it scrolls away with everything else - which is the whole point of unpinning it.
#[test]
fn the_rolling_summary_is_the_newest_row_of_the_timeline() {
    let mut run = a_pane_run();
    run.summary = Some(a_summary("Ran the tests.", false));
    run.entries = vec![
        RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text: "run the tests".to_owned(),
            created_at_ms: None,
        },
        RunEntry {
            tool: None,
            revision: 2,
            event_id: None,
            kind: RunEntryKind::Question,
            text: "which ones?".to_owned(),
            created_at_ms: None,
        },
    ];
    assert_eq!(
        texts(&run_history(&run, 40, 100, 0).entries),
        [
            "                         run the tests",
            "    question: which ones?",
            "    summary: Ran the tests.",
        ],
    );
}

/// A summary somebody asked for is both the newest entry and the rolling one, and saying it
/// twice under itself reads as a bug rather than as a history.
#[test]
fn a_rolling_summary_that_repeats_the_newest_filed_one_is_drawn_once() {
    let mut run = a_pane_run();
    run.summary = Some(a_summary("  Ran the tests.  ", false));
    run.entries = vec![RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: RunEntryKind::Summary,
        // What `push_summary_entry` files: the trimmed text of the same summary.
        text: "Ran the tests.".to_owned(),
        created_at_ms: None,
    }];
    assert_eq!(
        texts(&run_history(&run, 60, 100, 0).entries),
        ["    summary: Ran the tests."],
    );

    // A summary that has moved on since is a second row, where it was made.
    run.summary = Some(a_summary("Ran the tests and shipped it.", false));
    assert_eq!(
        texts(&run_history(&run, 60, 100, 0).entries),
        [
            "    summary: Ran the tests.",
            "    summary: Ran the tests and shipped it."
        ],
    );
}

/// The half has eight rows on a normal pane, so a paragraph break costs an eighth of what
/// there is to read. The conversation, with the whole pane, keeps its air.
#[test]
fn the_half_draws_the_blocks_hard_against_each_other() {
    let mut run = a_pane_run();
    run.summary = None;
    run.entries = vec![RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: RunEntryKind::Prompt,
        text: "first\n\nsecond".to_owned(),
        created_at_ms: None,
    }];
    assert_eq!(
        texts(&run_history(&run, 40, 100, 0).entries),
        [
            "                                first",
            "                                second"
        ]
    );
}

/// A result that opens with a bullet or a heading is still one: splicing `result: ` into
/// the source would have made it neither, so the label takes a line of its own instead.
#[test]
fn a_label_never_swallows_the_block_it_names() {
    let mut run = a_pane_run();
    run.summary = None;
    run.outcome = RunOutcome::Completed;
    run.summarized_revision = 0;
    run.entries = vec![RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: RunEntryKind::Result,
        text: "- one\n- two".to_owned(),
        created_at_ms: None,
    }];
    assert_eq!(
        texts(&run_history(&run, 40, 100, 0).entries),
        ["    result:", "    • one", "    • two"]
    );
}

/// The label shares the first row with the prose, and the host truncates a line rather
/// than wrapping it - so wrapping to the whole half and *then* putting the label on would
/// cut the tail off the first row of every entry in the pane.
#[test]
fn no_history_row_is_wider_than_the_half() {
    let mut run = a_pane_run();
    run.outcome = RunOutcome::Completed;
    run.summarized_revision = 0;
    run.summary = Some(a_summary(
        "Moved the renderer into agent-proto so both panes draw one dialect.",
        false,
    ));
    run.entries = vec![RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: RunEntryKind::Result,
        text: "Every word of this sentence is short enough to fit but the whole of it is not"
            .to_owned(),
        created_at_ms: None,
    }];

    for width in [HISTORY_MIN_COLS + 2, 30, 46, 80] {
        let history = run_history(&run, width, 100, 0);
        let inner = width - 2;
        for line in &history.entries {
            assert!(line.text.width() <= inner, "{width}: {:?}", line.text);
        }
    }
}

/// One `AgentRun` to vary, the way the pane receives them.
/// The age is the first thing on the row, in a column of its own, and an entry with no
/// recorded time keeps that column clear rather than closing it up - so every label in
/// the half starts at the same place and the times can be read straight down.
#[test]
fn an_entry_wears_its_age_in_the_left_hand_column() {
    let mut run = a_pane_run();
    run.summary = None;
    let minute = 60_000;
    run.entries = vec![
        RunEntry {
            tool: None,
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text: "run the tests".to_owned(),
            created_at_ms: Some(100 * minute),
        },
        RunEntry {
            tool: None,
            revision: 2,
            event_id: None,
            kind: RunEntryKind::Question,
            text: "which ones?".to_owned(),
            created_at_ms: Some(105 * minute),
        },
        RunEntry {
            tool: None,
            revision: 3,
            event_id: None,
            kind: RunEntryKind::Plan,
            text: "all of them".to_owned(),
            // Recorded before there was a time to record.
            created_at_ms: None,
        },
    ];

    let history = run_history(&run, 60, 106 * minute, 0);
    assert_eq!(
        texts(&history.entries),
        [
            " 6m                                          run the tests",
            " 1m question: which ones?",
            "    plan: all of them",
        ],
    );
    assert!(
        history.entries.iter().skip(1).all(|line| line
            .text
            .char_indices()
            .nth(AGE_WIDTH)
            .is_some_and(|(_, ch)| ch.is_alphabetic())),
        "every label starts in the same column, whether its entry has a time or not"
    );
}

/// The age is painted to be read as a column rather than as more label: undimmed and in
/// the navigation colour, independent of the outgoing prompt color.
#[test]
fn the_age_is_the_undimmed_part_of_the_furniture() {
    let mut run = a_pane_run();
    run.summary = None;
    run.entries = vec![RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: RunEntryKind::Prompt,
        text: "run the tests".to_owned(),
        created_at_ms: Some(0),
    }];

    let history = run_history(&run, 60, 5 * 60_000, 0);
    let row = &history.entries[0];
    assert!(
        row.styles[..AGE_WIDTH]
            .iter()
            .all(|style| !style.dim && style.color == Some(1)),
        "the age"
    );
    assert!(
        row.styles[row.styles.len() - "run the tests".len()..]
            .iter()
            .all(|style| !style.dim && style.color == Some(0)),
        "the outgoing prompt uses its theme color"
    );
}

/// A rolling summary is a row of the timeline like any other, so it says how old it is
/// too - and says nothing where the daemon that made it predates the stamp.
#[test]
fn a_rolling_summary_wears_its_age_as_well() {
    let mut run = a_pane_run();
    run.summary = Some(Summary {
        created_at_ms: Some(0),
        ..a_summary("Ran the tests.", false)
    });
    assert_eq!(
        texts(&run_history(&run, 60, 3 * 3_600_000, 0).entries),
        [" 3h summary: Ran the tests."],
    );

    run.summary = Some(a_summary("Ran the tests.", false));
    assert_eq!(
        texts(&run_history(&run, 60, 3 * 3_600_000, 0).entries),
        ["    summary: Ran the tests."],
    );
}

/// The buckets, at the edges. Truncated rather than rounded: a row cares that an entry is
/// roughly this old, not exactly.
#[test]
fn short_age_counts_whole_minutes_hours_and_days() {
    let cases = [
        (0, "now"),
        (59_000, "now"),
        (60_000, "1m"),
        (3_599_000, "59m"),
        (3_600_000, "1h"),
        (86_399_000, "23h"),
        (86_400_000, "1d"),
        (10 * 86_400_000, "10d"),
    ];
    for (elapsed, expected) in cases {
        assert_eq!(short_age(elapsed, 0), expected, "{elapsed}ms ago");
    }
    assert_eq!(
        short_age(0, 5_000),
        "now",
        "a clock that went backwards is not a negative age"
    );
}

/// The epoch, the two directions an offset can go, and the year boundaries the era shift
/// exists to get right.
#[test]
fn wall_clock_reads_an_epoch_millisecond_as_a_date_and_a_time() {
    let cases = [
        // The epoch itself, and the offsets either side of it.
        (0_i64, 0_i32, "1970-01-01 00:00:00 +00:00"),
        (0, 3600, "1970-01-01 01:00:00 +01:00"),
        (0, -18_000, "1969-12-31 19:00:00 -05:00"),
        // An offset that carries the local time over a day boundary in each direction.
        (86_399_000, 3600, "1970-01-02 00:59:59 +01:00"),
        (3_600_000, -18_000, "1969-12-31 20:00:00 -05:00"),
        // An offset that is not a whole hour, which is what the minutes field is for.
        (0, 20_700, "1970-01-01 05:45:00 +05:45"),
        // A leap day, and the day after it.
        (1_709_208_000_000, 0, "2024-02-29 12:00:00 +00:00"),
        (1_709_294_400_000, 0, "2024-03-01 12:00:00 +00:00"),
        // A non-leap century, which the /100 and /400 terms disagree about.
        (4_107_542_400_000, 0, "2100-03-01 00:00:00 +00:00"),
        // Well before the epoch, where truncating division would floor the wrong way.
        (-2_208_988_800_000, 0, "1900-01-01 00:00:00 +00:00"),
    ];
    for (epoch_ms, offset, expected) in cases {
        assert_eq!(
            wall_clock(yinz_tile::prelude::SignedUnixMillis::new(epoch_ms), offset),
            expected,
            "{epoch_ms}ms at {offset}s"
        );
    }
}

/// The seconds of the day, at both ends of it: a rendering that read the hour off a
/// truncating division would put midnight and noon in the same place on a pre-epoch date.
#[test]
fn wall_clock_keeps_the_time_of_day_inside_the_day() {
    for day in [-800_i64, -1, 0, 1, 20_000] {
        let midnight = day * 86_400_000;
        assert!(
            wall_clock(yinz_tile::prelude::SignedUnixMillis::new(midnight), 0)
                .ends_with("00:00:00 +00:00"),
            "day {day} did not start at midnight: {}",
            wall_clock(yinz_tile::prelude::SignedUnixMillis::new(midnight), 0)
        );
        assert!(
            wall_clock(
                yinz_tile::prelude::SignedUnixMillis::new(midnight + 86_399_000),
                0
            )
            .ends_with("23:59:59 +00:00"),
            "day {day} did not end a second before midnight: {}",
            wall_clock(
                yinz_tile::prelude::SignedUnixMillis::new(midnight + 86_399_000),
                0
            )
        );
    }
}

fn a_pane_run() -> AgentRun {
    AgentRun {
        id: "claude:session-1".to_owned(),
        pane_id: 3,
        yinz_session_name: "yinz".to_owned().into(),
        host_id: HostId::new(HOST),
        origin_run_id: None,
        previous_run_id: None,
        session_id: "session-1".to_owned(),
        kind: "claude".to_owned(),
        cwd: None,
        host: None,
        bubblewrap: None,
        started_at_ms: 10,
        ended_at_ms: Some(20),
        outcome: RunOutcome::Active,
        revision: 1,
        summarized_revision: 0,
        summary: None,
        entries: Vec::new(),
        omitted_entries: 0,
    }
}

/// The claim the strip's `!` makes, on the divider the user is already reading - and made
/// from the row's flag, so the two cannot disagree.
#[test]
fn a_blocked_agent_is_marked_on_the_divider() {
    let waiting = summary_history(7, "claude", &a_summary("Shall I?", true), true, 30, 0);
    assert_eq!(waiting.label, "7 · claude !");
    // An agent that merely finished is not.
    let done = summary_history(7, "claude", &a_summary("Shall I?", false), false, 30, 0);
    assert!(!done.label.contains('!'), "{}", done.label);
    // Nor is one whose stored summary still says it was waiting, once the row stopped
    // saying so: the user answered, and the screen the summary was about is gone.
    let answered = summary_history(7, "claude", &a_summary("Shall I?", true), false, 30, 0);
    assert!(!answered.label.contains('!'), "{}", answered.label);
}

/// Better nothing than a column of single words - the half draws a placeholder instead.
#[test]
fn there_is_no_history_where_there_is_no_room_for_one() {
    let one = a_summary("Ran the tests.", false);
    assert!(summary_history(7, "claude", &one, false, HISTORY_MIN_COLS + 1, 0).is_empty());
    assert!(
        !summary_history(7, "claude", &one, false, HISTORY_MIN_COLS + 2, 0).is_empty(),
        "just enough"
    );
}

/// A row can say something about the run an ancestor id points at without holding that run:
/// the archive is paged, so an origin is often not loaded, and the kind is the front of every
/// id `run_id` mints.
#[test]
fn a_run_id_names_the_kind_that_minted_it() {
    assert_eq!(
        run_id_kind("claude:9f3c1a7e-4b21-4d0a-8c55-7e2f6b1d90aa"),
        Some("claude"),
    );
    // The fallback shape, machine-named or not: the kind is still what leads.
    assert_eq!(run_id_kind("codex:pane:7:1764618000000"), Some("codex"));
    assert_eq!(
        run_id_kind("codex:pane:7:1764618000000:9f3c1a7e-4b21-4d0a-8c55-7e2f6b1d90aa"),
        Some("codex"),
    );
    // Neither shape, and a leading empty kind: nothing to name.
    assert_eq!(run_id_kind("whatever"), None);
    assert_eq!(run_id_kind(":session-1"), None);
    assert_eq!(run_id_kind(""), None);
    // Whatever `run_id` builds, it round-trips.
    for (kind, session) in [("claude", "abc"), ("codex", "")] {
        let id = run_id(kind, 7, session, 1764618000000, &HostId::new("host"));
        assert_eq!(run_id_kind(&id), Some(kind), "{id}");
    }
}

/// The divider names *which* run it is drawing, and a real id does not fit one: a hook
/// agent's is a uuid and an agent with no session id of its own is named by a raw epoch.
/// Both shapes are cut back to the part a person reads, so the divider is not truncated
/// into a meaningless prefix by `fit_label`.
#[test]
fn a_run_divider_names_a_run_a_person_can_read() {
    assert_eq!(
        short_run_id("claude:9f3c1a7e-4b21-4d0a-8c55-7e2f6b1d90aa"),
        "claude:9f3c1a7e",
    );
    assert_eq!(short_run_id("claude:pane:7:1764618000000"), "claude:pane:7");
    // The shape that names the machine too, which `splitn(4, ':')` reaches without being
    // taught there are two fallbacks: everything after the epoch lands in the part that is
    // thrown away.
    assert_eq!(
        short_run_id("claude:pane:7:1764618000000:9f3c1a7e-4b21-4d0a-8c55-7e2f6b1d90aa"),
        "claude:pane:7",
    );
    // Short enough to be read whole is left whole.
    assert_eq!(short_run_id("claude:session-1"), "claude:session-1");
    // Neither shape: not ours to shorten.
    assert_eq!(short_run_id("whatever"), "whatever");

    let mut run = a_pane_run();
    run.id = "claude:9f3c1a7e-4b21-4d0a-8c55-7e2f6b1d90aa".to_owned();
    let label = run_history(&run, 40, 100, 0).label;
    assert_eq!(label, "claude:9f3c1a7e");
    assert_eq!(
        fit_label(label.clone(), 40),
        label,
        "and a divider the width of the pane keeps all of it"
    );
}

/// The `!` outranks the age on a divider too narrow for both: how old the answer is can
/// wait, that the agent is blocked on you cannot.
#[test]
fn a_narrow_summary_divider_loses_the_age_before_the_marker() {
    let minute = 60_000;
    let mut summary = a_summary("Shall I?", true);
    summary.created_at_ms = Some(100 * minute);
    let label = summary_history(
        7,
        "claude",
        &summary,
        true,
        HISTORY_MIN_COLS + 2,
        105 * minute,
    )
    .label;
    assert_eq!(label, "7 · claude ! · 5m", "uncut where it is built");
    // A divider with less room than that - the scroll readout shares the row - loses the
    // age and keeps the marker, where before it kept the age and lost the marker.
    let cut = fit_label(label, 19);
    assert!(cut.contains('!'), "{cut}");
    assert!(!cut.contains("5m"), "{cut}");
}

/// [`age_gutter`] right-aligns into a fixed column and pads without truncating, so an
/// age wider than the column would push that row's label one place further in than
/// every other row's. `short_age` is what guarantees it cannot.
#[test]
fn an_age_never_outgrows_the_column_it_is_drawn_in() {
    let day = 86_400_000;
    for seconds in [0, 59, 60, 3_599, 3_600, 86_399, 86_400] {
        let age = short_age(seconds * 1000, 0);
        assert!(age.width() < AGE_WIDTH, "{age}");
    }
    for days in [1, 98, 99, 100, 365, 3_650] {
        let age = short_age(days * day, 0);
        assert!(age.width() < AGE_WIDTH, "{days} days: {age}");
    }
    // Past the ceiling the column stops counting rather than overflowing.
    assert_eq!(short_age(98 * day, 0), "98d");
    assert_eq!(short_age(99 * day, 0), "99d");
    assert_eq!(short_age(3_650 * day, 0), "99d");
}

/// A kind is up to 32 characters and this pane is half a panel wide, so the label can
/// be asked for more room than the divider has. It is cut rather than allowed to push
/// the rule off the row - but only by whoever draws the divider, which is the one place
/// that knows what else is on that row.
#[test]
fn a_label_too_wide_for_its_divider_is_cut_rather_than_burst() {
    let kind = "a-very-long-agent-kind-indeed-xx";
    let label = summary_history(7, kind, &a_summary("Done.", true), true, 26, 0).label;
    assert_eq!(label, format!("7 · {kind} !"), "uncut where it is built");

    let cut = fit_label(label, 26);
    assert_eq!(cut.width(), 22, "{cut}");
    assert!(kind.starts_with(&cut["7 · ".len()..]));

    // And a divider with no room for a label at all keeps the rule.
    assert!(fit_label("claude · active".to_owned(), 3).is_empty());
}

/// A hint line loses whole keys from its end, never half of one, and says when it did.
#[test]
fn a_hint_line_drops_whole_segments_from_the_end() {
    let segments = ["Enter focus", "/ search", "w waiting (0)", "s summary"];
    let whole = segments.join(" · ");
    assert_eq!(
        fit_hints(&segments, whole.width()),
        whole,
        "no marker when all fit"
    );
    assert_eq!(fit_hints(&segments, 200), whole);
    // Room for two keys but not for the marker as well: the keys win.
    assert_eq!(fit_hints(&segments, 24), "Enter focus · / search");
    // Room for both: the marker says there is more.
    assert_eq!(fit_hints(&segments, 31), "Enter focus · / search · ? keys");
    // Only a first segment too wide for the row alone is ever cut.
    assert_eq!(fit_hints(&segments, 5), "Enter");
    assert_eq!(fit_hints(&segments, 0), "");
    assert_eq!(fit_hints(&[], 10), "");
    // An empty segment is a key a caller had nothing to say about, not a doubled rule.
    assert_eq!(fit_hints(&["a", "", "b"], 10), "a · b");

    for width in 0..=whole.width() + 1 {
        let hint = fit_hints(&segments, width);
        assert!(hint.width() <= width, "{width}: {hint:?}");
        if width < segments[0].width() {
            assert!(segments[0].starts_with(&hint), "{width}: {hint:?}");
            continue;
        }
        let (shown, marked) = match hint.strip_suffix(" · ? keys") {
            Some(shown) => (shown, true),
            None => (hint.as_str(), false),
        };
        let parts: Vec<&str> = shown.split(" · ").collect();
        assert_eq!(
            parts,
            segments[..parts.len()],
            "{width}: whole, from the front"
        );
        let dropped = parts.len() < segments.len();
        let room = shown.width() + " · ? keys".width() <= width;
        assert_eq!(marked, dropped && room, "{width}: {hint:?}");
    }
}

/// A line that asks for it gives up its trailing keys to say `? keys`, down to the ones it
/// keeps - and a line that must not have the marker never gets it.
#[test]
fn a_marked_hint_line_gives_way_to_the_marker() {
    let segments = [
        "Enter focus",
        "s summary",
        "p previous",
        "w waiting (0)",
        "/ search",
        "Tab history",
    ];
    let whole = segments.join(" · ");
    // Room for two keys but not the marker after them: the second gives way to it, where
    // `fit_hints` would keep it.
    assert_eq!(fit_hints_marked(&segments, 24, 1), "Enter focus · ? keys");
    assert_eq!(fit_hints(&segments, 24), "Enter focus · s summary");
    // Room for the marker after both, which `fit_hints` already draws.
    assert_eq!(
        fit_hints_marked(&segments, 34, 1),
        "Enter focus · s summary · ? keys"
    );
    // Two kept: there is no giving way below them, so the line is what `fit_hints` draws.
    assert_eq!(
        fit_hints_marked(&segments, 24, 2),
        "Enter focus · s summary"
    );
    // No room for the marker after even the first key, and a first key cut to fit.
    assert_eq!(fit_hints_marked(&segments, 15, 1), "Enter focus");
    assert_eq!(fit_hints_marked(&segments, 5, 1), "Enter");
    // Everything fits: nothing to point at.
    assert_eq!(fit_hints_marked(&segments, whole.width(), 1), whole);
    assert_eq!(fit_hints_marked(&segments, 200, 3), whole);

    let join = |n: usize| segments[..n].join(" · ");
    for keep in 1..=3 {
        for width in 1..=whole.width() {
            let hint = fit_hints_marked(&segments, width, keep);
            assert!(hint.width() <= width, "{keep} {width}: {hint:?}");
            if width < segments[0].width() {
                assert_eq!(hint, fit_hints(&segments, width), "{keep} {width}");
                continue;
            }
            let (shown, marked) = match hint.strip_suffix(" · ? keys") {
                Some(shown) => (shown, true),
                None => (hint.as_str(), false),
            };
            let parts: Vec<&str> = shown.split(" · ").collect();
            assert_eq!(
                parts,
                segments[..parts.len()],
                "{keep} {width}: whole, from the front"
            );
            // What `fit_segments` keeps, and so what `fit_hints` would draw.
            let kept = (1..=segments.len())
                .rev()
                .find(|&n| join(n).width() <= width)
                .unwrap();
            let dropped = kept < segments.len();
            // Never below `keep` for the marker's sake - but where fewer than `keep` fit at
            // all, the line is `fit_hints`'s, marker and all.
            let floor = keep.min(kept);
            let room = join(floor).width() + " · ? keys".width() <= width;
            assert_eq!(marked, dropped && room, "{keep} {width}: {hint:?}");
            // And it gives up no more than the marker needs.
            let most = if marked {
                (floor..=kept)
                    .rev()
                    .find(|&n| join(n).width() + " · ? keys".width() <= width)
                    .unwrap()
            } else {
                kept
            };
            assert_eq!(parts.len(), most, "{keep} {width}: {hint:?}");
        }
    }

    // A status can hold the separator itself, and is still one segment: never split to
    // make room for the marker.
    let status = ["2 of 5 · waiting", "Enter", "Tab history"];
    assert_eq!(fit_hints_marked(&status, 24, 1), "2 of 5 · waiting · Enter");
    assert_eq!(
        fit_hints_marked(&status, 25, 1),
        "2 of 5 · waiting · ? keys"
    );

    // The unmarked line is the keys that fit and nothing else, however much was dropped.
    assert_eq!(fit_keys(&segments, 34), "Enter focus · s summary");
    assert_eq!(fit_keys(&segments, whole.width()), whole);
    for width in 0..=whole.width() + 1 {
        let keys = fit_keys(&segments, width);
        assert!(keys.width() <= width, "{width}: {keys:?}");
        assert!(!keys.ends_with("? keys"), "{width}: {keys:?}");
        assert!(whole.starts_with(&keys), "{width}: {keys:?}");
    }
}
// -- prompts that a harness wrote ----------------------------------------

/// A notification of the shape Claude Code actually submits, with `result` as long as the
/// caller asks for.
fn task_notification(result_lines: usize) -> String {
    format!(
        "<task-notification>\n\
         <task-id>a10fe836c6a9fa4ca</task-id>\n\
         <output-file>/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output</output-file>\n\
         <status>completed</status>\n\
         <summary>Agent \"/code-review high up to 3 subagents unstaged code\" finished</summary>\n\
         <note>A task-notification fires each time this agent stops.\n\
         The same task-id may notify more than once.</note>\n\
         <result>{}</result>\n\
         </task-notification>",
        "a line of the review\n".repeat(result_lines)
    )
}

fn run_of_prompts(prompts: &[&str]) -> AgentRun {
    let mut run = a_pane_run();
    run.entries = prompts
        .iter()
        .enumerate()
        .map(|(index, text)| RunEntry {
            tool: None,
            revision: index as u64 + 1,
            event_id: None,
            kind: RunEntryKind::Prompt,
            text: (*text).to_owned(),
            created_at_ms: Some(10),
        })
        .collect();
    run.revision = run.entries.len() as u64;
    run
}

/// The whole point, in one assertion: the rows say what happened rather than which tags it
/// arrived in.
#[test]
fn a_task_notification_on_the_timeline_reads_as_what_it_is() {
    let history = run_history(&run_of_prompts(&[&task_notification(3)]), 52, 10, 0);
    let rows = texts(&history.entries).join("\n");

    assert!(!rows.contains("prompt:"), "{rows}");
    assert!(rows.contains('✓'), "{rows}");
    assert!(rows.contains("a10fe836"), "{rows}");
    assert!(rows.contains("/code-review"), "{rows}");
    assert!(rows.contains("▸ result 3 lines"), "{rows}");
    // None of what made the row unreadable survives.
    assert!(!rows.contains('<'), "{rows}");
    assert!(!rows.contains("/private/tmp"), "{rows}");
    assert!(!rows.contains("notify more than once"), "{rows}");
}

/// The load-bearing one. `MAX_ENTRY_ROWS` is four, and the row that says what `Enter` would
/// show is the one worth protecting: `DIGEST_SUMMARY_COLS` is sized so `cap_rows` never
/// eats it at a width a half-pane actually has.
#[test]
fn the_digest_fits_the_rows_a_timeline_entry_gets() {
    for width in [HISTORY_MIN_COLS + 2, 30, 46, 52, 80] {
        let history = run_history(&run_of_prompts(&[&task_notification(400)]), width, 10, 0);
        assert_eq!(history.events.len(), 1, "at {width}");
        assert!(
            history.events[0].len <= MAX_ENTRY_ROWS,
            "{} rows at {width}",
            history.events[0].len
        );
        assert_eq!(history.entries.len(), history.events[0].len, "at {width}");
        if width >= 46 {
            let rows = texts(&history.entries).join("\n");
            assert!(
                rows.contains('▸'),
                "the affordances row was cut at {width}: {rows}"
            );
        }
    }
}

/// The same guarantee in a script that spends two columns to the character. Cut by
/// character rather than by column, this summary is twice as wide as the budget says: it
/// takes a third row, and `cap_rows` eats the affordances row to fit.
#[test]
fn a_digest_of_wide_characters_fits_the_same_rows() {
    let wide = format!(
        "<task-notification>\n\
         <task-id>a10fe836c6a9fa4ca</task-id>\n\
         <status>completed</status>\n\
         <summary>{}</summary>\n\
         <result>{}</result>\n\
         </task-notification>",
        "代理审查了这个补丁 ".repeat(6),
        "a line of the review\n".repeat(400),
    );
    for width in [HISTORY_MIN_COLS + 2, 30, 46, 52, 80] {
        let history = run_history(&run_of_prompts(&[&wide]), width, 10, 0);
        assert_eq!(history.events.len(), 1, "at {width}");
        assert!(
            history.events[0].len <= MAX_ENTRY_ROWS,
            "{} rows at {width}",
            history.events[0].len
        );
        if width >= 46 {
            let rows = texts(&history.entries).join("\n");
            assert!(
                rows.contains('▸'),
                "the affordances row was cut at {width}: {rows}"
            );
        }
    }
}

/// The invariant this design is most likely to break by accident, and the one that keeps it
/// a rendering change rather than a record change: the digest is drawn, the prompt is kept.
#[test]
fn the_history_event_behind_a_digest_still_carries_the_raw_prompt() {
    let raw = task_notification(3);
    let history = run_history(&run_of_prompts(&[&raw]), 52, 10, 0);
    assert_eq!(&*history.events[0].text, raw);
    assert_eq!(history.events[0].kind, "prompt");
}

/// Every row belongs to one event and no row to two — still true once an event's rows come
/// from somewhere other than its own text.
#[test]
fn an_events_spans_still_tile_the_entries() {
    let run = run_of_prompts(&[&task_notification(3), "plain prose", &task_notification(1)]);
    let history = run_history(&run, 52, 10, 0);
    let mut at = 0;
    for event in &history.events {
        assert_eq!(event.start, at, "{:?}", history.events);
        at = event.end();
    }
    assert_eq!(at, history.entries.len());
}

/// The regression guard that matters most: nothing a person typed goes anywhere near the
/// scanner's output.
#[test]
fn a_plain_prompt_keeps_its_prose_and_markdown() {
    let prose = "fix the flaky reconnect test\n\n- check the timeout\n- check the retry";
    let run = run_of_prompts(&[prose]);
    let history = run_history(&run, 52, 10, 0);
    assert_eq!(&*history.events[0].text, prose);

    assert_eq!(
        texts(&history.entries),
        [
            "now                   fix the flaky reconnect test",
            "                      • check the timeout",
            "                      • check the retry",
        ]
    );
}

/// Before this the tab bar spent a whole woken turn reading
/// `<task-notification> <task-id>a10fe836…`.
#[test]
fn a_pane_woken_by_a_task_is_named_for_the_task() {
    let title = prompt_title(&task_notification(3)).expect("a title");
    assert!(title.starts_with("task · Agent \"/code-review"), "{title}");
    assert!(!title.contains('<'), "{title}");

    // And a person's prompt is named exactly as it was.
    assert_eq!(
        prompt_title("fix the flaky test").as_deref(),
        Some("fix the flaky test")
    );
}

/// An archive row is a sentence now rather than the opening tag the first line of such a
/// prompt literally is.
#[test]
fn a_run_woken_by_a_task_is_titled_by_the_task_and_not_by_its_tags() {
    let title = run_title(&run_of_prompts(&[&task_notification(3)]));
    assert!(title.starts_with("task · Agent"), "{title}");
    assert!(!title.contains('<'), "{title}");
}

/// A run woken by a background task and later typed into by a person is about what the
/// person typed, wherever in the run they typed it.
#[test]
fn a_run_that_a_person_later_typed_into_is_titled_by_what_they_typed() {
    let run = run_of_prompts(&[
        &task_notification(3),
        "rebase onto main and rerun the suite",
    ]);
    assert_eq!(run_title(&run), "rebase onto main and rerun the suite");
}

fn prompt_event(prompt: &str) -> yinz_tile::agent::HookEvent {
    yinz_tile::agent::HookEvent {
        hook_event_name: "UserPromptSubmit".to_owned(),
        prompt: Some(prompt.to_owned()),
        ..Default::default()
    }
}

/// The id of a prompt, composed exactly as the ingest path composes it: the text the entry
/// will keep is what the id is taken from.
fn id_of(prompt: &str) -> Option<String> {
    let event = prompt_event(prompt);
    let (_, text) = run_entry(&event)?;
    run_event_id(&event, &text)
}

/// A prompt has no `tool_use_id`, so before this the ingest path could only compare one
/// against the entry immediately before it and a redelivery with anything in between landed
/// twice. A second notification for the same task still does, because it is a second thing
/// happening — see `envelope::task_id`.
#[test]
fn a_redelivered_notification_does_not_land_twice() {
    let first = id_of(&task_notification(3)).expect("an id");
    assert_eq!(id_of(&task_notification(3)), Some(first.clone()));
    assert!(
        first.starts_with("UserPromptSubmit:task-notification:"),
        "{first}"
    );
    assert_ne!(id_of(&task_notification(4)), Some(first));

    // A person's prompt keeps today's adjacent-only rule, which is the right one for it.
    assert_eq!(id_of("fix the flaky test"), None);
}

/// Reports that differ only beyond the retained prefix must not share a history id.
#[test]
fn a_notification_cut_by_the_broadcast_cap_gets_no_id() {
    let lines = yinz_tile::agent::HOOK_BROADCAST_MAX_BYTES / 10;
    let prune = |text: &str| {
        let event = prompt_event(text);
        event
            .broadcast(serde_json::to_string(&event).unwrap())
            .0
            .into_owned()
    };
    let (first, second) = (
        prune(&task_notification(lines)),
        prune(&task_notification(lines + 1)),
    );
    assert_ne!(
        id_of(&task_notification(lines)),
        id_of(&task_notification(lines + 1))
    );
    assert_eq!(
        first.prompt, second.prompt,
        "the cap took the difference between them"
    );
    assert_ne!(first.prompt_sha256, second.prompt_sha256);
    for event in [&first, &second] {
        let (_, text) = run_entry(event).expect("an entry");
        assert_eq!(run_event_id(event, &text), None);
    }
}

/// A collapsed paste over the broadcast cap reaches the history as what was pasted, not as
/// an opener whose closer the cap cut off.
#[test]
fn an_oversize_collapsed_paste_is_recorded_without_its_wrapper() {
    let body = "build\n".repeat(yinz_tile::agent::HOOK_BROADCAST_MAX_BYTES / 4);
    let event = prompt_event(&format!(
        "\n\n<pasted_content id=\"8c9e\">\n{body}\n</pasted_content id=\"8c9e\">\n"
    ));
    let (pruned, _) = event.broadcast(serde_json::to_string(&event).unwrap());
    let (kind, text) = run_entry(&pruned).expect("an entry");
    assert_eq!(kind, RunEntryKind::Prompt);
    assert!(text.starts_with("build\nbuild"), "{}", &text[..40]);
    assert!(!text.contains("pasted_content"));
}

/// Finding 2, in the form that bites: the id has to be taken from the text the entry keeps,
/// or it is a hash of a string nothing has.
#[test]
fn an_entrys_id_is_a_function_of_the_text_it_stores() {
    let typed = task_notification(3);
    let expected = id_of(&typed).expect("an id");

    // `sanitize` drops a bare control character, so both deliveries record the same text —
    // and an id taken from the raw prompt would have called them two different events.
    let belled = typed.replace("a line", "a\u{7} line");
    assert_ne!(belled, typed);
    assert_eq!(id_of(&belled).as_deref(), Some(expected.as_str()));

    // And the whole point of taking it from there: it can be computed again from the entry.
    let (_, text) = run_entry(&prompt_event(&typed)).expect("an entry");
    let stored = RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind: RunEntryKind::Prompt,
        text,
        created_at_ms: None,
    };
    assert_eq!(entry_event_id(&stored).as_deref(), Some(expected.as_str()));
}

/// The reading the raw prompt gets wrong. `next_closing` wants the closing tag to end a
/// line, and under CRLF the byte after it is `\r`, so the element reads as cut and loses its
/// identity — while the text actually recorded, with the carriage returns dropped, is whole.
#[test]
fn a_notification_with_windows_line_endings_is_still_one_notification() {
    let crlf = task_notification(3).replace('\n', "\r\n");
    let expected = id_of(&task_notification(3)).expect("an id");
    assert_eq!(
        id_of(&crlf).as_deref(),
        Some(expected.as_str()),
        "the same notification, delivered by a harness that ends its lines differently"
    );
}

/// A run older than the entry-row split comes back out of its JSON `payload` blob, where
/// there were no ids to come back with. Compared as stored it matches nothing ever, so the
/// oldest runs — the ones with the most history — were the ones a redelivery landed twice
/// against.
#[test]
fn a_run_recorded_before_ids_existed_is_still_compared_by_one() {
    let (_, text) = run_entry(&prompt_event(&task_notification(3))).expect("an entry");
    let legacy = |kind, text: &str| RunEntry {
        tool: None,
        revision: 1,
        event_id: None,
        kind,
        text: text.to_owned(),
        created_at_ms: None,
    };
    assert_eq!(
        entry_event_id(&legacy(RunEntryKind::Prompt, &text)),
        id_of(&task_notification(3)).map(Cow::Owned),
    );

    // Only a prompt. A tool call's identity was never in its text, and a summary's `None`
    // is a statement rather than a gap — see `push_summary_entry`.
    assert_eq!(entry_event_id(&legacy(RunEntryKind::Result, &text)), None);
    assert_eq!(entry_event_id(&legacy(RunEntryKind::Summary, &text)), None);
    assert_eq!(
        entry_event_id(&legacy(RunEntryKind::Prompt, "fix the flaky test")),
        None,
        "and a prompt with no envelope in it still has no identity"
    );

    // An id it was given is the one it keeps, whatever its text now says.
    let stored = RunEntry {
        tool: None,
        event_id: Some("UserPromptSubmit:given".to_owned()),
        ..legacy(RunEntryKind::Prompt, &text)
    };
    assert_eq!(
        entry_event_id(&stored).as_deref(),
        Some("UserPromptSubmit:given")
    );
}

#[test]
fn the_worktree_cell_names_the_checkout_a_run_worked_in() {
    let run = |cwd: Option<&str>, host: Option<&str>| AgentRunHeader {
        id: "claude:session-1".to_owned(),
        pane_id: 7,
        yinz_session_name: "main".into(),
        host_id: HostId::new(HOST),
        session_id: "session-1".to_owned(),
        origin_run_id: None,
        previous_run_id: None,
        kind: "claude".to_owned(),
        cwd: cwd.map(str::to_owned),
        host: host.map(str::to_owned),
        bubblewrap: None,
        started_at_ms: 10,
        ended_at_ms: None,
        outcome: RunOutcome::Active,
        revision: 1,
        summary: None,
        title: "hook history".to_owned(),
    };
    assert_eq!(worktree_cell(&run(Some("/work/agent"), None,)), "agent");
    assert_eq!(
        worktree_cell(&run(Some("/work/agent/.claude/worktrees/topic"), None,)),
        "topic",
        "two worktrees of one project are told apart by the leaf, which is why it is the leaf",
    );
    assert_eq!(
        worktree_cell(&run(Some("/work/agent"), Some("build-box"),)),
        "build-box:agent",
        "another machine's agent is not this one",
    );
    assert_eq!(worktree_cell(&run(None, None,)), "");
    assert_eq!(
        worktree_cell(&run(Some(""), Some("build-box"),)),
        "",
        "a run that reported no folder names none, rather than a bare `build-box:`",
    );
}

#[test]
fn panes_keep_their_checkout_location() {
    let ssh = AgentPane {
        host: Some("build-box".to_owned()),
        cwd: Some("/work/agent".to_owned()),
        ..pane(8, "claude", Some("claude"))
    };
    assert_eq!(
        ssh.checkout_location(),
        Some(Location::new(Some("build-box"), "/work/agent"))
    );

    let local = AgentPane {
        cwd: Some("/work/agent".to_owned()),
        ..pane(9, "claude", Some("claude"))
    };
    assert_eq!(
        local.checkout_location(),
        Some(Location::local("/work/agent"))
    );

    // Nothing said about either half is no location at all, which is what keeps a
    // pane from being probed under a half-known one.
    assert_eq!(pane(10, "zsh", None).checkout_location(), None);
}

#[test]
fn history_messages_use_opposite_alignment_and_theme_colors() {
    for width in [22, 40, 80] {
        let mut run = run_of_prompts(&["**hello** 界"]);
        run.entries.push(RunEntry {
            kind: RunEntryKind::Message,
            revision: 2,
            ..run.entries[0].clone()
        });
        let history = run_history(&run, width, 10, 0);
        let outgoing = &history.entries[0];
        let incoming = &history.entries[1];
        assert_eq!(outgoing.text.width(), width - 2);
        assert_eq!(&incoming.text[AGE_WIDTH..], "hello 界");
        for (row, color) in [(outgoing, 0), (incoming, 2)] {
            let body = &row.styles[row.styles.len() - "hello 界".chars().count()..];
            assert!(body.iter().all(|style| style.color == Some(color)));
            assert!(body[..5].iter().all(|style| style.bold));
            assert!(!row.text.contains("prompt:"));
            assert!(!row.text.contains("message:"));
        }
    }
}
