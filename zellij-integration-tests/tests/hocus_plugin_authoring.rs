#![cfg(unix)]

//! Authoring a Lua plugin end to end: a real session, host filesystem operations, a real file
//! on disk, and a real Lua plugin loaded from it.
//!
//! The unit tests cover path resolution and tool results separately. This test checks that
//! hocus reaches the host's `/plugins` filesystem root and loads the file it wrote there.
//!
//! It also proves the part no unit test can: that a wasm plugin, which cannot open a file in a
//! config dir at all, still gets a plugin written there and then running.

use std::fs;

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

/// `shared true` because hocus warns without it, and the warning would be the first thing on
/// screen. The store is loaded because hocus expects a snapshot from it.
fn hocus_config(user_plugin_dir: &str) -> String {
    format!(
        r#"user_plugin_dir "{user_plugin_dir}"
plugins {{
  hocus location="zellij:hocus" {{
    shared true
  }}
  hocus-store location="zellij:hocus-store"
}}
load_plugins clear-defaults=true {{
  "hocus-store"
}}
"#
    )
}

const HOCUS_LAYOUT: &str = r#"layout {
    pane {
        plugin location="hocus"
    }
}
"#;

/// Put a line into hocus and send it. Slash commands run the tool inline — no API key involved —
/// which is what makes the result readable from the screen.
///
/// Pasted rather than typed: a long line arrives as one `PastedText` event instead of a burst of
/// keystrokes that outruns the render.
fn submit(zellij: &TestSession, line: &str) {
    submit_awaiting(zellij, line, &line[line.len().saturating_sub(20)..]);
}

/// The same, for a line long enough to wrap in the input box — where the tail is split across
/// two rows and so is never one contiguous run in the snapshot. `marker` is something on the
/// line that is not near a wrap.
fn submit_awaiting(zellij: &TestSession, line: &str, marker: &str) {
    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(line.as_bytes());
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(marker)
    });
    zellij.send_stdin(&keys::ENTER);
}

/// Ctrl-l. The result of a call echoes its arguments, so the query has to be off the screen
/// entirely for the next answer to be what puts it back.
fn clear(zellij: &TestSession, gone: &str) {
    zellij.send_stdin(&[0x0c]);
    zellij.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains(gone)
    });
}

#[test]
fn hocus_writes_a_lua_plugin_and_loads_it() {
    // The folder hocus will be told about, empty to begin with: the write is what creates it,
    // which is the behaviour worth proving as much as the write itself.
    let plugins = tempfile::tempdir().expect("a temp dir");
    let plugin_dir = plugins.path().join("plugins");
    let plugin_dir_string = plugin_dir.display().to_string();

    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&hocus_config(&plugin_dir_string))
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();

    zellij.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    // The docs are compiled in, so they answer with nothing on disk at all.
    submit(&zellij, r#"/plugin_docs {"section": "quickstart"}"#);
    // The tail of the section rather than its head: the transcript is pinned to the bottom, so
    // the last lines of a long result are the ones on screen.
    zellij.wait_until("the guide reaches the model", |grid_snapshot| {
        grid_snapshot.contains("anything else is wasm")
    });
    clear(&zellij, "anything else is wasm");

    // Nothing there yet, and the folder does not even exist.
    submit(&zellij, "/plugin_list");
    zellij.wait_until("the folder is empty", |grid_snapshot| {
        grid_snapshot.contains("no plugins yet")
    });
    clear(&zellij, "no plugins yet");

    // A name that climbs out of the folder is refused before anything runs.
    submit(
        &zellij,
        r#"/plugin_write {"name": "../escape", "source": "return {}"}"#,
    );
    zellij.wait_until("a name that is a path is refused", |grid_snapshot| {
        grid_snapshot.contains("not a plugin name") || grid_snapshot.contains("single file name")
    });
    clear(&zellij, "plugin name");

    // The write itself.
    //
    // No confirmation appears here even though `plugin_write` is in the confirmation table:
    // typing a slash command *is* the confirmation, so `run_manual` goes straight to
    // `run_head`. The prompt the model would get is covered by the unit tests instead.
    let source =
        "local M = {}\\nfunction M.render() return \\\"probe is running\\\" end\\nreturn M";
    submit_awaiting(
        &zellij,
        &format!(r#"/plugin_write {{"name": "probe", "source": "{source}"}}"#),
        "M.render",
    );
    zellij.wait_until("the plugin is written", |grid_snapshot| {
        grid_snapshot.contains("call plugin_reload")
    });

    // On disk, in the folder the write had to create.
    let written = plugin_dir.join("probe.lua");
    assert!(
        written.exists(),
        "expected the plugin at {}",
        written.display()
    );
    let contents = fs::read_to_string(&written).expect("the plugin is readable");
    assert!(contents.contains("probe is running"), "{contents}");
    assert!(contents.ends_with('\n'), "exactly one trailing newline");

    clear(&zellij, "call plugin_reload");

    // Listed by name rather than by path, since a name is what the tools take back.
    submit(&zellij, "/plugin_list");
    zellij.wait_until("the plugin is listed", |grid_snapshot| {
        grid_snapshot.contains("probe.lua")
    });
    clear(&zellij, "probe.lua");

    // Read back through the host, into a plugin that cannot open the file itself.
    submit(&zellij, r#"/plugin_read {"name": "probe"}"#);
    zellij.wait_until("the plugin is read back", |grid_snapshot| {
        grid_snapshot.contains("probe is running")
    });
    clear(&zellij, "probe is running");

    // Writing it again without `overwrite` is refused after checking the host's file metadata.
    submit(
        &zellij,
        r#"/plugin_write {"name": "probe", "source": "return {}"}"#,
    );
    zellij.wait_until("the existing plugin is not clobbered", |grid_snapshot| {
        grid_snapshot.contains("already exists")
    });
    let after = fs::read_to_string(&written).expect("the plugin is still readable");
    assert!(
        after.contains("probe is running"),
        "the original must survive a refused overwrite, got: {after}"
    );
    clear(&zellij, "already exists");

    // A symlink in the folder passes every rule about the *name*, so the containment that the
    // name check can only assert lexically is enforced by the host, on the real link.
    //
    // The write is the one that matters — `overwrite=true` would truncate whatever the link
    // points at, and the confirmation the user saw named only "link" — but the read is refused
    // for the same reason and by the same guard.
    let outside = plugins.path().join("outside.lua");
    fs::write(&outside, "-- outside the plugin folder\n").expect("a file outside the folder");
    std::os::unix::fs::symlink(&outside, plugin_dir.join("link.lua")).expect("a symlink");

    submit(&zellij, r#"/plugin_read {"name": "link"}"#);
    zellij.wait_until("reading through a symlink is refused", |grid_snapshot| {
        grid_snapshot.contains("is a symlink")
    });
    clear(&zellij, "is a symlink");

    submit_awaiting(
        &zellij,
        r#"/plugin_write {"name": "link", "source": "return {}", "overwrite": true}"#,
        "overwrite",
    );
    zellij.wait_until("writing through a symlink is refused", |grid_snapshot| {
        grid_snapshot.contains("is a symlink")
    });
    let untouched = fs::read_to_string(&outside).expect("the outside file is still there");
    assert_eq!(
        untouched, "-- outside the plugin folder\n",
        "an overwrite must not reach through a link out of the folder"
    );
    clear(&zellij, "is a symlink");

    // A targeted change, which is what a plugin that already works should get instead of a
    // whole file sent again from memory. The file it replaces is kept beside it.
    submit_awaiting(
        &zellij,
        r#"/plugin_edit {"name": "probe", "old_string": "probe is running", "new_string": "probe is edited"}"#,
        "probe is edited",
    );
    zellij.wait_until("the plugin is edited", |grid_snapshot| {
        grid_snapshot.contains("edited probe.lua")
    });
    let edited = fs::read_to_string(&written).expect("the plugin is readable");
    assert!(edited.contains("probe is edited"), "{edited}");
    let backup = fs::read_to_string(plugin_dir.join("probe.bak.lua")).expect("the backup is kept");
    assert!(
        backup.contains("probe is running"),
        "the backup holds the version that was replaced, got: {backup}"
    );
    clear(&zellij, "edited probe.lua");

    // An edit that matches nothing changes nothing, and says so rather than guessing.
    submit_awaiting(
        &zellij,
        r#"/plugin_edit {"name": "probe", "old_string": "never in the file", "new_string": "x"}"#,
        "never in the file",
    );
    zellij.wait_until("an edit that matches nothing is refused", |grid_snapshot| {
        grid_snapshot.contains("plugin_read it and copy")
    });
    clear(&zellij, "plugin_read it and copy");

    // The listing annotates the original with its backup, without listing the backup separately.
    submit(&zellij, "/plugin_list");
    let listing = zellij.wait_until("the plugin is listed with its backup", |grid_snapshot| {
        grid_snapshot.contains("probe.lua")
            && grid_snapshot.contains("probe.bak kept from an earlier edit")
    });
    assert!(
        !listing.contains("probe.bak.lua"),
        "a backup must not be listed as a separate plugin: {listing}"
    );
    clear(&zellij, "probe.lua");

    // And finally the whole point: the file becomes a running plugin. `plugin_reload` starts one
    // that is not already running, so this opens it in a pane of its own.
    submit(&zellij, r#"/plugin_reload {"name": "probe"}"#);
    zellij.wait_until("the plugin is loaded and renders", |grid_snapshot| {
        grid_snapshot.contains("probe is edited")
    });

    zellij.quit();
}

/// The hook a plugin author writes for the loop below. It logs every firing on one line, so
/// that the order the points fired in can be read off `zellij.log` - and, opened in place of the
/// pane for `agent_exit`, it logs from `load`, where that point's payload arrives as the
/// configuration.
const AGENT_LOOP_HOOK: &str = r#"
local zellij = require("zellij")
local M = {}

local function log_firing(point, payload)
  -- A headless point's payload is typed (`pane_id`, `agent.state`, `exit_code`); `agent_exit`
  -- arrives as the plugin's configuration, a string map with `agent_` keys.
  local pane = payload.pane_id or payload.agent_pane_id or "?"
  local detail = (payload.agent and string.lower(payload.agent.state))
    or payload.exit_code or payload.agent_exit_code or ""
  zellij.log("fired " .. point .. " pane=" .. tostring(pane) .. " " .. tostring(detail))
end

function M.load(config)
  zellij.subscribe { "hook" }
  if config.agent_pane_id then
    log_firing("agent_exit", config)
  else
    zellij.log("loaded headless")
  end
end

zellij.on("hook", function(e) log_firing(e.point, e.payload) end)

function M.render() return "agent-loop hook is up" end

return M
"#;

/// The agent's state as one `pane_update` event reports it for `terminal_id`, or `None` when
/// the pane is not in the event or carries no record.
fn agent_state_in(event_json: &str, terminal_id: u32) -> Option<String> {
    let event: serde_json::Value = serde_json::from_str(event_json).ok()?;
    event
        .get("PaneUpdate")?
        .get("panes")?
        .as_object()?
        .values()
        .filter_map(|panes| panes.as_array())
        .flatten()
        .find(|pane| pane["id"] == terminal_id && pane["is_plugin"] == false)?
        .get("agent")?
        .get("state")?
        .as_str()
        .map(str::to_owned)
}

/// The self-programming loop, end to end in one session: a hook is written into the plugin
/// folder the way `zellij plugins write` writes it, bound and persisted with `zellij action
/// set-hook --write`, and then fired by an agent that reports through `zellij action
/// agent-report` from a pane `zellij run --block-until-exit` opened - with `zellij events`
/// watching the agent's record change and `zellij.log` showing each firing in order.
///
/// Every piece is unit tested on its own; this is the one place all of them are proven to
/// meet. The agent is a fake pty standing in for `claude`, driven the way its hook script
/// would drive it: it reports busy, reports waiting, and exits.
#[test]
fn a_written_hook_is_bound_and_fired_by_a_reporting_agent_end_to_end() {
    use zellij_integration_tests::{claim_first_terminal_and_wait_for_prompt, default_timeout};
    use zellij_utils::cli::CliAction;

    // A config dir of its own, so `--write` has a `config.d` to land in and the plugin folder
    // is where a real install would keep it.
    let config_dir = zellij_integration_tests::test_env::init()
        .join("config")
        .join("agent-hook-loop");
    fs::create_dir_all(&config_dir).unwrap();
    let plugin_dir = config_dir.join("plugins");
    // The agent shares the first tab with the shell: with the placement rule on, `claude`
    // would get a tab of its own here (one pane's share of this terminal is under the default
    // floor - see `agent_pane_placement.rs`), and a pane alone in its tab draws no frame, so
    // nothing on screen would say "claude". Placement is not what this test is about.
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(config_dir.clone())
        .with_config(&format!(
            "user_plugin_dir \"{}\"\nagent_pane_min_columns 0",
            plugin_dir.display()
        ))
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    // 1. `zellij plugins write agent-loop < hook.lua`: the same function the command calls,
    //    compiling the source before anything touches the disk.
    let written = zellij_server::plugin_authoring::write(
        &plugin_dir,
        "agent-loop",
        AGENT_LOOP_HOOK,
        false,
        true,
    )
    .expect("the hook is written");
    assert_eq!(written.path, plugin_dir.join("agent-loop.lua"));

    // 2. `zellij action set-hook <point> file:agent-loop.lua --write`, for every point the loop
    //    passes through. A bare `file:` name is looked up in the plugin folder.
    let points = ["agent_start", "agent_waiting", "pane_exit", "agent_exit"];
    for point in points {
        let (exit_code, reported) = zellij.run_cli_action_reporting(CliAction::SetHook {
            point: point.to_owned(),
            url: "file:agent-loop.lua".to_owned(),
            write: true,
        });
        assert_eq!(exit_code, 0, "set-hook {point} failed: {reported}");
    }
    let managed = config_dir.join("config.d").join("zellij-managed.kdl");
    let persisted = fs::read_to_string(&managed)
        .unwrap_or_else(|e| panic!("{} was not written: {e}", managed.display()));
    for point in points {
        assert!(
            persisted.contains(&format!("{point} \"file:agent-loop.lua\"")),
            "{point} is not in the drop-in:\n{persisted}"
        );
    }

    // 4 (first half). `zellij events --type pane_update` watches from before the agent starts,
    //    so every state its record passes through is on the stream.
    let events = zellij.stream_events(&["pane_update"]);
    events.wait_for_event("PaneUpdate");

    // 3. `zellij run --block-until-exit -- claude`: the pane holds on exit, so the exit reaches
    //    the hooks, and the client returns with the command's status.
    let agent = zellij.run_blocking_command_holding_on_exit(&["claude"]);
    let agent_pty = zellij.expect_pty_spawn();
    let terminal_id = agent_pty.terminal_id();
    let pane_id = format!("terminal_{terminal_id}");
    zellij.wait_until("the agent pane is open", |grid_snapshot| {
        grid_snapshot.contains("claude")
    });

    // What the agent's hook script does: `zellij action agent-report --state ...` from inside
    // the pane. The first report is the record appearing, which is `agent_start`; the move to
    // `waiting` is `agent_waiting`.
    let report = |state: &str, detail: Option<&str>| CliAction::AgentReport {
        pane_id: Some(pane_id.clone()),
        kind: Some("claude".to_owned()),
        state: Some(state.to_owned()),
        detail: detail.map(str::to_owned),
        cwd: None,
        hook_json: false,
        clear: false,
        run_id: None,
    };
    assert_eq!(zellij.run_cli_action(report("busy", None)), 0);
    assert_eq!(zellij.run_cli_action(report("waiting", Some("y/n"))), 0);

    // The command exits. `pane_exit` fires, then `agent_exit` opens the hook in the pane's
    // place, and the blocking client comes back with the status.
    agent_pty.exit(Some(0));
    assert_eq!(agent.wait_for_exit(), 0);
    zellij.wait_until(
        "the agent_exit hook stands in for the pane",
        |grid_snapshot| grid_snapshot.contains("agent-loop hook is up"),
    );

    // 5. `zellij.log` has one line per firing, and the headless points reached one instance in
    //    the order they fired. `agent_exit` opens an instance of its own, whose `load` runs on
    //    a thread of its own while the headless instance is still replaying what it was sent
    //    during its load - so where its line lands relative to the others is scheduling, not
    //    the behaviour under test; that it fired, with the right payload, is.
    let deadline = std::time::Instant::now() + default_timeout();
    let log = loop {
        let log = zellij_integration_tests::test_env::log_tail(2000);
        if log.contains("[lua:agent-loop.lua] fired agent_exit") {
            break log;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the agent_exit hook never logged its firing; the log's tail:\n{}",
            zellij_integration_tests::test_env::log_tail(60)
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let firing = |line: &str| {
        log.find(&format!("[lua:agent-loop.lua] fired {line}"))
            .unwrap_or_else(|| panic!("no firing logged as {line:?}; the log:\n{log}"))
    };
    let order = [
        firing(&format!("agent_start pane={terminal_id} busy")),
        firing(&format!("agent_waiting pane={terminal_id} waiting")),
        firing(&format!("pane_exit pane={terminal_id} 0")),
    ];
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "the hooks fired out of order (offsets {order:?} in the log):\n{log}"
    );
    firing(&format!("agent_exit pane={terminal_id} 0"));
    // `agent_waiting` fired while the load `agent_start` began was still in flight, and
    // reached that instance rather than loading a second one.
    assert_eq!(
        log.matches("[lua:agent-loop.lua] loaded headless").count(),
        1,
        "one headless instance serves every firing:\n{log}"
    );

    // 4 (second half). The stream saw the record move busy -> waiting -> exited.
    let mut states: Vec<String> = vec![];
    for (_, event_json) in events.streamed_events() {
        if let Some(state) = agent_state_in(&event_json, terminal_id) {
            if states.last() != Some(&state) {
                states.push(state);
            }
        }
    }
    assert_eq!(
        states,
        ["Busy", "Waiting", "Exited"],
        "the agent's states as `zellij events` streamed them"
    );
}
