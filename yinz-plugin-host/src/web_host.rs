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
    loaded: bool,
}
#[derive(Default, Debug)]
struct State {
    target: Option<PluginId>,
    configured: Option<yinz_types::input::layout::RunPlugin>,
    startup: Option<super::web_startup::Startup>,
    permission_generation: Option<u64>,
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
    pub fn configure(&self, params: &yinz_bus::web::WebPluginParams) {
        let mut state = self.state();
        state.configured = params.plugin.get_run_plugin();
        state.startup = Some(super::web_startup::Startup::new(params.application.clone()));
    }
    pub fn configured(&self) -> Option<yinz_types::input::layout::RunPlugin> {
        self.state().configured.clone()
    }
    pub fn target(&self) -> Option<PluginId> {
        self.state().target
    }
    pub fn reserve(&self, id: PluginId) {
        let mut state = self.state();
        if state.target.is_none() && !state.startup.as_ref().is_some_and(|s| s.stopped()) {
            state.target = Some(id);
            if let Some(startup) = &mut state.startup {
                startup.reserve(id);
            }
        }
    }
    pub fn begin_load(&self, id: PluginId, load: u64) {
        let mut state = self.state();
        if state.target == Some(id)
            && let Some(startup) = &mut state.startup
        {
            startup.begin(id, load, std::time::Instant::now());
        }
    }
    pub fn loaded(&self, id: PluginId, generation: u64) {
        let mut state = self.state();
        if state.target == Some(id)
            && let Some(instance) = state.instances.get_mut(&generation)
        {
            instance.loaded = true;
        }
        Self::refresh_in(&mut state, std::time::Instant::now());
    }
    fn refresh_in(state: &mut State, now: std::time::Instant) {
        let generation = state.target.and_then(|id| Self::selected_in(state, id));
        // A selected parent can disappear on an executor before its cancellation message
        // arrives. Its permission clock must never remain attached to the next parent.
        if state.permission_generation.is_some() && state.permission_generation != generation {
            state.permission_generation = None;
            if let Some(startup) = &mut state.startup {
                startup.permission(false, now);
            }
        }
        let published = generation.is_some_and(|generation| {
            state.instances.get(&generation).is_some_and(|i| i.loaded)
                && state
                    .site
                    .as_ref()
                    .and_then(SiteHandle::current)
                    .is_some_and(|site| {
                        site.generation == generation
                            && (!site.pages.is_empty()
                                || site.assets.values().any(|files| !files.is_empty()))
                    })
        });
        if let Some(startup) = &mut state.startup {
            startup.refresh(generation, published, now);
        }
    }
    pub fn pulse(&self, now: std::time::Instant) -> super::web_startup::Action {
        let mut state = self.state();
        Self::refresh_in(&mut state, now);
        state
            .startup
            .as_mut()
            .map_or(super::web_startup::Action::None, |s| s.pulse(now))
    }
    pub fn failed(
        &self,
        id: PluginId,
        load: Option<u64>,
        generation: Option<u64>,
        failure: yinz_types::web::ApplicationFailure,
    ) {
        let mut state = self.state();
        if state.target == Some(id)
            && let Some(startup) = &mut state.startup
        {
            startup.fail(load, generation, failure, std::time::Instant::now());
        }
    }
    pub fn start_failed(&self, failure: yinz_types::web::ApplicationFailure) {
        let mut state = self.state();
        if let Some(startup) = &mut state.startup {
            startup.start_failed(failure, std::time::Instant::now());
        }
    }
    pub fn paused(&self, paused: bool) {
        if let Some(startup) = &mut self.state().startup {
            startup.pause(paused, std::time::Instant::now());
        }
    }
    /// Permission recipients identify their parent even for worker requests. Validate both
    /// incarnation and load here, atomically with publication/retirement, never by plugin ID alone.
    pub fn permissions_pending(
        &self,
        id: PluginId,
        load: u64,
        generations: &std::collections::HashSet<u64>,
    ) {
        let mut state = self.state();
        if state.target != Some(id)
            || !state
                .startup
                .as_ref()
                .is_some_and(|s| s.status().load == Some(load) && !s.stopped())
        {
            return;
        }
        let selected = Self::selected_in(&state, id);
        let waiting = selected.filter(|generation| generations.contains(generation));
        state.permission_generation = waiting;
        if let Some(startup) = &mut state.startup {
            startup.permission(waiting.is_some(), std::time::Instant::now());
        }
    }
    pub fn permission_denied(&self, id: PluginId, load: u64, generation: u64) {
        let mut state = self.state();
        if state.target != Some(id) || Self::selected_in(&state, id) != Some(generation) {
            return;
        }
        if let Some(startup) = &mut state.startup {
            startup.fail(
                Some(load),
                Some(generation),
                yinz_types::web::ApplicationFailure::PermissionDenied,
                std::time::Instant::now(),
            );
        }
    }
    pub fn reset(&self, id: PluginId) {
        let mut state = self.state();
        if state.target == Some(id) {
            state.permission_generation = None;
        }
        if state.target == Some(id)
            && let Some(startup) = &mut state.startup
        {
            startup.reset();
        }
    }
    pub fn stop(&self, id: PluginId) {
        let mut state = self.state();
        if state.target == Some(id) {
            state.permission_generation = None;
        }
        if state.target == Some(id)
            && let Some(startup) = &mut state.startup
        {
            startup.stop();
        }
    }
    pub fn status(&self) -> Option<yinz_types::web::ApplicationStatus> {
        self.state().startup.as_ref().map(|s| s.status())
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
                loaded: false,
            },
        );
        Self::refresh_in(&mut self.state(), std::time::Instant::now());
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
        let mut state = self.state();
        use yinz_types::web::{PublicationRejection, PublicationRejectionReason as Reason};
        let selected = state
            .target
            .and_then(|target| Self::selected_in(&state, target));
        let reason = if state.target.is_none() {
            Reason::UnsetTarget
        } else if state.target != Some(id) {
            Reason::DifferentTarget
        } else {
            Reason::UnavailableOrNonselectedGeneration
        };
        if state.target != Some(id) || selected != Some(generation) {
            let rejection = PublicationRejection {
                reason,
                target: state.target,
                attempted_plugin: id,
                attempted_generation: generation,
                selected_generation: selected,
            };
            // Other plugins' rejected commands cannot alter configured-app readiness/failure.
            if state.target == Some(id)
                && let Some(startup) = &mut state.startup
            {
                startup.publication_rejected(rejection.clone());
            }
            bail!("only the live configured web plugin can publish the site: {rejection:?}; load={:?}", state.startup.as_ref().and_then(|s| s.status().load));
        }
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
        Self::refresh_in(&mut state, std::time::Instant::now());
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
        Self::refresh_in(&mut state, std::time::Instant::now());
    }
    pub fn retire(&self, generation: u64) {
        let mut state = self.state();
        Self::retire_in(&mut state, generation);
        Self::refresh_in(&mut state, std::time::Instant::now());
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
