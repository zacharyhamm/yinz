//! The plugin-authoring tools' pure halves: turning a name the model sent into a path - the real
//! one and the one under the host's `/plugins` root - and shaping what goes to and comes back from
//! the host's file commands.
//!
//! All of it runs before anything reaches the host, which is the point — a name that escapes the
//! plugin folder, or a write with nothing to write, is refused without a file being touched.

use super::*;
use zellij_tile::prelude::{FileMetadata, FsEntry};

const DIR: &str = "/home/someone/.config/zellij/plugins";

/// A compile error names the file the loader would name, whatever spelling the write used.
#[test]
fn the_chunk_name_is_the_file_the_loader_would_name() {
    for (name, chunk) in [
        ("keys", "keys.lua"),
        ("keys.lua", "keys.lua"),
        (" keys ", "keys.lua"),
        ("app/ui", "ui.lua"),
        ("hocus/weather", "weather.lua"),
    ] {
        assert_eq!(chunk_name(name), chunk, "{name}");
    }
}

/// Off wasm every source compiles, so what is tested is the shape around the answer: only a
/// `plugin_write` with a name and a source is ever refused, and never by the host stand-in.
#[test]
fn only_a_complete_plugin_write_is_candidate_for_a_compile_refusal() {
    let call = |name: &str, arguments: &str| ToolCall {
        id: "1".to_owned(),
        name: name.to_owned(),
        arguments: arguments.to_owned(),
    };
    assert_eq!(
        uncompilable_write(&call(
            "plugin_write",
            r#"{"name":"keys","source":"return {}"}"#
        )),
        None
    );
    assert_eq!(
        uncompilable_write(&call("plugin_write", r#"{"name":"keys"}"#)),
        None,
        "a missing source is plugin_source's refusal, not a compile error"
    );
    assert_eq!(
        uncompilable_write(&call("plugin_reload", r#"{"name":"keys"}"#)),
        None
    );
    assert_eq!(
        compile_report("keys", "return {}").unwrap(),
        "keys.lua compiles"
    );
}

fn args(arguments: &str) -> Args {
    Args::parse(arguments).unwrap()
}

#[test]
fn a_name_resolves_with_or_without_its_extension() {
    for named in ["keys", "keys.lua"] {
        assert_eq!(
            resolve_plugin_file(DIR, Some(named)).unwrap(),
            format!("{DIR}/keys.lua"),
            "'{named}' should land on the same file"
        );
    }
    // Trimmed, because a model that pads a value should not thereby name a different file.
    assert_eq!(
        resolve_plugin_file(DIR, Some("  keys  ")).unwrap(),
        format!("{DIR}/keys.lua")
    );
}

/// The path a reload is built from must be absolute, and this is the guard on that.
///
/// `RunPluginLocation::parse` joins a *relative* `file:` path onto the cwd whenever one is
/// supplied, and `start_or_reload_plugin` always supplies one — so a relative URL would be
/// resolved against the session's cwd and never reach the plugin folder at all. It would fail
/// only at load time, in a pane, which is exactly where nobody is looking.
#[test]
fn a_resolved_plugin_path_is_always_absolute() {
    for named in ["keys", "keys.lua", "a-b_c.1"] {
        let path = resolve_plugin_file(DIR, Some(named)).unwrap();
        assert!(
            std::path::Path::new(&path).is_absolute(),
            "{named:?} resolved to the relative {path:?}"
        );
        assert!(path.starts_with(DIR), "{path:?} escaped {DIR}");
        assert!(path.ends_with(".lua"), "{path:?} is not a Lua file");
    }
}

#[test]
fn a_name_that_is_a_path_is_refused() {
    for named in [
        "../keys",
        "a/b/c",
        "/etc/passwd",
        "..",
        ".",
        "sub/../keys",
        "a/",
        "/a",
        "a//b",
    ] {
        assert!(
            resolve_plugin_file(DIR, Some(named)).is_err(),
            "{named:?} should not be accepted as a plugin name"
        );
    }
}

/// One folder is allowed, for the modules a plugin `require`s: `app/ui` is
/// `<dir>/app/ui.lua`, which `require("app.ui")` finds beside `app.lua`.
#[test]
fn a_module_name_resolves_one_folder_down() {
    assert_eq!(
        resolve_plugin_file(DIR, Some("app/ui")).unwrap(),
        format!("{DIR}/app/ui.lua")
    );
    assert_eq!(
        resolve_plugin_file(DIR, Some("app/init.lua")).unwrap(),
        format!("{DIR}/app/init.lua")
    );
}

/// The host's file commands take the same name under `/plugins`: one set of rules, two roots,
/// so the file that is written and the file that is reloaded cannot be different files.
#[test]
fn a_plugin_fs_path_is_the_same_name_under_the_plugins_root() {
    assert_eq!(plugin_fs_path(Some("keys")).unwrap(), "/plugins/keys.lua");
    assert_eq!(
        plugin_fs_path(Some("app/ui")).unwrap(),
        "/plugins/app/ui.lua"
    );
    assert!(plugin_fs_path(Some("../keys")).is_err());
    assert!(plugin_fs_path(None).unwrap_err().contains("name"));
}

/// Never option-shaped, even though every path either follows a `--` or sits in a quoted shell
/// positional — the rule is cheap and the failure mode it forecloses is not.
#[test]
fn a_name_that_looks_like_an_option_is_refused() {
    assert!(resolve_plugin_file(DIR, Some("-rf")).is_err());
    assert!(resolve_plugin_file(DIR, Some("--version")).is_err());
}

#[test]
fn a_name_with_anything_exotic_in_it_is_refused() {
    for named in [
        "keys plugin",
        "keys;rm -rf /",
        "keys$(whoami)",
        "keys\\x",
        "kéys",
    ] {
        assert!(
            resolve_plugin_file(DIR, Some(named)).is_err(),
            "{named:?} should not be accepted as a plugin name"
        );
    }
    assert!(resolve_plugin_file(DIR, Some(&"k".repeat(200))).is_err());
}

#[test]
fn a_missing_name_or_folder_says_which_is_missing() {
    assert!(resolve_plugin_file(DIR, None).unwrap_err().contains("name"));
    assert!(resolve_plugin_file(DIR, Some("   "))
        .unwrap_err()
        .contains("name"));
    // The case the whole feature turns on: nothing to write into, said in terms the user can
    // act on rather than as a path that failed to parse.
    let no_folder = resolve_plugin_file("", Some("keys")).unwrap_err();
    assert!(no_folder.contains("user_plugin_dir"), "{no_folder}");
}

/// What the model reads from `plugin_log`: the host's lines as they are, a pointer at
/// `zellij.log` when there are none yet, and the host's own reason when it could not look.
#[test]
fn the_log_tool_passes_lines_on_and_explains_an_empty_log() {
    assert_eq!(
        plugin_log_answer("keys", Ok("INFO [lua:keys.lua] pressed t\n".to_owned())),
        "INFO [lua:keys.lua] pressed t"
    );
    let empty = plugin_log_answer("keys.lua", Ok(String::new()));
    assert!(
        empty.starts_with("no log lines from keys.lua yet") && empty.contains("zellij.log"),
        "{empty}"
    );
    assert_eq!(
        plugin_log_answer("keys", Err("no zellij log at /tmp/zellij.log".to_owned())),
        "no zellij log at /tmp/zellij.log"
    );
}

/// What the model reads after a reload. A pane that is not there is not a success; a pane
/// showing the loader's error text is named as a failure up front.
#[test]
fn a_load_report_names_the_outcome_before_the_pane_text() {
    let path = format!("{DIR}/keys.lua");
    let missing = plugin_load_report(&path, None);
    assert!(missing.contains("no pane is running it yet"), "{missing}");
    assert!(!missing.contains("loaded in"), "{missing}");

    let fine = plugin_load_report(
        &path,
        Some(PluginObservation {
            pane_id: 7,
            contents: "hello - 0 keys\n\n".to_owned(),
        }),
    );
    assert!(
        fine.starts_with(&format!("{path} loaded in plugin pane 7")),
        "{fine}"
    );
    assert!(fine.ends_with("hello - 0 keys"), "{fine}");
    // A frame is the weakest evidence there is, and the report says so - but only when there is
    // a plugin to drive, never over the top of a load error.
    assert!(fine.contains("only proves it loads"), "{fine}");

    for error in [
        "failed to load the Lua plugin at '/x/keys.lua'. A plugin must return a table",
        "Panic!\nfailed to run the plugin's `update`: keys.lua:12: attempt to index nil",
    ] {
        let failed = plugin_load_report(
            &path,
            Some(PluginObservation {
                pane_id: 7,
                contents: error.to_owned(),
            }),
        );
        assert!(failed.contains("FAILED to load"), "{failed}");
        assert!(!failed.contains("only proves it loads"), "{failed}");
        assert!(failed.contains(error.lines().last().unwrap()), "{failed}");
    }
}

/// A successful reload's pane render is a screenshot, and it is re-sent on every later round
/// of the conversation. Three reloads used to put 30k chars of repeated pane drawing in the
/// transcript permanently; a screenful is what identifies the plugin, and `read_pane` fetches
/// the rest on the rare occasion it matters.
#[test]
fn a_successful_reload_report_keeps_only_a_screenful() {
    let report = plugin_load_report(
        &format!("{DIR}/dash.lua"),
        Some(PluginObservation {
            pane_id: 7,
            contents: "row of a very wide dashboard\n".repeat(1_000),
        }),
    );

    assert!(report.contains("loaded in plugin pane 7"), "{report}");
    assert!(report.len() < 2_200, "the report is {} chars", report.len());
    assert!(report.contains("pane truncated"), "{report}");
}

/// A load error is the only thing in this report the model can act on, and a traceback through
/// a few required modules is not short. It keeps its room.
#[test]
fn a_failed_reload_keeps_its_error_text() {
    let traceback = format!(
        "failed to load the Lua plugin at '/x/keys.lua'\n{}",
        "  stack traceback line\n".repeat(300)
    );

    let report = plugin_load_report(
        &format!("{DIR}/keys.lua"),
        Some(PluginObservation {
            pane_id: 7,
            contents: traceback.clone(),
        }),
    );

    assert!(report.contains("FAILED to load"), "{report}");
    assert!(
        report.len() > 6_000,
        "the traceback survives: {} chars",
        report.len()
    );
}

/// The verdict is read from the whole pane, not from the truncated copy. This used to work by
/// luck - the markers land near the top of a real failure - and the success cap is now small
/// enough that the luck would have run out, turning a failed load into a reported success.
#[test]
fn the_verdict_is_read_from_the_whole_pane_not_the_truncated_one() {
    let late = format!(
        "{}failed to run the plugin's `update`: keys.lua:12: attempt to index nil",
        "harmless earlier output\n".repeat(400)
    );

    let report = plugin_load_report(
        &format!("{DIR}/keys.lua"),
        Some(PluginObservation {
            pane_id: 7,
            contents: late,
        }),
    );

    assert!(report.contains("FAILED to load"), "{report}");
    assert!(!report.contains("only proves it loads"), "{report}");
}

#[test]
fn a_source_ends_in_exactly_one_newline_however_it_arrived() {
    for sent in [
        r#""local M = {}""#,
        r#""local M = {}\n""#,
        r#""local M = {}\n\n\n""#,
    ] {
        let source =
            plugin_source(&args(&format!(r#"{{"name":"keys","source":{sent}}}"#))).unwrap();
        assert_eq!(source, "local M = {}\n");
    }
}

/// There is no size cap any more: the source no longer travels as one environment string.
#[test]
fn a_write_with_no_source_is_refused_before_anything_is_touched() {
    assert!(plugin_source(&args(r#"{"name":"keys"}"#))
        .unwrap_err()
        .contains("source"));
    assert!(plugin_source(&args(r#"{"name":"keys","source":"   "}"#))
        .unwrap_err()
        .contains("source"));
    let huge = "x".repeat(200_000);
    assert!(plugin_source(&args(
        &serde_json::json!({ "name": "keys", "source": huge }).to_string()
    ))
    .is_ok());
}

/// A read is a page of the file, as `read_file` is: the `sed -n 'a,bp'` shape without the sed.
#[test]
fn a_read_is_a_page_of_lines() {
    assert_eq!(
        read_range(&args(r#"{"name":"keys"}"#)),
        (1, FILE_TOOL_LINES)
    );
    assert_eq!(
        read_range(&args(r#"{"name":"keys","start_line":10,"line_count":5}"#)),
        (10, 14)
    );
    assert_eq!(read_range(&args(r#"{"name":"keys","start_line":0}"#)).0, 1);

    let file = "one\ntwo\nthree\nfour\n";
    assert_eq!(lines_in_range(file, 1, 2), "one\ntwo");
    assert_eq!(lines_in_range(file, 3, 300), "three\nfour");
    assert_eq!(lines_in_range(file, 10, 14), "", "past the end is no lines");
    assert_eq!(lines_in_range(file, 2, 2), "two");
}

/// A listing answers in names, because a name is what every plugin tool takes back — handing
/// over paths would invite the model to send one and be refused. One folder deep, for modules,
/// and only Lua files.
#[test]
fn a_listing_is_names_one_folder_deep() {
    let entry = |path: &str, is_dir: bool| FsEntry {
        path: path.to_owned(),
        metadata: Some(FileMetadata {
            is_dir,
            is_file: !is_dir,
            ..Default::default()
        }),
    };
    let entries = vec![
        entry("/plugins/probe.lua", false),
        entry("/plugins/notes.md", false),
        entry("/plugins/app", true),
        entry("/plugins/app/ui.lua", false),
        FsEntry {
            path: "/plugins/keys.lua".to_owned(),
            metadata: None,
        },
    ];
    assert_eq!(
        listing_names("/plugins", &entries),
        vec!["app/ui.lua", "keys.lua", "probe.lua"]
    );
    assert!(listing_names("/plugins", &[]).is_empty());
}

/// A write says what happened next: the reload to call, and the permissions it granted, so the
/// model does not go looking for a prompt that will never come.
#[test]
fn a_write_report_names_the_next_step_and_what_it_granted() {
    let plain = write_report("probe", &[], &Kept::Nothing);
    assert!(plain.contains("plugin_reload"), "{plain}");
    assert!(!plain.contains("granted"), "{plain}");
    assert!(!plain.contains("backup"), "{plain}");
    let granted = write_report("probe.lua", &[PermissionType::ReadFiles], &Kept::Nothing);
    assert!(granted.contains("wrote probe.lua"), "{granted}");
    assert!(granted.contains("granted it read_files"), "{granted}");
    assert!(exists_report("probe").contains("overwrite=true"));
    // A rewrite that lost something is recoverable, and only a report that names the copy
    // makes that true for the model rather than only for the disk.
    let replaced = write_report("probe", &[], &Kept::At(backup_name("probe")));
    assert!(replaced.contains("probe.bak"), "{replaced}");
    let edited = edit_report("probe", &[], &Kept::At(backup_name("probe")));
    assert!(edited.contains("edited probe.lua"), "{edited}");
    assert!(edited.contains("probe.bak"), "{edited}");
    // And a backup that could not be written is said out loud: everything else in this report
    // tells the model the old version is one read away.
    let unkept = edit_report("probe", &[], &Kept::Failed);
    assert!(unkept.contains("no backup to read"), "{unkept}");
}

/// The backup is one version deep, and the version worth having is the last one that differed:
/// a rewrite of the file with what it already says must leave the copy alone.
#[test]
fn a_write_that_changes_nothing_keeps_nothing() {
    let same = "return {}\n";
    assert_eq!(keep_replaced("probe", Some(same), same), Kept::Nothing);
    assert_eq!(keep_replaced("probe", None, same), Kept::Nothing);
    assert_eq!(keep_replaced("probe", Some("   "), same), Kept::Nothing);
    // Off wasm there is no plugin folder to write into, which is the failure this reports.
    assert_eq!(
        keep_replaced("probe", Some("return { old = true }\n"), same),
        Kept::Failed
    );
}

/// An edit is the alternative to re-sending a file from memory, so it has to be exact: the two
/// ways it can be ambiguous are refusals, not guesses.
#[test]
fn an_edit_applies_once_or_says_why_it_cannot() {
    let source = "local M = {}\nM.tick = 1\nreturn M\n";
    assert_eq!(
        apply_edit(source, "M.tick = 1", "M.tick = 5", false).unwrap(),
        "local M = {}\nM.tick = 5\nreturn M\n"
    );
    let missing = apply_edit(source, "M.tock = 1", "x", false).unwrap_err();
    assert!(missing.contains("plugin_read"), "{missing}");
    let twice = "a\na\n";
    let ambiguous = apply_edit(twice, "a", "b", false).unwrap_err();
    assert!(ambiguous.contains("appears 2 times"), "{ambiguous}");
    assert_eq!(apply_edit(twice, "a", "b", true).unwrap(), "b\nb\n");
    assert!(apply_edit(source, "", "x", false).is_err());
    assert!(apply_edit(source, "M.tick = 1", "M.tick = 1", false).is_err());
}

/// The findings ride under the verdict, never over it, and a source with nothing wrong with it
/// adds nothing to the answer.
#[test]
fn the_lint_qualifies_a_report_without_displacing_it() {
    let clean = with_lint("probe.lua compiles".to_owned(), "probe", "return {}\n");
    assert_eq!(clean, "probe.lua compiles");
    let trap = r#"
zellij.request_permission { "plugin_data" }
zellij.on("permission_request_result", function()
  zellij.data.define { name = "todo", version = 1, namespace = "todo", attrs = { { "title", "text" } } }
end)
"#;
    let warned = with_lint("probe.lua compiles".to_owned(), "probe", trap);
    assert!(warned.starts_with("probe.lua compiles"), "{warned}");
    assert!(warned.contains("shared:"), "{warned}");
    assert!(warned.contains("not blocked"), "{warned}");
}

/// `summarize` is what the transcript shows, and a whole Lua file on that line would bury the
/// conversation it is part of.
#[test]
fn a_write_is_summarized_without_its_source() {
    let summary = summarize(&call(
        "plugin_write",
        r#"{"name":"keys","source":"line one\nline two\nline three"}"#,
    ));
    assert_eq!(summary, "plugin_write keys (3 lines)");
    assert!(!summary.contains("line one"));
}

/// The source is still shown where there is room for it: the user is agreeing to code they have
/// not otherwise read, so a prompt naming only the file would be worth nothing.
#[test]
fn a_write_confirmation_shows_the_source() {
    let detail = confirmation_detail(&call(
        "plugin_write",
        r#"{"name":"keys","source":"local M = {}\nreturn M"}"#,
    ))
    .join("\n");
    assert!(detail.contains("writes keys.lua"), "{detail}");
    assert!(detail.contains("local M = {}"), "{detail}");
    assert!(detail.contains("return M"), "{detail}");
}

#[test]
fn a_long_source_is_shown_in_full_in_the_confirmation() {
    let source: Vec<String> = (0..50).map(|n| format!("-- line {n}")).collect();
    let detail = confirmation_detail(&call(
        "plugin_write",
        &serde_json::json!({ "name": "keys", "source": source.join("\n") }).to_string(),
    ))
    .join("\n");
    assert!(detail.contains("-- line 0"), "{detail}");
    assert!(detail.contains("-- line 49"), "{detail}");
    assert!(!detail.contains("more lines"), "{detail}");
}

#[test]
fn a_long_source_is_cut_short_in_the_toast() {
    let source: Vec<String> = (0..50).map(|n| format!("-- line {n}")).collect();
    let toast = confirmation_toast(&call(
        "plugin_write",
        &serde_json::json!({ "name": "keys", "source": source.join("\n") }).to_string(),
    ));
    assert!(toast.contains("-- line 0"), "{toast}");
    assert!(toast.contains("30 more lines"), "{toast}");
    assert!(!toast.contains("-- line 49"), "{toast}");
}

#[test]
fn the_permissions_a_source_asks_for_are_read_off_it() {
    let source = r#"
        local zellij = require("zellij")
        function M.load()
          zellij.subscribe { "key" }
          zellij.request_permission {
            "read_files", "WriteFiles",
            'run_commands',   -- single quotes too
            "not_a_permission",
          }
          zellij.request_permission({ "read_files" })   -- a repeat, and the other call shape
        end
    "#;
    assert_eq!(
        requested_permissions(source),
        vec![
            PermissionType::ReadFiles,
            PermissionType::WriteFiles,
            PermissionType::RunCommands,
        ]
    );
    assert!(requested_permissions("local M = {}\nreturn M").is_empty());
    // A list built at runtime is not read: the plugin gets its prompt.
    assert!(requested_permissions("zellij.request_permission(perms)").is_empty());
    assert_eq!(
        permission_names(&[PermissionType::ReadFiles, PermissionType::FullHdAccess]),
        vec!["read_files", "full_hd_access"]
    );
}

/// The approval of a write is the approval of its permissions, so the sentence being agreed
/// to has to say what they are.
#[test]
fn a_write_confirmation_names_the_permissions_it_grants() {
    let with = consequence(&call(
        "plugin_write",
        r#"{"name":"keys","source":"zellij.request_permission { \"read_files\", \"run_commands\" }"}"#,
    ))
    .unwrap();
    assert!(
        with.ends_with("and grants it: read_files, run_commands"),
        "{with}"
    );
    let without = consequence(&call("plugin_write", r#"{"name":"keys","source":"x"}"#)).unwrap();
    assert!(!without.contains("grants"), "{without}");
}

/// An edit grants the set difference between the complete source before and after it. Looking
/// only at `new_string` mistakes an existing permission repeated there for a new grant.
#[test]
fn an_edit_grants_only_permissions_added_to_the_complete_source() {
    let args = Args::parse(
        r#"{"old_string":"\"read_files\"","new_string":"\"read_files\", \"write_files\""}"#,
    )
    .unwrap();
    let (_, granted) =
        edit_source_and_permissions(r#"zellij.request_permission { "read_files" }"#, &args)
            .unwrap();
    assert_eq!(
        granted,
        vec![PermissionType::WriteFiles],
        "read_files was already present"
    );

    let unchanged = Args::parse(r#"{"old_string":"old","new_string":"new"}"#).unwrap();
    assert!(edit_source_and_permissions(
        "zellij.request_permission { \"read_files\" }\n-- old",
        &unchanged,
    )
    .unwrap()
    .1
    .is_empty());

    let removed = Args::parse(r#"{"old_string":", \"write_files\"","new_string":""}"#).unwrap();
    assert!(edit_source_and_permissions(
        r#"zellij.request_permission { "read_files", "write_files" }"#,
        &removed,
    )
    .unwrap()
    .1
    .is_empty());
}

/// A toast is a glance. `source` was capped for that reason and an edit's two code arguments
/// are no different - together they can be longer than the file.
#[test]
fn a_long_edit_is_cut_short_in_the_toast() {
    let new_string: Vec<String> = (0..50).map(|n| format!("-- line {n}")).collect();
    let edit = call(
        "plugin_edit",
        &serde_json::json!({
            "name": "keys",
            "old_string": "-- todo",
            "new_string": new_string.join("\n"),
        })
        .to_string(),
    );
    let toast = confirmation_toast(&edit);
    assert!(toast.contains("-- line 0"), "{toast}");
    assert!(toast.contains("30 more lines"), "{toast}");
    assert!(!toast.contains("-- line 49"), "{toast}");
    // The confirmation itself still shows all of it: that is what is being agreed to.
    let detail = confirmation_detail(&edit).join("\n");
    assert!(detail.contains("-- line 49"), "{detail}");
}

#[test]
fn replacing_a_plugin_says_so_rather_than_saying_it_writes_one() {
    let replacing = consequence(&call(
        "plugin_write",
        r#"{"name":"keys","source":"x","overwrite":true}"#,
    ))
    .unwrap();
    assert!(replacing.contains("replaces keys.lua"), "{replacing}");

    // Malformed arguments must still produce a sentence: that is when the user most needs one.
    let malformed = consequence(&call("plugin_write", "not json at all")).unwrap();
    assert!(malformed.contains("plugin folder"), "{malformed}");
}

// -- reading a plugin ---------------------------------------------------

fn numbered_lines(count: usize) -> String {
    (1..=count)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The failure this exists to prevent: a model reads the default page of a longer plugin,
/// believes it holds the file, and rewrites it from what it has - dropping everything past
/// line 300. The answer has to say it is a fragment, and say where the rest starts.
#[test]
fn a_partial_read_says_how_much_is_left_and_how_to_get_it() {
    let file = numbered_lines(900);

    let answer = read_answer(&file, 1, 300);

    assert!(answer.starts_with("lines 1-300 of 900\n"), "{answer}");
    assert!(answer.contains("line 300"));
    assert!(!answer.contains("line 301"));
    assert!(
        answer.contains("… 600 more lines (plugin_read start_line=301)"),
        "{answer}"
    );
}

/// A whole file is a whole file. No footer, because there is nothing left to fetch and the
/// suggestion would send the model on a round trip for an empty page.
#[test]
fn a_whole_file_read_has_no_more_lines_marker() {
    let answer = read_answer(&numbered_lines(12), 1, 300);

    assert!(answer.starts_with("lines 1-12 of 12\n"), "{answer}");
    assert!(!answer.contains("more lines"), "{answer}");
}

/// The last page ends exactly where the file does.
#[test]
fn the_final_page_reports_the_real_last_line() {
    let answer = read_answer(&numbered_lines(350), 301, 600);

    assert!(answer.starts_with("lines 301-350 of 350\n"), "{answer}");
    assert!(!answer.contains("more lines"), "{answer}");
}

/// "No lines" alone reads as "this plugin is empty", which is a different and much more
/// alarming thing than "you asked for a range past the end".
#[test]
fn a_read_past_the_end_says_how_long_the_file_actually_is() {
    assert_eq!(
        read_answer(&numbered_lines(40), 900, 1200),
        "no lines there: the file has 40 lines"
    );
    assert_eq!(read_answer("", 1, 300), "the file is empty");
}

/// Line numbers are deliberately absent: `plugin_edit` matches `old_string` against the file
/// exactly, so text handed to the model with prefixes on it would never match.
#[test]
fn the_lines_are_returned_verbatim_so_an_edit_can_match_them() {
    let file = "local M = {}\nreturn M\n";

    let answer = read_answer(file, 1, 300);

    let body = answer.split_once('\n').expect("a header").1;
    assert_eq!(body, "local M = {}\nreturn M");
}

/// A failed edit must say the file is untouched: the compile gate runs before anything is
/// written, and the old wording - shared with the write path - left the model unable to tell
/// whether the broken version had landed. It must also ask for another edit rather than for
/// the whole file, which is how a missed edit used to turn into a full rewrite.
#[test]
fn a_failed_edit_says_the_file_is_unchanged_and_asks_for_another_edit() {
    let report = edit_compile_report("keys.lua", "unexpected symbol near '{'");

    assert!(report.contains("NOT applied"), "{report}");
    assert!(report.contains("unchanged"), "{report}");
    assert!(report.contains("plugin_edit again"), "{report}");
    assert!(
        !report.contains("whole file"),
        "an edit is never fixed by sending the whole file: {report}"
    );
}

/// The write-path refusal must not send an edit back to a whole-file rewrite either.
#[test]
fn no_plugin_refusal_asks_for_the_whole_file_again() {
    let missing = plugin_source(&args("{}")).expect_err("no source");
    assert!(missing.contains("plugin_edit"), "{missing}");
    assert!(!missing.contains("not a patch"), "{missing}");
}
