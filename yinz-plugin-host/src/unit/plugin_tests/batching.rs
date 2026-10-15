//! A batch of events reaching one plugin costs it one render, not one per event.
//!
//! A wheel flick is tens of `ScrollUp(3)`s a second, each of which used to be its own executor
//! job, its own wasm `render()` per attached client and its own full re-parse on the screen
//! thread. The plugin thread now merges the `Update`s waiting on its bus and the wasm bridge
//! delivers a plugin its whole share of the batch in one job, rendering once at the end. Every
//! event is still delivered, in order, with its count - the tests here read them back off a Scheme
//! plugin that logs each one and renders the log.

#[cfg(test)]
const CLIENT_7: yinz_types::internal_ids::ClientId = yinz_types::internal_ids::ClientId::new(7);

use super::*;
use crate::ClientId;
use crate::{coalesce_hovers, drain_plugin_events, drain_reloads, drain_updates};
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;
use yinz_types::keys::{BareKey, KeyWithModifier};
use {yinz_types::data::Mouse, yinz_types::data::PermissionStatus};

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
        .join(" ");
    format!(
        r#"
(define lines '())
(define (log text) (set! lines (append lines (list text))))
(define (load config)
  (yinz-command (hash "Subscribe" '("Mouse" "Key" "PermissionRequestResult")))
  (unless (null? '({permissions})) (yinz-command (hash "RequestPluginPermissions" '({permissions}))))
  (log "ready"))
(define (update raw)
  (define client (number->string (hash-ref (yinz-command "GetPluginIds") "client_id")))
  (cond
    ((and (hash? raw) (hash-contains? raw "PermissionRequestResult")) (log "granted") #t)
    ((and (hash? raw) (hash-contains? raw "Mouse"))
      (define event (hash-ref raw "Mouse"))
      (define kind (car (hash-keys->list event)))
      (define position (hash-ref event kind))
      (define at (if (list? position) (string-join (map number->string position) ",") (number->string position)))
      (log (string-append kind " " at " client=" client)) #t)
    ((and (hash? raw) (hash-contains? raw "Key"))
      (define bare (hash-ref (hash-ref raw "Key") "bare_key"))
      (define key (if (hash? bare) (hash-ref bare "Char") bare))
      (log (string-append "key " key " client=" client))
      (cond ((equal? key "u") (yinz-command (hash "UnblockCliPipeInput" "batch_pipe")) #f)
        ((equal? key "b") (yinz-command (hash "BlockCliPipeInput" "batch_pipe")) #f)
        ((equal? key "e") (error "boom"))
        (else #t)))
    (else #f)))
(define (pipe message)
  (log (string-append "pipe " (hash-ref message "name")))
  (yinz-command (hash "BlockCliPipeInput" "batch_pipe")) #t)
(define (render rows cols)
  (string-append "size " (number->string rows) "x" (number->string cols)
    " client=" (number->string (hash-ref (yinz-command "GetPluginIds") "client_id"))
    "\n" (string-join lines "\n")))
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
        let client_id = ClientId::new(1);
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
                        ScreenInstruction::RequestPluginPermissions(_plugin_id, permission) => {
                            let _ = to_plugin.send(PluginInstruction::PermissionAnswer {
                                request_id: permission.request_id,
                                status: PermissionStatus::Granted,
                                cache_path: Some(cache_path.clone()),
                            });
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
        self.load_source(&logger_source(permissions), configuration, permissions);
    }

    fn load_source(&self, source: &str, configuration: &[(&str, &str)], permissions: &[&str]) {
        let path = self.dir.path().join("logger.scm");
        std::fs::write(&path, source).unwrap();
        let url = format!("file:{}", path.display());
        let mut run_plugin =
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(&url, &None, None, None)
                .unwrap();
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
            Some(TabId::new(1)),
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
            focus_plugin: false,
            is_eof: false,
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

    harness.update((0..20).map(|_| (ClientId::new(1), scroll_up(3))).collect());

    let frame = harness.wait_for(&"ScrollUp 3 client=1\n".repeat(19));
    assert_eq!(
        logged_after_ready(&frame),
        vec!["ScrollUp 3 client=1"; 20],
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
        .filter(|assets| assets.iter().any(|(_, frame)| frame.contains("ScrollUp")))
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
    harness.add_client(crate::ClientId::new(2));

    harness.update(vec![
        (crate::ClientId::new(1), scroll_up(3)),
        (crate::ClientId::new(2), key('x')),
        (crate::ClientId::new(1), scroll_up(3)),
    ]);

    let frame = harness.wait_for("key x client=2\nScrollUp 3 client=1");
    assert_eq!(
        logged_after_ready(&frame),
        vec![
            "ScrollUp 3 client=1",
            "key x client=2",
            "ScrollUp 3 client=1"
        ],
        "each event ran as the client it was directed at"
    );
    // One `Update` in, so one render out, deterministically - see `a_batch_of_scrolls_renders_once`.
    let renders_of_the_batch: Vec<_> = harness
        .plugin_bytes()
        .into_iter()
        .filter(|assets| assets.iter().any(|(_, frame)| frame.contains("ScrollUp")))
        .collect();
    assert_eq!(renders_of_the_batch.len(), 1, "one render for the batch");
    let mut clients: Vec<_> = renders_of_the_batch[0]
        .iter()
        .map(|(client_id, _)| *client_id)
        .collect();
    clients.sort();
    assert_eq!(
        clients,
        vec![ClientId::new(1), ClientId::new(2)],
        "with a frame for every attached client"
    );
}

/// One instance, one size, every client.
///
/// `render_plugin_for_clients` reads `rows`/`columns` off the instance once and then loops the
/// clients, and the session itself is reflowed to the *smallest* attached client
/// (`screen/sizing.rs`), so two terminals of different sizes do not give one plugin two
/// viewports. agent leans on exactly this: it keeps a single `transcript_rows` and a single
/// measured `max_offset` for every client, and per-client state there would be dead weight.
/// If that ever stops being true, agent's viewport bookkeeping is silently wrong — so fail
/// here, next to the host code that decides it, rather than there.
#[test]
#[serial(plugin_tests)]
pub fn a_shared_plugin_renders_every_client_at_one_size() {
    let harness = Harness::new();
    harness.load_logger(&[("shared", "true")], &[]);
    harness.add_client(crate::ClientId::new(2));

    harness.update(vec![(crate::ClientId::new(1), scroll_up(3))]);

    harness.wait_for("ScrollUp 3 client=1");
    let batch = harness
        .plugin_bytes()
        .into_iter()
        .find(|assets| assets.iter().any(|(_, frame)| frame.contains("ScrollUp")))
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
    harness.load_logger(&[], &["ReadCliPipes"]);

    harness.cli_pipe("hold");
    harness.wait_for("pipe hold");

    harness.update(vec![(crate::ClientId::new(1), key(UNBLOCK))]);

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
/// made: the `yinz pipe` client one of them released would otherwise stay blocked until some
/// later event happened to reach the plugin.
#[test]
#[serial(plugin_tests)]
pub fn an_error_mid_batch_still_delivers_the_unblocks_before_it() {
    let harness = Harness::new();
    harness.load_logger(&[], &["ReadCliPipes"]);

    harness.cli_pipe("hold");
    harness.wait_for("pipe hold");

    harness.update(vec![
        (crate::ClientId::new(1), key(UNBLOCK)),
        (crate::ClientId::new(1), key(RAISE)),
    ]);

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
    harness.load_logger(&[], &["ReadCliPipes"]);

    harness.cli_pipe("hold");
    harness.wait_for("pipe hold");

    harness.update(vec![
        (crate::ClientId::new(1), key(UNBLOCK)),
        (crate::ClientId::new(1), key(BLOCK)),
    ]);
    // a rendering batch behind it, to know the first has been handled in full
    harness.update(vec![(crate::ClientId::new(1), key('x'))]);
    harness.wait_for("key b client=1\nkey x client=1");
    assert!(
        !unblocks_of(&harness.server.lock(), PIPE),
        "the block came last, so the pipe stays held"
    );

    harness.update(vec![(crate::ClientId::new(1), key(UNBLOCK))]);
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
        .map(|n| (ClientId::new(1), Event::Mouse(Mouse::Hover(n, n as usize))))
        .collect();
    batch.push((crate::ClientId::new(1), scroll_up(3)));
    harness.update(batch);

    let frame = harness.wait_for("ScrollUp 3 client=1");
    assert_eq!(
        logged_after_ready(&frame),
        vec!["Hover 5,5 client=1", "ScrollUp 3 client=1"],
        "the stale samples were dropped, the newest and the scroll kept"
    );
}

fn update_of(events: &[Event]) -> PluginInstruction {
    PluginInstruction::Update(
        events
            .iter()
            .map(|event| (None, Some(ClientId::new(1)), event.clone()))
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
fn deno_drain_preserves_clients_order_and_each_instruction_boundary() {
    let id = crate::PluginId::new(1);
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    for boundary in [
        PluginInstruction::PluginEvent(id, first, 42, scroll_up(3)),
        PluginInstruction::PluginEvent(crate::PluginId::new(2), first, 41, scroll_up(3)),
        PluginInstruction::AddClient(second),
        update_of(&[scroll_up(3)]),
    ] {
        let expected = format!("{boundary:?}");
        let bus = bus_with(vec![
            PluginInstruction::PluginEvent(id, second, 41, scroll_up(2)),
            boundary,
            PluginInstruction::PluginEvent(id, first, 41, scroll_up(4)),
        ]);
        let mut queued = None;
        assert_eq!(
            drain_plugin_events(&bus, id, 41, vec![(first, scroll_up(1), None)], &mut queued),
            vec![(first, scroll_up(1), None), (second, scroll_up(2), None)],
        );
        assert_eq!(format!("{:?}", queued.unwrap().0), expected);
        assert!(matches!(
            bus.try_recv(),
            Some((PluginInstruction::PluginEvent(_, _, 41, _), _))
        ));
        assert!(bus.try_recv().is_none());
    }
}

#[test]
fn deno_drain_stops_at_its_cap() {
    let id = crate::PluginId::new(1);
    let client = ClientId::new(1);
    let bus = bus_with(
        (0..300)
            .map(|_| PluginInstruction::PluginEvent(id, client, 41, scroll_up(1)))
            .collect(),
    );
    let mut queued = None;
    let events = drain_plugin_events(
        &bus,
        id,
        41,
        vec![(client, scroll_up(1), None)],
        &mut queued,
    );
    assert_eq!(events.len(), 1 + crate::MAX_UPDATE_DRAIN);
    assert!(queued.is_none());
    assert!(bus.try_recv().is_some());
}

#[test]
pub fn drain_merges_adjacent_updates_and_stops_at_the_first_other_kind() {
    let bus = bus_with(vec![
        update_of(&[scroll_up(2)]),
        PluginInstruction::AddClient(crate::ClientId::new(7)),
        update_of(&[scroll_up(3)]),
    ]);
    let mut queued = None;

    let merged = drain_updates(
        &bus,
        vec![(None, Some(crate::ClientId::new(1)), scroll_up(1))],
        &mut queued,
    );

    let events: Vec<_> = merged.iter().map(|(_, _, event)| event.clone()).collect();
    assert_eq!(
        events,
        vec![scroll_up(1), scroll_up(2)],
        "the adjacent update was merged in, in order"
    );
    assert!(
        matches!(queued, Some((PluginInstruction::AddClient(CLIENT_7), _))),
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
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(url, &None, None, None)
            .unwrap(),
        TabId::new(1),
        Size::default(),
        None,
    )
}

fn reload_by_url_of(url: &str) -> PluginInstruction {
    PluginInstruction::ReloadPluginByUrl(
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(url, &None, None, None)
            .unwrap(),
    )
}

/// A plugin asking for the same reload in a loop once filled this bus faster than the reloads
/// could run; the run behind the first one collapses into it.
#[test]
pub fn adjacent_reloads_of_one_plugin_collapse_to_one() {
    let bus = bus_with(vec![
        reload_of("file:/plugins/keys.scm"),
        reload_of("file:/plugins/keys.scm"),
        update_of(&[scroll_up(1)]),
        reload_of("file:/plugins/keys.scm"),
    ]);
    let mut queued = None;

    let (taken, _) = drain_reloads(&bus, reload_of("file:/plugins/keys.scm"), &mut queued);

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
        reload_of("file:/plugins/other.scm"),
        reload_of("file:/plugins/keys.scm"),
    ]);
    let mut queued = None;
    drain_reloads(&bus, reload_of("file:/plugins/keys.scm"), &mut queued);
    assert!(
        matches!(&queued, Some((PluginInstruction::Reload(_, _, run, _, _, _), _))
            if run.location_string().ends_with("other.scm")),
        "got {queued:?}"
    );
    assert!(
        bus.try_recv().is_some(),
        "nothing past the stop was touched"
    );

    let bus = bus_with(vec![reload_by_url_of("file:/plugins/keys.scm")]);
    let mut queued = None;
    drain_reloads(&bus, reload_of("file:/plugins/keys.scm"), &mut queued);
    assert!(
        matches!(queued, Some((PluginInstruction::ReloadPluginByUrl(_), _))),
        "a by-url reload is not the same instruction as a pane reload"
    );
}

/// A `Reload` also says where the pane goes if the plugin turns out not to be running, so two
/// that name the same plugin but differ there are two requests: the second stops the drain.
#[test]
pub fn a_reload_of_the_same_plugin_with_other_pane_arguments_stops_the_drain() {
    let differing = [
        PluginInstruction::Reload(
            Some(true), // floating rather than tiled
            None,
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                "file:/plugins/keys.scm",
                &None,
                None,
                None,
            )
            .unwrap(),
            TabId::new(1),
            Size::default(),
            None,
        ),
        PluginInstruction::Reload(
            Some(false),
            Some("titled".to_owned()),
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                "file:/plugins/keys.scm",
                &None,
                None,
                None,
            )
            .unwrap(),
            TabId::new(1),
            Size::default(),
            None,
        ),
        PluginInstruction::Reload(
            Some(false),
            None,
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                "file:/plugins/keys.scm",
                &None,
                None,
                None,
            )
            .unwrap(),
            TabId::new(2), // another tab
            Size::default(),
            None,
        ),
    ];
    for other in differing {
        let bus = bus_with(vec![other]);
        let mut queued = None;
        drain_reloads(&bus, reload_of("file:/plugins/keys.scm"), &mut queued);
        assert!(
            matches!(queued, Some((PluginInstruction::Reload(..), _))),
            "a reload with other pane arguments is left for its own turn, got {queued:?}"
        );
    }
}

/// The CLIs waiting on collapsed reloads are answered when their completion handle is dropped,
/// so the handles come back to the caller rather than being dropped in the drain — otherwise
/// they would report success before the reload they were folded into had run.
#[test]
pub fn collapsed_reloads_hand_their_completion_handles_to_the_caller() {
    let run = || {
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            "file:/plugins/keys.scm",
            &None,
            None,
            None,
        )
        .unwrap()
    };
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let waiting = PluginInstruction::Reload(
        Some(false),
        None,
        run(),
        TabId::new(1),
        Size::default(),
        Some(yinz_bus::route::NotificationEnd::new(tx)),
    );
    let bus = bus_with(vec![waiting]);
    let mut queued = None;

    let (_first, completions) =
        drain_reloads(&bus, reload_of("file:/plugins/keys.scm"), &mut queued);

    assert_eq!(
        completions.len(),
        1,
        "the collapsed reload's handle comes back"
    );
    assert!(
        rx.try_recv().is_err(),
        "nothing has been answered while the handle is still held"
    );
    drop(completions);
    assert!(
        rx.try_recv().is_ok(),
        "dropping the handle is what answers the CLI"
    );
}

/// `NotificationEnd::join` is how collapsed reloads ride along with the survivor: they are
/// answered when it is, with whatever it was told - here the pane id an `AddPlugin` sets.
#[test]
pub fn joined_completions_are_answered_together_with_the_primarys_payload() {
    use yinz_bus::route::NotificationEnd;
    let (tx_a, mut rx_a) = tokio::sync::oneshot::channel();
    let (tx_b, mut rx_b) = tokio::sync::oneshot::channel();
    let (tx_c, mut rx_c) = tokio::sync::oneshot::channel();

    let mut joined = NotificationEnd::join(
        Some(NotificationEnd::new(tx_a)),
        vec![NotificationEnd::new(tx_b), NotificationEnd::new(tx_c)],
    )
    .expect("a primary always yields a handle");
    joined.set_affected_pane_id(PaneId::Plugin(crate::PluginId::new(7)));
    assert!(
        rx_a.try_recv().is_err() && rx_b.try_recv().is_err() && rx_c.try_recv().is_err(),
        "nobody is answered while the joined handle is held"
    );

    drop(joined);
    for rx in [&mut rx_a, &mut rx_b, &mut rx_c] {
        let result = rx
            .try_recv()
            .expect("answered when the joined handle drops");
        assert_eq!(
            result.affected_pane_id,
            Some(PaneId::Plugin(crate::PluginId::new(7)))
        );
    }

    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let promoted = NotificationEnd::join(None, vec![NotificationEnd::new(tx)])
        .expect("with no primary the first follower stands in");
    assert!(rx.try_recv().is_err());
    drop(promoted);
    assert!(rx.try_recv().is_ok());

    assert!(NotificationEnd::join(None, vec![]).is_none());
}

/// Two identical reloads of a plugin that is not running: the survivor becomes an `AddPlugin`
/// carrying its completion, and the collapsed one's must go the same way rather than being
/// dropped at the end of the plugin thread's turn - which answered its CLI before the pane
/// existed and with no pane id.
#[test]
#[serial(plugin_tests)]
pub fn collapsed_reloads_of_an_unloaded_plugin_are_answered_by_the_add_plugin() {
    use yinz_bus::route::NotificationEnd;
    use {
        yinz_types::input::layout::PluginUserConfiguration, yinz_types::input::layout::RunPlugin,
        yinz_types::input::layout::RunPluginLocation,
    };

    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let run_plugin = || {
        RunPluginOrAlias::RunPlugin(RunPlugin {
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from(&*SCRIPT_PLUGIN_FIXTURE)),
            configuration: PluginUserConfiguration::new(BTreeMap::new()),
            ..Default::default()
        })
    };
    let (tx_first, mut rx_first) = tokio::sync::oneshot::channel();
    let (tx_second, mut rx_second) = tokio::sync::oneshot::channel();
    // The client the plugin loads for. Handling it also keeps the thread busy while both reloads
    // land on the bus behind it, so the second is there to collapse when the first is taken.
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(crate::ClientId::new(1)));
    for tx in [tx_first, tx_second] {
        let _ = plugin_thread_sender.send(PluginInstruction::Reload(
            Some(false),
            None,
            run_plugin(),
            TabId::new(1),
            Size { rows: 20, cols: 80 },
            Some(NotificationEnd::new(tx)),
        ));
    }

    let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
    let mut add_plugins = vec![];
    while std::time::Instant::now() < deadline {
        let Ok((instruction, _)) =
            screen_receiver.recv_timeout(std::time::Duration::from_millis(200))
        else {
            continue;
        };
        if let ScreenInstruction::AddPlugin(..) = instruction {
            add_plugins.push(instruction);
            if add_plugins.len() == 1 {
                // Give a second `AddPlugin` - a reload that did not collapse - time to show.
                std::thread::sleep(std::time::Duration::from_millis(300));
                while let Ok((instruction, _)) = screen_receiver.try_recv() {
                    if let ScreenInstruction::AddPlugin(..) = instruction {
                        add_plugins.push(instruction);
                    }
                }
                break;
            }
        }
    }
    assert_eq!(
        add_plugins.len(),
        1,
        "one reload of a plugin that is not running starts it once"
    );
    assert!(
        rx_first.try_recv().is_err() && rx_second.try_recv().is_err(),
        "neither CLI is answered while the screen still holds the AddPlugin"
    );

    // What the screen does once the pane exists.
    let ScreenInstruction::AddPlugin(_, _, _, _, _, _, plugin_id, _, _, _, _, _, _, completion, _) =
        add_plugins.pop().unwrap()
    else {
        unreachable!()
    };
    let mut completion = completion.expect("the survivor carries its completion");
    completion.set_affected_pane_id(PaneId::Plugin(plugin_id));
    drop(completion);
    for rx in [&mut rx_first, &mut rx_second] {
        let result = rx
            .try_recv()
            .expect("both CLIs are answered by the AddPlugin");
        assert_eq!(result.affected_pane_id, Some(PaneId::Plugin(plugin_id)));
    }
    teardown();
}

#[test]
pub fn coalescing_hovers_keeps_positions_before_wheel_events() {
    let hover =
        |client: ClientId, at: isize| (None, Some(client), Event::Mouse(Mouse::Hover(at, 0)));
    let updates = vec![
        hover(crate::ClientId::new(1), 0),
        hover(crate::ClientId::new(1), 1),
        hover(crate::ClientId::new(2), 1),
        (None, Some(crate::ClientId::new(1)), scroll_up(3)),
        hover(crate::ClientId::new(1), 0),
        hover(crate::ClientId::new(1), 2),
        (
            Some(yinz_types::internal_ids::PluginId::new(4)),
            Some(crate::ClientId::new(1)),
            Event::Mouse(Mouse::Hover(9, 9)),
        ),
        hover(crate::ClientId::new(2), 2),
        (None, Some(crate::ClientId::new(1)), scroll_up(3)),
    ];

    let coalesced = coalesce_hovers(updates);

    assert_eq!(
        coalesced,
        vec![
            hover(crate::ClientId::new(1), 1),
            hover(crate::ClientId::new(2), 1),
            (None, Some(ClientId::new(1)), scroll_up(3)),
            hover(crate::ClientId::new(1), 2),
            (
                Some(yinz_types::internal_ids::PluginId::new(4)),
                Some(ClientId::new(1)),
                Event::Mouse(Mouse::Hover(9, 9))
            ),
            hover(crate::ClientId::new(2), 2),
            (None, Some(ClientId::new(1)), scroll_up(3)),
        ],
        "each client's last hover survives before each wheel, keyed by plugin too"
    );
}

#[test]
#[serial(plugin_tests)]
fn asynchronous_pipe_hold_releases_after_guest_render_output_without_screen_acknowledgements() {
    let harness = Harness::new();
    harness.load_source(
        r#"
(define stream #f)
(define ready #f)
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("ReadCliPipes"))
    (lambda (_) (set! ready #t) (yinz-request-render))))
(define (pipe message)
  (set! stream (hash-ref (hash-ref message "source") "Cli"))
  (yinz-command (hash "BlockCliPipeInput" stream))
  (yinz-command (hash "CliPipeOutput" (list stream "callback"))) #t)
(define (render rows cols)
  (if stream
    (let ((current stream))
      (set! stream #f)
      (yinz-then (yinz-sleep 0) (lambda (_)
        (yinz-command (hash "CliPipeOutput" (list current "async render")))
        (yinz-command (hash "UnblockCliPipeInput" current))
        (if ready "ready granted" "starting"))))
    (if ready "ready granted" "starting")))
"#,
        &[],
        &["ReadCliPipes"],
    );
    harness.cli_pipe("async");
    harness.server.wait_until("asynchronous hold completes", WAIT, |log| {
        log.iter().any(|instruction| matches!(instruction, ServerInstruction::UnblockCliPipeInput(pipe) if pipe == PIPE)).then_some(())
    });
    let outcome: Vec<_> = harness
        .server
        .lock()
        .iter()
        .filter_map(|instruction| match instruction {
            ServerInstruction::CliPipeOutput(pipe, output) if pipe == PIPE => Some(output.clone()),
            ServerInstruction::UnblockCliPipeInput(pipe) if pipe == PIPE => Some("success".into()),
            ServerInstruction::CliPipeError(pipe, error) if pipe == PIPE => panic!("{error}"),
            _ => None,
        })
        .collect();
    assert_eq!(outcome, ["callback", "async render", "success"]);
}
