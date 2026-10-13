//! Session-level instructions: a client attaching, the session being saved or renamed, and the
//! parts of `Reconfigure` that reach past `Screen::reconfigure`.
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TabNumber;

use super::thread_main::{replay_pending_pane_event, PendingEvents};
use super::Screen;
use crate::panes::PaneId;
use crate::pty::PtyInstruction;
use crate::route::NotificationEnd;
use crate::session_layout_metadata::SessionLayoutMetadata;
use crate::ClientId;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use yinz_ipc::ipc::ServerToClientMsg;
use yinz_runtime::consts::YINZ_SOCK_DIR;
use yinz_types::data::SessionInfo;
use yinz_types::pane_size::Size;
use yinz_types::style::ThemeHue;

impl Screen {
    /// Renames the session, refusing a name another live or resurrectable session holds, and
    /// tells every client the new name.
    pub(super) fn rename_session(&mut self, name: SessionName, client_id: ClientId) -> Result<()> {
        if name == self.session_name {
            return Ok(());
        }
        let ownership = self.bus.senders.session_ownership.clone();
        let has_ownership = ownership.is_some();
        {
            let err_context = || "Failed to rename session".to_string();
            let old_session_name = self.session_name.clone();
            let mut mother_stopped = false;
            let mut transaction = || -> Result<()> {
                if !has_ownership
                    && (self.peer_sessions_cache.contains_key(&name)
                        || YINZ_SOCK_DIR
                            .join(&name)
                            .try_exists()
                            .context("check destination socket")?)
                {
                    return Err(anyhow!("A session by this name already exists."));
                }
                if self.resurrectable_sessions_cache.contains_key(&name)
                    || yinz_runtime::consts::session_info_folder_for_session(&name)
                        .try_exists()
                        .context("check destination session cache")?
                {
                    return Err(anyhow!(
                        "A resurrectable session by this name exists, cannot use this name."
                    ));
                }
                mother_stopped = true;
                self.mother_supervisor = None;
                self.mother_status = None;
                super::mother_supervisor::rename_memory(
                    &yinz_runtime::consts::YINZ_CACHE_DIR,
                    old_session_name.as_str(),
                    name.as_str(),
                    || {
                        if let Some(os_input) = &self.bus.os_input {
                            os_input.rename_pty_host_session(&old_session_name, &name)?;
                        }
                        let result = self
                            .bus
                            .senders
                            .session_checkpoint
                            .rename(&old_session_name, &name)
                            .and_then(|()| {
                                std::fs::rename(
                                    YINZ_SOCK_DIR.join(&old_session_name),
                                    YINZ_SOCK_DIR.join(&name),
                                )
                                .context("rename IPC socket")
                                .inspect_err(|_| {
                                    self.bus
                                        .senders
                                        .session_checkpoint
                                        .rename(&name, &old_session_name)
                                        .context(
                                            "restore session checkpoint after socket rename failed",
                                        )
                                        .non_fatal();
                                })
                            });
                        if result.is_err() {
                            if let Some(os_input) = &self.bus.os_input {
                                os_input
                                    .rename_pty_host_session(&name, &old_session_name)
                                    .context("restore session ownership after rename failed")
                                    .non_fatal();
                            }
                        }
                        result
                    },
                )
            };
            let renamed = match ownership {
                Some(ownership) => ownership.rename(&YINZ_SOCK_DIR.join(&name), transaction),
                None => transaction(),
            };
            if let Err(error) = renamed {
                if let Some(os_input) = &self.bus.os_input {
                    os_input
                        .send_to_client(
                            client_id,
                            ServerToClientMsg::LogError {
                                lines: vec![format!("Failed to rename session: {error:#}")],
                            },
                        )
                        .non_fatal();
                }
                Err::<(), _>(error)
                    .context("Failed to rename session")
                    .non_fatal();
                if mother_stopped {
                    self.sync_mother_supervisor();
                }
                return Ok(());
            }

            // update state
            self.session_name = name.clone();
            self.default_mode_info.session_name = Some(name.clone());
            for mode_info in self.mode_info.values_mut() {
                mode_info.session_name = Some(name.clone());
            }
            for tab in self.tabs.values_mut() {
                tab.rename_session(name.clone()).with_context(err_context)?;
            }

            self.sync_mother_supervisor();

            // report
            self.log_and_report_session_state()
                .with_context(err_context)?;

            let connected_client_ids: Vec<ClientId> = self.active_tab_ids.keys().copied().collect();
            for client_id in connected_client_ids {
                if let Some(os_input) = &mut self.bus.os_input {
                    let _ = os_input.send_to_client(
                        client_id,
                        ServerToClientMsg::RenamedSession { name: name.clone() },
                    );
                }
            }
        }
        Ok(())
    }

    /// Collects the session's current shape and hands it to the pty thread to write to disk.
    pub(super) fn save_session(&mut self, completion_tx: Option<NotificationEnd>) -> Result<()> {
        let err_context = || "Failed to save session";

        self.update_active_pane_ids();
        let pane_manifest = self.generate_and_report_pane_state()?;
        let tab_infos = self.generate_and_report_tab_state()?;

        #[cfg(not(test))]
        let (available_layouts, _layout_errors) = yinz_config::layout::list_available_layouts(
            self.layout_dir.clone(),
            &self.default_layout_name,
        );
        #[cfg(test)]
        let available_layouts = vec![];

        let creation_time = {
            let sock_path = YINZ_SOCK_DIR.join(&self.session_name);
            std::fs::metadata(&sock_path)
                .ok()
                .and_then(|f| f.created().ok().or_else(|| f.modified().ok()))
                .and_then(|d| d.elapsed().ok())
                .map(|d| Duration::from_secs(d.as_secs()))
                .unwrap_or_default()
        };
        let session_info = SessionInfo {
            mother: self.reported_mother(&pane_manifest),
            mother_supervision: self
                .reported_mother(&pane_manifest)
                .and(self.mother_status.clone()),
            name: self.session_name.clone(),
            tabs: tab_infos,
            panes: pane_manifest,
            connected_clients: self.active_tab_ids.keys().len(),
            is_current_session: true,
            available_layouts,
            plugins: Default::default(),
            tab_history: self
                .tab_history
                .iter()
                .map(|(id, history)| (*id, history.clone()))
                .collect(),
            pane_history: self
                .pane_history
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect(),
            creation_time,
        };

        let session_layout_metadata = if self.session_serialization {
            self.get_layout_metadata(Some(self.default_shell.clone()), None)
        } else {
            // Create empty metadata if serialization is disabled
            SessionLayoutMetadata::new(self.default_layout.clone())
        };

        self.bus
            .senders
            .send_to_pty(PtyInstruction::SaveSessionToDisk {
                session_name: self.session_name.clone(),
                session_info,
                session_layout_metadata,
                completion_tx,
            })
            .with_context(err_context)?;
        Ok(())
    }

    /// A client has attached: size the surface for it, reflow every tab, restore the focus it
    /// asked for, replay what was waiting for a client, and report and render on the way out.
    /// Rejects the client if its size leaves nothing to lay out.
    ///
    /// The trace lines around each step feed `attach_traces_waiting_for_first_render`, which
    /// the first render after this closes.
    pub(super) fn attach_client(
        &mut self,
        client_id: ClientId,
        client_size: Size,
        tab_number_to_focus: Option<TabNumber>,
        pane_id_to_focus: Option<PaneId>,
        attach_traces_waiting_for_first_render: &mut HashMap<ClientId, Instant>,
        pending: &mut PendingEvents,
    ) -> Result<()> {
        let attach_started_at = Instant::now();
        attach_traces_waiting_for_first_render.insert(client_id, attach_started_at);
        log::info!(
            "attach trace: screen client_id={client_id} AddClient begin size={client_size:?}"
        );
        let departed = self.departed_follow_source();
        let retired_source =
            match self.register_client_transaction(client_id, departed, Some(client_size), true) {
                Ok(source) => source,
                Err(error) => {
                    Err::<(), _>(error)
                        .context("failed to attach client at its reported size")
                        .non_fatal();
                    attach_traces_waiting_for_first_render.remove(&client_id);
                    return Ok(());
                },
            };
        if let Some(source) = retired_source {
            pending.remove_client_plugin_output(source);
        }
        if let Some(pane_id) = pane_id_to_focus {
            log::info!(
                "attach trace: screen client_id={client_id} restoring pane focus {pane_id:?}"
            );
            self.focus_pane_with_id(pane_id, true, false, client_id)?;
        } else if let Some(tab_number_to_focus) = tab_number_to_focus {
            log::info!(
            "attach trace: screen client_id={client_id} restoring tab focus number={tab_number_to_focus}"
        );
            self.go_to_tab(tab_number_to_focus, client_id)?;
        }
        log::info!(
            "attach trace: screen client_id={client_id} focus restoration complete after {:?}",
            attach_started_at.elapsed()
        );
        self.show_guest_modals_for_new_client(client_id);
        let focused_pane_id = self.get_active_pane_id(&client_id);
        if let Some(focused) = focused_pane_id {
            self.report_key_passthrough_state(client_id, focused, focused);
        }
        log::info!("attach trace: screen client_id={client_id} sync_scroll_mode_on_focus begin");
        self.sync_scroll_mode_on_focus(client_id)?;
        log::info!(
            "attach trace: screen client_id={client_id} pending events drain begin after {:?}",
            attach_started_at.elapsed()
        );
        for event in std::mem::take(&mut pending.waiting_for_client) {
            replay_pending_pane_event(self, event, pending).non_fatal();
        }
        log::info!("attach trace: screen client_id={client_id} log_and_report_session_state begin");
        self.log_and_report_session_state()?;

        log::info!(
            "attach trace: screen client_id={client_id} scheduling first render after {:?}",
            attach_started_at.elapsed()
        );
        self.render()?;
        log::info!(
            "attach trace: screen client_id={client_id} AddClient handler complete after {:?}",
            attach_started_at.elapsed()
        );
        Ok(())
    }

    /// `Reconfigure` changed `agent_double_escape`: every tab takes the new value and forgets
    /// the half-typed escapes it was counting under the old one.
    pub(super) fn set_agent_double_escape(&mut self, enabled: bool) {
        self.agent_double_escape = enabled;
        for tab in self.tabs.values_mut() {
            tab.agent_double_escape = enabled;
            tab.pending_agent_escapes.clear();
        }
    }

    /// `reconfigure` painted the static theme; put the session's effective hue back on top of
    /// it, against the possibly changed dark/light definitions.
    pub(super) fn reapply_theme_after_reconfigure(&mut self, explicit_theme_hue: Option<ThemeHue>) {
        if explicit_theme_hue != self.configured_explicit_theme_hue {
            self.apply_configured_explicit_theme_hue(explicit_theme_hue)
                .non_fatal();
        } else {
            self.reapply_effective_theme_mode().non_fatal();
        }
        self.resolve_default_theme_mode().non_fatal();
    }
}
