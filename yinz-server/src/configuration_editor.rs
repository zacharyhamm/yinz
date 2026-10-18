use super::*;
use std::hash::{Hash, Hasher};
use yinz_config::editor::{self, find_node, Disk};
use yinz_types::configuration::{Destination, Edit, Patch};

impl SessionConfiguration {
    fn editor_revision(&self, disk: &Disk) -> String {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        format!("{:?}", disk).hash(&mut hash);
        format!("{:?}", self.saved_config).hash(&mut hash);
        let clients: BTreeMap<_, _> = self.runtime_config.iter().collect();
        format!("{clients:?}").hash(&mut hash);
        format!("{:?}", self.detached_runtime_config).hash(&mut hash);
        format!("{:?}", self.editor_overrides).hash(&mut hash);
        format!("{:016x}", hash.finish())
    }

    fn editor_snapshot(
        &self,
        disk: &Disk,
        notice: &str,
        theme: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let clients = self
            .runtime_config
            .keys()
            .map(|client| {
                Ok((
                    client.to_string(),
                    editor::inspection_document(
                        &self.get_client_configuration(client),
                        theme,
                        true,
                    )?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let (_, saved, _) = disk.preview(&[], &[])?;
        Ok(serde_json::json!({
            "revision": self.editor_revision(disk), "catalog": editor::catalog(),
            "saved": editor::inspection_document(&saved, theme, false)?, "clients": clients,
            "overrides": self.editor_overrides, "notice": notice, "actions": editor::action_templates(&saved)?, "action_properties": editor::action_properties(), "action_children": editor::action_children(), "sections": editor::SECTIONS, "section_timing": {"env": "On next session start"}, "templates": editor::templates(&saved)?,
        }))
    }

    pub(super) fn apply_editor_overrides(&self, config: Config) -> Config {
        match editor::apply(&config, &self.editor_overrides) {
            Ok(config) => config,
            Err(error) => {
                log::error!("Cannot apply configuration overrides: {error}");
                config
            },
        }
    }
}

impl SessionMetaData {
    pub(super) fn apply_palette_theme(
        &mut self,
        client: ClientId,
        request: uuid::Uuid,
        name: &str,
        permanent: bool,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<(), String> {
        let disk = Disk::read(self.config_file_path.as_deref(), self.config_dir.as_deref())?;
        let current = self.session_configuration.get_client_configuration(&client);
        let fragment = theme_fragment(&current, name).map_err(|e| e.to_string())?;
        let options = Config::from_kdl(&fragment, None).map_err(|e| e.to_string())?;
        let patches = editor::document(&options)?
            .into_iter()
            .filter(|n| matches!(n.name.as_str(), "theme" | "theme_dark" | "theme_light"))
            .map(|node| Patch {
                reset: false,
                path: vec![node.name.clone()],
                node: Some(node),
            })
            .collect();
        let edit = Edit {
            // This edit is created and applied in one server turn; only editor drafts need revisions.
            revision: String::new(),
            destination: if permanent {
                Destination::Permanent
            } else {
                Destination::Session
            },
            patches,
            reset: vec![],
        };
        self.apply_configuration_edit(&disk, edit, Some(cancelled), Some((client, request)))?;
        Ok(())
    }

    pub(super) fn inspect_configuration(
        &self,
        theme: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let disk = Disk::read(self.config_file_path.as_deref(), self.config_dir.as_deref())?;
        self.session_configuration.editor_snapshot(&disk, "", theme)
    }

    pub(super) fn configuration_editor(
        &mut self,
        edit: Option<Edit>,
    ) -> Result<serde_json::Value, String> {
        let disk = Disk::read(self.config_file_path.as_deref(), self.config_dir.as_deref())?;
        let Some(edit) = edit else {
            return self.session_configuration.editor_snapshot(&disk, "", None);
        };
        if edit.revision != self.session_configuration.editor_revision(&disk) {
            return Err("Configuration changed. Reload before applying your draft.".into());
        }
        let notice = self.apply_configuration_edit(&disk, edit, None, None)?;
        let disk = Disk::read(self.config_file_path.as_deref(), self.config_dir.as_deref())?;
        self.session_configuration
            .editor_snapshot(&disk, &notice, None)
    }

    fn apply_configuration_edit(
        &mut self,
        disk: &Disk,
        mut edit: Edit,
        cancelled: Option<&std::sync::atomic::AtomicBool>,
        theme_reply: Option<(ClientId, uuid::Uuid)>,
    ) -> Result<String, String> {
        if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err("Theme selection was cancelled".into());
        }
        if edit.patches.len() + edit.reset.len() > 1024 {
            return Err("Too many configuration edits".into());
        }
        edit.patches = editor::normalize_patches(&edit.patches)?;
        edit.reset = edit
            .reset
            .iter()
            .map(|path| editor::expand_binding_paths(path))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();
        let catalog = editor::catalog();
        if edit.destination == Destination::Session {
            for path in edit.patches.iter().map(|p| &p.path).chain(&edit.reset) {
                if path.first().is_some_and(|root| root == "env")
                    || catalog.iter().any(|s| {
                        path.first() == Some(&s.name) && s.timing == "On next session start"
                    })
                {
                    return Err("This setting can only be saved permanently; it takes effect on next session start".into());
                }
            }
        }
        let mut staged = self.session_configuration.clone();
        let touched: Vec<_> = edit
            .patches
            .iter()
            .map(|p| p.path.clone())
            .chain(edit.reset.iter().cloned())
            .collect();
        staged
            .editor_overrides
            .retain(|p| !touched.iter().any(|path| p.path.starts_with(path)));
        for path in &touched {
            if staged
                .editor_overrides
                .iter()
                .any(|p| path.starts_with(&p.path))
            {
                staged.editor_overrides.push(Patch {
                    path: path.clone(),
                    node: None,
                    reset: true,
                });
            }
        }
        if edit.destination == Destination::Session {
            staged.editor_overrides.extend(edit.patches.clone());
        }
        let mut notice = "Applied to this session, including future clients".to_owned();
        let pending_write = if edit.destination == Destination::Permanent {
            let (fragment, saved, underlay) = disk.preview(&edit.patches, &edit.reset)?;
            // Keep client-specific values; only the submitted paths are reconciled with disk.
            let saved_nodes = editor::document(&saved)?;
            let saved_patches: Vec<_> = touched
                .iter()
                .map(|path| Patch {
                    reset: false,
                    path: path.clone(),
                    node: find_node(&saved_nodes, path).cloned(),
                })
                .collect();
            if touched.iter().any(|path| {
                matches!(
                    path.first().map(String::as_str),
                    Some("plugins" | "load_plugins")
                )
            }) {
                let mut background = staged.saved_config.clone();
                if let Some(current) = &staged.background_plugins {
                    background.background_plugins = current.declarations.clone();
                    background.background_plugins_cleared = current.cleared;
                    background.plugins.merge(current.aliases.clone());
                }
                staged.background_plugins = Some(BackgroundPluginConfiguration::from_config(
                    &editor::apply(&background, &saved_patches)?,
                ));
            }
            staged.pending_editor_reload = Some(saved.clone());
            staged.saved_config = saved;
            for config in staged
                .runtime_config
                .values_mut()
                .chain(staged.detached_runtime_config.iter_mut())
            {
                *config = editor::apply(config, &saved_patches)?;
            }
            notice = "Saved permanently".into();
            if theme_reply.is_none() {
                let (_, previous_saved, _) = disk.preview(&[], &[])?;
                let expected_nodes =
                    editor::document(&editor::apply(&previous_saved, &edit.patches)?)?;
                if edit.patches.iter().any(|p| {
                    find_node(&saved_nodes, &p.path) != find_node(&expected_nodes, &p.path)
                }) {
                    notice.push_str("; another configuration layer overrides part of this change");
                }
                if touched.iter().any(|path| {
                    catalog.iter().any(|s| {
                        path.first() == Some(&s.name) && s.timing == "On next session start"
                    })
                }) {
                    notice.push_str("; startup settings take effect on next session start");
                }
            }
            Some((fragment, underlay))
        } else {
            None
        };
        let mut changes = vec![];
        staged.editor_effective.clear();
        for client in staged.runtime_config.keys().chain(
            staged
                .detached_runtime_config
                .as_ref()
                .map(|_| &ClientId::NONE),
        ) {
            let base = staged.base_client_configuration(client);
            let effective = editor::apply(&base, &staged.editor_overrides)?;
            if staged.runtime_config.contains_key(client) {
                staged.editor_effective.insert(*client, effective.clone());
            }
            let effective = effective
                .resolve_plugin_repositories(
                    self.config_file_path.as_deref(),
                    self.config_dir.as_deref(),
                )
                .map_err(|e| e.to_string())?;
            validate_background_plugins(&effective).map_err(|e| format!("{e:#}"))?;
            if effective != self.session_configuration.get_client_configuration(client) {
                changes.push((*client, effective));
            }
        }
        validate_background_plugins(&editor::apply(
            &staged.saved_config,
            &staged.editor_overrides,
        )?)
        .map_err(|e| format!("{e:#}"))?;
        if let Some((fragment, underlay)) = pending_write {
            Config::try_write_editor_fragment(&disk.overlay, &fragment, underlay, || {
                if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                    return Err("Theme selection was cancelled".into());
                }
                let now = Disk::read(self.config_file_path.as_deref(), self.config_dir.as_deref())?;
                if now != *disk {
                    Err("Configuration changed on disk. Reload before saving.".into())
                } else {
                    Ok(())
                }
            })?;
        }
        staged.refresh_effective_keybinds();
        self.session_configuration = staged;
        // Close the preview before reconfiguration can restore its original theme.
        if let Some((client, request)) = theme_reply {
            self.senders
                .send_to_screen(ScreenInstruction::PaletteThemeSaved {
                    client,
                    request,
                    result: Ok(()),
                })
                .context("acknowledging theme edit")
                .non_fatal();
        }
        self.propagate_configuration_changes(changes, edit.destination == Destination::Permanent);
        Ok(notice)
    }
}

#[cfg(test)]
#[path = "unit/configuration_editor.rs"]
mod tests;
