//! Native clipboard history in the same shared store used by plugins and the data CLI.
use project_data::store;
use std::sync::Arc;
use yinz_bus::plugins::{ClipboardHistoryEntry, PluginInstruction};
use yinz_bus::thread_bus::ThreadSenders;
use yinz_runtime::channels::{self, SenderWithContext};
use yinz_runtime::errors::prelude::*;
use yinz_storage::plugin_data_store::{ApplyOutcome, PluginDataStore};
use yinz_types::data::CopyDestination;
use yinz_types::plugin_data::Mutation;

/// Keep SQLite's busy timeout off the dispatch thread, preserving submission order.
/// Dropping the sender drains the queue and releases the host's shutdown token.
pub(super) fn start_worker(
    data: Arc<PluginDataStore>,
    senders: ThreadSenders,
    shutdown: tokio::sync::mpsc::Sender<()>,
) -> SenderWithContext<ClipboardHistoryEntry> {
    let (sender, receiver) = channels::unbounded();
    yinz_bus::global_async_runtime::get_tokio_runtime().spawn_blocking(move || {
        let _shutdown = shutdown;
        while let Ok((entry, context)) = receiver.recv() {
            yinz_runtime::errors::update_thread_ctx(&context);
            match record(&data, entry) {
                Ok(outcome) => senders
                    .send_to_plugin(PluginInstruction::RefreshDataWatches {
                        namespace: store::SHARED.into(),
                        entities: outcome.touched_entities,
                    })
                    .context("enqueue clipboard history watch refresh")
                    .non_fatal(),
                Err(error) => Err::<(), _>(error)
                    .context("failed to persist clipboard history")
                    .non_fatal(),
            }
        }
    });
    SenderWithContext::new(sender)
}

pub(super) fn record(data: &PluginDataStore, entry: ClipboardHistoryEntry) -> Result<ApplyOutcome> {
    data.define_entity(
        store::SHARED,
        "yinz:clipboard-history",
        &store::clipboard_history_def(),
        false,
    )
    .context("define clipboard history")?;
    let destination = match entry.destination {
        CopyDestination::Command => "command",
        CopyDestination::Primary => "primary",
        CopyDestination::System => "system",
    };
    // No uid means insert, even when text and timestamps match an earlier submission.
    data.apply(
        store::SHARED,
        &[Mutation::put("clipboard_history")
            .set("content", entry.content.into_bytes())
            .set("copied_at_ms", entry.copied_at_ms)
            .set("host_id", entry.host_id.to_string())
            .set("yinz_session_name", entry.yinz_session_name.to_string())
            .set("destination", destination)
            .build()],
    )
    .context("append clipboard history")
}

#[cfg(test)]
#[path = "unit/clipboard_history.rs"]
mod tests;
