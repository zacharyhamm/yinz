use super::*;
use serde_json::json;

const REFRESH_MS: i64 = 60_000;

// Lab API shapes (`GET /api/v1/lab/issues` and `/issues/{n}`), including a
// shipped issue with a comment (#2180). Bodies are shortened for the fixture.
const SEARCH: &str = r#"{"issues":[
    {"id":"i-2254","number":2254,"title":"Invite output","type":"bug","status":"triaged","authorId":"u1","authorUsername":"skunk-ape","source":"swamp"},
    {"id":"i-2180","number":2180,"title":"CLI referral link","type":"platform","status":"shipped","authorId":"u1","authorUsername":"skunk-ape","source":"swamp-club"}
],"total":2,"tierByUsername":{},"relationshipSummaries":{}}"#;
const DETAIL: &str = r###"{
    "issue":{"id":"i-2180","number":2180,"title":"CLI referral link","type":"platform","status":"shipped",
        "authorId":"u1","authorUsername":"skunk-ape","source":"swamp-club","body":"## Referral link\n\nPrint **one URL**.",
        "assignees":[{"userId":"u1","username":"skunk-ape"}]},
    "audience":{"visibility":"author_and_admins","reasons":[]},
    "comments":[{"id":"c1","issueId":"i-2180","authorId":"u2","authorUsername":"hammz","body":"Depends on **#2179**.","createdAt":"2026-09-16T16:20:51.953Z"}],
    "events":[],"lifecycleEntries":[],"relationships":[],"isAdmin":false,"canEdit":false,"currentUserId":""
}"###;
const EMPTY: &str = r#"{"issues":[],"total":0}"#;

trait FromLab: Sized {
    fn from_reply(status: u16, body: &[u8]) -> Result<Self, String>;
}

impl FromLab for Search {
    fn from_reply(status: u16, body: &[u8]) -> Result<Self, String> {
        swamp_lab::parse::<lab::SearchResponse>(status, &server(), body)
            .map(|search| Search::from_lab(&search, &Credentials::anonymous()))
            .map_err(|error| error.to_string())
    }
}

impl FromLab for Detail {
    fn from_reply(status: u16, body: &[u8]) -> Result<Self, String> {
        swamp_lab::parse::<lab::IssueDetail>(status, &server(), body)
            .map(|detail| Detail::from_lab(&detail, &Credentials::anonymous()))
            .map_err(|error| error.to_string())
    }
}

fn decode<T: FromLab>(body: &[u8]) -> Result<T, String> {
    T::from_reply(200, body)
}

/// The headers any real server reply has; none at all means no reply.
fn server() -> BTreeMap<String, String> {
    BTreeMap::from([("content-type".into(), "application/json".into())])
}

fn search_url(query: &str) -> String {
    Client::new(&Credentials::anonymous())
        .search(&search_query(query, 0).unwrap())
        .unwrap()
        .url
}

fn ready() -> State {
    let mut state = State {
        ..Default::default()
    };
    state.swamp.pane_visible = true;
    state.swamp.permission = Permission::Granted;
    state.swamp.credentials = Some(Credentials::anonymous());
    state.swamp.keep_credentials = true;
    state
}

fn context(state: &State) -> BTreeMap<String, String> {
    state.swamp.requests.last().expect("a request").1.clone()
}

fn reply(state: &mut State, json: &str) {
    reply_status(state, 200, json);
}

fn reply_status(state: &mut State, status: u16, json: &str) {
    let context = context(state);
    state.swamp_web_result(status, &server(), json.as_bytes(), &context);
}

fn pane_update(plugin_id: PluginId, suppressed: bool) -> Event {
    Event::PaneUpdate(PaneManifest {
        panes: std::collections::HashMap::from([(
            TabPosition::new(0),
            vec![PaneInfo {
                id: PaneId::Plugin(plugin_id),
                is_suppressed: suppressed,
                ..Default::default()
            }],
        )]),
    })
}

fn loaded() -> State {
    let mut state = ready();
    state.activate_swamp();
    reply(&mut state, SEARCH);
    state.render(9, 80);
    state
}

fn key(state: &mut State, key: BareKey) {
    state.on_key(KeyWithModifier::new(key));
}

fn reader() -> State {
    let mut state = loaded();
    key(&mut state, BareKey::Char('j'));
    key(&mut state, BareKey::Enter);
    reply(&mut state, DETAIL);
    state.render(9, 80);
    state
}

fn ack(state: &mut State, outcome: wire::AskOutcome) -> bool {
    state.pipe(PipeMessage::new(
        PipeSource::Plugin(PluginId::new(0)),
        wire::ASK_ACK,
        &Some(
            serde_json::to_string(&wire::AskAck {
                outcome,
                detail: "no API key".into(),
            })
            .unwrap(),
        ),
        &None,
        false,
    ))
}

/// `a` then `y`: the question, and the answer that sends.
fn triage(state: &mut State) {
    key(state, BareKey::Char('a'));
    key(state, BareKey::Char('y'));
}

#[test]
fn triage_captures_selection_and_suppresses_duplicate_fetches_and_handoffs() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('j'));
    triage(&mut state);
    let count = state.swamp.requests.len();
    key(&mut state, BareKey::Char('k'));
    triage(&mut state);
    assert_eq!(state.swamp.requests.len(), count);
    assert!(
        !ack(&mut state, wire::AskOutcome::Sent),
        "fetch is not a handoff"
    );
    reply(&mut state, DETAIL);
    let text = state.swamp.triage_text.clone().unwrap();
    for expected in [
        "CLI referral link",
        "## Referral link",
        "https://swamp-club.com/lab/2180",
        "Depends on **#2179**.",
        "Is this an extension issue or a swamp issue, or both?",
        "swamp-extensions",
        "project_list",
        "never substitute the active project",
        "agent=claude",
        "model=opus",
        "branch=issue-2180",
        "reuse=true",
        "prompt=\"triage issue 2180\"",
        "reuse_prompt=\"resume issue 2180\"",
        "one agent in each",
        "independently",
        "confirmation rules",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    triage(&mut state);
    assert_eq!(state.swamp.requests.len(), count);
    assert!(ack(&mut state, wire::AskOutcome::Queued));
    assert!(state.swamp.note().unwrap().contains("#2180 queued"));
    assert!(!ack(&mut state, wire::AskOutcome::Sent));
}

#[test]
fn triage_in_reader_fetches_current_detail_and_reports_each_acknowledgement() {
    let mut state = reader();
    assert!(state.swamp.header(usize::MAX).contains("a triage"));
    for (outcome, note) in [
        (wire::AskOutcome::Sent, "sent"),
        (wire::AskOutcome::Queued, "queued"),
        (wire::AskOutcome::Refused, "no API key"),
    ] {
        let count = state.swamp.requests.len();
        triage(&mut state);
        assert!(state.swamp.triage_waiting);
        assert_eq!(state.swamp.requests.len(), count + 1);
        assert!(!ack(&mut state, outcome));
        reply(
            &mut state,
            &DETAIL.replace("Print **one URL**.", "Fresh body."),
        );
        assert!(state
            .swamp
            .triage_text
            .as_ref()
            .unwrap()
            .contains("Fresh body."));
        assert!(ack(&mut state, outcome));
        assert!(state.swamp.note().unwrap().contains(note));
        assert!(state.swamp.note().unwrap().contains("#2180"));
        assert_eq!(
            state.swamp.noted().unwrap().1 == Tone::Error,
            outcome == wire::AskOutcome::Refused
        );
        assert!(!state.swamp.triage_waiting);
    }
}

#[test]
fn triage_waits_for_existing_requests_and_recovers_from_fetch_errors() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('r'));
    key(&mut state, BareKey::Char('j'));
    triage(&mut state);
    key(&mut state, BareKey::Char('k'));
    reply(&mut state, SEARCH);
    assert!(matches!(
        state.swamp.pending.as_ref().unwrap().target,
        Target::Triage(2180)
    ));
    let context = context(&state);
    // No headers: the request never reached a server.
    state.swamp_web_result(400, &BTreeMap::new(), b"offline", &context);
    assert!(!state.swamp.triage_waiting);
    assert!(state.swamp.note().unwrap().contains("offline"));
    assert!(state.swamp.has_error(), "a failed triage marks the tab");
    triage(&mut state);
    reply(&mut state, DETAIL); // now selected 2254, so this is a mismatched reply
    assert!(state.swamp.note().unwrap().contains("wrong issue number"));
    assert!(state.swamp.triage_text.is_none());
}

#[test]
fn triage_continues_after_leaving_the_issue_surface() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('j'));
    key(&mut state, BareKey::Enter);
    triage(&mut state);
    key(&mut state, BareKey::Esc);
    state.update(pane_update(state.own_plugin_id, true));
    reply(&mut state, DETAIL); // reader fetch was already in flight
    assert!(matches!(
        state.swamp.pending.as_ref().unwrap().target,
        Target::Triage(2180)
    ));
    reply(&mut state, DETAIL);
    assert!(state
        .swamp
        .triage_text
        .as_deref()
        .unwrap()
        .contains("triage issue 2180"));
}

#[test]
fn triage_does_not_capture_text_entry() {
    let mut state = loaded();
    assert!(state.swamp.header(usize::MAX).contains("a triage"));
    key(&mut state, BareKey::Char('/'));
    key(&mut state, BareKey::Char('a'));
    assert_eq!(
        state.swamp.editing.as_ref().map(TextInput::get_text),
        Some("a")
    );
    key(&mut state, BareKey::Esc);
    key(&mut state, BareKey::Char('f'));
    key(&mut state, BareKey::Char('a'));
    assert!(!state.swamp.triage_waiting);
    assert!(state.swamp.triage_text.is_none());
}

#[test]
fn cli_shapes_include_comments_empty_results_and_unknown_statuses() {
    let search = decode::<Search>(SEARCH.as_bytes()).unwrap();
    assert_eq!(search.issues.len(), 2);
    let detail = decode::<Detail>(DETAIL.as_bytes()).unwrap();
    let lines = detail_lines(&detail, 100);
    let text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "#2180 CLI referral link",
        "Shipped · platform · by skunk-ape",
        "Assignees: skunk-ape",
        "https://swamp-club.com/lab/2180",
        "Print one URL.",
        "Comments (1)",
        "hammz · 2026-09-16T16:20:51.953Z",
        "Depends on #2179.",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(lines
        .iter()
        .flat_map(|line| &line.styles)
        .any(|style| style.bold));
    let empty = decode::<Search>(EMPTY.as_bytes()).unwrap();
    assert!(empty.issues.is_empty());
    let unknown = SEARCH.replace("triaged", "reviewing");
    let search = decode::<Search>(unknown.as_bytes()).unwrap();
    assert!(issue_lines(&search.issues[0], 100, now_ms())[0]
        .text
        .contains("reviewing"));
    let mut value: serde_json::Value = serde_json::from_str(DETAIL).unwrap();
    value["issue"]["assignees"] = json!([]);
    value["comments"] = json!([]);
    let detail = decode::<Detail>(value.to_string().as_bytes()).unwrap();
    assert!(detail.issue.assignees.is_empty() && detail.comments.is_empty());
}

#[test]
fn timestamps_decode_and_render_relative_ages_and_precise_offsets() {
    let mut value: serde_json::Value = serde_json::from_str(DETAIL).unwrap();
    value["issue"]["createdAt"] = json!("2026-09-18T05:00:00-05:00");
    value["issue"]["updatedAt"] = json!("2026-09-18T13:55:00.123+02:00");
    let detail = decode::<Detail>(value.to_string().as_bytes()).unwrap();
    let now = DateTime::parse_from_rfc3339("2026-09-18T12:00:00.123Z")
        .unwrap()
        .timestamp_millis();
    let lines = issue_lines(&detail.issue, 100, now);
    assert_eq!(
        lines.last().unwrap().text,
        "  Created 2h ago · Updated 5m ago"
    );
    assert!(issue_lines(&detail.issue, 100, now + REFRESH_MS)
        .last()
        .unwrap()
        .text
        .contains("Updated 6m ago"));
    let precise = detail_lines(&detail, 100)
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>();
    assert!(precise.contains(&"Created: 2026-09-18T05:00:00-05:00".into()));
    assert!(precise.contains(&"Updated: 2026-09-18T13:55:00.123+02:00".into()));
    for cols in 1..50 {
        assert!(issue_lines(&detail.issue, cols, now)
            .iter()
            .all(|line| line.text.width() <= cols));
        assert!(detail_lines(&detail, cols)
            .iter()
            .all(|line| line.text.width() <= cols));
    }
    assert!(issue_lines(&detail.issue, 100, 0)
        .last()
        .unwrap()
        .text
        .contains("Created now · Updated now"));
}

#[test]
fn missing_null_and_invalid_timestamps_are_unknown() {
    let mut value: serde_json::Value = serde_json::from_str(DETAIL).unwrap();
    for invalid in [
        None,
        Some(json!(null)),
        Some(json!("not a timestamp")),
        Some(json!("2026-09-18T10:00:00")), // no timezone
        Some(json!("2026-02-30T10:00:00Z")),
        Some(json!(42)),
        Some(json!(false)),
        Some(json!({})),
        Some(json!([])),
    ] {
        if let Some(invalid) = invalid {
            value["issue"]["createdAt"] = invalid.clone();
            value["issue"]["updatedAt"] = invalid;
        }
        let detail = decode::<Detail>(value.to_string().as_bytes()).unwrap();
        assert!(detail.issue.created_at.is_none() && detail.issue.updated_at.is_none());
        assert_eq!(
            issue_lines(&detail.issue, 100, now_ms())
                .last()
                .unwrap()
                .text,
            "  Created unknown · Updated unknown"
        );
        let precise = detail_lines(&detail, 100);
        for label in ["Created: unknown", "Updated: unknown"] {
            assert!(precise.iter().any(|line| line.text == label));
        }
    }
}

fn timestamped_search() -> String {
    let mut value: serde_json::Value = serde_json::from_str(SEARCH).unwrap();
    let template = value["issues"][0].clone();
    value["issues"] = json!([]);
    for (number, created, updated) in [
        (
            1,
            Some("2026-09-18T10:00:00Z"),
            Some("2026-09-18T10:00:00Z"),
        ),
        (
            2,
            Some("2026-09-18T09:00:00Z"),
            Some("2026-09-18T12:00:00Z"),
        ),
        (
            3,
            Some("2026-09-18T11:00:00+01:00"),
            Some("2026-09-18T13:00:00+01:00"),
        ),
        (4, None, Some("invalid")),
        (5, Some("invalid"), None),
        (6, None, None),
    ] {
        let mut issue = template.clone();
        issue["number"] = json!(number);
        if number != 6 {
            issue["createdAt"] = json!(created);
            issue["updatedAt"] = json!(updated);
        }
        value["issues"].as_array_mut().unwrap().push(issue);
    }
    value.to_string()
}

#[test]
fn all_sort_orders_use_instants_then_descending_numbers_with_unknowns_last() {
    let mut state = ready();
    state.activate_swamp();
    reply(&mut state, &timestamped_search());
    assert_eq!(state.swamp.sort, 0);
    assert_eq!(state.swamp.selected_number(), Some(3));
    let count = state.swamp.requests.len();
    for (sort, expected) in [
        [3, 2, 1, 6, 5, 4],
        [1, 3, 2, 6, 5, 4],
        [3, 1, 2, 6, 5, 4],
        [2, 3, 1, 6, 5, 4],
    ]
    .into_iter()
    .enumerate()
    {
        choose(&mut state, 'o', SORTS[sort]);
        assert!(!state.swamp.is_editing());
        assert_eq!(state.swamp.sort, sort);
        assert_eq!(
            state
                .swamp
                .issues()
                .iter()
                .map(|issue| issue.number)
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(state.swamp.selected_number(), Some(3));
        assert_eq!(state.swamp.requests.len(), count, "sorting stays local");
        assert!(state.swamp.header(usize::MAX).contains("6 issues"));
    }
    key(&mut state, BareKey::Char('o'));
    key(&mut state, BareKey::Up);
    key(&mut state, BareKey::Esc);
    assert_eq!(state.swamp.sort, 3, "cancel keeps the committed sort");
    key(&mut state, BareKey::Char('o'));
    key(&mut state, BareKey::Up);
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.sort, 2);
    assert_eq!(state.swamp.requests.len(), count);
}

#[test]
fn sorting_and_refresh_reveal_selection_and_keep_sort_across_surfaces() {
    let mut state = ready();
    state.activate_swamp();
    reply(&mut state, &timestamped_search());
    state.render(8, 12);
    state.swamp.list_scroll = Some(usize::MAX);
    choose(&mut state, 'o', "Created oldest first");
    assert_eq!(state.swamp.list_scroll, None);
    state.render(8, 12);
    assert_eq!(state.swamp.selected_number(), Some(3));
    assert_eq!(
        state.swamp.list_rows[state.swamp.window].0,
        state.swamp.selected
    );
    let mut refresh: serde_json::Value = serde_json::from_str(&timestamped_search()).unwrap();
    refresh["issues"][2]["createdAt"] = json!("2026-09-18T08:00:00Z");
    refresh["issues"].as_array_mut().unwrap().reverse();
    key(&mut state, BareKey::Char('r'));
    state.swamp.list_scroll = Some(usize::MAX);
    reply(&mut state, &refresh.to_string());
    assert_eq!(state.swamp.selected_number(), Some(3));
    assert_eq!(state.swamp.selected, 0);
    assert_eq!(state.swamp.list_scroll, None);
    state.render(8, 12);
    assert_eq!(state.swamp.window, 0);
    state.update(pane_update(state.own_plugin_id, true));
    state.update(pane_update(state.own_plugin_id, false));
    reply(&mut state, &refresh.to_string());
    assert_eq!(state.swamp.sort, 3);
    assert_eq!(state.swamp.selected_number(), Some(3));
    assert_eq!(state.swamp.selected, 0);
}

#[test]
fn minute_ticks_redraw_issue_ages_while_a_refresh_is_pending() {
    let mut state = loaded();
    assert!(
        state.swamp.shows_an_age(),
        "the header says how old the list is"
    );
    key(&mut state, BareKey::Char('r'));
    reply(&mut state, &timestamped_search());
    state.render(11, 80);
    assert!(state.swamp.shows_an_age());
    key(&mut state, BareKey::Char('r'));
    let count = state.swamp.requests.len();
    state.age_minute = now_ms() / REFRESH_MS - 1;
    assert!(state.update(Event::Timer(FractionalSeconds::new(0.0))));
    assert_eq!(state.swamp.requests.len(), count);
    assert!(!state.update(Event::Timer(FractionalSeconds::new(0.0))));
    key(&mut state, BareKey::Enter);
    assert!(
        !state.swamp.shows_an_age(),
        "details use precise timestamps"
    );
    key(&mut state, BareKey::Esc);
    state.render(0, 80);
    assert!(!state.swamp.shows_an_age());
    state.render(11, 80);
    state.swamp_visibility(false);
    assert!(!state.swamp.shows_an_age());
}

#[test]
fn list_layout_is_reused_while_navigating_and_typing() {
    let mut state = loaded();
    let poison = markdown::MdLine::plain("cached title");
    for input in [
        BareKey::Down,
        BareKey::Up,
        BareKey::End,
        BareKey::Home,
        BareKey::PageDown,
        BareKey::PageUp,
        BareKey::Char('/'),
        BareKey::Char('a'),
        BareKey::Backspace,
        BareKey::Esc,
        BareKey::Char('f'),
        BareKey::Char('a'),
        BareKey::Esc,
    ] {
        state.swamp.list_rows[1].1 = poison.clone();
        let cache_key = state.swamp.list_cache_key;
        key(&mut state, input);
        state.render(9, 80);
        // Crossing a real minute boundary is allowed to rebuild the layout.
        if state.swamp.list_cache_key == cache_key {
            assert_eq!(state.swamp.list_rows[1].1, poison);
        }
        assert!(state
            .swamp
            .list_rows
            .iter()
            .all(|(_, line)| !line.text.starts_with('>')));
    }
    assert_eq!(state.swamp.issues().len(), 2);
    assert_eq!(state.swamp.requests.len(), 1);
}

#[test]
fn list_layout_is_invalidated_by_filters_sorting_results_and_width() {
    let mut state = loaded();
    choose(&mut state, 's', "triaged");
    assert!(state.swamp.list_rows.is_empty());
    assert!(state.swamp.list_cache_key.is_none());
    assert!(!state.swamp_mouse(Mouse::LeftClick(state.swamp.last_body_top as isize, 0)));
    state.render(9, 80);
    assert_eq!(state.swamp.list_rows.len(), 3);
    assert!(state.swamp.list_rows[0].1.text.contains("#2254"));

    key(&mut state, BareKey::Char('r'));
    reply(&mut state, &timestamped_search());
    assert!(state.swamp.list_rows.is_empty());
    assert!(state.swamp.list_cache_key.is_none());
    state.render(19, 80);
    assert_eq!(state.swamp.list_rows.len(), 18);
    assert!(state.swamp.list_rows[0].1.text.contains("#3"));

    choose(&mut state, 'o', "Created oldest first");
    assert!(state.swamp.list_rows.is_empty());
    assert!(state.swamp.list_cache_key.is_none());
    state.render(19, 80);
    assert!(state.swamp.list_rows[0].1.text.contains("#2"));
    state.render(19, 12);
    assert_eq!(state.swamp.list_cache_key.unwrap().0, 12);
    assert!(state.swamp.list_rows.len() > 18);
    assert!(state
        .swamp
        .list_rows
        .iter()
        .all(|(_, line)| line.text.width() <= 12));

    choose(&mut state, 's', "closed");
    state.render(19, 80);
    assert!(state.swamp.list_rows.is_empty());
    choose(&mut state, 's', "all");
    state.render(19, 80);
    assert_eq!(state.swamp.list_rows.len(), 18);
}

#[test]
fn list_layout_refreshes_age_labels_on_the_next_minute() {
    let mut state = ready();
    state.activate_swamp();
    reply(&mut state, &timestamped_search());
    let now = timestamp(Some("2026-09-18T12:05:00Z"))
        .unwrap()
        .timestamp_millis();
    state.swamp.fill_list_cache(80, now);
    assert!(state.swamp.list_rows[2].1.text.contains("Updated 5m ago"));
    state.swamp.list_rows[1].1 = markdown::MdLine::plain("cached title");
    state.swamp.fill_list_cache(80, now + REFRESH_MS - 1);
    assert_eq!(state.swamp.list_rows[1].1.text, "cached title");
    state.swamp.fill_list_cache(80, now + REFRESH_MS);
    assert_eq!(state.swamp.list_rows[1].1.text, "  Invite output");
    assert!(state.swamp.list_rows[2].1.text.contains("Updated 6m ago"));
}

fn type_text(state: &mut State, text: &str) {
    for ch in text.chars() {
        key(state, BareKey::Char(ch));
    }
}

fn choose(state: &mut State, shortcut: char, text: &str) {
    key(state, BareKey::Char(shortcut));
    type_text(state, text);
    key(state, BareKey::Enter);
}

fn clear_source(state: &mut State) {
    key(state, BareKey::Home);
    state.on_key(KeyWithModifier::new(BareKey::Char('k')).with_ctrl_modifier());
}

#[test]
fn filters_combine_locally_and_source_edits_apply_cancel_clear() {
    let mut state = loaded();
    choose(&mut state, 's', "triaged");
    choose(&mut state, 't', "bug");
    choose(&mut state, 'f', "  swamp  ");
    assert_eq!(state.swamp.source, "swamp");
    assert_eq!(state.swamp.issues().len(), 1);
    assert_eq!(state.swamp.selected_number(), Some(2254));
    assert_eq!(state.swamp.requests.len(), 1);
    choose(&mut state, '/', "two words");
    assert_eq!(state.swamp.requests.len(), 2);
    assert_eq!(
        state.swamp.requests.last().unwrap().0.url,
        search_url("two words")
    );
    reply(&mut state, SEARCH);
    assert_eq!(state.swamp.issues().len(), 1);
    key(&mut state, BareKey::Char('f'));
    key(&mut state, BareKey::Backspace);
    key(&mut state, BareKey::Esc);
    assert_eq!(state.swamp.source, "swamp");
    key(&mut state, BareKey::Char('f'));
    clear_source(&mut state);
    type_text(&mut state, "cli tag");
    key(&mut state, BareKey::Enter);
    assert!(state.swamp.is_editing());
    assert_eq!(state.swamp.source, "swamp");
    assert!(state
        .swamp
        .input_error
        .as_ref()
        .unwrap()
        .contains("must be 1-64 letters"));
    assert!(state.swamp.list_error.is_none());
    clear_source(&mut state);
    key(&mut state, BareKey::Enter);
    assert!(state.swamp.source.is_empty());
    assert!(!state.swamp.is_editing());
    assert!(!state.swamp.has_error());
    assert!(state.swamp.note().is_none());
    choose(&mut state, 's', "all");
    choose(&mut state, 't', "all");
    assert_eq!(state.swamp.issues().len(), 2);
    assert_eq!(state.swamp.requests.len(), 2);
}

#[test]
fn correcting_clearing_or_restoring_a_source_removes_its_error_banner_and_marker() {
    for (previous, corrected) in [
        ("swamp", "swamp-club"),
        ("swamp", ""),
        ("swamp", "swamp"),
        ("", ""),
    ] {
        let mut state = loaded();
        choose(&mut state, 'f', previous);
        key(&mut state, BareKey::Char('f'));
        clear_source(&mut state);
        type_text(&mut state, "invalid source");
        key(&mut state, BareKey::Enter);
        state.render(11, 80);
        assert_eq!(state.swamp.source, previous);
        assert!(state.swamp.has_error());
        assert!(state.swamp.note().unwrap().starts_with("Error:"));
        assert!(state.swamp.has_error());
        let error_body_top = state.swamp.last_body_top;
        let error_rows = state.swamp.note_rows(11, 80).len();
        assert!(error_rows > 0);

        clear_source(&mut state);
        type_text(&mut state, corrected);
        key(&mut state, BareKey::Enter);
        state.render(11, 80);
        assert_eq!(state.swamp.source, corrected);
        assert!(!state.swamp.is_editing());
        assert!(!state.swamp.has_error());
        assert!(state.swamp.note().is_none());
        assert!(!state.swamp.has_error());
        assert_eq!(state.swamp.last_body_top + error_rows, error_body_top);
        assert_eq!(state.swamp.requests.len(), 1);
    }
}

#[test]
fn select_typeahead_is_provisional_and_unmatched_input_cannot_commit() {
    let mut state = loaded();
    let count = state.swamp.requests.len();
    key(&mut state, BareKey::Char('s'));
    type_text(&mut state, "iprg");
    assert_eq!(state.swamp.status, 0);
    assert_eq!(state.swamp.requests.len(), count);
    key(&mut state, BareKey::Enter);
    assert_eq!(STATUSES[state.swamp.status], "in_progress");
    assert!(!state.swamp.is_editing());
    reply(&mut state, SEARCH);
    let count = state.swamp.requests.len();
    choose(&mut state, 's', "iprg");
    assert_eq!(
        state.swamp.requests.len(),
        count,
        "unchanged commit does not search"
    );
    key(&mut state, BareKey::Char('t'));
    type_text(&mut state, "zzzz");
    key(&mut state, BareKey::Enter);
    assert!(state.swamp.is_editing());
    assert_eq!(state.swamp.kind, 0);
    assert_eq!(state.swamp.requests.len(), count);
    key(&mut state, BareKey::Esc);
    key(&mut state, BareKey::Char('t'));
    key(&mut state, BareKey::Down);
    key(&mut state, BareKey::Esc);
    assert_eq!(state.swamp.kind, 0);
    assert_eq!(state.swamp.requests.len(), count);
    key(&mut state, BareKey::Char('t'));
    key(&mut state, BareKey::Down);
    key(&mut state, BareKey::Enter);
    assert_eq!(TYPES[state.swamp.kind], "bug");
}

#[test]
fn source_suggestions_remember_only_committed_unique_tags() {
    let mut state = loaded();
    choose(&mut state, 'f', "custom");
    reply(&mut state, SEARCH);
    choose(&mut state, 'f', "");
    assert_eq!(
        state.swamp.requests.len(),
        1,
        "unchanged source does not search"
    );
    key(&mut state, BareKey::Char('f'));
    clear_source(&mut state);
    type_text(&mut state, "cancelled");
    key(&mut state, BareKey::Esc);
    key(&mut state, BareKey::Char('f'));
    assert_eq!(
        state
            .swamp
            .filter
            .as_ref()
            .unwrap()
            .form
            .combo_by_id("value")
            .unwrap()
            .suggestions(),
        ["swamp", "custom"]
    );
    clear_source(&mut state);
    key(&mut state, BareKey::Down);
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.source, "swamp");
    assert_eq!(state.swamp.source_tags, ["custom"]);
    reply(&mut state, SEARCH);
    key(&mut state, BareKey::Char('f'));
    clear_source(&mut state);
    type_text(&mut state, "cstm");
    key(&mut state, BareKey::Down);
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.source, "custom");
    assert_eq!(state.swamp.source_tags, ["custom"]);
}

#[test]
fn filter_mouse_selects_on_release_and_intercepts_issue_rows() {
    for shortcut in ['s', 't', 'f', 'o', 'u'] {
        let mut state = loaded();
        let (row, x, _, _) = *state
            .swamp
            .filter_cells
            .iter()
            .find(|cell| cell.3 == shortcut)
            .unwrap();
        state.on_mouse(Mouse::LeftClick((row as isize + 1) - 1, x));
        state.render(13, 80);
        state.on_mouse(Mouse::Release((row as isize + 1) - 1, x));
        assert!(state.swamp.is_editing(), "opening release must not commit");
        let rect = state
            .swamp
            .filter
            .as_ref()
            .unwrap()
            .form
            .overlay_rect()
            .unwrap();
        let y = rect.y as isize + if shortcut == 'f' { 0 } else { 1 };
        let count = state.swamp.requests.len();
        state.on_mouse(Mouse::LeftClick(y, rect.x));
        state.render(13, 80);
        assert_eq!(state.swamp.requests.len(), count);
        assert!(state.swamp.opened.is_none());
        state.on_mouse(Mouse::Release(y + 1, rect.x));
        assert!(
            state.swamp.is_editing(),
            "release off the pressed option cancels the click"
        );
        state.on_mouse(Mouse::LeftClick(y, rect.x));
        state.on_mouse(Mouse::Release(y, rect.x));
        assert!(!state.swamp.is_editing());
        assert!(state.swamp.opened.is_none());
        let expected_count = count;
        assert_eq!(state.swamp.requests.len(), expected_count);
        match shortcut {
            's' => assert_eq!(STATUSES[state.swamp.status], "open"),
            't' => assert_eq!(TYPES[state.swamp.kind], "bug"),
            'o' => assert_eq!(state.swamp.sort, 1),
            'u' => assert_eq!(state.swamp.assignee.as_deref(), Some("")),
            _ => assert_eq!(state.swamp.source, "swamp"),
        }
        reply(&mut state, SEARCH);
        key(&mut state, BareKey::Char(shortcut));
        state.render(13, 80);
        // On an issue row, clear of the dropdown hanging under the chip.
        let line = state.swamp.last_body_top as isize + 1;
        let overlay = state.swamp.filter.as_ref().unwrap().form.overlay_rect();
        let column = if overlay.is_some_and(|rect| rect.x > 0) {
            0
        } else {
            79
        };
        state.on_mouse(Mouse::LeftClick((line) - 1, column));
        assert!(!state.swamp.is_editing());
        // The click that closed the editor was not a click on the row under it.
        state.on_mouse(Mouse::Release((line) - 1, column));
        assert!(state.swamp.opened.is_none());
        assert_eq!(state.swamp.requests.len(), expected_count);
    }
}

#[test]
fn native_filter_mouse_routes_content_rows_to_the_form() {
    let mut state = loaded();
    state.render(13, 80);
    let (row, x, _, _) = *state
        .swamp
        .filter_cells
        .iter()
        .find(|cell| cell.3 == 's')
        .unwrap();
    state.update(Event::Mouse(Mouse::LeftClick(row as isize, x)));
    state.render(13, 80);
    state.update(Event::Mouse(Mouse::Release(row as isize, x)));
    let rect = state
        .swamp
        .filter
        .as_ref()
        .unwrap()
        .form
        .overlay_rect()
        .unwrap();
    let y = rect.y as isize + 1;
    state.update(Event::Mouse(Mouse::LeftClick(y, rect.x)));
    state.update(Event::Mouse(Mouse::Release(y, rect.x)));
    assert_eq!(STATUSES[state.swamp.status], "open");
    assert!(!state.swamp.is_editing());

    reply(&mut state, SEARCH);
    key(&mut state, BareKey::Char('s'));
    state.render(13, 80);
    let count = state.swamp.requests.len();
    state.update(Event::Mouse(Mouse::LeftClick(
        state.swamp.last_body_top as isize,
        79,
    )));
    assert!(!state.swamp.is_editing());
    assert_eq!(state.swamp.requests.len(), count);
}

#[test]
fn filter_chips_and_active_forms_fit_resized_panes() {
    let mut state = loaded();
    for shortcut in ['s', 't', 'f', 'o', 'u'] {
        key(&mut state, BareKey::Char(shortcut));
        for cols in 1..80 {
            for rows in [2, 4, 12] {
                state.render(usize::saturating_sub(rows, 1), cols);
                for (row, x, width, _) in &state.swamp.filter_cells {
                    assert!(x + width <= cols);
                    assert!(*row < state.swamp.last_body_top);
                }
                let editor = state.swamp.filter.as_ref().unwrap();
                for rect in editor.row.into_iter().chain(editor.form.overlay_rect()) {
                    assert!(rect.x + rect.columns <= cols);
                    assert!(rect.y + rect.rows <= rows);
                }
            }
        }
        state.render(0, 1);
        let editor = state.swamp.filter.as_ref().unwrap();
        assert!(editor.row.is_none() && editor.form.overlay_rect().is_none());
        key(&mut state, BareKey::Esc);
    }
    state.render(11, 20);
    assert_eq!(state.swamp.filter_cells.len(), 5);
    let chip_rows: BTreeSet<_> = state.swamp.filter_cells.iter().map(|cell| cell.0).collect();
    assert!(
        chip_rows.len() < 5,
        "narrow chips share rows: {chip_rows:?}"
    );
    let line = state.swamp.last_body_top as isize + 1;
    assert!(state.on_mouse(Mouse::LeftClick((line) - 1, 0)));
    assert_eq!(state.swamp.opened, None, "a press only selects");
    assert!(state.on_mouse(Mouse::Release((line) - 1, 0)));
    assert_eq!(state.swamp.opened, Some(2254));
}

#[test]
fn leaving_the_surface_cancels_filter_edits_and_keeps_committed_values() {
    let mut state = loaded();
    choose(&mut state, 't', "security");
    reply(&mut state, SEARCH);
    for leave in 0..4 {
        key(&mut state, BareKey::Char('f'));
        type_text(&mut state, "provisional");
        match leave {
            0 => {
                state.update(pane_update(state.own_plugin_id, true));
            },
            // hocus putting the panel away, as `vms::VISIBILITY` hands it on.
            1 => {
                state.update(pane_update(state.own_plugin_id, true));
            },
            2 => {
                state.update(pane_update(state.own_plugin_id, true));
            },
            _ => state.hide_the_panel(),
        }
        assert!(!state.swamp.is_editing());
        assert!(state.swamp.source.is_empty());
        assert_eq!(TYPES[state.swamp.kind], "security");
        state.update(pane_update(state.own_plugin_id, false));
        state.update(pane_update(state.own_plugin_id, false));
        state.update(pane_update(state.own_plugin_id, false));
        reply(&mut state, SEARCH);
    }
}

#[test]
fn malformed_replies_and_http_errors_are_explained() {
    for bytes in [
        &b""[..],
        b"not JSON",
        b"{}",
        br#"{"issues":{},"total":1}"#,
        br#"{"issues":[],"total":"1"}"#,
        b"\xff",
    ] {
        assert!(decode::<Search>(bytes)
            .unwrap_err()
            .contains("unexpected reply from swamp-club"));
    }
    let bad = DETAIL.replace(
        r#""createdAt":"2026-09-16T16:20:51.953Z""#,
        r#""createdAt":42"#,
    );
    assert!(decode::<Detail>(bad.as_bytes()).is_err());
    assert_eq!(
        Search::from_reply(400, br#"{"error":"Invalid status filter"}"#).unwrap_err(),
        "HTTP 400: Invalid status filter"
    );
    assert!(Detail::from_reply(404, br#"{"error":"Issue not found"}"#)
        .unwrap_err()
        .contains("Issue not found"));
    assert!(Search::from_reply(401, br#"{"error":"Unauthorized"}"#)
        .unwrap_err()
        .contains("swamp auth login"));
    let limited = swamp_lab::parse::<lab::SearchResponse>(
        429,
        &BTreeMap::from([("retry-after".into(), "30".into())]),
        br#"{"error":"Rate limit exceeded"}"#,
    )
    .unwrap_err();
    assert!(limited.to_string().contains("retry in 30s"));
}

#[test]
fn search_text_is_one_encoded_parameter_and_statuses_use_the_api_spelling() {
    assert_eq!(
        search_url(""),
        "https://swamp-club.com/api/v1/lab/issues?limit=200"
    );
    let query = "two words; $(touch /tmp/no) `echo no` --help";
    assert_eq!(
        search_url(query),
        "https://swamp-club.com/api/v1/lab/issues?q=two+words%3B+%24%28touch+%2Ftmp%2Fno%29+%60echo+no%60+--help\
         &limit=200"
    );
    assert!(search_query(&"q".repeat(201), 0).is_err());
    let mut state = loaded();
    for status in STATUSES.iter().skip(1).chain([&STATUSES[0]]) {
        choose(&mut state, 's', status_label(status));
        let url = &state.swamp.requests.last().unwrap().0.url;
        assert_eq!(*url, search_url(""));
        reply(&mut state, SEARCH);
    }
    assert_eq!(state.swamp.status, 0);
    key(&mut state, BareKey::Char('/'));
    for ch in "q".repeat(201).chars() {
        key(&mut state, BareKey::Char(ch));
    }
    let commands = state.swamp.requests.len();
    key(&mut state, BareKey::Enter);
    assert!(state.swamp.is_editing());
    assert_eq!(state.swamp.query, "");
    assert_eq!(state.swamp.requests.len(), commands);
}

#[test]
fn search_edits_apply_or_cancel_and_superseded_replies_cannot_win() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('/'));
    for ch in "first".chars() {
        key(&mut state, BareKey::Char(ch));
    }
    key(&mut state, BareKey::Esc);
    assert_eq!(state.swamp.query, "");
    assert_eq!(state.swamp.requests.len(), 1);
    key(&mut state, BareKey::Char('/'));
    key(&mut state, BareKey::Char('a'));
    key(&mut state, BareKey::Enter);
    let old = context(&state);
    key(&mut state, BareKey::Char('/'));
    key(&mut state, BareKey::Backspace);
    key(&mut state, BareKey::Char('b'));
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.requests.len(), 2, "wait for the first process");
    assert_eq!(state.swamp.query, "b");
    reply(&mut state, EMPTY);
    assert_eq!(
        state.swamp.issues().len(),
        0,
        "old query cannot populate a new query"
    );
    assert_eq!(state.swamp.requests.len(), 3);
    assert!(state.swamp.requests.last().unwrap().0.url.contains("?q=b&"));
    assert!(!state.swamp_web_result(200, &server(), SEARCH.as_bytes(), &old));
    assert!(state.swamp.pending.is_some());
    reply(&mut state, EMPTY);
    assert!(state.swamp.issues().is_empty());
    assert!(state.swamp.pending.is_none());
    key(&mut state, BareKey::Char('/'));
    key(&mut state, BareKey::Backspace);
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.issues().len(), 2);
    assert_eq!(
        state.swamp.requests.len(),
        3,
        "clearing search reuses the catalogue"
    );
}

#[test]
fn clearing_a_pending_search_repaints_when_the_obsolete_reply_removes_loading() {
    for visibility in ["visible", "hidden", "other surface"] {
        let mut state = loaded();
        let catalogue = serde_json::to_value(&state.swamp.search).unwrap();
        choose(&mut state, '/', "a");
        let pending = context(&state);
        key(&mut state, BareKey::Char('/'));
        key(&mut state, BareKey::Backspace);
        key(&mut state, BareKey::Enter);
        assert_eq!(state.swamp.note().as_deref(), Some("Loading…"));
        assert_eq!(
            serde_json::to_value(&state.swamp.search).unwrap(),
            catalogue
        );
        match visibility {
            "hidden" => {
                state.swamp_visibility(false);
            },
            "other surface" => {
                state.update(pane_update(state.own_plugin_id, true));
            },
            _ => {},
        }
        assert_eq!(
            state.swamp_web_result(200, &server(), EMPTY.as_bytes(), &pending),
            visibility == "visible"
        );
        assert!(state.swamp.note().is_none());
        assert!(state.swamp.pending.is_none());
        assert_eq!(
            serde_json::to_value(&state.swamp.search).unwrap(),
            catalogue
        );
        assert_eq!(state.swamp.requests.len(), 2);
        assert!(!state.swamp_web_result(200, &server(), EMPTY.as_bytes(), &pending));
    }
}

#[test]
fn refreshing_preserves_selection_and_content_after_failure() {
    let mut state = loaded();
    state.swamp.selected = 1;
    state.swamp.list_scroll = Some(1);
    key(&mut state, BareKey::Char('r'));
    let mut reordered: serde_json::Value = serde_json::from_str(SEARCH).unwrap();
    reordered["issues"].as_array_mut().unwrap().reverse();
    reply(&mut state, &reordered.to_string());
    assert_eq!(state.swamp.selected, 1);
    assert_eq!(state.swamp.list_scroll, None);
    key(&mut state, BareKey::Char('r'));
    reply_status(&mut state, 503, r#"{"error":"Network unavailable"}"#);
    assert_eq!(state.swamp.issues()[state.swamp.selected].number, 2180);
    assert!(state.swamp.note().unwrap().contains("Network unavailable"));
    key(&mut state, BareKey::Char('r'));
    reply(&mut state, SEARCH);
    assert_eq!(state.swamp.selected, 1);
    assert!(!state.swamp.has_error());
    key(&mut state, BareKey::Char('r'));
    reply(&mut state, EMPTY);
    assert_eq!(state.swamp.selected, 0);
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.opened, None);
}

#[test]
fn timer_ticks_and_returning_to_panel_do_not_reload_issues() {
    let mut state = reader();
    let count = state.swamp.requests.len();
    for _ in 0..3 {
        assert!(!state.update(Event::Timer(FractionalSeconds::new(0.0))));
    }
    state.update(pane_update(state.own_plugin_id, true));
    assert!(!state.activate_swamp());
    state.update(pane_update(state.own_plugin_id, false));
    state.swamp_visibility(false);
    assert!(!state.activate_swamp());
    state.swamp_visibility(true);
    state.update(Event::Timer(FractionalSeconds::new(0.0)));
    assert_eq!(state.swamp.requests.len(), count);
    key(&mut state, BareKey::Char('r'));
    assert_eq!(state.swamp.requests.len(), count + 1);
    assert!(!state.update(Event::Timer(FractionalSeconds::new(0.0))));
    // The issue on screen first, then the catalogue behind it.
    assert_eq!(
        state.swamp.requests.last().unwrap().0.url,
        "https://swamp-club.com/api/v1/lab/issues/2180"
    );
    reply(&mut state, DETAIL);
    assert_eq!(state.swamp.requests.last().unwrap().0.url, search_url(""));
    reply(&mut state, SEARCH);
    assert_eq!(state.swamp.requests.len(), count + 2);
}

#[test]
fn polling_waits_for_pane_visibility_confirmation() {
    for surface_event in [false, true] {
        let mut state = State {
            ..Default::default()
        };
        state.swamp.permission = Permission::Granted;
        state.swamp.credentials = Some(Credentials::anonymous());
        state.swamp.keep_credentials = true;
        assert!(!state.activate_swamp());
        if surface_event {
            assert!(!state.update(Event::Visible(true)));
        }
        assert!(!state.update(Event::PaneUpdate(PaneManifest::default())));
        assert!(!state.activate_swamp());
        assert!(state.swamp.requests.is_empty());
        assert!(state.update(pane_update(state.own_plugin_id, false)));
        assert_eq!(state.swamp.requests.len(), 1);
    }
}

#[test]
fn pane_suppression_and_visibility_events_control_polling() {
    for surface_first in [false, true] {
        let mut state = ready();
        state.update(Event::Visible(false));
        state.update(pane_update(state.own_plugin_id, true));
        let mut restore = [
            Event::Visible(true),
            pane_update(state.own_plugin_id, false),
        ];
        if !surface_first {
            restore.reverse();
        }
        assert!(!state.update(restore[0].clone()));
        assert!(!state.update(restore[0].clone()));
        assert!(!state.update(Event::PaneUpdate(PaneManifest::default())));
        assert!(!state.swamp.visible());
        assert!(!state.activate_swamp());
        assert!(state.swamp.requests.is_empty());

        assert!(state.update(restore[1].clone()));
        assert!(state.swamp.visible());
        assert_eq!(state.swamp.requests.len(), 1);
        reply(&mut state, SEARCH);
        for event in restore {
            assert!(!state.update(event));
        }
        assert!(!state.update(Event::PaneUpdate(PaneManifest::default())));
        assert!(state.swamp.visible());
        assert_eq!(state.swamp.requests.len(), 1, "refresh only on reopening");
    }
}

#[test]
fn dismissal_preserves_surface_state_and_hidden_results_wait_for_reopening() {
    for surface_hidden in [false, true] {
        let mut state = reader();
        key(&mut state, BareKey::Char('r'));
        let count = state.swamp.requests.len();
        let pending = context(&state);
        if surface_hidden {
            state.update(Event::Visible(false));
        }
        state.hide_the_panel();
        assert_eq!(state.swamp.surface_hidden, surface_hidden);
        assert!(!state.swamp.pane_visible);
        assert_eq!(
            state.swamp.pending.as_ref().unwrap().id,
            pending[REQUEST_ID]
        );
        if surface_hidden {
            assert!(!state.update(pane_update(state.own_plugin_id, false)));
        } else {
            assert!(!state.update(Event::Visible(true)));
        }
        assert!(!state.activate_swamp());
        // The reader's issue was asked for first; the catalogue behind it waits.
        assert!(!state.update(Event::WebRequestResult(
            200,
            server(),
            DETAIL.as_bytes().to_vec(),
            pending,
        )));
        assert!(state.swamp.pending.is_none());
        assert!(state.swamp.loading.is_some());
        assert_eq!(state.swamp.requests.len(), count);

        if surface_hidden {
            assert!(state.update(Event::Visible(true)));
        } else {
            assert!(state.update(pane_update(state.own_plugin_id, false)));
        }
        assert_eq!(state.swamp.requests.len(), count + 1);
        assert_eq!(state.swamp.requests.last().unwrap().0.url, search_url(""));
        reply(&mut state, SEARCH);
        assert!(!state.update(pane_update(state.own_plugin_id, false)));
        assert!(!state.update(Event::Visible(true)));
        assert_eq!(state.swamp.requests.len(), count + 1);
    }
}

#[test]
fn permission_denial_can_be_retried_and_grants_while_hidden_do_not_poll() {
    for surface_hidden in [false, true] {
        let mut state = ready();
        state.swamp.permission = Permission::Unrequested;
        state.activate_swamp();
        assert!(state.swamp.requests.is_empty());
        assert!(
            state.swamp.permission == Permission::Unrequested,
            "arriving on the tab asks the host nothing"
        );
        assert!(state.swamp.note().unwrap().ends_with("r to allow"));
        key(&mut state, BareKey::Char('r'));
        assert!(state.swamp.permission == Permission::Waiting);
        state.update(Event::PermissionRequestResult(PermissionStatus::Denied));
        assert!(state.swamp.note().unwrap().contains("Permission denied"));
        state.update(Event::Timer(FractionalSeconds::new(0.0)));
        assert!(state.swamp.permission == Permission::Denied);
        key(&mut state, BareKey::Char('r'));
        assert!(state.swamp.permission == Permission::Waiting);
        if surface_hidden {
            state.update(Event::Visible(false));
            state.update(pane_update(state.own_plugin_id, false));
        } else {
            state.hide_the_panel();
            state.update(Event::Visible(true));
        }
        assert!(!state.update(Event::PermissionRequestResult(PermissionStatus::Granted)));
        assert!(state.swamp.requests.is_empty());
        if surface_hidden {
            state.update(Event::Visible(true));
        } else {
            state.update(pane_update(state.own_plugin_id, false));
        }
        assert_eq!(state.swamp.requests.len(), 1);
    }
}

#[test]
fn detail_replies_match_both_request_and_issue_and_survive_refresh_failure() {
    let mut state = reader();
    state.swamp.detail_scroll = 4;
    // A refresh in the reader asks for its issue ahead of the catalogue.
    key(&mut state, BareKey::Char('r'));
    let mut wrong_context = context(&state);
    wrong_context.insert(ISSUE_NUMBER.into(), "2254".into());
    assert!(!state.swamp_web_result(200, &server(), DETAIL.as_bytes(), &wrong_context));
    assert!(state.swamp.pending.is_some());
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    assert!(state
        .swamp
        .detail_error
        .as_ref()
        .unwrap()
        .contains("wrong issue number"));
    assert_eq!(state.swamp.detail.as_ref().unwrap().issue.number, 2180);
    assert_eq!(state.swamp.detail_scroll, 4);
    reply(&mut state, SEARCH);
    key(&mut state, BareKey::Char('r'));
    reply(&mut state, DETAIL);
    reply(&mut state, SEARCH);
    assert_eq!(state.swamp.detail_scroll, 4);
    assert!(!state.swamp.has_error());

    key(&mut state, BareKey::Char('r'));
    let old = context(&state);
    key(&mut state, BareKey::Esc);
    key(&mut state, BareKey::Up);
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.opened, Some(2254));
    state.swamp_web_result(200, &server(), DETAIL.as_bytes(), &old);
    assert!(state.swamp.detail.is_none());
    assert_eq!(context(&state).get(ISSUE_NUMBER).unwrap(), "2254");
    assert!(!state.swamp_web_result(200, &server(), DETAIL.as_bytes(), &old));
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    assert_eq!(state.swamp.detail.as_ref().unwrap().issue.number, 2254);
}

#[test]
fn reader_scrolls_with_keys_and_wheel_and_narrow_renders_stay_bounded() {
    let mut state = reader();
    key(&mut state, BareKey::Down);
    assert_eq!(state.swamp.detail_scroll, 1);
    state.render(9, 80);
    key(&mut state, BareKey::PageDown);
    assert_eq!(state.swamp.detail_scroll, state.swamp.max_scroll);
    state.on_mouse(Mouse::Hover((3) - 1, 0));
    state.on_mouse(Mouse::ScrollUp(1));
    assert_eq!(state.swamp.detail_scroll, state.swamp.max_scroll - 1);
    key(&mut state, BareKey::Home);
    assert_eq!(state.swamp.detail_scroll, 0);
    key(&mut state, BareKey::End);
    assert_eq!(state.swamp.detail_scroll, state.swamp.max_scroll);
    state.render(0, 80);
    let scroll = state.swamp.detail_scroll;
    assert!(!state.on_mouse(Mouse::ScrollDown(2)));
    assert_eq!(state.swamp.detail_scroll, scroll);
    for cols in 0..40 {
        for rows in [0, 1, 2, 3, 10] {
            state.render(usize::saturating_sub(rows, 1), cols);
            assert!(state.swamp.last_body_rows <= usize::saturating_sub(rows, 1));
        }
        if cols > 0 {
            let detail = decode::<Detail>(DETAIL.as_bytes()).unwrap();
            assert!(detail_lines(&detail, cols)
                .iter()
                .all(|line| line.text.width() <= cols));
        }
        assert!(issue_lines(&state.swamp.issues()[0], cols, now_ms())
            .iter()
            .all(|line| line.text.width() <= cols));
    }
    key(&mut state, BareKey::Esc);
    state.render(7, 4);
    assert!(state.swamp.header(usize::MAX).contains("2 issues"));
}

#[test]
fn titles_wrap_without_losing_words_and_stay_bounded() {
    let mut state = loaded();
    let issue = &mut state.swamp.search.as_mut().unwrap().issues[0];
    issue.title = "alpha beta\ngamma\nabcdefghijk\n界面 é".into();
    let lines = issue_lines(issue, 8, now_ms());
    assert_eq!(
        lines[1..7]
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        &[
            "  alpha",
            "  beta",
            "  gamma",
            "  abcdef",
            "  ghijk",
            "  界面 é"
        ]
    );
    assert!(!lines[0].text.contains("alpha"));
    for line in &lines[1..7] {
        assert_eq!(line.styles.len(), line.text.chars().count());
        assert!(line.styles[2..].iter().all(|style| *style == TITLE));
    }
    for cols in 0..20 {
        assert!(issue_lines(issue, cols, now_ms())
            .iter()
            .all(|line| line.text.width() <= cols));
    }
    for selected in [false, true] {
        for line in &lines {
            assert_eq!(
                list_text(line, selected, false)
                    .serialize()
                    .starts_with('x'),
                selected
            );
        }
    }
}

#[test]
fn issue_colors_survive_selection_wrapping_and_clipping() {
    let mut detail = decode::<Detail>(DETAIL.as_bytes()).unwrap();
    detail.issue.title = "界面 é title".into();
    detail.issue.kind = "種類".into();
    for (status, color) in [
        ("open", Some(1)),
        ("triaged", Some(0)),
        ("in_progress", Some(3)),
        ("shipped", Some(2)),
        ("closed", None),
        ("未知", None),
    ] {
        detail.issue.status = status.into();
        let expected_status = markdown::MdStyle {
            color,
            ..Default::default()
        };
        for selected in [false, true] {
            let lines = issue_lines(&detail.issue, 100, 0);
            assert!(!lines[0].text.starts_with('>'));
            assert_eq!(
                list_text(&lines[0], selected, true)
                    .content()
                    .starts_with('>'),
                selected
            );
            let start = format!("  #{} ", detail.issue.number).chars().count();
            let end = start + status_label(status).chars().count();
            assert!(lines[0].styles[1..start].iter().all(|s| *s == METADATA));
            assert!(lines[0].styles[start..end]
                .iter()
                .all(|s| *s == expected_status));
            assert!(lines[0].styles[end..].iter().all(|s| *s == METADATA));
            assert!(lines[1].styles[2..].iter().all(|s| *s == TITLE));
            assert!(lines[2].styles[2..].iter().all(|s| *s == METADATA));
            for cols in 0..30 {
                let clipped = issue_lines(&detail.issue, cols, 0).remove(0);
                assert!(list_text(&clipped, selected, true).content().width() <= cols);
                assert_eq!(
                    clipped.styles,
                    lines[0].styles[..clipped.text.chars().count()]
                );
            }
            for line in &lines {
                assert_eq!(
                    list_text(line, true, false).serialize(),
                    format!("x{}", list_text(line, false, false).serialize())
                );
            }
        }
        let headers = detail_lines(&detail, 100);
        let title_start = format!("#{} ", detail.issue.number).chars().count();
        assert!(headers[0].styles[..title_start]
            .iter()
            .all(|s| *s == METADATA));
        assert!(headers[0].styles[title_start..].iter().all(|s| *s == TITLE));
        let status_end = status_label(status).chars().count();
        assert!(headers[1].styles[..status_end]
            .iter()
            .all(|s| *s == expected_status));
        assert!(headers[1].styles[status_end..]
            .iter()
            .all(|s| *s == METADATA));
        assert!(headers[2..6]
            .iter()
            .flat_map(|line| &line.styles)
            .all(|s| *s == METADATA));
        for cols in 1..30 {
            let wrapped = detail_lines(&detail, cols);
            // Wrapping may discard spaces, but every visible header character keeps its style.
            let expected = headers[..6]
                .iter()
                .flat_map(|line| line.text.chars().zip(&line.styles))
                .filter(|(ch, _)| !ch.is_whitespace())
                .collect::<Vec<_>>();
            let actual = wrapped
                .iter()
                .take_while(|line| !line.text.is_empty())
                .flat_map(|line| line.text.chars().zip(&line.styles))
                .filter(|(ch, _)| !ch.is_whitespace())
                .collect::<Vec<_>>();
            if cols >= 2 {
                assert_eq!(actual, expected, "width {cols}");
            }
            for line in wrapped
                .iter()
                .chain(issue_lines(&detail.issue, cols, 0).iter())
            {
                assert!(line.text.width() <= cols);
                assert!(line.styles.is_empty() || line.styles.len() == line.text.chars().count());
            }
        }
    }
}

#[test]
fn page_navigation_counts_rendered_rows_instead_of_issues() {
    let mut state = loaded();
    let issues = (0..20)
        .map(|_| {
            decode::<Search>(SEARCH.as_bytes())
                .unwrap()
                .issues
                .remove(0)
        })
        .collect();
    state.swamp.cache.catalogue = Some(Search {
        issues,
        fetched_at: 0,
    });
    state.swamp.filter_issues();
    state.render_swamp(8, 80);
    key(&mut state, BareKey::PageDown);
    assert_eq!(state.swamp.selected, 1);
    state.render_swamp(8, 80);
    key(&mut state, BareKey::PageUp);
    assert_eq!(state.swamp.selected, 0);
    key(&mut state, BareKey::End);
    state.render_swamp(8, 80);
    assert_eq!(state.swamp.selected, 19);
    assert_eq!(state.swamp.window, state.swamp.max_scroll);
    key(&mut state, BareKey::Home);
    state.render_swamp(8, 80);
    assert_eq!(state.swamp.window, 0);
}

#[test]
fn wrapped_rows_drive_clicks_navigation_scroll_and_resize() {
    let mut state = loaded();
    state.swamp.search.as_mut().unwrap().issues[0].title = "word ".repeat(40);
    state.render_swamp(10, 12);
    let first_end = state
        .swamp
        .list_rows
        .iter()
        .position(|(index, _)| *index == 1)
        .unwrap();
    assert!(first_end > state.swamp.last_body_rows);
    assert_eq!(state.swamp.window, 0);
    state.swamp_mouse(Mouse::ScrollDown(3));
    state.render_swamp(10, 12);
    assert_eq!(state.swamp.window, 3);
    assert_eq!(state.swamp.selected, 0);
    let line = state.swamp.last_body_top as isize;
    assert!(state.swamp_mouse(Mouse::LeftClick(line, 0)));
    assert!(state.swamp_mouse(Mouse::Release(line, 0)));
    assert_eq!(state.swamp.opened, Some(2254));
    key(&mut state, BareKey::Esc);
    state.render_swamp(10, 12);
    key(&mut state, BareKey::Down);
    state.render_swamp(10, 12);
    assert_eq!(state.swamp.selected, 1);
    assert_eq!(state.swamp.window, first_end);
    assert_eq!(state.swamp.list_rows[state.swamp.window].0, 1);
    key(&mut state, BareKey::Up);
    state.render_swamp(10, 12);
    assert_eq!(state.swamp.window, 0);
    key(&mut state, BareKey::PageDown);
    assert_eq!(state.swamp.selected, 1);
    key(&mut state, BareKey::PageUp);
    assert_eq!(state.swamp.selected, 0);
    state.render_swamp(10, 12);
    state.swamp_mouse(Mouse::ScrollDown(usize::MAX));
    state.render_swamp(10, 12);
    assert_eq!(state.swamp.window, state.swamp.max_scroll);
    state.render_swamp(20, 100);
    assert_eq!(state.swamp.window, 0);
    assert_eq!(state.swamp.list_scroll, Some(0));
    let continuation = state
        .swamp
        .list_rows
        .iter()
        .rposition(|(index, _)| *index == 1)
        .unwrap();
    let line = (state.swamp.last_body_top + continuation) as isize;
    assert!(state.swamp_mouse(Mouse::LeftClick(line, 0)));
    assert!(state.swamp_mouse(Mouse::Release(line, 0)));
    assert_eq!(state.swamp.opened, Some(2180));
}

/// Answers the pending `auth.json` read with `file` as its output.
fn answer_login(state: &mut State, file: &[u8]) {
    let context = state.swamp.commands.last().unwrap().1.clone();
    state.swamp_result(Some(0), file, &[], &context);
}

fn login(key: &str) -> Vec<u8> {
    format!(r#"{{"serverUrl":"https://lab.example.test","apiKey":"{key}","username":"keeper"}}"#)
        .into_bytes()
}

fn sent_key(state: &State) -> Option<String> {
    let (request, _) = state.swamp.requests.last().unwrap();
    request.headers.get("x-api-key").cloned()
}

fn page(numbers: &[u64], total: u64) -> String {
    let mut value: serde_json::Value = serde_json::from_str(SEARCH).unwrap();
    let template = value["issues"][0].clone();
    value["issues"] = numbers
        .iter()
        .map(|number| {
            let mut issue = template.clone();
            issue["number"] = json!(number);
            issue
        })
        .collect();
    value["total"] = json!(total);
    value.to_string()
}

#[test]
fn assignees_filter_all_any_person_or_unassigned_and_combine_with_other_filters() {
    let mut state = ready();
    state.activate_swamp();
    let mut data: serde_json::Value = serde_json::from_str(&page(&[1, 2, 3, 4], 4)).unwrap();
    data["issues"][0]["assignees"] = json!([
        {"userId":"z", "username":"zoe"}, {"userId":"a", "username":"amy"}
    ]);
    data["issues"][1]["assignees"] = json!([{"userId":"a", "username":"amy"}]);
    data["issues"][1]["source"] = json!("swamp-club");
    data["issues"][2]["assignees"] = json!([{"userId":"z", "username":"zoe"}]);
    data["issues"][2]["status"] = json!("closed");
    data["issues"][2]["type"] = json!("feature");
    reply(&mut state, &data.to_string());
    assert_eq!(
        state.swamp.assignee_options(),
        ["All", "Unassigned", "amy", "zoe"]
    );
    choose(&mut state, 'u', "zoe");
    assert_eq!(
        state
            .swamp
            .issues()
            .iter()
            .map(|issue| issue.number)
            .collect::<Vec<_>>(),
        [3, 1]
    );
    key(&mut state, BareKey::Down);
    choose(&mut state, 's', "triaged");
    choose(&mut state, 't', "bug");
    choose(&mut state, 'f', "swamp");
    assert_eq!(state.swamp.selected_number(), Some(1));
    choose(&mut state, 'u', "amy");
    assert_eq!(state.swamp.selected_number(), Some(1));
    assert_eq!(state.swamp.issues().len(), 1);
    choose(&mut state, 'u', "Unassigned");
    assert_eq!(state.swamp.selected_number(), Some(4));
    assert!(state
        .swamp
        .header(usize::MAX)
        .contains("1 of 4 issues · cached"));
    choose(&mut state, 'u', "All");
    assert_eq!(state.swamp.issues().len(), 2);
    for cancel in [BareKey::Esc, BareKey::Tab] {
        key(&mut state, BareKey::Char('u'));
        type_text(&mut state, "amy");
        key(&mut state, cancel);
        assert_eq!(state.swamp.assignee, None);
    }
    assert_eq!(state.swamp.requests.len(), 1);
}

#[test]
fn pagination_finds_assignees_beyond_page_one_and_deduplicates_at_returned_offsets() {
    let mut state = ready();
    state.activate_swamp();
    assert_eq!(state.swamp.note().as_deref(), Some("Loading issues…"));
    reply(&mut state, &page(&(1..=200).collect::<Vec<_>>(), 203));
    assert!(
        state.swamp.issues().is_empty(),
        "partial catalogues are never displayed"
    );
    // Counted, never against the Lab's total, which here promises one issue too many.
    assert_eq!(state.swamp.note().as_deref(), Some("Loading… 200 so far"));
    assert_eq!(
        state.swamp.requests.last().unwrap().0.url,
        format!("{}&offset=200", search_url(""))
    );
    let mut second: serde_json::Value = serde_json::from_str(&page(&[200, 201], 203)).unwrap();
    second["issues"][1]["assignees"] = json!([{"userId":"a", "username":"amy"}]);
    reply(&mut state, &second.to_string());
    assert!(state
        .swamp
        .requests
        .last()
        .unwrap()
        .0
        .url
        .ends_with("offset=202"));
    // 200 came back twice; the count is of distinct issues.
    assert_eq!(state.swamp.note().as_deref(), Some("Loading… 201 so far"));
    reply(&mut state, &page(&[202], 203));
    assert!(
        state.swamp.pending.is_none(),
        "reported total terminates the load"
    );
    assert!(state.swamp.note().is_none());
    assert_eq!(state.swamp.issues().len(), 202);
    choose(&mut state, 'u', "amy");
    assert_eq!(state.swamp.selected_number(), Some(201));
    assert_eq!(state.swamp.issues().len(), 1);
    assert_eq!(state.swamp.requests.len(), 3);
}

#[test]
fn empty_pages_finish_and_repeated_pages_or_mid_load_errors_preserve_the_cache() {
    let mut state = loaded();
    let before = serde_json::to_value(&state.swamp.cache).unwrap();
    for failure in ["repeated", "network", "malformed"] {
        key(&mut state, BareKey::Char('r'));
        reply(&mut state, &page(&[1, 2], 50));
        assert_eq!(serde_json::to_value(&state.swamp.cache).unwrap(), before);
        match failure {
            "repeated" => reply(&mut state, &page(&[2, 1], 50)),
            "network" => reply_status(&mut state, 503, r#"{"error":"unavailable"}"#),
            _ => reply(&mut state, "invalid"),
        }
        assert!(state.swamp.pending.is_none());
        assert!(state.swamp.loading.is_none());
        assert!(state.swamp.has_error());
        assert_eq!(serde_json::to_value(&state.swamp.cache).unwrap(), before);
        assert_eq!(state.swamp.issues().len(), 2);
    }
    key(&mut state, BareKey::Char('r'));
    reply(&mut state, &page(&[1, 2], 50));
    reply(&mut state, &page(&[], 50));
    assert!(state.swamp.pending.is_none());
    assert!(!state.swamp.has_error());
    assert_eq!(state.swamp.selected_number(), Some(2));
}

#[test]
fn body_search_pages_are_cached_locally_and_reload_refreshes_both_snapshots() {
    let mut state = loaded();
    choose(&mut state, '/', "body-only phrase");
    assert!(state
        .swamp
        .requests
        .last()
        .unwrap()
        .0
        .url
        .contains("q=body-only+phrase"));
    reply(&mut state, &page(&[9], 2));
    assert!(state
        .swamp
        .requests
        .last()
        .unwrap()
        .0
        .url
        .ends_with("offset=1"));
    reply(&mut state, &page(&[10], 2));
    assert_eq!(state.swamp.issues().len(), 2);
    assert!(state
        .swamp
        .issues()
        .iter()
        .all(|issue| !issue.title.contains("body-only phrase")));
    let count = state.swamp.requests.len();
    choose(&mut state, 's', "triaged");
    choose(&mut state, 't', "bug");
    choose(&mut state, 'u', "Unassigned");
    state.swamp.query.clear();
    state.search_swamp();
    assert_eq!(state.swamp.selected_number(), Some(2254));
    choose(&mut state, '/', "body-only phrase");
    assert_eq!(state.swamp.requests.len(), count);
    assert_eq!(state.swamp.issues().len(), 2);
    key(&mut state, BareKey::Enter);
    reply(&mut state, &DETAIL.replace("2180", "10"));
    key(&mut state, BareKey::Char('r'));
    assert!(state
        .swamp
        .requests
        .last()
        .unwrap()
        .0
        .url
        .ends_with("/issues/10"));
    reply(&mut state, &DETAIL.replace("2180", "10"));
    assert_eq!(state.swamp.requests.last().unwrap().0.url, search_url(""));
    reply(&mut state, SEARCH);
    assert_eq!(
        state.swamp.requests.last().unwrap().0.url,
        search_url("body-only phrase")
    );
    reply(&mut state, &page(&[10], 1));
    assert_eq!(
        state.swamp.cache.catalogue.as_ref().unwrap().issues.len(),
        2
    );
    assert_eq!(state.swamp.issues().len(), 1);
    assert!(state.swamp.pending.is_none());
}

#[test]
fn superseded_pages_including_unauthorized_replies_cannot_replace_a_new_search() {
    let mut state = loaded();
    choose(&mut state, '/', "old");
    reply(&mut state, &page(&[1], 3));
    let stale = context(&state);
    state.swamp.query = "new".into();
    state.search_swamp();
    reply_status(&mut state, 401, r#"{"error":"Unauthorized"}"#);
    assert!(
        state.swamp.commands.is_empty(),
        "stale 401 does not reread credentials"
    );
    assert_eq!(
        state.swamp.requests.last().unwrap().0.url,
        search_url("new")
    );
    assert!(!state.swamp_web_result(200, &server(), page(&[2], 2).as_bytes(), &stale));
    reply(&mut state, &page(&[7], 1));
    assert_eq!(state.swamp.selected_number(), Some(7));
    assert_eq!(state.swamp.cache.text_search.as_ref().unwrap().0, "new");
    key(&mut state, BareKey::Char('r'));
    let old_reload = context(&state);
    let count = state.swamp.requests.len();
    key(&mut state, BareKey::Char('r'));
    assert_eq!(state.swamp.requests.len(), count, "reloads never overlap");
    reply_status(&mut state, 503, r#"{"error":"stale failure"}"#);
    assert!(!state.swamp.has_error());
    assert_eq!(state.swamp.selected_number(), Some(7));
    assert!(!state.swamp_web_result(200, &server(), EMPTY.as_bytes(), &old_reload));
    reply(&mut state, SEARCH);
    reply(&mut state, &page(&[8], 1));
    assert_eq!(state.swamp.selected_number(), Some(8));
}

#[test]
fn persistent_catalogue_and_search_are_reused_after_resolving_credentials_on_restart() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = ready();
    state.swamp.cache_dir = Some(directory.path().to_owned());
    state.swamp.keep_credentials = false;
    state.activate_swamp();
    answer_login(&mut state, &login("swamp_secret"));
    let mut assigned: serde_json::Value = serde_json::from_str(SEARCH).unwrap();
    assigned["issues"][1]["assignees"] = json!([{"userId":"k", "username":"keeper"}]);
    reply(&mut state, &assigned.to_string());
    choose(&mut state, '/', "body match");
    reply(&mut state, &page(&[99], 1));
    let saved = std::fs::read_to_string(state.swamp.cache_path().unwrap()).unwrap();
    assert!(!saved.contains("swamp_secret"));
    let mut restarted = ready();
    restarted.swamp.cache_dir = Some(directory.path().to_owned());
    restarted.swamp.keep_credentials = false;
    restarted.activate_swamp();
    assert!(
        restarted.swamp.issues().is_empty(),
        "identity must be resolved first"
    );
    answer_login(&mut restarted, &login("swamp_secret"));
    assert_eq!(restarted.swamp.issues().len(), 2);
    choose(&mut restarted, 'u', "keeper");
    assert_eq!(restarted.swamp.issues().len(), 1);
    assert_eq!(restarted.swamp.issues()[0].source, "swamp-club");
    choose(&mut restarted, 'u', "All");
    choose(&mut restarted, '/', "body match");
    assert_eq!(restarted.swamp.selected_number(), Some(99));
    assert!(restarted.swamp.requests.is_empty());
    restarted.update(pane_update(restarted.own_plugin_id, true));
    restarted.update(pane_update(restarted.own_plugin_id, false));
    answer_login(&mut restarted, &login("swamp_secret"));
    assert!(restarted.swamp.requests.is_empty());
    key(&mut restarted, BareKey::Char('r'));
    answer_login(&mut restarted, &login("swamp_secret"));
    reply_status(&mut restarted, 503, r#"{"error":"offline"}"#);
    reply_status(&mut restarted, 503, r#"{"error":"offline"}"#);
    assert_eq!(restarted.swamp.selected_number(), Some(99));
    assert_eq!(
        std::fs::read_to_string(restarted.swamp.cache_path().unwrap()).unwrap(),
        saved
    );
    restarted.activate_swamp();
    answer_login(&mut restarted, &login("different_key"));
    assert!(
        restarted.swamp.search_wanted,
        "the new identity needs this query too"
    );
    restarted.activate_swamp();
    reply(&mut restarted, EMPTY);
    let count = restarted.swamp.requests.len();
    answer_login(&mut restarted, &login("swamp_secret"));
    assert_eq!(restarted.swamp.selected_number(), Some(99));
    assert_eq!(
        restarted.swamp.requests.len(),
        count,
        "old identity's queued search cannot refresh a valid cache"
    );
}

#[test]
fn identity_changes_clear_data_and_isolate_keys_servers_and_anonymous_access() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = ready();
    state.swamp.cache_dir = Some(directory.path().to_owned());
    state.swamp.keep_credentials = false;
    for file in [
        login("same-prefix-key-one"),
        login("same-prefix-key-two"),
        String::from_utf8(login("same-prefix-key-two"))
            .unwrap()
            .replace("lab.example.test", "other.example.test")
            .into_bytes(),
        vec![],
    ] {
        state.activate_swamp();
        answer_login(&mut state, &file);
        assert!(state.swamp.issues().is_empty());
        assert!(state.swamp.detail.is_none());
        assert!(state.swamp.opened.is_none());
        reply(&mut state, SEARCH);
        key(&mut state, BareKey::Down);
        key(&mut state, BareKey::Enter);
        reply(&mut state, DETAIL);
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 4);
    state.activate_swamp();
    let count = state.swamp.requests.len();
    answer_login(&mut state, &login("same-prefix-key-one"));
    assert_eq!(
        state.swamp.requests.len(),
        count,
        "returning identity reuses its own cache"
    );
    assert_eq!(state.swamp.issues().len(), 2);
}

#[test]
fn activation_during_a_page_load_resolves_identity_before_accepting_the_reply() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('r'));
    reply(&mut state, &page(&[1], 3));
    state.swamp.keep_credentials = false;
    state.update(pane_update(state.own_plugin_id, true));
    state.update(pane_update(state.own_plugin_id, false));
    reply(&mut state, &page(&[2, 3], 3));
    assert_eq!(state.swamp.selected_number(), Some(2254));
    answer_login(&mut state, &login("new_identity"));
    assert!(state.swamp.issues().is_empty());
    assert_eq!(state.swamp.loading.as_ref().unwrap().offset, 0);
    reply(&mut state, EMPTY);
    assert!(state.swamp.issues().is_empty());
}

#[test]
fn corrupt_or_incompatible_caches_load_from_api_and_write_errors_keep_fresh_results() {
    let directory = tempfile::tempdir().unwrap();
    let identity = cache::identity(&Credentials::anonymous());
    let path = directory.path().join(format!("{identity}.json"));
    for invalid in [
        "not json",
        r#"{"version":999,"catalogue":null,"text_search":null}"#,
    ] {
        std::fs::write(&path, invalid).unwrap();
        let mut state = ready();
        state.swamp.cache_dir = Some(directory.path().to_owned());
        state.activate_swamp();
        assert_eq!(state.swamp.requests.len(), 1);
        reply(&mut state, SEARCH);
        assert!(cache::Cache::read(&path).unwrap().catalogue.is_some());
    }
    let mut state = ready();
    state.swamp.cache_dir = Some(path.clone()); // parent is a file, so saving must fail
    state.activate_swamp();
    reply(&mut state, SEARCH);
    assert_eq!(state.swamp.issues().len(), 2);
    assert!(state.swamp.note().unwrap().contains("Cache not saved"));
    assert!(cache::Cache::read(&path).unwrap().catalogue.is_some());
}

#[test]
fn credentials_are_read_at_the_start_of_every_refresh() {
    let mut state = ready();
    state.swamp.keep_credentials = false;
    state.activate_swamp();
    assert!(state.swamp.requests.is_empty());
    let (argv, context) = state.swamp.commands.last().unwrap().clone();
    assert_eq!(argv, swamp_lab::auth::auth_json_argv());
    // A reply that is not the pending read is ignored, a web reply included.
    assert!(!state.swamp_web_result(200, &server(), SEARCH.as_bytes(), &context));
    answer_login(&mut state, &login("swamp_k"));
    let (request, _) = state.swamp.requests.last().unwrap();
    assert_eq!(
        request.url,
        "https://lab.example.test/api/v1/lab/issues?limit=200"
    );
    assert_eq!(sent_key(&state).as_deref(), Some("swamp_k"));
    reply(&mut state, SEARCH);
    assert_eq!(
        state.swamp.issues()[0].url,
        "https://lab.example.test/lab/2254"
    );
    // Opening an issue within the cycle reuses the login.
    state.render(9, 80);
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.requests.len(), 2);
    assert_eq!(state.swamp.commands.len(), 1);
    assert_eq!(sent_key(&state).as_deref(), Some("swamp_k"));
    reply(&mut state, DETAIL);
    // The next refresh reads it again, and so picks up a key switched since.
    key(&mut state, BareKey::Char('r'));
    assert_eq!(state.swamp.commands.len(), 2);
    answer_login(&mut state, &login("swamp_k2"));
    assert_eq!(sent_key(&state).as_deref(), Some("swamp_k2"));
}

#[test]
fn a_login_after_an_anonymous_start_is_picked_up_on_refresh() {
    let mut state = ready();
    state.swamp.keep_credentials = false;
    state.activate_swamp();
    answer_login(&mut state, b"");
    assert_eq!(sent_key(&state), None);
    reply(&mut state, SEARCH);
    key(&mut state, BareKey::Char('r'));
    answer_login(&mut state, &login("swamp_k"));
    assert_eq!(sent_key(&state).as_deref(), Some("swamp_k"));
}

#[test]
fn no_login_is_anonymous_and_an_unreadable_login_waits_for_a_retry() {
    let mut state = ready();
    state.swamp.keep_credentials = false;
    state.activate_swamp();
    answer_login(&mut state, b"");
    assert_eq!(sent_key(&state), None);

    let mut state = ready();
    state.swamp.keep_credentials = false;
    state.activate_swamp();
    let context = state.swamp.commands.last().unwrap().1.clone();
    state.swamp_result(Some(1), b"", b"Permission denied", &context);
    assert!(state.swamp.requests.is_empty());
    assert!(state.swamp.has_error());
    assert!(state.swamp.note().unwrap().contains("Permission denied"));
    state.update(Event::Timer(FractionalSeconds::new(0.0)));
    assert_eq!(state.swamp.commands.len(), 1, "no retry loop");
    key(&mut state, BareKey::Char('r'));
    assert_eq!(state.swamp.commands.len(), 2);
    answer_login(&mut state, b"{not json");
    assert!(state.swamp.note().unwrap().contains("auth.json"));
}

#[test]
fn a_rejected_key_is_reread_once_per_refresh_then_reported() {
    let mut state = loaded();
    state.swamp.keep_credentials = false;
    key(&mut state, BareKey::Char('r'));
    answer_login(&mut state, &login("swamp_old"));
    reply_status(&mut state, 401, r#"{"error":"Unauthorized"}"#);
    assert!(state.swamp.credentials.is_none());
    answer_login(&mut state, &login("swamp_old"));
    assert_eq!(state.swamp.requests.len(), 3, "the search went again");
    reply_status(&mut state, 401, r#"{"error":"Unauthorized"}"#);
    assert_eq!(state.swamp.commands.len(), 2, "only one reread");
    assert!(state.swamp.note().unwrap().contains("swamp auth login"));
    assert!(
        state.swamp.issues().is_empty(),
        "the old identity is cleared"
    );
    // After `swamp auth login` again, the next refresh reads the new key and succeeds.
    key(&mut state, BareKey::Char('r'));
    assert_eq!(state.swamp.commands.len(), 3);
    answer_login(&mut state, &login("swamp_new"));
    assert_eq!(sent_key(&state).as_deref(), Some("swamp_new"));
    reply(&mut state, SEARCH);
    assert!(!state.swamp.has_error());
    assert!(!state.swamp.reloaded_after_401);
}

/// A finished triage says so only while nothing more urgent has the row, and goes when
/// the user moves on: it used to outrank loading and every error until the next `r`.
#[test]
fn a_finished_triage_gives_way_to_loading_errors_and_what_was_typed() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('j'));
    triage(&mut state);
    reply(&mut state, DETAIL);
    assert!(ack(&mut state, wire::AskOutcome::Sent));
    let note = state.swamp.note().unwrap();
    assert!(note.starts_with("#2180 sent to Hocus") && note.contains("chat"));
    // The chat's input box is two Tabs away: its tab bar first, then the box.
    assert!(note.ends_with("Tab Tab"), "{note}");
    assert_eq!(state.swamp.noted().unwrap().1, Tone::Quiet);
    assert_eq!(state.swamp.note_rows(11, 24).len(), 2);
    key(&mut state, BareKey::Tab);
    assert_eq!(state.ring_yields, [wire::STOP_TABS]);
    key(&mut state, BareKey::Char('k'));
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.note().as_deref(), Some("Loading #2254…"));
    reply_status(&mut state, 503, r#"{"error":"unavailable"}"#);
    assert!(state.swamp.note().unwrap().starts_with("Error:"));
    assert_eq!(state.swamp.noted().unwrap().1, Tone::Error);
    key(&mut state, BareKey::Esc);

    triage(&mut state);
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    assert!(ack(&mut state, wire::AskOutcome::Queued));
    assert!(state.swamp.note().unwrap().starts_with("#2254 queued"));
    key(&mut state, BareKey::Char('f'));
    type_text(&mut state, "bad tag");
    key(&mut state, BareKey::Enter);
    assert!(state.swamp.note().unwrap().contains("must be 1-64"));
    key(&mut state, BareKey::Esc);
    choose(&mut state, 's', "triaged");
    assert!(
        state.swamp.note().is_none(),
        "a filter commit is moving on too"
    );
}

#[test]
fn a_refused_or_unanswered_triage_is_an_error_and_can_be_asked_again() {
    let mut state = loaded();
    triage(&mut state);
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    assert!(ack(&mut state, wire::AskOutcome::Refused));
    assert!(state
        .swamp
        .note()
        .unwrap()
        .starts_with("Hocus refused #2254"));
    assert_eq!(state.swamp.noted().unwrap().1, Tone::Error);
    assert!(state.swamp.has_error());

    // Sent, and never acknowledged: after the deadline the note stops saying it is on its
    // way, and says so without claiming it failed.
    triage(&mut state);
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    assert_eq!(
        state.swamp.note().as_deref(),
        Some("Sending #2254 to Hocus…")
    );
    let deadline = state.swamp.triage_deadline_ms;
    assert!(!state.expire_swamp_triage(deadline - 1));
    assert!(state.expire_swamp_triage(deadline));
    assert!(!state.expire_swamp_triage(deadline + 1), "given up on once");
    let note = state.swamp.note().unwrap();
    assert!(note.contains("did not answer about #2254") && note.ends_with("a retry"));
    assert_eq!(state.swamp.noted().unwrap().1, Tone::Error);
    assert!(
        !ack(&mut state, wire::AskOutcome::Sent),
        "too late to count"
    );
    let count = state.swamp.requests.len();
    triage(&mut state);
    assert_eq!(state.swamp.requests.len(), count + 1, "a asks again");

    // `r` is what someone does about a "Sending…" that has sat there.
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    assert!(state.swamp.triage_waiting);
    key(&mut state, BareKey::Char('r'));
    assert!(!state.swamp.triage_waiting);
    reply(&mut state, SEARCH);
    assert!(state.swamp.note().unwrap().contains("did not answer"));
    // Off the Swamp tab the expiry still happens, and redraws for the tab's `!`: the bar
    // is on show whichever tab is.
    triage(&mut state);
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    state.update(pane_update(state.own_plugin_id, true));
    assert!(!state.swamp.has_error());
    assert!(state.expire_swamp_triage(i64::MAX));
    assert!(!state.swamp.triage_waiting);
    assert!(state.swamp.has_error());
    state.render(11, 60);
    assert!(state.swamp.has_error());
}

/// The other way a triage fails while the user is elsewhere: the fetch before the handoff.
#[test]
fn off_tab_triage_fetch_failure_redraws_for_the_tab_marker() {
    let mut state = loaded();
    triage(&mut state);
    state.update(pane_update(state.own_plugin_id, true));
    let context = context(&state);
    assert!(state.swamp_web_result(500, &server(), br#"{"error":"boom"}"#, &context));
    assert!(state.swamp.has_error());
    assert!(!state.swamp.triage_waiting);
}

#[test]
fn errors_wrap_so_the_remedy_and_the_retry_key_are_read() {
    let mut state = loaded();
    state.swamp.keep_credentials = false;
    key(&mut state, BareKey::Char('r'));
    answer_login(&mut state, &login("swamp_old"));
    reply_status(&mut state, 401, r#"{"error":"Unauthorized"}"#);
    answer_login(&mut state, &login("swamp_old"));
    reply_status(&mut state, 401, r#"{"error":"Unauthorized"}"#);
    state.render(11, 46);
    let rows = state.swamp.note_rows(11, 46);
    assert!(rows.len() > 1, "{rows:?}");
    assert!(rows.join(" ").contains("swamp auth login"), "{rows:?}");
    // The action is never split: it rides the last row, or takes one of its own.
    assert_eq!(rows.last().unwrap(), "r retry", "{rows:?}");
    assert!(rows.iter().all(|row| row.width() <= 46));
    // The chips follow the note down, and still open their filters.
    let (row, x, _, shortcut) = state.swamp.filter_cells[0];
    assert_eq!(row, 1 + rows.len());
    let last_chip_row = state.swamp.filter_cells.last().unwrap().0;
    assert_eq!(state.swamp.last_body_top, last_chip_row + 1);
    assert!(state.on_mouse(Mouse::LeftClick((row as isize + 1) - 1, x)));
    assert_eq!(state.swamp.filter.as_ref().unwrap().shortcut, shortcut);
    key(&mut state, BareKey::Esc);
    // The default panel's 24 columns: an error may take half the pane, which is room for
    // the remedy as well as what went wrong.
    state.render(11, 24);
    let rows = state.swamp.note_rows(11, 24);
    assert!(rows.join(" ").contains("`swamp auth login`"), "{rows:?}");
    assert!(rows.last().unwrap().ends_with("r retry"), "{rows:?}");
    assert!(rows.iter().all(|row| row.width() <= 24), "{rows:?}");
    assert!(state.swamp.last_body_rows >= 1);
    // Too short for all of it: cut, marked, the key kept, and a row left for the body.
    state.render(4, 24);
    let rows = state.swamp.note_rows(4, 24);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[1].ends_with("… · r retry"), "{rows:?}");
    assert!(rows.iter().all(|row| row.width() <= 24));
    assert_eq!(state.swamp.last_body_top, 3);
    assert_eq!(state.swamp.last_body_rows, 1);
}

/// A validation message is about the box still open: no `r retry`, which in the box types
/// an `r`, and gone with the box.
#[test]
fn what_was_typed_is_not_a_network_error() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('f'));
    type_text(&mut state, "cli tag");
    key(&mut state, BareKey::Enter);
    let note = state.swamp.note().unwrap();
    assert!(note.contains("must be 1-64"), "{note}");
    assert!(
        note.ends_with("Esc cancel") && !note.contains("r retry"),
        "{note}"
    );
    state.render(11, 80);
    assert!(state.swamp.has_error());
    key(&mut state, BareKey::Esc);
    assert!(!state.swamp.has_error());
    assert!(state.swamp.note().is_none());
    state.render(11, 80);
    assert!(!state.swamp.has_error());

    key(&mut state, BareKey::Char('/'));
    type_text(&mut state, &"q".repeat(201));
    key(&mut state, BareKey::Enter);
    assert!(state.swamp.is_editing());
    let note = state.swamp.note().unwrap();
    assert!(note.contains("longer than 200") && !note.contains("r retry"));
    key(&mut state, BareKey::Esc);
    assert!(!state.swamp.has_error());
    assert!(state.swamp.note().is_none());

    // A real network error outlasts a good Source and a bad one.
    key(&mut state, BareKey::Char('r'));
    reply_status(&mut state, 503, r#"{"error":"Network unavailable"}"#);
    choose(&mut state, 'f', "swamp");
    assert_eq!(state.swamp.source, "swamp");
    assert!(state.swamp.note().unwrap().contains("Network unavailable"));
    choose(&mut state, 'f', " bad");
    key(&mut state, BareKey::Esc);
    assert!(state.swamp.note().unwrap().contains("Network unavailable"));
}

#[test]
fn opening_an_issue_mid_refresh_fetches_it_before_the_next_page() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('r'));
    reply(&mut state, &page(&[1], 3));
    assert!(state
        .swamp
        .requests
        .last()
        .unwrap()
        .0
        .url
        .ends_with("offset=1"));
    let count = state.swamp.requests.len();
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.opened, Some(2254));
    assert_eq!(state.swamp.requests.len(), count, "one request at a time");
    assert_eq!(state.swamp.note().as_deref(), Some("Loading #2254…"));
    reply(&mut state, &page(&[2], 3));
    assert!(state
        .swamp
        .requests
        .last()
        .unwrap()
        .0
        .url
        .ends_with("/issues/2254"));
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    assert_eq!(state.swamp.detail.as_ref().unwrap().issue.number, 2254);
    assert!(state
        .swamp
        .requests
        .last()
        .unwrap()
        .0
        .url
        .ends_with("offset=2"));
    assert_eq!(state.swamp.note().as_deref(), Some("Loading… 2 so far"));
    reply(&mut state, &page(&[3], 3));
    assert_eq!(
        state.swamp.cache.catalogue.as_ref().unwrap().issues.len(),
        3
    );
    assert!(state.swamp.pending.is_none());
}

#[test]
fn c_clears_query_and_filters_and_reuses_the_catalogue() {
    let mut state = loaded();
    assert!(!state.swamp.header(usize::MAX).contains("c clear"));
    choose(&mut state, 's', "triaged");
    choose(&mut state, 't', "bug");
    choose(&mut state, 'o', "Created oldest first");
    choose(&mut state, '/', "two words");
    reply(&mut state, SEARCH);
    assert!(state
        .swamp
        .header(usize::MAX)
        .starts_with("/two words · Esc/c clear · Enter open · / edit"));
    // The clear key outlasts the others on a narrow row, whatever the query's length, and
    // gives way to `? keys` no more than the query does.
    state.swamp.query = "q".repeat(100);
    assert!(state.swamp.header(30).contains("Esc/c clear"));
    state.swamp.query = "two words".into();
    let count = state.swamp.requests.len();
    key(&mut state, BareKey::Char('c'));
    assert!(state.swamp.query.is_empty());
    assert_eq!((state.swamp.status, state.swamp.kind), (0, 0));
    assert!(state.swamp.source.is_empty() && state.swamp.assignee.is_none());
    assert_eq!(state.swamp.sort, 3, "sort is not a filter");
    assert_eq!(state.swamp.issues().len(), 2);
    assert_eq!(state.swamp.requests.len(), count, "the catalogue is reused");
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('c'))));
    assert!(!state.swamp.header(usize::MAX).contains("c clear"));
    assert_eq!(state.swamp.empty_list(), "No issues found");
    choose(&mut state, 's', "closed");
    assert!(state.swamp.issues().is_empty());
    assert_eq!(state.swamp.empty_list(), "No issues match · Esc/c clear");
    assert!(state.swamp.header(usize::MAX).contains("0 of 2 issues"));
    // In the box it is a letter.
    key(&mut state, BareKey::Char('/'));
    key(&mut state, BareKey::Char('c'));
    assert_eq!(
        state.swamp.editing.as_ref().map(TextInput::get_text),
        Some("c")
    );
    key(&mut state, BareKey::Esc);
    // In the reader it does nothing.
    let mut state = reader();
    state.swamp.status = 2;
    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('c'))));
    assert_eq!(state.swamp.status, 2);
}

#[test]
fn every_chip_keeps_its_key_and_narrow_chips_leave_the_list_room() {
    let mut state = loaded();
    state.swamp.source = "a-very-long-source-tag-name".into();
    for cols in 8..=60 {
        let (_, labels) = state.swamp.layout_filters(12, cols);
        assert_eq!(labels.len(), 5, "{cols}");
        for ((_, x, width, key), label) in state.swamp.filter_cells.iter().zip(&labels) {
            assert!(x + width <= cols, "{cols}: {label}");
            assert!(
                label.contains(&format!("({key})]")) || label.starts_with(&format!("[{key} ")),
                "{cols}: {label}"
            );
        }
    }
    state.swamp.source.clear();
    // The default panel on an 80-column terminal: two rows of chips, not five.
    state.render(12, 24);
    assert_eq!(state.swamp.filter_cells.len(), 5);
    assert_eq!(state.swamp.last_body_top, 3);
    assert!(state.swamp.last_body_rows >= 3);
    // Wide enough, the chips say what they filter.
    let (_, labels) = state.swamp.layout_filters(12, 120);
    assert_eq!(labels[3], "[Sort: Updated ↓ (o)]");
    // Too short for the chips and a list: the list keeps a row.
    state.render(3, 80);
    assert!(state.swamp.last_body_rows >= 1);
}

#[test]
fn swamp_asks_the_host_only_when_asked_or_granted_before() {
    let directory = tempfile::tempdir().unwrap();
    let unrequested = || {
        let mut state = ready();
        state.swamp.cache_dir = Some(directory.path().to_owned());
        state.swamp.permission = Permission::Unrequested;
        state
    };
    let mut state = unrequested();
    state.update(pane_update(state.own_plugin_id, false));
    state.take_ring_stop(Some(hocus_proto::vms::STOP_TABS));
    assert!(state.tabs_focused);
    key(&mut state, BareKey::Enter);
    assert!(!state.tabs_focused);
    assert!(
        state.swamp.permission == Permission::Unrequested,
        "walking the tab bar past Swamp asks nothing"
    );
    assert_eq!(state.swamp.noted().unwrap().1, Tone::Ask);
    // A key the bar does not own goes to the tab below it.
    key(&mut state, BareKey::Char('r'));
    assert!(state.swamp.permission == Permission::Waiting);
    state.update(Event::PermissionRequestResult(PermissionStatus::Granted));
    assert_eq!(state.swamp.requests.len(), 1);

    // The next load remembers the grant and asks straight away; the host answers from its
    // own cache, without a prompt.
    let mut next = unrequested();
    next.activate_swamp();
    assert!(next.swamp.permission == Permission::Waiting);
    // A denial forgets it.
    next.update(Event::PermissionRequestResult(PermissionStatus::Denied));
    let mut third = unrequested();
    third.activate_swamp();
    assert!(third.swamp.permission == Permission::Unrequested);
}

#[test]
fn ctrl_r_refreshes_as_plain_r_does() {
    let ctrl_r = KeyWithModifier::new(BareKey::Char('r')).with_ctrl_modifier();
    let mut state = loaded();
    let count = state.swamp.requests.len();
    assert!(state.on_key(ctrl_r.clone()));
    assert_eq!(state.swamp.requests.len(), count + 1);
    assert_eq!(state.swamp.requests.last().unwrap().0.url, search_url(""));
    assert!(state.swamp.header(usize::MAX).contains("Ctrl-r refresh"));
    let mut state = ready();
    state.swamp.permission = Permission::Denied;
    assert!(state.on_key(ctrl_r));
    assert!(state.swamp.permission == Permission::Waiting);
}

#[test]
fn a_click_selects_and_only_a_release_on_the_same_cell_opens() {
    let mut state = loaded();
    state.render(11, 80);
    let count = state.swamp.requests.len();
    let second = (state.swamp.last_body_top + 3) as isize; // 2180's first row
    assert!(state.swamp_mouse(Mouse::LeftClick(second, 4)));
    assert_eq!(state.swamp.selected_number(), Some(2180));
    assert!(state.swamp.opened.is_none());
    assert_eq!(state.swamp.requests.len(), count);
    // A press that became a drag was selecting text.
    state.swamp_mouse(Mouse::Hold(second, 10));
    assert!(!state.swamp_mouse(Mouse::Release(second, 10)));
    assert!(state.swamp.opened.is_none());
    // A release with no press of its own opens nothing either.
    assert!(!state.swamp_mouse(Mouse::Release(second, 4)));
    assert_eq!(state.swamp.requests.len(), count);
    state.swamp_mouse(Mouse::LeftClick(second, 4));
    assert!(state.swamp_mouse(Mouse::Release(second, 4)));
    assert_eq!(state.swamp.opened, Some(2180));
    assert_eq!(state.swamp.requests.len(), count + 1);
    reply(&mut state, DETAIL);
    state.render(11, 80);
    // In the reader the header row is the one target: the way back, on a release on the
    // cell pressed. A drag across it is copying the `#N`.
    assert!(!state.swamp_mouse(Mouse::LeftClick(3, 0)));
    assert_eq!(state.swamp.opened, Some(2180));
    assert!(!state.swamp_mouse(Mouse::LeftClick(0, 1)));
    state.swamp_mouse(Mouse::Hold(0, 5));
    assert!(!state.swamp_mouse(Mouse::Release(0, 5)));
    assert_eq!(state.swamp.opened, Some(2180));
    assert!(!state.swamp_mouse(Mouse::LeftClick(0, 5)));
    assert_eq!(state.swamp.opened, Some(2180), "not on the press");
    assert!(state.swamp_mouse(Mouse::Release(0, 5)));
    assert!(state.swamp.opened.is_none());
    assert_eq!(state.panel_hides, 0);
    assert_eq!(state.swamp.selected_number(), Some(2180));
}

/// hocus carrying the panel to another Yinz tab is a `Visible(false)` and a
/// `Visible(true)` with the panel on screen the whole time.
#[test]
fn a_tab_switch_that_carries_the_panel_keeps_the_open_filter_and_the_login() {
    let mut state = ready();
    state.swamp.keep_credentials = false;
    state.activate_swamp();
    answer_login(&mut state, b"");
    reply(&mut state, SEARCH);
    let commands = state.swamp.commands.len();
    key(&mut state, BareKey::Char('f'));
    type_text(&mut state, "provisional");
    for visible in [false, true, false, true] {
        state.update(Event::Visible(visible));
    }
    assert!(state.swamp.has_filter_editor());
    assert_eq!(
        state.swamp.filter.as_ref().unwrap().form.text("value"),
        "provisional"
    );
    assert_eq!(state.swamp.commands.len(), commands, "no second login read");
    assert!(state.swamp.note().is_none(), "no Loading…");
    // Work that waited while the tab was hidden goes out when it is back.
    key(&mut state, BareKey::Esc);
    state.update(Event::Visible(false));
    key(&mut state, BareKey::Enter);
    let requests = state.swamp.requests.len();
    state.update(Event::Visible(true));
    assert_eq!(state.swamp.requests.len(), requests + 1);
    reply(&mut state, DETAIL.replace("2180", "2254").as_str());
    // Putting the panel away is another matter: the filter closes, the login is read again.
    key(&mut state, BareKey::Esc);
    key(&mut state, BareKey::Char('f'));
    state.update(pane_update(state.own_plugin_id, true));
    assert!(!state.swamp.is_editing());
    state.update(pane_update(state.own_plugin_id, false));
    assert_eq!(state.swamp.commands.len(), commands + 1);
}

#[test]
fn triage_asks_first_and_any_other_key_keeps_the_issue_here() {
    let mut state = loaded();
    let count = state.swamp.requests.len();
    key(&mut state, BareKey::Char('a'));
    assert_eq!(state.swamp.note().as_deref(), Some("Triage #2254? y/n"));
    assert_eq!(state.swamp.noted().unwrap().1, Tone::Ask);
    assert_eq!(state.swamp.requests.len(), count);
    // Any other key drops the question and does its job.
    key(&mut state, BareKey::Char('j'));
    assert!(state.swamp.confirming_triage.is_none());
    assert_eq!(state.swamp.selected_number(), Some(2180));
    key(&mut state, BareKey::Char('y'));
    assert!(!state.swamp.triage_waiting);
    // Esc and n drop it without hiding the panel.
    for answer in [BareKey::Esc, BareKey::Char('n')] {
        key(&mut state, BareKey::Char('a'));
        assert!(state.on_key(KeyWithModifier::new(answer)));
        assert!(state.swamp.confirming_triage.is_none());
        assert_eq!(state.panel_hides, 0);
    }
    // A selection moved under the question turns its `y` into nothing.
    key(&mut state, BareKey::Char('a'));
    state.swamp.selected = 0;
    key(&mut state, BareKey::Char('y'));
    assert!(!state.swamp.triage_waiting);
    // So does a click on another issue, and a hide.
    key(&mut state, BareKey::Char('a'));
    state.render(11, 80);
    let top = state.swamp.last_body_top as isize;
    state.swamp_mouse(Mouse::LeftClick(top + 3, 0));
    assert!(state.swamp.confirming_triage.is_none());
    key(&mut state, BareKey::Char('a'));
    state.hide_the_panel();
    assert!(state.swamp.confirming_triage.is_none());
    assert_eq!(state.swamp.requests.len(), count);
    state.update(pane_update(state.own_plugin_id, false));
    triage(&mut state);
    assert!(state.swamp.triage_waiting);
    assert_eq!(state.swamp.requests.len(), count + 1);
}

#[test]
fn a_paste_lands_in_the_query_and_the_source_box_but_not_a_select() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('/'));
    type_text(&mut state, "ab");
    key(&mut state, BareKey::Left);
    assert!(state.update(Event::PastedText("x\r\ny".into())));
    assert_eq!(
        state.swamp.editing.as_ref().map(TextInput::get_text),
        Some("ax yb")
    );
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.query, "ax yb");
    reply(&mut state, EMPTY);
    key(&mut state, BareKey::Char('f'));
    assert!(state.update(Event::PastedText("swamp-club".into())));
    assert_eq!(
        state.swamp.filter.as_ref().unwrap().form.text("value"),
        "swamp-club"
    );
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.source, "swamp-club");
    key(&mut state, BareKey::Char('s'));
    assert!(!state.update(Event::PastedText("closed".into())));
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.status, 0);
    // Outside a box a paste is not a string of commands.
    assert!(!state.update(Event::PastedText("ar".into())));
    assert!(state.swamp.confirming_triage.is_none());
}

#[test]
fn the_query_box_edits_mid_text_and_keeps_its_cursor_in_view() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('/'));
    type_text(&mut state, "refral");
    for _ in 0..3 {
        key(&mut state, BareKey::Left);
    }
    type_text(&mut state, "er");
    assert_eq!(
        state.swamp.editing.as_ref().map(TextInput::get_text),
        Some("referral")
    );
    // The cursor sits after "refer", on the header row: content row 0.
    assert_eq!(state.swamp_query_cursor(11, 80), Some((6, 0)));
    assert_eq!(
        state.swamp.query_line(80).unwrap().0,
        "/referral · Enter apply · Esc cancel"
    );
    // Past the edge the text scrolls with the cursor and the hints give way.
    key(&mut state, BareKey::End);
    type_text(&mut state, &"x".repeat(40));
    let (line, column, _) = state.swamp.query_line(24).unwrap();
    assert!(line.width() <= 24 && line.ends_with('x'), "{line}");
    assert!(!line.contains("Esc"));
    assert_eq!(column, 23);
    assert!(state.swamp_query_cursor(0, 24).is_none());
    state.on_key(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier());
    assert_eq!(
        state.swamp.editing.as_ref().map(TextInput::get_text),
        Some("")
    );
    // Tab closes it, as it closes the Source box, and goes on round the ring.
    key(&mut state, BareKey::Tab);
    assert!(state.swamp.editing.is_none());
    assert!(state.swamp_query_cursor(11, 80).is_none());
    assert_eq!(state.ring_yields, [wire::STOP_TABS]);
    // Emptied, Enter clears the search, and says so.
    choose(&mut state, '/', "words");
    key(&mut state, BareKey::Char('/'));
    state.on_key(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier());
    assert!(state
        .swamp
        .query_line(80)
        .unwrap()
        .0
        .contains("Enter clear"));
}

/// The hints come one at a time, Enter's first, and only whole: at the default panel's 24
/// columns the pair never fitted beside an empty box, which then said nothing at all.
#[test]
fn the_query_box_names_its_keys_one_at_a_time() {
    let mut state = loaded();
    key(&mut state, BareKey::Char('/'));
    assert_eq!(state.swamp.query_line(24).unwrap().0, "/ · Enter apply");
    type_text(&mut state, "referral");
    assert_eq!(
        state.swamp.query_line(24).unwrap().0,
        "/referral · Enter apply"
    );
    let hints = ["Enter apply", "Enter clear", "Esc cancel"];
    for cols in 1..=40 {
        let (line, _, hint_start) = state.swamp.query_line(cols).unwrap();
        assert!(line.width() <= cols, "{cols}: {line}");
        let tail: String = line.chars().skip(hint_start).collect();
        for hint in tail.split(" · ").filter(|hint| !hint.is_empty()) {
            let hint = hint.strip_prefix("· ").unwrap_or(hint);
            assert!(hints.contains(&hint), "{cols}: {line}");
        }
    }
    key(&mut state, BareKey::Esc);
    choose(&mut state, '/', "words");
    key(&mut state, BareKey::Char('/'));
    state.on_key(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier());
    assert_eq!(state.swamp.query_line(24).unwrap().0, "/ · Enter clear");
}

#[test]
fn the_header_leads_with_keys_and_drops_whole_segments() {
    let state = loaded();
    let full = state.swamp.header(usize::MAX);
    assert_eq!(
        full,
        "Enter open · / search · a triage · Ctrl-r refresh · 2 issues · cached now"
    );
    for cols in "Enter open".width()..full.width() {
        let header = state.swamp.header(cols);
        assert!(header.width() <= cols, "{cols}: {header}");
        for segment in header.split(" · ") {
            assert!(
                segment == "? keys" || full.split(" · ").any(|whole| whole == segment),
                "{cols}: {header}"
            );
        }
    }
    // The default panel on an 80-column terminal still names the keys, and says there are
    // more: a key or two give way to the marker rather than the marker never fitting.
    assert_eq!(state.swamp.header(24), "Enter open · ? keys");
    assert_eq!(state.swamp.header(28), "Enter open · ? keys");
    assert_eq!(state.swamp.header(34), "Enter open · / search · ? keys");
    for cols in "Enter open · ? keys".width()..full.width() {
        assert!(state.swamp.header(cols).ends_with("? keys"), "{cols}");
    }
    let state = reader();
    assert_eq!(
        state.swamp.header(40),
        "#2180 · Esc back · a triage · ? keys"
    );
    // Where the issue's number and the way back do not both fit, the way back stays: the
    // body's first row names the issue anyway. The room is what `render_swamp` gives the
    // reader's header at 24 columns, beside each scroll readout.
    for (above, below) in [(0, 22), (3, 19), (0, 9)] {
        let readout = scroll_readout(above, below).width();
        let header = state.swamp.header(24 - (readout + 4 + FOCUS_MARK_COLS));
        assert!(header.starts_with("Esc back"), "{above}/{below}: {header}");
    }
}

/// At the 24 columns the default panel gives the list on an 80-column terminal, every tab
/// is still named on the bar, and Swamp's `!` with it: the last tab is the one a narrow bar
/// used to cut first, and the mark is what says to look there.

#[test]
fn the_key_sheet_names_every_key_each_mode_answers() {
    let keys = |state: &State| {
        state
            .swamp_key_sheet()
            .into_iter()
            .map(|(keys, _)| keys)
            .collect::<Vec<_>>()
    };
    let mut state = loaded();
    let list = keys(&state);
    for expected in [
        "Enter",
        "j/k ↑/↓",
        "PgUp/PgDn",
        "Home/End",
        "/",
        "s/t/f/u",
        "o",
        "a",
        "Ctrl-r/r",
        "Tab",
        "Shift+Tab",
        "Esc",
        "click",
        "click a chip",
        "wheel",
    ] {
        assert!(
            list.iter().any(|keys| keys == expected),
            "{expected}: {list:?}"
        );
    }
    assert_eq!(list[0], "Enter");
    assert!(!list.iter().any(|keys| keys == "Esc/c"), "nothing to clear");
    choose(&mut state, 's', "triaged");
    // Esc clears a filtered list before it hides the panel, so the sheet says that instead.
    let filtered = keys(&state);
    assert!(filtered.iter().any(|keys| keys == "Esc/c"));
    assert!(!filtered.iter().any(|keys| keys == "Esc"), "{filtered:?}");
    key(&mut state, BareKey::Char('a'));
    assert_eq!(keys(&state)[..3], ["y", "n/Esc", "any other key"]);
    key(&mut state, BareKey::Esc);
    // A box being typed in has no sheet: `?` is a character there.
    key(&mut state, BareKey::Char('/'));
    key(&mut state, BareKey::Char('?'));
    assert!(state.key_sheet_scroll.is_none());
    assert_eq!(
        state.swamp.editing.as_ref().map(TextInput::get_text),
        Some("?")
    );
    key(&mut state, BareKey::Esc);
    key(&mut state, BareKey::Char('f'));
    key(&mut state, BareKey::Char('?'));
    assert!(state.key_sheet_scroll.is_none());
    assert!(state.swamp.has_filter_editor());
    key(&mut state, BareKey::Esc);
    let state = reader();
    let reading = keys(&state);
    assert_eq!(reading[0], "Esc");
    assert!(reading.iter().any(|keys| keys == "click the top row"));
    let mut state = ready();
    state.swamp.permission = Permission::Unrequested;
    assert_eq!(keys(&state)[0], "r");
}

/// Esc is one step back before the way out, from a search or a filter as from the reader:
/// the panel's docs promised it, and the key used to hide the panel over a filtered list.
#[test]
fn esc_steps_out_of_a_search_or_filter_then_hides() {
    let mut state = loaded();
    choose(&mut state, 's', "triaged");
    choose(&mut state, 'o', "Created oldest first");
    choose(&mut state, '/', "two words");
    reply(&mut state, SEARCH);
    state.render(11, 80);
    key(&mut state, BareKey::Enter);
    assert_eq!(state.swamp.opened, Some(2254));
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    key(&mut state, BareKey::Esc);
    assert!(state.swamp.opened.is_none());
    assert_eq!(state.swamp.query, "two words");
    assert_eq!(STATUSES[state.swamp.status], "triaged");
    assert_eq!(state.panel_hides, 0);
    let count = state.swamp.requests.len();
    key(&mut state, BareKey::Esc);
    assert!(state.swamp.query.is_empty());
    assert_eq!(state.swamp.status, 0);
    assert_eq!(state.swamp.sort, 3, "sort is not a filter");
    assert_eq!(state.swamp.requests.len(), count, "the catalogue is reused");
    assert_eq!(state.panel_hides, 0);
    key(&mut state, BareKey::Esc);
    assert_eq!(state.panel_hides, 1);
}

/// `y` while the login cannot be read has nothing to fetch the issue with. It used to wait
/// for the next `r`, which then sent a triage asked for minutes before - and until then
/// `a` was refused, as if one were on its way.
#[test]
fn a_triage_confirmed_while_the_login_is_unreadable_is_dropped_not_parked() {
    let mut state = loaded();
    state.swamp.keep_credentials = false;
    key(&mut state, BareKey::Char('r'));
    let context = state.swamp.commands.last().unwrap().1.clone();
    state.swamp_result(Some(1), b"", b"Permission denied", &context);
    let requests = state.swamp.requests.len();
    triage(&mut state);
    assert_eq!(state.swamp.requests.len(), requests);
    assert!(!state.swamp.triage_waiting);
    assert!(state.swamp.triage_wanted.is_none());
    assert!(state.swamp.note().unwrap().contains("Permission denied"));
    key(&mut state, BareKey::Char('a'));
    assert!(state.swamp.confirming_triage.is_some(), "a asks again");
    key(&mut state, BareKey::Char('n'));
    // And `r` only reloads.
    key(&mut state, BareKey::Char('r'));
    answer_login(&mut state, b"");
    assert!(!state
        .swamp
        .requests
        .last()
        .unwrap()
        .0
        .url
        .contains("/issues/"));
    assert!(!matches!(
        state.swamp.pending.as_ref().map(|r| &r.target),
        Some(Target::Triage(_))
    ));
}

/// The note row says whichever happened last, a triage or a fetch error: a triage asked
/// for over a standing error used to say nothing of itself until the error went.
#[test]
fn a_triage_started_over_an_error_has_the_row_until_a_newer_error() {
    let mut state = loaded();
    state.swamp.cache_error = Some("disk full".into());
    assert!(state.swamp.note().unwrap().starts_with("Cache not saved"));
    triage(&mut state);
    assert_eq!(state.swamp.note().as_deref(), Some("Fetching #2254…"));
    assert_eq!(state.swamp.noted().unwrap().1, Tone::Quiet);
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    assert!(state.swamp.note().unwrap().starts_with("Sending #2254"));
    assert!(ack(&mut state, wire::AskOutcome::Refused));
    assert!(state
        .swamp
        .note()
        .unwrap()
        .starts_with("Hocus refused #2254"));
    assert_eq!(state.swamp.noted().unwrap().1, Tone::Error);
    // Moving on from the triage gives the row back to the error it was over.
    state.render(11, 80);
    key(&mut state, BareKey::Enter);
    assert!(state.swamp.note().unwrap().starts_with("Cache not saved"));

    // An error that lands while the triage waits is the newer news, until the triage moves.
    let mut state = loaded();
    key(&mut state, BareKey::Char('r'));
    triage(&mut state);
    assert_eq!(state.swamp.note().as_deref(), Some("Fetching #2254…"));
    reply_status(&mut state, 503, r#"{"error":"unavailable"}"#);
    assert!(state.swamp.note().unwrap().starts_with("Error:"));
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    assert!(state.swamp.note().unwrap().starts_with("Sending #2254"));
}

/// Putting the panel away ends what belongs to the moment, a filter being chosen among it,
/// and keeps a search being typed, as the Agents search box keeps its own.
#[test]
fn a_hide_closes_the_filter_editor_and_keeps_the_query_box() {
    let mut state = loaded();
    for owner in [true, false] {
        // hocus's `vms::VISIBILITY`, or the panel's own Esc.
        let hide = |state: &mut State| match owner {
            true => {
                state.update(pane_update(state.own_plugin_id, true));
            },
            false => state.hide_the_panel(),
        };
        let show = |state: &mut State| match owner {
            true => {
                state.update(pane_update(state.own_plugin_id, false));
            },
            false => {
                state.update(pane_update(state.own_plugin_id, false));
            },
        };
        key(&mut state, BareKey::Char('f'));
        type_text(&mut state, "provisional");
        hide(&mut state);
        assert!(!state.swamp.has_filter_editor(), "{owner}");
        show(&mut state);
        key(&mut state, BareKey::Char('/'));
        type_text(&mut state, "foo");
        hide(&mut state);
        assert!(!state.swamp.has_filter_editor(), "{owner}");
        assert_eq!(
            state.swamp.editing.as_ref().map(TextInput::get_text),
            Some("foo"),
            "{owner}"
        );
        show(&mut state);
        assert!(state.swamp.is_editing(), "{owner}");
        key(&mut state, BareKey::Esc);
    }
}

/// A list only re-filtered from the catalogue already held has retried nothing, so a
/// failed load's error stays up, `!` and all, until a request actually goes out.
#[test]
fn clearing_or_reusing_a_search_keeps_a_network_error() {
    let mut state = loaded();
    choose(&mut state, 's', "triaged");
    key(&mut state, BareKey::Char('r'));
    reply_status(&mut state, 503, r#"{"error":"Network unavailable"}"#);
    let count = state.swamp.requests.len();
    let still_failed = |state: &mut State| {
        assert_eq!(state.swamp.requests.len(), count);
        assert!(state.swamp.note().unwrap().contains("Network unavailable"));
        state.render(11, 80);
        assert!(state.swamp.has_error());
    };
    key(&mut state, BareKey::Char('c'));
    assert_eq!(state.swamp.status, 0);
    still_failed(&mut state);
    choose(&mut state, '/', "");
    still_failed(&mut state);
    choose(&mut state, '/', "two words");
    assert_eq!(state.swamp.requests.len(), count + 1);
    assert!(state.swamp.list_error.is_none());
}

/// A text search's failure is about that search: cleared, the catalogue shown in its place
/// has not failed, and neither the error nor `Swamp!` stays over it until `r`.
#[test]
fn a_failed_text_search_takes_its_error_with_it() {
    let mut state = loaded();
    choose(&mut state, '/', "flaky");
    reply_status(&mut state, 503, r#"{"error":"Network unavailable"}"#);
    assert!(state.swamp.note().unwrap().contains("Network unavailable"));
    state.render(11, 80);
    assert!(state.swamp.has_error());
    key(&mut state, BareKey::Char('c'));
    assert!(state.swamp.list_error.is_none());
    state.render(11, 80);
    assert!(!state.swamp.has_error());
}

/// At the default panel's 24 columns a note that comes and goes in a few seconds takes one
/// row, so the list under it holds still: a triage used to push it down a row for the
/// question, and again for the fetch.
#[test]
fn transient_notes_hold_one_row_at_the_narrowest_panel() {
    let mut state = loaded();
    let mut top = None;
    let mut one_row = |state: &mut State| {
        state.render(11, 24);
        let rows = state.swamp.note_rows(11, 24);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(state.swamp.note_rows(11, 46).len(), 1, "{rows:?}");
        let body_top = state.swamp.last_body_top;
        assert_eq!(*top.get_or_insert(body_top), body_top, "{rows:?}");
    };
    key(&mut state, BareKey::Char('a'));
    one_row(&mut state);
    key(&mut state, BareKey::Char('y'));
    one_row(&mut state);
    reply(&mut state, &DETAIL.replace("2180", "2254"));
    one_row(&mut state);
    // The acknowledgements stay put longer, and say where to go: two rows at most.
    for outcome in [wire::AskOutcome::Queued, wire::AskOutcome::Sent] {
        if outcome == wire::AskOutcome::Sent {
            triage(&mut state);
            reply(&mut state, &DETAIL.replace("2180", "2254"));
        }
        assert!(ack(&mut state, outcome));
        let rows = state.swamp.note_rows(11, 24);
        assert!(rows.len() <= 2, "{rows:?}");
        assert_eq!(state.swamp.note_rows(11, 46).len(), 1, "{rows:?}");
    }

    let mut state = ready();
    state.activate_swamp();
    assert_eq!(state.swamp.note_rows(11, 24).len(), 1);
    reply(&mut state, &page(&(1..=200).collect::<Vec<_>>(), 203));
    assert_eq!(state.swamp.note().as_deref(), Some("Loading… 200 so far"));
    assert_eq!(state.swamp.note_rows(11, 24).len(), 1);
    assert_eq!(state.swamp.note_rows(11, 46).len(), 1);
}

/// Tab in the `/` box closes it - the box takes every key ahead of the tab bar - but keeps
/// what was typed for the next `/`, and goes on to where Tab goes from the list.
#[test]
fn tab_in_the_query_box_walks_the_ring_and_keeps_the_draft() {
    let mut state = loaded();
    let count = state.swamp.requests.len();
    key(&mut state, BareKey::Char('/'));
    type_text(&mut state, "referral");
    assert!(state.on_key(KeyWithModifier::new(BareKey::Tab)));
    assert!(state.swamp.editing.is_none());
    assert_eq!(state.ring_yields, [wire::STOP_TABS]);
    assert_eq!(state.swamp.query, "", "walked away from, not searched");
    assert_eq!(state.swamp.requests.len(), count);
    key(&mut state, BareKey::Char('/'));
    assert_eq!(
        state.swamp.editing.as_ref().map(TextInput::get_text),
        Some("referral")
    );
    // Shift+Tab is a step the other way: onto the tab bar, whose keys are then the bar's.
    state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
    assert!(state.tabs_focused);
    assert!(state.swamp.editing.is_none());
    key(&mut state, BareKey::Left);
    assert!(state.tabs_focused);
    assert_eq!(state.ring_yields, [wire::STOP_TABS]);
    key(&mut state, BareKey::Right);
    key(&mut state, BareKey::Down);
    assert!(state.tabs_focused);
    key(&mut state, BareKey::Enter);
    assert!(!state.tabs_focused);
    // Clearing the search clears what was being typed for it too.
    choose(&mut state, 's', "triaged");
    key(&mut state, BareKey::Char('c'));
    key(&mut state, BareKey::Char('/'));
    assert_eq!(
        state.swamp.editing.as_ref().map(TextInput::get_text),
        Some("")
    );
}

/// While the panel's tab bar has the keys, the header names the bar's keys rather than the
/// tab's, which the next press does not reach - in the header's own row, not a footer that
/// takes a row off the list and gives it back as the focus moves.
#[test]
fn the_bar_hint_takes_the_swamp_header_not_a_row() {
    let mut state = loaded();
    state.render(11, 60);
    let body_rows = state.swamp.last_body_rows;
    assert!(state.swamp_header(60).starts_with("Enter open"));
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    state.render(11, 60);
    assert!(state.swamp_header(60).contains("←/→ choose"));
    assert_eq!(state.swamp.last_body_rows, body_rows);
    state.tabs_focused = false;
    state.bar_focused = vec![];
    state.render(11, 60);
    assert_eq!(state.swamp.last_body_rows, body_rows);
    assert!(!state.swamp_header(60).contains("←/→ choose"));
    // An open box keeps its draft while the bar owns the keys and hints.
    key(&mut state, BareKey::Char('s'));
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    assert!(state.swamp_header(60).contains("←/→ choose"));
    key(&mut state, BareKey::Esc);
    // The reader's header is the same row.
    let mut state = reader();
    state.tabs_focused = true;
    state.bar_focused = vec![state.own_client_id];
    state.render(11, 80);
    assert!(state.swamp_header(60).contains("←/→ choose"));
}

#[test]
#[ignore = "manual check: reads the host's swamp login and calls swamp-club over the network"]
fn live_swamp_club() {
    let argv = swamp_lab::auth::auth_json_argv();
    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .output()
        .unwrap();
    let file = swamp_lab::auth::auth_json_from_output(
        output.status.code(),
        &output.stdout,
        &output.stderr,
    )
    .unwrap();
    let credentials = Credentials::resolve(&std::env::vars().collect(), file.as_deref()).unwrap();
    let send = |request: swamp_lab::Request| {
        use isahc::{ReadResponseExt, RequestExt};
        let mut builder = isahc::Request::get(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        let mut response = builder.body(()).unwrap().send().unwrap();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_owned()))
            .collect::<BTreeMap<_, _>>();
        (
            response.status().as_u16(),
            headers,
            response.bytes().unwrap(),
        )
    };
    let client = Client::new(&credentials);
    let (status, headers, body) = send(client.search(&search_query("", 0).unwrap()).unwrap());
    let search = swamp_lab::parse::<lab::SearchResponse>(status, &headers, &body).unwrap();
    let search = Search::from_lab(&search, &credentials);
    assert!(search.issues.len() <= PAGE as usize);
    if let Some(issue) = search.issues.first() {
        let (status, headers, body) = send(client.get(issue.number).unwrap());
        let detail = swamp_lab::parse::<lab::IssueDetail>(status, &headers, &body).unwrap();
        let detail = Detail::from_lab(&detail, &credentials);
        assert_eq!(detail.issue.number, issue.number);
        assert!(!detail_lines(&detail, 80).is_empty());
    }
}

#[test]
fn queued_bar_keys_and_paste_stay_out_of_content() {
    let mut state = State::default();
    state.take_ring_stop(Some(hocus_proto::vms::STOP_TABS));
    for key in [
        BareKey::Right,
        BareKey::Right,
        BareKey::Left,
        BareKey::Down,
        BareKey::Char('j'),
        BareKey::Char('/'),
        BareKey::Char('?'),
    ] {
        assert!(state.on_key(KeyWithModifier::new(key)));
        assert!(state.tabs_focused);
    }
    assert!(!state.on_paste("queued paste"));
    assert!(state.tabs_focused);
    for key in [BareKey::Enter, BareKey::Tab, BareKey::Esc] {
        state.take_ring_stop(Some(hocus_proto::vms::STOP_TABS));
        assert!(state.on_key(KeyWithModifier::new(key)));
        assert!(!state.tabs_focused);
    }
}
