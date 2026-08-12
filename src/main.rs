mod commands;
#[cfg(test)]
mod tests;

#[cfg(target_os = "windows")]
compile_error!("Zellij does not support Windows");

use clap::Parser;
use zellij_utils::{
    cli::{CliAction, CliArgs, Command, Sessions},
    consts::{create_config_and_cache_folders, VERSION},
    data::UnblockCondition,
    envs,
    input::config::Config,
    logging::*,
    setup::Setup,
    shared::web_server_base_url_from_config,
};

fn main() {
    // A pty-host supervisor is spawned once per pane, so it branches before the
    // logger, the cache-folder setup and the clap builder — none of which it
    // needs, and all of which it would otherwise pay for on every pane. It must
    // also close the descriptors it inherited from the server before anything
    // opens a new one, so it sets up its own logging afterwards.
    #[cfg(unix)]
    if let Some(label) = zellij_server::pty_host::pty_host_invocation() {
        zellij_server::pty_host::run_pty_host(label);
    }

    configure_logger();
    create_config_and_cache_folders();
    let opts = CliArgs::parse();

    // Talks only to supervisor sockets, so like `recordings` it runs without any
    // client or server — the whole point is that it works when the session is
    // already gone.
    // `adopt` is the exception: unlike list/kill it has to reach a running
    // session, so it goes through the ordinary CLI-action path below rather than
    // exiting here.
    #[cfg(unix)]
    if let Some(Command::PtyHosts(pty_hosts)) = opts.command.clone() {
        if !matches!(pty_hosts, zellij_utils::cli::PtyHosts::Adopt { .. }) {
            if let Err(e) = commands::pty_hosts(pty_hosts) {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
    }

    // Handle offline recordings command before client/server startup
    if let Some(Command::Recordings(rec)) = opts.command.clone() {
        let data_dir = opts
            .data_dir
            .clone()
            .unwrap_or_else(zellij_utils::home::get_default_data_dir);
        if let Err(e) = commands::recordings(rec, data_dir) {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    {
        let config = Config::try_from(&opts).ok();
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
                skip_plugin_cache,
                x,
                y,
                width,
                height,
                pinned,
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
                global: false,
                edge: None,
                size: None,
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
            skip_plugin_cache,
            x,
            y,
            width,
            height,
            pinned,
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
                skip_plugin_cache,
                x,
                y,
                width,
                height,
                pinned,
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
                global: false,
                edge: None,
                size: None,
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
            near_current_pane,
            no_focus,
            borderless,
            tab_id,
        })) = opts.command
        {
            let mut file = file;
            let cwd = cwd.or_else(|| std::env::current_dir().ok());
            if file.is_relative() {
                if let Some(cwd) = cwd.as_ref() {
                    file = cwd.join(file);
                }
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
                near_current_pane,
                no_focus,
                borderless,
                tab_id,
            };
            commands::send_action_to_session(command_cli_action, opts.session, config);
            std::process::exit(0);
        }
        if let Some(Command::Sessions(Sessions::Pipe {
            name,
            payload,
            args,
            plugin,
            plugin_configuration,
        })) = opts.command
        {
            let command_cli_action = CliAction::Pipe {
                name,
                payload,
                args,
                plugin,
                plugin_configuration,

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
        commands::start_server(path, opts.debug);
    } else if opts.pty_host.is_some() {
        // Only reachable when `--pty-host` was not the first argument, so the
        // fast path in `main()` did not catch it. There is no control channel on
        // fd 3 in that case; say so rather than blocking on a read forever.
        eprintln!(
            "--pty-host is an internal flag: zellij spawns pane supervisors itself. \
             There is nothing to run by hand."
        );
        std::process::exit(1);
    } else if opts.layout.is_some() || opts.layout_string.is_some() {
        if let Some(session_name) = opts
            .session
            .as_ref()
            .cloned()
            .or_else(|| envs::get_session_name().ok())
        {
            let config = Config::try_from(&opts).ok();
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
            commands::send_action_to_session(new_layout_cli_action, Some(session_name), config);
        } else {
            commands::start_client(opts);
        }
    } else if let Some(layout_for_new_session) = &opts.new_session_with_layout {
        let mut opts = opts.clone();
        opts.new_session_with_layout = None;
        opts.layout = Some(layout_for_new_session.clone());
        commands::start_client(opts);
    } else if let Some(Command::Web(web_opts)) = &opts.command {
        if web_opts.get_start() {
            let daemonize = web_opts.daemonize;
            commands::start_web_server(
                opts.clone(),
                daemonize,
                web_opts.ip,
                web_opts.port,
                web_opts.cert.clone(),
                web_opts.key.clone(),
            );
        } else if web_opts.stop {
            match commands::stop_web_server() {
                Ok(()) => {
                    println!("Stopped web server.");
                },
                Err(e) => {
                    eprintln!("Failed to stop web server: {}", e);
                    std::process::exit(2)
                },
            }
        } else if web_opts.status {
            let mut config_options = commands::get_config_options_from_cli_args(&opts)
                .expect("Can't find config options");
            if let Some(ip) = web_opts.ip {
                config_options.web_server_ip = Some(ip);
            }
            if let Some(port) = web_opts.port {
                config_options.web_server_port = Some(port);
            }
            let web_server_base_url = web_server_base_url_from_config(config_options);
            match commands::web_server_status(&web_server_base_url, web_opts.timeout) {
                Ok(version) => {
                    let version = version.trim();
                    println!(
                        "Web server online with version: {}. Checked: {}",
                        version, web_server_base_url
                    );
                    if version != VERSION {
                        println!();
                        println!(
                            "Note: this version differs from the current Zellij version: {}.",
                            VERSION
                        );
                        println!("Consider stopping the server with: zellij web --stop");
                        println!("And then restarting it with: zellij web --start");
                    }
                },
                Err(_e) => {
                    println!("Web server is offline, checked: {}", web_server_base_url);
                },
            }
        } else if web_opts.create_token {
            let read_only = false;
            match commands::create_auth_token(web_opts.token_name.clone(), read_only) {
                Ok(token_and_name) => {
                    println!("Created token successfully");
                    println!();
                    println!("{}", token_and_name);
                },
                Err(e) => {
                    eprintln!("Failed to create token: {}", e);
                    std::process::exit(2)
                },
            }
        } else if web_opts.create_read_only_token {
            let read_only = true;
            match commands::create_auth_token(web_opts.token_name.clone(), read_only) {
                Ok(token_and_name) => {
                    println!("Created token successfully");
                    println!();
                    println!("{}", token_and_name);
                },
                Err(e) => {
                    eprintln!("Failed to create token: {}", e);
                    std::process::exit(2)
                },
            }
        } else if let Some(token_name_to_revoke) = &web_opts.revoke_token {
            match commands::revoke_auth_token(token_name_to_revoke) {
                Ok(revoked) => {
                    if revoked {
                        println!("Successfully revoked token.");
                    } else {
                        eprintln!("Token by that name does not exist.");
                        std::process::exit(2)
                    }
                },
                Err(e) => {
                    eprintln!("Failed to revoke token: {}", e);
                    std::process::exit(2)
                },
            }
        } else if web_opts.revoke_all_tokens {
            match commands::revoke_all_auth_tokens() {
                Ok(_) => {
                    println!("Successfully revoked all auth tokens");
                },
                Err(e) => {
                    eprintln!("Failed to revoke all auth tokens: {}", e);
                    std::process::exit(2)
                },
            }
        } else if web_opts.list_tokens {
            match commands::list_auth_tokens() {
                Ok(token_list) => {
                    for item in token_list {
                        println!("{}", item);
                    }
                },
                Err(e) => {
                    eprintln!("Failed to list tokens: {}", e);
                    std::process::exit(2)
                },
            }
        }
    } else {
        commands::start_client(opts);
    }
}
