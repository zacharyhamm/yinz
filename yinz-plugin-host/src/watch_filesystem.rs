use super::{ClientId, PluginId, PluginInstruction};
use crate::plugin_map::PluginEnv;

use std::path::Path;
use std::time::Duration;
use yinz_bus::thread_bus::ThreadSenders;

use notify_debouncer_full::{
    new_debouncer_opt,
    notify::{Config, EventKind, RecommendedWatcher, RecursiveMode},
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::Event;

const DEBOUNCE_DURATION_MS: u64 = 400;

pub(crate) type FilesystemWatcher = Debouncer<RecommendedWatcher, RecommendedCache>;

impl PluginEnv {
    pub(crate) fn watch_filesystem(&mut self) {
        if self.filesystem_watcher.is_none() {
            watch_filesystem(
                self.senders.clone(),
                &self.plugin_cwd,
                self.plugin_id,
                self.client_id,
                self.generation,
            )
            .map(|watcher| self.filesystem_watcher = Some(watcher))
            .context("failed to watch plugin host folder")
            .non_fatal();
        }
    }

    pub(crate) fn set_host_folder(&mut self, path: std::path::PathBuf) {
        let was_watching = self.filesystem_watcher.take().is_some();
        self.plugin_cwd = path;
        if was_watching {
            self.watch_filesystem();
        }
    }
}

fn plugin_events_from_filesystem_events(
    events: Vec<DebouncedEvent>,
    current_dir: &Path,
) -> Vec<Event> {
    let path_prefix_in_plugins = Path::new("/host");
    let mut create_paths = vec![];
    let mut read_paths = vec![];
    let mut update_paths = vec![];
    let mut delete_paths = vec![];

    for event in events {
        let paths = match event.kind {
            EventKind::Access(_) => &mut read_paths,
            EventKind::Create(_) => &mut create_paths,
            EventKind::Modify(_) => &mut update_paths,
            EventKind::Remove(_) => &mut delete_paths,
            _ => continue,
        };
        paths.extend(event.paths.iter().filter_map(|path| {
            path.strip_prefix(current_dir)
                .ok()
                .map(|relative| path_prefix_in_plugins.join(relative))
        }));
    }

    // Watch events are path-only; fs_metadata supplies current metadata on demand.
    let mut plugin_events = vec![];
    if !read_paths.is_empty() {
        plugin_events.push(Event::FileSystemRead(
            read_paths.into_iter().map(|path| (path, None)).collect(),
        ));
    }
    if !create_paths.is_empty() {
        plugin_events.push(Event::FileSystemCreate(
            create_paths.into_iter().map(|path| (path, None)).collect(),
        ));
    }
    if !update_paths.is_empty() {
        plugin_events.push(Event::FileSystemUpdate(
            update_paths.into_iter().map(|path| (path, None)).collect(),
        ));
    }
    if !delete_paths.is_empty() {
        plugin_events.push(Event::FileSystemDelete(
            delete_paths.into_iter().map(|path| (path, None)).collect(),
        ));
    }
    plugin_events
}

fn watch_filesystem(
    senders: ThreadSenders,
    plugin_cwd: &Path,
    plugin_id: PluginId,
    client_id: ClientId,
    generation: Option<u64>,
) -> Result<FilesystemWatcher> {
    let current_dir = plugin_cwd
        .canonicalize()
        .context("resolving watched host folder")?;
    let watched_dir = current_dir.clone();
    let mut debouncer = new_debouncer_opt(
        Duration::from_millis(DEBOUNCE_DURATION_MS),
        None,
        move |result: DebounceEventResult| match result {
            Ok(events) => {
                for event in plugin_events_from_filesystem_events(events, &current_dir) {
                    let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
                        plugin_id, client_id, generation, event,
                    ));
                }
            },
            Err(errors) => errors
                .iter()
                .for_each(|error| log::error!("watch error: {error:?}")),
        },
        RecommendedCache::new(),
        Config::default().with_follow_symlinks(false),
    )?;

    debouncer.watch(&watched_dir, RecursiveMode::Recursive)?;
    Ok(debouncer)
}

#[cfg(test)]
mod tests {
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
            let PluginInstruction::PluginEvent(plugin, client, generation, event) = instruction
            else {
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
}
