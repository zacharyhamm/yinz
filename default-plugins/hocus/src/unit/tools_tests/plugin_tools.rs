//! The plugin-authoring tools' pure halves: turning a name the model sent into a path - the real
//! one and the one under the host's `/plugins` root - and shaping what goes to and comes back from
//! the host's file commands.
//!
//! All of it runs before anything reaches the host, which is the point — a name that escapes the
//! plugin folder, or a write with nothing to write, is refused without a file being touched.

use super::*;
use crate::plugin_source::{
    apply_edits, listing, missing_report, outline_answer, search, span_list, EditPair, Edited,
    LineSpan, Listed, Search, MAX_CONTEXT, MAX_SEARCH_HITS, SEARCH_HITS,
};
use zellij_tile::prelude::{FileMetadata, FsEntry};

const DIR: &str = "/home/someone/.config/zellij/plugins";

/// A compile error names the file the loader would name, whatever spelling the write used.
#[test]
fn the_chunk_name_is_the_file_the_loader_would_name() {
    for (name, chunk) in [
        ("keys", "keys.ts"),
        ("keys.ts", "keys.ts"),
        (" keys ", "keys.ts"),
        ("app/ui", "ui.ts"),
        ("hocus/weather", "weather.ts"),
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
            r#"{"name":"keys","source":"export default {};"}"#
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
        compile_report("keys", "export default {};").unwrap(),
        "keys.ts compiles"
    );
}

fn args(arguments: &str) -> Args {
    Args::parse(arguments).unwrap()
}

#[test]
fn a_name_resolves_with_or_without_its_extension() {
    for named in ["keys", "keys.ts"] {
        assert_eq!(
            resolve_plugin_file(DIR, Some(named)).unwrap(),
            format!("{DIR}/keys.ts"),
            "'{named}' should land on the same file"
        );
    }
    // Trimmed, because a model that pads a value should not thereby name a different file.
    assert_eq!(
        resolve_plugin_file(DIR, Some("  keys  ")).unwrap(),
        format!("{DIR}/keys.ts")
    );
}

/// The path a reload is built from must be absolute, and this is the guard on that.
///
/// `zellij_runtime::plugin_location::parse_plugin_location` joins a *relative* `file:` path onto the cwd whenever one is
/// supplied, and `start_or_reload_plugin` always supplies one — so a relative URL would be
/// resolved against the session's cwd and never reach the plugin folder at all. It would fail
/// only at load time, in a pane, which is exactly where nobody is looking.
#[test]
fn a_resolved_plugin_path_is_always_absolute() {
    for named in ["keys", "keys.ts", "a-b_c.1"] {
        let path = resolve_plugin_file(DIR, Some(named)).unwrap();
        assert!(
            std::path::Path::new(&path).is_absolute(),
            "{named:?} resolved to the relative {path:?}"
        );
        assert!(path.starts_with(DIR), "{path:?} escaped {DIR}");
        assert!(path.ends_with(".ts"), "{path:?} is not a TypeScript file");
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
/// `<dir>/app/ui.ts`, imported with an explicit relative path.
#[test]
fn a_module_name_resolves_one_folder_down() {
    assert_eq!(
        resolve_plugin_file(DIR, Some("app/ui")).unwrap(),
        format!("{DIR}/app/ui.ts")
    );
    assert_eq!(
        resolve_plugin_file(DIR, Some("app/init.ts")).unwrap(),
        format!("{DIR}/app/init.ts")
    );
}

/// The host's file commands take the same name under `/plugins`: one set of rules, two roots,
/// so the file that is written and the file that is reloaded cannot be different files.
#[test]
fn a_plugin_fs_path_is_the_same_name_under_the_plugins_root() {
    assert_eq!(plugin_fs_path(Some("keys")).unwrap(), "/plugins/keys.ts");
    assert_eq!(
        plugin_fs_path(Some("app/ui")).unwrap(),
        "/plugins/app/ui.ts"
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
        plugin_log_answer("keys", Ok("INFO [deno:keys.ts] pressed t\n".to_owned())),
        "INFO [deno:keys.ts] pressed t"
    );
    let empty = plugin_log_answer("keys.ts", Ok(String::new()));
    assert!(
        empty.starts_with("no log lines from keys.ts yet") && empty.contains("zellij.log"),
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
    let path = format!("{DIR}/keys.ts");
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
        "failed to start the script plugin at '/x/main.ts': resolve module /x/missing.ts: No such file or directory (os error 2)",
        "failed to start the script plugin at '/x/main.ts': TypeError: plugin must default-export a lifecycle object",
        "failed to start the script plugin at '/x/keys.ts'. A plugin must export its lifecycle",
        "Panic!\nfailed to run the plugin's `update`: keys.ts:12: attempt to index nil",
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
        &format!("{DIR}/dash.ts"),
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
        "failed to start the script plugin at '/x/keys.ts'\n{}",
        "  stack traceback line\n".repeat(300)
    );

    let report = plugin_load_report(
        &format!("{DIR}/keys.ts"),
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
        "{}failed to run the plugin's `update`: keys.ts:12: attempt to index nil",
        "harmless earlier output\n".repeat(400)
    );

    let report = plugin_load_report(
        &format!("{DIR}/keys.ts"),
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
        r#""const M = {};""#,
        r#""const M = {};\n""#,
        r#""const M = {};\n\n\n""#,
    ] {
        let source =
            plugin_source(&args(&format!(r#"{{"name":"keys","source":{sent}}}"#))).unwrap();
        assert_eq!(source, "const M = {};\n");
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
/// and only TypeScript files, excluding backups.
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
        entry("/plugins/probe.ts", false),
        entry("/plugins/probe.bak.ts", false),
        entry("/plugins/notes.md", false),
        entry("/plugins/app", true),
        entry("/plugins/app/ui.ts", false),
        entry("/plugins/app/ui.bak.ts", false),
        FsEntry {
            path: "/plugins/keys.ts".to_owned(),
            metadata: None,
        },
    ];
    assert_eq!(
        listing_names("/plugins", &entries),
        vec!["app/ui.ts", "keys.ts", "probe.ts"]
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
    let granted = write_report("probe.ts", &[PermissionType::ReadFiles], &Kept::Nothing);
    assert!(granted.contains("wrote probe.ts"), "{granted}");
    assert!(granted.contains("granted it read_files"), "{granted}");
    assert!(exists_report("probe").contains("overwrite=true"));
    // A rewrite that lost something is recoverable, and only a report that names the copy
    // makes that true for the model rather than only for the disk.
    let replaced = write_report("probe", &[], &Kept::At(backup_name("probe")));
    assert!(replaced.contains("probe.bak"), "{replaced}");
    let changed = Edited {
        source: "export default {};\n".to_owned(),
        changed: vec![LineSpan { first: 1, last: 1 }],
    };
    let edited = edit_report("probe", &[], &Kept::At(backup_name("probe")), &changed);
    assert!(edited.contains("edited probe.ts"), "{edited}");
    assert!(edited.contains("probe.bak"), "{edited}");
    // And a backup that could not be written is said out loud: everything else in this report
    // tells the model the old version is one read away.
    let unkept = edit_report("probe", &[], &Kept::Failed, &changed);
    assert!(unkept.contains("no backup to read"), "{unkept}");
}

/// The backup is one version deep, and the version worth having is the last one that differed:
/// a rewrite of the file with what it already says must leave the copy alone.
#[test]
fn a_write_that_changes_nothing_keeps_nothing() {
    let same = "export default {};\n";
    assert_eq!(keep_replaced("probe", Some(same), same), Kept::Nothing);
    assert_eq!(keep_replaced("probe", None, same), Kept::Nothing);
    assert_eq!(keep_replaced("probe", Some("   "), same), Kept::Nothing);
    // Off wasm there is no plugin folder to write into, which is the failure this reports.
    assert_eq!(
        keep_replaced("probe", Some("return { old = true }\n"), same),
        Kept::Failed
    );
}

fn pair(old: &str, new: &str) -> EditPair {
    EditPair {
        old_string: old.to_owned(),
        new_string: new.to_owned(),
        replace_all: false,
    }
}

/// An edit is the alternative to re-sending a file from memory, so it has to be exact: the two
/// ways it can be ambiguous are refusals, not guesses - and the refusal for several says where
/// each one is, so the next old_string can carry the lines around the one that was meant.
#[test]
fn an_edit_applies_once_or_says_why_it_cannot() {
    let source = "const M = {};\nM.tick = 1\nexport default M;\n";
    let edited = apply_edits(source, &[pair("M.tick = 1", "M.tick = 5")]).unwrap();
    assert_eq!(
        edited.source,
        "const M = {};\nM.tick = 5\nexport default M;\n"
    );
    assert_eq!(edited.changed, vec![LineSpan { first: 2, last: 2 }]);
    let missing = apply_edits(source, &[pair("M.tock = 1", "x")]).unwrap_err();
    assert!(missing.contains("plugin_read"), "{missing}");
    let twice = "a\na\n";
    let ambiguous = apply_edits(twice, &[pair("a", "b")]).unwrap_err();
    assert!(
        ambiguous.contains("appears 2 times, at lines 1 and 2"),
        "{ambiguous}"
    );
    let all = apply_edits(
        twice,
        &[EditPair {
            replace_all: true,
            ..pair("a", "b")
        }],
    )
    .unwrap();
    assert_eq!(all.source, "b\nb\n");
    assert_eq!(all.changed, vec![LineSpan { first: 1, last: 2 }]);
    assert!(apply_edits(source, &[pair("", "x")]).is_err());
    assert!(apply_edits(source, &[pair("M.tick = 1", "M.tick = 1")]).is_err());
}

/// The findings ride under the verdict, never over it, and a source with nothing wrong with it
/// adds nothing to the answer.
#[test]
fn the_lint_qualifies_a_report_without_displacing_it() {
    let clean = with_lint(
        "probe.ts compiles".to_owned(),
        "probe",
        "export default {};\n",
    );
    assert_eq!(clean, "probe.ts compiles");
    let trap = r#"
import zellij from 'zellij:plugin';
zellij.on('PermissionRequestResult', () => {
  zellij.data.define('todo', {title:'text'}, {namespace:'todo'});
});
"#;
    let warned = with_lint("probe.ts compiles".to_owned(), "probe", trap);
    assert!(warned.starts_with("probe.ts compiles"), "{warned}");
    assert!(warned.contains("shared:"), "{warned}");
    assert!(warned.contains("not blocked"), "{warned}");
}

/// `summarize` is what the transcript shows, and a whole TypeScript file on that line would bury the
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
        r#"{"name":"keys","source":"const M = {};\nexport default M;"}"#,
    ))
    .join("\n");
    assert!(detail.contains("writes keys.ts"), "{detail}");
    assert!(detail.contains("const M = {};"), "{detail}");
    assert!(detail.contains("export default M;"), "{detail}");
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
import zellij from 'zellij:plugin';
export default {load() {
  zellij.requestPermission(['ReadFiles','WriteFiles','RunCommands','not_a_permission']);
  zellij.requestPermission(['ReadFiles']); // duplicate
}};
"#;
    assert_eq!(
        source_permissions("keys", source).unwrap(),
        vec![
            PermissionType::ReadFiles,
            PermissionType::WriteFiles,
            PermissionType::RunCommands
        ]
    );
    assert!(source_permissions("keys", "export default {};")
        .unwrap()
        .is_empty());
    assert!(
        source_permissions("keys", "zellij.requestPermission(perms);")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        permission_names(&[PermissionType::ReadFiles, PermissionType::FullHdAccess]),
        vec!["read_files", "full_hd_access"]
    );
}

#[test]
fn source_permission_cache_tracks_names_source_and_errors() {
    test_seams::reset();
    let read = "zellij.requestPermission(['ReadFiles']);";
    let write = "zellij.requestPermission(['WriteFiles']);";
    for _ in 0..3 {
        assert_eq!(
            source_permissions("keys", read).unwrap(),
            vec![PermissionType::ReadFiles]
        );
        assert_eq!(
            source_permissions("keys", write).unwrap(),
            vec![PermissionType::WriteFiles]
        );
    }
    assert_eq!(test_seams::PLUGIN_CHECK_CALLS.get(), 2);
    assert_eq!(
        source_permissions("other", write).unwrap(),
        vec![PermissionType::WriteFiles]
    );
    assert_eq!(test_seams::PLUGIN_CHECK_CALLS.get(), 3);
    // A third entry evicts the oldest source, keeping retained source memory bounded.
    assert_eq!(
        source_permissions("keys", read).unwrap(),
        vec![PermissionType::ReadFiles]
    );
    assert_eq!(test_seams::PLUGIN_CHECK_CALLS.get(), 4);

    let invalid = "export default {";
    let error = source_permissions("keys", invalid).unwrap_err();
    assert!(!error.is_empty());
    assert_eq!(source_permissions("keys", invalid).unwrap_err(), error);
    assert_eq!(test_seams::PLUGIN_CHECK_CALLS.get(), 5);
    // Failures to obtain a report are retried, rather than saved as empty permissions.
    for _ in 0..2 {
        assert!(source_permissions("../keys", read).is_err());
    }
    assert_eq!(test_seams::PLUGIN_CHECK_CALLS.get(), 7);
    test_seams::reset();
}

/// The approval of a write is the approval of its permissions, so the sentence being agreed
/// to has to say what they are.
#[test]
fn a_write_confirmation_names_the_permissions_it_grants() {
    let with = consequence(&call(
        "plugin_write",
        r#"{"name":"keys","source":"zellij.requestPermission([ \"ReadFiles\", \"RunCommands\" ]);"}"#,
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
        r#"{"name":"keys","old_string":"\"ReadFiles\"","new_string":"\"ReadFiles\", \"WriteFiles\""}"#,
    )
    .unwrap();
    let (_, granted) =
        edit_source_and_permissions(r#"zellij.requestPermission([ "ReadFiles" ]);"#, &args)
            .unwrap();
    assert_eq!(
        granted,
        vec![PermissionType::WriteFiles],
        "read_files was already present"
    );

    let unchanged =
        Args::parse(r#"{"name":"keys","old_string":"old","new_string":"new"}"#).unwrap();
    assert!(edit_source_and_permissions(
        "zellij.requestPermission([ \"ReadFiles\" ]);\n// old",
        &unchanged,
    )
    .unwrap()
    .1
    .is_empty());

    let removed =
        Args::parse(r#"{"name":"keys","old_string":", \"WriteFiles\"","new_string":""}"#).unwrap();
    assert!(edit_source_and_permissions(
        r#"zellij.requestPermission([ "ReadFiles", "WriteFiles" ]);"#,
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
    assert!(replacing.contains("replaces keys.ts"), "{replacing}");

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

    let answer = read_answer(&file, 1, 300, "plugin_read name=keys");

    assert!(answer.starts_with("lines 1-300 of 900\n"), "{answer}");
    assert!(answer.contains("line 300"));
    assert!(!answer.contains("line 301"));
    assert!(
        answer.contains("… 600 more lines (plugin_read name=keys start_line=301)"),
        "{answer}"
    );
}

/// A whole file is a whole file. No footer, because there is nothing left to fetch and the
/// suggestion would send the model on a round trip for an empty page.
#[test]
fn a_whole_file_read_has_no_more_lines_marker() {
    let answer = read_answer(&numbered_lines(12), 1, 300, "plugin_read name=keys");

    assert!(answer.starts_with("lines 1-12 of 12\n"), "{answer}");
    assert!(!answer.contains("more lines"), "{answer}");
}

/// The last page ends exactly where the file does.
#[test]
fn the_final_page_reports_the_real_last_line() {
    let answer = read_answer(&numbered_lines(350), 301, 600, "plugin_read name=keys");

    assert!(answer.starts_with("lines 301-350 of 350\n"), "{answer}");
    assert!(!answer.contains("more lines"), "{answer}");
}

/// "No lines" alone reads as "this plugin is empty", which is a different and much more
/// alarming thing than "you asked for a range past the end".
#[test]
fn a_read_past_the_end_says_how_long_the_file_actually_is() {
    assert_eq!(
        read_answer(&numbered_lines(40), 900, 1200, "plugin_read name=keys"),
        "no lines there: the file has 40 lines"
    );
    assert_eq!(
        read_answer("", 1, 300, "plugin_read name=keys"),
        "the file is empty"
    );
}

/// Line numbers are deliberately absent: `plugin_edit` matches `old_string` against the file
/// exactly, so text handed to the model with prefixes on it would never match.
#[test]
fn the_lines_are_returned_verbatim_so_an_edit_can_match_them() {
    let file = "const M = {};\nexport default M;\n";

    let answer = read_answer(file, 1, 300, "plugin_read name=keys");

    let body = answer.split_once('\n').expect("a header").1;
    assert_eq!(body, "const M = {};\nexport default M;");
}

/// A failed edit must say the file is untouched: the compile gate runs before anything is
/// written, and the old wording - shared with the write path - left the model unable to tell
/// whether the broken version had landed. It must also ask for another edit rather than for
/// the whole file, which is how a missed edit used to turn into a full rewrite.
#[test]
fn a_failed_edit_says_the_file_is_unchanged_and_asks_for_another_edit() {
    let report = edit_compile_report("keys.ts", "unexpected symbol near '{'");

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

// -- searching a plugin ---------------------------------------------------

fn search_with(query: &str, context: usize) -> Search<'_> {
    Search {
        query,
        case_insensitive: false,
        context,
        max_hits: SEARCH_HITS,
    }
}

/// The shape is ripgrep's, which the model already reads from `search_files`: a hit carries its
/// file and line, a context line the same with dashes, and groups that do not touch are kept
/// apart - so a hit can be told from its surroundings, and its text copied as it is.
#[test]
fn a_search_answers_in_the_shape_search_files_does() {
    let keys = "const M = {};\nM.load = function() {\n  zellij.subscribe([\"Key\"]);\n};\nexport default M;\n";
    let clock = "const M = {};\nM.render = function() {\n  return new Date().toISOString();\n};\nexport default M;\n";
    let files = [("keys.ts", keys), ("clock.ts", clock)];

    let hits = search(&files, &search_with("function", 0))
        .unwrap()
        .unwrap();
    assert_eq!(
        hits,
        "keys.ts:2:M.load = function() {\nclock.ts:2:M.render = function() {"
    );

    let with_context = search(&files, &search_with("function", 1))
        .unwrap()
        .unwrap();
    assert_eq!(
        with_context,
        "keys.ts-1-const M = {};\nkeys.ts:2:M.load = function() {\nkeys.ts-3-  zellij.subscribe([\"Key\"]);\n--\nclock.ts-1-const M = {};\nclock.ts:2:M.render = function() {\nclock.ts-3-  \
         return new Date().toISOString();"
    );
}

/// What follows the second separator is the line as it is in the file, indentation included:
/// that is what makes a hit something a `plugin_edit` can be built from without a read.
#[test]
fn a_search_hit_is_verbatim_after_the_second_separator() {
    let source = "M.load = function() {\n\t  zellij.subscribe([\"Key\"]);  \n};\n";
    let hits = search(&[("keys.ts", source)], &search_with("subscribe", 0))
        .unwrap()
        .unwrap();
    assert_eq!(hits, "keys.ts:2:\t  zellij.subscribe([\"Key\"]);  ");
}

/// Two hits whose context windows overlap or touch print once, as one group with no separator
/// between them - rg's rule, and the one that keeps a line from appearing twice.
#[test]
fn context_lines_never_repeat_a_hit_and_touching_groups_merge() {
    let source = "a\nb\nhit one\nc\nhit two\nd\ne\nf\nhit three\n";
    let hits = search(&[("x.ts", source)], &search_with("^hit", 1))
        .unwrap()
        .unwrap();
    assert_eq!(
        hits,
        "x.ts-2-b\nx.ts:3:hit one\nx.ts-4-c\nx.ts:5:hit two\nx.ts-6-d\n--\nx.ts-8-f\nx.ts:9:hit three"
    );
}

/// Every hit is re-sent on every later turn, so a search that matches half the folder is cut,
/// and the cut says how much it left out and how to see it.
#[test]
fn a_search_is_capped_and_says_how_many_it_left_out() {
    let source = "x\n".repeat(100);
    let hits = search(&[("x.ts", &source)], &search_with("x", 0))
        .unwrap()
        .unwrap();
    assert_eq!(
        hits.lines()
            .filter(|line| line.starts_with("x.ts:"))
            .count(),
        SEARCH_HITS
    );
    assert!(
        hits.ends_with(
            "… 40 of 100 matching lines shown; raise max_results, or narrow the query or name"
        ),
        "{hits}"
    );
}

/// Nothing is `None`, so the caller says what was searched: "no matches" alone reads as "no
/// plugins".
#[test]
fn a_search_with_no_hits_is_none_so_the_caller_can_say_what_it_searched() {
    assert_eq!(
        search(&[("x.ts", "export default {};\n")], &search_with("zzz", 0)).unwrap(),
        None
    );
    assert_eq!(no_matches(Some("keys"), 1), "no matches in keys.ts");
    assert_eq!(no_matches(None, 7), "no matches in 7 plugins");
}

/// A bad pattern is answered with the regex's own error, which names the problem, rather than
/// with "no matches", which would send the model looking for a plugin that is not there.
#[test]
fn an_invalid_regex_is_answered_with_the_regex_error() {
    let error = search(&[("x.ts", "export default {};\n")], &search_with("(", 0)).unwrap_err();
    assert!(error.contains("regex parse error"), "{error}");
}

#[test]
fn a_case_insensitive_search_finds_either_case() {
    let files = [("x.ts", "Render\nrender\n")];
    let mut options = search_with("render", 0);
    assert_eq!(search(&files, &options).unwrap().unwrap(), "x.ts:2:render");
    options.case_insensitive = true;
    assert_eq!(
        search(&files, &options).unwrap().unwrap(),
        "x.ts:1:Render\nx.ts:2:render"
    );
}

/// The options are clamped to what a tool answer should cost, and an empty query is refused
/// rather than matching every line of every plugin.
#[test]
fn search_options_clamp_context_and_max_results_and_refuse_an_empty_query() {
    let clamped = args(r#"{"query":" render ","context":99,"max_results":9999}"#);
    let options = search_options(&clamped).unwrap();
    assert_eq!(options.query, "render");
    assert_eq!(options.context, MAX_CONTEXT);
    assert_eq!(options.max_hits, MAX_SEARCH_HITS);
    let plain = args(r#"{"query":"x"}"#);
    let defaults = search_options(&plain).unwrap();
    assert_eq!(
        (
            defaults.context,
            defaults.max_hits,
            defaults.case_insensitive
        ),
        (0, SEARCH_HITS, false)
    );
    assert!(search_options(&args(r#"{"query":"  "}"#)).is_err());
    assert!(search_options(&args("{}")).is_err());
}

// -- the outline -----------------------------------------------------------

/// The outline is the cheap first read: one line per thing defined, with its line number, so
/// the page that follows can be a range. Numbers align so the column reads.
#[test]
fn an_outline_answer_numbers_every_entry_and_states_the_length() {
    let source = "import zellij from 'zellij:plugin';\nexport function render() {return 'hi';}\nzellij.requestPermission(['ReadFiles']);\n";
    let answer = outline_answer("keys", source);
    assert!(answer.starts_with("keys.ts: 3 lines\n"), "{answer}");
    assert!(
        answer.contains("2: function render") && answer.contains("3: permissions: ReadFiles"),
        "{answer}"
    );
}

#[test]
fn an_outline_of_a_plugin_with_nothing_in_it_still_states_its_length() {
    let answer = outline_answer("probe.ts", "// just a note\nexport default {};\n");
    assert!(
        answer.starts_with("probe.ts: 2 lines; nothing to outline"),
        "{answer}"
    );
    assert_eq!(outline_answer("probe", ""), "the file is empty");
}

// -- listing ---------------------------------------------------------------

/// A listing says how long each plugin is, so the model can tell a 40-line one from a 900-line
/// one before deciding how to read it, and names the backup where there is one.
#[test]
fn a_listing_states_each_plugins_length_and_whether_a_backup_exists() {
    let listed = [
        Listed {
            name: "keys.ts",
            lines: Ok(412),
            backup: true,
        },
        Listed {
            name: "app/ui.ts",
            lines: Ok(1),
            backup: false,
        },
        Listed {
            name: "broken.ts",
            lines: Err("too big".to_owned()),
            backup: false,
        },
    ];
    assert_eq!(
        listing(&listed),
        "keys.ts  412 lines  (keys.bak.ts kept from an earlier edit)\napp/ui.ts  1 line\n\
         broken.ts  unreadable: too big"
    );
    assert_eq!(listing(&[]), "no plugins yet");
}

#[test]
fn backed_up_names_reads_the_bak_suffix_back_to_the_plugin() {
    let entry = |path: &str| FsEntry {
        path: path.to_owned(),
        metadata: None,
    };
    let entries = [
        entry("/plugins/keys.bak.ts"),
        entry("/plugins/keys.ts"),
        entry("/plugins/app/ui.bak.ts"),
    ];
    assert_eq!(
        backed_up_names("/plugins", &entries),
        vec!["keys.ts", "app/ui.ts"]
    );
}

// -- where an edit landed, and why one missed ----------------------------------

/// The answer says where the file changed and how long it is now, which is what lets the next
/// call be a range read - or the next edit - rather than a page.
#[test]
fn an_edit_report_says_where_the_file_changed() {
    let edited = Edited {
        source: "a\n".repeat(12),
        changed: vec![LineSpan { first: 2, last: 4 }],
    };
    assert_eq!(
        edit_report("keys", &[], &Kept::Nothing, &edited),
        "edited keys.ts at lines 2-4 (now 12 lines); call plugin_reload to load it"
    );
    let two = Edited {
        source: "a\n".repeat(12),
        changed: vec![
            LineSpan { first: 2, last: 2 },
            LineSpan { first: 9, last: 10 },
        ],
    };
    let report = edit_report("keys", &[PermissionType::ReadFiles], &Kept::Nothing, &two);
    assert!(
        report.starts_with(
            "edited keys.ts at line 2, lines 9-10 (now 12 lines) and granted it read_files;"
        ),
        "{report}"
    );
}

/// A deletion has no text to point at, so it is named by the line it was made on; past five
/// spans the rest are counted, since the report rides on every later turn.
#[test]
fn span_list_names_a_deletion_by_its_line_and_caps_at_five() {
    let deleted = apply_edits("a\nb\nc\n", &[pair("b\n", "")]).unwrap();
    assert_eq!(deleted.source, "a\nc\n");
    assert_eq!(deleted.changed, vec![LineSpan { first: 2, last: 2 }]);
    let many: Vec<LineSpan> = (1..=7)
        .map(|n| LineSpan {
            first: n * 3,
            last: n * 3,
        })
        .collect();
    assert_eq!(
        span_list(&many),
        "line 3, line 6, line 9, line 12, line 15, … and 2 more"
    );
}

/// The commonest miss is whitespace: tabs against spaces, or a trailing space the model did not
/// see. Saying where the near-match is and quoting it verbatim saves the read that used to be
/// the only recovery.
#[test]
fn a_missing_old_string_that_differs_only_in_whitespace_is_located_and_quoted() {
    let source =
        "const M = {};\nM.load = function() {\n\tzellij.subscribe([\"Key\"]);\n\treturn true\n};\n";
    let report = missing_report(
        source,
        "M.load = function() {\n  zellij.subscribe([\"Key\"]);\n  return true",
    );
    assert!(
        report.starts_with(
            "old_string is not in the file exactly, but lines 2-4 match it apart from whitespace."
        ),
        "{report}"
    );
    assert!(
        report.ends_with("M.load = function() {\n\tzellij.subscribe([\"Key\"]);\n\treturn true"),
        "{report}"
    );
    assert!(!report.contains("plugin_read"), "{report}");
    // A one-line old_string is found anywhere in its line.
    let one = missing_report(source, "subscribe([\"Key\"]);  ");
    assert!(
        one.starts_with(
            "old_string is not in the file exactly, but line 3 matches it apart from whitespace."
        ),
        "{one}"
    );
}

/// When the first line is right and the rest is not, the miss is anchored there and the file's
/// own text from that point is quoted, so the next old_string can be copied rather than guessed.
#[test]
fn a_missing_old_string_whose_first_line_exists_is_anchored_there() {
    let source = "const M = {};\nM.load = function() {\n  zellij.subscribe([\"Key\"]);\n};\nexport default M;\n";
    let report = missing_report(
        source,
        "M.load = function() {\n  zellij.subscribe([\"Timer\"]);",
    );
    assert_eq!(
        report,
        "old_string is not in the file. Its first line is at line 2; the file reads from line \
         2:\nM.load = function() {\n  zellij.subscribe([\"Key\"]);\n};\nexport default M;"
    );
}

#[test]
fn a_missing_old_string_with_no_anchor_falls_back_to_the_read_advice() {
    let report = missing_report(
        "const M = {};\nexport default M;\n",
        "nothing like this\nat all",
    );
    assert!(report.contains("plugin_read"), "{report}");
}

// -- batches -----------------------------------------------------------------

/// Edits apply in order, each against the text the one before it produced: the second may name
/// what the first wrote.
#[test]
fn edits_apply_in_order_against_the_text_the_previous_one_produced() {
    let edited = apply_edits("a\n", &[pair("a", "b"), pair("b", "c")]).unwrap();
    assert_eq!(edited.source, "c\n");
}

/// The spans are of the edited text, not the original: an edit that adds lines moves everything
/// after it, and a report in the old numbering would send a range read to the wrong place.
#[test]
fn a_batch_reports_spans_against_the_final_text() {
    let source: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    let edited = apply_edits(
        &source,
        &[
            pair("line 8\n", "line eight\n"),
            pair("line 2\n", "two a\ntwo b\ntwo c\n"),
        ],
    )
    .unwrap();
    assert_eq!(
        edited.changed,
        vec![
            LineSpan { first: 2, last: 4 },
            LineSpan {
                first: 10,
                last: 10
            },
        ]
    );
    assert_eq!(edited.source.lines().nth(9), Some("line eight"));
}

/// One miss refuses the whole batch and nothing is written; the refusal names which edit, with
/// the same diagnosis a single edit gets, and says the file is unchanged.
#[test]
fn a_failing_edit_in_a_batch_refuses_the_whole_batch_and_names_it() {
    let error = apply_edits(
        "a\nb\n",
        &[pair("a", "x"), pair("zzz", "y"), pair("b", "z")],
    )
    .unwrap_err();
    assert!(
        error.starts_with("edit 2 of 3: old_string is not in the file"),
        "{error}"
    );
    assert!(
        error.ends_with("Nothing was written; the file is unchanged."),
        "{error}"
    );
}

/// The two forms are one code path, and exactly one of them per call.
#[test]
fn the_single_form_is_a_one_element_batch_and_the_forms_do_not_mix() {
    let single = args(r#"{"name":"keys","old_string":"a","new_string":"b","replace_all":true}"#)
        .edit_batch()
        .unwrap();
    assert_eq!(
        single,
        vec![EditPair {
            replace_all: true,
            ..pair("a", "b")
        }]
    );
    let batch = args(
        r#"{"name":"keys","edits":[{"old_string":"a","new_string":"b"},{"old_string":"c","new_string":"d","replace_all":true}]}"#,
    )
    .edit_batch()
    .unwrap();
    assert_eq!(batch.len(), 2);
    assert!(batch[1].replace_all);
    let mixed = args(r#"{"name":"keys","old_string":"a","new_string":"b","edits":[]}"#)
        .edit_batch()
        .unwrap_err();
    assert!(mixed.contains("not both"), "{mixed}");
    let empty = args(r#"{"name":"keys","edits":[]}"#)
        .edit_batch()
        .unwrap_err();
    assert!(empty.contains("edits is empty"), "{empty}");
    let neither = args(r#"{"name":"keys"}"#).edit_batch().unwrap_err();
    assert!(neither.contains("old_string is required"), "{neither}");
}

/// The transcript line counts the batch, not its code.
#[test]
fn a_batch_is_summarized_by_its_count() {
    let summary = summarize(&call(
        "plugin_edit",
        r#"{"name":"keys","edits":[{"old_string":"a\nb","new_string":"c"},{"old_string":"d","new_string":"e\nf\ng"}]}"#,
    ));
    assert_eq!(summary, "plugin_edit keys (2 edits, 3 lines -> 4)");
    let single = summarize(&call(
        "plugin_edit",
        r#"{"name":"keys","old_string":"a","new_string":"b\nc"}"#,
    ));
    assert_eq!(single, "plugin_edit keys (1 lines -> 2)");
}

/// A batch is agreed to pair by pair: each old and new string is a block of its own under the
/// edit's number, in the prompt in full and in the toast cut short - after a few edits as well
/// as within each one.
#[test]
fn a_batch_edit_renders_each_pair_as_its_own_block() {
    let long: Vec<String> = (0..30).map(|n| format!("-- line {n}")).collect();
    let edits: Vec<serde_json::Value> = (1..=5)
        .map(|n| {
            serde_json::json!({
                "old_string": format!("old {n}"),
                "new_string": long.join("\n"),
                "replace_all": n == 2,
            })
        })
        .collect();
    let edit = call(
        "plugin_edit",
        &serde_json::json!({ "name": "keys", "edits": edits }).to_string(),
    );
    let detail = confirmation_detail(&edit).join("\n");
    assert!(
        detail.contains("changes keys.ts in your Zellij plugin folder in 5 places"),
        "{detail}"
    );
    assert!(
        detail.contains("edit 1 old_string:\n    old 1\n  edit 1 new_string:\n    -- line 0"),
        "{detail}"
    );
    assert!(detail.contains("edit 2 replace_all: true"), "{detail}");
    assert!(!detail.contains("edit 1 replace_all"), "{detail}");
    assert!(detail.contains("edit 5 old_string:"), "{detail}");
    assert!(!detail.contains("more edits"), "{detail}");
    let toast = confirmation_toast(&edit);
    assert!(toast.contains("edit 3 old_string:"), "{toast}");
    assert!(!toast.contains("edit 4 old_string:"), "{toast}");
    assert!(toast.contains("… 2 more edits"), "{toast}");
    assert!(toast.contains("10 more lines"), "{toast}");
}

#[test]
fn edit_strings_are_required_but_explicit_deletions_are_valid() {
    for (arguments, missing) in [
        (r#"{"name":"keys","old_string":"a"}"#, "new_string"),
        (r#"{"new_string":""}"#, "old_string"),
    ] {
        assert!(args(arguments).edit_batch().unwrap_err().contains(missing));
        let batch = format!(r#"{{"edits":[{arguments}]}}"#);
        assert!(Args::parse(&batch).unwrap_err().contains(missing));
    }
    for arguments in [
        r#"{"name":"keys","old_string":"a","new_string":""}"#,
        r#"{"edits":[{"old_string":"a","new_string":""}]}"#,
    ] {
        let edits = args(arguments).edit_batch().unwrap();
        assert!(!edits[0].replace_all);
        assert_eq!(apply_edits("ab", &edits).unwrap().source, "b");
    }
    for arguments in [
        r#"{"name":"keys","old_string":"a","edits":[{"old_string":"a","new_string":""}]}"#,
        r#"{"new_string":"","edits":[{"old_string":"a","new_string":""}]}"#,
    ] {
        assert!(args(arguments)
            .edit_batch()
            .unwrap_err()
            .contains("not both"));
    }
}

#[test]
fn later_replacements_remap_changed_span_endpoints() {
    for (source, edits, expected, changed) in [
        // A later replacement contains the entire earlier change.
        (
            "a\nb\nc\nd\ne\nf\n",
            vec![pair("c", "C"), pair("a\nb\nC\nd\ne", "x")],
            "x\nf\n",
            vec![LineSpan { first: 1, last: 1 }],
        ),
        // Only the start or end of an earlier span lies inside the next replacement.
        (
            "abcdef",
            vec![pair("cd", "C\nD"), pair("abC", "x")],
            "x\nDef",
            vec![LineSpan { first: 1, last: 2 }],
        ),
        (
            "abcdef",
            vec![pair("cd", "C\nD"), pair("Def", "x")],
            "abC\nx",
            vec![LineSpan { first: 1, last: 2 }],
        ),
        // A deletion collapses the earlier span to its deletion point.
        (
            "a\nb\nc\n",
            vec![pair("b", "B"), pair("a\nB\n", "")],
            "c\n",
            vec![LineSpan { first: 1, last: 1 }],
        ),
        // Every preceding match moves the earlier change, in either direction.
        (
            "a\na\nz\n",
            vec![
                pair("z", "Z"),
                EditPair {
                    replace_all: true,
                    ..pair("a\n", "a\nb\n")
                },
            ],
            "a\nb\na\nb\nZ\n",
            vec![LineSpan { first: 1, last: 5 }],
        ),
        (
            "a\na\nz\n",
            vec![
                pair("z", "Z"),
                EditPair {
                    replace_all: true,
                    ..pair("a\n", "")
                },
            ],
            "Z\n",
            vec![LineSpan { first: 1, last: 1 }],
        ),
    ] {
        let edited = apply_edits(source, &edits).unwrap();
        assert_eq!(edited.source, expected);
        assert_eq!(edited.changed, changed, "{edits:?}");
    }
}

#[test]
fn script_load_diagnostics_beyond_the_success_allowance_survive() {
    for diagnostic in [
        "resolve module /x/missing.ts: No such file or directory (os error 2)",
        "TypeError: plugin must default-export a lifecycle object",
    ] {
        let contents = format!(
            "{}failed to start the script plugin at '/x/main.ts': {diagnostic}",
            "earlier output\n".repeat(200)
        );
        let report = plugin_load_report(
            "/x/main.ts",
            Some(PluginObservation {
                pane_id: 7,
                contents,
            }),
        );
        assert!(report.contains("FAILED to load"), "{report}");
        assert!(!report.contains("only proves it loads"), "{report}");
        assert!(report.contains(diagnostic), "{report}");
    }
}
