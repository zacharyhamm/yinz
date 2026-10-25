#![cfg(unix)]

//! A plugin gets a pane only when it declares one with `(yinz-declare-pane)`. One that does not
//! runs headless for its whole life: no pane record in any tab, the floating layer, a layout
//! slot or a multipane, however it was started.
//!
//! Every test here pairs the absence of a pane with positive evidence that the plugin did what
//! it was started for (a pipe answered, a REPL evaluated, a reload took effect), so none of them
//! can pass because nothing happened.

use std::path::{Path, PathBuf};
use std::time::Instant;
use yinz_cli::args::CliAction;
use yinz_integration_tests::{LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};
use yinz_types::cli::{PluginOperation, PluginState, PluginStatus};
use yinz_types::data::{PaneListEntry, PermissionType};

/// Answers a CLI pipe with its plugin id, a version marker and the payload, and never renders:
/// it has no `render`, and with no declaration it has nowhere to render to.
fn recipient_source(version: &str, declared: bool) -> String {
    let declaration = if declared {
        "(yinz-declare-pane)\n"
    } else {
        ""
    };
    let render = if declared {
        format!("(define (render rows columns) \"RECIPIENT PANE {version}\")\n")
    } else {
        String::new()
    };
    format!(
        r#"{declaration}(define (load configuration)
  (yinz-wait-for-permissions '("ReadCliPipes" "ChangeApplicationState")))
(define (pipe message)
  (define source (hash-ref message "source"))
  (when (and (hash? source) (hash-contains? source "Cli"))
    (yinz-command (hash "CliPipeOutput" (list (hash-ref source "Cli")
      (string-append "ANSWER " (number->string (hash-ref (yinz-command "GetPluginIds") "plugin_id"))
        " {version} " (hash-ref message "payload"))))))
  #f)
{render}"#
    )
}

/// Pre-grant `permissions` to the plugin at `url`, so no permission modal stands between the
/// test and the plugin's answer.
fn grant(url: &str, permissions: Vec<PermissionType>) {
    let cache_path = yinz_runtime::consts::YINZ_PLUGIN_PERMISSIONS_CACHE.clone();
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let mut cache =
        yinz_config::permission::PermissionCache::from_path_or_default(Some(cache_path));
    cache.cache(
        yinz_runtime::plugin_location::parse_plugin_location(url, None)
            .unwrap()
            .to_string(),
        permissions,
    );
    cache.write_to_file().unwrap();
}

/// A short directory for plugin sources: the location is part of the toasts the tests read,
/// and a long one wraps.
fn plugin_dir() -> tempfile::TempDir {
    yinz_integration_tests::test_env::init();
    tempfile::Builder::new().prefix("h").tempdir().unwrap()
}

fn write_recipient(dir: &Path, version: &str, declared: bool) -> (PathBuf, String) {
    let path = dir.join("r.scm");
    std::fs::write(&path, recipient_source(version, declared)).unwrap();
    let url = format!("file:{}", path.display());
    grant(
        &url,
        vec![
            PermissionType::ReadCliPipes,
            PermissionType::ChangeApplicationState,
        ],
    );
    (path, url)
}

/// A session of one terminal and no plugins at all, so any plugin pane is the one under test.
fn bare_session(extra_config: &str, layout: &str) -> TestSession {
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "load_plugins clear-defaults=true {{}}\n{extra_config}"
        ))
        .with_layout(LayoutInfo::Stringified(layout.into()))
        .start();
    yinz.expect_pty_spawn()
        .output(b"HEADLESS SESSION READY\r\n");
    yinz.wait_until("the session's terminal is ready", |grid| {
        grid.contains("HEADLESS SESSION READY")
    });
    yinz
}

fn cli_pipe(plugin: &str, payload: &str) -> CliAction {
    CliAction::Pipe {
        focus_plugin: false,
        name: Some("headless-pipe".into()),
        payload: Some(payload.into()),
        args: None,
        plugin: Some(plugin.into()),
        plugin_configuration: None,
        configuration_json: None,
        force_launch_plugin: false,
        skip_plugin_cache: false,
        floating_plugin: None,
        in_place_plugin: None,
        plugin_cwd: None,
        plugin_title: None,
    }
}

/// Pipe `payload` to `plugin` and return the plugin id and version it answered with.
fn pipe(yinz: &TestSession, plugin: &str, payload: &str) -> (u32, String) {
    let (status, output) = yinz.run_cli_action_reporting(cli_pipe(plugin, payload));
    assert_eq!(status, 0, "pipe to {plugin} failed: {output}");
    let words: Vec<_> = output.split_whitespace().collect();
    assert!(
        words.len() == 4 && words[0] == "ANSWER" && words[3] == payload,
        "the plugin did not answer the pipe: {output:?}"
    );
    (words[1].parse().unwrap(), words[2].to_owned())
}

/// Every pane in the session, suppressed ones included.
fn panes(yinz: &TestSession) -> Vec<PaneListEntry> {
    let (status, json) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(status, 0, "{json}");
    serde_json::from_str(&json).unwrap()
}

fn plugin_panes(yinz: &TestSession) -> Vec<PaneListEntry> {
    panes(yinz)
        .into_iter()
        .filter(|pane| pane.pane_info.is_plugin())
        .collect()
}

fn assert_no_plugin_pane(yinz: &TestSession, when: &str) {
    let plugin_panes = plugin_panes(yinz);
    assert!(
        plugin_panes.is_empty(),
        "{when}: a plugin pane exists: {plugin_panes:#?}"
    );
}

fn plugin_request(
    yinz: &TestSession,
    operation: PluginOperation,
    plugin: &Path,
) -> yinz_types::cli::PluginResponse {
    yinz_client::store_client::plugin_request(
        &yinz.session_name().into(),
        yinz_types::cli::PluginRequest {
            operation,
            target: Some(plugin.display().to_string()),
            configuration: None,
            fallback: Some(yinz_types::input::layout::RunPlugin {
                location: yinz_types::input::layout::RunPluginLocation::File(plugin.to_owned()),
                ..Default::default()
            }),
            candidates: vec![],
            caller: None,
        },
    )
    .unwrap()
}

/// `yinz plugins list`: every instance the session runs.
fn plugins_list(yinz: &TestSession) -> Vec<PluginStatus> {
    yinz_client::store_client::plugin_request(
        &yinz.session_name().into(),
        yinz_types::cli::PluginRequest {
            operation: PluginOperation::List,
            target: None,
            configuration: None,
            fallback: None,
            candidates: vec![],
            caller: None,
        },
    )
    .unwrap()
    .plugins
}

fn instances_of(yinz: &TestSession, location_part: &str) -> Vec<PluginStatus> {
    plugins_list(yinz)
        .into_iter()
        .filter(|status| status.location.contains(location_part))
        .collect()
}

/// Wait for `predicate` over the plugin list, polling the session: every reply is correlated,
/// so each one observes the plugin thread after its preceding work.
fn wait_for_plugins(
    yinz: &TestSession,
    what: &str,
    predicate: impl Fn(&[PluginStatus]) -> bool,
) -> Vec<PluginStatus> {
    let deadline = Instant::now() + yinz_integration_tests::default_timeout();
    loop {
        let list = plugins_list(yinz);
        if predicate(&list) {
            return list;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; plugins list: {list:#?}\n{}",
            yinz_integration_tests::test_env::log_tail(80)
        );
        std::thread::yield_now();
    }
}

/// Wait until `predicate` holds for the session's panes.
fn wait_for_panes(
    yinz: &TestSession,
    what: &str,
    predicate: impl Fn(&[PaneListEntry]) -> bool,
) -> Vec<PaneListEntry> {
    let deadline = Instant::now() + yinz_integration_tests::default_timeout();
    loop {
        let panes = panes(yinz);
        if predicate(&panes) {
            return panes;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; panes: {panes:#?}\n{}",
            yinz_integration_tests::test_env::log_tail(80)
        );
        std::thread::yield_now();
    }
}

#[test]
fn a_cold_pipe_to_an_undeclared_plugin_delivers_without_a_pane() {
    let dir = plugin_dir();
    let (_path, url) = write_recipient(dir.path(), "v1", false);
    let mut yinz = bare_session(
        &format!("plugins {{ recipient location=\"{url}\"; }}"),
        "layout { pane; }",
    );
    assert_no_plugin_pane(&yinz, "before the pipe");
    assert!(instances_of(&yinz, "r.scm").is_empty());

    let (id, _) = pipe(&yinz, "recipient", "cold");
    assert_no_plugin_pane(&yinz, "after the cold pipe started the plugin");
    let instances = instances_of(&yinz, "r.scm");
    assert_eq!(instances.len(), 1, "{instances:#?}");
    assert_eq!(instances[0].id.get(), id);
    assert_eq!(instances[0].state, PluginState::Running);
    assert_eq!(instances[0].pane, Some(false));

    // The headless instance is the one later pipes find, by alias, URL and id alike.
    for destination in ["recipient".to_owned(), url.clone(), format!("plugin_{id}")] {
        assert_eq!(pipe(&yinz, &destination, "warm").0, id, "{destination}");
    }
    assert_no_plugin_pane(&yinz, "after the warm pipes");
    assert_eq!(instances_of(&yinz, "r.scm").len(), 1);
    // Exactly the layout's terminal: no floating pane was opened for the recipient either.
    let all = panes(&yinz);
    assert_eq!(all.len(), 1, "{all:#?}");
    assert!(!all[0].pane_info.is_floating);
    yinz.quit();
}

#[test]
fn the_repl_leaves_no_plugin_pane_before_during_or_after() {
    let mut yinz = bare_session("", "layout { pane; }");
    assert_no_plugin_pane(&yinz, "before the REPL connects");
    let mut repl = yinz_client::repl::Client::connect(&yinz.session_name().into(), None).unwrap();
    let reply = repl.eval("(+ 40 2)").unwrap();
    assert!(
        reply.get("error").is_none() && reply.to_string().contains("42"),
        "the REPL did not evaluate: {reply}"
    );
    let repls = instances_of(&yinz, "repl");
    assert_eq!(repls.len(), 1, "{repls:#?}");
    assert_eq!(repls[0].state, PluginState::Running);
    assert_eq!(repls[0].pane, Some(false));
    assert_no_plugin_pane(&yinz, "while the REPL is connected");
    assert!(
        !yinz.snapshot().contains("Scheme REPL"),
        "the REPL must not float a pane:\n{}",
        yinz.snapshot().text
    );
    // A second expression reaches the same instance.
    let reply = repl.eval("(string-append \"head\" \"less\")").unwrap();
    assert!(reply.to_string().contains("headless"), "{reply}");
    assert_eq!(instances_of(&yinz, "repl").len(), 1);
    repl.close().unwrap();
    drop(repl);
    assert_no_plugin_pane(&yinz, "after the REPL disconnected");
    assert!(!String::from_utf8_lossy(&yinz.raw_bytes()).contains("Scheme REPL"));
    yinz.quit();
}

#[test]
fn closing_a_headless_plugin_by_id_unloads_it_and_returns() {
    let dir = plugin_dir();
    let (path, url) = write_recipient(dir.path(), "v1", false);
    let mut yinz = bare_session(
        &format!("plugins {{ recipient location=\"{url}\"; }}"),
        "layout { pane; }",
    );
    let (id, _) = pipe(&yinz, "recipient", "start");
    assert_eq!(
        plugin_request(&yinz, PluginOperation::Inspect, &path).plugins[0].state,
        PluginState::Running
    );
    // Before this was answered, a close of a headless id waited forever for a pane to close.
    let (status, output) = yinz.run_cli_action_reporting(CliAction::ClosePane {
        pane_id: Some(format!("plugin_{id}")),
    });
    assert_eq!(status, 0, "{output}");
    wait_for_plugins(&yinz, "the closed plugin to unload", |list| {
        !list.iter().any(|status| status.id.get() == id)
    });
    assert_no_plugin_pane(&yinz, "after the close");
    // A fresh pipe starts a new instance rather than finding the closed one.
    let (new_id, _) = pipe(&yinz, "recipient", "again");
    assert_ne!(new_id, id);
    assert_no_plugin_pane(&yinz, "after the restart");
    yinz.quit();
}

/// The layout this session starts with names an undeclared plugin: the slot closes, the user is
/// told why, and the plugin never runs.
#[test]
fn a_startup_layout_slot_of_an_undeclared_plugin_is_removed_with_a_toast() {
    let dir = plugin_dir();
    let (path, url) = write_recipient(dir.path(), "v1", false);
    // By alias: the toast names the location, and a short one keeps it on few rows.
    let mut yinz = bare_session(
        &format!("plugins {{ undeclared location=\"{url}\"; }}"),
        UNDECLARED_SLOT_LAYOUT,
    );
    let panes = wait_for_panes(&yinz, "the slot to be removed", |panes| {
        panes.len() == 1 && !panes[0].pane_info.is_plugin()
    });
    // The remaining terminal takes the whole tab.
    assert!(!panes[0].pane_info.is_floating);
    assert!(
        plugin_request(&yinz, PluginOperation::Inspect, &path)
            .plugins
            .is_empty(),
        "the aborted plugin must not be running"
    );
    assert!(instances_of(&yinz, "r.scm").is_empty());
    yinz.wait_until("the toast explains the removed slot", |grid| {
        grid.contains("undeclared declares no pane") && grid.contains("was removed")
    });
    yinz.quit();
}

const UNDECLARED_SLOT_LAYOUT: &str = "layout { pane; pane { plugin location=\"undeclared\"; }; }";

/// Each new tab from a layout naming an undeclared plugin drops its slot, without leaking an
/// instance per tab, explaining it more than once, or holding the tab's first render back.
#[test]
fn new_tabs_drop_undeclared_slots_without_holding_back_their_first_render() {
    let dir = plugin_dir();
    let (path, url) = write_recipient(dir.path(), "v1", false);
    let mut yinz = bare_session(
        &format!("plugins {{ undeclared location=\"{url}\"; }}"),
        "layout { pane; }",
    );
    // The render blocker holds a new tab's renders until its plugins render once or 100 ms pass.
    // The plugin thread reports the slot's plugin headless before the tab's layout lands, so the
    // slot is dropped and no blocker is ever registered for it. That is asserted on the screen's
    // state rather than timed here, in yinz-screen's `paneless_plugins` unit tests: one for this
    // order, and one for a report that trails the layout and must release the blocker.
    for tab in 0..4 {
        assert_eq!(
            yinz.run_cli_action(CliAction::NewTab {
                name: Some(format!("headless-{tab}")),
                layout: None,
                layout_string: Some(UNDECLARED_SLOT_LAYOUT.into()),
                layout_dir: None,
                cwd: None,
                initial_command: vec![],
                initial_plugin: None,
                close_on_exit: false,
                start_suspended: false,
                block_until_exit_success: false,
                block_until_exit_failure: false,
                block_until_exit: false,
                no_focus: false,
            }),
            0
        );
        let terminal = yinz.expect_pty_spawn();
        let marker = format!("TAB {tab} TERMINAL READY");
        terminal.output(format!("{marker}\r\n").as_bytes());
        yinz.wait_until("the new tab's terminal renders", |grid| {
            grid.contains(&marker)
        });
        wait_for_panes(&yinz, "the new tab's slot to be removed", |panes| {
            panes.len() == tab + 2 && panes.iter().all(|pane| !pane.pane_info.is_plugin())
        });
        yinz.wait_until("the toast explains the removed slot", |grid| {
            grid.contains("undeclared declares no pane") && grid.contains("was removed")
        });
    }
    assert!(
        plugin_request(&yinz, PluginOperation::Inspect, &path)
            .plugins
            .is_empty(),
        "an aborted plugin must not be running"
    );
    assert!(instances_of(&yinz, "r.scm").is_empty());
    // The toast waits to be dismissed, so a second one for the same location would be on screen.
    let grid = yinz.snapshot();
    assert_eq!(
        grid.text.matches("declares no pane").count(),
        1,
        "the slot must be explained once per location:\n{}",
        grid.text
    );
    yinz.quit();
}

#[test]
fn a_reload_flips_between_a_pane_and_headless_both_ways() {
    let dir = plugin_dir();
    let (path, url) = write_recipient(dir.path(), "v1", true);
    let mut yinz = bare_session(
        &format!("plugins {{ recipient location=\"{url}\"; }}"),
        "layout { pane; }",
    );
    let (status, output) = yinz.run_cli_action_reporting(CliAction::LaunchPlugin {
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        url: url.parse().unwrap(),
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        no_focus: false,
        tab_id: None,
    });
    assert_eq!(status, 0, "{output}");
    yinz.wait_until("the declared plugin renders in its pane", |grid| {
        grid.contains("RECIPIENT PANE v1")
    });
    let plugin_pane = plugin_panes(&yinz);
    assert_eq!(plugin_pane.len(), 1, "{plugin_pane:#?}");
    let (id, _) = plugin_pane[0].pane_info.id.legacy_parts();
    assert_eq!(pipe(&yinz, "recipient", "declared"), (id, "v1".into()));

    // Declared to undeclared: the pane closes and the plugin keeps running without it.
    std::fs::write(&path, recipient_source("v2", false)).unwrap();
    let reloaded = plugin_request(&yinz, PluginOperation::Reload, &path);
    assert_eq!(reloaded.plugins[0].id.get(), id, "{reloaded:?}");
    wait_for_panes(&yinz, "the pane to close after the reload", |panes| {
        panes.iter().all(|pane| !pane.pane_info.is_plugin())
    });
    wait_for_plugins(&yinz, "the reloaded plugin to run headless", |list| {
        list.iter().any(|status| {
            status.id.get() == id
                && status.state == PluginState::Running
                && status.pane == Some(false)
        })
    });
    assert_eq!(pipe(&yinz, "recipient", "headless"), (id, "v2".into()));
    assert_no_plugin_pane(&yinz, "while headless");
    yinz.wait_until("the closed pane is gone from the screen", |grid| {
        !grid.contains("RECIPIENT PANE")
    });

    // Undeclared to declared: a pane comes back, floating and unfocused.
    std::fs::write(&path, recipient_source("v3", true)).unwrap();
    plugin_request(&yinz, PluginOperation::Reload, &path);
    let panes = wait_for_panes(&yinz, "the pane to return after the reload", |panes| {
        panes
            .iter()
            .any(|pane| pane.pane_info.id == yinz_types::pane_id::PaneId::from_legacy(id, true))
    });
    let returned = panes
        .iter()
        .find(|pane| pane.pane_info.is_plugin())
        .unwrap();
    assert!(returned.pane_info.is_floating, "{returned:#?}");
    assert!(!returned.pane_info.is_focused, "{returned:#?}");
    assert_eq!(
        panes
            .iter()
            .filter(|pane| pane.pane_info.is_plugin())
            .count(),
        1
    );
    assert_eq!(
        pipe(&yinz, "recipient", "declared-again"),
        (id, "v3".into())
    );
    let status = plugin_request(&yinz, PluginOperation::Inspect, &path);
    assert_eq!(status.plugins[0].pane, Some(true), "{status:?}");
    yinz.quit();
}

#[test]
fn start_or_reload_headless_plugin_restores_its_requested_tab_without_a_client_id() {
    let dir = plugin_dir();
    let (path, url) = write_recipient(dir.path(), "v1", false);
    let mut yinz = bare_session("", "layout { pane; }");
    let destination = panes(&yinz)[0].tab_id;
    assert_eq!(
        yinz.run_cli_action(CliAction::StartOrReloadPlugin {
            url: url.clone(),
            configuration: None,
            configuration_json: None,
        }),
        0
    );
    let (id, version) = pipe(&yinz, &url, "headless");
    assert_eq!(version, "v1");
    assert_no_plugin_pane(&yinz, "after start-or-reload starts a headless plugin");

    std::fs::write(&path, recipient_source("v2", true)).unwrap();
    assert_eq!(
        yinz.run_cli_action(CliAction::StartOrReloadPlugin {
            url: url.clone(),
            configuration: None,
            configuration_json: None,
        }),
        0
    );
    let panes = wait_for_panes(&yinz, "a pane in the requested tab", |panes| {
        panes
            .iter()
            .any(|pane| pane.pane_info.id == yinz_types::pane_id::PaneId::from_legacy(id, true))
    });
    let restored: Vec<_> = panes
        .iter()
        .filter(|pane| pane.pane_info.is_plugin())
        .collect();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].tab_id, destination);
    assert!(restored[0].pane_info.is_floating);
    assert_eq!(pipe(&yinz, &url, "restored"), (id, "v2".into()));
    yinz.quit();
}

#[test]
fn plugins_list_reports_each_instances_pane_declaration() {
    let dir = plugin_dir();
    let declared_dir = dir.path().join("d");
    std::fs::create_dir(&declared_dir).unwrap();
    let (declared, _) = write_recipient(&declared_dir, "v1", true);
    let (undeclared, _) = write_recipient(dir.path(), "v1", false);
    let mut yinz = bare_session("", "layout { pane; }");
    for path in [&declared, &undeclared] {
        let started = plugin_request(&yinz, PluginOperation::Start, path);
        assert_eq!(started.plugins.len(), 1, "{started:?}");
    }
    // Other services (the session's agent provider) may be listed too; count only these two.
    let list = wait_for_plugins(&yinz, "both plugins to run", |list| {
        list.iter()
            .filter(|status| {
                status.location.ends_with("r.scm") && status.state == PluginState::Running
            })
            .count()
            == 2
    });
    let pane_of = |path: &Path| {
        list.iter()
            .find(|status| status.location.ends_with(&path.display().to_string()))
            .unwrap_or_else(|| panic!("{} is not listed: {list:#?}", path.display()))
            .pane
    };
    assert_eq!(pane_of(&declared), Some(true));
    assert_eq!(pane_of(&undeclared), Some(false));
    // The declared background start is parked as a suppressed pane; the undeclared one has none.
    let plugin_panes = plugin_panes(&yinz);
    assert_eq!(plugin_panes.len(), 1, "{plugin_panes:#?}");
    assert!(plugin_panes[0].pane_info.is_suppressed);
    assert!(
        plugin_panes[0]
            .pane_info
            .plugin_url
            .as_deref()
            .is_some_and(|url| url.ends_with(&declared.display().to_string())),
        "{plugin_panes:#?}"
    );
    yinz.quit();
}

/// A multipane's owner loads an undeclared plugin into one of its slots. The plugin is never
/// run, the user is told, and the slot goes away: it must not linger as an empty or exited
/// member beside the owner.
#[test]
fn an_undeclared_multipane_child_closes_its_slot() {
    let dir = plugin_dir();
    let child = dir.path().join("c.scm");
    std::fs::write(&child, recipient_source("child", false)).unwrap();
    let owner = dir.path().join("o.scm");
    std::fs::write(
        &owner,
        r#"
(yinz-declare-pane)
(define state "requesting composition") (define container #f)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ChangeApplicationState" "RunCommands" "OpenTerminalsOrPlugins" "ReadApplicationState"))
    (lambda (_)
      (define own (hash "Plugin" (hash-ref (yinz-command "GetPluginIds") "plugin_id")))
      (define placement (hash "kind" "Dismissible" "coordinates"
        (hash "x" (hash "Fixed" 3) "y" (hash "Fixed" 3) "width" (hash "Fixed" 90) "height" (hash "Fixed" 18))))
      (set! container (yinz-command (hash "CreateMultipane" (hash "children" (list (hash "Existing" own) (hash "Terminal" (hash "path" "multipane-command"))) "placement" placement "split" (hash "direction" "LeftRight" "ratio" 5000) "focus" 0))))
      (yinz-command (hash "Subscribe" '("Key"))) (set! state "COMPOSITION OWNER"))))
(define (update event)
  (define key (and (hash? event) (hash-contains? event "Key") (let ([bare (hash-ref (hash-ref event "Key") "bare_key")]) (and (hash? bare) (and (hash-contains? bare "Char") (hash-ref bare "Char"))))))
  (cond [(equal? key "u")
         (yinz-command (hash "ReplaceMultipaneChild" (hash "id" (hash-ref container "id") "slot" 1 "child" (hash "Plugin" (hash "url" "CHILD_URL" "configuration" (hash))))))
         (set! state "OWNER AFTER CHILD") #t]
        [else #f]))
(define (render rows columns) state)
"#
        // By alias: the toast names the location, and a short one keeps it on few rows.
        .replace("CHILD_URL", "child"),
    )
    .unwrap();
    let owner_url = format!("file:{}", owner.display());
    grant(
        &owner_url,
        vec![
            PermissionType::ChangeApplicationState,
            PermissionType::RunCommands,
            PermissionType::OpenTerminalsOrPlugins,
            PermissionType::ReadApplicationState,
        ],
    );
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "load_plugins clear-defaults=true {{}}\nplugins {{ child location=\"file:{}\"; }}",
            child.display()
        ))
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n pane\n pane focus=true {{plugin location=\"{owner_url}\";}}\n}}"
        )))
        .start();
    yinz.expect_pty_spawn().output(b"BASE SHELL");
    let command = yinz.expect_pty_spawn();
    command.wait_for_command("multipane-command");
    command.output(b"LAUNCHED IN SLOT");
    yinz.wait_until("both slots of the composition render", |grid| {
        grid.contains("COMPOSITION OWNER") && grid.contains("LAUNCHED IN SLOT")
    });
    let list = || -> Vec<serde_json::Value> {
        let (code, output) = yinz.run_cli_action_reporting(CliAction::ListPanes {
            tab: false,
            command: false,
            state: true,
            geometry: true,
            all: true,
            json: true,
        });
        assert_eq!(code, 0, "{output}");
        serde_json::from_str(&output).unwrap()
    };
    let members: Vec<_> = list()
        .into_iter()
        .filter(|pane| pane.get("multipane").is_some())
        .collect();
    assert_eq!(members.len(), 2, "{members:#?}");
    let container = members[0]["multipane"]["id"].clone();
    let owner_id = members
        .iter()
        .find(|pane| pane["is_plugin"] == true)
        .unwrap()["id"]
        .clone();

    yinz.send_stdin(b"u");
    yinz.wait_until(
        "the owner placed the child and the toast explains it",
        |grid| grid.contains("OWNER AFTER CHILD") && grid.contains("child declares no pane"),
    );
    let deadline = Instant::now() + yinz_integration_tests::default_timeout();
    loop {
        let panes = list();
        let slot_gone = !panes.iter().any(|pane| {
            pane["multipane"]["id"] == container && pane["id"] != owner_id
                || pane["is_plugin"] == false && (pane["exited"] == true || pane["is_held"] == true)
        });
        if slot_gone {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the undeclared child's slot was left behind beside its owner: {panes:#?}"
        );
        std::thread::yield_now();
    }
    let panes = list();
    assert!(
        panes
            .iter()
            .any(|pane| pane["is_plugin"] == true && pane["id"] == owner_id),
        "the owner must survive its child's removal: {panes:#?}"
    );
    assert!(!panes.iter().any(|pane| pane["plugin_url"]
        .as_str()
        .is_some_and(|url| url.ends_with("c.scm"))));
    assert!(
        instances_of(&yinz, "c.scm").is_empty(),
        "the aborted child must not be running"
    );
    yinz.wait_until("the owner still renders", |grid| {
        grid.contains("OWNER AFTER CHILD") && !grid.contains("LAUNCHED IN SLOT")
    });
    yinz.quit();
}

/// `yinz plugin -- <undeclared>` succeeds headless and says so, and still prints the id that
/// addresses the plugin, last, where a script reading the launch's id finds it.
#[test]
fn launching_an_undeclared_plugin_prints_the_headless_note_and_its_id() {
    let dir = plugin_dir();
    let (_path, url) = write_recipient(dir.path(), "v1", false);
    let mut yinz = bare_session("", "layout { pane; }");
    let launch = || CliAction::LaunchPlugin {
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        url: url.parse().unwrap(),
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        no_focus: false,
        tab_id: None,
    };
    let (status, output) = yinz.run_cli_action_reporting(launch());
    assert_eq!(status, 0, "{output}");
    let lines: Vec<_> = output.lines().collect();
    assert_eq!(lines.len(), 2, "{output:?}");
    assert!(
        lines[0].ends_with("runs headless: it declares no pane"),
        "{output:?}"
    );
    let id: u32 = lines[1]
        .strip_prefix("plugin_")
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("the last line is not a plugin id: {output:?}"));
    // The id is the running headless instance's, and it has no pane.
    assert_eq!(pipe(&yinz, &format!("plugin_{id}"), "launched").0, id);
    assert_no_plugin_pane(&yinz, "after the launch");
    yinz.quit();
}

#[test]
fn headless_symlink_reload_keeps_launch_or_focus_on_the_existing_instance() {
    let dir = plugin_dir();
    let (first, _) = write_recipient(dir.path(), "v1", false);
    let current = dir.path().join("current.scm");
    std::os::unix::fs::symlink(&first, &current).unwrap();
    let url = format!("file:{}", current.display());
    let mut yinz = bare_session("", "layout { pane; }");
    let launch = || CliAction::LaunchOrFocusPlugin {
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        move_to_focused_tab: false,
        url: url.clone(),
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        tab_id: None,
    };
    let (status, output) = yinz.run_cli_action_reporting(launch());
    assert_eq!(status, 0, "{output}");
    let (id, version) = pipe(&yinz, &url, "initial");
    assert_eq!(version, "v1");
    let target = format!("plugin_{id}");
    assert_eq!(output.lines().last(), Some(target.as_str()));

    let second = dir.path().join("v2.scm");
    std::fs::write(&second, recipient_source("v2", false)).unwrap();
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(&second, &current).unwrap();
    grant(
        &url,
        vec![
            PermissionType::ReadCliPipes,
            PermissionType::ChangeApplicationState,
        ],
    );
    let reloaded = plugin_request(&yinz, PluginOperation::Reload, &current);
    assert_eq!(reloaded.plugins.len(), 1, "{reloaded:?}");
    assert_eq!(reloaded.plugins[0].id.get(), id, "{reloaded:?}");
    assert_eq!(pipe(&yinz, &target, "reloaded"), (id, "v2".into()));

    let (status, output) = yinz.run_cli_action_reporting(launch());
    assert_eq!(status, 0, "{output}");
    assert_eq!(output.lines().last(), Some(target.as_str()), "{output}");
    let instances = instances_of(&yinz, "v2.scm");
    assert_eq!(instances.len(), 1, "{instances:?}");
    assert_eq!(instances[0].id.get(), id);
    assert_no_plugin_pane(&yinz, "after the symlink reload and relaunch");
    yinz.quit();
}
