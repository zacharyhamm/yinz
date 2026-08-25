//! The plugin-authoring tools' two pure halves: turning a name the model sent into a path, and
//! turning that path into an argv.
//!
//! Both run before anything reaches the host, which is the point — a name that escapes the
//! plugin folder, or a source too large to pass, is refused without a command being spawned.

use super::*;

const DIR: &str = "/home/someone/.config/zellij/plugins";

fn plugin_argv_for(
    name: &str,
    arguments: &str,
) -> Result<(Vec<String>, BTreeMap<String, String>), String> {
    let args = Args::parse(arguments).unwrap();
    let target = resolve_plugin_file(DIR, args.name.as_deref())?;
    plugin_argv(name, &args, DIR, &target)
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
    for named in ["../keys", "a/b", "/etc/passwd", "..", ".", "sub/../keys"] {
        assert!(
            resolve_plugin_file(DIR, Some(named)).is_err(),
            "{named:?} should not be accepted as a plugin name"
        );
    }
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

#[test]
fn listing_asks_find_for_the_lua_files_one_level_down() {
    let (argv, env) = plugin_argv("plugin_list", &Args::default(), DIR, DIR).unwrap();
    assert_eq!(
        argv,
        vec![
            "find",
            DIR,
            "-maxdepth",
            "1",
            "-type",
            "f",
            "-name",
            "*.lua"
        ]
    );
    assert!(env.is_empty());
}

#[test]
fn reading_a_plugin_uses_the_read_file_shape() {
    let (argv, env) = plugin_argv_for("plugin_read", r#"{"name":"keys"}"#).unwrap();
    assert_eq!(argv[0], "sh");
    assert_eq!(argv[1], "-c");
    assert!(
        argv[2].contains(r#"exec sed -n "$2" -- "$1""#),
        "{}",
        argv[2]
    );
    assert_eq!(argv[3], "sh", "$0, so the file is $1 and the range $2");
    assert_eq!(argv[4], format!("{DIR}/keys.lua"));
    assert_eq!(argv[5], format!("1,{}p", FILE_TOOL_LINES));
    assert!(env.is_empty());

    let (argv, _) = plugin_argv_for(
        "plugin_read",
        r#"{"name":"keys","start_line":10,"line_count":5}"#,
    )
    .unwrap();
    assert_eq!(argv[5], "10,14p");
}

/// A reload runs no reload: the command asks the host the one question a wasm plugin cannot
/// answer, and `plugin_tool_result` starts the plugin when it comes back clean.
#[test]
fn a_reload_only_asks_whether_the_file_may_be_loaded() {
    let (argv, env) = plugin_argv_for("plugin_reload", r#"{"name":"keys"}"#).unwrap();
    assert_eq!(argv[0], "sh");
    assert_eq!(argv[1], "-c");
    assert!(argv[2].contains("-L"), "{}", argv[2]);
    assert!(
        argv[2].contains("[ ! -f \"$1\" ]"),
        "a name that was never written should be refused here: {}",
        argv[2]
    );
    assert!(
        !argv[2].contains("sed") && !argv[2].contains("printf %s"),
        "nothing is read or written by a reload: {}",
        argv[2]
    );
    assert_eq!(argv[4], format!("{DIR}/keys.lua"));
    assert!(env.is_empty());
}

/// The containment `resolve_plugin_file` can only assert lexically, enforced where it can
/// actually be checked: a symlink in the folder passes every rule about the *name*.
#[test]
fn every_tool_that_opens_the_path_refuses_a_symlink_first() {
    for (tool, arguments) in [
        ("plugin_read", r#"{"name":"keys"}"#),
        ("plugin_reload", r#"{"name":"keys"}"#),
        ("plugin_write", r#"{"name":"keys","source":"x"}"#),
        (
            "plugin_write",
            r#"{"name":"keys","source":"x","overwrite":true}"#,
        ),
    ] {
        let (argv, _) = plugin_argv_for(tool, arguments).unwrap();
        assert!(
            argv[2].contains(r#"if [ -L "$1" ]"#),
            "{tool} follows a symlink out of the plugin folder: {}",
            argv[2]
        );
        assert!(
            argv[2].contains("exit 1"),
            "{tool}'s guard does not stop the command: {}",
            argv[2]
        );
    }
}

/// The guard runs before the redirect, not merely somewhere in the script: an `overwrite=true`
/// that truncates first and checks afterwards would have destroyed the file already.
#[test]
fn a_write_checks_for_a_symlink_before_it_opens_anything() {
    for arguments in [
        r#"{"name":"keys","source":"x"}"#,
        r#"{"name":"keys","source":"x","overwrite":true}"#,
    ] {
        let (argv, _) = plugin_argv_for("plugin_write", arguments).unwrap();
        let guard = argv[2].find(r#"if [ -L "$1" ]"#).unwrap();
        let redirect = argv[2].find("printf %s").unwrap();
        assert!(guard < redirect, "{}", argv[2]);
    }
}

/// A failed `mkdir` must not fall through to the write. It used to be one `&&` chain, which the
/// guard's `if ... fi` would have broken: what follows the `;` after `fi` runs regardless.
#[test]
fn a_write_stops_at_the_first_step_that_fails() {
    let (argv, _) = plugin_argv_for("plugin_write", r#"{"name":"keys","source":"x"}"#).unwrap();
    assert!(
        argv[2].contains(r#"mkdir -p "$2" || exit 1"#),
        "{}",
        argv[2]
    );
}

/// The source travels in the environment, not in an argument: that keeps it out of `ps`, and
/// makes it a shell variable that is never re-parsed.
#[test]
fn writing_a_plugin_passes_the_source_through_the_environment() {
    let (argv, env) =
        plugin_argv_for("plugin_write", r#"{"name":"keys","source":"local M = {}"}"#).unwrap();

    assert_eq!(argv[0], "sh");
    assert_eq!(argv[1], "-c");
    assert_eq!(argv[3], "sh", "$0, so the file is $1 and the folder $2");
    assert_eq!(argv[4], format!("{DIR}/keys.lua"));
    assert_eq!(argv[5], DIR);
    assert_eq!(env.get("HOCUS_PLUGIN_SOURCE").unwrap(), "local M = {}\n");
    // Nothing the model wrote is in the script itself.
    assert!(!argv[2].contains("local M"));
}

/// `set -C` is what makes `overwrite=false` mean something: the write fails rather than
/// truncating a file that is already there.
#[test]
fn a_write_refuses_to_clobber_unless_overwrite_was_asked_for() {
    let (guarded, _) = plugin_argv_for("plugin_write", r#"{"name":"keys","source":"x"}"#).unwrap();
    assert!(guarded[2].contains("set -C"), "{}", guarded[2]);

    let (unguarded, _) = plugin_argv_for(
        "plugin_write",
        r#"{"name":"keys","source":"x","overwrite":true}"#,
    )
    .unwrap();
    assert!(!unguarded[2].contains("set -C"), "{}", unguarded[2]);

    // Both create the folder, so writing the first plugin does not need one to exist.
    assert!(guarded[2].contains("mkdir -p"));
    assert!(unguarded[2].contains("mkdir -p"));
}

#[test]
fn a_source_ends_in_exactly_one_newline_however_it_arrived() {
    for sent in [
        r#""local M = {}""#,
        r#""local M = {}\n""#,
        r#""local M = {}\n\n\n""#,
    ] {
        let (_, env) = plugin_argv_for(
            "plugin_write",
            &format!(r#"{{"name":"keys","source":{sent}}}"#),
        )
        .unwrap();
        assert_eq!(env.get("HOCUS_PLUGIN_SOURCE").unwrap(), "local M = {}\n");
    }
}

#[test]
fn a_write_with_no_source_or_an_enormous_one_is_refused_before_any_command() {
    assert!(plugin_argv_for("plugin_write", r#"{"name":"keys"}"#)
        .unwrap_err()
        .contains("source"));
    assert!(
        plugin_argv_for("plugin_write", r#"{"name":"keys","source":"   "}"#)
            .unwrap_err()
            .contains("source")
    );

    let huge = "x".repeat(MAX_PLUGIN_SOURCE_BYTES + 1);
    let error = plugin_argv_for(
        "plugin_write",
        &serde_json::json!({ "name": "keys", "source": huge }).to_string(),
    )
    .unwrap_err();
    assert!(error.contains("limit"), "{error}");
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
fn a_long_source_is_cut_short_in_the_confirmation() {
    let source: Vec<String> = (0..50).map(|n| format!("-- line {n}")).collect();
    let detail = confirmation_detail(&call(
        "plugin_write",
        &serde_json::json!({ "name": "keys", "source": source.join("\n") }).to_string(),
    ))
    .join("\n");
    assert!(detail.contains("-- line 0"), "{detail}");
    assert!(detail.contains("30 more lines"), "{detail}");
    assert!(!detail.contains("-- line 49"), "{detail}");
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
