//! The production plugin-loop AgentView arm, kept together for quiescence regression tests.
use crate::{update_quiescence::NativeTasks, PluginInstruction};
use std::sync::Arc;
use yinz_bus::{thread_bus::ThreadSenders, web::AgentViewCall};
use yinz_runtime::errors::prelude::*;
use yinz_storage::data_access::DataAccess;
use yinz_types::data::Event;

fn deliver(senders: &ThreadSenders, call: &AgentViewCall, reply: serde_json::Value) {
    senders
        .send_to_plugin(PluginInstruction::for_plugin_event(
            call.plugin_id,
            call.client,
            call.generation,
            Event::CustomMessage(
                "agent-view-response".into(),
                serde_json::json!({"id":call.id,"reply":reply}).to_string(),
            ),
        ))
        .context("deliver agent view response")
        .non_fatal();
}
pub(super) fn dispatch(
    call: AgentViewCall,
    paused: bool,
    tasks: &NativeTasks,
    senders: ThreadSenders,
    data: Arc<DataAccess>,
    shutdown: tokio::sync::mpsc::Sender<()>,
) -> Option<tokio::task::JoinHandle<()>> {
    if paused {
        deliver(
            &senders,
            &call,
            serde_json::json!({"error":{"code":"paused",
            "message":"session update has paused agent actions; retry after reconnect"}}),
        );
        return None;
    }
    // Register before spawning, so Prepare cannot slip between dispatch and worker startup.
    let native_task = tasks.start();
    Some(
        yinz_bus::global_async_runtime::get_tokio_runtime().spawn_blocking(move || {
            let _shutdown = shutdown;
            let _native_task = native_task;
            let reply = crate::yinz_exports::web_agents::handle(
                &call.request,
                call.client,
                &senders,
                &data,
            );
            deliver(&senders, &call, reply);
        }),
    )
}
