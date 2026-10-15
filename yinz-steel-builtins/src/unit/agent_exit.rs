use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};
use yinz_steel::Runtime;
use yinz_types::data::Event;

use crate::tests::{plugin_with_prelude, rendered_text, Host};

const HEAD: &str = "0123456789012345678901234567890123456789";

fn listing(secondary: bool) -> String {
    format!(
        "worktree /repo\0HEAD {HEAD}\0branch refs/heads/main\0\0{}",
        if secondary {
            format!("worktree /repo/topic\0HEAD {HEAD}\0branch refs/heads/topic\0\0")
        } else {
            String::new()
        }
    )
}

struct Exit {
    runtime: Runtime,
    host: Arc<Mutex<Host>>,
}

impl Exit {
    fn load(config: Value, project: Value) -> Self {
        let (runtime, host) = plugin_with_prelude(
            "agent-exit",
            config,
            BTreeMap::from([("ProjectConfigRead".to_owned(), project)]),
            "(define fake-now (vector 0)) (define (yinz-now-ms) (vector-ref fake-now 0))",
        );
        Self { runtime, host }
    }

    fn new(target: &str, listed: &str) -> Self {
        let mut plugin = Self::load(
            json!({"agent_cwd":"/repo/topic", "agent_target": target, "agent_pane_id":"7"}),
            json!({"config":{"default_branch":"main"},"error":null}),
        );
        plugin.reply("/repo/topic\n", 0, "");
        plugin.reply(listed, 0, "");
        plugin
    }

    fn commands(&self) -> Vec<Value> {
        self.host.lock().unwrap().commands.clone()
    }
    fn last_run(&self) -> Value {
        self.commands()
            .into_iter()
            .rev()
            .find_map(|command| command.get("RunCommand").cloned())
            .unwrap()
    }
    fn argv(&self) -> Value {
        self.last_run()[0].clone()
    }
    fn event(&mut self, event: Event) {
        self.runtime
            .update(serde_json::to_value(event).unwrap())
            .unwrap();
    }
    fn key(&mut self, key: &str) {
        self.modified_key(key, &[]);
    }
    fn modified_key(&mut self, key: &str, modifiers: &[&str]) {
        self.event(
            serde_json::from_value(json!({"Key":{"bare_key":key,"key_modifiers":modifiers}}))
                .unwrap(),
        );
    }
    fn reply(&mut self, stdout: &str, code: i32, stderr: &str) {
        let context = serde_json::from_value(self.last_run()[3].clone()).unwrap();
        self.event(Event::RunCommandResult(
            Some(code),
            stdout.as_bytes().to_vec(),
            stderr.as_bytes().to_vec(),
            context,
        ));
    }
    fn render(&mut self, rows: usize, cols: usize) -> String {
        rendered_text(&self.runtime.render(rows, cols).unwrap())
    }
    fn prepare(&mut self, listed: &str) {
        let before = self.commands().len();
        self.key("Delete");
        if self.commands().len() > before {
            self.reply(listed, 0, "");
        }
        assert!(self.render(24, 200).contains("Delete again"));
    }
    fn confirm(&mut self, listed: &str) {
        self.prepare(listed);
        self.key("Delete");
        if self.argv().as_array().unwrap().contains(&json!("fetch")) {
            self.reply("", 128, "origin unavailable");
        }
    }
    fn clean(&mut self) {
        assert_eq!(
            self.argv(),
            json!([
                "git",
                "-C",
                "/repo/topic",
                "status",
                "--porcelain",
                "--untracked-files=all"
            ])
        );
        self.reply("", 0, "");
        assert_eq!(self.argv()[3], "rev-list");
        self.reply("", 0, "");
    }
    fn preserved(&self) {
        let commands = serde_json::to_string(&self.commands()).unwrap();
        assert!(!commands.contains("\"worktree\",\"remove\""));
        assert!(!commands.contains("\"update-ref\""));
        assert!(!self.commands().contains(&json!("CloseSelf")));
    }
    fn advance(&mut self, now: usize) {
        self.runtime
            .eval(&format!("(vector-set! fake-now 0 {now})"))
            .unwrap();
        self.event(Event::Timer(yinz_types::units::FractionalSeconds::new(0.0)));
    }
}

#[test]
fn merge_proof_requires_local_or_freshly_fetched_base() {
    // fetch failure, local proof, local failure, remote proof, remote failure
    for (fetch_fails, local, local_fails, remote, remote_fails) in [
        (false, false, false, true, false),
        (false, true, false, false, false),
        (false, false, false, false, false),
        (true, true, false, false, false),
        (true, false, false, true, false),
        (false, false, true, true, false),
        (false, false, true, false, true),
    ] {
        let mut plugin = Exit::new("local", &listing(true));
        plugin.prepare(&listing(true));
        assert!(!serde_json::to_string(&plugin.commands())
            .unwrap()
            .contains("\"fetch\""));
        plugin.key("Delete");
        assert_eq!(
            plugin.argv(),
            json!([
                "git",
                "-C",
                "/repo",
                "fetch",
                "--no-tags",
                "origin",
                "+refs/heads/main:refs/remotes/origin/main"
            ])
        );
        plugin.reply("", if fetch_fails { 128 } else { 0 }, "fetch unavailable");
        plugin.reply(&listing(true), 0, "");
        plugin.reply("", 0, "");
        plugin.reply(
            if local { "" } else { HEAD },
            if local_fails { 128 } else { 0 },
            "local unavailable",
        );
        if !local && !fetch_fails {
            assert_eq!(
                plugin.argv()[5],
                format!("refs/remotes/origin/main..{HEAD}")
            );
            plugin.reply(
                if remote { "" } else { HEAD },
                if remote_fails { 128 } else { 0 },
                "remote unavailable",
            );
        }
        if local || (!fetch_fails && remote) {
            assert_eq!(
                plugin.argv(),
                json!([
                    "git",
                    "-C",
                    "/repo",
                    "worktree",
                    "remove",
                    "--",
                    "/repo/topic"
                ])
            );
            plugin.reply("", 0, "");
            assert_eq!(
                plugin.argv(),
                json!([
                    "git",
                    "-C",
                    "/repo",
                    "update-ref",
                    "-d",
                    "refs/heads/topic",
                    HEAD
                ])
            );
            plugin.reply("", 0, "");
            assert!(plugin
                .commands()
                .ends_with(&[json!({"CloseTerminalPane":7}), json!("CloseSelf")]));
        } else {
            let output = plugin.render(30, 200);
            assert!(
                output.contains("local main or freshly fetched origin/main"),
                "{output}"
            );
            assert!(output.contains("worktree and branch preserved"));
            if fetch_fails {
                assert!(output.contains("fetch unavailable"));
            }
            plugin.preserved();
        }
        if fetch_fails {
            assert!(!serde_json::to_string(&plugin.commands())
                .unwrap()
                .contains(&format!("refs/remotes/origin/main..{HEAD}")));
        }
    }
}

#[test]
fn fetch_deadline_ignores_late_results_and_never_uses_stale_remote_proof() {
    for local_proof in [true, false] {
        let mut plugin = Exit::new("local", &listing(true));
        plugin.prepare(&listing(true));
        plugin.key("Delete");
        let fetch = plugin.last_run();
        plugin.advance(9999);
        assert_eq!(plugin.last_run(), fetch);
        let count = plugin.commands().len();
        plugin.key("Esc");
        assert_eq!(plugin.commands().len(), count);
        plugin.advance(10000);
        assert_eq!(plugin.argv()[3], "worktree");
        let count = plugin.commands().len();
        plugin.event(Event::RunCommandResult(
            Some(0),
            vec![],
            vec![],
            serde_json::from_value(fetch[3].clone()).unwrap(),
        ));
        assert_eq!(plugin.commands().len(), count);
        plugin.reply(&listing(true), 0, "");
        plugin.reply("", 0, "");
        assert_eq!(plugin.argv()[5], format!("refs/heads/main..{HEAD}"));
        plugin.reply(if local_proof { "" } else { HEAD }, 0, "");
        if local_proof {
            plugin.reply("", 0, "");
            plugin.reply("", 0, "");
            assert_eq!(plugin.commands().last(), Some(&json!("CloseSelf")));
        } else {
            let failure = plugin.render(30, 200);
            assert!(failure.contains("Fetch origin/main: Error: git timed out after 10000 ms"));
            plugin.preserved();
            plugin.event(Event::RunCommandResult(
                Some(0),
                vec![],
                vec![],
                serde_json::from_value(fetch[3].clone()).unwrap(),
            ));
            assert_eq!(plugin.render(30, 200), failure);
            plugin.prepare(&listing(true));
            plugin.key("Esc");
            assert!(!plugin.commands().contains(&json!("CloseSelf")));
            plugin.key("Esc");
            assert!(plugin
                .commands()
                .ends_with(&[json!({"CloseTerminalPane":7}), json!("CloseSelf")]));
        }
    }
}

#[test]
fn completed_fetch_cancels_deadline_for_success_and_failure() {
    for code in [0, 128] {
        let mut plugin = Exit::new("local", &listing(true));
        plugin.prepare(&listing(true));
        plugin.key("Delete");
        plugin.reply("", code, "");
        let count = plugin.commands().len();
        plugin.advance(9999);
        plugin.advance(10000);
        assert_eq!(plugin.commands().len(), count);
        plugin.reply(&listing(true), 0, "");
        plugin.clean();
        plugin.reply("", 0, "");
        plugin.reply("", 0, "");
        assert_eq!(plugin.commands().last(), Some(&json!("CloseSelf")));
    }
}

#[test]
fn preserves_dirty_changed_or_disappeared_checkouts_in_both_local_targets() {
    for target in ["local", "bubblewrap"] {
        for change in [
            "head",
            "branch",
            "missing",
            " M tracked\n",
            "?? untracked\n",
            "M  staged\n",
        ] {
            let mut plugin = Exit::new(target, &listing(true));
            plugin.confirm(&listing(true));
            let listed = match change {
                "head" => listing(true).replace(HEAD, &"1".repeat(40)),
                "branch" => listing(true).replace("refs/heads/topic", "refs/heads/other"),
                "missing" => listing(false),
                _ => listing(true),
            };
            plugin.reply(&listed, 0, "");
            let expected = match change {
                "head" | "branch" => "identity changed",
                "missing" => "worktree disappeared",
                _ => {
                    plugin.reply(change, 0, "");
                    "uncommitted changes"
                },
            };
            assert!(plugin.render(24, 200).contains(expected));
            plugin.preserved();
        }
    }
}

#[test]
fn partial_cleanup_retains_diagnostics_disables_restart_and_retries_only_branch() {
    let mut plugin = Exit::new("local", &listing(true));
    plugin.confirm(&listing(true));
    plugin.reply(&listing(true), 0, "");
    plugin.clean();
    plugin.reply("", 0, "");
    plugin.reply(
        "",
        128,
        "cannot lock ref: is at another head but expected original",
    );
    let failure = plugin.render(24, 200);
    assert!(failure.contains("cannot lock ref"));
    assert!(failure.contains("Worktree removed; branch cleanup remains."));
    assert!(!failure.contains("restart agent"));
    let count = plugin.commands().len();
    plugin.key("Enter");
    assert_eq!(plugin.commands().len(), count);
    plugin.prepare(&listing(true));
    plugin.key("Esc");
    assert!(plugin.render(24, 200).contains("cannot lock ref"));
    plugin.confirm(&listing(true));
    let retry = plugin.commands()[count..]
        .iter()
        .filter(|command| command.get("Subscribe").is_none())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(retry.len(), 1, "retry only the branch deletion");
    assert_eq!(
        plugin.argv(),
        json!([
            "git",
            "-C",
            "/repo",
            "update-ref",
            "-d",
            "refs/heads/topic",
            HEAD
        ])
    );
    plugin.reply("", 0, "");
    assert_eq!(plugin.commands().last(), Some(&json!("CloseSelf")));
}

#[test]
fn detached_checkout_never_deletes_a_branch() {
    let detached = listing(true).replace("branch refs/heads/topic", "detached");
    let mut plugin = Exit::new("local", &detached);
    plugin.prepare(&detached);
    assert!(plugin.render(24, 200).contains("detached HEAD"));
    plugin.key("Delete");
    plugin.reply("", 128, "origin unavailable");
    plugin.reply(&detached, 0, "");
    plugin.clean();
    plugin.reply("", 0, "");
    assert_eq!(plugin.commands().last(), Some(&json!("CloseSelf")));
    assert!(!serde_json::to_string(&plugin.commands())
        .unwrap()
        .contains("update-ref"));
}

#[test]
fn confirmation_refreshes_exact_identity_and_must_be_fully_visible() {
    let changed = listing(true)
        .replace(HEAD, &"1".repeat(40))
        .replace("refs/heads/topic", "refs/heads/rebased");
    let mut plugin = Exit::new("local", &listing(true));
    plugin.key("Delete");
    assert!(plugin.render(24, 200).contains("Checking current checkout"));
    let count = plugin.commands().len();
    plugin.key("Delete");
    assert_eq!(plugin.commands().len(), count);
    plugin.reply(&changed, 0, "");
    plugin.key("Delete");
    assert_eq!(plugin.commands().len(), count);
    for (rows, cols) in [(1, 1), (2, 8), (8, 24), (24, 0)] {
        plugin.render(rows, cols);
        plugin.key("Delete");
        assert_eq!(plugin.commands().len(), count);
    }
    let clipped = plugin.render(8, 24);
    assert!(clipped.contains("Deletion disabled"));
    assert!(clipped.contains("Esc: cancel"));
    let confirmation = plugin.render(24, 80);
    assert!(confirmation.contains(&"1".repeat(40)));
    assert!(confirmation.contains("Branch: rebased"));
    plugin.key("Delete");
    plugin.reply("", 128, "origin unavailable");
    plugin.reply(&changed, 0, "");
    plugin.clean();
    plugin.reply("", 0, "");
    assert_eq!(
        plugin.argv(),
        json!([
            "git",
            "-C",
            "/repo",
            "update-ref",
            "-d",
            "refs/heads/rebased",
            "1".repeat(40)
        ])
    );
    plugin.reply("", 0, "");
    assert_eq!(plugin.commands().last(), Some(&json!("CloseSelf")));
}

#[test]
fn fallback_keys_and_modifiers_do_not_offer_unsafe_cleanup() {
    for (key, command) in [("Enter", "RerunCommandPane"), ("Esc", "CloseTerminalPane")] {
        let mut plugin = Exit::load(json!({"agent_pane_id":"7"}), json!(null));
        assert!(!plugin.render(24, 200).contains("Delete worktree"));
        let count = plugin.commands().len();
        plugin.key("Delete");
        plugin.modified_key(key, &["Ctrl"]);
        assert_eq!(plugin.commands().len(), count);
        plugin.key(key);
        assert!(plugin
            .commands()
            .ends_with(&[json!({command:7}), json!("CloseSelf")]));
    }
    for target in ["ssh", "unknown"] {
        let mut plugin = Exit::load(
            json!({"agent_cwd":"/repo/topic", "agent_target":target}),
            json!({"config":{}}),
        );
        assert!(!plugin.render(24, 200).contains("Delete worktree"));
        assert!(!plugin
            .commands()
            .iter()
            .any(|command| command.get("RunCommand").is_some()));
        if target == "ssh" {
            assert!(!serde_json::to_string(&plugin.commands())
                .unwrap()
                .contains("ProjectConfigRead"));
        }
    }
}

#[test]
fn primary_or_missing_repository_never_offers_delete_and_project_errors_remain_visible() {
    for project in [
        json!({"config":{},"error":"malformed project config"}),
        json!(null),
    ] {
        let mut plugin = Exit::load(
            json!({"agent_cwd":"/repo/topic","agent_target":"local"}),
            project.clone(),
        );
        plugin.reply("/repo/topic\n", 0, "");
        plugin.reply(&listing(true), 0, "");
        let output = plugin.render(24, 200);
        assert!(output.contains("Delete worktree"));
        assert!(output.contains(if project.is_null() {
            "Error:"
        } else {
            "malformed project config"
        }));
    }
    for repository in [true, false] {
        let mut plugin = Exit::load(
            json!({"agent_cwd":"/repo","agent_target":"local"}),
            json!({"config":{}}),
        );
        plugin.reply(
            "/repo\n",
            if repository { 0 } else { 128 },
            "not a repository",
        );
        if repository {
            plugin.reply(&listing(false), 0, "");
        }
        assert!(!plugin.render(24, 200).contains("Delete worktree"));
    }
}

#[test]
fn configured_base_and_project_read_failures_preserve_worktree() {
    for read_failure in [false, true] {
        let mut plugin = Exit::new("bubblewrap", &listing(true));
        plugin.host.lock().unwrap().replies.insert("ProjectConfigRead".to_owned(),
            json!({"config":{"default_branch":"trunk"},"error": if read_failure {Some("invalid configuration")} else {None}}));
        plugin.confirm(&listing(true));
        if !read_failure {
            plugin.reply(&listing(true), 0, "");
            plugin.reply("", 0, "");
            assert_eq!(plugin.argv()[5], format!("refs/heads/trunk..{HEAD}"));
            plugin.reply(HEAD, 0, "");
        }
        assert!(plugin.render(24, 200).contains(if read_failure {
            "invalid configuration"
        } else {
            "merged into local trunk"
        }));
        plugin.preserved();
    }
}

#[test]
fn wrap_preserves_terminal_columns_indentation_and_blank_lines() {
    let mut runtime = Runtime::new(|_| Ok(Value::Null)).unwrap();
    for (text, width, expected) in [
        ("", 10, vec![""]),
        ("aaa bbb ccc", 7, vec!["aaa bbb", "ccc"]),
        ("one\n\ntwo", 20, vec!["one", "", "two"]),
        ("    indented", 40, vec!["    indented"]),
        ("abc defghijk", 4, vec!["abc", "defg", "hijk"]),
        ("界界界", 4, vec!["界界", "界"]),
        ("a\tb", 8, vec!["a    b"]),
        ("a  ", 2, vec!["a"]),
    ] {
        runtime
            .eval(&format!(
                "(define wrapped (yinz-ui-wrap {} {width}))",
                serde_json::to_string(text).unwrap()
            ))
            .unwrap();
        assert_eq!(runtime.value("wrapped").unwrap(), json!(expected));
    }
}
