//! The standard locked-mode unlock key can also quote Ctrl+g.
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use uuid::Uuid;
use yinz_runtime::{channels::SenderWithContext, errors::prelude::*};
use yinz_types::keys::{BareKey, KeyModifier, KeyWithModifier};
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_types::data::Event, yinz_types::data::InputMode,
    yinz_types::input::actions::Action,
};

use crate::plugins::PluginInstruction;
use crate::screen::ScreenInstruction;
use crate::{
    global_async_runtime::get_tokio_runtime, ClientId, ServerInstruction, SessionMetaData,
};

/// The window used when `locked_escape_timeout_ms` is not set in the config. 0 there disables
/// the prefix entirely, so a locked-mode Ctrl+g unlocks immediately.
pub(crate) const DEFAULT_LOCKED_ESCAPE_TIMEOUT_MS: u64 = 250;

#[derive(Clone, Copy, Debug)]
pub(crate) struct LockedEscape {
    pub token: Uuid,
    deadline: Instant,
}

impl LockedEscape {
    fn new(now: Instant, timeout: Duration) -> Self {
        Self {
            token: Uuid::new_v4(),
            deadline: now + timeout,
        }
    }

    fn completes(&self, key: &KeyWithModifier, now: Instant) -> bool {
        now < self.deadline && key.bare_key == BareKey::Char('g') && key.has_no_modifiers()
    }
}

fn starts_escape(key: &KeyWithModifier, mode: InputMode, actions: &[Action]) -> bool {
    mode == InputMode::Locked
        && key.bare_key == BareKey::Char('g')
        && key.has_only_modifiers(&[KeyModifier::Ctrl])
        && matches!(
            actions,
            [Action::SwitchToMode {
                input_mode: InputMode::Normal
            }]
        )
}

impl SessionMetaData {
    pub(crate) fn resolve_locked_escape(&mut self, client_id: ClientId, token: Uuid) -> Result<()> {
        if self
            .locked_escapes
            .get(&client_id)
            .map(|pending| pending.token)
            != Some(token)
        {
            return Ok(());
        }
        self.senders
            .send_to_screen(ScreenInstruction::CloseContextMenu(client_id))
            .context("closing context menu for locked escape unlock")
            .non_fatal();
        self.senders
            .send_to_plugin(PluginInstruction::UserInput {
                client_id,
                action: Action::SwitchToMode {
                    input_mode: InputMode::Normal,
                },
                terminal_id: None,
                cli_client_id: None,
            })
            .context("logging locked escape unlock")
            .non_fatal();
        self.senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                Some(client_id),
                Event::InputReceived,
            )]))
            .context("reporting locked escape unlock")
            .non_fatal();
        self.change_input_mode(client_id, InputMode::Normal, None)
    }
}

/// Called only for interactive input, after the watcher filter. Resolving the
/// prefix updates the authoritative mode and queues its screen update under
/// the same lock, before the next key can be resolved or sent to the screen.
pub(crate) fn prepare_input(
    instruction: ClientToServerMsg,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
    to_server: &SenderWithContext<ServerInstruction>,
    client_id: ClientId,
) -> Result<Option<ClientToServerMsg>> {
    if !matches!(
        &instruction,
        ClientToServerMsg::Key { .. }
            | ClientToServerMsg::Paste(_)
            | ClientToServerMsg::Action {
                is_cli_client: false,
                ..
            }
    ) {
        return Ok(Some(instruction));
    }

    {
        let mut guard = session_data
            .write()
            .to_anyhow()
            .context("locking escape state")?;
        let Some(session) = guard.as_mut() else {
            return Ok(Some(instruction));
        };
        if session
            .command_palette_restore_modes
            .contains_key(&client_id)
        {
            return Ok(Some(instruction));
        }
        let pending = session.locked_escapes.get(&client_id).copied();
        if let (Some(pending), ClientToServerMsg::Key { key, .. }) = (pending, &instruction)
            && pending.completes(key, Instant::now())
        {
            session.locked_escapes.remove(&client_id);
            return Ok(Some(ClientToServerMsg::Action {
                action: Action::Write {
                    key_with_modifier: None,
                    bytes: vec![7],
                    is_kitty_keyboard_protocol: false,
                },
                terminal_id: None,
                client_id: None,
                is_cli_client: false,
            }));
        }
        if let Some(pending) = pending {
            session.resolve_locked_escape(client_id, pending.token)?;
        }
    }

    if let ClientToServerMsg::Key {
        key,
        raw_bytes,
        is_kitty_keyboard_protocol,
    } = &instruction
    {
        if key.bare_key != BareKey::Char('g') || !key.has_only_modifiers(&[KeyModifier::Ctrl]) {
            return Ok(Some(instruction));
        }
        let pending = {
            let mut guard = session_data
                .write()
                .to_anyhow()
                .context("locking escape state")?;
            let Some(session) = guard.as_mut() else {
                return Ok(Some(instruction));
            };
            if session.key_passthrough_clients.contains_key(&client_id)
                || session
                    .command_palette_restore_modes
                    .contains_key(&client_id)
            {
                return Ok(Some(instruction));
            }
            let timeout = session
                .session_configuration
                .get_client_locked_escape_timeout(&client_id);
            if timeout.is_zero() {
                // The prefix is configured off: Ctrl+g goes straight to its binding, which
                // means no pending escape to hold and no timer to expire it.
                return Ok(Some(instruction));
            }
            let Some((bindings, mode, default_mode)) =
                session.get_client_keybinds_and_mode(&client_id)
            else {
                return Ok(Some(instruction));
            };
            let actions = bindings.get_actions_for_key_in_mode_or_default_action(
                mode,
                key,
                raw_bytes.clone(),
                default_mode,
                *is_kitty_keyboard_protocol,
            );
            if !starts_escape(key, *mode, &actions) {
                return Ok(Some(instruction));
            }
            let pending = LockedEscape::new(Instant::now(), timeout);
            session.locked_escapes.insert(client_id, pending);
            pending
        };
        let to_server = to_server.clone();
        get_tokio_runtime().spawn(async move {
            tokio::time::sleep_until(pending.deadline.into()).await;
            to_server
                .send(ServerInstruction::ResolveLockedEscape(
                    client_id,
                    pending.token,
                ))
                .context("expiring locked escape")
                .non_fatal();
        });
        return Ok(None);
    }
    Ok(Some(instruction))
}

#[cfg(test)]
#[path = "unit/locked_escape_tests.rs"]
mod tests;
