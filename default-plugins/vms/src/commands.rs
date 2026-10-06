use crate::State;
const MAX_TOOL_OUTPUT: usize = 64_000;
use agent_proto::vms::{self, Operation, Request, ResultMessage, Snapshot};
use std::collections::BTreeMap;
use yinz_tile::prelude::*;

pub(crate) const CONTEXT: &str = "vm_panel";
const VALIDATE: &str = "vm_panel_validate";

impl State {
    pub(super) fn execute_vm_request(&mut self, request: Request) {
        if self.vm_panel_pending.is_some() {
            self.accept_vm_result(ResultMessage {
                request_id: request.request_id,
                operation: request.operation,
                target: request.target,
                snapshot: None,
                output: String::new(),
                error: vms::BUSY.into(),
            });
            return;
        }
        if !self.granted {
            self.accept_vm_result(ResultMessage {
                request_id: request.request_id,
                operation: request.operation,
                target: request.target,
                snapshot: None,
                output: String::new(),
                error: "Permission to run commands is required".into(),
            });
            return;
        }
        self.vm_panel_pending = Some(request.clone());
        if let Err(error) = self.launch_vm_panel_command(&request, true) {
            self.vm_panel_pending = None;
            self.accept_vm_result(ResultMessage {
                request_id: request.request_id,
                operation: request.operation,
                target: request.target,
                snapshot: None,
                output: String::new(),
                error,
            });
        }
    }

    fn launch_vm_panel_command(&self, request: &Request, validate: bool) -> Result<(), String> {
        // Resolving this path does not require an API key or a working model configuration.
        let yinz = self
            .configuration
            .get("yinz_path")
            .filter(|path| !path.trim().is_empty())
            .cloned()
            .or_else(|| self.env.get("YINZ_BIN").cloned())
            .unwrap_or_else(|| "yinz".into());
        let yinz = expand_home(&yinz, &self.env);
        let argv = panel_argv(request, validate, &yinz)?;
        let mut context = BTreeMap::from([(CONTEXT.to_owned(), request.request_id.to_string())]);
        if validate {
            context.insert(VALIDATE.to_owned(), "true".into());
        }
        run_command_with_output_limit(
            &argv.iter().map(String::as_str).collect::<Vec<_>>(),
            self.env
                .iter()
                .filter(|(key, _)| matches!(key.as_str(), "YINZ_CONFIG_FILE" | "YINZ_CONFIG_DIR"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            ".".into(),
            context,
            MAX_TOOL_OUTPUT,
        );
        Ok(())
    }

    pub(crate) fn on_vm_panel_result(
        &mut self,
        exit: Option<i32>,
        stdout: &[u8],
        stderr: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        let Some(request) = self.vm_panel_pending.as_ref() else {
            return false;
        };
        if context.get(CONTEXT) != Some(&request.request_id.to_string()) {
            return false;
        }
        let request = request.clone();
        if !self.granted {
            self.vm_panel_pending = None;
            return self.accept_vm_result(ResultMessage {
                request_id: request.request_id,
                operation: request.operation,
                target: request.target,
                snapshot: None,
                output: String::new(),
                error: "Permission to run commands was revoked".into(),
            });
        }
        let mut output = command_output(stdout, stderr);
        if exit == Some(0) && !context.contains_key(VALIDATE) {
            match request.operation {
                Operation::Changes => {
                    output = format!("Guest paths changed since sync: {}", output.trim())
                },
                Operation::Sync => {
                    output = match output.is_empty() {
                        true => "Worktree sync completed.".to_owned(),
                        false => format!("Worktree sync completed.\n{output}"),
                    }
                },
                _ => {},
            }
        }
        let mut error = if exit == Some(0) {
            String::new()
        } else {
            failure(exit, &output)
        };
        let mut snapshot = None;
        if error.is_empty() && context.contains_key(VALIDATE) {
            match serde_json::from_slice::<Snapshot>(stdout) {
                Ok(mut listed) => {
                    if request.operation == Operation::List {
                        // One baseline per physical VM, reused on all of its session leases.
                        for lease in &mut listed.vms {
                            if let Some(metrics) = &lease.entry.metrics {
                                lease.cpu_percent = self
                                    .vms
                                    .snapshot
                                    .vms
                                    .iter()
                                    .find(|old| {
                                        lease.entry.vm_id.is_some()
                                            && old.entry.vm_id == lease.entry.vm_id
                                    })
                                    .and_then(|old| old.entry.metrics.as_ref())
                                    .and_then(|old| vms::cpu_percent(old, metrics));
                            }
                        }
                        listed.current_session = self.current_session.clone();
                        if error.is_empty() {
                            snapshot = Some(listed);
                        }
                    } else if !listed
                        .vms
                        .iter()
                        .any(|lease| Some(lease.identity()) == request.target)
                    {
                        error = "The selected VM lease changed or disappeared; refresh and confirm again".into();
                    } else {
                        match self.launch_vm_panel_command(&request, false) {
                            Ok(()) => return false,
                            Err(detail) => error = detail,
                        }
                    }
                },
                Err(detail) => error = format!("Invalid or truncated VM listing: {detail}"),
            }
        }
        self.vm_panel_pending = None;
        self.accept_vm_result(ResultMessage {
            request_id: request.request_id,
            operation: request.operation,
            target: request.target,
            snapshot,
            output,
            error,
        })
    }
}

fn expand_home(path: &str, env: &BTreeMap<String, String>) -> String {
    match path
        .strip_prefix("~/")
        .zip(agent_proto::llm::non_empty(env, "HOME"))
    {
        Some((rest, home)) => format!("{}/{rest}", home.trim_end_matches('/')),
        None => path.to_owned(),
    }
}

#[cfg(test)]
#[path = "unit/commands.rs"]
mod home_path_tests;

/// What a VM command printed, stdout then stderr, as one text with no blank edges.
///
/// A stream that printed nothing is left out rather than joined as an empty line: the provider
/// reports most failures on stderr alone, and a leading newline is what the panel's one-row
/// divider would break on, pushing the reason into the details below it.
fn command_output(stdout: &[u8], stderr: &[u8]) -> String {
    [stdout, stderr]
        .into_iter()
        .map(String::from_utf8_lossy)
        .filter(|text| !text.trim().is_empty())
        .map(|text| text.trim().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The error a failed VM command is reported with: the exit status in words, then what it
/// printed, which is where the reason is.
fn failure(exit: Option<i32>, output: &str) -> String {
    let status = exit.map_or_else(
        || "no exit status".to_owned(),
        |code| format!("exit {code}"),
    );
    match output.trim() {
        "" => format!("VM command failed ({status})"),
        output => format!("VM command failed ({status}): {output}"),
    }
}

fn panel_argv(request: &Request, validate: bool, yinz: &str) -> Result<Vec<String>, String> {
    let operation = if validate {
        Operation::List
    } else {
        request.operation
    };
    let target = request.target.as_ref();
    if request.operation != Operation::List {
        target
            .ok_or("VM and session identity are required")?
            .validate()?;
    }
    let mut argv = vec![yinz.into(), "firecracker".into()];
    if operation == Operation::List {
        argv.extend(["list".into(), "--json".into()]);
    } else {
        let target = target.ok_or("VM and session identity are required")?;
        argv.extend([
            match operation {
                Operation::Changes => "changes",
                Operation::Sync => "sync",
                Operation::Close => "close",
                Operation::List => "list",
            }
            .into(),
            target.name.clone(),
            "--session".into(),
            target.session.clone(),
        ]);
    }
    if operation != Operation::List {
        argv.push("--expected-identity".into());
        argv.push(serde_json::to_string(&request.target).map_err(|error| error.to_string())?);
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mutations_carry_the_full_original_lease() {
        let target: vms::Identity = serde_json::from_str(r#"{"session":"work","name":"api","vm_id":"/vm/socket","generation":"42:100","worktree":"/repo with spaces"}"#).unwrap();
        for operation in [Operation::Close, Operation::Sync, Operation::Changes] {
            let request = Request {
                request_id: 1,
                operation,
                target: Some(target.clone()),
            };
            assert_eq!(
                panel_argv(&request, true, "/yinz").unwrap(),
                ["/yinz", "firecracker", "list", "--json"]
            );
            let argv = panel_argv(&request, false, "/yinz").unwrap();
            assert_eq!(argv[6], "--expected-identity");
            assert_eq!(
                serde_json::from_str::<vms::Identity>(&argv[7]).unwrap(),
                target
            );
        }
    }
}
