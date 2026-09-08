//! Real JS/TS files through the plugin thread and shared host command handlers.
//! Synchronization observes guest progress, never elapsed time or render counts.

use super::*;
use zellij_types::internal_ids::{PluginId, TabId};

const CLIENT: ClientId = ClientId::new(1);
const PLUGIN: PluginId = PluginId::new(0);
const SIZE: Size = Size { cols: 80, rows: 10 };

// CustomMessage is ungated; InputReceived would require ReadApplicationState.
fn release_event() -> Event {
    Event::CustomMessage("acceptance-release".into(), String::new())
}

fn run_plugin(path: &std::path::Path) -> RunPluginOrAlias {
    RunPluginOrAlias::RunPlugin(RunPlugin {
        location: RunPluginLocation::File(path.to_owned()),
        configuration: PluginUserConfiguration::new(BTreeMap::from([(
            "greeting".into(),
            "configured".into(),
        )])),
        ..Default::default()
    })
}

fn load(sender: &SenderWithContext<PluginInstruction>, plugin: RunPluginOrAlias) {
    sender
        .send(PluginInstruction::Load(
            Some(false),
            false,
            false,
            Some("deno acceptance".into()),
            plugin,
            Some(TabId::new(1)),
            None,
            CLIENT,
            SIZE,
            None,
            None,
            false,
            None,
            None,
            None,
            None,
        ))
        .unwrap();
}

struct Harness {
    sender: SenderWithContext<PluginInstruction>,
    log: Watched<Vec<ScreenInstruction>>,
    directory: tempfile::TempDir,
    path: PathBuf,
    teardown: Option<Box<dyn FnOnce()>>,
    logger: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn new(extension: &str, source: &str, status: PermissionStatus) -> Self {
        Self::with_permission_reply(extension, source, Some(status))
    }

    fn with_permission_reply(
        extension: &str,
        source: &str,
        status: Option<PermissionStatus>,
    ) -> Self {
        Self::with_event_sender(extension, source, status, None)
    }

    fn with_event_sender(
        extension: &str,
        source: &str,
        status: Option<PermissionStatus>,
        event_sender: Option<SenderWithContext<PluginInstruction>>,
    ) -> Self {
        let directory = tempdir().unwrap();
        let path = directory.path().join(format!("acceptance.{extension}"));
        std::fs::write(&path, source).unwrap();
        let (sender, receiver, teardown) = create_plugin_thread_with_hooks_and_event_sender(
            Some(directory.path().to_owned()),
            None,
            None,
            Default::default(),
            event_sender,
        );
        let log = Watched::new(vec![]);
        let logger = match status {
            Some(status) => permissions_and_log_until_disconnect(
                &log,
                receiver,
                directory.path().join("permissions.kdl"),
                sender.clone(),
                CLIENT,
                status,
            ),
            None => log_until_disconnect(&log, receiver),
        };
        sender.send(PluginInstruction::AddClient(CLIENT)).unwrap();
        // Callers may write imported dependencies before starting the load.
        Self {
            sender,
            log,
            directory,
            path,
            teardown: Some(teardown),
            logger: Some(logger),
        }
    }

    fn load(&self) {
        load(&self.sender, run_plugin(&self.path));
    }

    fn load_shared(&self) {
        load(
            &self.sender,
            RunPluginOrAlias::RunPlugin(RunPlugin {
                location: RunPluginLocation::File(self.path.clone()),
                configuration: PluginUserConfiguration::new(BTreeMap::from([(
                    "shared".into(),
                    "true".into(),
                )])),
                ..Default::default()
            }),
        );
    }

    fn event(&self, event: Event) {
        self.sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(CLIENT),
                event,
            )]))
            .unwrap();
    }

    fn reload(&self) {
        self.sender
            .send(PluginInstruction::Reload(
                Some(false),
                Some("deno acceptance".into()),
                run_plugin(&self.path),
                TabId::new(1),
                SIZE,
                None,
            ))
            .unwrap();
    }

    fn rendered(&self, text: &str) -> String {
        wait_for_render_containing(&self.log, text)
    }

    fn finish(mut self) -> Vec<ScreenInstruction> {
        self.stop();
        let log = self.log.lock().clone();
        log
    }

    fn stop(&mut self) {
        if let Some(teardown) = self.teardown.take() {
            teardown();
        }
        if let Some(logger) = self.logger.take() {
            logger.join().unwrap();
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // Preserve Watched's timeout and frame diagnostics if the runtime is stuck.
            // nextest isolates each test in a process; request shutdown without joining
            // a worker that may be the reason the wait failed. Successful tests still join.
            let _ = self.sender.send(PluginInstruction::Exit);
        } else {
            self.stop();
        }
    }
}

const PERMISSIONS: &str = r#"
let state = "starting";
export default {
  async load(config) {
    zellij.subscribe(["PermissionRequestResult"]);
    // Try the protected operation both before and after the user's answer.
    try { zellij.command("RenameTabWithId", [1, "before-grant"]); } catch (_) {}
    // The helper and an independent observer must share the same host event.
    const answer = zellij.waitForEvent("PermissionRequestResult");
    let helper = "resolved";
    try { await zellij.waitForPermissions(["ChangeApplicationState"]); }
    catch (_) { helper = "rejected"; }
    const status = await answer;
    const ids = zellij.command("GetPluginIds");
    let outcome = "returned";
    try { zellij.command("RenameTabWithId", [1, "after-grant"]); }
    catch (_) { outcome = "rejected"; }
    state = `${config.greeting} ${status} plugin=${ids.plugin_id} client=${ids.client_id} ${outcome} helper=${helper}`;
  },
  render() { return state; }
};
"#;

fn permission_round_trip(status: PermissionStatus, expected: &str) -> Vec<String> {
    let harness = Harness::new("js", PERMISSIONS, status);
    harness.load();
    let outcome = if status == PermissionStatus::Granted {
        "returned helper=resolved"
    } else {
        "rejected helper=rejected"
    };
    harness.rendered(&format!(
        "configured {expected} plugin=0 client=1 {outcome}"
    ));
    harness
        .finish()
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::RenameTabWithId(_, name, _) => {
                Some(String::from_utf8(name.clone()).unwrap())
            },
            _ => None,
        })
        .collect()
}

#[test]
#[serial(plugin_tests)]
fn deno_permission_grant_resumes_async_load_and_decodes_command_replies() {
    assert_eq!(
        permission_round_trip(PermissionStatus::Granted, "Granted"),
        ["after-grant"]
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_permission_denial_never_routes_the_protected_command() {
    assert!(permission_round_trip(PermissionStatus::Denied, "Denied").is_empty());
}

#[test]
#[serial(plugin_tests)]
fn deno_typescript_awaits_an_event_and_timer_then_renders_asynchronously_after_resize() {
    let harness = Harness::new(
        "ts",
        r#"
import { zellij, definePlugin } from "zellij:plugin";
let state: string = "starting";
export default definePlugin({
  async load(config) {
    zellij.subscribe(["Key"]);
    const key = zellij.waitForEvent("Key");
    state = `${config.greeting} awaiting key`;
    zellij.requestRender();
    await key;
    await zellij.sleep(0);
    state = "key and timer completed";
  },
  async render(rows: number, columns: number) {
    await zellij.sleep(0);
    return `${state} ${rows}x${columns}`;
  }
});
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("configured awaiting key 10x80");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("key and timer completed 10x80");
    harness
        .sender
        .send(PluginInstruction::Resize(PLUGIN, 113, 27))
        .unwrap();
    harness.rendered("key and timer completed 27x113");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn deno_resize_discards_an_in_flight_async_render_with_the_old_dimensions() {
    let harness = Harness::new(
        "ts",
        r#"
let release: (() => void) | undefined;
export default {
  load() { zellij.subscribe(["Key"]); },
  async update() {
    await zellij.sleep(0);
    release?.();
    return true;
  },
  async render(rows: number, columns: number) {
    if (columns === 80) {
      const ready = new Promise<void>(resolve => { release = resolve; });
      // This instruction marks the point the old render is suspended.
      zellij.command("SetSelectable", false);
      await ready;
    }
    return `finished async render ${rows}x${columns}`;
  }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.log.wait_until(
        "the original render is awaiting a key",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::Resize(PLUGIN, 113, 27))
        .unwrap();
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("finished async render 27x113");
    let log = harness.finish();
    assert!(
        !rendered_assets(&log)
            .iter()
            .any(|(_, frame)| frame.contains("finished async render 10x80")),
        "the suspended render must not publish its obsolete dimensions"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_shared_promises_keep_their_origin_after_another_clients_event_and_timer() {
    let harness = Harness::new(
        "ts",
        r#"
import type { Event } from "zellij:plugin";
type Reply = { client_id: number };
const origin = () => (zellij.command("GetPluginIds") as Reply).client_id;
const waiting: number[] = [];
const completed: string[] = [];
let loads = 0;
export default {
  load() { loads++; zellij.subscribe(["Key"]); },
  async update(event: Event) {
    if (typeof event !== "object" || !("Key" in event)) return false;
    const started = origin();
    const release = zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    waiting.push(started);
    zellij.requestRender();
    await release;
    const afterEvent = origin();
    await zellij.sleep(0);
    completed.push(`${started}->${afterEvent}->${origin()}`);
    return true;
  },
  async render() {
    const started = origin();
    await zellij.sleep(0);
    return `loads=${loads} waiting=${waiting.join(",")} done=${[...completed].sort().join(",")} render=${started}->${origin()}`;
  }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load_shared();
    harness.rendered("loads=1 waiting= done= render=1->1");
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    for client in [CLIENT, other] {
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(client),
                Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
            )]))
            .unwrap();
    }
    harness.rendered("loads=1 waiting=1,2");
    // Both tasks resume during client 2's event; each must retain its own origin.
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            Some(other),
            release_event(),
        )]))
        .unwrap();
    harness.log.wait_until_described(
        "both clients' async callbacks and frames retain their origin",
        default_timeout(),
        |log| {
            [CLIENT, other]
                .into_iter()
                .all(|client| {
                    rendered_assets(log).iter().any(|(rendered_client, frame)| {
                        *rendered_client == client
                            && frame.contains("done=1->1->1,2->2->2")
                            && frame.contains(&format!("render={client}->{client}"))
                    })
                })
                .then_some(())
        },
        |log| describe_renders(log),
    );
    let log = harness.finish();
    assert!(
        !rendered_assets(&log)
            .iter()
            .any(|(_, frame)| frame.contains("loads=2")),
        "a shared plugin must keep one instance"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_shared_permission_wait_survives_an_attach_and_replays_the_queued_event() {
    let harness = Harness::with_permission_reply(
        "ts",
        r#"
import type { Event } from "zellij:plugin";
let loads = 0;
let keys = 0;
let state = "starting";
export default {
  async load() {
    loads++;
    zellij.subscribe(["Key"]);
    await zellij.waitForPermissions(["ChangeApplicationState"]);
    state = "granted";
  },
  update(event: Event) {
    if (typeof event === "object" && "Key" in event) { keys++; return true; }
    return false;
  },
  render() { return `loads=${loads} ${state} keys=${keys}`; }
};
"#,
        None,
    );
    harness.load_shared();
    harness.log.wait_until(
        "the shared plugin is waiting on the permission prompt",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::RequestPluginPermissions(..)))
                .then_some(())
        },
    );
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            Some(other),
            Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
        )]))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::PermissionRequestResult(
            PLUGIN,
            Some(CLIENT),
            vec![PermissionType::ChangeApplicationState],
            PermissionStatus::Granted,
            Some(harness.directory.path().join("permissions.kdl")),
        ))
        .unwrap();
    harness.log.wait_until_described(
        "the attached clients see the single granted instance's queued key",
        default_timeout(),
        |log| {
            [CLIENT, other]
                .into_iter()
                .all(|client| {
                    rendered_assets(log).iter().any(|(rendered_client, frame)| {
                        *rendered_client == client && frame.contains("loads=1 granted keys=1")
                    })
                })
                .then_some(())
        },
        |log| describe_renders(log),
    );
    let log = harness.finish();
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::RequestPluginPermissions(..)))
            .count(),
        1,
        "attaching must reuse the shared instance's permission wait"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_reload_rejects_an_old_host_timer_completion_but_accepts_the_current_one() {
    let source = r#"
import type { Event } from "zellij:plugin";
const label = "original";
let state = "pending";
let timers = 0;
let fences = 0;
export default {
  async load() {
    zellij.subscribe(["CustomMessage"]);
    await zellij.sleep(0);
    state = "completed";
  },
  update(event: Event) {
    if (typeof event !== "object") return false;
    if ("Timer" in event) { timers++; return true; }
    if ("CustomMessage" in event && event.CustomMessage[0] === "acceptance-release") {
      fences++;
      return true;
    }
    return false;
  },
  render() { return `${label} ${state} timers=${timers} fences=${fences}`; }
};
"#;
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let harness = Harness::with_event_sender("ts", source, None, Some(to_proxy.clone()));
    let completions = Watched::new(Vec::<PluginInstruction>::new());
    let proxy = std::thread::spawn({
        let completions = completions.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing
                .recv_timeout(FAKE_SCREEN_RECV_TIMEOUT)
                .expect("the timer proxy must be stopped after teardown");
            match instruction {
                PluginInstruction::Exit => break,
                timer @ PluginInstruction::DenoEvent(_, _, _, Event::Timer(_)) => {
                    completions.lock().push(timer);
                },
                // Capture the legacy envelope too, so losing the generation at the
                // SetTimeout producer fails explicitly instead of accidentally delivering it.
                PluginInstruction::Update(ref events)
                    if events
                        .iter()
                        .any(|(_, _, event)| matches!(event, Event::Timer(_))) =>
                {
                    completions.lock().push(instruction);
                },
                other => {
                    let _ = sender.send(other);
                },
            }
        }
    });
    harness.load();
    let old = completions.wait_until(
        "the old SDK sleep produced a real host completion",
        default_timeout(),
        |events| events.first().cloned(),
    );
    harness.rendered("original pending timers=0 fences=0");
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness.reload();
    let current = completions.wait_until(
        "the replacement SDK sleep produced its own host completion",
        default_timeout(),
        |events| events.get(1).cloned(),
    );
    harness.rendered("replacement pending timers=0 fences=0");
    match (&old, &current) {
        (
            PluginInstruction::DenoEvent(old_id, old_client, old_generation, Event::Timer(_)),
            PluginInstruction::DenoEvent(new_id, new_client, new_generation, Event::Timer(_)),
        ) => {
            assert_eq!((*old_id, *old_client), (PLUGIN, CLIENT));
            assert_eq!((*new_id, *new_client), (PLUGIN, CLIENT));
            assert_ne!(
                old_generation, new_generation,
                "reload must change the timer's generation"
            );
        },
        _ => panic!("host SetTimeout must tag its completions: old={old:?}, current={current:?}"),
    }
    // Both timers have already fired, but neither completion has been delivered.
    // Deliver the old one followed by a FIFO event the new lifecycle acknowledges.
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    let frame = harness.rendered("fences=1");
    assert!(frame.contains("replacement pending timers=0 fences=1"),
        "an old host timer must neither reach update nor resolve the replacement's SDK sleep: {frame}");
    harness.sender.send(current).unwrap();
    harness.rendered("replacement completed timers=1 fences=1");
    harness.finish();
    // Stop explicitly: event-loop wakers may retain the plugin-only proxy sender
    // after isolate disposal, so channel disconnect is not the proxy's lifetime.
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn deno_reload_rereads_imports_and_cancels_the_old_event_await() {
    let harness = Harness::new(
        "ts",
        r#"
import { label } from "./dependency.ts";
let state: string = "starting";
export default {
  async load() {
    const event = zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    state = `${label} awaiting input`;
    zellij.requestRender();
    await event;
    // An observable host effect catches an old continuation even if its render is discarded.
    zellij.command("SetSelectable", label === "second");
    state = `${label} completed`;
  },
  render() { return state; }
};
"#,
        PermissionStatus::Granted,
    );
    let dependency = harness.directory.path().join("dependency.ts");
    std::fs::write(&dependency, "export const label: string = 'first';").unwrap();
    harness.load();
    harness.rendered("first awaiting input");
    std::fs::write(&dependency, "export const label: string = 'second';").unwrap();
    harness.reload();
    harness.rendered("second awaiting input");
    harness.event(release_event());
    harness.rendered("second completed");
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("first completed")));
    assert!(!log
        .iter()
        .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))));
    assert!(log
        .iter()
        .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true))));
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1,
        "reload must retain the existing pane"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_removed_client_reattaches_with_fresh_state_and_without_its_old_awaits() {
    client_reattaches(false);
}

#[test]
#[serial(plugin_tests)]
fn deno_reused_client_id_reconnects_without_waiting_for_disposal() {
    client_reattaches(true);
}

fn client_reattaches(immediate: bool) {
    let harness = Harness::new(
        "ts",
        r#"
import type { Event } from "zellij:plugin";
let keys = 0;
let completed = 0;
export default {
  load() { zellij.subscribe(["Key"]); },
  async update(event: Event) {
    if (typeof event !== "object" || !("Key" in event)) return false;
    const ordinal = ++keys;
    const ready = zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    zellij.requestRender();
    await ready;
    // Only the discarded instance reaches ordinal 2.
    zellij.command("SetSelectable", ordinal !== 2);
    completed++;
    return true;
  },
  render() { return `client=${zellij.getPluginIds().client_id} keys=${keys} completed=${completed}`; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("client=1 keys=0 completed=0");
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    harness.rendered("client=2 keys=0 completed=0");
    for _ in 0..2 {
        harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    }
    harness.rendered("client=1 keys=2 completed=0");
    let before_reattach = harness.log.lock().len();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    if !immediate {
        // The original case observes disposal before reattaching. The immediate
        // case deliberately sends RemoveClient/AddClient back to back.
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(other),
                Event::Key(KeyWithModifier::new(BareKey::Char('b'))),
            )]))
            .unwrap();
        harness.rendered("client=2 keys=1 completed=0");
    }
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    harness.log.wait_until_described(
        "reattaching client 1 creates a fresh instance",
        default_timeout(),
        |log| {
            rendered_assets(&log[before_reattach..])
                .iter()
                .any(|(client, frame)| {
                    *client == CLIENT && frame.contains("client=1 keys=0 completed=0")
                })
                .then_some(())
        },
        |log| describe_renders(log),
    );
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('c'))));
    harness.rendered("client=1 keys=1 completed=0");
    if immediate {
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(other),
                Event::Key(KeyWithModifier::new(BareKey::Char('b'))),
            )]))
            .unwrap();
        harness.rendered("client=2 keys=1 completed=0");
    }
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            None,
            release_event(),
        )]))
        .unwrap();
    for client in [CLIENT, other] {
        harness.rendered(&format!("client={client} keys=1 completed=1"));
    }
    let log = harness.finish();
    assert!(
        !log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))),
        "the removed instance's pending callbacks must never run"
    );
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("completed=2")));
}

#[test]
#[serial(plugin_tests)]
fn deno_disconnect_during_pending_async_load_discards_its_continuation() {
    let source = r#"
const label = "original";
let state = `${label} load pending`;
export default {
  async load() {
    await zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    zellij.command("SetSelectable", label === "replacement");
    state = `${label} load completed`;
  },
  render() { return state; }
};
"#;
    let harness = Harness::new("ts", source, PermissionStatus::Granted);
    harness.load();
    harness.rendered("original load pending");
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    harness.rendered("replacement load pending");
    harness.event(release_event());
    harness.rendered("replacement load completed");
    let log = harness.finish();
    assert!(
        !log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))),
        "disconnect must revoke the suspended load's host access"
    );
    assert!(log
        .iter()
        .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true))));
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("original load completed")));
}

#[test]
#[serial(plugin_tests)]
fn deno_disconnect_interrupts_initial_load_before_the_instance_is_published() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() {
    zellij.command("SetSelectable", false);
    for (;;) {}
  },
  render() { return "disconnected initialization was published"; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.log.wait_until(
        "the loader entered guest code before publishing the instance",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    // Reattaching may provision this plugin for the new client. Its next load must
    // be safe, while the original isolate remains inside the already-running loop.
    std::fs::write(
        &harness.path,
        "export default { render() { return 'replacement after interrupted initialization'; } };",
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    // A new client and plugin can progress only once the old initialization released
    // the plugin map. No release event or second host call can free that infinite loop.
    harness
        .sender
        .send(PluginInstruction::AddClient(ClientId::new(2)))
        .unwrap();
    let survivor = harness.directory.path().join("survivor.js");
    std::fs::write(
        &survivor,
        "export default { render() { return 'client survived pending initialization'; } };",
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("client survived pending initialization");
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("disconnected initialization was published")));
}

#[test]
#[serial(plugin_tests)]
fn deno_shutdown_delivers_before_close_to_an_idle_plugin_once() {
    let harness = Harness::new(
        "ts",
        r#"
import type { Event } from "zellij:plugin";
export default {
  load() { zellij.subscribe(["BeforeClose"]); },
  update(event: Event) {
    if (event === "BeforeClose") {
      // Shutdown must still permit host calls from an idle plugin's close callback.
      zellij.command("SetSelectable", false);
    }
    return false;
  },
  render() { return "idle plugin ready for shutdown"; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("idle plugin ready for shutdown");
    // Exercise session shutdown, not an explicit Unload that happens to close a plugin.
    harness.sender.send(PluginInstruction::Exit).unwrap();
    harness.log.wait_until_described(
        "the idle plugin receives BeforeClose during shutdown",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
        |log| describe_renders(log),
    );
    let log = harness.finish();
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
            .count(),
        1,
        "BeforeClose must be delivered exactly once"
    );
    assert!(
        !log.iter().any(
            |i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage)
        if *id == PLUGIN && stage.is_error())
        ),
        "normal shutdown must not report a guest crash"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_unload_disposes_pending_load_and_render_awaits() {
    let harness = Harness::new(
        "ts",
        r#"
export default {
  async load() {
    await zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    zellij.command("SetSelectable", false);
  },
  async render() {
    const ready = zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    // Load has already suspended; now render has registered its own await too.
    zellij.command("SetSelectable", true);
    await ready;
    zellij.command("SetSelectable", false);
    return "disposed render resumed";
  }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.log.wait_until(
        "both guest callbacks have registered pending awaits",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true)))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::Unload(PLUGIN))
        .unwrap();
    let survivor = harness.directory.path().join("survivor.ts");
    std::fs::write(
        &survivor,
        r#"
let state = "survivor awaiting input";
export default {
  async load() {
    await zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    state = "survivor received input";
  },
  render() { return state; }
};
"#,
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("survivor awaiting input");
    // Broadcast the very event both discarded callbacks awaited, then observe its delivery.
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            None,
            Some(CLIENT),
            release_event(),
        )]))
        .unwrap();
    harness.rendered("survivor received input");
    let log = harness.finish();
    assert!(
        !log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))),
        "unloaded callbacks must not issue host commands"
    );
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("disposed render resumed")));
}

#[test]
#[serial(plugin_tests)]
fn deno_rejected_promise_reports_the_original_typescript_dependency_line() {
    let harness = Harness::new(
        "ts",
        r#"
import { rejectAfterInput } from "./failure.ts";
import type { Event } from "zellij:plugin";
let state = "ready to reject";
export default {
  load() { zellij.subscribe(["Key"]); },
  update(event: Event) {
    if (typeof event !== "object" || !("Key" in event)) return false;
    state = "awaiting rejection";
    zellij.requestRender();
    return rejectAfterInput();
  },
  render() { return state; }
};
"#,
        PermissionStatus::Granted,
    );
    // These erased lines make a generated-JS line number observably wrong.
    let dependency = r#"interface ErasedBeforeFunction {
  first: string;
  second: number;
  third: boolean;
}
export async function rejectAfterInput(): Promise<never> {
  await zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
  throw new Error("acceptance rejected promise");
}
"#;
    let expected_line = dependency
        .lines()
        .position(|line| line.contains("throw new Error"))
        .unwrap()
        + 1;
    std::fs::write(harness.directory.path().join("failure.ts"), dependency).unwrap();
    harness.load();
    harness.rendered("ready to reject");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("awaiting rejection");
    harness.event(release_event());
    let error = harness.log.wait_until_described(
        "the asynchronous rejection reaches the plugin's error UI",
        default_timeout(),
        |log| {
            log.iter().find_map(|i| match i {
                ScreenInstruction::UpdatePluginLoadingStage(id, stage)
                    if *id == PLUGIN && stage.is_error() =>
                {
                    Some(format!("{stage:?}"))
                },
                _ => None,
            })
        },
        |log| describe_renders(log),
    );
    assert!(error.contains("acceptance rejected promise"), "{error}");
    assert!(
        error.contains(&format!("failure.ts:{expected_line}:")),
        "the error must name the original TS dependency and source line: {error}"
    );
    harness.finish();
}

fn unload_runaway(source: &str, start_with_key: bool) {
    let harness = Harness::new("js", source, PermissionStatus::Granted);
    harness.load();
    if start_with_key {
        harness.rendered("ready to loop");
        harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    }
    harness.log.wait_until(
        "the guest entered the callback that loops",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::Unload(PLUGIN))
        .unwrap();
    // An unloaded generation no longer owns a pane to paint an error into. Observe
    // continued plugin-thread progress instead of waiting for its suppressed crash UI.
    let survivor = harness.directory.path().join("survivor.js");
    std::fs::write(
        &survivor,
        "export default { render() { return 'survivor is alive'; } };",
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("survivor is alive");
    // The survivor may get a different executor. finish also joins the screen logger,
    // which cannot disconnect until the old executor releases its screen senders.
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn deno_reload_by_url_interrupts_an_infinite_update_and_runs_the_replacement() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() { zellij.subscribe(["Key"]); },
  update() {
    zellij.command("SetSelectable", false);
    for (;;) {}
  },
  render() { return "original ready to loop"; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("original ready to loop");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.log.wait_until(
        "the old instance entered its infinite update",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    std::fs::write(
        &harness.path,
        r#"
let keys = 0;
export default {
  load() {
    zellij.subscribe(["Key"]);
    zellij.command("SetSelectable", true);
  },
  update() { keys++; return true; },
  render() { return `replacement alive keys=${keys}`; }
};
"#,
    )
    .unwrap();
    // Exercise location matching while the guest owns its instance lock. An ID-only
    // reload would bypass the lookup that formerly deadlocked before cancellation.
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    harness.rendered("replacement alive keys=0");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('b'))));
    harness.rendered("replacement alive keys=1");
    let log = harness.finish();
    assert!(
        log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true))),
        "the replacement must have a fresh cancellation token and working host commands"
    );
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1,
        "reload by URL must replace the running instance in its existing pane"
    );
    assert!(log.iter().any(|i| matches!(i, ScreenInstruction::PluginBytes(assets)
        if assets.iter().any(|asset| asset.plugin_id == PLUGIN && frame_of(&asset.bytes).contains("replacement alive keys=1")))),
        "the original plugin ID must identify the responsive replacement");
}

#[test]
#[serial(plugin_tests)]
fn deno_reload_by_url_delivers_idle_before_close_before_loading_the_replacement() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() { zellij.subscribe(["BeforeClose"]); },
  update(event) {
    if (event === "BeforeClose") zellij.command("SetSelectable", false);
    return false;
  },
  render() { return "idle original ready for reload"; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("idle original ready for reload");
    std::fs::write(
        &harness.path,
        r#"
export default {
  load() { zellij.command("SetSelectable", true); },
  render() { return "idle replacement loaded"; }
};
"#,
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    harness.rendered("idle replacement loaded");
    let log = harness.finish();
    let selectable: Vec<bool> = log
        .iter()
        .filter_map(|i| match i {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(selectable, [false, true],
        "the idle original's BeforeClose must retain host access and run exactly once, before replacement load");
    assert!(
        !log.iter().any(
            |i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage)
        if *id == PLUGIN && stage.is_error())
        ),
        "reloading an idle plugin must not crash it"
    );
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_unload_interrupts_an_infinite_update_without_host_calls() {
    unload_runaway(
        r#"
export default {
  load() { zellij.subscribe(["Key"]); },
  update() { zellij.command("SetSelectable", false); for (;;) {} },
  render() { return "ready to loop"; }
};
"#,
        true,
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_unload_interrupts_an_infinite_load_without_host_calls() {
    unload_runaway(
        r#"
export default {
  load() { zellij.command("SetSelectable", false); for (;;) {} }
};
"#,
        false,
    );
}
