use super::*;
use crate::background_worker::Message;
use std::collections::VecDeque;
use std::sync::mpsc::Receiver;

impl SteelPlugin {
    pub(crate) fn worker_names(&mut self) -> Result<Vec<String>> {
        let names = self.with_runtime(false, |runtime| {
            serde_json::from_value::<Vec<String>>(runtime.invoke("yinz-worker-names", vec![])?)
                .context("reading Steel worker names")
        })?;
        if names.len() > 16 || names.iter().any(|name| name.is_empty() || name.len() > 128) {
            bail!("a Steel plugin supports up to 16 workers with names of 1–128 bytes");
        }
        Ok(names)
    }
}

pub(crate) fn run(
    name: &str,
    source: &str,
    path: &Path,
    env: PluginEnv,
    cancel: Arc<AtomicBool>,
    generation: u64,
    receiver: Receiver<Message>,
) -> Result<()> {
    let mut plugin = SteelPlugin::with_generation(source, path, env, cancel, generation)?;
    plugin.with_runtime(false, |runtime| {
        runtime.invoke("yinz-worker-bind", vec![Value::String(name.to_owned())])?;
        Ok(())
    })?;
    let mut jobs = VecDeque::new();
    let mut active_reservation = None;
    while let Ok(message) = receiver.recv() {
        if plugin.is_disposed() || plugin.env().cancel.load(Ordering::Acquire) {
            break;
        }
        match message {
            Message::Job(client, name, payload, reservation) => {
                jobs.push_back((client, name, payload, reservation));
            },
            Message::Event(client, event, reply) => {
                if reply.is_some_and(|reply| reply.claim().is_none()) {
                    continue;
                }
                if crate::plugin_bridge::check_event_permission(plugin.env(), &event).0
                    == PermissionStatus::Granted
                {
                    plugin.env_mut().client_id = client;
                    if let Event::HostFolderChanged(path) = &event {
                        plugin.env_mut().set_host_folder(path.clone());
                    }
                    let event = crate::event::event_to_json(&event)?;
                    plugin.with_runtime(true, |runtime| {
                        runtime.invoke("yinz-worker-event", vec![event])?;
                        Ok(())
                    })?;
                }
            },
            Message::Exit => break,
            Message::Tick => {
                plugin.tick()?;
            },
            Message::Permission(completion) => {
                plugin.permission_completion(completion)?;
            },
        }
        loop {
            let ready = plugin.with_runtime(false, |runtime| {
                Ok(runtime.invoke("yinz-worker-ready", vec![])?.as_bool() == Some(true))
            })?;
            if !ready {
                break;
            }
            active_reservation.take();
            let Some((client, name, payload, reservation)) = jobs.pop_front() else {
                break;
            };
            active_reservation = Some(reservation);
            plugin.env_mut().client_id = client;
            plugin.with_runtime(true, |runtime| {
                runtime.invoke(
                    "yinz-worker-message",
                    vec![Value::String(name), Value::String(payload)],
                )?;
                Ok(())
            })?;
        }
    }
    Ok(())
}
