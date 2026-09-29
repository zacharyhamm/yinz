//! CLI attachment to project-owned Swamp runs. Execution belongs to managed Swamp.
use std::path::PathBuf;
use std::process::{Command, Stdio};
use yinz_cli::args::{CliArgs, Workflow};
use yinz_runtime::errors::prelude::*;

pub(crate) fn run(command: Workflow, opts: &CliArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("cannot read working directory")?;
    let snapshot = yinz_config::project_config::access(&cwd, None).map_err(|e| anyhow!(e))?;
    if let Some(error) = snapshot.error {
        return Err(anyhow!(error));
    }
    let binary = std::env::current_exe().context("cannot resolve Yinz executable")?;
    let session = opts.session.clone().or_else(|| {
        yinz_runtime::envs::get_session_name()
            .ok()
            .map(|name| name.into_string())
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start workflow client runtime")?;
    match command {
        Workflow::Tools(operation) => {
            let request = operation
                .request(&cwd, &mut std::io::stdin())
                .map_err(anyhow::Error::msg)?;
            let session =
                crate::commands::resolve_session_for_stream(session, "operate workflows in");
            let action = yinz_cli::notes_todos::pipe_action("workflow_action", request);
            println!("{}", yinz_client::store_client::request(&session, action)?);
        },
        Workflow::Server { command } => {
            use yinz_cli::args::WorkflowServer;
            let action = match command {
                WorkflowServer::Status => "status",
                WorkflowServer::Restart => "restart",
                WorkflowServer::Stop => "stop",
            };
            println!(
                "{}",
                runtime.block_on(yinz_swamp::lifecycle(snapshot.root, action))?
            );
        },
        Workflow::Run {
            name,
            launch,
            input,
            input_file,
            json,
            no_view,
        } => {
            let launch = launch.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let input = if let Some(path) = input_file {
                crate::commands::read_source(Some(&path))?
            } else {
                input
            };
            let inputs: serde_json::Value =
                serde_json::from_str(&input).context("--input must be a JSON object")?;
            if !inputs.is_object() {
                return Err(anyhow!("--input must be a JSON object"));
            }
            let request = serde_json::json!({"method":"workflow_run", "work":launch, "key":"launch", "workflow":name, "directory":cwd,"inputs":inputs});
            // Print before dispatch so an interrupted caller can recover by token.
            if !json {
                println!("Launch: {launch}");
            }
            let result = pipe(
                &binary,
                session.as_deref(),
                "yinz:swamp-bridge",
                "swamp",
                &request,
            )
            .with_context(|| {
                format!("launch {launch}; inspect run history before repeating a mutation")
            })?;
            if json {
                println!("{}", serde_json::json!({"launch":launch,"result":result}));
            } else {
                println!("{result}");
            }
            if !no_view && !json {
                open_view(
                    &binary,
                    session.as_deref(),
                    &snapshot.root,
                    None,
                    Some(&launch),
                )?;
            }
        },
        Workflow::Show { run, json } => {
            if json {
                let session =
                    crate::commands::resolve_session_for_stream(session, "inspect workflows in");
                let action = yinz_cli::notes_todos::pipe_action(
                    "workflow_action",
                    serde_json::json!({"tool":"workflow_run_get","args":{"run":run},"directory":cwd}),
                );
                println!("{}", yinz_client::store_client::request(&session, action)?);
            } else {
                open_view(
                    &binary,
                    session.as_deref(),
                    &snapshot.root,
                    Some(&run),
                    None,
                )?;
            }
        },
        Workflow::Answer {
            run,
            model,
            question,
            answer,
            submission,
        } => {
            let input = serde_json::json!({"run":run,"question":question,"answer":answer,
                "submission":submission.unwrap_or_else(||uuid::Uuid::new_v4().to_string())});
            let request = serde_json::json!({"method":"workflow_answer","directory":snapshot.root,"model":model,"inputs":input});
            println!(
                "{}",
                pipe(
                    &binary,
                    session.as_deref(),
                    "yinz:swamp-bridge",
                    "swamp",
                    &request
                )?["result"]
            );
        },
        Workflow::Cancel { run } => {
            let request = serde_json::json!({"method":"workflow_cancel", "directory":snapshot.root, "run":run});
            println!(
                "{}",
                pipe(
                    &binary,
                    session.as_deref(),
                    "yinz:swamp-bridge",
                    "swamp",
                    &request
                )?
            );
        },
    }
    Ok(())
}
fn pipe(
    binary: &PathBuf,
    session: Option<&str>,
    plugin: &str,
    name: &str,
    request: &serde_json::Value,
) -> Result<serde_json::Value> {
    if plugin == "yinz:swamp-bridge" {
        let session = crate::commands::resolve_session_for_stream(
            session.map(str::to_owned),
            "operate workflows in",
        );
        return yinz_client::store_client::raw_request(
            &session,
            yinz_cli::notes_todos::pipe_action(name, request.clone()),
        );
    }
    let mut command = Command::new(binary);
    if let Some(session) = session {
        command.args(["--session", session]);
    }
    let output = command
        .args([
            "pipe",
            "--name",
            name,
            "--plugin",
            plugin,
            "--",
            &request.to_string(),
        ])
        .stdin(Stdio::null())
        .output()
        .context("cannot contact workflow bridge")?;
    if !output.status.success() {
        return Err(anyhow!(
            "Workflow bridge failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let reply: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("invalid workflow bridge reply")?;
    if let Some(error) = reply.get("error") {
        return Err(anyhow!("{error}"));
    }
    Ok(reply)
}
fn open_view(
    binary: &PathBuf,
    session: Option<&str>,
    root: &PathBuf,
    run: Option<&str>,
    launch: Option<&str>,
) -> Result<()> {
    let request = serde_json::json!({"directory":root,"run":run,"launch":launch});
    let mut tabs = Command::new(binary);
    if let Some(session) = session {
        tabs.args(["--session", session]);
    }
    let tabs = tabs
        .args(["action", "list-tabs", "--json"])
        .output()
        .context("cannot locate a tab for the workflow view")?;
    if !tabs.status.success() {
        return Err(anyhow!("Cannot inspect session tabs"));
    }
    let tabs: Vec<serde_json::Value> =
        serde_json::from_slice(&tabs.stdout).context("invalid session tab list")?;
    let tab = tabs
        .iter()
        .find(|tab| tab["active"] == true)
        .or_else(|| tabs.first())
        .and_then(|tab| tab["tab_id"].as_u64())
        .context("session has no tab for the workflow view")?;
    let mut command = Command::new(binary);
    if let Some(session) = session {
        command.args(["--session", session]);
    }
    let pane = command
        .args([
            "plugin",
            "--tab-id",
            &tab.to_string(),
            "--floating",
            "--width",
            "90%",
            "--height",
            "90%",
            "--",
            "yinz:swamp-workflow",
        ])
        .stdin(Stdio::null())
        .output()
        .context("cannot open workflow view pane")?;
    if !pane.status.success() {
        return Err(anyhow!(
            "Cannot open workflow view: {}",
            String::from_utf8_lossy(&pane.stderr)
        ));
    }
    let destination = std::str::from_utf8(&pane.stdout)
        .context("invalid workflow view pane ID")?
        .trim();
    if !matches!(
        destination.parse::<yinz_types::pane_id::PaneId>(),
        Ok(yinz_types::pane_id::PaneId::Plugin(_))
    ) {
        return Err(anyhow!("Invalid workflow view pane ID: {destination:?}"));
    }
    pipe(binary, session, destination, "show", &request)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_view_targets_the_launched_pane_and_rejects_invalid_output() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("yinz");
        yinz_test_support::write_warm_script(
            &binary,
            r#"cd "$(dirname "$0")"
case "$1" in
  action) echo '[{"active":true,"tab_id":1}]' ;;
  plugin) cat launch-output ;;
  pipe) printf '%s\n' "$@" > pipe-args; echo '{"attached":true}' ;;
esac
"#,
        );
        let output = dir.path().join("launch-output");
        let pipe_args = dir.path().join("pipe-args");
        for pane in ["plugin_7\n", "plugin_8\n"] {
            std::fs::write(&output, pane).unwrap();
            open_view(&binary, None, &dir.path().to_path_buf(), Some("run"), None).unwrap();
            let args = std::fs::read_to_string(&pipe_args).unwrap();
            assert!(args.contains(&format!("--plugin\n{}\n", pane.trim())));
            std::fs::remove_file(&pipe_args).unwrap();
        }
        for pane in [
            "",
            "terminal_7",
            "7",
            "plugin_bad",
            "plugin_4294967296",
            "plugin_7\nplugin_8",
            "unexpected output",
        ] {
            std::fs::write(&output, pane).unwrap();
            assert!(
                open_view(&binary, None, &dir.path().to_path_buf(), Some("run"), None).is_err()
            );
            assert!(
                !pipe_args.exists(),
                "invalid output must not dispatch a pipe"
            );
        }
    }
}
