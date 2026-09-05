//! What the outgoing body leaves out because the transcript already holds something newer.
//!
//! The thing under test is a trade, so most of these assert both halves of it: the stale copy
//! is gone, *and* the newest one is still there, *and* the transcript itself never moved.

use super::*;

/// A `plugin_write` as the model sends one: the whole file in the arguments.
fn write(id: &str, name: &str, source: &str) -> Message {
    Message::tool_calls(vec![call(
        id,
        "plugin_write",
        &serde_json::json!({ "name": name, "source": source, "overwrite": true }).to_string(),
    )])
}

fn plugin_call(id: &str, tool: &str, args: serde_json::Value) -> Message {
    Message::tool_calls(vec![call(id, tool, &args.to_string())])
}

fn result(id: &str, content: &str) -> Message {
    Message::tool_result(id, content)
}

/// A tool result at the size the ones this is about actually are: a `plugin_read` page, a pane
/// render, a docs section. A placeholder is ~150 characters and is only worth sending in place
/// of something bigger, so a toy fixture would test the guard rather than the rule.
fn bulky(marker: &str) -> String {
    format!(
        "{marker}\n{}",
        "a line of the thing that came back\n".repeat(60)
    )
}

/// Every `arguments` string in the outgoing body, in order.
fn wire_arguments_in(body: &serde_json::Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .map(|call| {
            call["function"]["arguments"]
                .as_str()
                .expect("a string")
                .to_owned()
        })
        .collect()
}

fn wire_tool_results(body: &serde_json::Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].as_str().expect("a string").to_owned())
        .collect()
}

/// Plugin-sized, not example-sized. A placeholder is ~150 characters, so a two-line source is
/// not worth replacing and is deliberately left alone - see `stale_arguments`. What this is
/// about is the 4-12 KB file a real plugin is.
fn draft(marker: &str) -> String {
    format!(
        "-- {marker}\nlocal M = {{}}\n{}return M\n",
        "local function helper() return 1 end\n".repeat(60)
    )
}

const DRAFT: &str = "draft one";
const FINAL: &str = "draft two, with the feature";

/// The single largest thing in a plugin-authoring transcript: a `plugin_write` carries the
/// complete Lua source in its arguments, and those arguments are echoed back on every later
/// request for the rest of the conversation. Five rounds of write-reload-fix used to re-upload
/// four drafts nobody would read again.
#[test]
fn a_superseded_write_sends_a_placeholder_instead_of_the_source() {
    let cfg = test_config();
    let transcript = vec![
        Message::user("write me a plugin"),
        write("c1", "todo.lua", &draft(DRAFT)),
        result("c1", "wrote todo.lua"),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let arguments = wire_arguments_in(&body_value(&cfg, &transcript));

    assert!(!arguments[0].contains("draft one"), "{}", arguments[0]);
    assert!(
        arguments[0].contains("plugin_read name=todo"),
        "{}",
        arguments[0]
    );
    assert!(
        arguments[1].contains("draft two, with the feature"),
        "the newest draft is what the model is reasoning from: {}",
        arguments[1]
    );
}

/// The rule is "the most recent of each subject", not "the most recent overall". Two plugins
/// being written in one conversation is ordinary, and neither supersedes the other.
#[test]
fn a_write_is_only_superseded_by_a_write_of_the_same_file() {
    let cfg = test_config();
    let transcript = vec![
        write("c1", "todo.lua", &draft(DRAFT)),
        result("c1", "wrote todo.lua"),
        write("c2", "clock.lua", &draft("the clock")),
        result("c2", "wrote clock.lua"),
    ];

    let arguments = wire_arguments_in(&body_value(&cfg, &transcript));

    assert!(arguments[0].contains("draft one"), "{}", arguments[0]);
    assert!(arguments[1].contains("the clock"), "{}", arguments[1]);
}

/// `.lua` is optional everywhere the tools take a name, so the two spellings have to be the
/// same file here or half the supersessions would be missed.
#[test]
fn the_lua_suffix_does_not_make_it_a_different_file() {
    let cfg = test_config();
    let transcript = vec![
        write("c1", "todo", &draft(DRAFT)),
        result("c1", "wrote todo.lua"),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let arguments = wire_arguments_in(&body_value(&cfg, &transcript));

    assert!(!arguments[0].contains("draft one"), "{}", arguments[0]);
}

/// An edit is the cheap path and its strings are small, but a later *write* replaces the whole
/// file, which makes the diff it describes a diff against something that no longer exists.
#[test]
fn an_edit_is_superseded_by_a_later_write_of_the_same_file() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_edit",
            serde_json::json!({
                "name": "todo.lua",
                "old_string": "return M",
                // An edit that inserts a block, which is the shape of edit big enough to be
                // worth eliding. A one-line edit is smaller than the placeholder and is left
                // exactly as it is - see `stale_arguments`.
                "new_string": draft("setmetatable"),
            }),
        ),
        result("c1", "edited todo.lua"),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let arguments = wire_arguments_in(&body_value(&cfg, &transcript));

    assert!(!arguments[0].contains("setmetatable"), "{}", arguments[0]);
    assert!(
        arguments[0].contains("todo.lua"),
        "the name survives, so the call still says which file it was about: {}",
        arguments[0]
    );
}

/// A read is a picture of a file at a moment. A later read, or any later change, makes it a
/// picture of something that is no longer there - and it is up to 40k characters.
#[test]
fn a_read_is_superseded_by_a_later_read_or_change_of_the_same_file() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_read",
            serde_json::json!({ "name": "todo.lua" }),
        ),
        result("c1", &bulky("the old file")),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
        plugin_call(
            "c3",
            "plugin_read",
            serde_json::json!({ "name": "todo.lua" }),
        ),
        result("c3", &bulky("the new file")),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("the old file"), "{}", results[0]);
    assert!(
        results[0].contains("plugin_read name=todo"),
        "{}",
        results[0]
    );
    assert_eq!(results[1], "wrote todo.lua", "reports are small; they stay");
    assert!(results[2].contains("the new file"), "{}", results[2]);
}

/// A read is about a *page*, not a file. Walking a 900-line plugin in three reads used to elide
/// the first two as "out of date" though nothing had changed under them - and the recovery hint
/// sent the model back to page 1, which it already had.
#[test]
fn paging_through_a_file_keeps_every_page() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_read",
            serde_json::json!({ "name": "todo.lua" }),
        ),
        result("c1", &bulky("page one")),
        plugin_call(
            "c2",
            "plugin_read",
            serde_json::json!({ "name": "todo.lua", "start_line": 301 }),
        ),
        result("c2", &bulky("page two")),
        plugin_call(
            "c3",
            "plugin_read",
            serde_json::json!({ "name": "todo.lua", "start_line": 601 }),
        ),
        result("c3", &bulky("page three")),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(results[0].contains("page one"), "{}", results[0]);
    assert!(results[1].contains("page two"), "{}", results[1]);
    assert!(results[2].contains("page three"), "{}", results[2]);
}

/// The same page fetched twice is still the same page twice, and the second one is the one to
/// keep.
#[test]
fn the_same_page_read_twice_is_sent_once() {
    let cfg = test_config();
    let page = serde_json::json!({ "name": "todo.lua", "start_line": 301, "line_count": 300 });
    let transcript = vec![
        plugin_call("c1", "plugin_read", page.clone()),
        result("c1", &bulky("the first look")),
        plugin_call("c2", "plugin_read", page),
        result("c2", &bulky("the second look")),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("the first look"), "{}", results[0]);
    assert!(results[0].contains("lines 301-600"), "{}", results[0]);
    assert!(results[1].contains("the second look"), "{}", results[1]);
}

/// A read the file has outgrown is still elided, and the hint that replaces it has to name the
/// page - sending the model back to line 1 for a file it was reading at line 600 is how a
/// second, larger read gets spent on text it already had.
#[test]
fn a_page_a_write_invalidated_is_recovered_at_its_own_line() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_read",
            serde_json::json!({ "name": "todo.lua", "start_line": 301 }),
        ),
        result("c1", &bulky("the old page")),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("the old page"), "{}", results[0]);
    assert!(
        results[0].contains("plugin_read name=todo start_line=301"),
        "{}",
        results[0]
    );
}

/// `plugin_check` names a file and sends nothing. It used to claim the file's source slot all
/// the same, which elided the `plugin_write` behind it as "replaced by a later write" that
/// never happened - taking away the only copy of the source the model was working from.
#[test]
fn a_check_does_not_supersede_the_write_it_is_checking() {
    let cfg = test_config();
    let transcript = vec![
        write("c1", "todo.lua", &draft(FINAL)),
        result("c1", "wrote todo.lua"),
        plugin_call(
            "c2",
            "plugin_check",
            serde_json::json!({ "name": "todo.lua" }),
        ),
        result("c2", "todo.lua compiles"),
    ];

    let arguments = wire_arguments_in(&body_value(&cfg, &transcript));

    assert!(arguments[0].contains(FINAL), "{}", arguments[0]);
}

/// Two edits of one file in one batch arrive as one assistant message. The walk is backwards so
/// that the newest wins, and that has to hold inside a turn as well as across them: forwards,
/// the *first* call was taken for the newest and the version that stands was the one elided.
#[test]
fn the_last_call_of_a_batch_is_the_one_that_stands() {
    let cfg = test_config();
    let transcript = vec![
        Message::tool_calls(vec![
            call(
                "c1",
                "plugin_write",
                &serde_json::json!({ "name": "todo.lua", "source": draft(DRAFT) }).to_string(),
            ),
            call(
                "c2",
                "plugin_write",
                &serde_json::json!({ "name": "todo.lua", "source": draft(FINAL) }).to_string(),
            ),
        ]),
        result("c1", "wrote todo.lua"),
        result("c2", "wrote todo.lua"),
    ];

    let arguments = wire_arguments_in(&body_value(&cfg, &transcript));

    assert!(!arguments[0].contains(DRAFT), "{}", arguments[0]);
    assert!(arguments[1].contains(FINAL), "{}", arguments[1]);
}

/// A reload answer embeds the plugin pane's render. Three reloads of the same plugin is three
/// screenshots, two of which are of a version that has since been replaced.
#[test]
fn a_reload_dump_is_superseded_by_a_later_reload_of_the_same_plugin() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_reload",
            serde_json::json!({ "name": "todo.lua" }),
        ),
        result("c1", &bulky("first render")),
        plugin_call(
            "c2",
            "plugin_reload",
            serde_json::json!({ "name": "todo.lua" }),
        ),
        result("c2", &bulky("second render")),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("first render"), "{}", results[0]);
    assert!(results[1].contains("second render"), "{}", results[1]);
}

/// The same docs section fetched twice is 4-13 KB of identical text sitting in the transcript
/// twice, and re-sent twice on every later round.
#[test]
fn a_repeated_docs_section_is_sent_once() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_docs",
            serde_json::json!({ "section": "data" }),
        ),
        result("c1", &bulky("the store rules")),
        plugin_call(
            "c2",
            "plugin_docs",
            serde_json::json!({ "section": "hooks" }),
        ),
        result("c2", &bulky("the hook rules")),
        plugin_call(
            "c3",
            "plugin_docs",
            serde_json::json!({ "section": "data" }),
        ),
        result("c3", &bulky("the store rules")),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("the store rules"), "{}", results[0]);
    assert!(
        results[1].contains("the hook rules"),
        "a different section stays"
    );
    assert!(results[2].contains("the store rules"), "{}", results[2]);
}

/// The invariant `chat_request_body` names: every call must be answered, and every arguments
/// string must be a JSON object, or the next request is rejected outright.
#[test]
fn an_elided_body_is_still_a_well_formed_request() {
    let cfg = test_config();
    let transcript = vec![
        write("c1", "todo.lua", &draft(DRAFT)),
        result("c1", "wrote todo.lua"),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let body = body_value(&cfg, &transcript);

    let answered: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["tool_call_id"].as_str().unwrap())
        .collect();
    assert_eq!(answered, vec!["c1", "c2"]);
    for arguments in wire_arguments_in(&body) {
        let parsed: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&arguments).expect("arguments must be a JSON object");
        assert!(parsed.contains_key("name"), "{arguments}");
    }
}

/// The archive writes `Message` as it stands and the pane draws `content`/`display`. Elision
/// happens on the way out and must leave both alone, or a conversation would be missing from
/// its own history.
#[test]
fn elision_never_touches_the_transcript() {
    let cfg = test_config();
    let transcript = vec![
        write("c1", "todo.lua", &draft(DRAFT)),
        result("c1", "wrote todo.lua"),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];
    let before = transcript.clone();

    let _ = request_body(&cfg, &transcript);

    assert_eq!(transcript, before);
}

/// The Responses transport pairs each reasoning item with the output item it produced, by
/// position. Rewriting an argument string changes no positions - but that is the kind of thing
/// that is true until someone reorders the loop, so it is asserted rather than argued.
#[test]
fn elision_leaves_the_responses_item_order_alone() {
    let mut cfg = test_config();
    cfg.api = Api::Responses;
    let transcript = vec![
        write("c1", "todo.lua", &draft(DRAFT)),
        result("c1", "wrote todo.lua"),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let body: serde_json::Value = serde_json::from_slice(&request_body(&cfg, &transcript)).unwrap();

    let kinds: Vec<&str> = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "function_call",
            "function_call_output",
            "function_call",
            "function_call_output"
        ]
    );
    let first = body["input"][0]["arguments"].as_str().unwrap();
    assert!(!first.contains("draft one"), "{first}");
    assert_eq!(
        body["input"][0]["call_id"], "c1",
        "the pairing is untouched"
    );
}

/// The guard that keeps this from being a false economy: a placeholder is ~150 characters, so
/// replacing a short payload with one costs bytes *and* costs the model the text. Small things
/// are left exactly as they are.
#[test]
fn something_smaller_than_the_placeholder_is_left_alone() {
    let cfg = test_config();
    let transcript = vec![
        write("c1", "todo.lua", "return {}\n"),
        result("c1", "wrote todo.lua"),
        write("c2", "todo.lua", "return { 1 }\n"),
        result("c2", "wrote todo.lua"),
    ];

    let arguments = wire_arguments_in(&body_value(&cfg, &transcript));

    assert!(arguments[0].contains("return {}"), "{}", arguments[0]);
    assert_eq!(
        request_body(&cfg, &transcript).len(),
        {
            // The same body the un-elided builder would produce, since nothing was elided.
            let plain = vec![
                write("c1", "todo.lua", "return {}\n"),
                result("c1", "wrote todo.lua"),
                write("c2", "todo.lua", "return { 1 }\n"),
                result("c2", "wrote todo.lua"),
            ];
            request_body(&cfg, &plain).len()
        },
        "a transcript with nothing worth eliding goes out byte-identical"
    );
}

/// A call whose arguments never parsed - a truncated stream, a model that emitted prose - has
/// no subject, and must pass through untouched rather than being rewritten into one.
#[test]
fn a_call_with_unparseable_arguments_is_left_alone() {
    let cfg = test_config();
    let transcript = vec![
        Message::tool_calls(vec![call(
            "c1",
            "plugin_write",
            "{\"name\": \"todo.lua\", \"sou",
        )]),
        result("c1", "arguments were not valid JSON"),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let arguments = wire_arguments_in(&body_value(&cfg, &transcript));

    // `wire_arguments` still substitutes `{}` for a non-object, which is the pre-existing rule
    // and is what keeps the next request well formed.
    assert_eq!(arguments[0], "{}");
    assert!(arguments[1].contains("draft two"), "{}", arguments[1]);
}

/// `body_bytes` says a request was large. This is what says why, and it has to name the tool a
/// result came from - which is only knowable through the call that produced it.
#[test]
fn the_breakdown_names_the_biggest_contributors() {
    let transcript = vec![
        Message::user("write me a plugin"),
        plugin_call(
            "c1",
            "plugin_docs",
            serde_json::json!({ "section": "data" }),
        ),
        result("c1", &bulky("the data section")),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let rows = wire_breakdown(&transcript, 3);

    let labels: Vec<&str> = rows.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels[0], "call:plugin_write", "{rows:?}");
    assert!(labels.contains(&"result:plugin_docs"), "{rows:?}");
    assert!(
        rows.windows(2).all(|pair| pair[0].1 >= pair[1].1),
        "largest first: {rows:?}"
    );
}

/// The breakdown reports what went out, not what is stored, or it would point at a superseded
/// draft as the thing to do something about.
#[test]
fn the_breakdown_reports_the_elided_sizes() {
    let transcript = vec![
        write("c1", "todo.lua", &draft(DRAFT)),
        result("c1", "wrote todo.lua"),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let rows = wire_breakdown(&transcript, 4);
    let sizes: Vec<usize> = rows.iter().map(|(_, chars)| *chars).collect();

    assert!(
        sizes[0] > sizes[1] * 2,
        "the superseded draft is a fraction of the live one: {rows:?}"
    );
}

/// Not a contract, a measurement: a realistic plugin-authoring transcript, and what a request
/// carrying it costs now. Printed rather than asserted tightly, so a regression shows up as a
/// number in the run rather than as a mystery.
#[test]
fn a_realistic_authoring_transcript_is_a_fraction_of_what_it_was() {
    let mut cfg = test_config();
    cfg.tools = true;
    let source = |marker: &str| {
        format!(
            "-- {marker}\nlocal M = {{}}\n{}return M\n",
            "local function helper(a, b) return a + b end\n".repeat(140)
        )
    };
    let pane = |marker: &str| format!("{marker}\n{}", "│ a row of the dashboard │\n".repeat(120));
    let docs = |marker: &str| format!("## {marker}\n{}", "a paragraph of the guide.\n".repeat(150));

    let mut transcript = vec![Message::user("write me a todo plugin")];
    for section in ["quickstart", "data", "commands"] {
        transcript.push(plugin_call(
            &format!("d{section}"),
            "plugin_docs",
            serde_json::json!({ "section": section }),
        ));
        transcript.push(result(&format!("d{section}"), &docs(section)));
    }
    // Five rounds of the loop this is all about: write, reload, look, fix.
    for round in 1..=5 {
        let write_id = format!("w{round}");
        transcript.push(write(
            &write_id,
            "todo.lua",
            &source(&format!("draft {round}")),
        ));
        transcript.push(result(&write_id, "wrote todo.lua"));
        let reload_id = format!("r{round}");
        transcript.push(plugin_call(
            &reload_id,
            "plugin_reload",
            serde_json::json!({ "name": "todo.lua" }),
        ));
        transcript.push(result(&reload_id, &pane(&format!("render {round}"))));
    }

    let stored: usize = transcript
        .iter()
        .map(|m| {
            m.content.len()
                + m.tool_calls
                    .iter()
                    .map(|c| c.arguments.len())
                    .sum::<usize>()
        })
        .sum();
    let body = body_value(&cfg, &transcript);
    let sent = serde_json::to_vec(&body["messages"])
        .expect("messages serialize")
        .len();
    let body_bytes = serde_json::to_vec(&body).expect("body serializes").len();

    println!(
        "stored {stored} chars -> {sent} message bytes ({}% saved); whole body {body_bytes} bytes, of \
         which ~29k is the tool specs",
        100 - sent * 100 / stored
    );
    assert!(
        sent < stored / 2,
        "the transcript should go out at under half its stored size: {sent} vs {stored}"
    );
}

/// A search is a picture of the plugin it looked through. A later edit of that plugin makes it
/// a picture of something that is no longer there, and the placeholder says to search again.
#[test]
fn a_search_is_superseded_by_a_later_edit_of_the_plugin_it_searched() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_search",
            serde_json::json!({ "name": "todo.lua", "query": "render" }),
        ),
        result("c1", &bulky("todo.lua:12:function M.render()")),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("function M.render"), "{}", results[0]);
    assert!(results[0].contains("out of date"), "{}", results[0]);
}

/// A search of the folder is about every plugin, so any later write stales it; one confined to
/// a plugin is stale only when that plugin changes.
#[test]
fn a_folder_wide_search_is_superseded_by_any_later_write_but_a_named_one_only_by_its_own() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_search",
            serde_json::json!({ "query": "render" }),
        ),
        result("c1", &bulky("every render")),
        plugin_call(
            "c2",
            "plugin_search",
            serde_json::json!({ "name": "todo", "query": "render" }),
        ),
        result("c2", &bulky("todo's render")),
        write("c3", "clock.lua", &draft("the clock")),
        result("c3", "wrote clock.lua"),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("every render"), "{}", results[0]);
    assert!(results[1].contains("todo's render"), "{}", results[1]);
}

/// The same words looked for twice are the same search whatever context was asked for, and the
/// later answer is the one kept; different words are a different search and stay.
#[test]
fn the_same_search_twice_is_sent_once_whatever_its_context() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_search",
            serde_json::json!({ "query": "render", "context": 2 }),
        ),
        result("c1", &bulky("first look")),
        plugin_call(
            "c2",
            "plugin_search",
            serde_json::json!({ "query": "on\\(", "context": 2 }),
        ),
        result("c2", &bulky("the handlers")),
        plugin_call(
            "c3",
            "plugin_search",
            serde_json::json!({ "query": "render" }),
        ),
        result("c3", &bulky("second look")),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("first look"), "{}", results[0]);
    assert!(results[0].contains("appears again below"), "{}", results[0]);
    assert!(results[1].contains("the handlers"), "{}", results[1]);
    assert!(results[2].contains("second look"), "{}", results[2]);
}

/// An outline is a map of the file as it was; an edit redraws it, and another plugin's outline
/// is another map altogether.
#[test]
fn an_outline_is_superseded_by_a_later_edit_of_the_same_plugin() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_outline",
            serde_json::json!({ "name": "todo.lua" }),
        ),
        result("c1", &bulky("the old map")),
        plugin_call(
            "c2",
            "plugin_edit",
            serde_json::json!({
                "name": "todo",
                "old_string": "return M",
                "new_string": draft("setmetatable"),
            }),
        ),
        result(
            "c2",
            "edited todo.lua at line 60 (now 64 lines); call plugin_reload to load it",
        ),
        plugin_call(
            "c3",
            "plugin_outline",
            serde_json::json!({ "name": "clock" }),
        ),
        result("c3", &bulky("the clock's map")),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("the old map"), "{}", results[0]);
    assert!(
        results[0].contains("plugin_outline name=todo"),
        "{}",
        results[0]
    );
    assert!(
        results[2].contains("the clock's map"),
        "another plugin's outline stays"
    );
}

/// A listing is of the folder, and a write adds to the folder.
#[test]
fn a_listing_is_superseded_by_a_later_write_or_listing() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call("c1", "plugin_list", serde_json::json!({})),
        result("c1", &bulky("the old folder")),
        write("c2", "clock.lua", &draft("the clock")),
        result("c2", "wrote clock.lua"),
        plugin_call("c3", "plugin_list", serde_json::json!({})),
        result("c3", &bulky("the new folder")),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("the old folder"), "{}", results[0]);
    assert!(results[2].contains("the new folder"), "{}", results[2]);
}

/// A docs query is keyed by what it looked for and where: the same one again is sent once, and
/// one confined to a section is not the same as one over the whole guide. Pages of a section
/// are kept alongside each other, as a plugin's pages are.
#[test]
fn a_repeated_docs_query_is_sent_once_and_pages_of_a_section_are_kept() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_docs",
            serde_json::json!({ "query": "run_command" }),
        ),
        result("c1", &bulky("every spelling")),
        plugin_call(
            "c2",
            "plugin_docs",
            serde_json::json!({ "query": "run_command", "section": "prelude_commands" }),
        ),
        result("c2", &bulky("the prelude's spelling")),
        plugin_call(
            "c3",
            "plugin_docs",
            serde_json::json!({ "section": "prelude_commands", "start_line": 1, "line_count": 100 }),
        ),
        result("c3", &bulky("page one")),
        plugin_call(
            "c4",
            "plugin_docs",
            serde_json::json!({ "section": "prelude_commands", "start_line": 101, "line_count": 100 }),
        ),
        result("c4", &bulky("page two")),
        plugin_call(
            "c5",
            "plugin_docs",
            serde_json::json!({ "query": "run_command" }),
        ),
        result("c5", &bulky("every spelling again")),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(!results[0].contains("every spelling"), "{}", results[0]);
    assert!(
        results[1].contains("the prelude's spelling"),
        "{}",
        results[1]
    );
    assert!(results[2].contains("page one"), "{}", results[2]);
    assert!(results[3].contains("page two"), "{}", results[3]);
    assert!(
        results[4].contains("every spelling again"),
        "{}",
        results[4]
    );
}

/// `data.lua` is a plugin and `data` is a section, and a page of one is not a page of the other.
#[test]
fn a_docs_page_and_a_plugin_page_of_the_same_name_are_different_subjects() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_read",
            serde_json::json!({ "name": "data.lua", "start_line": 1, "line_count": 100 }),
        ),
        result("c1", &bulky("the plugin")),
        plugin_call(
            "c2",
            "plugin_docs",
            serde_json::json!({ "section": "data", "start_line": 1, "line_count": 100 }),
        ),
        result("c2", &bulky("the section")),
    ];

    let results = wire_tool_results(&body_value(&cfg, &transcript));

    assert!(results[0].contains("the plugin"), "{}", results[0]);
    assert!(results[1].contains("the section"), "{}", results[1]);
}

/// A batch carries as much code as a write does, in `edits`, and a later write replaces it the
/// same way: the array goes and the name stays, so the call still says which file it was about
/// and the arguments are still an object.
#[test]
fn a_superseded_batch_edit_sends_a_placeholder_instead_of_its_edits() {
    let cfg = test_config();
    let transcript = vec![
        plugin_call(
            "c1",
            "plugin_edit",
            serde_json::json!({
                "name": "todo.lua",
                "edits": [
                    { "old_string": "return M", "new_string": draft("setmetatable") },
                    { "old_string": "local M = {}", "new_string": "local M = { tick = 0 }" },
                ],
            }),
        ),
        result(
            "c1",
            "edited todo.lua at lines 2-64, line 70 (now 130 lines); call plugin_reload to load it",
        ),
        write("c2", "todo.lua", &draft(FINAL)),
        result("c2", "wrote todo.lua"),
    ];

    let arguments = wire_arguments_in(&body_value(&cfg, &transcript));

    assert!(!arguments[0].contains("setmetatable"), "{}", arguments[0]);
    let elided: serde_json::Value = serde_json::from_str(&arguments[0]).unwrap();
    assert_eq!(elided["name"], "todo.lua");
    assert!(
        elided["edits"].is_string(),
        "the array became the placeholder: {}",
        arguments[0]
    );
    assert!(arguments[1].contains(FINAL), "{}", arguments[1]);
}

/// And a batch is itself the newest statement about a file, so the write before it goes.
#[test]
fn a_batch_edit_is_the_newest_statement_about_a_file() {
    let cfg = test_config();
    let transcript = vec![
        write("c1", "todo.lua", &draft(DRAFT)),
        result("c1", "wrote todo.lua"),
        plugin_call(
            "c2",
            "plugin_edit",
            serde_json::json!({
                "name": "todo.lua",
                "edits": [{ "old_string": "return M", "new_string": draft("setmetatable") }],
            }),
        ),
        result(
            "c2",
            "edited todo.lua at lines 2-64 (now 130 lines); call plugin_reload to load it",
        ),
    ];

    let arguments = wire_arguments_in(&body_value(&cfg, &transcript));

    assert!(!arguments[0].contains(DRAFT), "{}", arguments[0]);
    assert!(arguments[1].contains("setmetatable"), "{}", arguments[1]);
}

#[test]
fn docs_search_elision_distinguishes_case_modes_and_defaults_to_sensitive() {
    let cfg = test_config();
    let mut transcript = Vec::new();
    for (id, case_mode) in [
        ("c1", None),
        ("c2", Some(true)),
        ("c3", Some(false)),
        ("c4", Some(true)),
    ] {
        let mut arguments = serde_json::json!({"query": "Render"});
        if let Some(mode) = case_mode {
            arguments["case_insensitive"] = mode.into();
        }
        transcript.push(plugin_call(id, "plugin_docs", arguments));
        transcript.push(result(id, &bulky(id)));
    }
    let distinct = wire_tool_results(&body_value(&cfg, &transcript[..4]));
    assert!(distinct[0].contains("c1"));
    assert!(distinct[1].contains("c2"));
    let repeated = wire_tool_results(&body_value(&cfg, &transcript));
    assert!(repeated[0].starts_with("[elided:"));
    assert!(repeated[1].starts_with("[elided:"));
    assert!(repeated[2].contains("c3"));
    assert!(repeated[3].contains("c4"));
}
