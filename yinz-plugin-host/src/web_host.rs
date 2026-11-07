//! Per-session web state shared by the plugin thread and every plugin instance's env.
//!
//! The plugin thread records which plugin is the configured web plugin; that plugin's
//! `HttpAssets` and `HttpPublish` commands freeze files into the session's [`SiteHandle`],
//! which the HTTP server reads.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use yinz_bus::web::ReplySlot;
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::PluginId;
use yinz_types::web::{Site, SiteHandle, StaticFile};

/// Pending web requests of one plugin instance, keyed by request id.
#[expect(dead_code, reason = "WS-C stores this on PluginEnv")]
pub(crate) type WebPending = HashMap<u64, ReplySlot>;

/// Created once per session in `plugin_thread_main`; cloned into each plugin env.
#[derive(Clone, Default)]
pub(crate) struct WebHost(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    target: Option<PluginId>,
    site: Option<SiteHandle>,
}

impl WebHost {
    pub fn new(site: Option<SiteHandle>) -> Self {
        WebHost(Arc::new(Mutex::new(State { target: None, site })))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set_target(&self, id: Option<PluginId>) {
        self.state().target = id;
    }

    #[expect(dead_code, reason = "WS-C uses this for the web command identity gate")]
    pub fn is_target(&self, id: PluginId) -> bool {
        self.state().target == Some(id)
    }

    /// The site to extend for `generation`: the current one, or a fresh one when another
    /// generation (or nothing) published it.
    fn edit_site(
        &self,
        id: PluginId,
        generation: u64,
        edit: impl FnOnce(&mut Site) -> Result<()>,
    ) -> Result<()> {
        let state = self.state();
        anyhow::ensure!(
            state.target == Some(id),
            "only the configured web plugin can publish the site"
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

    #[expect(dead_code, reason = "WS-C calls this from the HttpAssets handler")]
    pub fn publish_assets(
        &self,
        id: PluginId,
        generation: u64,
        prefix: String,
        files: BTreeMap<String, StaticFile>,
    ) -> Result<()> {
        self.edit_site(id, generation, |site| {
            site.assets.insert(prefix, files);
            Ok(())
        })
    }

    #[expect(dead_code, reason = "WS-C calls this from the HttpPublish handler")]
    pub fn publish_page(
        &self,
        id: PluginId,
        generation: u64,
        path: String,
        file: StaticFile,
    ) -> Result<()> {
        self.edit_site(id, generation, |site| {
            site.pages.insert(path, file);
            Ok(())
        })
    }

    /// Clear the site if `generation` published it.
    #[expect(dead_code, reason = "WS-C calls this when an instance is disposed")]
    pub fn retire(&self, generation: u64) {
        let state = self.state();
        if let Some(handle) = &state.site
            && handle
                .current()
                .is_some_and(|site| site.generation == generation)
        {
            handle.set(None);
        }
    }
}
