use super::{host, run};
use crate::plugin_tests::runtime_parity::SteelReference;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use yinz_terminal::panes::grid::Grid;
use yinz_types::{data::Event, units::FractionalSeconds};

struct Panel {
    plugin: SteelReference,
    clock: Arc<AtomicU64>,
}
impl Panel {
    fn new() -> Self {
        let clock = Arc::new(AtomicU64::new(90_000));
        let (_, responder) = host(&[], clock.clone());
        let mut plugin = SteelReference::with_clock(
            "agents",
            BTreeMap::from([("view".into(), "agents".into())]),
            Path::new("/work"),
            responder,
            Some(clock.clone()),
        );
        plugin
            .runtime
            .eval("(define (usage-test-set! key value) (a-set! key value) (void))")
            .unwrap();
        let mut panel = Self { plugin, clock };
        panel.set("rows", json!([row(7, "codex"), row(8, "claude")]));
        panel.set(
            "snapshot",
            json!({"panes": [
            {"pane_id":7,"title":"Review code","command":"codex","status":"Running"},
            {"pane_id":8,"title":"Fix tests","command":"claude","status":"Running"}
        ], "runs":[], "hooks":{}, "activity":{}, "summaries":{}}),
        );
        panel.requests();
        panel
    }
    fn set(&mut self, key: &str, value: Value) {
        self.plugin
            .runtime
            .invoke("usage-test-set!", vec![json!(key), value])
            .unwrap();
    }
    fn show(&mut self) {
        self.plugin.update(&Event::Visible(true));
    }
    fn tick(&mut self, milliseconds: u64) {
        self.clock.fetch_add(milliseconds, Ordering::SeqCst);
        self.plugin
            .update(&Event::Timer(FractionalSeconds::new(2.0)));
    }
    fn key(&mut self, key: &str) {
        self.plugin.update(&Event::Key(key.parse().unwrap()));
    }
    fn requests(&self) -> Vec<Value> {
        let commands = self.plugin.take_commands();
        let requests: Vec<_> = commands
            .iter()
            .filter_map(|command| command.get("AgentControl").cloned())
            .collect();
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.get("Subscribe") == Some(&json!(["CustomMessage"])))
                .count(),
            requests.len(),
            "each usage request must subscribe for its correlated SDK reply"
        );
        requests
    }
    fn request(&self, provider: &str) -> Value {
        let requests = self.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert_eq!(
            serde_json::from_str::<Value>(requests[0]["request"].as_str().unwrap()).unwrap(),
            json!({"Usage":{"provider":provider}})
        );
        requests[0].clone()
    }
    fn reply(&mut self, request: &Value, usage: Value) -> bool {
        self.plugin.update(&Event::CustomMessage(
            "agent-response".into(),
            json!({"id":request["id"],"response":{"outcome":"ok","usage":usage}}).to_string(),
        ))
    }
    fn lines(&mut self) -> String {
        self.plugin
            .runtime
            .invoke("a-usage-lines", vec![])
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn observation(&mut self, provider: &str) -> Value {
        self.plugin
            .runtime
            .invoke("a-get", vec![json!("usage_cache")])
            .unwrap()[provider]["observation"]
            .clone()
    }
    fn render(&mut self, rows: usize, cols: usize) -> String {
        let mut grid = Grid::new(
            rows,
            cols,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            false,
            false,
            true,
            true,
            false,
        );
        vte::Parser::new().advance(&mut grid, self.plugin.render(rows, cols).as_bytes());
        grid.as_character_lines()
            .into_iter()
            .map(|line| {
                line.into_iter()
                    .map(|cell| cell.character)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
fn row(pane: u32, provider: &str) -> Value {
    json!({"Agent":{"pane_id":pane,"kind":provider,"text":format!("{pane} {provider} ○"),"waiting":false}})
}
fn usage(provider: &str, used: f64) -> Value {
    json!({"provider":provider,"status":"ok","observed_at_ms":90_000,"plan":"Pro",
        "windows":[{"label":"5h","used_percent":used,"resets_at_ms":3_690_000},
            {"label":"7d","used_percent":25.5,"resets_at_ms":null}],"message":null})
}

#[test]
fn agents_usage_queries_canonical_providers_for_hook_reported_rows() {
    let mut panel = Panel::new();
    let mut message = super::snapshot_message(1, &[], 2);
    let mut snapshot: Value = serde_json::from_str(message.payload.as_ref().unwrap()).unwrap();
    snapshot["hooks"]["7"] = json!({"phase":"Idle"});
    message.payload = Some(snapshot.to_string());
    panel.plugin.pipe(&message);
    let rows = panel
        .plugin
        .runtime
        .invoke("a-get", vec![json!("rows")])
        .unwrap();
    for provider in ["claude", "codex"] {
        let selected = rows
            .as_array()
            .unwrap()
            .iter()
            .position(|row| row["Agent"]["kind"] == format!("{provider}◆"))
            .expect("the service snapshot produces a row with the hook marker");
        panel.set("selected", json!(selected));
        panel.show();
        let request = panel.request(provider);
        panel.reply(&request, usage(provider, 42.5));
        let lines = panel.lines();
        assert!(
            lines.contains(&format!("Local {provider} account")),
            "{lines}"
        );
        assert!(lines.contains("42.5% used"), "{lines}");
        panel.plugin.update(&Event::Visible(false));
    }
}

#[test]
fn agents_usage_refreshes_only_when_visible_and_throttles_manual_requests_in_flight() {
    let mut panel = Panel::new();
    panel.tick(2_000);
    assert!(panel.requests().is_empty());
    panel.show();
    let first = panel.request("codex");
    panel.key("Ctrl r");
    panel.key("Ctrl r");
    assert!(panel.requests().is_empty());
    panel.plugin.update(&Event::Visible(false));
    panel.show();
    assert!(panel.requests().is_empty());
    assert!(panel.lines().contains("Refreshing"));
    panel.reply(&first, usage("codex", 40.0));
    panel.tick(59_999);
    assert!(panel.requests().is_empty());
    panel.tick(1);
    let second = panel.request("codex");
    panel.reply(&second, usage("codex", 45.0));
    panel.key("Ctrl r");
    let manual = panel.request("codex");
    panel.plugin.update(&Event::Visible(false));
    assert!(
        !panel.reply(&manual, usage("codex", 99.0)),
        "hidden results must not request a render"
    );
    assert_eq!(
        panel.observation("codex")["windows"][0]["used_percent"],
        99.0
    );
    panel.show();
    assert!(panel.requests().is_empty());
    panel.plugin.update(&Event::Visible(false));
    panel.tick(120_000);
    assert!(panel.requests().is_empty());
    panel.show();
    panel.request("codex");
}

#[test]
fn agents_usage_explicit_hidden_event_overrides_an_unsuppressed_container() {
    let mut panel = Panel::new();
    panel.show();
    let request = panel.request("codex");
    panel.reply(&request, usage("codex", 40.0));
    panel.plugin.update(&Event::Visible(false));
    panel
        .plugin
        .runtime
        .update(json!({"PaneUpdate":{"panes":{"0":[{
            "id":1,"is_plugin":true,"is_suppressed":false,"is_focused":false,"is_floating":false
        }]}}}))
        .unwrap();
    panel.tick(120_000);
    assert!(panel.requests().is_empty());
    panel.show();
    panel.request("codex");
}

#[test]
fn agents_usage_correlates_providers_and_keeps_successful_data_when_refresh_fails() {
    let mut panel = Panel::new();
    panel.show();
    let codex = panel.request("codex");
    panel.key("Down");
    let claude = panel.request("claude");
    panel.reply(&codex, usage("codex", 101.5));
    assert!(panel.lines().contains("Local claude account"));
    assert!(!panel.lines().contains("101.5"));
    panel.reply(&claude, usage("claude", 38.0));
    panel.key("Up");
    assert!(panel.requests().is_empty());
    assert!(panel.lines().contains("101.5% used"));
    panel.key("Ctrl r");
    let refresh = panel.request("codex");
    panel.reply(
        &refresh,
        json!({"provider":"codex","status":"expired","windows":[],
        "message":"Sign in again","observed_at_ms":null}),
    );
    let lines = panel.lines();
    assert!(lines.contains("101.5% used"), "{lines}");
    assert!(
        lines.contains("Stale") && lines.contains("Credentials expired"),
        "{lines}"
    );
    assert_eq!(panel.observation("codex")["observed_at_ms"], 90_000);
    panel.key("Ctrl r");
    let malformed = panel.request("codex");
    panel.reply(&malformed, usage("claude", 0.0));
    assert!(panel.lines().contains("Could not read provider usage"));
    assert_eq!(
        panel.observation("codex")["windows"][0]["used_percent"],
        101.5
    );
    let frame = panel.render(25, 64);
    assert!(frame.contains("Refresh failed"), "{frame}");
    assert!(frame.contains("Stale"), "{frame}");
    panel.key("Ctrl r");
    let empty = panel.request("codex");
    panel.reply(
        &empty,
        json!({"provider":"codex","status":"ok","windows":[]}),
    );
    assert_eq!(
        panel.observation("codex")["windows"][0]["used_percent"],
        101.5
    );
    assert!(panel.lines().contains("Could not read provider usage"));
}

#[test]
fn agents_usage_times_out_and_ignores_late_responses() {
    let mut panel = Panel::new();
    panel.show();
    let request = panel.request("codex");
    panel.tick(30_000);
    let lines = panel.lines();
    assert!(lines.contains("Usage refresh timed out"), "{lines}");
    assert!(!lines.contains("0% used"));
    panel.reply(&request, usage("codex", 88.0));
    assert!(panel.observation("codex").is_null());
    panel.tick(30_000);
    panel.request("codex");
}

#[test]
fn agents_usage_does_not_query_local_accounts_for_remote_or_isolated_agents() {
    for (provider, location, message) in [
        ("codex", json!({"host":"build-box"}), "SSH agents"),
        ("claude", json!({"bubblewrap":"/work"}), "isolated agents"),
    ] {
        let mut panel = Panel::new();
        let mut pane = location;
        pane["pane_id"] = json!(7);
        pane["title"] = json!(provider);
        pane["status"] = json!("Running");
        panel.set("snapshot", json!({"panes":[pane],"runs":[]}));
        panel.set("rows", json!([row(7, provider)]));
        panel.show();
        panel.key("Ctrl r");
        assert!(panel.requests().is_empty());
        assert!(panel.lines().contains(message));
        assert!(!panel.lines().contains("Local"));
    }
}

#[test]
fn agents_usage_displays_the_hosts_unsupported_status_for_other_providers() {
    let mut panel = Panel::new();
    panel.set("rows", json!([row(7, "gemini")]));
    panel.set(
        "snapshot",
        json!({"panes":[{"pane_id":7,"title":"gemini","status":"Running"}]}),
    );
    panel.show();
    let request = panel.request("gemini");
    panel.reply(
        &request,
        serde_json::to_value(crate::provider_usage::collect("gemini")).unwrap(),
    );
    let lines = panel.lines();
    assert!(lines.contains("Unsupported"), "{lines}");
    assert!(
        lines.contains("Usage reporting is not available for this provider."),
        "{lines}"
    );
    let summary = panel
        .plugin
        .runtime
        .invoke("a-usage-summary", vec![])
        .unwrap();
    assert_eq!(summary, "Local gemini: provider quota is not supported");
}

#[test]
fn agents_usage_renders_account_windows_resets_and_unknowns_in_the_selected_details() {
    let mut panel = Panel::new();
    panel.show();
    let request = panel.request("codex");
    let mut measurement = usage("codex", 101.5);
    measurement["plan"] = json!("Pro\nplan\u{001b}[31m");
    measurement["windows"].as_array_mut().unwrap().extend([
        json!({"label":"Sonnet","used_percent":30.0,"resets_at_ms":null}),
        json!({"label":"Opus","used_percent":105.0,"resets_at_ms":null}),
    ]);
    panel.reply(&request, measurement);
    let frame = panel.render(30, 120);
    for expected in [
        "Local codex account",
        "Pro plan",
        "101.5% used",
        "resets in 1h",
        "reset unknown",
        "checked now",
        "Opus: 105",
    ] {
        assert!(frame.contains(expected), "missing {expected:?}:\n{frame}");
    }
    for (rows, cols) in [(20, 60), (12, 34), (6, 20), (2, 8)] {
        panel.render(rows, cols);
    }
    assert!(
        panel.requests().is_empty(),
        "rendering must not collect usage"
    );
}

#[test]
fn agents_usage_summary_remains_visible_in_the_default_overlay_with_run_history() {
    let mut panel = Panel::new();
    panel.show();
    let request = panel.request("codex");
    panel.reply(&request, usage("codex", 101.5));
    panel
        .plugin
        .runtime
        .invoke(
            "a-load-run!",
            vec![json!(run("active", 7, false)), json!(0)],
        )
        .unwrap();
    let frame = panel.render(15, 48);
    assert!(frame.contains("Local codex: up to 101.5% used"), "{frame}");
    panel.key("Ctrl r");
    let refresh = panel.request("codex");
    panel.reply(
        &refresh,
        json!({"provider":"codex","status":"error","windows":[],"message":"Provider unavailable"}),
    );
    let frame = panel.render(15, 48);
    assert!(
        frame.contains("101.5% used") && frame.contains("stale"),
        "{frame}"
    );
    panel.set("usage_cache", json!({}));
    panel.key("Ctrl r");
    let refresh = panel.request("codex");
    panel.reply(
        &refresh,
        json!({"provider":"codex","status":"unavailable","windows":[],"message":"No local login"}),
    );
    let frame = panel.render(15, 48);
    assert!(frame.contains("Local codex: quota unavailable"), "{frame}");
}
