use super::*;
use notify_debouncer_full::notify::{
    event::{AccessKind, CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode},
    Event as NotifyEvent,
};
use std::path::PathBuf;
use std::time::Instant;

fn event(kind: EventKind, paths: &[&str]) -> DebouncedEvent {
    let event = paths.iter().fold(NotifyEvent::new(kind), |event, path| {
        event.add_path(Path::new("/workspace").join(path))
    });
    DebouncedEvent::new(event, Instant::now())
}

#[test]
fn multi_path_filesystem_events_keep_every_path() {
    let events = vec![
        event(EventKind::Create(CreateKind::File), &["new-a", "new-b"]),
        event(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            &["changed-a", "changed-b"],
        ),
        event(
            EventKind::Remove(RemoveKind::File),
            &["deleted-a", "deleted-b"],
        ),
        event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["rename-from", "rename-to"],
        ),
        event(EventKind::Access(AccessKind::Read), &["read-a", "read-b"]),
    ];

    assert_eq!(
        plugin_events_from_filesystem_events(events, Path::new("/workspace")),
        vec![
            Event::FileSystemRead(vec![
                (PathBuf::from("/host/read-a"), None),
                (PathBuf::from("/host/read-b"), None),
            ]),
            Event::FileSystemCreate(vec![
                (PathBuf::from("/host/new-a"), None),
                (PathBuf::from("/host/new-b"), None),
            ]),
            Event::FileSystemUpdate(vec![
                (PathBuf::from("/host/changed-a"), None),
                (PathBuf::from("/host/changed-b"), None),
                (PathBuf::from("/host/rename-from"), None),
                (PathBuf::from("/host/rename-to"), None),
            ]),
            Event::FileSystemDelete(vec![
                (PathBuf::from("/host/deleted-a"), None),
                (PathBuf::from("/host/deleted-b"), None),
            ]),
        ]
    );
}

#[test]
fn filesystem_events_outside_the_watched_root_are_not_exposed() {
    assert_eq!(
        plugin_events_from_filesystem_events(
            vec![event(
                EventKind::Create(CreateKind::File),
                &["inside", "/private/outside"]
            )],
            Path::new("/workspace"),
        ),
        vec![Event::FileSystemCreate(vec![(
            PathBuf::from("/host/inside"),
            None
        )])],
    );
}

#[test]
fn watchers_follow_each_plugins_host_folder_and_address_only_their_owner() {
    use yinz_runtime::channels::{self, SenderWithContext};

    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let replacement = tempfile::tempdir().unwrap();
    let (sender, receiver) = channels::unbounded();
    let mut one = crate::test_env::test_env();
    one.senders.to_plugin = Some(SenderWithContext::new(sender.clone()));
    one.generation = Some(42);
    one.plugin_cwd = first.path().to_owned();
    one.watch_filesystem();
    assert!(one.filesystem_watcher.is_some());
    let mut two = crate::test_env::test_env();
    two.senders.to_plugin = Some(SenderWithContext::new(sender));
    two.plugin_id = PluginId::new(2);
    two.client_id = ClientId::new(3);
    two.generation = Some(43);
    two.plugin_cwd = second.path().to_owned();
    two.watch_filesystem();
    assert!(two.filesystem_watcher.is_some());
    one.set_host_folder(replacement.path().to_owned());
    assert!(one.filesystem_watcher.is_some());
    std::fs::write(replacement.path().join("one"), "one").unwrap();
    std::fs::write(second.path().join("two"), "two").unwrap();

    let deadline = Instant::now() + yinz_test_support::default_timeout();
    let mut seen = [false; 2];
    while !seen.iter().all(|seen| *seen) {
        let (instruction, _) = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        let PluginInstruction::PluginEvent(plugin, client, generation, event) = instruction else {
            panic!("watch event was not scoped to a generation: {instruction:?}");
        };
        let paths = match event {
            Event::FileSystemCreate(paths) | Event::FileSystemUpdate(paths) => paths,
            _ => continue,
        };
        for (path, _) in paths {
            if path == Path::new("/host/one") {
                assert_eq!(
                    (plugin, client, generation),
                    (one.plugin_id, one.client_id, 42)
                );
                seen[0] = true;
            } else if path == Path::new("/host/two") {
                assert_eq!(
                    (plugin, client, generation),
                    (two.plugin_id, two.client_id, 43)
                );
                seen[1] = true;
            }
        }
    }
}
