//! CLI attachment to project-owned Swamp runs. Execution belongs to a supervised pane.
use std::path::PathBuf;
use std::process::{Command, Stdio};
use zellij_cli::args::{CliArgs, Workflow};
use zellij_runtime::errors::prelude::*;

pub(crate) fn run(command: Workflow, opts: &CliArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("cannot read working directory")?;
    let snapshot = zellij_config::project_config::access(&cwd, None).map_err(|e| anyhow!(e))?;
    if let Some(error) = snapshot.error {
        return Err(anyhow!(error));
    }
    let binary = std::env::current_exe().context("cannot resolve Zellij executable")?;
    let session = opts.session.clone().or_else(|| {
        zellij_runtime::envs::get_session_name()
            .ok()
            .map(|name| name.into_string())
    });
    match command {
        Workflow::Run {
            name,
            launch,
            input,
            no_view,
        } => {
            let launch = launch.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let inputs: serde_json::Value =
                serde_json::from_str(&input).context("--input must be a JSON object")?;
            if !inputs.is_object() {
                return Err(anyhow!("--input must be a JSON object"));
            }
            let request = serde_json::json!({"method":"workflow_run", "work":launch, "key":"launch", "workflow":name, "directory":cwd,"inputs":inputs});
            // Print before dispatch so an interrupted caller can recover by token.
            println!("Launch: {launch}");
            let result = pipe(
                &binary,
                session.as_deref(),
                "zellij:swamp-bridge",
                "swamp",
                &request,
            )?;
            println!("{result}");
            if !no_view {
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
                let result = Command::new("swamp")
                    .current_dir(&snapshot.root)
                    .args(["workflow", "history", "get", &run, "--json"])
                    .status()
                    .context("cannot read Swamp run")?;
                if !result.success() {
                    return Err(anyhow!("Swamp could not read this run"));
                }
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
            let status = Command::new("swamp")
                .current_dir(&snapshot.root)
                .args([
                    "model",
                    "method",
                    "run",
                    &model,
                    "submit",
                    "--input",
                    &input.to_string(),
                    "--json",
                ])
                .status()
                .context("cannot submit workflow answer")?;
            if !status.success() {
                return Err(anyhow!(
                    "Answer was not delivered; inspect the method report before recovery"
                ));
            }
        },
        Workflow::Cancel { run } => {
            let request = serde_json::json!({"method":"workflow_cancel", "directory":snapshot.root, "run":run});
            println!(
                "{}",
                pipe(
                    &binary,
                    session.as_deref(),
                    "zellij:swamp-bridge",
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
            "zellij:swamp-workflow",
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
        destination.parse::<zellij_types::pane_id::PaneId>(),
        Ok(zellij_types::pane_id::PaneId::Plugin(_))
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
        let binary = dir.path().join("zellij");
        zellij_test_support::write_warm_script(
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
