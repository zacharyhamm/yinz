//! Turning a `ClientInfo` into the first message we send the server.
//!
//! Both entry points — the interactive `start_client` and the detached
//! `start_server_detached` — have to answer the same questions before any
//! terminal work happens: where is the session socket, does a server already
//! exist or must we fork one, which layout applies, and what does the server
//! need to know about this client. They differ only in whether a controlling
//! terminal exists to report errors on, and in the window size they claim.

use crate::os_input_output::ClientOsApi;
use crate::terminal_escapes::ClientTeardown;
use crate::{create_ipc_pipe, spawn_server_error_message, ClientInfo};
use std::path::{Path, PathBuf};
use zellij_cli::args::CliArgs;
use zellij_runtime::envs;
use zellij_runtime::host_terminal_env::host_terminal_env;
use zellij_types::pane_size::Size;
use {
    zellij_config::config::Config, zellij_ipc::ipc::ClientToServerMsg,
    zellij_types::data::LayoutInfo, zellij_types::data::LayoutMetadata,
    zellij_types::input::cli_assets::CliAssets, zellij_types::input::options::Options,
};

/// The window size reported to a server that no client is attached to yet.
/// A real size arrives with the first client that attaches.
const DETACHED_WINDOW_SIZE: Size = Size { cols: 50, rows: 50 };

/// Everything the two entry points share while working out their first
/// message to the server.
pub(crate) struct StartupContext<'a> {
    pub cli_args: &'a CliArgs,
    pub config_options: &'a Options,
    /// The size to claim for the session's window.
    pub terminal_window_size: Size,
    /// How to hand the terminal back if startup fails. `None` when there is
    /// no controlling terminal to restore, i.e. the detached path.
    pub teardown: Option<ClientTeardown>,
}

impl<'a> StartupContext<'a> {
    /// Context for a client that owns a controlling terminal.
    pub fn attached(
        cli_args: &'a CliArgs,
        config_options: &'a Options,
        terminal_window_size: Size,
        teardown: ClientTeardown,
    ) -> Self {
        Self {
            cli_args,
            config_options,
            terminal_window_size,
            teardown: Some(teardown),
        }
    }

    /// Context for spawning a server with no client attached to it.
    pub fn detached(cli_args: &'a CliArgs, config_options: &'a Options) -> Self {
        Self {
            cli_args,
            config_options,
            terminal_window_size: DETACHED_WINDOW_SIZE,
            teardown: None,
        }
    }

    /// Pick the layout to start with: an explicit one if the caller has it,
    /// otherwise `--layout` from the CLI, otherwise the configured default.
    ///
    /// `options` supplies the layout dir and default layout. It is not always
    /// `self.config_options`: when attaching, the options resolved against
    /// the target session take precedence.
    fn resolve_layout(
        &self,
        explicit: Option<LayoutInfo>,
        options: &Options,
    ) -> Option<LayoutInfo> {
        explicit.or_else(|| {
            self.cli_args
                .layout
                .as_ref()
                .and_then(|l| {
                    zellij_config::layout::layout_info_from_cli(
                        &options.layout_dir,
                        &Some(l.clone()),
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                    )
                })
                .or_else(|| {
                    zellij_config::layout::layout_info_from_config(
                        &options.layout_dir,
                        &options.default_layout,
                    )
                })
        })
    }

    fn build_cli_assets(
        &self,
        options: &Options,
        layout: Option<LayoutInfo>,
        force_run_layout_commands: bool,
        cwd: Option<PathBuf>,
    ) -> CliAssets {
        CliAssets {
            config_file_path: Config::config_file_path(
                self.cli_args.config.as_deref(),
                self.cli_args.config_dir.as_deref(),
            ),
            config_dir: self.cli_args.config_dir.clone(),
            should_ignore_config: self.cli_args.is_setup_clean(),
            configuration_options: Some(options.clone()),
            layout,
            terminal_window_size: self.terminal_window_size,
            data_dir: self.cli_args.data_dir.clone(),
            is_debug: self.cli_args.debug,
            max_panes: self.cli_args.max_panes,
            force_run_layout_commands,
            cwd,
            host_terminal_env: host_terminal_env(),
        }
    }

    /// Fork the server process for a session that does not have one yet, and
    /// Does not return if the server cannot be spawned — there is no session
    /// to attach to.
    fn spawn_server(&self, os_input: &dyn ClientOsApi, ipc_pipe: &Path) {
        if let Err(e) = os_input.spawn_server(ipc_pipe, self.cli_args.debug) {
            ClientTeardown::exit_after_startup_error(self.teardown, spawn_server_error_message(e));
        }
    }

    /// Build the message that opens the conversation with the server,
    /// spawning that server first when the session is a new one.
    ///
    /// `focus` is only meaningful when attaching to an existing session; the
    /// detached path passes `Default::default()`.
    pub fn first_message(
        &self,
        os_input: &mut Box<dyn ClientOsApi>,
        info: ClientInfo,
        focus: AttachFocus,
    ) -> (ClientToServerMsg, PathBuf) {
        match info {
            ClientInfo::Attach(name, config_options) => {
                envs::set_session_name(name.clone());
                os_input.update_session_name(name);
                let ipc_pipe = create_ipc_pipe(self.teardown);

                // Attach uses the options carried on the variant rather than
                // self.config_options, since they were resolved against the
                // session being attached to.
                let layout = if let Some(layout_string) = &self.cli_args.layout_string {
                    Some(LayoutInfo::Stringified(layout_string.clone()))
                } else {
                    self.resolve_layout(None, &config_options)
                };
                let cli_assets = self.build_cli_assets(&config_options, layout, false, None);

                (
                    ClientToServerMsg::AttachClient {
                        cli_assets,
                        tab_position_to_focus: focus.tab_position,
                        pane_to_focus: focus.pane.map(|(pane_id, is_plugin)| {
                            zellij_ipc::ipc::PaneReference { pane_id, is_plugin }
                        }),
                    },
                    ipc_pipe,
                )
            },
            ClientInfo::Watch(name, _config_options) => {
                envs::set_session_name(name.clone());
                os_input.update_session_name(name);
                let ipc_pipe = create_ipc_pipe(self.teardown);

                (
                    ClientToServerMsg::AttachWatcherClient {
                        terminal_size: self.terminal_window_size,
                    },
                    ipc_pipe,
                )
            },
            ClientInfo::Resurrect(name, path_to_layout, force_run_commands, cwd) => {
                envs::set_session_name(name.clone());
                let cli_assets = self.build_cli_assets(
                    self.config_options,
                    Some(LayoutInfo::File(
                        path_to_layout.display().to_string(),
                        LayoutMetadata::default(),
                    )),
                    force_run_commands,
                    cwd,
                );

                os_input.update_session_name(name);
                let ipc_pipe = create_ipc_pipe(self.teardown);
                self.spawn_server(&**os_input, &ipc_pipe);

                (
                    ClientToServerMsg::FirstClientConnected { cli_assets },
                    ipc_pipe,
                )
            },
            ClientInfo::New(name, layout_info, layout_cwd) => {
                envs::set_session_name(name.clone());
                let cli_assets = self.build_cli_assets(
                    self.config_options,
                    self.resolve_layout(layout_info, self.config_options),
                    false,
                    layout_cwd,
                );

                os_input.update_session_name(name);
                let ipc_pipe = create_ipc_pipe(self.teardown);
                self.spawn_server(&**os_input, &ipc_pipe);

                (
                    ClientToServerMsg::FirstClientConnected { cli_assets },
                    ipc_pipe,
                )
            },
        }
    }
}

/// What the server should focus once this client attaches. Meaningless when
/// creating a session, so it defaults to "whatever the layout says".
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct AttachFocus {
    pub tab_position: Option<usize>,
    pub pane: Option<(u32, bool)>,
}
