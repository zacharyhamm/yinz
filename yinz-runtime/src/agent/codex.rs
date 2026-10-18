use std::path::PathBuf;

#[derive(Default)]
pub struct BridgeOptions {
    pub cd: Option<PathBuf>,
    pub project_overrides: Vec<String>,
}

/// Only known, unambiguous UI arguments may acquire an implicit --remote endpoint.
pub fn bridge_options(args: &[String]) -> Option<BridgeOptions> {
    let mut options = BridgeOptions::default();
    let mut args = args.iter().map(String::as_str);
    let mut first_positional = true;
    let mut resume_or_fork = false;
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') {
            let (option, inline_value) = if arg.starts_with("--") {
                arg.split_once('=')
                    .map_or((arg, None), |(option, value)| (option, Some(value)))
            } else if matches!(
                arg.as_bytes().get(1),
                Some(b'c' | b'm' | b'p' | b's' | b'C' | b'a')
            ) {
                (
                    &arg[..2],
                    (arg.len() > 2).then(|| arg[2..].trim_start_matches('=')),
                )
            } else {
                return None;
            };
            match option {
                "--config" | "-c" | "--model" | "-m" | "--profile" | "-p" | "--sandbox" | "-s"
                | "--cd" | "-C" | "--ask-for-approval" | "-a" | "--enable" | "--disable"
                | "--local-provider" => {
                    let value = inline_value.or_else(|| args.next());
                    if value.is_none_or(|value| value.is_empty() || value.starts_with('-')) {
                        return None;
                    }
                    if matches!(option, "--config" | "-c")
                        && value.and_then(|value| value.split_once('=').map(|(key, _)| key.trim()))
                            == Some("sandbox_workspace_write.writable_roots")
                    {
                        return None;
                    }
                    if matches!(option, "--config" | "-c") {
                        if let Some(value) = value.filter(|value| {
                            value.split_once('=').is_some_and(|(key, _)| {
                                key.trim() == "projects" || key.trim().starts_with("projects.")
                            })
                        }) {
                            options.project_overrides.push(value.to_owned());
                        }
                    }
                    if matches!(option, "--cd" | "-C") {
                        options.cd = value.map(PathBuf::from);
                    }
                },
                "--oss"
                | "--full-auto"
                | "--approve-for-me"
                | "--dangerously-bypass-approvals-and-sandbox"
                | "--dangerously-bypass-hook-trust"
                | "--search"
                | "--no-alt-screen"
                | "--strict-config"
                    if inline_value.is_none() => {},
                "--last" | "--all" | "--include-non-interactive"
                    if resume_or_fork && inline_value.is_none() => {},
                // Includes --add-dir (unsupported with --remote), explicit endpoints,
                // standalone launches, help, and variadic --image arguments.
                _ => return None,
            }
        } else if first_positional {
            first_positional = false;
            match arg {
                "resume" | "fork" => resume_or_fork = true,
                "agents"
                | "exec"
                | "e"
                | "review"
                | "login"
                | "logout"
                | "mcp"
                | "plugin"
                | "app-server"
                | "remote-control"
                | "completion"
                | "update"
                | "doctor"
                | "sandbox"
                | "debug"
                | "apply"
                | "a"
                | "queue"
                | "archive"
                | "delete"
                | "migrate-rollouts"
                | "unarchive"
                | "features"
                | "exec-server"
                | "execpolicy"
                | "tcp-tunnel"
                | "responses-api-proxy"
                | "stdio-to-uds"
                | "cloud"
                | "help" => return None,
                _ => {},
            }
        }
    }
    Some(options)
}
