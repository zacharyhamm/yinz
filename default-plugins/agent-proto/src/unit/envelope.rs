use super::*;

/// A notification of the shape Claude Code actually submits.
fn notification() -> String {
    [
        "<task-notification>",
        "<task-id>a10fe836c6a9fa4ca</task-id>",
        "<output-file>/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output</output-file>",
        "<status>completed</status>",
        "<summary>Agent \"/code-review high up to 3 subagents\" finished</summary>",
        "<note>A task-notification fires each time this agent stops.",
        "The same task-id may notify more than once.</note>",
        "<result>The review found four issues.",
        "Two are in the scanner.</result>",
        "</task-notification>",
    ]
    .join("\n")
}

/// A notification for `task` whose result says `result`. The pairs below need two that
/// agree for a while and then do not, which the fixed [`notification`] cannot give.
fn notification_of(task: &str, result: &str) -> String {
    format!(
            "<task-notification>\n<task-id>{task}</task-id>\n<status>completed</status>\n<result>{result}</result>\n</task-notification>"
        )
}

fn names(sections: &[Section]) -> Vec<&str> {
    sections.iter().map(|s| s.name.as_str()).collect()
}

// -- the scanner --------------------------------------------------------

/// The common case, and the one that must cost nothing: a prompt with no angle bracket in
/// it is borrowed whole and never scanned.
#[test]
fn a_prompt_with_no_angle_bracket_is_prose_and_nothing_else() {
    let prompt = parse("fix the flaky reconnect test");
    assert_eq!(prompt.prose, "fix the flaky reconnect test");
    assert!(prompt.envelopes.is_empty());
    assert!(matches!(prompt.prose, Cow::Borrowed(_)), "it was copied");
    assert_eq!(digest("fix the flaky reconnect test"), None);
}

/// Every field the kind asked for comes out, in the order it arrived, and the raw slice is
/// the whole element — which is what the reader's `r` shows.
#[test]
fn a_task_notification_becomes_its_fields() {
    let text = notification();
    let prompt = parse(&text);
    assert_eq!(prompt.prose, "");
    assert_eq!(prompt.envelopes.len(), 1);
    let envelope = &prompt.envelopes[0];
    assert_eq!(envelope.tag, "task-notification");
    assert!(!envelope.truncated);
    assert_eq!(envelope.raw, text.trim());
    assert_eq!(
        envelope.fields.iter().map(|f| f.name).collect::<Vec<_>>(),
        [
            "task-id",
            "output-file",
            "status",
            "summary",
            "note",
            "result"
        ]
    );
    assert_eq!(envelope.field("task-id"), Some("a10fe836c6a9fa4ca"));
    assert_eq!(envelope.field("status"), Some("completed"));
    assert_eq!(
        envelope.field("result"),
        Some("The review found four issues.\nTwo are in the scanner.")
    );
}

/// Two levels and no more. A `<result>` full of markup is a result, and the markup inside
/// it comes back untouched rather than being read as structure.
#[test]
fn a_field_keeps_the_tags_nested_inside_it() {
    let text = "<task-notification>\n<result>see <b>this</b> and\n<other-thing>that</other-thing>\n</result>\n</task-notification>";
    let prompt = parse(text);
    assert_eq!(
        prompt.envelopes[0].field("result"),
        Some("see <b>this</b> and\n<other-thing>that</other-thing>")
    );
    assert_eq!(
        prompt.envelopes.len(),
        1,
        "the nested tag was not top level"
    );
}

/// A same-named child must not close its parent. Without the depth count the outer element
/// would end at the inner one's closer and the rest of the prompt would become prose.
#[test]
fn a_same_named_child_does_not_close_its_parent_early() {
    let text =
        "<some-thing>\n<some-thing>inner</some-thing>\nouter tail\n</some-thing>\ntrailing prose";
    let prompt = parse(text);
    assert_eq!(prompt.envelopes.len(), 1);
    assert_eq!(
        prompt.envelopes[0].body.trim(),
        "<some-thing>inner</some-thing>\nouter tail"
    );
    assert_eq!(prompt.prose, "trailing prose");
}

/// `HookEvent::pruned_to` cuts a long prompt, so an element that never closes is a real
/// case. Everything that arrived is kept, and the cut is reported rather than hidden.
#[test]
fn an_envelope_cut_off_mid_result_keeps_what_arrived_and_says_it_was_cut() {
    let text = "<task-notification>\n<status>completed</status>\n<result>the review found";
    let prompt = parse(text);
    let envelope = &prompt.envelopes[0];
    assert!(envelope.truncated, "the element ran off the end");
    assert_eq!(envelope.field("status"), Some("completed"));
    assert_eq!(envelope.field("result"), Some("the review found"));
    assert!(
        envelope
            .fields
            .iter()
            .find(|f| f.name == "result")
            .expect("a result")
            .truncated
    );
    assert!(digest(text).expect("a digest").contains("cut"));
}

/// The shapes a person writes that look like a tag and are not. Any of them matching would
/// swallow the rest of what they typed.
#[test]
fn a_less_than_in_prose_is_not_an_envelope() {
    for text in [
        "make it a Vec<String> instead",
        "assert that if x < y then z",
        "wrap it in <b>bold</b> please",
        "the type is HashMap<String, Vec<u8>>",
    ] {
        let prompt = parse(text);
        assert!(
            prompt.envelopes.is_empty(),
            "{text:?} was read as an envelope"
        );
        assert_eq!(prompt.prose, text);
        assert_eq!(digest(text), None, "{text:?}");
    }
}

/// Condition 1. A hyphenated tag written mid-sentence is a person writing, not a harness.
#[test]
fn an_envelope_has_to_open_a_line_of_its_own() {
    let text = "rename it to <task-notification>foo</task-notification> in the docs";
    assert!(parse(text).envelopes.is_empty());
    assert_eq!(digest(text), None);
}

/// Condition 2, which is the whole of what makes the unknown-tag fallback safe: an
/// unregistered tag has to be hyphenated before this claims it.
#[test]
fn only_a_hyphenated_tag_reaches_the_generic_fallback() {
    for bare in [
        "<div>\nx\n</div>",
        "<p>\nx\n</p>",
        "<Task-Note>\nx\n</Task-Note>",
    ] {
        assert!(parse(bare).envelopes.is_empty(), "{bare:?}");
    }
    let prompt = parse("<some-new-thing>\nfirst line\nsecond\n</some-new-thing>");
    assert_eq!(prompt.envelopes.len(), 1);
    assert_eq!(prompt.envelopes[0].tag, "some-new-thing");
}

/// Nothing here reads an attribute, and a tag that grows one must not stop being seen.
#[test]
fn attributes_on_a_tag_do_not_hide_it() {
    let prompt =
        parse("<task-notification version=\"2\">\n<status>failed</status>\n</task-notification>");
    assert_eq!(prompt.envelopes.len(), 1);
    assert_eq!(prompt.envelopes[0].field("status"), Some("failed"));
}

/// The commonest shape of all: a person's prompt with the harness's reminder stapled to it.
/// What they typed has to survive whole and lead.
#[test]
fn a_system_reminder_appended_to_a_prompt_leaves_the_prose_alone() {
    let text = "rebase onto main and rerun the suite\n\n<system-reminder>\nRemember to check the plan mode state.\nIt is a long reminder.\n</system-reminder>";
    let prompt = parse(text);
    assert_eq!(prompt.prose, "rebase onto main and rerun the suite");
    assert_eq!(prompt.envelopes.len(), 1);

    let drawn = digest(text).expect("a digest");
    assert!(
        drawn.starts_with("rebase onto main and rerun the suite"),
        "{drawn}"
    );
    assert!(drawn.contains("▸ system-reminder"), "{drawn}");
    assert!(!drawn.contains("Remember to check"), "{drawn}");
    // The reminder never names a pane: it says how to behave, not what the turn is for.
    assert_eq!(title(text), None);
    assert_eq!(question(text), "rebase onto main and rerun the suite");
}

/// Three top-level siblings that mean one thing. Absorbing them is what lets the renderers
/// read one shape rather than reassembling it three times over.
#[test]
fn a_slash_command_is_its_three_elements_read_as_one_thing() {
    let text = "<command-name>/plan</command-name>\n<command-args>design the renderer</command-args>\n<local-command-stdout>Enabled plan mode\nsecond line</local-command-stdout>";
    let prompt = parse(text);
    assert_eq!(prompt.envelopes.len(), 1, "{:?}", prompt.envelopes);
    assert_eq!(title(text).as_deref(), Some("/plan design the renderer"));

    let drawn = digest(text).expect("a digest");
    assert!(drawn.starts_with("/plan design the renderer"), "{drawn}");
    assert!(drawn.contains("▸ stdout 2 lines"), "{drawn}");
    assert_eq!(names(&sections(text)), ["command", "stdout"]);
}

/// Which of the three a harness writes first is its own business, and Claude Code has put
/// the message ahead of the command it belongs to. Absorbed only forwards, that read as a
/// nameless message and a command that had lost it — and named the pane after the message.
#[test]
fn a_command_message_before_the_name_is_still_one_command() {
    let text = "<command-message>plan is running…</command-message>\n<command-name>/plan</command-name>\n<command-args>design the renderer</command-args>";
    let prompt = parse(text);
    assert_eq!(prompt.envelopes.len(), 1, "{:?}", prompt.envelopes);
    let envelope = &prompt.envelopes[0];
    assert_eq!(envelope.tag, "command-name");
    assert_eq!(
        envelope.fields.iter().map(|f| f.name).collect::<Vec<_>>(),
        ["command-message", "command-name", "command-args"],
        "the fields read in the order they arrived"
    );
    assert_eq!(title(text).as_deref(), Some("/plan design the renderer"));
}

/// And a second command does not take the first one's siblings with it.
#[test]
fn a_second_slash_command_does_not_steal_the_first_ones_siblings() {
    let text = "<command-message>a</command-message>\n<command-name>/a</command-name>\n<command-message>b</command-message>\n<command-name>/b</command-name>";
    let prompt = parse(text);
    assert_eq!(prompt.envelopes.len(), 2, "{:?}", prompt.envelopes);
    assert_eq!(prompt.envelopes[0].field("command-message"), Some("a"));
    assert_eq!(prompt.envelopes[0].field("command-name"), Some("/a"));
    assert_eq!(prompt.envelopes[1].field("command-message"), Some("b"));
    assert_eq!(prompt.envelopes[1].field("command-name"), Some("/b"));
}

/// The shape as it actually arrives: the expansion indents every line after the first, and
/// `crate::text::sanitize` turns a leading tab into four spaces more. Held to column zero,
/// two thirds of the command lands in `prose`, which is neither escaped nor cut.
#[test]
fn an_indented_slash_command_expansion_is_still_one_envelope() {
    let text = "<command-name>/plan</command-name>\n            <command-message>plan</command-message>\n            <command-args>design the renderer</command-args>\n<local-command-stdout>Enabled plan mode</local-command-stdout>";
    let prompt = parse(text);
    assert_eq!(prompt.prose, "", "{:?}", prompt.prose);
    assert_eq!(prompt.envelopes.len(), 1, "{:?}", prompt.envelopes);
    assert_eq!(
        prompt.envelopes[0].raw, "<command-name>/plan</command-name>",
        "the indentation is not part of the element"
    );
    assert_eq!(title(text).as_deref(), Some("/plan design the renderer"));
    assert_eq!(names(&sections(text)), ["command", "stdout"]);
}

/// Condition 1 bends for a tag the table knows and for nothing else, because what it is
/// there to protect is an indented line of somebody's code block.
#[test]
fn an_indented_tag_the_table_does_not_know_is_still_prose() {
    let text = "the shape it arrives in:\n\n    <some-new-thing>\n    one\n    </some-new-thing>";
    let prompt = parse(text);
    assert!(prompt.envelopes.is_empty(), "{:?}", prompt.envelopes);
    assert_eq!(digest(text), None);
}

/// A harness that writes two fields to one line is a shape, not a risk. Held to the
/// end-of-line rule the first of them swallowed the rest of the element and reported itself
/// cut, which also cost the notification its identity.
#[test]
fn two_fields_written_on_one_line_are_two_fields() {
    let text = "<task-notification>\n<status>completed</status> <summary>it finished</summary>\n</task-notification>";
    let envelope = &parse(text).envelopes[0];
    assert_eq!(envelope.field("status"), Some("completed"));
    assert_eq!(envelope.field("summary"), Some("it finished"));
    assert!(envelope.fields.iter().all(|field| !field.truncated));
}

/// And what the rule is still for: a result describing this scanner names the closing tag
/// mid-sentence, where closing would end an element the writer never opened.
#[test]
fn a_closing_tag_written_mid_sentence_does_not_close_a_field_early() {
    let text = "<task-notification>\n<result>the scanner looks for </result> at the end of a line\nand then stops.</result>\n</task-notification>";
    let envelope = &parse(text).envelopes[0];
    assert_eq!(
        envelope.field("result"),
        Some("the scanner looks for </result> at the end of a line\nand then stops.")
    );
    assert!(!envelope.truncated);
}

/// A self-closing tag opens nothing, so claiming it left `closing_of` looking for a closer
/// that was never coming — and the element swallowing the rest of the prompt.
#[test]
fn a_self_closing_tag_does_not_swallow_the_rest_of_the_prompt() {
    let text = "<some-thing/>\nfix the flaky reconnect test";
    let prompt = parse(text);
    assert!(prompt.envelopes.is_empty(), "{:?}", prompt.envelopes);
    assert_eq!(prompt.prose, text);
    assert_eq!(digest(text), None);
}

/// The same inside an envelope, where it would have taken every field after it with it.
#[test]
fn a_self_closing_child_does_not_run_to_the_end_of_the_envelope() {
    let text = "<task-notification>\n<status>completed</status>\n<result/>\n<summary>it finished</summary>\n</task-notification>";
    let envelope = &parse(text).envelopes[0];
    assert_eq!(envelope.field("summary"), Some("it finished"));
    assert_eq!(envelope.field("result"), None);
    assert!(envelope.fields.iter().all(|field| !field.truncated));
}

/// The live path does not sanitise — `agent-service` reads the prompt as it arrived — so a
/// harness ending its lines the Windows way had every element it sent read as cut, and a
/// cut element has no identity to dedupe by.
#[test]
fn a_notification_with_carriage_returns_is_not_read_as_cut() {
    let crlf = notification().replace('\n', "\r\n");
    let envelope = &parse(&crlf).envelopes[0];
    assert!(!envelope.truncated);
    assert_eq!(envelope.field("status"), Some("completed"));
    assert!(dedupe_id(&crlf).is_some(), "and so it has an identity");
}

// -- what a reader is told ----------------------------------------------

/// The row exists to say which task reported and how it went, in that order, before it says
/// anything else.
#[test]
fn the_digest_of_a_task_notification_says_the_status_the_id_and_the_summary() {
    let drawn = digest(&notification()).expect("a digest");
    let first = drawn.lines().next().expect("a first line");
    assert!(first.starts_with("✓ **a10fe836** ·"), "{first}");
    assert!(
        first.contains("/code-review high up to 3 subagents"),
        "{first}"
    );
    assert!(!drawn.contains("task-notification"), "{drawn}");
    assert!(!drawn.contains("/private/tmp"), "{drawn}");
    assert!(drawn.contains("▸ note 2 lines"), "{drawn}");
    assert!(drawn.contains("▸ result 2 lines"), "{drawn}");
    assert!(drawn.contains("▸ output-file"), "{drawn}");
}

/// The digest is markdown source that the timeline then parses, so a summary an agent wrote
/// must not be able to restyle the row it lands in.
#[test]
fn the_digest_escapes_what_the_agent_wrote() {
    let text =
        "<task-notification>\n<summary># **x** | y ~~z~~ <b></summary>\n</task-notification>";
    let drawn = digest(text).expect("a digest");
    assert!(
        drawn.contains(r"\# \*\*x\*\* \| y \~\~z\~\~ \<b\>"),
        "{drawn}"
    );
    let rendered: String = markdown::render(&drawn, 200, markdown::Options::compact())
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("# **x** | y ~~z~~ <b>"), "{rendered}");
}

/// A harness that drops a field costs us that piece, not the row.
#[test]
fn a_notification_with_no_summary_still_says_which_task_it_was() {
    let text = "<task-notification>\n<task-id>a10fe836c6a9fa4ca</task-id>\n<status>completed</status>\n</task-notification>";
    let drawn = digest(text).expect("a digest");
    assert!(
        drawn.starts_with("✓ **a10fe836** · background task"),
        "{drawn}"
    );
    assert_eq!(title(text).as_deref(), Some("task · a10fe836"));

    let bare = "<task-notification>\n<summary>it finished</summary>\n</task-notification>";
    let drawn = digest(bare).expect("a digest");
    assert!(drawn.starts_with("it finished"), "{drawn}");
}

/// Guessing at a status we have never seen would be worse than showing it.
#[test]
fn an_unknown_status_is_shown_as_it_arrived() {
    let text = "<task-notification>\n<status>throttled</status>\n<summary>waiting</summary>\n</task-notification>";
    let drawn = digest(text).expect("a digest");
    assert!(drawn.starts_with("· throttled waiting"), "{drawn}");
}

/// The summariser is asked what the agent is doing. Tags, the note's protocol boilerplate
/// and a path the model cannot read are all noise against that question.
#[test]
fn the_question_the_summariser_is_asked_carries_no_tags_and_no_note() {
    let asked = question(&notification());
    assert!(
        asked.starts_with("background task a10fe836 completed:"),
        "{asked}"
    );
    assert!(asked.contains("The review found four issues."), "{asked}");
    assert!(!asked.contains('<'), "{asked}");
    assert!(!asked.contains("notify more than once"), "{asked}");
    assert!(!asked.contains("/private/tmp"), "{asked}");
}

/// The short fields are why the event was drawn at all, so they are open; `note` and
/// `result` are why it could not be drawn whole, so they are not.
#[test]
fn the_sections_a_reader_opens_start_with_the_short_ones_open() {
    let folds = sections(&notification());
    assert_eq!(
        names(&folds),
        [
            "status",
            "task-id",
            "summary",
            "output-file",
            "note",
            "result"
        ]
    );
    for fold in &folds {
        match fold.name.as_str() {
            "note" | "result" => assert!(!fold.open && fold.foldable(), "{}", fold.name),
            _ => assert!(fold.open, "{}", fold.name),
        }
    }
    let output = folds
        .iter()
        .find(|f| f.name == "output-file")
        .expect("a path");
    assert_eq!(
        output.path.as_deref(),
        Some("/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output")
    );
}

/// An envelope nobody registered still reads better than its own markup, and is still
/// openable whole. That is the point of the table having a fallback at all.
#[test]
fn an_unknown_tag_still_gets_a_row_and_a_fold() {
    let text = "<some-new-thing>\nThe first line of whatever it said\nand more below it\n</some-new-thing>";
    let drawn = digest(text).expect("a digest");
    // The first line of what it said leads; the tag is named once, on the marker row, so a
    // four-row entry does not spend one of them saying the same word twice.
    assert!(
        drawn.starts_with("The first line of whatever it said"),
        "{drawn}"
    );
    assert!(drawn.contains("▸ some-new-thing 2 lines"), "{drawn}");
    assert_eq!(drawn.matches("some-new-thing").count(), 1, "{drawn}");
    assert_eq!(names(&sections(text)), ["some-new-thing"]);
    assert!(question(text).starts_with("some-new-thing: The first line"));
}

/// The table's spellings are load-bearing in two directions — `kind_of` looks a tag up in
/// it, and the scanner will only ever hand it a hyphenated lowercase name — so a typo in
/// one is a kind that can never be reached.
#[test]
fn every_kind_in_the_table_has_a_hyphenated_lowercase_tag_and_no_two_claim_one() {
    let mut seen: Vec<&str> = Vec::new();
    for kind in KINDS {
        assert!(
            is_tag_name(kind.tag, Rules::Toplevel),
            "{:?} is not a tag the scanner can produce",
            kind.tag
        );
        assert!(!seen.contains(&kind.tag), "{:?} is claimed twice", kind.tag);
        assert!(
            std::ptr::eq(kind_of(kind.tag), *kind),
            "{:?} is not reachable by its tag",
            kind.tag
        );
        seen.push(kind.tag);
        for absorbed in kind.absorbs {
            assert!(is_tag_name(absorbed, Rules::Toplevel), "{absorbed:?}");
        }
    }
    assert!(std::ptr::eq(kind_of("no-such-thing"), &GENERIC));
}

/// The reason `DIGEST_SUMMARY_COLS` exists: a talkative harness must not be able to spend
/// the rows the affordances line needs.
#[test]
fn an_enormous_result_costs_the_digest_one_row() {
    let text = format!(
            "<task-notification>\n<status>completed</status>\n<summary>{}</summary>\n<result>{}</result>\n</task-notification>",
            "a very long summary ".repeat(40),
            "a line of the result\n".repeat(4000),
        );
    let drawn = digest(&text).expect("a digest");
    assert!(drawn.len() < 400, "the digest is {} bytes", drawn.len());
    assert!(drawn.contains('…'), "the summary was not cut: {drawn}");
    assert!(drawn.contains("▸ result 4000 lines"), "{drawn}");
}

/// Forty-eight *columns*, not forty-eight characters. Counted by character, a summary in a
/// script that spends two columns to each of them is twice as wide as the budget says, takes
/// a third row, and leaves `cap_rows` to eat the row saying what `Enter` would show.
#[test]
fn a_summary_is_cut_to_columns_rather_than_characters() {
    let wide = "这是一个很长的摘要 ".repeat(8);
    let drawn = cut(&wide, DIGEST_SUMMARY_COLS);
    assert!(
        drawn.width() <= DIGEST_SUMMARY_COLS,
        "{} columns: {drawn}",
        drawn.width()
    );
    assert!(drawn.ends_with('…'), "{drawn}");
    // A word too long to break at is cut mid-word, and the `…` is spent out of the budget
    // rather than added to it, so even that case fits the row it was measured for.
    let one_word = "x".repeat(200);
    assert_eq!(
        cut(&one_word, DIGEST_SUMMARY_COLS).width(),
        DIGEST_SUMMARY_COLS
    );
}

/// Order is the whole of what a timeline is, and a prompt carrying several envelopes is no
/// different from one carrying several paragraphs.
#[test]
fn a_prompt_carrying_several_envelopes_draws_them_in_the_order_they_arrived() {
    let text = "<first-thing>\none\n</first-thing>\n<second-thing>\ntwo\n</second-thing>";
    let prompt = parse(text);
    assert_eq!(
        prompt.envelopes.iter().map(|e| e.tag).collect::<Vec<_>>(),
        ["first-thing", "second-thing"]
    );
    let drawn = digest(text).expect("a digest");
    assert!(
        drawn.find("first-thing") < drawn.find("second-thing"),
        "{drawn}"
    );
}

// -- the dedupe key -----------------------------------------------------

/// A redelivery of one notification is one thing, however far apart the two arrive.
#[test]
fn two_deliveries_of_one_notification_share_an_id() {
    let text = notification();
    assert_eq!(dedupe_id(&text), dedupe_id(&text));
    assert!(dedupe_id(&text)
        .expect("an id")
        .starts_with("task-notification:a10fe836c6a9fa4ca:"));
}

/// The envelope's own note says the same task-id may notify more than once — a resumed
/// agent reports again with a *different result*, and that is a second thing happening
/// rather than a second delivery of the first.
#[test]
fn two_notifications_for_one_task_do_not_share_an_id() {
    let first = notification();
    let second = first.replace(
        "The review found four issues.",
        "The review found one issue.",
    );
    assert_ne!(first, second);
    assert_ne!(dedupe_id(&first), dedupe_id(&second));
}

/// Nothing but a task notification has an identity of its own, so nothing else changes the
/// adjacent-only dedupe the ingest path already does.
#[test]
fn only_a_task_notification_carries_a_dedupe_id() {
    assert_eq!(dedupe_id("plain prose"), None);
    assert_eq!(dedupe_id("<system-reminder>\nx\n</system-reminder>"), None);
    assert_eq!(dedupe_id("<some-new-thing>\nx\n</some-new-thing>"), None);
}

/// A cut element is a prefix of a notification, and a hash of it identifies the prefix.
/// The task id is still right there — the point is that it is refused anyway, because an
/// id built from it would claim to stand for an element we did not receive.
#[test]
fn a_cut_notification_has_no_identity() {
    let whole = notification_of("a10fe836c6a9fa4ca", "the review found four issues");
    let cut = &whole[..whole.len() - 20];

    let envelope = &parse(cut).envelopes[0];
    assert!(envelope.truncated, "the element ran off the end");
    assert_eq!(envelope.field("task-id"), Some("a10fe836c6a9fa4ca"));
    assert_eq!(dedupe_id(cut), None);
    assert!(dedupe_id(&whole).is_some(), "and a whole one still has one");
}

/// The collision, from the data rather than from an argument: `pruned_to` takes a prompt at
/// a fixed length with no marker, so two reports of one task that agree up to it arrive
/// byte for byte the same.
///
/// Which is also why folding the cut flag and the length into the key would not have
/// helped — both are equal across exactly the pair they would have to tell apart.
#[test]
fn two_notifications_cut_at_one_length_are_indistinguishable() {
    let shared = "a line of the review\n".repeat(20);
    let first = notification_of("a10fe836c6a9fa4ca", &format!("{shared}four issues"));
    let second = notification_of("a10fe836c6a9fa4ca", &format!("{shared}one issue"));
    assert_ne!(first, second);
    assert_ne!(
        dedupe_id(&first),
        dedupe_id(&second),
        "delivered whole they are two reports"
    );

    // Cut inside the part they agree on, which is where the cap lands on any result long
    // enough to be cut at all.
    let at = 300;
    assert_eq!(first[..at], second[..at], "the cut took the difference");
    for cut in [&first[..at], &second[..at]] {
        assert!(parse(cut).envelopes[0].truncated);
        assert_eq!(dedupe_id(cut), None);
    }
}

/// A prompt is one delivery, so its identity is the identity of everything identifiable in
/// it. Two background tasks reporting together are one prompt, and a later prompt sharing
/// only the first of them is a different prompt rather than a redelivery of it.
#[test]
fn a_prompt_carrying_two_notifications_is_identified_by_both() {
    let first = notification_of("aaaa1111", "the first task finished");
    let second = notification_of("bbbb2222", "the second task finished");
    let third = notification_of("cccc3333", "the third task finished");
    let both = format!("{first}\n{second}");

    assert_eq!(dedupe_id(&both), dedupe_id(&format!("{first}\n{second}")));
    assert_ne!(dedupe_id(&both), dedupe_id(&format!("{first}\n{third}")));
    assert_ne!(
        dedupe_id(&both),
        dedupe_id(&first),
        "one of the two is not the pair"
    );
}

// -- the reader's rows --------------------------------------------------

/// The reader's rows for `sections`, parsed and wrapped the way the reader does.
fn reader_lines(sections: &[Section], width: usize) -> ReaderLines {
    let blocks: Vec<Vec<markdown::MdBlock>> = sections.iter().map(section_blocks).collect();
    wrap_sections(blocks.iter().map(Vec::as_slice), width)
}

/// The spans have to tile the rows exactly, or a cursor lands on a section that does not
/// own the row under it — the same invariant `HistoryLines::events` holds.
#[test]
fn a_readers_spans_tile_its_rows() {
    let mut folds = sections(&notification());
    folds[5].open = true;
    let rows = reader_lines(&folds, 46);
    assert_eq!(rows.starts.len(), folds.len());
    assert_eq!(rows.starts[0], 0);
    for pair in rows.starts.windows(2) {
        assert!(pair[0] < pair[1], "{:?}", rows.starts);
    }
    assert!(*rows.starts.last().expect("a last start") < rows.lines.len());
}

/// A shut fold costs its header and nothing else; opening it is what spends the rows. The
/// body comes through as the markdown the agent wrote rather than as an escaped value.
#[test]
fn opening_a_fold_is_what_spends_the_rows() {
    let mut folds = sections(&notification());
    let shut = reader_lines(&folds, 60).lines.len();
    let result = folds
        .iter_mut()
        .find(|f| f.name == "result")
        .expect("a result");
    result.open = true;
    let open = reader_lines(&folds, 60).lines.len();
    assert!(open > shut, "{open} rows is not more than {shut}");

    let text: Vec<String> = reader_lines(&folds, 60)
        .lines
        .iter()
        .map(|line| line.text.clone())
        .collect();
    assert!(
        text.iter().any(|line| line.contains("▾ result")),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|line| line.contains("The review found four issues.")),
        "{text:?}"
    );
    assert!(text.iter().any(|line| line.contains("▸ note")), "{text:?}");
    assert!(
        !text
            .iter()
            .any(|line| line.contains("task-notification fires")),
        "a shut fold drew its body: {text:?}"
    );
}
