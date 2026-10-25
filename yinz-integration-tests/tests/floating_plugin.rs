#![cfg(unix)]

use yinz_cli::args::CliAction;
use yinz_integration_tests::{keys, Size, TestRunner, PROMPT, TERMINAL_SIZE};

const PIN_MARKER: &str = "PIN [ ]";

#[test]
fn hidden_configuration_stays_hidden_after_terminal_resize() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            r#"
keybinds {
    normal {
        bind "Ctrl b" {
            LaunchOrFocusPlugin "yinz:configuration" { floating true; }
        }
    }
}
"#,
        )
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until("terminal ready", |grid| grid.contains("$ "));
    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until("Configuration floating pane shown", |grid_snapshot| {
        grid_snapshot.contains(PIN_MARKER) && grid_snapshot.contains("Configuration")
    });
    yinz.send_stdin(&keys::alt('f'));
    yinz.wait_until("floating panes hidden", |grid_snapshot| {
        !grid_snapshot.contains(PIN_MARKER) && grid_snapshot.status_bar_appears()
    });

    yinz.resize(Size {
        cols: 140,
        rows: 30,
    });
    terminal.wait_for_size(
        "terminal pane resized to the larger display",
        |cols, _rows| cols > 120,
    );
    yinz.wait_until("display settled at the new size", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
    });

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.wait_until("pane mode entered after the resize", |grid_snapshot| {
        grid_snapshot
            .lines()
            .last()
            .is_some_and(|last_line| last_line.contains("Fullscreen"))
    });
    yinz.send_stdin(&keys::ESC);
    let settled = yinz.wait_until("normal mode restored after the resize", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
    });

    assert!(
        !settled.contains(PIN_MARKER),
        "hidden Configuration floating pane must stay hidden after a terminal resize\n{}",
        settled.text,
    );
    yinz.quit();
}

#[test]
fn startup_has_no_unexpected_floating_pane() {
    assert_startup_has_no_floating_pane("");
}

#[test]
fn legacy_startup_options_cannot_enable_a_floating_pane() {
    for config in [
        "show_startup_tips true",
        "show_release_notes true",
        "show_release_notes false",
    ] {
        assert_startup_has_no_floating_pane(config);
    }
}

fn assert_startup_has_no_floating_pane(config: &str) {
    let mut yinz = TestRunner::new(TERMINAL_SIZE).with_config(config).start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    let ready = yinz.wait_until("startup ready", |grid| {
        grid.contains("$ ") && grid.status_bar_appears()
    });
    assert!(!ready.contains(PIN_MARKER), "{}", ready.text);
    yinz.quit();
}

/// The default configuration starts a dozen plugins in the background, and only the layout's
/// bars declare a pane: everything else runs headless, so the session's panes are the terminal
/// and the bars. The wait is for those background plugins to be running, so the pane list is
/// read after they had every chance to add a pane.
#[test]
fn default_background_plugins_add_no_pane_beside_the_layout_bars() {
    const BACKGROUND: [&str; 7] = [
        "agent-service",
        "project-store",
        "session-palette",
        "layout-palette",
        "plugin-palette",
        "configuration-palette",
        "link",
    ];
    let mut yinz = TestRunner::new(TERMINAL_SIZE).start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_for_app_load();
    let deadline = std::time::Instant::now() + yinz_integration_tests::default_timeout();
    let running = loop {
        let plugins = yinz_client::store_client::plugin_request(
            &yinz.session_name().into(),
            yinz_types::cli::PluginRequest {
                operation: yinz_types::cli::PluginOperation::List,
                target: None,
                configuration: None,
                fallback: None,
                candidates: vec![],
                caller: None,
            },
        )
        .unwrap()
        .plugins;
        let all_running = BACKGROUND.iter().all(|name| {
            plugins.iter().any(|status| {
                status.location.ends_with(name)
                    && status.state == yinz_types::cli::PluginState::Running
            })
        });
        if all_running {
            break plugins;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for the default background plugins to run: {plugins:#?}"
        );
        std::thread::yield_now();
    };
    for status in &running {
        let is_bar = ["tab-bar", "status-bar", "compact-bar"]
            .iter()
            .any(|bar| status.location.ends_with(bar));
        assert_eq!(status.pane, Some(is_bar), "{status:#?}");
    }
    let (code, json) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(code, 0, "{json}");
    let panes: Vec<yinz_types::data::PaneListEntry> = serde_json::from_str(&json).unwrap();
    let plugin_urls: Vec<_> = panes
        .iter()
        .filter(|pane| pane.pane_info.is_plugin())
        .map(|pane| pane.pane_info.plugin_url.clone().unwrap_or_default())
        .collect();
    let mut bars = plugin_urls.clone();
    bars.sort();
    assert_eq!(
        bars,
        ["status-bar", "tab-bar"],
        "only the layout bars may have plugin panes: {panes:#?}"
    );
    assert!(panes
        .iter()
        .filter(|pane| pane.pane_info.is_plugin())
        .all(|pane| !pane.pane_info.is_floating && !pane.pane_info.is_suppressed));
    yinz.quit();
}
