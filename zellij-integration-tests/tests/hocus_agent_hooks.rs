#![cfg(unix)]

//! The one thing the plugin's own tests cannot prove: that a hook script's
//! `zellij pipe` actually arrives at `hocus-agentd` carrying the pane it fired in.
//!
//! Everything on either side of that is unit tested — `agents::hook_phase` and the
//! status cell in `hocus-proto`, the daemon's handling in `hocus-agentd`, the settings
//! merge in `zellij hocus hooks`. What none of them can see is the seam in the middle:
//! a CLI pipe carries a `PipeSource::Cli` with nothing but a pipe id in it, so the pane
//! id has to travel as a `--args` pair, through the same `PluginUserConfiguration`
//! parser a plugin's KDL block uses. If that pair does not survive the trip, every
//! event is dropped in silence and every test above still passes.
//!
//! So the pipe comes from outside every plugin, exactly as the generated script sends
//! it, and the assertion is read off hocus's own `agent_list` on the grid.

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};
use zellij_utils::cli::CliAction;

/// hocus and the daemon, both shared, with the daemon loaded in the background as the
/// real config has it. `zellij:` rather than `file:` for the reason `hocus_store.rs`
/// gives: neither plugin asks for permissions it expects a builtin to be granted.
const HOCUS_CONFIG: &str = r#"plugins {
  hocus location="zellij:hocus" {
    shared true
  }
  hocus-agentd location="zellij:hocus-agentd" {
    shared true
  }
}
load_plugins clear-defaults=true {
  "hocus-agentd"
}
"#;

/// A hocus pane to read the answer off, and one terminal pane to be the agent.
///
/// `focus=true` on the hocus pane because there are two of them: without it the
/// keystrokes this test sends land in the sleeping terminal instead, starting with the
/// `y` that answers the permission prompt.
///
/// The terminal pane is named rather than running anything: `agents::kind_from_title`
/// is enough to make it an agent for the daemon, and a pane that only sleeps cannot
/// print anything that would muddle the transcript below.
const HOCUS_LAYOUT: &str = r#"layout {
    pane focus=true {
        plugin location="hocus"
    }
    pane name="claude" command="sleep" {
        args "600"
    }
}
"#;

/// Run a hocus slash command and wait for the line to be in the box before sending it.
///
/// Pasted rather than typed, for the reason `hocus_store.rs` gives: typed, the burst
/// outruns the render and the line arrives half finished.
fn submit(zellij: &TestSession, line: &str) {
    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(line.as_bytes());
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(&line[line.len().saturating_sub(12)..])
    });
    zellij.send_stdin(&keys::ENTER);
}

/// One Claude Code hook event, sent the way the generated script sends it.
///
/// Deliberately no `plugin` argument: a bare `zellij:hocus-agentd` url carries none of
/// the alias' `shared true`, so it would miss the running daemon and launch a second
/// one. A broadcast reaches whatever is already running, and `agent_hook` is handled in
/// exactly one place.
fn send_hook(zellij: &mut TestSession, pane_id: u32, payload: &str) {
    zellij.run_cli_action(CliAction::Pipe {
        name: Some("agent_hook".to_owned()),
        payload: Some(payload.to_owned()),
        args: Some(
            format!("pane_id={pane_id}")
                .parse()
                .expect("a k=v pair parses"),
        ),
        plugin: None,
        plugin_configuration: None,
        // Every one of these only matters when the pipe *launches* a plugin, which
        // this must never do — see above.
        force_launch_plugin: false,
        skip_plugin_cache: false,
        floating_plugin: None,
        in_place_plugin: None,
        plugin_cwd: None,
        plugin_title: None,
    });
}

/// Clear hocus's transcript, so the next answer is the only thing that can put a word
/// back on the screen.
fn clear(zellij: &TestSession, gone: &str) {
    zellij.send_stdin(&[0x0c]); // Ctrl-l
    zellij.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains(gone)
    });
}

#[test]
fn a_hook_from_the_cli_makes_hocus_report_the_agent_as_waiting() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();

    zellij.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    // The daemon has to have seen the pane before a hook about it can mean anything,
    // and this is also where the pane's id is learned: nothing else in the test knows
    // what the layout's one terminal was numbered.
    submit(&zellij, r#"/agent_list {}"#);
    zellij.wait_until("the daemon has found the agent pane", |grid_snapshot| {
        grid_snapshot.contains("agents")
    });
    let listed = zellij.snapshot().to_string();
    let pane_id = agent_pane_id(&listed);
    assert!(
        !listed.contains("waiting"),
        "nothing has reported anything yet:\n{listed}"
    );

    clear(&zellij, "agents");

    // The message the generated hook script sends, and the only thing in this session
    // that can put the word "waiting" on the screen.
    send_hook(
        &mut zellij,
        pane_id,
        r#"{"hook_event_name":"PermissionRequest","session_id":"cc-1"}"#,
    );

    submit(&zellij, r#"/agent_list {}"#);
    zellij.wait_until(
        "hocus reports the agent as blocked on the user",
        |grid_snapshot| grid_snapshot.contains("waiting"),
    );
}

/// The pane id `agent_list` drew, off the grid.
///
/// The list is presented rather than printed as JSON: hocus renders the markdown, so
/// one agent is a line reading `• 0 running — claude`. This takes the number off the
/// first such line.
fn agent_pane_id(screen: &str) -> u32 {
    screen
        .lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix('\u{2022}')?;
            rest.split_whitespace().next()?.parse().ok()
        })
        .unwrap_or_else(|| panic!("agent_list listed no pane:\n{screen}"))
}
