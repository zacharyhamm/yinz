//! Plugin layout parsing, permission validation, application, and persistence.

use super::pane_creation::{run_action, translate_plugin_path};
use super::permissions::{ensure_plugin_permissions, LAYOUT_FILE_PERMISSIONS};
use super::write_response;
use crate::plugin_map::PluginEnv;
use crate::PaneId;
use kdl::KdlDocument;
use prost::Message;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use yinz_config::config::ConfigError;
use yinz_plugin_api::event::layout_parsing_error::ErrorType as ProtobufLayoutParsingErrorType;
use yinz_plugin_api::event::{ProtobufLayoutParsingError, ProtobufSyntaxError};
use yinz_plugin_api::plugin_command::{
    dump_layout_response, parse_layout_response, ProtobufDeleteLayoutResponse,
    ProtobufDumpLayoutResponse, ProtobufEditLayoutResponse, ProtobufGetLayoutDirResponse,
    ProtobufNewTabResponse, ProtobufNewTabUnfocusedResponse, ProtobufNewTabsResponse,
    ProtobufNewTiledPaneInTabResponse, ProtobufParseLayoutResponse, ProtobufRenameLayoutResponse,
    ProtobufSaveLayoutResponse,
};
use yinz_routing::route_action;
use yinz_runtime::errors::prelude::*;
use yinz_runtime::home::default_layout_dir;
use yinz_types::data::{
    DeleteLayoutResponse, EditLayoutResponse, FileToOpen, LayoutInfo, LayoutMetadata,
    LayoutParsingError, NewPanePlacement, NewTabResponse, NewTabUnfocusedResponse,
    NewTiledPaneInTabResponse, PermissionType, RenameLayoutResponse, SaveLayoutResponse,
    TabMetadata,
};
use yinz_types::input::actions::Action;
use yinz_types::input::command::{OpenFilePayload, RunCommand, RunCommandAction, TerminalAction};
use yinz_types::input::layout::{Layout, Run, TabLayoutInfo};
use yinz_types::internal_ids::{TabId, TabPosition};

pub(super) fn dump_layout(env: &mut PluginEnv, layout_name: String) {
    let layout_path = PathBuf::from(&layout_name);

    let layout_dir = env.layout_dir.clone().or_else(default_layout_dir);
    let result = validate_plugin_layout_path(&layout_name, layout_dir.as_ref())
        .map_err(|e| e.to_string())
        .and_then(|_| {
            yinz_config::layout::stringified_from_dir(&layout_path, layout_dir.as_ref())
                .map_err(|e| e.to_string())
        });
    let response = match result {
        Ok((_, layout_content, _)) => ProtobufDumpLayoutResponse {
            result: Some(dump_layout_response::Result::LayoutContent(layout_content)),
        },
        Err(e) => ProtobufDumpLayoutResponse {
            result: Some(dump_layout_response::Result::Error(e.to_string())),
        },
    };

    write_response(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send layout dump to plugin {}", env.name()))
        .non_fatal();
}

pub(super) fn get_layout_dir(env: &mut PluginEnv) {
    // Get layout dir from env or fall back to default
    let layout_dir = env.layout_dir.clone().or_else(default_layout_dir);

    // Convert PathBuf to String
    let layout_dir_string = layout_dir
        .and_then(|path| path.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| String::from(""));

    let response = ProtobufGetLayoutDirResponse {
        layout_dir: layout_dir_string,
    };

    write_response(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send layout dir to plugin {}", env.name()))
        .non_fatal();
}

pub(super) fn parse_layout(env: &mut PluginEnv, layout_string: String) {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Parse the KDL layout - same parameters as list_available_layouts in layout.rs:1260
    let parse_result = yinz_config::kdl::layout::from_kdl(
        &layout_string,
        Some("parse_layout_api".to_string()), // file_name for error reporting
        None,                                 // no swap layouts
        None,                                 // no cwd
    );

    let response = match parse_result {
        Ok(layout) => {
            // Extract tabs from layout
            // This logic matches LayoutMetadata::from in data.rs:1762-1772
            let layout_tabs = layout.tabs();
            let tabs = if layout_tabs.is_empty() {
                // Use default template if no explicit tabs defined
                let (tiled_pane_layout, floating_pane_layout) = layout.new_tab();
                vec![TabMetadata::from(&(
                    None,
                    tiled_pane_layout,
                    floating_pane_layout,
                ))]
            } else {
                layout
                    .tabs()
                    .into_iter()
                    .map(|tab| TabMetadata::from(&tab))
                    .collect()
            };

            // Get current time as Unix epoch timestamp
            let current_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string();

            let layout_metadata = LayoutMetadata {
                tabs,
                creation_time: current_time.clone(),
                update_time: current_time,
            };

            // Convert LayoutMetadata to protobuf
            match layout_metadata.try_into() {
                Ok(protobuf_metadata) => ProtobufParseLayoutResponse {
                    result: Some(parse_layout_response::Result::Metadata(protobuf_metadata)),
                },
                Err(e) => {
                    log::error!("Failed to convert metadata to protobuf: {}", e);
                    // Fallback to SyntaxError if conversion fails
                    let error = LayoutParsingError::SyntaxError;
                    let protobuf_error = error.try_into().unwrap_or(ProtobufLayoutParsingError {
                        error_type: Some(ProtobufLayoutParsingErrorType::SyntaxError(
                            ProtobufSyntaxError {},
                        )),
                    });
                    ProtobufParseLayoutResponse {
                        result: Some(parse_layout_response::Result::Error(protobuf_error)),
                    }
                },
            }
        },
        Err(config_error) => {
            // Build LayoutParsingError following list_available_layouts pattern
            // See yinz-types/src/input/layout.rs
            let file_name = "parse_layout_api".to_string();
            let source_code = layout_string;

            let error = match config_error {
                ConfigError::KdlError(kdl_err) => LayoutParsingError::KdlError {
                    kdl_error: Box::new(kdl_err.into()),
                    file_name,
                    source_code,
                },
                _ => LayoutParsingError::SyntaxError,
            };

            // Convert LayoutParsingError to protobuf
            // TryFrom is implemented in yinz-plugin-api/src/event.rs
            let protobuf_error = error.try_into().unwrap_or_else(|e| {
                log::error!("Failed to convert error to protobuf: {}", e);
                ProtobufLayoutParsingError {
                    error_type: Some(ProtobufLayoutParsingErrorType::SyntaxError(
                        ProtobufSyntaxError {},
                    )),
                }
            });

            ProtobufParseLayoutResponse {
                result: Some(parse_layout_response::Result::Error(protobuf_error)),
            }
        },
    };

    write_response(env, &response.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to send parse layout response to plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

pub(super) fn layout_run_permissions(layout: &Layout) -> BTreeSet<PermissionType> {
    let mut permissions = BTreeSet::new();
    let mut inspect =
        |tiled_layout: &yinz_types::input::layout::TiledPaneLayout,
         floating_layouts: &[yinz_types::input::layout::FloatingPaneLayout]| {
            let runs = tiled_layout
                .extract_run_instructions()
                .into_iter()
                .flatten()
                .chain(
                    floating_layouts
                        .iter()
                        .filter(|pane| !pane.already_running)
                        .filter_map(|pane| pane.run.clone()),
                );
            for run in runs {
                match run {
                    Run::Command(_) => {
                        permissions.insert(PermissionType::RunCommands);
                    },
                    Run::EditFile(..) => {
                        permissions.insert(PermissionType::OpenFiles);
                    },
                    Run::Plugin(_) => {
                        permissions.insert(PermissionType::OpenTerminalsOrPlugins);
                    },
                    Run::Cwd(_) => {},
                }
            }
        };
    if layout.tabs.is_empty() {
        if let Some((tiled_layout, floating_layouts)) = layout.template.as_ref() {
            inspect(tiled_layout, floating_layouts);
        }
    } else {
        for (_, tiled_layout, floating_layouts) in &layout.tabs {
            inspect(tiled_layout, floating_layouts);
        }
    }
    permissions
}

pub(super) fn validate_plugin_layout_path(
    layout_name: &str,
    layout_dir: Option<&PathBuf>,
) -> Result<()> {
    let requested = Path::new(layout_name);
    if requested.components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        return Err(anyhow!(
            "plugin layout path must stay inside the layout directory: {}",
            requested.display()
        ));
    }

    let Some(layout_dir) = layout_dir.cloned().or_else(default_layout_dir) else {
        return Ok(());
    };
    let requested = layout_dir.join(requested);
    let candidates = [
        requested.clone(),
        requested.with_extension("kdl"),
        requested.with_extension("swap.kdl"),
    ];
    if !candidates.iter().any(|candidate| candidate.exists()) {
        return Ok(());
    }

    let canonical_root = layout_dir.canonicalize().with_context(|| {
        format!(
            "failed to resolve layout directory {}",
            layout_dir.display()
        )
    })?;
    for candidate in candidates
        .into_iter()
        .filter(|candidate| candidate.exists())
    {
        let canonical_candidate = candidate
            .canonicalize()
            .with_context(|| format!("failed to resolve layout path {}", candidate.display()))?;
        if !canonical_candidate.starts_with(&canonical_root) {
            return Err(anyhow!(
                "plugin layout path resolves outside the layout directory: {}",
                candidate.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn ensure_layout_info_permissions(
    env: &PluginEnv,
    layout_info: &LayoutInfo,
) -> Result<()> {
    match layout_info {
        LayoutInfo::File(layout_name, _) => {
            validate_plugin_layout_path(layout_name, env.layout_dir.as_ref())?;
            ensure_plugin_permissions(env, LAYOUT_FILE_PERMISSIONS)
        },
        LayoutInfo::BuiltIn(_) | LayoutInfo::Stringified(_) => {
            let layout =
                yinz_config::layout::from_layout_info(&env.layout_dir, layout_info.clone())
                    .map_err(|e| anyhow!("Failed to parse layout: {:?}", e))?;
            ensure_plugin_permissions(env, layout_run_permissions(&layout))
        },
    }
}

fn layout_from_plugin_info(env: &PluginEnv, layout_info: LayoutInfo) -> Result<Layout> {
    ensure_layout_info_permissions(env, &layout_info)?;
    yinz_config::layout::from_layout_info(&env.layout_dir, layout_info)
        .map_err(|e| anyhow!("Failed to parse layout: {:?}", e))
}

pub(super) fn new_tabs_with_layout(env: &mut PluginEnv, raw_layout: &str) -> Result<()> {
    let layout = yinz_config::layout::from_str(
        raw_layout,
        format!("Layout from plugin: {}", env.name()),
        None,
        None,
    )
    .map_err(|e| anyhow!("Failed to parse layout: {:?}", e))?;
    ensure_plugin_permissions(env, layout_run_permissions(&layout))?;
    apply_layout(env, layout);
    Ok(())
}

pub(super) fn new_tabs_with_layout_info(
    env: &mut PluginEnv,
    layout_info: LayoutInfo,
) -> Result<()> {
    let layout = layout_from_plugin_info(env, layout_info)?;
    apply_layout(env, layout);
    Ok(())
}

fn apply_layout(env: &mut PluginEnv, layout: Layout) {
    let mut tabs_to_open = vec![];
    let tabs = layout.tabs();
    // KDL already embeds layout/tab/pane cwds in the runs. Leave the tab override
    // unset so panes without a cwd inherit normally, as with new_tab(None, None).
    let cwd = None;
    if tabs.is_empty() {
        let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
        let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());
        let action = Action::NewTab {
            layout_source: None,
            tiled_layout: layout.template.as_ref().map(|t| t.0.clone()),
            floating_layouts: layout.template.map(|t| t.1).unwrap_or_default(),
            swap_tiled_layouts,
            swap_floating_layouts,
            tab_name: None,
            should_change_focus_to_new_tab: true,
            cwd,
            initial_panes: None,
            first_pane_unblock_condition: None,
        };
        tabs_to_open.push(action);
    } else {
        let focused_tab_index = layout.focused_tab_index().unwrap_or(TabPosition::new(0));
        for (tab_index, (tab_name, tiled_pane_layout, floating_pane_layout)) in
            layout.tabs().into_iter().enumerate()
        {
            let should_focus_tab = TabPosition::new(tab_index) == focused_tab_index;
            let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
            let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());
            let action = Action::NewTab {
                layout_source: None,
                tiled_layout: Some(tiled_pane_layout),
                floating_layouts: floating_pane_layout,
                swap_tiled_layouts,
                swap_floating_layouts,
                tab_name,
                should_change_focus_to_new_tab: should_focus_tab,
                cwd: cwd.clone(),
                initial_panes: None,
                first_pane_unblock_condition: None,
            };
            tabs_to_open.push(action);
        }
    }
    let mut tab_ids = Vec::new();

    for action in tabs_to_open {
        let error_msg = || "Failed to create layout tab".to_string();
        let result = apply_action!(action, error_msg, env);

        // Collect tab ID from each action
        if let Some(tab_id) = result.and_then(|r| r.affected_tab_id) {
            tab_ids.push(tab_id);
        }
    }

    // Write response
    let response = ProtobufNewTabsResponse::from(tab_ids);
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write layout tabs response".to_string())
        .non_fatal();
}

pub(super) fn new_tab(env: &mut PluginEnv, name: Option<String>, cwd: Option<String>) {
    let cwd = cwd.map(|c| translate_plugin_path(env, PathBuf::from(c)));
    let action = Action::NewTab {
        layout_source: None,
        tiled_layout: None,
        floating_layouts: vec![],
        swap_tiled_layouts: None,
        swap_floating_layouts: None,
        tab_name: name,
        should_change_focus_to_new_tab: true,
        cwd,
        initial_panes: None,
        first_pane_unblock_condition: None,
    };
    let error_msg = || "Failed to open new tab".to_string();
    let result = apply_action!(action, error_msg, env);

    // Extract tab_id from result - return None if not present
    let tab_id: NewTabResponse = result.and_then(|r| r.affected_tab_id);

    // Write response to plugin
    let response = ProtobufNewTabResponse::from(tab_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write new_tab response".to_string())
        .non_fatal();
}

pub(super) fn new_tab_unfocused(env: &mut PluginEnv, name: Option<String>, cwd: Option<String>) {
    let cwd = cwd.map(|c| translate_plugin_path(env, PathBuf::from(c)));
    let action = Action::NewTab {
        layout_source: None,
        tiled_layout: None,
        floating_layouts: vec![],
        swap_tiled_layouts: None,
        swap_floating_layouts: None,
        tab_name: name,
        should_change_focus_to_new_tab: false,
        cwd,
        initial_panes: None,
        first_pane_unblock_condition: None,
    };
    let error_msg = || "Failed to open new tab (unfocused)".to_string();
    let result = apply_action!(action, error_msg, env);

    let tab_id: NewTabUnfocusedResponse = result.and_then(|r| r.affected_tab_id);

    let response = ProtobufNewTabUnfocusedResponse::from(tab_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write new_tab_unfocused response".to_string())
        .non_fatal();
}

pub(super) fn new_tiled_pane_in_tab(env: &mut PluginEnv, tab_id: TabId) {
    let error_msg = || format!("failed to open new tiled pane in tab {}", tab_id);
    let default_shell = env.default_shell.clone().unwrap_or_else(|| {
        TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    });
    let run_command_action: Option<RunCommandAction> = match default_shell {
        TerminalAction::RunCommand(run_command) => Some(run_command.into()),
        _ => None,
    };
    let action = Action::NewTiledPane {
        direction: None,
        command: run_command_action,
        pane_name: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: Some(tab_id),
    };
    let result = apply_action!(action, error_msg, env);

    let pane_id: NewTiledPaneInTabResponse = result.and_then(|r| r.affected_pane_id);

    let response = ProtobufNewTiledPaneInTabResponse::from(pane_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write new_tiled_pane_in_tab response".to_string())
        .non_fatal();
}

pub(super) fn save_layout(
    env: &mut PluginEnv,
    layout_name: String,
    layout_kdl: String,
    overwrite: bool,
) {
    let err_context = || format!("Failed to save layout '{}'", layout_name);

    let response = try_save_layout(env, &layout_name, &layout_kdl, overwrite)
        .map(|_| SaveLayoutResponse::Ok(()))
        .unwrap_or_else(SaveLayoutResponse::Err);

    // Serialize and send response back to plugin
    let protobuf_response = ProtobufSaveLayoutResponse::from(response);
    let serialized = protobuf_response.encode_to_vec();
    write_response(env, &serialized)
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn try_save_layout(
    env: &PluginEnv,
    layout_name: &str,
    layout_kdl: &str,
    overwrite: bool,
) -> Result<(), String> {
    // Step 1: Sanitize layout name for path traversal and invalid characters
    let safe_name = sanitize_layout_name(layout_name)?;

    // Step 2: Validate the layout by parsing it
    yinz_config::kdl::layout::from_kdl(
        layout_kdl,
        Some(format!("{}.kdl", safe_name)),
        None, // No swap layouts
        None, // No cwd override
    )
    .map_err(|config_error| format!("Invalid layout KDL: {:?}", config_error))?;

    // Step 3: Get layout_dir from PluginEnv
    let layout_dir = env
        .layout_dir
        .clone()
        .or_else(default_layout_dir)
        .ok_or_else(|| "Layout directory not found".to_string())?;

    let file_path = layout_dir.join(format!("{}.kdl", safe_name));
    std::fs::create_dir_all(&layout_dir)
        .map_err(|e| format!("Failed to create layout directory: {}", e))?;

    let mut parsed_layout: KdlDocument = layout_kdl
        .parse()
        .map_err(|e| format!("Invalid layout KDL: {}", e))?;
    parsed_layout.fmt();
    // Publish a complete file, without following a destination symlink or racing another save.
    let mut staged = tempfile::NamedTempFile::new_in(&layout_dir)
        .map_err(|e| format!("Failed to stage layout file: {}", e))?;
    staged
        .write_all(parsed_layout.to_string().as_bytes())
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|e| format!("Failed to write layout file: {}", e))?;
    let published = if overwrite {
        staged.persist(&file_path)
    } else {
        staged.persist_noclobber(&file_path)
    };
    published.map_err(|e| {
        if e.error.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "Layout file '{}' already exists. Use overwrite flag to replace it.",
                safe_name
            )
        } else {
            format!("Failed to publish layout file: {}", e)
        }
    })?;

    Ok(())
}

pub(super) fn delete_layout(env: &mut PluginEnv, layout_name: String) {
    let err_context = || format!("Failed to delete layout '{}'", layout_name);

    let response = try_delete_layout(env, &layout_name)
        .map(|_| DeleteLayoutResponse::Ok(()))
        .unwrap_or_else(DeleteLayoutResponse::Err);

    let protobuf_response: ProtobufDeleteLayoutResponse = response.into();
    let serialized = protobuf_response.encode_to_vec();
    write_response(env, &serialized)
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn rename_layout(env: &mut PluginEnv, old_layout_name: String, new_layout_name: String) {
    let err_context = || {
        format!(
            "Failed to rename layout '{}' to '{}'",
            old_layout_name, new_layout_name
        )
    };

    let response = try_rename_layout(env, &old_layout_name, &new_layout_name)
        .map(|_| RenameLayoutResponse::Ok(()))
        .unwrap_or_else(RenameLayoutResponse::Err);

    let protobuf_response: ProtobufRenameLayoutResponse = response.into();
    let serialized = protobuf_response.encode_to_vec();
    write_response(env, &serialized)
        .with_context(err_context)
        .non_fatal();
}

fn try_delete_layout(env: &PluginEnv, layout_name: &str) -> Result<(), String> {
    // Sanitize the layout name to prevent directory traversal
    let safe_name =
        sanitize_layout_name(layout_name).map_err(|e| format!("Invalid layout name: {}", e))?;

    // Get the layout directory from PluginEnv
    let layout_dir = env
        .layout_dir
        .clone()
        .or_else(default_layout_dir)
        .ok_or_else(|| "Layout directory not found".to_string())?;

    // Construct the full file path
    let file_path = layout_dir.join(format!("{}.kdl", safe_name));

    // Check if the file exists
    if !file_path.exists() {
        return Err(format!("Layout '{}' not found", safe_name));
    }

    // Delete the file
    std::fs::remove_file(&file_path).map_err(|e| format!("Failed to delete layout file: {}", e))?;

    Ok(())
}

pub(super) fn try_rename_layout(
    env: &PluginEnv,
    old_layout_name: &str,
    new_layout_name: &str,
) -> Result<(), String> {
    // Step 1: Sanitize both layout names
    let safe_old_name = sanitize_layout_name(old_layout_name)
        .map_err(|e| format!("Invalid old layout name: {}", e))?;

    let safe_new_name = sanitize_layout_name(new_layout_name)
        .map_err(|e| format!("Invalid new layout name: {}", e))?;

    // Step 2: Get layout directory from PluginEnv
    let layout_dir = env
        .layout_dir
        .clone()
        .or_else(default_layout_dir)
        .ok_or_else(|| "Layout directory not found".to_string())?;

    // Step 3: Construct file paths
    let old_file_path = layout_dir.join(format!("{}.kdl", safe_old_name));
    let new_file_path = layout_dir.join(format!("{}.kdl", safe_new_name));

    // Step 4: Check if source file exists
    if !old_file_path.exists() {
        return Err(format!("Layout '{}' not found", safe_old_name));
    }

    rename_layout_file(&old_file_path, &new_file_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "Layout '{}' already exists. Cannot rename '{}' to existing layout name.",
                safe_new_name, safe_old_name
            )
        } else {
            format!("Failed to rename layout file: {error}")
        }
    })?;
    Ok(())
}

fn rename_layout_file(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use nix::fcntl::{renameat2, RenameFlags, AT_FDCWD};
        renameat2(AT_FDCWD, from, AT_FDCWD, to, RenameFlags::RENAME_NOREPLACE)
            .map_err(std::io::Error::from)
    }
    #[cfg(any(target_os = "macos", all(target_os = "linux", not(target_env = "gnu"))))]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let from = CString::new(from.as_os_str().as_bytes())?;
        let to = CString::new(to.as_os_str().as_bytes())?;
        // Both paths are valid NUL-terminated strings and remain alive for the syscall.
        #[cfg(target_os = "macos")]
        let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (from, to);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic layout rename without replacement is unavailable on this platform",
        ))
    }
}

pub(super) fn edit_layout(
    env: &mut PluginEnv,
    layout_name: String,
    context: BTreeMap<String, String>,
) {
    let err_context = || format!("Failed to edit layout '{}'", layout_name);

    let response = try_edit_layout(env, &layout_name, context)
        .map(|_| EditLayoutResponse::Ok(()))
        .unwrap_or_else(EditLayoutResponse::Err);

    let protobuf_response: ProtobufEditLayoutResponse = response.into();
    let serialized = protobuf_response.encode_to_vec();
    write_response(env, &serialized)
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn try_edit_layout(
    env: &PluginEnv,
    layout_name: &str,
    context: BTreeMap<String, String>,
) -> Result<(), String> {
    // Sanitize the layout name to prevent directory traversal
    let safe_name =
        sanitize_layout_name(layout_name).map_err(|e| format!("Invalid layout name: {}", e))?;

    // Get the layout directory from PluginEnv
    let layout_dir = env
        .layout_dir
        .clone()
        .or_else(default_layout_dir)
        .ok_or_else(|| "Layout directory not found".to_string())?;

    // Construct the full file path
    let file_path = layout_dir.join(format!("{}.kdl", safe_name));

    // Create FileToOpen for the layout file
    let file_to_open = FileToOpen {
        path: file_path,
        line_number: None,
        cwd: Some(layout_dir.clone()),
    };

    // Create an Action::EditFile
    let action = Action::EditFile {
        payload: OpenFilePayload::new(
            file_to_open.path,
            file_to_open.line_number,
            file_to_open.cwd,
        )
        .with_originating_plugin(env.originating_plugin(context)),
        direction: None,
        floating: false,
        in_place: !env.background,
        close_replaced_pane: false,
        start_suppressed: false,
        placement: NewPanePlacement::Floating(None),
        near_current_pane: !env.background,
        no_focus: false,
        tab_id: None,
    };
    // Route the action - this is fallible
    route_action(
        action,
        env.client_id,
        None,
        Some(PaneId::Plugin(env.plugin_id)),
        env.senders.clone(),
        env.default_shell.clone(),
        None,
        env.default_mode,
        None,
        false,
        false, // is_client_input
    )
    .map(|_| ())
    .map_err(|e| format!("Failed to route edit action: {:?}", e))
}

/// Sanitize layout name to prevent path traversal and invalid filenames
/// Returns the sanitized name or an error message
fn sanitize_layout_name(name: &str) -> Result<String, String> {
    // Check for empty name
    if name.is_empty() {
        return Err("Layout name cannot be empty".to_string());
    }

    // Check for path separators (prevent directory traversal)
    if name.contains('/') || name.contains('\\') {
        return Err("Layout name cannot contain path separators".to_string());
    }

    // Check for parent directory references
    if name.contains("..") {
        return Err("Layout name cannot contain '..'".to_string());
    }

    // Check length (most filesystems have 255 char limit, leave room for .kdl)
    if name.len() > 250 {
        return Err("Layout name too long (max 250 characters)".to_string());
    }

    // Only allow alphanumeric, hyphens, underscores, and spaces
    let valid_chars = name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ');

    if !valid_chars {
        return Err(
            "Layout name can only contain letters, numbers, hyphens, underscores, and spaces"
                .to_string(),
        );
    }

    Ok(name.to_string())
}

pub(super) fn override_layout(
    env: &mut PluginEnv,
    layout_info: LayoutInfo,
    retain_existing_terminal_panes: bool,
    retain_existing_plugin_panes: bool,
    apply_only_to_active_tab: bool,
    context: BTreeMap<String, String>,
) -> Result<()> {
    let layout = layout_from_plugin_info(env, layout_info)?;

    // Convert all tabs to Vec<TabLayoutInfo>
    let tabs: Vec<TabLayoutInfo> = layout
        .tabs
        .iter()
        .enumerate()
        .map(|(index, (tab_name, tiled, floating))| TabLayoutInfo {
            tab_index: TabPosition::new(index),
            tab_name: tab_name.clone(),
            tiled_layout: tiled.clone(),
            floating_layouts: floating.clone(),
            swap_tiled_layouts: Some(layout.swap_tiled_layouts.clone()),
            swap_floating_layouts: Some(layout.swap_floating_layouts.clone()),
        })
        .collect();

    // If no tabs, create default tab
    let tabs = if tabs.is_empty() {
        let (tiled, floating) = layout.new_tab();
        vec![TabLayoutInfo {
            tab_index: TabPosition::new(0),
            tab_name: None,
            tiled_layout: tiled,
            floating_layouts: floating,
            swap_tiled_layouts: Some(layout.swap_tiled_layouts),
            swap_floating_layouts: Some(layout.swap_floating_layouts),
        }]
    } else {
        tabs
    };

    let action = Action::OverrideLayout {
        tabs,
        retain_existing_terminal_panes,
        retain_existing_plugin_panes,
        apply_only_to_active_tab,
    };
    run_action(env, action, context);
    Ok(())
}
