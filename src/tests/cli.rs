use clap::{CommandFactory, Parser};
use {yinz_cli::args::CliArgs, yinz_cli::args::Command};

const CLI_PARSE_STACK_SIZE: usize = 32 * 1024 * 1024;

/// clap's derived builder for `CliArgs` needs megabytes of stack in a debug build (the
/// derive expands the huge `CliAction` enum into one unoptimized function), close to
/// libtest's 2MiB per-test threads. Parse on a thread with room.
fn try_parse_cli<I, S>(args: I) -> Result<CliArgs, clap::Error>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    std::thread::Builder::new()
        .stack_size(CLI_PARSE_STACK_SIZE)
        .spawn(move || CliArgs::try_parse_from(args))
        .expect("failed to spawn cli parse thread")
        .join()
        .expect("cli parse thread panicked")
}

#[test]
fn verify_cli() {
    std::thread::Builder::new()
        .stack_size(CLI_PARSE_STACK_SIZE)
        .spawn(|| CliArgs::command().debug_assert())
        .expect("failed to spawn cli parse thread")
        .join()
        .expect("cli parse thread panicked");
}

// ---------------------------------------------------------------------------
// `yinz hooks`
// ---------------------------------------------------------------------------

#[test]
fn cli_agent_hooks_parses_its_three_subcommands() {
    use yinz_cli::args::Hooks;

    let install = try_parse_cli(["yinz", "hooks", "install"]).expect("parsed");
    assert!(matches!(
        install.command,
        Some(Command::Hooks(Hooks::Install {
            agent: None,
            project: false,
            project_dir: None,
            dry_run: false
        }))
    ));

    let project_dir =
        try_parse_cli(["yinz", "hooks", "install", "--project-dir", "/work/app"]).expect("parsed");
    match project_dir.command {
        Some(Command::Hooks(Hooks::Install {
            project_dir: Some(dir),
            ..
        })) => assert_eq!(dir, std::path::PathBuf::from("/work/app")),
        other => panic!("expected an install with a project dir, got {other:?}"),
    }

    let project = try_parse_cli(["yinz", "hooks", "install", "--project"]).expect("parsed");
    assert!(matches!(
        project.command,
        Some(Command::Hooks(Hooks::Install { project: true, .. }))
    ));

    let status = try_parse_cli(["yinz", "hooks", "status"]).expect("parsed");
    assert!(matches!(
        status.command,
        Some(Command::Hooks(Hooks::Status { agent: None }))
    ));

    let codex = try_parse_cli(["yinz", "hooks", "install", "--agent", "codex"]).expect("parsed");
    assert!(matches!(
        codex.command,
        Some(Command::Hooks(Hooks::Install {
            agent: Some(agent),
            ..
        })) if agent == "codex"
    ));

    let opencode =
        try_parse_cli(["yinz", "hooks", "install", "--agent", "opencode"]).expect("parsed");
    assert!(matches!(
        opencode.command,
        Some(Command::Hooks(Hooks::Install {
            agent: Some(agent),
            ..
        })) if agent == "opencode"
    ));

    let muse = try_parse_cli(["yinz", "hooks", "install", "--agent", "muse"]).expect("parsed");
    assert!(matches!(
        muse.command,
        Some(Command::Hooks(Hooks::Install {
            agent: Some(agent),
            ..
        })) if agent == "muse"
    ));

    assert!(
        try_parse_cli(["yinz", "hooks"]).is_err(),
        "`hooks` on its own names no action"
    );
}

/// The one thing an installer that edits somebody else's config file must get right:
/// it may add and remove its own entries and it may touch nothing else.
#[test]
fn installing_hooks_leaves_the_rest_of_the_settings_alone() {
    use crate::agent_hooks::{install_into, installed_events, uninstall_from};
    use serde_json::json;

    let original = json!({
        "model": "opus",
        "permissions": { "defaultMode": "auto" },
        "hooks": {
            "Stop": [
                { "hooks": [ { "type": "command", "command": "/usr/local/bin/mine.sh" } ] }
            ],
            // Deliberately an event this fork has never wired and has no reason to:
            // the point is that a growing table stays out of everything beside it.
            "FileChanged": [
                { "matcher": "auto", "hooks": [ { "type": "command", "command": "notify" } ] }
            ]
        }
    });

    let installed = install_into(original.clone(), "/home/me/.claude/yinz/yinz-hook.sh")
        .expect("valid settings");
    assert_eq!(installed["model"], "opus");
    assert_eq!(installed["permissions"]["defaultMode"], "auto");
    assert_eq!(
        installed["hooks"]["FileChanged"], original["hooks"]["FileChanged"],
        "an event we do not wire is not ours to touch"
    );
    // Someone else's Stop hook is still there, beside ours rather than instead of it.
    let stop = installed["hooks"]["Stop"].as_array().expect("an array");
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0]["hooks"][0]["command"], "/usr/local/bin/mine.sh");

    // And uninstalling puts the file back exactly as it was.
    assert_eq!(uninstall_from(installed), original);
    assert!(installed_events(&original).is_empty());
}

/// Running the installer twice is a thing people do, and a thing an upgrade does.
#[test]
fn installing_twice_wires_each_event_once() {
    use crate::agent_hooks::{install_into, installed_events};
    use serde_json::{json, Value};

    let script = "/home/me/.claude/yinz/yinz-hook.sh";
    let once = install_into(Value::Object(Default::default()), script).expect("valid settings");
    let twice = install_into(once.clone(), script).expect("valid settings");
    assert_eq!(once, twice);

    let events = installed_events(&twice);
    // Against the table rather than a number: it has grown once already, and a literal
    // here is a test that goes stale silently the next time it does.
    assert_eq!(
        events.len(),
        crate::agent_hooks::CLAUDE_EVENTS.len(),
        "{events:?}"
    );
    for event in &events {
        assert_eq!(
            twice["hooks"][event].as_array().expect("an array").len(),
            1,
            "{event} wired more than once"
        );
    }

    // A path that moved still counts as ours, so it is replaced rather than doubled.
    let moved = install_into(twice, "/opt/yinz/yinz-hook.sh").expect("valid settings");
    assert_eq!(
        moved["hooks"]["Stop"].as_array().expect("an array").len(),
        1
    );
    assert_eq!(
        moved["hooks"]["Stop"][0]["hooks"][0]["command"],
        json!("/opt/yinz/yinz-hook.sh")
    );
}

/// Uninstalling from a file that has nothing of ours in it is not a licence to tidy it.
#[test]
fn uninstalling_removes_only_what_we_wrote() {
    use crate::agent_hooks::uninstall_from;
    use serde_json::json;

    let theirs = json!({
        "hooks": {
            "Stop": [
                { "hooks": [
                    { "type": "command", "command": "/usr/local/bin/mine.sh" },
                    { "type": "command", "command": "/home/me/.claude/yinz/yinz-hook.sh" }
                ] },
                { "hooks": [ { "type": "command", "command": "/home/me/.claude/yinz/yinz-hook.sh" } ] }
            ]
        }
    });

    let cleaned = uninstall_from(theirs);
    assert_eq!(
        cleaned,
        json!({ "hooks": { "Stop": [ { "hooks": [
            { "type": "command", "command": "/usr/local/bin/mine.sh" }
        ] } ] } }),
        "ours goes, theirs stays, and the group we emptied goes with it"
    );

    // A file with nothing but our entries loses the `hooks` key entirely rather than
    // keeping a husk of empty arrays.
    let ours_only = json!({ "model": "opus", "hooks": { "Stop": [
        { "hooks": [ { "type": "command", "command": "yinz-hook.sh" } ] }
    ] } });
    assert_eq!(uninstall_from(ours_only), json!({ "model": "opus" }));

    // And a settings file that is not an object at all is treated as an empty one, so
    // an install onto a machine where nobody has written one still works.
    assert_eq!(uninstall_from(json!(null)), json!({}));
}

#[test]
fn installing_hooks_rejects_settings_shapes_it_cannot_merge() {
    use crate::agent_hooks::install_into;
    use serde_json::json;

    assert!(install_into(json!({ "hooks": [] }), "yinz-hook.sh").is_err());
    assert!(install_into(
        json!({ "hooks": { "Stop": { "hooks": [] } } }),
        "yinz-hook.sh"
    )
    .is_err());
}

#[test]
fn uninstalling_hooks_preserves_user_authored_empty_events() {
    use crate::agent_hooks::uninstall_from;
    use serde_json::json;

    let settings = json!({
        "hooks": {
            "SomeEvent": [],
            "Stop": [
                { "hooks": [] },
                { "hooks": [{ "command": "yinz-hook.sh" }] }
            ]
        }
    });
    assert_eq!(
        uninstall_from(settings),
        json!({ "hooks": { "SomeEvent": [], "Stop": [{ "hooks": [] }] } })
    );
}

#[test]
fn server_termios_requires_server() {
    assert!(try_parse_cli(["yinz", "--server-termios", "null"]).is_err());
    let args = try_parse_cli([
        "yinz",
        "--server",
        "/tmp/test.sock",
        "--server-termios",
        "null",
    ])
    .expect("internal server arguments");
    assert_eq!(args.server_termios.as_deref(), Some("null"));
    assert!(try_parse_cli(["yinz", "--server", "/tmp/test.sock"])
        .unwrap()
        .server_termios
        .is_none());
}

#[test]
fn explicit_theme_hue_keeps_cli_choices() {
    use yinz_types::style::ThemeHue;
    for (value, expected) in [("light", ThemeHue::Light), ("dark", ThemeHue::Dark)] {
        let args = try_parse_cli(["yinz", "options", "--explicit-theme-hue", value]).unwrap();
        let Some(Command::Options(options)) = args.command else {
            panic!("expected options command");
        };
        assert_eq!(options.explicit_theme_hue, Some(expected));
    }
    for invalid in ["LIGHT", "Dark", "unknown"] {
        assert!(try_parse_cli(["yinz", "options", "--explicit-theme-hue", invalid]).is_err());
    }
}

#[test]
fn removed_recording_commands_and_options_are_rejected() {
    for args in [
        vec!["yinz", "recordings", "list"],
        vec!["yinz", "recordings", "search", "text"],
        vec!["yinz", "recordings", "export", "1"],
        vec!["yinz", "recordings", "compact"],
        vec!["yinz", "options", "--session-recording", "true"],
        vec!["yinz", "options", "--session-recording-input", "true"],
    ] {
        assert!(try_parse_cli(args.clone()).is_err(), "{args:?}");
    }
}

#[test]
fn bubblewrap_prepare_respects_the_selected_config_directory() {
    let opts = try_parse_cli([
        "yinz",
        "--config-dir",
        "/custom/config",
        "bubblewrap",
        "prepare",
        "--project",
        "/work/repo",
    ])
    .unwrap();
    assert_eq!(
        opts.config_dir.as_deref(),
        Some(std::path::Path::new("/custom/config"))
    );
    assert!(
        matches!(opts.command, Some(Command::Bubblewrap(yinz_cli::args::Bubblewrap::Prepare { project: Some(path) })) if path == std::path::Path::new("/work/repo"))
    );
}
