//! Plugin namespaces, guarded data mutations, and watch delivery.

use crate::plugin_map::PluginEnv;
use crate::response::{value, DecodedResponse};
use crate::PluginInstruction;
use std::collections::BTreeSet;
use yinz_bus::screen::ScreenInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_storage::plugin_data_store;
use yinz_types::data::{
    validate_namespace, DataEntityInfo, DataError, EntityDef, Event, EventType, FetchRequest,
    Mutation, PermissionType, WatchMode, SHARED_NAMESPACE_PREFIX,
};
use yinz_types::internal_ids::DataWatchId;

// ------------------------------------------------------------------ data store
//
// These are all "purely local work" handlers: they run on the calling plugin's pinned executor
// thread, do their work against the shared SQLite store, and write the response inline. No other
// yinz thread is involved, and nothing is delivered as an event — except the watch notifications,
// which are queued through the plugin thread rather than dispatched here (see `notify_watchers`).

/// Work out which namespace a command means, and whether this plugin may use it.
///
/// `None` is the plugin's own private namespace, which it cannot name and therefore cannot mistake
/// for anyone else's. Anything else must be `shared:`-prefixed and needs `SharedPluginData`.
///
/// The shared check lives here rather than in `check_command_permission` because that sees only the
/// command, not its arguments — the same reason event permissions are checked separately.
fn resolve_data_namespace(env: &PluginEnv, namespace: Option<String>) -> Result<String, DataError> {
    let Some(namespace) = namespace else {
        return Ok(plugin_data_store::private_namespace(
            env.data_owner.as_ref(),
        ));
    };
    validate_namespace(Some(&namespace))?;
    if !may_reach_shared_namespaces(env) {
        return Err(DataError::PermissionDenied(format!(
            "reading or writing the shared namespace `{}` needs the SharedPluginData permission",
            namespace
        )));
    }
    Ok(namespace)
}

/// Whether this plugin may reach `shared:` namespaces at all.
///
/// Split out of [`resolve_data_namespace`] because the catalog asks the same question without
/// having a namespace to resolve: it is deciding what to *list*, not what to open.
fn may_reach_shared_namespaces(env: &PluginEnv) -> bool {
    env.plugin_config.is_builtin()
        || env
            .permissions
            .lock()
            .unwrap()
            .as_ref()
            .map(|permissions| permissions.contains(&PermissionType::SharedPluginData))
            .unwrap_or(false)
}

/// The namespace a fetch means, resolved from the request it is carried in.
fn resolve_fetch_namespace(env: &PluginEnv, request: &FetchRequest) -> Result<String, DataError> {
    resolve_data_namespace(env, request.namespace.clone())
}

pub(super) fn data_define_entity(
    env: &mut PluginEnv,
    namespace: Option<String>,
    def: EntityDef,
    allow_destructive: bool,
) -> Result<DecodedResponse> {
    let plugin_url = env.data_owner.to_string();
    match resolve_data_namespace(env, namespace).and_then(|namespace| {
        env.data_access.plugin_data().define_entity(
            &namespace,
            &plugin_url,
            &def,
            allow_destructive,
        )
    }) {
        Ok(info) => value(info),
        Err(e) => Err(anyhow!("{e}")),
    }
}

pub(super) fn data_apply(
    env: &mut PluginEnv,
    namespace: Option<String>,
    mutations: Vec<Mutation>,
) -> Result<DecodedResponse> {
    match resolve_data_namespace(env, namespace).and_then(|namespace| {
        Ok((
            namespace.clone(),
            env.data_access
                .plugin_data()
                .apply(&namespace, &mutations)?,
        ))
    }) {
        Ok((namespace, outcome)) => {
            notify_watchers(env, &namespace, &outcome.touched_entities);
            value(outcome.report)
        },
        // A conflict is its own answer, not an error: the caller asked for a compare-and-set and
        // is being told the comparison failed, which is a race to retry rather than a request to
        // fix.
        Err(DataError::Conflict(message)) => Err(anyhow!(
            "{}{message}",
            crate::response::DATA_CONFLICT_MARKER
        )),
        Err(e) => Err(anyhow!("{e}")),
    }
}

pub(super) fn data_fetch(env: &mut PluginEnv, request: FetchRequest) -> Result<DecodedResponse> {
    match resolve_fetch_namespace(env, &request)
        .and_then(|namespace| env.data_access.plugin_data().fetch(&namespace, &request))
    {
        Ok(rows) => crate::binary_data::rows(&rows).map(DecodedResponse::Json),
        Err(e) => Err(anyhow!("{e}")),
    }
}

pub(super) fn data_count(env: &mut PluginEnv, request: FetchRequest) -> Result<DecodedResponse> {
    match resolve_fetch_namespace(env, &request)
        .and_then(|namespace| env.data_access.plugin_data().count(&namespace, &request))
    {
        Ok(count) => value(count),
        Err(e) => Err(anyhow!("{e}")),
    }
}

pub(super) fn data_catalog(
    env: &mut PluginEnv,
    namespace: Option<String>,
) -> Result<DecodedResponse> {
    match catalog_entities(env, namespace) {
        Ok(entities) => crate::binary_data::catalog(entities).map(DecodedResponse::Json),
        Err(e) => Err(anyhow!("{e}")),
    }
}

/// The entities this plugin may see.
///
/// A named namespace goes through [`resolve_data_namespace`], so listing a `shared:` one costs
/// exactly the permission fetching from it would.
///
/// An unnamed one is the discovery case, and is the one place `None` does not mean "my own
/// namespace": that would answer only with what the caller declared itself, which it already
/// knows. It is the plugin's own namespace *plus* every `shared:` one, once it holds
/// `SharedPluginData`.
///
/// Another plugin's private namespace is never listed, whatever the permissions. `SharedPluginData`
/// is a claim on `shared:`, and this is the only call that could otherwise enumerate namespaces
/// nobody chose to publish.
fn catalog_entities(
    env: &PluginEnv,
    namespace: Option<String>,
) -> Result<Vec<DataEntityInfo>, DataError> {
    if namespace.is_some() {
        let namespace = resolve_data_namespace(env, namespace)?;
        return env.data_access.plugin_data().catalog(Some(&namespace));
    }
    let own = resolve_data_namespace(env, None)?;
    let shared_visible = may_reach_shared_namespaces(env);
    Ok(env
        .data_access
        .plugin_data()
        .catalog(None)?
        .into_iter()
        .filter(|entity| {
            entity.namespace == own
                || (shared_visible && entity.namespace.starts_with(SHARED_NAMESPACE_PREFIX))
        })
        .collect())
}

pub(super) fn data_watch(
    env: &mut PluginEnv,
    request: FetchRequest,
    mode: WatchMode,
) -> Result<DecodedResponse> {
    match resolve_fetch_namespace(env, &request).and_then(|namespace| {
        let mut generations = env.watch_generations.lock().unwrap();
        let handle = env.data_access.plugin_data().register_watch(
            &namespace,
            env.plugin_id,
            env.client_id,
            request.clone(),
            mode,
        )?;
        if let Some(generation) = env.generation {
            generations.insert(handle.watch_id, generation);
        }
        Ok(handle)
    }) {
        Ok(handle) => {
            // Event delivery is gated on the plugin's subscriptions, so a plugin that watched but
            // forgot to subscribe would silently never hear anything. Subscribing it here makes
            // `data_watch` mean what it says.
            env.subscriptions
                .lock()
                .unwrap()
                .insert(EventType::PluginDataChanged);
            Ok(DecodedResponse::Json(serde_json::json!({
                "watch_id": handle.watch_id,
                "rows": crate::binary_data::rows(&handle.rows)?,
            })))
        },
        Err(e) => Err(anyhow!("{e}")),
    }
}

pub(super) fn data_unwatch(env: &mut PluginEnv, watch_id: DataWatchId) -> Result<DecodedResponse> {
    let mut generations = env.watch_generations.lock().unwrap();
    match env
        .data_access
        .plugin_data()
        .unregister_watch(env.plugin_id, watch_id)
    {
        Ok(()) => {
            generations.remove(&watch_id);
            value(true)
        },
        Err(e) => Err(anyhow!("{e}")),
    }
}

/// Re-run every watch the write could have affected, and queue an event for each whose results
/// actually changed.
///
/// The events are *queued* through the plugin thread rather than dispatched here. This handler runs
/// on a pinned executor thread that holds the calling plugin's `running_plugin` lock, so calling
/// back into the plugin directly would deadlock whenever a plugin watches something it also writes
/// — which is the normal case, not an exotic one.
fn notify_watchers(env: &PluginEnv, namespace: &str, touched_entities: &BTreeSet<String>) {
    notify_data_watchers(
        env.data_access.plugin_data(),
        &env.watch_generations,
        &env.senders,
        namespace,
        touched_entities,
    );
}

pub(crate) fn notify_data_watchers(
    store: &yinz_storage::plugin_data_store::PluginDataStore,
    watch_generations: &crate::plugin_map::WatchGenerations,
    senders: &yinz_bus::thread_bus::ThreadSenders,
    namespace: &str,
    touched_entities: &BTreeSet<String>,
) {
    if namespace == project_data::store::SHARED
        && (touched_entities.contains("project") || touched_entities.contains("agent_assignment"))
    {
        senders
            .send_to_screen(ScreenInstruction::InvalidateAgentEventProjects)
            .non_fatal();
    }
    // Refresh and recipient lookup must be atomic with registration/unwatch/disposal.
    let generations = watch_generations.lock().unwrap();
    let changed = match store.refresh_watches(namespace, touched_entities) {
        Ok(changed) => changed,
        Err(e) => {
            log::error!("failed to refresh data store watches: {}", e);
            return;
        },
    };
    deliver_data_watch_changes(changed, &generations, senders);
}

pub(crate) fn poll_data_watchers(
    store: &yinz_storage::plugin_data_store::PluginDataStore,
    watch_generations: &crate::plugin_map::WatchGenerations,
    senders: &yinz_bus::thread_bus::ThreadSenders,
) {
    let generations = watch_generations.lock().unwrap();
    match store.poll_external_watches() {
        Ok(changed) => deliver_data_watch_changes(changed, &generations, senders),
        Err(e) => log::error!("failed to poll data store watches: {e}"),
    }
}

fn deliver_data_watch_changes(
    changed: Vec<yinz_storage::plugin_data_store::WatchChange>,
    generations: &std::collections::HashMap<DataWatchId, u64>,
    senders: &yinz_bus::thread_bus::ThreadSenders,
) {
    for change in changed {
        let event = Event::PluginDataChanged {
            watch_id: change.watch_id,
            truncated: change.rows.truncated,
            rows: change.rows,
        };
        senders
            .send_to_plugin(PluginInstruction::for_plugin_event(
                change.plugin_id,
                change.client_id,
                generations.get(&change.watch_id).copied(),
                event,
            ))
            .with_context(|| "failed to deliver a data store change".to_string())
            .non_fatal();
    }
}
