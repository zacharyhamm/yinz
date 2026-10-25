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
#[path = "unit/watch_filesystem.rs"]
mod tests;
