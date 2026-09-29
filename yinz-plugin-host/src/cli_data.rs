//! CLI administration uses the server store, with the same watch delivery as plugins.
use std::collections::BTreeSet;
use yinz_runtime::errors::prelude::*;
use yinz_storage::plugin_data_store::PluginDataStore;
use yinz_types::{cli::DataRequest, plugin_data::Mutation};

pub(crate) fn run(
    store: &PluginDataStore,
    request: DataRequest,
    notify: impl FnOnce(&str, &BTreeSet<String>),
) -> Result<String> {
    let data = match request {
        DataRequest::Catalog { namespace } => serde_json::to_value(
            store.catalog(namespace.as_deref()).context("read data catalog")?,
        ),
        DataRequest::Fetch(mut request) => {
            // Validation of FetchRequest namespaces is plugin-specific. CLI administrators
            // may address private namespaces, but only ones in the server's catalog.
            let namespace = request.namespace.take().ok_or_else(|| anyhow!("an explicit namespace is required"))?;
            require_entity(store, &namespace, &request.entity)?;
            serde_json::to_value(store.fetch(&namespace, &request).context("fetch plugin data")?)
        },
        DataRequest::Count(mut request) => {
            let namespace = request.namespace.take().ok_or_else(|| anyhow!("an explicit namespace is required"))?;
            require_entity(store, &namespace, &request.entity)?;
            serde_json::to_value(store.count(&namespace, &request).context("count plugin data")?)
        },
        DataRequest::Apply { namespace, mutations } => {
            for (index, mutation) in mutations.iter().enumerate() {
                require_entity(store, &namespace, mutation.entity())?;
                match mutation {
                    Mutation::Put { uid: Some(_), expect_rev: None, .. }
                    | Mutation::Update { expect_rev: None, .. } => return Err(anyhow!("replacement writes require expect_rev from the value you read (0 asserts an absent uid)")),
                    Mutation::Delete { entity, filter } if !index.checked_sub(1).and_then(|i| mutations.get(i)).is_some_and(|guard| matches!(guard, Mutation::AssertRows { entity: e, filter: f, .. } if e == entity && f == filter)) => return Err(anyhow!("deletion requires an immediately preceding AssertRows with the same entity and filter in the atomic batch")),
                    _ => {},
                }
            }
            let outcome = store.apply(&namespace, &mutations).context("apply plugin data batch")?;
            notify(&namespace, &outcome.touched_entities);
            serde_json::to_value(outcome.report)
        },
    }.context("encode plugin data result")?;
    serde_json::to_string(&data).context("encode CLI reply")
}

fn require_entity(store: &PluginDataStore, namespace: &str, entity: &str) -> Result<()> {
    if !store
        .catalog(Some(namespace))
        .context("read namespace catalog")?
        .iter()
        .any(|info| info.def.name == entity)
    {
        return Err(anyhow!(
            "no catalogued entity {entity:?} in namespace {namespace:?}; use yinz data catalog"
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/cli_data.rs"]
mod tests;
