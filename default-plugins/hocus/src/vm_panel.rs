use crate::archive::AGENT_LIST_LOCATION;
use crate::{api, tools, State, MAX_TOOL_OUTPUT};
use hocus_proto::vms::{self, Operation, Request, ResultMessage, Snapshot};
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

pub(crate) const CONTEXT: &str = "vm_panel";
const VALIDATE: &str = "vm_panel_validate";

impl State {
    pub(crate) fn verified_vm_companion(&self) -> Option<PluginId> {
        if self.vm_panel_owner != Some(self.own_plugin_id) {
            return None;
        }
        self.agents_plugin.filter(|id| {
            self.plugin_instances
                .get(id)
                .is_some_and(|plugin| plugin.location == AGENT_LIST_LOCATION && plugin.has_pane)
        })
    }

    pub(crate) fn send_vm_panel_visibility(&self, visible: bool) {
        if let Some(id) = self.verified_vm_companion() {
            pipe_message_to_plugin(
                MessageToPlugin::new(vms::VISIBILITY)
                    .with_destination_plugin_id(id)
                    .with_payload(visible.to_string()),
            );
        }
    }

    pub(crate) fn on_vm_panel_request(&mut self, message: PipeMessage) -> bool {
        let PipeSource::Plugin(sender) = message.source else {
            return false;
        };
        // Only the companion view may use this direct command path.
        if self.verified_vm_companion() != Some(sender) {
            return false;
        }
        let Some(request) = message
            .payload
            .as_deref()
            .and_then(|value| serde_json::from_str::<Request>(value).ok())
        else {
            return false;
        };
        let error = if !self.granted {
            // The prompt, or hocus's page on a refusal, is in the conversation half beside the
            // list, which is where this points.
            Some("hocus has no permission to run commands - see the conversation")
        } else if self.vm_panel_pending.is_some() {
            // The shared text, so the panel can tell "busy" from a real failure.
            Some(vms::BUSY)
        } else {
            None
        };
        if let Some(error) = error {
            vm_reply(sender, &request, None, String::new(), error.to_owned());
            return false;
        }
        self.vm_panel_pending = Some((sender, request.clone()));
        // Every mutation is revalidated against a new provider listing before it executes.
        if let Err(error) = self.launch_vm_panel_command(&request, true) {
            self.vm_panel_pending = None;
            vm_reply(sender, &request, None, String::new(), error);
        }
        false
    }

    fn launch_vm_panel_command(&self, request: &Request, validate: bool) -> Result<(), String> {
        // Resolving this path does not require an API key or a working model configuration.
        let zellij = api::expand_home(
            self.kdl
                .get("zellij_path")
                .map(String::as_str)
                .filter(|path| !path.trim().is_empty())
                .unwrap_or(api::DEFAULT_ZELLIJ),
            &self.env,
        );
        let argv = panel_argv(request, validate, &zellij)?;
        let mut context = BTreeMap::from([(CONTEXT.to_owned(), request.request_id.to_string())]);
        if validate {
            context.insert(VALIDATE.to_owned(), "true".into());
        }
        run_command_with_output_limit(
            &argv.iter().map(String::as_str).collect::<Vec<_>>(),
            self.env
                .iter()
                .filter(|(key, _)| {
                    matches!(key.as_str(), "ZELLIJ_CONFIG_FILE" | "ZELLIJ_CONFIG_DIR")
                })
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
        let Some((sender, request)) = self.vm_panel_pending.as_ref() else {
            return false;
        };
        if context.get(CONTEXT) != Some(&request.request_id.to_string()) {
            return false;
        }
        let (sender, request) = (*sender, request.clone());
        if self.verified_vm_companion() != Some(sender) || !self.granted {
            self.vm_panel_pending = None;
            return false;
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
                                    .vm_panel_snapshot
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
                        listed.current_session = self.vm_panel_current_session.clone();
                        if error.is_empty() {
                            self.vm_panel_snapshot = listed.clone();
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
        vm_reply(sender, &request, snapshot, output, error);
        false
    }
}

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

fn panel_argv(request: &Request, validate: bool, zellij: &str) -> Result<Vec<String>, String> {
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
    let args = tools::Args::parse(
        &serde_json::json!({"vm": target.map(|target| &target.name)}).to_string(),
    )?;
    let mut argv = tools::vm_argv(
        operation.tool(),
        &args,
        zellij,
        target
            .map(|target| target.session.as_str())
            .unwrap_or_default(),
    )?;
    if operation != Operation::List {
        argv.push("--expected-identity".into());
        argv.push(serde_json::to_string(&request.target).map_err(|error| error.to_string())?);
    }
    Ok(argv)
}

fn vm_reply(
    sender: PluginId,
    request: &Request,
    snapshot: Option<Snapshot>,
    output: String,
    error: String,
) {
    let reply = ResultMessage {
        request_id: request.request_id,
        operation: request.operation,
        target: request.target.clone(),
        snapshot,
        output,
        error,
    };
    if let Ok(payload) = serde_json::to_string(&reply) {
        pipe_message_to_plugin(
            MessageToPlugin::new(vms::RESULT)
                .with_destination_plugin_id(sender)
                .with_payload(payload),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> State {
        let mut state = State {
            own_is_floating: Some(true),
            agents_plugin: Some(PluginId::new(7)),
            ..Default::default()
        };
        state.on_sessions(vec![session()]);
        state
    }
    fn session() -> SessionInfo {
        SessionInfo {
            is_current_session: true,
            plugins: BTreeMap::from([
                (
                    PluginId::new(0),
                    PluginInfo {
                        location: "zellij:hocus".into(),
                        configuration: hocus_proto::launch::overlay_configuration(),
                    },
                ),
                (
                    PluginId::new(7),
                    PluginInfo {
                        location: "zellij:hocus-agents".into(),
                        ..Default::default()
                    },
                ),
            ]),
            panes: PaneManifest {
                panes: std::collections::HashMap::from([(
                    Default::default(),
                    vec![
                        PaneInfo {
                            id: PaneId::Plugin(PluginId::new(0)),
                            is_floating: true,
                            ..Default::default()
                        },
                        PaneInfo {
                            id: PaneId::Plugin(PluginId::new(7)),
                            is_floating: true,
                            ..Default::default()
                        },
                    ],
                )]),
            },
            ..Default::default()
        }
    }
    fn message(sender: u32, id: u64) -> PipeMessage {
        PipeMessage::new(
            PipeSource::Plugin(PluginId::new(sender)),
            vms::REQUEST,
            &Some(
                serde_json::to_string(&Request {
                    request_id: id,
                    operation: Operation::List,
                    target: None,
                })
                .unwrap(),
            ),
            &None,
            true,
        )
    }
    fn announcement(sender: u32, announced: u32) -> PipeMessage {
        PipeMessage::new(
            PipeSource::Plugin(PluginId::new(sender)),
            hocus_proto::wire::AGENT_SELECTION,
            &Some(
                serde_json::to_string(&hocus_proto::wire::Selection {
                    revision: None,
                    plugin_id: PluginId::new(announced),
                    pane_id: None,
                    run_id: None,
                })
                .unwrap(),
            ),
            &None,
            true,
        )
    }

    #[test]
    fn vm_panel_announcements_are_discovery_until_the_host_verifies_the_companion() {
        let mut state = State {
            granted: true,
            own_is_floating: Some(true),
            ..Default::default()
        };
        state.pipe(announcement(8, 7));
        assert!(
            state.agents_plugin.is_none(),
            "a sender cannot announce another plugin"
        );
        state.pipe(announcement(7, 7));
        assert_eq!(state.agents_plugin, Some(PluginId::new(7)));
        state.on_vm_panel_request(message(7, 1));
        assert!(
            state.vm_panel_pending.is_none(),
            "discovery grants no command access"
        );

        let mut manifest = session();
        manifest
            .plugins
            .get_mut(&PluginId::new(7))
            .unwrap()
            .location = "file:/tmp/hocus-agents.wasm".into();
        state.on_sessions(vec![manifest]);
        state.on_vm_panel_request(message(7, 2));
        assert!(
            state.vm_panel_pending.is_none(),
            "only the builtin companion is trusted"
        );

        state.on_sessions(vec![session()]);
        state.pipe(announcement(8, 7));
        state.on_vm_panel_request(message(8, 3));
        assert!(state.vm_panel_pending.is_none());
        state.on_vm_panel_request(message(7, 4));
        assert!(state.vm_panel_pending.is_some());
    }

    #[test]
    fn vm_panel_removal_revokes_access_and_reload_requires_a_fresh_manifest() {
        let mut state = state();
        state.granted = true;
        state.on_vm_panel_request(message(7, 1));
        let mut manifest = session();
        manifest.plugins.remove(&PluginId::new(7));
        state.on_sessions(vec![manifest.clone()]);
        assert!(state.vm_panel_pending.is_none());
        state.on_vm_panel_request(message(7, 2));
        assert!(state.vm_panel_pending.is_none());
        state.pipe(announcement(9, 9));
        state.on_vm_panel_request(message(9, 3));
        assert!(state.vm_panel_pending.is_none());

        manifest.plugins.insert(
            PluginId::new(9),
            session().plugins[&PluginId::new(7)].clone(),
        );
        manifest
            .panes
            .panes
            .values_mut()
            .flatten()
            .find(|pane| pane.id == PaneId::Plugin(PluginId::new(7)))
            .unwrap()
            .id = PaneId::Plugin(PluginId::new(9));
        state.on_sessions(vec![manifest]);
        state.on_vm_panel_request(message(7, 4));
        assert!(state.vm_panel_pending.is_none());
        state.on_vm_panel_request(message(9, 5));
        assert_eq!(state.vm_panel_pending.as_ref().unwrap().1.request_id, 5);
        state.on_sessions(vec![]);
        assert!(state.vm_panel_pending.is_none());
        assert!(state.verified_vm_companion().is_none());
    }
    #[test]
    fn vm_panel_argv_keeps_the_original_identity_for_every_guarded_command() {
        let expected: vms::Identity = serde_json::from_str(r#"{"session":"work","name":"review","vm_id":"/vm/socket","generation":"42:100","worktree":"/repo with spaces"}"#).unwrap();
        for operation in [Operation::Close, Operation::Sync, Operation::Changes] {
            let request = Request {
                request_id: 1,
                operation,
                target: Some(expected.clone()),
            };
            assert_eq!(
                panel_argv(&request, true, "/zellij").unwrap(),
                ["/zellij", "firecracker", "list", "--json"]
            );
            let argv = panel_argv(&request, false, "/zellij").unwrap();
            assert_eq!(argv[6], "--expected-identity");
            assert_eq!(
                serde_json::from_str::<vms::Identity>(&argv[7]).unwrap(),
                expected
            );
            let mut incomplete = request;
            incomplete.target.as_mut().unwrap().generation = None;
            assert!(panel_argv(&incomplete, true, "/zellij").is_err());
            assert!(panel_argv(&incomplete, false, "/zellij").is_err());
        }
    }

    #[test]
    fn vm_panel_commands_require_the_companion_and_permission_and_bypass_the_chat() {
        let mut state = state();
        state.on_vm_panel_request(message(7, 1));
        assert!(state.vm_panel_pending.is_none());
        state.granted = true;
        state.on_vm_panel_request(message(8, 1));
        assert!(state.vm_panel_pending.is_none());
        state.on_vm_panel_request(message(7, 1));
        assert_eq!(state.vm_panel_pending.as_ref().unwrap().1.request_id, 1);
        state.on_vm_panel_request(message(7, 2));
        assert_eq!(state.vm_panel_pending.as_ref().unwrap().1.request_id, 1);
        assert!(state.pending.is_empty());
        assert!(state.messages.is_empty());
    }
    #[test]
    fn vm_panel_errors_and_stale_replies_preserve_the_last_success() {
        let mut state = state();
        state.granted = true;
        state.vm_panel_snapshot =
            serde_json::from_str(r#"{"vms":[{"session":"s","name":"api","state":"running"}]}"#)
                .unwrap();
        state.on_vm_panel_request(message(7, 1));
        let mut context = BTreeMap::from([
            (CONTEXT.into(), "2".into()),
            (VALIDATE.into(), "true".into()),
        ]);
        state.on_vm_panel_result(Some(0), b"{\"vms\":[]}", b"", &context);
        assert!(state.vm_panel_pending.is_some());
        context.insert(CONTEXT.into(), "1".into());
        state.on_vm_panel_result(Some(1), b"", b"provider failed", &context);
        assert!(state.vm_panel_pending.is_none());
        assert_eq!(state.vm_panel_snapshot.vms.len(), 1);
    }
    /// The provider reports "not configured" on stderr alone, with an exit status of 1. The
    /// panel's divider is one row, so the reason has to come first and whole: no leading line
    /// break from an empty stdout, and no Rust `Some(..)` for the status.
    #[test]
    fn a_failed_vm_command_reads_as_a_sentence() {
        let output = command_output(b"", b"Error: Firecracker is not configured\n");
        assert_eq!(output, "Error: Firecracker is not configured");
        let error = failure(Some(1), &output);
        assert_eq!(
            error,
            "VM command failed (exit 1): Error: Firecracker is not configured"
        );
        assert!(!error.contains("Some("));

        assert_eq!(failure(None, ""), "VM command failed (no exit status)");
        assert_eq!(command_output(b"listed\n", b"warned\n"), "listed\nwarned");
        assert_eq!(command_output(b" \n", b""), "");
    }

    #[test]
    fn vm_panel_revalidates_the_full_lease_before_a_mutation() {
        let mut state = state();
        state.granted = true;
        let snapshot: Snapshot = serde_json::from_str(
            r#"{"vms":[{"session":"s","name":"api","state":"running","vm_id":"old"}]}"#,
        )
        .unwrap();
        state.vm_panel_pending = Some((
            PluginId::new(7),
            Request {
                request_id: 4,
                operation: Operation::Close,
                target: Some(snapshot.vms[0].identity()),
            },
        ));
        let context = BTreeMap::from([
            (CONTEXT.into(), "4".into()),
            (VALIDATE.into(), "true".into()),
        ]);
        state.on_vm_panel_result(
            Some(0),
            br#"{"vms":[{"session":"s","name":"api","state":"running","vm_id":"replacement"}]}"#,
            b"",
            &context,
        );
        assert!(
            state.vm_panel_pending.is_none(),
            "the replacement VM must not reach close"
        );
        assert!(state.pending.is_empty());
    }
}
