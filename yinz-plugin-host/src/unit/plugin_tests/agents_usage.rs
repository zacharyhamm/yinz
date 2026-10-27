use super::{host, run};
use crate::plugin_tests::runtime_parity::SteelReference;
use prost::Message;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use yinz_plugin_api::{
    plugin_command::ProtobufGetCellPixelSizeResponse, plugin_ids::ProtobufPluginIds,
};
use yinz_terminal::panes::grid::Grid;
use yinz_types::{data::Event, units::FractionalSeconds};
use yinz_types::{
    data::{ModeInfo, PluginCommand},
    style::Style,
};

struct Panel {
    plugin: SteelReference,
    clock: Arc<AtomicU64>,
    client: Arc<AtomicU64>,
    cell_width: Arc<AtomicU64>,
}
impl Panel {
    fn new() -> Self {
        let clock = Arc::new(AtomicU64::new(90_000));
        let (_, responder) = host(&[], clock.clone());
        let client = Arc::new(AtomicU64::new(1));
        let cell_width = Arc::new(AtomicU64::new(8));
        let acting = client.clone();
        let pixels = cell_width.clone();
        let responder = Arc::new(move |command: &PluginCommand| match command {
            PluginCommand::GetPluginIds => {
                let mut ids = ProtobufPluginIds::decode(responder(command)?.as_slice()).unwrap();
                ids.client_id = acting.load(Ordering::SeqCst) as _;
                Some(ids.encode_to_vec())
            },
            PluginCommand::GetCellPixelSize => Some(
                ProtobufGetCellPixelSizeResponse {
                    size: (pixels.load(Ordering::SeqCst) > 0).then(|| {
                        yinz_plugin_api::event::CellPixelSize {
                            width: pixels.load(Ordering::SeqCst) as u32,
                            height: 18,
                        }
                    }),
                }
                .encode_to_vec(),
            ),
            _ => responder(command),
        });
        let mut plugin = SteelReference::with_clock(
            "agents",
            BTreeMap::from([("view".into(), "agents".into())]),
            Path::new("/work"),
            responder,
            Some(clock.clone()),
        );
        plugin
            .runtime
            .eval("(define (usage-test-set! key value) (a-set! key value) (void)) (define (usage-test-cache-size) (length (hash-keys->list (a-get \"quota_images\"))))")
            .unwrap();
        let mut panel = Self {
            plugin,
            clock,
            client,
            cell_width,
        };
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

#[test]
fn agents_quota_selects_and_rounds_without_losing_exact_usage() {
    let mut panel = Panel::new();
    for (used, rounded, remaining, tone) in [
        (68.49, 68, 31.51, "success"),
        (74.99, 75, 25.01, "success"),
        (75.0, 75, 25.0, "attention"),
        (90.0, 90, 10.0, "attention"),
        (90.01, 90, 9.99, "error"),
        (101.5, 102, 0.0, "error"),
    ] {
        let observation = usage("codex", used);
        panel.set(
            "usage_cache",
            json!({"codex":{"observation":observation,"result":observation}}),
        );
        let display = panel
            .plugin
            .runtime
            .invoke("a-usage-display", vec![])
            .unwrap();
        assert_eq!(display["value"], rounded);
        assert_eq!(display["tone"], tone);
        assert!((display["remaining"].as_f64().unwrap() - remaining).abs() < 0.001);
        assert_eq!(display["featured"]["used_percent"], used);
        assert!(panel.lines().contains(&format!("{used:?}% used")));
    }
    let observation = usage("codex", 10.0);
    panel.set(
        "usage_cache",
        json!({"codex":{"observation":observation,"result":observation}}),
    );
    let display = panel
        .plugin
        .runtime
        .invoke("a-usage-display", vec![])
        .unwrap();
    assert_eq!(display["featured"]["label"], "7d");
    assert_eq!(display["value"], 26);
    let observation = usage("codex", 25.5);
    panel.set(
        "usage_cache",
        json!({"codex":{"observation":observation,"result":observation}}),
    );
    let display = panel
        .plugin
        .runtime
        .invoke("a-usage-display", vec![])
        .unwrap();
    assert_eq!(display["featured"]["label"], "5h");
    panel.tick(120_000);
    let display = panel
        .plugin
        .runtime
        .invoke("a-usage-display", vec![])
        .unwrap();
    assert_eq!(display["tone"], "neutral");
    assert!(display["freshness"].as_str().unwrap().contains("Stale 2m"));
    panel.set("rows", json!([]));
    assert_eq!(
        panel
            .plugin
            .runtime
            .invoke("a-usage-display", vec![])
            .unwrap(),
        false
    );
}

#[test]
fn agents_quota_footer_reserves_history_and_ignores_footer_clicks() {
    let mut panel = Panel::new();
    panel.show();
    let request = panel.request("codex");
    panel.reply(&request, usage("codex", 68.0));
    panel
        .plugin
        .runtime
        .invoke(
            "a-load-run!",
            vec![json!(run("active", 7, false)), json!(0)],
        )
        .unwrap();
    for (rows, cols) in [
        (30, 120),
        (20, 68),
        (15, 48),
        (8, 90),
        (4, 90),
        (2, 90),
        (6, 20),
    ] {
        let frame = panel.render(rows, cols);
        let get = |panel: &mut Panel, key| {
            panel
                .plugin
                .runtime
                .invoke("a-get", vec![json!(key)])
                .unwrap()
        };
        let footer = get(&mut panel, "last_quota_top");
        if let Some(footer) = footer.as_u64() {
            assert!(footer < rows as u64, "{frame}");
            let before = get(&mut panel, "history_cursor");
            panel
                .plugin
                .runtime
                .update(json!({"Mouse":{"LeftClick":[footer, cols-1]}}))
                .unwrap();
            panel
                .plugin
                .runtime
                .update(json!({"Mouse":{"Release":[footer, cols-1]}}))
                .unwrap();
            assert_eq!(get(&mut panel, "history_cursor"), before);
            assert_eq!(get(&mut panel, "reading"), false);
            let body_top = get(&mut panel, "last_history_top").as_u64().unwrap()
                + get(&mut panel, "last_label_rows").as_u64().unwrap()
                + 1
                + get(&mut panel, "last_history_body_top").as_u64().unwrap();
            let body_rows = get(&mut panel, "last_history_body_rows").as_u64().unwrap();
            assert!(body_top + body_rows <= footer, "{frame}");
            if cols == 120 {
                assert!(body_rows >= 4);
                assert!(frame.contains("68% USED · 5h"));
            }
        }
    }
    panel.set("rows", json!([{"Run":{"run_id":"archived","kind":"codex","worktree":"topic","title":"Archive","started_at_ms":1000,"continuation":false}}]));
    let frame = panel.render(30, 120);
    assert!(!frame.contains("Local codex"));
}

#[test]
fn agents_quota_images_reuse_ids_and_cache_each_clients_value_theme_and_geometry() {
    let mut panel = Panel::new();
    panel.show();
    let request = panel.request("codex");
    panel.reply(&request, usage("codex", 68.0));
    let themes = yinz_config::assets::get_default_themes();
    let image = |output: &str| {
        output
            .split("\x1b_Ga=t")
            .nth(1)
            .unwrap()
            .split("\x1b_Ga=p")
            .next()
            .unwrap()
            .to_owned()
    };
    let mut first = String::new();
    for (client, theme) in [
        (1, "gruvbox-dark"),
        (2, "gruvbox-light"),
        (1, "gruvbox-light"),
    ] {
        panel.client.store(client, Ordering::SeqCst);
        let mut mode = ModeInfo {
            style: Style {
                colors: themes.get_theme(theme).unwrap().palette,
                ..Default::default()
            },
            ..Default::default()
        };
        mode.capabilities.kitty_graphics = true;
        panel.plugin.update(&Event::ModeUpdate(mode.clone()));
        let output = panel.plugin.render(30, 120);
        assert!(
            output.contains("a=t,f=100,t=d,q=2,i=1"),
            "missing PNG transmission"
        );
        assert!(output.contains('\u{10eeee}'), "missing image placeholders");
        assert_eq!(image(&output), image(&panel.plugin.render(30, 120)));
        if first.is_empty() {
            first = image(&output);
        } else {
            assert_ne!(first, image(&output));
        }
        let cache = panel
            .plugin
            .runtime
            .invoke("usage-test-cache-size", vec![])
            .unwrap();
        assert_eq!(
            cache.as_u64().unwrap(),
            if client == 1 && theme == "gruvbox-dark" {
                1
            } else {
                2
            }
        );
        mode.capabilities.kitty_graphics = false;
        panel.plugin.update(&Event::ModeUpdate(mode));
        let output = panel.plugin.render(30, 120);
        assert!(!output.contains('\u{10eeee}'));
        assert!(panel.render(30, 120).contains("68% USED"));
    }
    panel.plugin.runtime.eval("(a-set! \"client_modes\" (hash-insert (a-get \"client_modes\") 1 (hash-insert (hash-ref (a-get \"client_modes\") 1) \"capabilities\" (hash \"kitty_graphics\" #t))))").unwrap();
    let observation = usage("codex", 69.6);
    panel.set(
        "usage_cache",
        json!({"codex":{"observation":observation,"result":observation}}),
    );
    let changed = panel.plugin.render(30, 120);
    assert!(changed.contains("a=t,f=100,t=d,q=2,i=1"));
    assert_ne!(image(&changed), first);
    for width in [9, 0, 1000] {
        panel.cell_width.store(width, Ordering::SeqCst);
        let output = panel.plugin.render(30, 120);
        assert_eq!(output.contains('\u{10eeee}'), width == 9);
        if width == 9 {
            assert_ne!(image(&output), first);
        }
    }
}
