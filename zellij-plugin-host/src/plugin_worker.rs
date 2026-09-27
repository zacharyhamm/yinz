use crate::plugin_map::PluginEnv;
use crate::zellij_exports::wasi_write_object;
use wasmi::{Instance, Store};

use prost::Message;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use zellij_plugin_api::message::ProtobufMessage;
use zellij_runtime::errors::prelude::*;

pub struct RunningWorker {
    pub instance: Instance,
    pub name: String,
    pub store: Store<PluginEnv>,
}

impl RunningWorker {
    pub fn new(store: Store<PluginEnv>, instance: Instance, name: &str) -> Self {
        RunningWorker {
            store,
            instance,
            name: name.into(),
        }
    }
    pub fn send_message(&mut self, message: String, payload: String) -> Result<()> {
        let err_context = || "Failed to send message to worker".to_string();
        let protobuf_message = ProtobufMessage {
            name: message,
            payload,
            ..Default::default()
        };
        let protobuf_bytes = protobuf_message.encode_to_vec();
        let work_function = self
            .instance
            .get_typed_func::<(), ()>(&mut self.store, &self.name)
            .with_context(err_context)?;
        wasi_write_object(self.store.data_mut(), &protobuf_bytes).with_context(err_context)?;
        work_function
            .call(&mut self.store, ())
            .with_context(err_context)?;
        Ok(())
    }
}

pub enum MessageToWorker {
    Message(String, String), // message, payload
    Exit,
}

pub fn plugin_worker(mut worker: RunningWorker) -> Result<UnboundedSender<MessageToWorker>> {
    let (sender, mut receiver): (
        UnboundedSender<MessageToWorker>,
        UnboundedReceiver<MessageToWorker>,
    ) = unbounded_channel();
    // Guest callbacks are synchronous and can block. Keep them off the shared I/O
    // runtime, whose four threads also drive timers and command replies.
    std::thread::Builder::new()
        .name(format!("plugin-worker-{}", worker.store.data().plugin_id))
        .spawn(move || loop {
            match receiver.blocking_recv() {
                Some(MessageToWorker::Message(message, payload)) => {
                    if let Err(e) = worker.send_message(message, payload) {
                        Err::<(), _>(e)
                            .context("failed to run plugin worker callback")
                            .non_fatal();
                    }
                },
                Some(MessageToWorker::Exit) => {
                    break;
                },
                None => {
                    break;
                },
            }
        })
        .context("failed to start plugin worker thread")?;
    Ok(sender)
}

#[cfg(test)]
#[path = "unit/plugin_worker_tests.rs"]
mod tests;
