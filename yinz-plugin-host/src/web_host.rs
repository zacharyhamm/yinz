//! Session publication ownership and per-incarnation request slots.
use std::collections::{BTreeMap, HashMap};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use yinz_bus::web::ReplySlot;
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::PluginId;
use yinz_types::web::{Site, SiteHandle, StaticFile, WebReply};

pub(crate) type WebPending = HashMap<u64, ReplySlot>;
pub(crate) fn unavailable() -> WebReply {
    WebReply::Unavailable {
        reason: "web plugin unavailable".into(),
        retry_after_s: 5,
    }
}

/// A queued executor closure can disappear without running (for example at shutdown).
/// Resolve explicitly even when a bus instruction still holds another slot clone.
pub(crate) struct QueuedReply(Option<ReplySlot>);
impl QueuedReply {
    pub fn new(reply: ReplySlot) -> Self {
        Self(Some(reply))
    }
    pub fn delivered(&mut self) {
        self.0.take();
    }
}
impl Drop for QueuedReply {
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            reply.send(unavailable());
        }
    }
}

#[derive(Clone, Default, Debug)]
pub(crate) struct WebHost(Arc<Mutex<State>>);
#[derive(Debug)]
struct Instance {
    id: PluginId,
    pending: Arc<Mutex<WebPending>>,
    unavailable: Arc<AtomicBool>,
}
#[derive(Default, Debug)]
struct State {
    target: Option<PluginId>,
    site: Option<SiteHandle>,
    instances: BTreeMap<u64, Instance>,
}
impl WebHost {
    pub fn new(site: Option<SiteHandle>) -> Self {
        Self(Arc::new(Mutex::new(State {
            site,
            ..Default::default()
        })))
    }
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn set_target(&self, id: Option<PluginId>) {
        let mut state = self.state();
        if state.target != id
            && let Some(site) = &state.site
        {
            site.set(None);
        }
        state.target = id;
    }
    pub fn is_target(&self, id: PluginId) -> bool {
        self.state().target == Some(id)
    }
    pub fn register(
        &self,
        id: PluginId,
        generation: u64,
        pending: Arc<Mutex<WebPending>>,
        unavailable: Arc<AtomicBool>,
    ) {
        self.state().instances.insert(
            generation,
            Instance {
                id,
                pending,
                unavailable,
            },
        );
    }
    /// First live parent instance; workers never register publication or reply authority.
    pub fn selected(&self, id: PluginId) -> Option<u64> {
        let state = self.state();
        (state.target == Some(id))
            .then(|| Self::selected_in(&state, id))
            .flatten()
    }
    fn selected_in(state: &State, id: PluginId) -> Option<u64> {
        state
            .instances
            .iter()
            .find(|(_, instance)| {
                instance.id == id && !instance.unavailable.load(Ordering::Acquire)
            })
            .map(|(generation, _)| *generation)
    }
    /// Admit under the same lock as retirement, so an unload cannot miss a new slot.
    pub fn admit(
        &self,
        id: PluginId,
        generation: u64,
        request_id: u64,
        slot: ReplySlot,
    ) -> std::result::Result<(), WebReply> {
        let state = self.state();
        if state.target != Some(id) || Self::selected_in(&state, id) != Some(generation) {
            return Err(unavailable());
        }
        let instance = state.instances.get(&generation).ok_or_else(unavailable)?;
        let mut pending = instance.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|_, slot| !slot.is_closed());
        if pending.len() >= 64 || pending.contains_key(&request_id) {
            return Err(WebReply::Unavailable {
                reason: "busy".into(),
                retry_after_s: 2,
            });
        }
        pending.insert(request_id, slot);
        Ok(())
    }

    fn edit_site(
        &self,
        id: PluginId,
        generation: u64,
        edit: impl FnOnce(&mut Site) -> Result<()>,
    ) -> Result<()> {
        let state = self.state();
        anyhow::ensure!(
            state.target == Some(id) && Self::selected_in(&state, id) == Some(generation),
            "only the live configured web plugin can publish the site"
        );
        let handle = state
            .site
            .as_ref()
            .context("the web interface is not enabled in this session")?;
        let mut site = match handle.current() {
            Some(site) if site.generation == generation && site.plugin_id == Some(id) => {
                Site::clone(&site)
            },
            _ => Site {
                plugin_id: Some(id),
                generation,
                ..Default::default()
            },
        };
        edit(&mut site)?;
        handle.set(Some(Arc::new(site)));
        Ok(())
    }
    pub fn publish_assets(
        &self,
        id: PluginId,
        generation: u64,
        prefix: String,
        files: BTreeMap<String, StaticFile>,
    ) -> Result<()> {
        validate_path(&prefix, true)?;
        self.edit_site(id, generation, |site| {
            anyhow::ensure!(
                site.assets.contains_key(&prefix) || site.assets.len() < 4,
                "too many asset prefixes"
            );
            anyhow::ensure!(
                !site.pages.keys().any(|path| path.starts_with(&prefix)),
                "asset prefix contains a page"
            );
            site.assets.insert(prefix, files);
            Ok(())
        })
    }
    pub fn publish_page(
        &self,
        id: PluginId,
        generation: u64,
        path: String,
        file: StaticFile,
    ) -> Result<()> {
        validate_path(&path, false)?;
        anyhow::ensure!(file.bytes.len() <= 512 * 1024, "page exceeds 512 KiB");
        self.edit_site(id, generation, |site| {
            anyhow::ensure!(
                site.pages.contains_key(&path) || site.pages.len() < 16,
                "too many pages"
            );
            anyhow::ensure!(
                !site.assets.keys().any(|prefix| path.starts_with(prefix)),
                "page is under an asset prefix"
            );
            site.pages.insert(path, file);
            Ok(())
        })
    }
    pub fn retire_plugin(&self, id: PluginId) {
        let mut state = self.state();
        let generations: Vec<_> = state
            .instances
            .iter()
            .filter(|(_, instance)| instance.id == id)
            .map(|(generation, _)| *generation)
            .collect();
        for generation in generations {
            Self::retire_in(&mut state, generation);
        }
    }
    pub fn retire(&self, generation: u64) {
        Self::retire_in(&mut self.state(), generation);
    }
    fn retire_in(state: &mut State, generation: u64) {
        if let Some(instance) = state.instances.remove(&generation) {
            instance.unavailable.store(true, Ordering::Release);
            for (_, slot) in instance
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain()
            {
                slot.send(unavailable());
            }
        }
        if let Some(handle) = &state.site
            && handle
                .current()
                .is_some_and(|site| site.generation == generation)
        {
            handle.set(None);
        }
    }
}
fn validate_path(path: &str, prefix: bool) -> Result<()> {
    anyhow::ensure!(
        path.starts_with('/') && !path.starts_with("/api/") && path != "/api",
        "invalid public path"
    );
    anyhow::ensure!(
        path.bytes().all(|byte| byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || b"/_-".contains(&byte)
            || (!prefix && byte == b'.')),
        "invalid public path characters"
    );
    let components = path[1..].strip_suffix('/').unwrap_or(&path[1..]);
    anyhow::ensure!(
        path == "/"
            || !components
                .split('/')
                .any(|part| part == "." || part == ".." || part.is_empty()),
        "noncanonical public path"
    );
    anyhow::ensure!(
        !path.contains("//") && (!prefix || (path.len() > 1 && path.ends_with('/'))),
        "invalid asset prefix"
    );
    Ok(())
}
#[cfg(test)]
#[path = "unit/web_host.rs"]
mod tests;
