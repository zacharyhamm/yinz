use crate::{
    args::{CliArgs, Command, SessionCommand, Sessions},
    options::merge_from_cli,
    setup::Setup,
};
use clap::Parser;
use yinz_runtime::errors::prelude::*;
use yinz_types::input::options::Options;

fn boolean_options(value: Option<bool>) -> Options {
    Options {
        simplified_ui: value,
        disable_splash_screen: value,
        agent_double_escape: value,
        mouse_mode: value,
        detachable_panes: value,
        pane_frames: value,
        auto_layout: value,
        mirror_session: value,
        session_serialization: value,
        serialize_pane_viewport: value,
        focus_follows_mouse: value,
        mouse_click_through: value,
        context_menu_on_right_click: value,
        ..Default::default()
    }
}

#[test]
fn explicit_cli_booleans_override_config_and_can_be_applied_twice() {
    for configured in [None, Some(false), Some(true)] {
        for supplied in [None, Some(false), Some(true)] {
            let base = boolean_options(configured);
            let cli = boolean_options(supplied);
            assert_eq!(
                Options::from(crate::options::CliOptions::from(cli.clone())),
                cli
            );
            let expected = boolean_options(supplied.or(configured));
            let merged = merge_from_cli(&base, cli.clone());
            assert_eq!(merged, expected, "config={configured:?}, cli={supplied:?}");
            assert_eq!(merge_from_cli(&merged, cli), expected);
        }
    }
}

#[test]
fn attach_boolean_options_survive_setup_and_the_final_command_merge() -> Result<()> {
    // Clap's full command tree needs the same stack size as the other CLI parser tests.
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| -> Result<()> {
            let config_dir = tempfile::tempdir().context("create isolated config directory")?;
            let config_path = config_dir.path().join("config.kdl");
            for configured in [false, true] {
                std::fs::write(
                    &config_path,
                    format!("pane_frames {configured}\ndisable_splash_screen {configured}\n"),
                )
                .context("write test configuration")?;
                for supplied in [None, Some(false), Some(true)] {
                    let mut argv = vec![
                        "yinz".to_owned(),
                        "--config-dir".to_owned(),
                        config_dir.path().display().to_string(),
                        "attach".to_owned(),
                        "example".to_owned(),
                        "options".to_owned(),
                    ];
                    if let Some(value) = supplied {
                        argv.extend(["--pane-frames".to_owned(), value.to_string()]);
                        argv.extend(["--disable-splash-screen".to_owned(), value.to_string()]);
                    }
                    let args = CliArgs::try_parse_from(argv).context("parse attach options")?;
                    let (_, _, options, _, options_without_layout) = Setup::from_cli_args(&args)
                        .map_err(|error| anyhow!("load attach configuration: {error}"))?;
                    let Some(Command::Sessions(Sessions::Attach {
                        options: Some(cli), ..
                    })) = args.command
                    else {
                        return Err(anyhow!("expected attach options"));
                    };
                    let SessionCommand::Options(cli) = *cli;
                    assert_eq!(cli.pane_frames, supplied);
                    assert_eq!(cli.disable_splash_screen, supplied);
                    let expected = Some(supplied.unwrap_or(configured));
                    for base in [options, options_without_layout] {
                        assert_eq!(base.pane_frames, expected);
                        assert_eq!(base.disable_splash_screen, expected);
                        // The attach command reapplies its options after Setup::from_cli_args.
                        assert_eq!(
                            merge_from_cli(&base, cli.clone().into()).pane_frames,
                            expected,
                            "config={configured}, cli={supplied:?}"
                        );
                        assert_eq!(
                            merge_from_cli(&base, cli.clone().into()).disable_splash_screen,
                            expected
                        );
                    }
                }
            }
            Ok(())
        })
        .context("spawn attach options test")?
        .join()
        .unwrap()
}
