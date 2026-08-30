use super::PluginInstruction;
use std::path::PathBuf;

use crate::thread_bus::ThreadSenders;
use std::path::Path;
use std::time::Duration;

use notify_debouncer_full::{
    new_debouncer,
    notify::{EventKind, RecommendedWatcher, RecursiveMode},
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache,
};
use zellij_utils::{data::Event, errors::prelude::Result};

const DEBOUNCE_DURATION_MS: u64 = 400;

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
        paths.extend(event.paths.iter().map(|path| {
            let stripped_path = path.strip_prefix(current_dir).unwrap_or(path);
            path_prefix_in_plugins.join(stripped_path)
        }));
    }

    // TODO: at some point we might want to add FileMetadata to these, but right now
    // the API is a bit unstable, so let's not rock the boat too much by adding another
    // expensive syscall
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

pub fn watch_filesystem(
    senders: ThreadSenders,
    zellij_cwd: &Path,
) -> Result<Debouncer<RecommendedWatcher, RecommendedCache>> {
    let current_dir = PathBuf::from(zellij_cwd);
    let mut debouncer = new_debouncer(
        Duration::from_millis(DEBOUNCE_DURATION_MS),
        None,
        move |result: DebounceEventResult| match result {
            Ok(events) => {
                let updates = plugin_events_from_filesystem_events(events, &current_dir)
                    .into_iter()
                    .map(|event| (None, None, event))
                    .collect::<Vec<_>>();
                if !updates.is_empty() {
                    let _ = senders.send_to_plugin(PluginInstruction::Update(updates));
                }
            },
            Err(errors) => errors
                .iter()
                .for_each(|error| log::error!("watch error: {error:?}")),
        },
    )?;

    debouncer.watch(zellij_cwd, RecursiveMode::Recursive)?;
    Ok(debouncer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify_debouncer_full::notify::{
        event::{AccessKind, CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode},
        Event as NotifyEvent,
    };
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
}
