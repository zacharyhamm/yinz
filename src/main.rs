use yinz_runtime::errors::prelude::*;
mod agent;
mod agent_hooks;
mod agent_report;
mod commands;
mod portable_skills;
mod remote_session;
mod repl;
mod session_menu;
#[cfg(test)]
mod tests;

#[cfg(target_os = "windows")]
compile_error!("Yinz does not support Windows");

use clap::Parser;
use yinz_runtime::logging::*;
use {
    yinz_cli::args::CliAction, yinz_cli::args::CliArgs, yinz_cli::args::Command,
    yinz_cli::args::Sessions, yinz_cli::setup::Setup,
    yinz_runtime::consts::create_config_and_cache_folders, yinz_types::data::UnblockCondition,
};

fn main() {
    let process_started = std::time::Instant::now();
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new(yinz_steel::worker::ARG))
    {
        if let Err(error) = yinz_steel::worker::run_worker() {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some(yinz_config::project_config::bubblewrap::RUN_FLAG)
    {
        let args: Vec<String> = std::env::args().skip(2).collect();
        let result = (|| {
            let [root, separator, command @ ..] = args.as_slice() else {
                anyhow::bail!("invalid Bubblewrap invocation");
            };
            anyhow::ensure!(separator == "--", "missing Bubblewrap command separator");
            yinz_config::project_config::bubblewrap::Environment::at_root(std::path::Path::new(
                root,
            ))
            .run(command)
        })();
        if let Err(error) = result {
            eprintln!("Error: {error:#}");
        }
        std::process::exit(1);
    }
    if std::env::args().nth(1).as_deref() == Some(yinz_process::claude_turn::OBSERVER_FLAG) {
        if yinz_process::claude_turn::run().is_err() {
            std::process::exit(1);
        }
        return;
    }
    yinz_server::run_compilation_worker();
    yinz_server::plugin_testing::run_worker();
    if std::env::args().nth(1).as_deref() == Some("--plugin-test-server") {
        yinz_runtime::logging::configure_logger();
        let socket: std::path::PathBuf = std::env::args_os()
            .nth(2)
            .expect("test server socket")
            .into();
        let os = yinz_server_os::get_server_os_input().expect("test server OS");
        let owner_socket = socket.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let _ = std::io::stdin().read(&mut [0]);
            loop {
                if let Ok(stream) = yinz_ipc::consts::ipc_connect(&owner_socket) {
                    let _ = yinz_ipc::ipc::IpcSenderWithContext::<
                        yinz_ipc::ipc::ClientToServerMsg,
                    >::new(stream)
                    .send_client_msg(yinz_ipc::ipc::ClientToServerMsg::KillSession);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
        yinz_server::start_server_impl(Box::new(os), socket, true);
        return;
    }
    // A pty-host supervisor is spawned once per pane, so it branches before the
    // logger, the cache-folder setup and the clap builder — none of which it
    // needs, and all of which it would otherwise pay for on every pane. It must
    // also close the descriptors it inherited from the server before anything
    // opens a new one, so it sets up its own logging afterwards.
    #[cfg(unix)]
    if let Some(label) = yinz_server::pty_host::pty_host_invocation() {
        yinz_server::pty_host::run_pty_host(label);
    }

    // A broker is reached over ssh, which runs a *login-less* command with a
    // minimal environment, so it branches here for the same reasons as the
    // supervisor. `ensure` must also keep stdout clean — its one JSON line is
    // the contract the connecting side parses.
    #[cfg(unix)]
    if let Some(mode) = yinz_server::pty_host::remote::broker::broker_invocation() {
        let result = match mode.as_str() {
            "ensure" => yinz_server::pty_host::remote::broker::ensure(),
            "serve" => yinz_server::pty_host::remote::broker::serve_forever(),
            "report" => yinz_server::pty_host::remote::broker::report_agent_hook(process_started),
            other => Err(yinz_runtime::errors::prelude::anyhow!(
                "unknown --pty-broker mode {other:?}, expected 'ensure', 'serve' or 'report'"
            )),
        };
        match result {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            },
        }
    }

    let opts = CliArgs::parse();
    if let Some(Command::RemoteSessions {
        client_version,
        contract_version,
    }) = &opts.command
    {
        if let Err(error) = remote_session::list(client_version, *contract_version) {
            eprintln!("Error: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(Command::RemoteSession {
        client_version,
        contract_version,
        create,
        session_name,
    }) = &opts.command
    {
        let result =
            remote_session::bootstrap(session_name, *create, client_version, *contract_version);
        if let Err(error) = result {
            eprintln!("Error: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(Command::Sessions(Sessions::Attach {
        remote: Some(host),
        session_name,
        create,
        ..
    })) = &opts.command
    {
        configure_logger();
        let result = remote_session::attach(
            &opts,
            host,
            session_name.as_deref().unwrap_or_default(),
            *create,
        );
        match result {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("Error: {error:#}");
                std::process::exit(1);
            },
        }
    }
    if let Some(Command::Repl { eval }) = &opts.command {
        if let Err(error) = repl::run(&opts, eval.as_deref()) {
            eprintln!("Error: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(Command::Action(action)) = &opts.command
        && matches!(action.as_ref(), CliAction::AgentReport { .. })
    {
        let result = agent_report::run(*action.clone(), opts.session.clone(), process_started);
        // Errors are recorded without echoing hook input or other sensitive arguments.
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    if matches!(
        &opts.command,
        Some(Command::Agent(yinz_cli::args::Agent::CodexReport))
    ) {
        let result = std::env::current_exe()
            .context("find this yinz binary")
            .and_then(|binary| yinz_codex::report(&binary));
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    if let Some(Command::Bubblewrap(yinz_cli::args::Bubblewrap::Prepare { project })) =
        &opts.command
    {
        let result = (|| {
            let project = project
                .clone()
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)
                .context("resolve project")?;
            let config = opts
                .config_dir
                .clone()
                .or_else(yinz_runtime::home::find_default_config_dir);
            let root = yinz_config::project_config::root(&project).map_err(anyhow::Error::msg)?;
            yinz_config::project_config::bubblewrap::Environment::for_project(&root)?
                .prepare(config.as_deref())
        })();
        match result {
            Ok(()) => std::process::exit(0),
            Err(error) => {
                eprintln!("Error: {error:#}");
                std::process::exit(1);
            },
        }
    }
    let startup_kind = if opts.server.is_some() {
        Some("server")
    } else {
        match &opts.command {
            None | Some(Command::Options(_)) => Some("new"),
            Some(Command::Setup(setup)) if setup.clean => Some("new"),
            Some(Command::Sessions(Sessions::Attach { .. })) => Some("attach"),
            Some(Command::Sessions(Sessions::Watch { .. })) => Some("watch"),
            _ => None,
        }
    };
    let logger_started = std::time::Instant::now();
    configure_logger();
    if let Some(kind) = startup_kind {
        use std::io::IsTerminal;
        log::info!(
            "startup trace: process kind={kind} ppid={} exe={:?} version={} debug={} term={:?} stdin_tty={} stdout_tty={} pre_logger_us={} logger_us={}",
            nix::unistd::getppid(),
            std::env::current_exe().ok(),
            yinz_runtime::consts::VERSION,
            cfg!(debug_assertions),
            std::env::var_os("TERM"),
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
            logger_started.duration_since(process_started).as_micros(),
            logger_started.elapsed().as_micros(),
        );
        startup_stage("cli.create_directories", create_config_and_cache_folders);
    } else {
        create_config_and_cache_folders();
    }

    if let Some(Command::Agent(command)) = opts.command.clone() {
        match agent::run(command, &opts) {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("Error: {error:#}");
                std::process::exit(1);
            },
        }
    }

    // Talks only to supervisor sockets, so it runs without any
    // client or server — the whole point is that it works when the session is
    // already gone.
    // `adopt` is the exception: unlike list/kill it has to reach a running
    // session, so it goes through the ordinary CLI-action path below rather than
    // exiting here.
    #[cfg(unix)]
    if let Some(Command::PtyHosts(pty_hosts)) = opts.command.clone()
        && !matches!(pty_hosts, yinz_cli::args::PtyHosts::Adopt { .. })
    {
        if let Err(e) = commands::pty_hosts(pty_hosts) {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    // Local file work only - a hook is installed into Claude Code's settings, not
    // into a session - so it branches here beside the other two commands that need
    // neither a client nor a server.
    if let Some(Command::Hooks(hooks)) = opts.command.clone() {
        if let Err(e) = agent_hooks::run(
            hooks,
            &opts
                .data_dir
                .clone()
                .unwrap_or_else(yinz_runtime::home::get_default_data_dir),
        ) {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    if let Some(Command::Skills(command)) = opts.command.clone() {
        if let Err(error) = portable_skills::run(command) {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }
    if let Some(Command::Data(command)) = opts.command.clone() {
        if let Err(error) = commands::data(command, &opts) {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    // Plugin authoring: local file work, plus one action sent to a session for `reload`.
    if let Some(Command::Plugins(plugins)) = opts.command.clone() {
        if let Err(e) = commands::plugins(plugins, &opts) {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    {
        let config = if startup_kind.is_some() {
            startup_stage("cli.initial_config", || yinz_cli::setup::load_config(&opts))
        } else {
            yinz_cli::setup::load_config(&opts)
        }
        .ok();
        if let Some(Command::Action(cli_action)) = opts.command {
            commands::send_action_to_session(*cli_action, opts.session, config);
            std::process::exit(0);
        }
        #[cfg(unix)]
        if let Some(Command::PtyHosts(pty_hosts)) = opts.command.clone() {
            commands::pty_host_adopt(pty_hosts, opts.session, config);
            std::process::exit(0);
        }
        if let Some(Command::Subscribe(subscribe_cli)) = opts.command {
            commands::subscribe_to_session(subscribe_cli, opts.session, config);
            std::process::exit(0);
        }
        if let Some(Command::Events(events_cli)) = opts.command {
            commands::stream_session_events(events_cli, opts.session, config);
            std::process::exit(0);
        }
        if let Some(Command::Sessions(Sessions::Run {
            command,
            direction,
            cwd,
            floating,
            in_place,
            close_replaced_pane,
            name,
            close_on_exit,
            start_suspended,
            x,
            y,
            width,
            height,
            pinned,
            overlay,
            modal,
            stacked,
            blocking,
            block_until_exit_success,
            block_until_exit_failure,
            block_until_exit,
            near_current_pane,
            no_focus,
            borderless,
            tab_id,
        })) = opts.command
        {
            let cwd = cwd.or_else(|| std::env::current_dir().ok());
            let skip_plugin_cache = false; // N/A for this action

            // Compute the unblock condition
            let unblock_condition = if block_until_exit_success {
                Some(UnblockCondition::OnExitSuccess)
            } else if block_until_exit_failure {
                Some(UnblockCondition::OnExitFailure)
            } else if block_until_exit {
                Some(UnblockCondition::OnAnyExit)
            } else {
                None
            };

            let command_cli_action = CliAction::NewPane {
                command,
                // `yinz run`/`edit` stay local for now; remote panes are
                // reached through `yinz action new-pane --remote-host`.
                remote_host: None,

                plugin: None,
                direction,
                cwd,
                floating,
                in_place,
                close_replaced_pane,
                pane_id: None,
                name,
                close_on_exit,
                start_suspended,
                configuration: None,
                configuration_json: None,
                skip_plugin_cache,
                x,
                y,
                width,
                height,
                pinned,
                overlay,
                modal,
                stacked,
                blocking,
                block_until_exit_success: false,
                block_until_exit_failure: false,
                block_until_exit: false,
                unblock_condition,
                near_current_pane,
                no_focus,
                borderless,
                tab_id,
            };
            commands::send_action_to_session(command_cli_action, opts.session, config);
            std::process::exit(0);
        }
        if let Some(Command::Sessions(Sessions::Plugin {
            url,
            floating,
            in_place,
            close_replaced_pane,
            configuration,
            configuration_json,
            skip_plugin_cache,
            x,
            y,
            width,
            height,
            pinned,
            overlay,
            modal,
            no_focus,
            borderless,
            tab_id,
        })) = opts.command
        {
            let cwd = None;
            let stacked = false;
            let blocking = false;
            let unblock_condition = None;
            let command_cli_action = CliAction::NewPane {
                command: vec![],
                remote_host: None,

                plugin: Some(url),
                direction: None,
                cwd,
                floating,
                in_place,
                close_replaced_pane,
                pane_id: None,
                name: None,
                close_on_exit: false,
                start_suspended: false,
                configuration,
                configuration_json,
                skip_plugin_cache,
                x,
                y,
                width,
                height,
                pinned,
                overlay,
                modal,
                stacked,
                blocking,
                block_until_exit_success: false,
                block_until_exit_failure: false,
                block_until_exit: false,
                unblock_condition,
                near_current_pane: false,
                no_focus,
                borderless,
                tab_id,
            };
            commands::send_action_to_session(command_cli_action, opts.session, config);
            std::process::exit(0);
        }
        if let Some(Command::Sessions(Sessions::Edit {
            file,
            direction,
            line_number,
            floating,
            in_place,
            close_replaced_pane,
            cwd,
            x,
            y,
            width,
            height,
            pinned,
            overlay,
            modal,
            near_current_pane,
            no_focus,
            borderless,
            tab_id,
        })) = opts.command
        {
            let mut file = file;
            let cwd = cwd.or_else(|| std::env::current_dir().ok());
            if file.is_relative()
                && let Some(cwd) = cwd.as_ref()
            {
                file = cwd.join(file);
            }
            let command_cli_action = CliAction::Edit {
                file,
                direction,
                line_number,
                floating,
                in_place,
                close_replaced_pane,
                cwd,
                x,
                y,
                width,
                height,
                pinned,
                overlay,
                modal,
                near_current_pane,
                no_focus,
                borderless,
                tab_id,
            };
            commands::send_action_to_session(command_cli_action, opts.session, config);
            std::process::exit(0);
        }
        if let Some(Command::Sessions(Sessions::Pipe {
            focus_plugin,
            name,
            payload,
            args,
            plugin,
            plugin_configuration,
            configuration_json,
        })) = opts.command
        {
            let command_cli_action = CliAction::Pipe {
                focus_plugin,
                name,
                payload,
                args,
                plugin,
                plugin_configuration,
                configuration_json,

                force_launch_plugin: false,
                skip_plugin_cache: false,
                floating_plugin: None,
                in_place_plugin: None,
                plugin_cwd: None,
                plugin_title: None,
            };
            commands::send_action_to_session(command_cli_action, opts.session, config);
            std::process::exit(0);
        }
    }

    if let Some(Command::Sessions(Sessions::ListSessions {
        no_formatting,
        short,
        exited,
        reverse,
    })) = opts.command
    {
        commands::list_sessions(no_formatting, short, reverse, exited);
    } else if let Some(Command::Sessions(Sessions::ListAliases)) = opts.command {
        commands::list_aliases(opts);
    } else if let Some(Command::Sessions(Sessions::Watch { ref session_name })) = opts.command {
        commands::watch_session(session_name.clone(), opts);
    } else if let Some(Command::Sessions(Sessions::KillAllSessions { yes })) = opts.command {
        commands::kill_all_sessions(yes);
    } else if let Some(Command::Sessions(Sessions::KillSession { ref target_session })) =
        opts.command
    {
        commands::kill_session(target_session);
    } else if let Some(Command::Sessions(Sessions::DeleteAllSessions { yes, force })) = opts.command
    {
        commands::delete_all_sessions(yes, force);
    } else if let Some(Command::Sessions(Sessions::DeleteSession {
        ref target_session,
        force,
    })) = opts.command
    {
        commands::delete_session(target_session, force);
    } else if let Some(path) = opts.server {
        commands::start_server(path, opts.debug, opts.server_termios);
    } else if opts.pty_host.is_some() {
        // Only reachable when `--pty-host` was not the first argument, so the
        // fast path in `main()` did not catch it. There is no control channel on
        // fd 3 in that case; say so rather than blocking on a read forever.
        eprintln!(
            "--pty-host is an internal flag: yinz spawns pane supervisors itself. \
             There is nothing to run by hand."
        );
        std::process::exit(1);
    } else if opts.layout.is_some() || opts.layout_string.is_some() {
        if opts.session.is_some() || yinz_process::pane_context::inside_pane() {
            let config = yinz_cli::setup::load_config(&opts).ok();
            let options = Setup::from_cli_args(&opts).ok().map(|r| r.2);
            let new_layout_cli_action = CliAction::NewTab {
                layout: opts.layout.clone(),
                layout_string: opts.layout_string.clone(),
                layout_dir: options.as_ref().and_then(|o| o.layout_dir.clone()),
                name: None,
                cwd: options.as_ref().and_then(|o| o.default_cwd.clone()),
                initial_command: vec![],
                initial_plugin: None,
                close_on_exit: Default::default(),
                start_suspended: Default::default(),
                block_until_exit_success: false,
                block_until_exit_failure: false,
                block_until_exit: false,
                no_focus: false,
            };
            commands::send_action_to_session(new_layout_cli_action, opts.session.clone(), config);
        } else {
            commands::start_client(opts);
        }
    } else if let Some(layout_for_new_session) = &opts.new_session_with_layout {
        let mut opts = opts.clone();
        opts.new_session_with_layout = None;
        opts.layout = Some(layout_for_new_session.clone());
        commands::start_client(opts);
    } else {
        commands::start_client(opts);
    }
}
