//! A batch of events reaching one plugin costs it one render, not one per event.
//!
//! A wheel flick is tens of `ScrollUp(3)`s a second, each of which used to be its own executor
//! job, its own wasm `render()` per attached client and its own full re-parse on the screen
//! thread. The plugin thread now merges the `Update`s waiting on its bus and the wasm bridge
//! delivers a plugin its whole share of the batch in one job, rendering once at the end. Every
//! event is still delivered, in order, with its count - the tests here read them back off a Lua
//! plugin that logs each one and renders the log.

use super::*;
use crate::plugins::{coalesce_hovers, drain_reloads, drain_updates};
use crate::ClientId;
use zellij_test_support::Watched;
use zellij_utils::data::{BareKey, KeyWithModifier, Mouse, PermissionStatus};

const WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// The pipe the logger blocks from its `pipe` handler and unblocks on `UNBLOCK`, blocks again
/// on `BLOCK`, and the key on which its `update` raises an error.
const PIPE: &str = "batch_pipe";
const UNBLOCK: char = 'u';
const BLOCK: char = 'b';
const RAISE: char = 'e';

/// Logs one line per event, tagged with the client it was delivered for, and renders the log.
/// Mouse and key events need no permission, and a test counting renders asks for none: the
/// render a permission answer forces would muddy the count. The pipe test asks for
/// `read_cli_pipes` and waits for the grant before it starts.
fn logger_source(permissions: &[&str]) -> String {
    let permissions = permissions
        .iter()
        .map(|permission| format!("{permission:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
    local zellij = require("zellij")
    local M = {{}}
    local lines = {{}}
    local function log(s) lines[#lines + 1] = s end
    function M.load()
      zellij.subscribe {{ "mouse", "key", "permission_request_result" }}
      if #{{{permissions}}} > 0 then zellij.request_permission {{{permissions}}} end
      log("ready")
    end
    function M.update(event)
      local client = zellij.get_plugin_ids().client_id
      if event.name == "permission_request_result" then
        log("granted")
        return true
      elseif event.name == "mouse" then
        local at = event.lines and tostring(event.lines) or (event.line .. "," .. event.col)
        log(event.kind .. " " .. at .. " client=" .. client)
        return true
      elseif event.name == "key" then
        log("key " .. event.key .. " client=" .. client)
        if event.key == "u" then
          zellij.unblock_cli_pipe_input("batch_pipe")
          return false
        elseif event.key == "b" then
          zellij.block_cli_pipe_input("batch_pipe")
          return false
        elseif event.key == "e" then
          error("boom")
        end
        return true
      end
      return false
    end
    function M.pipe(message)
      log("pipe " .. message.name)
      zellij.block_cli_pipe_input("batch_pipe")
      return true
    end
    -- The size and the acting client head every frame, before the log, so that
    -- `logged_after_ready` never sees them. A shared plugin is rendered once per
    -- client off one instance, and the head is how a test reads back the size
    -- each of those passes was given.
    function M.render(rows, cols)
      local head = "size " .. rows .. "x" .. cols
        .. " client=" .. zellij.get_plugin_ids().client_id
      return head .. "\n" .. table.concat(lines, "\n")
    end
    return M
"#
    )
}

struct Harness {
    to_plugin: SenderWithContext<PluginInstruction>,
    screen: Watched<Vec<ScreenInstruction>>,
    server: Watched<Vec<ServerInstruction>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    teardown: Option<Box<dyn FnOnce()>>,
    client_id: ClientId,
    dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("permissions_test.kdl");
        let (to_plugin, server_receiver, screen_receiver, teardown) =
            create_plugin_thread_with_server_receiver(Some(PathBuf::from(dir.path())), None);
        let client_id = 1;
        let screen = Watched::new(vec![]);
        let server = Watched::new(vec![]);
        let screen_thread = std::thread::Builder::new()
            .name("fake_screen_thread".to_string())
            .spawn({
                let screen = screen.clone();
                let to_plugin = to_plugin.clone();
                move || loop {
                    let Ok((instruction, _)) = screen_receiver.recv_timeout(WAIT * 3) else {
                        break;
                    };
                    match instruction {
                        ScreenInstruction::Exit => break,
                        ScreenInstruction::RequestPluginPermissions(plugin_id, permission) => {
                            let _ = to_plugin.send(PluginInstruction::PermissionRequestResult(
                                plugin_id,
                                Some(client_id),
                                permission.permissions,
                                PermissionStatus::Granted,
                                Some(cache_path.clone()),
                            ));
                        },
                        other => screen.lock().push(other),
                    }
                }
            })
            .unwrap();
        // the server channel is bounded, so someone has to keep draining it
        let server_thread = std::thread::Builder::new()
            .name("fake_server_thread".to_string())
            .spawn({
                let server = server.clone();
                move || loop {
                    let Ok((instruction, _)) = server_receiver.recv_timeout(WAIT * 3) else {
                        break;
                    };
                    match instruction {
                        ServerInstruction::KillSession => break,
                        other => server.lock().push(other),
                    }
                }
            })
            .unwrap();
        let _ = to_plugin.send(PluginInstruction::AddClient(client_id));
        Harness {
            to_plugin,
            screen,
            server,
            threads: vec![screen_thread, server_thread],
            teardown: Some(teardown),
            client_id,
            dir,
        }
    }

    fn load_logger(&self, configuration: &[(&str, &str)], permissions: &[&str]) {
        let path = self.dir.path().join("logger.lua");
        std::fs::write(&path, logger_source(permissions)).unwrap();
        let url = format!("file:{}", path.display());
        let mut run_plugin = RunPluginOrAlias::from_url(&url, &None, None, None).unwrap();
        if let RunPluginOrAlias::RunPlugin(run_plugin) = &mut run_plugin {
            for (key, value) in configuration {
                run_plugin.configuration.insert(*key, *value);
            }
        }
        let _ = self.to_plugin.send(PluginInstruction::Load(
            Some(false),
            false,
            false,
            Some("batching_test_plugin".to_owned()),
            run_plugin,
            Some(1),
            None,
            self.client_id,
            Size { cols: 80, rows: 20 },
            None,
            None,
            false,
            None,
            None,
            None,
            None,
        ));
        self.wait_for("ready");
        if !permissions.is_empty() {
            self.wait_for("granted");
        }
    }

    fn add_client(&self, client_id: ClientId) {
        let _ = self.to_plugin.send(PluginInstruction::AddClient(client_id));
    }

    /// One `Update` carrying the whole batch, as the plugin thread's drain would hand it on.
    fn update(&self, events: Vec<(ClientId, Event)>) {
        let _ = self.to_plugin.send(PluginInstruction::Update(
            events
                .into_iter()
                .map(|(client_id, event)| (None, Some(client_id), event))
                .collect(),
        ));
    }

    fn cli_pipe(&self, name: &str) {
        let _ = self.to_plugin.send(PluginInstruction::CliPipe {
            pipe_id: PIPE.to_owned(),
            name: name.to_owned(),
            payload: None,
            plugin: None,
            args: None,
            configuration: None,
            floating: None,
            pane_id_to_replace: None,
            pane_title: None,
            cwd: None,
            skip_cache: false,
            cli_client_id: self.client_id,
            source_pane_id: None,
        });
    }

    /// Block until some rendered frame contains `needle`, and return that frame.
    fn wait_for(&self, needle: &str) -> String {
        self.screen.wait_until_described(
            &format!("a render containing {needle:?}"),
            WAIT,
            |log| {
                rendered_assets(log)
                    .into_iter()
                    .map(|(_, frame)| frame)
                    .find(|frame| frame.contains(needle))
            },
            |log| format!("{log:#?}"),
        )
    }

    /// Every `PluginBytes` instruction so far, as its (client, frame) assets.
    fn plugin_bytes(&self) -> Vec<Vec<(ClientId, String)>> {
        self.screen
            .lock()
            .iter()
            .filter_map(|instruction| match instruction {
                ScreenInstruction::PluginBytes(assets) => Some(
                    assets
                        .iter()
                        .map(|asset| (asset.client_id, frame_of(&asset.bytes)))
                        .collect(),
                ),
                _ => None,
            })
            .collect()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(teardown) = self.teardown.take() {
            teardown();
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn frame_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\n\r", "\n")
}

fn rendered_assets(instructions: &[ScreenInstruction]) -> Vec<(ClientId, String)> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::PluginBytes(assets) => Some(assets),
            _ => None,
        })
        .flatten()
        .map(|asset| (asset.client_id, frame_of(&asset.bytes)))
        .filter(|(_, frame)| !frame.is_empty())
        .collect()
}

fn scroll_up(lines: usize) -> Event {
    Event::Mouse(Mouse::ScrollUp(lines))
}

fn key(c: char) -> Event {
    Event::Key(KeyWithModifier::new(BareKey::Char(c)))
}

/// The `size RxC` head the logger prints before its log: the dimensions `render` was called
/// with, and the client the pass was made for.
fn size_and_client_of(frame: &str) -> (&str, &str) {
    let head = frame.lines().next().expect("every frame has a head line");
    let (size, client) = head
        .split_once(" client=")
        .unwrap_or_else(|| panic!("no size head in frame:\n{frame}"));
    (size, client)
}

/// The lines of `frame` after the plugin's `ready` line: what the batch made it log.
fn logged_after_ready(frame: &str) -> Vec<&str> {
    frame
        .lines()
        .skip_while(|line| *line != "ready")
        .skip(1)
        .collect()
}

#[test]
#[serial(plugin_tests)]
pub fn a_batch_of_scrolls_renders_once() {
    let harness = Harness::new();
    harness.load_logger(&[], &[]);

    harness.update((0..20).map(|_| (1, scroll_up(3))).collect());

    let frame = harness.wait_for(&format!("{}", "scroll_up 3 client=1\n".repeat(19)));
    assert_eq!(
        logged_after_ready(&frame),
        vec!["scroll_up 3 client=1"; 20],
        "every notch was delivered, in order, with its count"
    );
    // Counting renders is the point here, and the count is a property of the code rather than of
    // the scheduler: the batch goes in as *one* `Update`, and `drain_updates` can only merge
    // adjacent `Update`s, never split one - so one instruction is one `update_plugins` call, one
    // job, one `PluginBytes`. Nothing else can produce a scroll-bearing frame either: the logger
    // subscribes only to what this test sends, and it asks for no permissions, so the render a
    // permission answer forces never fires. This is why the general rule against render counts
    // (AGENTS.md) does not apply - and why the content assertion above cannot stand in for it:
    // the plugin logs the same twenty lines whether or not they were batched, so that assertion
    // passes on the un-batched code too. The property under test is the *absence* of the
    // nineteen intermediate frames.
    let renders_of_the_batch = harness
        .plugin_bytes()
        .into_iter()
        .filter(|assets| assets.iter().any(|(_, frame)| frame.contains("scroll_up")))
        .count();
    assert_eq!(
        renders_of_the_batch, 1,
        "and they cost one render between them"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn a_shared_plugin_sees_each_event_under_its_acting_client() {
    let harness = Harness::new();
    harness.load_logger(&[("shared", "true")], &[]);
    harness.add_client(2);

    harness.update(vec![(1, scroll_up(3)), (2, key('x')), (1, scroll_up(3))]);

    let frame = harness.wait_for("key x client=2\nscroll_up 3 client=1");
    assert_eq!(
        logged_after_ready(&frame),
        vec![
            "scroll_up 3 client=1",
            "key x client=2",
            "scroll_up 3 client=1"
        ],
        "each event ran as the client it was directed at"
    );
    // One `Update` in, so one render out, deterministically - see `a_batch_of_scrolls_renders_once`.
    let renders_of_the_batch: Vec<_> = harness
        .plugin_bytes()
        .into_iter()
        .filter(|assets| assets.iter().any(|(_, frame)| frame.contains("scroll_up")))
        .collect();
    assert_eq!(renders_of_the_batch.len(), 1, "one render for the batch");
    let mut clients: Vec<_> = renders_of_the_batch[0]
        .iter()
        .map(|(client_id, _)| *client_id)
        .collect();
    clients.sort();
    assert_eq!(
        clients,
        vec![1, 2],
        "with a frame for every attached client"
    );
}

/// One instance, one size, every client.
///
/// `render_plugin_for_clients` reads `rows`/`columns` off the instance once and then loops the
/// clients, and the session itself is reflowed to the *smallest* attached client
/// (`screen/sizing.rs`), so two terminals of different sizes do not give one plugin two
/// viewports. hocus leans on exactly this: it keeps a single `transcript_rows` and a single
/// measured `max_offset` for every client, and per-client state there would be dead weight.
/// If that ever stops being true, hocus's viewport bookkeeping is silently wrong — so fail
/// here, next to the host code that decides it, rather than there.
#[test]
#[serial(plugin_tests)]
pub fn a_shared_plugin_renders_every_client_at_one_size() {
    let harness = Harness::new();
    harness.load_logger(&[("shared", "true")], &[]);
    harness.add_client(2);

    harness.update(vec![(1, scroll_up(3))]);

    harness.wait_for("scroll_up 3 client=1");
    let batch = harness
        .plugin_bytes()
        .into_iter()
        .find(|assets| assets.iter().any(|(_, frame)| frame.contains("scroll_up")))
        .expect("the batch rendered");

    let mut heads: Vec<(&str, &str)> = batch
        .iter()
        .map(|(_, frame)| size_and_client_of(frame))
        .collect();
    heads.sort_by_key(|(_, client)| *client);
    let sizes: Vec<&str> = heads.iter().map(|(size, _)| *size).collect();
    let clients: Vec<&str> = heads.iter().map(|(_, client)| *client).collect();

    assert_eq!(clients, vec!["1", "2"], "a pass for each attached client");
    assert_eq!(
        sizes,
        vec!["size 20x80"; 2],
        "rendered at the instance's one size, not at a size of their own"
    );
}

/// A plugin may answer a batch without rendering and still unblock a CLI pipe from it; the
/// unblock rides on the render asset, so a batch that renders nothing must send it on its own.
#[test]
#[serial(plugin_tests)]
pub fn a_batch_that_renders_nothing_still_unblocks_cli_pipes() {
    let harness = Harness::new();
    harness.load_logger(&[], &["read_cli_pipes"]);

    harness.cli_pipe("hold");
    harness.wait_for("pipe hold");

    harness.update(vec![(1, key(UNBLOCK))]);

    harness.server.wait_until_described(
        "the pipe to be unblocked by a batch that did not render",
        WAIT,
        |log| {
            log.iter()
                .any(|instruction| {
                    matches!(instruction, ServerInstruction::UnblockCliPipeInput(pipe) if pipe == PIPE)
                })
                .then_some(())
        },
        |log| format!("{log:#?}"),
    );
    assert!(
        !harness
            .plugin_bytes()
            .iter()
            .any(|assets| assets.iter().any(|(_, frame)| frame.contains("key u"))),
        "the key did not render"
    );
}

/// A guest error drops the rest of the batch, but not the pipe changes the events before it
/// made: the `zellij pipe` client one of them released would otherwise stay blocked until some
/// later event happened to reach the plugin.
#[test]
#[serial(plugin_tests)]
pub fn an_error_mid_batch_still_delivers_the_unblocks_before_it() {
    let harness = Harness::new();
    harness.load_logger(&[], &["read_cli_pipes"]);

    harness.cli_pipe("hold");
    harness.wait_for("pipe hold");

    harness.update(vec![(1, key(UNBLOCK)), (1, key(RAISE))]);

    harness.server.wait_until_described(
        "the pipe to be unblocked although the batch's last event raised",
        WAIT,
        |log| unblocks_of(log, PIPE).then_some(()),
        |log| format!("{log:#?}"),
    );
}

/// Pipe changes are read off the plugin after each event, so a block after an unblock in the
/// same batch leaves the pipe blocked, as it did when every event was its own job. One drain
/// at the end would merge the two with unblock winning.
#[test]
#[serial(plugin_tests)]
pub fn the_last_change_to_a_pipe_in_a_batch_wins() {
    let harness = Harness::new();
    harness.load_logger(&[], &["read_cli_pipes"]);

    harness.cli_pipe("hold");
    harness.wait_for("pipe hold");

    harness.update(vec![(1, key(UNBLOCK)), (1, key(BLOCK))]);
    // a rendering batch behind it, to know the first has been handled in full
    harness.update(vec![(1, key('x'))]);
    harness.wait_for("key b client=1\nkey x client=1");
    assert!(
        !unblocks_of(&harness.server.lock(), PIPE),
        "the block came last, so the pipe stays held"
    );

    harness.update(vec![(1, key(UNBLOCK))]);
    harness.server.wait_until_described(
        "the pipe to be released by a later batch",
        WAIT,
        |log| unblocks_of(log, PIPE).then_some(()),
        |log| format!("{log:#?}"),
    );
}

/// Whether the server has been told to unblock `pipe`.
fn unblocks_of(log: &[ServerInstruction], pipe: &str) -> bool {
    log.iter().any(|instruction| {
        matches!(instruction, ServerInstruction::UnblockCliPipeInput(name) if name == pipe)
    })
}

/// A hover is a position sample: within one batch only the newest per client is delivered.
#[test]
#[serial(plugin_tests)]
pub fn hovers_in_a_batch_keep_only_the_last() {
    let harness = Harness::new();
    harness.load_logger(&[], &[]);

    let mut batch: Vec<(ClientId, Event)> = (1..=5)
        .map(|n| (1, Event::Mouse(Mouse::Hover(n, n as usize))))
        .collect();
    batch.push((1, scroll_up(3)));
    harness.update(batch);

    let frame = harness.wait_for("scroll_up 3 client=1");
    assert_eq!(
        logged_after_ready(&frame),
        vec!["hover 5,5 client=1", "scroll_up 3 client=1"],
        "the stale samples were dropped, the newest and the scroll kept"
    );
}

fn update_of(events: &[Event]) -> PluginInstruction {
    PluginInstruction::Update(
        events
            .iter()
            .map(|event| (None, Some(1), event.clone()))
            .collect(),
    )
}

fn bus_with(instructions: Vec<PluginInstruction>) -> Bus<PluginInstruction> {
    let (sender, receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let sender = SenderWithContext::new(sender);
    for instruction in instructions {
        sender.send(instruction).unwrap();
    }
    Bus::new(vec![receiver], None, None, None, None, None, None, None)
}

#[test]
pub fn drain_merges_adjacent_updates_and_stops_at_the_first_other_kind() {
    let bus = bus_with(vec![
        update_of(&[scroll_up(2)]),
        PluginInstruction::AddClient(7),
        update_of(&[scroll_up(3)]),
    ]);
    let mut queued = None;

    let merged = drain_updates(&bus, vec![(None, Some(1), scroll_up(1))], &mut queued);

    let events: Vec<_> = merged.iter().map(|(_, _, event)| event.clone()).collect();
    assert_eq!(
        events,
        vec![scroll_up(1), scroll_up(2)],
        "the adjacent update was merged in, in order"
    );
    assert!(
        matches!(queued, Some((PluginInstruction::AddClient(7), _))),
        "the first other kind was pulled and set aside for the next iteration"
    );
    let (left_behind, _) = bus
        .try_recv()
        .expect("the update behind it is still on the bus");
    assert!(matches!(left_behind, PluginInstruction::Update(ref updates) if updates.len() == 1));
    assert!(bus.try_recv().is_none());
}

#[test]
pub fn drain_stops_at_its_cap() {
    let bus = bus_with((0..300).map(|_| update_of(&[scroll_up(1)])).collect());
    let mut queued = None;

    let merged = drain_updates(&bus, vec![], &mut queued);

    assert_eq!(merged.len(), 256, "one pass merges at most the cap");
    assert!(queued.is_none());
    assert!(bus.try_recv().is_some(), "the rest waits for the next pass");
}

fn reload_of(url: &str) -> PluginInstruction {
    PluginInstruction::Reload(
        Some(false),
        None,
        RunPluginOrAlias::from_url(url, &None, None, None).unwrap(),
        1,
        Size::default(),
        None,
    )
}

fn reload_by_url_of(url: &str) -> PluginInstruction {
    PluginInstruction::ReloadPluginByUrl(
        RunPluginOrAlias::from_url(url, &None, None, None).unwrap(),
    )
}

/// A plugin asking for the same reload in a loop once filled this bus faster than the reloads
/// could run; the run behind the first one collapses into it.
#[test]
pub fn adjacent_reloads_of_one_plugin_collapse_to_one() {
    let bus = bus_with(vec![
        reload_of("file:/plugins/keys.lua"),
        reload_of("file:/plugins/keys.lua"),
        update_of(&[scroll_up(1)]),
        reload_of("file:/plugins/keys.lua"),
    ]);
    let mut queued = None;

    let taken = drain_reloads(&bus, reload_of("file:/plugins/keys.lua"), &mut queued);

    assert!(matches!(taken, PluginInstruction::Reload(..)));
    assert!(
        matches!(queued, Some((PluginInstruction::Update(_), _))),
        "the first other kind stops the drain and is set aside, in order"
    );
    let (left_behind, _) = bus.try_recv().expect("the reload behind the update stays");
    assert!(matches!(left_behind, PluginInstruction::Reload(..)));
    assert!(bus.try_recv().is_none());
}

/// Only the *same* plugin's reloads merge, and only the same kind of reload: a reload of
/// another plugin, or a by-url reload of this one, is a different instruction and stops the
/// drain rather than disappearing into the first.
#[test]
pub fn a_reload_of_another_plugin_or_kind_stops_the_drain() {
    let bus = bus_with(vec![
        reload_of("file:/plugins/other.lua"),
        reload_of("file:/plugins/keys.lua"),
    ]);
    let mut queued = None;
    drain_reloads(&bus, reload_of("file:/plugins/keys.lua"), &mut queued);
    assert!(
        matches!(&queued, Some((PluginInstruction::Reload(_, _, run, _, _, _), _))
            if run.location_string().ends_with("other.lua")),
        "got {queued:?}"
    );
    assert!(
        bus.try_recv().is_some(),
        "nothing past the stop was touched"
    );

    let bus = bus_with(vec![reload_by_url_of("file:/plugins/keys.lua")]);
    let mut queued = None;
    drain_reloads(&bus, reload_of("file:/plugins/keys.lua"), &mut queued);
    assert!(
        matches!(queued, Some((PluginInstruction::ReloadPluginByUrl(_), _))),
        "a by-url reload is not the same instruction as a pane reload"
    );
}

#[test]
pub fn coalescing_hovers_keeps_the_newest_per_client_and_nothing_else_moves() {
    let hover =
        |client: ClientId, at: isize| (None, Some(client), Event::Mouse(Mouse::Hover(at, 0)));
    let updates = vec![
        hover(1, 1),
        hover(2, 1),
        (None, Some(1), scroll_up(3)),
        hover(1, 2),
        (Some(4), Some(1), Event::Mouse(Mouse::Hover(9, 9))),
        hover(2, 2),
        (None, Some(1), scroll_up(3)),
    ];

    let coalesced = coalesce_hovers(updates);

    assert_eq!(
        coalesced,
        vec![
            (None, Some(1), scroll_up(3)),
            hover(1, 2),
            (Some(4), Some(1), Event::Mouse(Mouse::Hover(9, 9))),
            hover(2, 2),
            (None, Some(1), scroll_up(3)),
        ],
        "each client's last hover survives, keyed by plugin too; scrolls are untouched"
    );
}
