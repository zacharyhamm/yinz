//! Definitions and helpers for sending and receiving messages between threads.

use crate::{
    background_jobs::BackgroundJob, os_input_output::ServerOsApi, plugins::PluginInstruction,
    pty::PtyInstruction, pty_writer::PtyWriteInstruction, screen::ScreenInstruction, ClientId,
    ServerInstruction,
};
use std::path::Path;
use yinz_config::config::Config;
use yinz_config::screen_config::ScreenConfig;
use yinz_runtime::channels;
use yinz_runtime::channels::SenderWithContext;
use yinz_runtime::errors::prelude::*;
use yinz_runtime::home::user_plugin_dir_or_default;
use yinz_types::diagnostics::ErrorContext;
use yinz_types::input::command::{RunCommand, TerminalAction};
use yinz_types::input::options::Options;

#[cfg(test)]
#[path = "unit/thread_bus_tests.rs"]
mod tests;

// Generations are process-wide runtime identities. Replies belong to the runtime,
// including while notifications wait on the bus or its pinned executor.
// ponytail: one registry lock per plugin reply; shard if reply traffic contends.
type PluginReply = (ClientId, yinz_types::data::Event);
#[derive(Default)]
struct ReplyMailbox {
    next_id: u64,
    pending: std::collections::BTreeMap<u64, PluginReply>,
    cleanup: Option<std::sync::mpsc::Sender<PluginReply>>,
}
static REPLY_MAILBOXES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<u64, ReplyMailbox>>,
> = std::sync::LazyLock::new(Default::default);

/// An internal notification can claim its reply only on the owning executor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginReplyId {
    generation: u64,
    id: u64,
}

impl PluginReplyId {
    pub fn claim(self) -> Option<PluginReply> {
        let mut mailboxes = REPLY_MAILBOXES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mailbox = mailboxes.get_mut(&self.generation)?;
        if mailbox.cleanup.is_some() {
            return None;
        }
        mailbox.pending.remove(&self.id)
    }
}

pub struct PluginReplies {
    generation: u64,
}

impl PluginReplies {
    pub fn new(generation: u64) -> Self {
        REPLY_MAILBOXES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(generation, ReplyMailbox::default());
        Self { generation }
    }

    pub fn begin_cleanup(&self) -> std::sync::mpsc::Receiver<PluginReply> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut mailboxes = REPLY_MAILBOXES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(mailbox) = mailboxes.get_mut(&self.generation) {
            for (_, reply) in std::mem::take(&mut mailbox.pending) {
                let _ = sender.send(reply);
            }
            mailbox.cleanup = Some(sender);
        }
        receiver
    }
}

impl Drop for PluginReplies {
    fn drop(&mut self) {
        REPLY_MAILBOXES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.generation);
    }
}

/// A container for senders to the different threads in yinz on the server side
#[derive(Default, Clone)]
pub struct ThreadSenders {
    pub to_screen: Option<SenderWithContext<ScreenInstruction>>,
    pub to_pty: Option<SenderWithContext<PtyInstruction>>,
    pub to_plugin: Option<SenderWithContext<PluginInstruction>>,
    pub to_server: Option<SenderWithContext<ServerInstruction>>,
    pub to_pty_writer: Option<SenderWithContext<PtyWriteInstruction>>,
    pub to_background_jobs: Option<SenderWithContext<BackgroundJob>>,
    pub session_ownership: Option<std::sync::Arc<yinz_ipc::session_ownership::SessionOwnership>>,
    pub session_checkpoint: std::sync::Arc<crate::background_jobs::SessionCheckpointWriter>,
    // this is a convenience for the unit tests
    // it's not advisable to set it to true in production code
    pub should_silently_fail: bool,
}

impl ThreadSenders {
    pub fn send_to_screen(&self, instruction: ScreenInstruction) -> Result<()> {
        if self.should_silently_fail {
            let _ = self
                .to_screen
                .as_ref()
                .map(|sender| sender.send(instruction))
                .unwrap_or_else(|| Ok(()));
            Ok(())
        } else {
            let sender = self
                .to_screen
                .as_ref()
                .context("failed to get screen sender")?;
            match instruction {
                // Layout handoffs need failure cleanup; save requests must not acknowledge
                // success when the screen never received them.
                ScreenInstruction::ApplyLayout(..)
                | ScreenInstruction::OverrideLayoutComplete(..)
                | ScreenInstruction::SaveSession(..) => sender.send_required(instruction),
                _ => sender.send(instruction),
            }
            .to_anyhow()
            .context("failed to send message to screen")
        }
    }

    pub fn send_to_pty(&self, instruction: PtyInstruction) -> Result<()> {
        if self.should_silently_fail {
            let _ = self
                .to_pty
                .as_ref()
                .map(|sender| sender.send(instruction))
                .unwrap_or_else(|| Ok(()));
            Ok(())
        } else {
            let sender = self.to_pty.as_ref().context("failed to get pty sender")?;
            match instruction {
                // A failed handoff must let the spawn worker tear down its terminals.
                PtyInstruction::SpawnResolved(_) => sender.send_required(instruction),
                _ => sender.send(instruction),
            }
            .to_anyhow()
            .context("failed to send message to pty")
        }
    }

    pub fn send_to_plugin(&self, instruction: PluginInstruction) -> Result<()> {
        let instruction = if let PluginInstruction::PluginEvent(plugin, client, generation, event) =
            instruction
        {
            let mut mailboxes = REPLY_MAILBOXES
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(mailbox) = mailboxes.get_mut(&generation) {
                if let Some(sender) = &mailbox.cleanup {
                    let _ = sender.send((client, event));
                    return Ok(());
                }
                let id = mailbox.next_id;
                mailbox.next_id = id.checked_add(1).context("plugin reply IDs exhausted")?;
                mailbox.pending.insert(id, (client, event.clone()));
                // Enqueue under the routing lock to preserve producer order with cleanup.
                return self.send_plugin_notification(PluginInstruction::PluginReply(
                    plugin,
                    client,
                    generation,
                    event,
                    PluginReplyId { generation, id },
                ));
            }
            PluginInstruction::PluginEvent(plugin, client, generation, event)
        } else {
            instruction
        };
        self.send_plugin_notification(instruction)
    }

    fn send_plugin_notification(&self, instruction: PluginInstruction) -> Result<()> {
        if self.should_silently_fail {
            let _ = self
                .to_plugin
                .as_ref()
                .map(|sender| sender.send(instruction))
                .unwrap_or_else(|| Ok(()));
            Ok(())
        } else {
            self.to_plugin
                .as_ref()
                .context("failed to get plugin sender")?
                .send(instruction)
                .to_anyhow()
                .context("failed to send message to plugin")
        }
    }

    pub fn send_to_server(&self, instruction: ServerInstruction) -> Result<()> {
        if self.should_silently_fail {
            let _ = self
                .to_server
                .as_ref()
                .map(|sender| sender.send(instruction))
                .unwrap_or_else(|| Ok(()));
            Ok(())
        } else {
            self.to_server
                .as_ref()
                .context("failed to get server sender")?
                .send(instruction)
                .to_anyhow()
                .context("failed to send message to server")
        }
    }
    pub fn send_to_pty_writer(&self, instruction: PtyWriteInstruction) -> Result<()> {
        if self.should_silently_fail {
            let _ = self
                .to_pty_writer
                .as_ref()
                .map(|sender| sender.send(instruction))
                .unwrap_or_else(|| Ok(()));
            Ok(())
        } else {
            self.to_pty_writer
                .as_ref()
                .context("failed to get pty writer sender")?
                .send(instruction)
                .to_anyhow()
                .context("failed to send message to pty writer")
        }
    }
    pub fn send_to_background_jobs(&self, background_job: BackgroundJob) -> Result<()> {
        if self.should_silently_fail {
            let _ = self
                .to_background_jobs
                .as_ref()
                .map(|sender| sender.send(background_job))
                .unwrap_or_else(|| Ok(()));
            Ok(())
        } else {
            self.to_background_jobs
                .as_ref()
                .context("failed to get background jobs sender")?
                .send(background_job)
                .to_anyhow()
                .context("failed to send message to background jobs")
        }
    }

    /// Hand each client's new configuration to every thread that holds a part of it: the screen,
    /// the plugins and the pty, in that order per client. When the configuration was written to
    /// disk, the plugins are then asked to look for changes to their own configuration.
    ///
    /// Each config must already be the client's effective one, plugin default keybinds
    /// included: the threads adopt it as is. `config_dir` is the session's `--config-dir`, from
    /// which a config that does not name `user_plugin_dir` has it re-derived.
    pub fn reconfigure_threads(
        &self,
        config_changes: Vec<(ClientId, Config)>,
        config_dir: Option<&Path>,
        was_written_to_disk: bool,
    ) {
        let background = config_changes
            .first()
            .map(|(client, config)| (*client, config.background_plugins.clone()));
        let mut new_plugin_config = None;
        for (client_id, new_config) in config_changes {
            new_plugin_config.get_or_insert_with(|| new_config.plugins.clone());
            let default_shell = default_shell_action(&new_config.options);
            self.send_to_screen(ScreenInstruction::Reconfigure {
                client_id,
                keybinds: new_config.keybinds.clone(),
                config: ScreenConfig::from_config(&new_config),
            })
            .context("reconfiguring the screen")
            .non_fatal();
            self.send_to_plugin(PluginInstruction::Reconfigure {
                plugin_aliases: new_config.plugins.clone(),
                client_id,
                keybinds: Some(new_config.keybinds),
                default_mode: new_config.options.default_mode,
                default_shell,
                layout_dir: new_config.options.layout_dir,
                // Re-derived rather than passed through: `PluginBridge::reconfigure` assigns
                // this verbatim, so a config that does not name the option explicitly would
                // otherwise wipe the folder derived at session start. The server's cwd has
                // not moved since then, so a relative folder resolves to the same place.
                user_plugin_dir: user_plugin_dir_or_default(
                    new_config.options.user_plugin_dir,
                    config_dir.map(Path::to_path_buf),
                ),
                hooks: new_config.hooks,
                context_menu: new_config.context_menu,
                was_written_to_disk,
            })
            .context("reconfiguring the plugins")
            .non_fatal();
            self.send_to_pty(PtyInstruction::Reconfigure {
                client_id,
                default_editor: new_config.options.scrollback_editor,
                post_command_discovery_hook: new_config.options.post_command_discovery_hook,
            })
            .context("reconfiguring the pty")
            .non_fatal();
        }

        if let Some((client_id, plugins)) = background {
            self.send_to_plugin(PluginInstruction::ReconcileBackgroundPlugins {
                plugins,
                client_id,
            })
            .context("reconcile background plugins")
            .non_fatal();
        }
        if was_written_to_disk {
            if let Some(new_plugins) = new_plugin_config {
                self.send_to_plugin(PluginInstruction::DetectPluginConfigChanges(new_plugins))
                    .context("reporting plugin config changes")
                    .non_fatal();
            }
        }
    }

    /// Tell every worker thread to exit, the screen first: it can still need every other worker
    /// while it finishes its queued instructions, so they are told only once `wait_for_screen`
    /// returns. A caller that owns the screen thread joins it there. Background jobs stay
    /// alive until `wait_for_plugins` returns so plugin unload hooks can await commands.
    ///
    /// The sends are allowed to fail, because a thread that has already exited has nothing left
    /// to be told. A new worker channel gets its `Exit` here, so every shutdown path sends it.
    pub fn exit_threads(&self, wait_for_screen: impl FnOnce(), wait_for_plugins: impl FnOnce()) {
        let _ = self.send_to_screen(ScreenInstruction::Exit);
        wait_for_screen();
        let _ = self.send_to_pty(PtyInstruction::Exit);
        let _ = self.send_to_plugin(PluginInstruction::Exit);
        let _ = self.send_to_pty_writer(PtyWriteInstruction::Exit);
        wait_for_plugins();
        let _ = self.send_to_background_jobs(BackgroundJob::Exit);
    }

    #[allow(unused)]
    pub fn silently_fail_on_send(mut self) -> Self {
        // this is mostly used for the tests, see struct
        self.should_silently_fail = true;
        self
    }
    #[allow(unused)]
    pub fn replace_to_pty_writer(
        &mut self,
        new_pty_writer: SenderWithContext<PtyWriteInstruction>,
    ) {
        // this is mostly used for the tests, see struct
        self.to_pty_writer.replace(new_pty_writer);
    }
    #[allow(unused)]
    pub fn replace_to_pty(&mut self, new_pty: SenderWithContext<PtyInstruction>) {
        // this is mostly used for the tests, see struct
        self.to_pty.replace(new_pty);
    }

    #[allow(unused)]
    pub fn replace_to_plugin(&mut self, new_to_plugin: SenderWithContext<PluginInstruction>) {
        // this is mostly used for the tests, see struct
        self.to_plugin.replace(new_to_plugin);
    }
}

/// What a new pane runs when `options` names a default shell: that shell, started in the
/// default cwd and titled by what runs in it.
pub fn default_shell_action(options: &Options) -> Option<TerminalAction> {
    options.default_shell.as_ref().map(|shell| {
        TerminalAction::RunCommand(RunCommand {
            command: shell.clone(),
            cwd: options.default_cwd.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    })
}

/// A container for a receiver, OS input and the senders to a given thread
#[derive(Default)]
pub struct Bus<T> {
    receivers: Vec<channels::Receiver<(T, ErrorContext)>>,
    pub senders: ThreadSenders,
    pub os_input: Option<Box<dyn ServerOsApi>>,
}

impl<T> Bus<T> {
    pub fn new(
        receivers: Vec<channels::Receiver<(T, ErrorContext)>>,
        to_screen: Option<&SenderWithContext<ScreenInstruction>>,
        to_pty: Option<&SenderWithContext<PtyInstruction>>,
        to_plugin: Option<&SenderWithContext<PluginInstruction>>,
        to_server: Option<&SenderWithContext<ServerInstruction>>,
        to_pty_writer: Option<&SenderWithContext<PtyWriteInstruction>>,
        to_background_jobs: Option<&SenderWithContext<BackgroundJob>>,
        os_input: Option<Box<dyn ServerOsApi>>,
    ) -> Self {
        Bus {
            receivers,
            senders: ThreadSenders {
                to_screen: to_screen.cloned(),
                to_pty: to_pty.cloned(),
                to_plugin: to_plugin.cloned(),
                to_server: to_server.cloned(),
                to_pty_writer: to_pty_writer.cloned(),
                to_background_jobs: to_background_jobs.cloned(),
                session_checkpoint: Default::default(),
                session_ownership: None,
                should_silently_fail: false,
            },
            os_input: os_input.clone(),
        }
    }
    #[allow(unused)]
    pub fn should_silently_fail(mut self) -> Self {
        // this is mostly used for the tests
        self.senders.should_silently_fail = true;
        self
    }
    #[allow(unused)]
    pub fn empty() -> Self {
        // this is mostly used for the tests
        Bus {
            receivers: vec![],
            senders: ThreadSenders {
                to_screen: None,
                to_pty: None,
                to_plugin: None,
                to_server: None,
                to_pty_writer: None,
                to_background_jobs: None,
                session_checkpoint: Default::default(),
                session_ownership: None,
                should_silently_fail: true,
            },
            os_input: None,
        }
    }

    pub fn recv(&self) -> Result<(T, ErrorContext), channels::RecvError> {
        let mut selector = channels::Select::new();
        self.receivers.iter().for_each(|r| {
            selector.recv(r);
        });
        let oper = selector.select();
        let idx = oper.index();
        oper.recv(&self.receivers[idx])
    }

    /// A message that is already waiting, without blocking for one. Lets a thread drain what
    /// has queued up behind the message it just took.
    pub fn try_recv(&self) -> Option<(T, ErrorContext)> {
        self.receivers.iter().find_map(|r| r.try_recv().ok())
    }

    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<(T, ErrorContext), channels::RecvTimeoutError> {
        let mut selector = channels::Select::new();
        self.receivers.iter().for_each(|r| {
            selector.recv(r);
        });
        match selector.select_timeout(timeout) {
            Ok(oper) => {
                let idx = oper.index();
                oper.recv(&self.receivers[idx])
                    .map_err(|_| channels::RecvTimeoutError::Disconnected)
            },
            Err(_) => Err(channels::RecvTimeoutError::Timeout),
        }
    }
}
