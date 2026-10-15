use super::{decode, encode};
use agent_proto::{agents, panes, paths, wire};
use anyhow::{bail, ensure, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;

fn arg<T: DeserializeOwned>(args: &Value, index: usize) -> Result<T> {
    T::deserialize(args.get(index).unwrap_or(&Value::Null))
        .context("invalid model arguments")
        .with_context(|| format!("argument {index}"))
}
pub(super) fn pane_views(args: Value) -> Result<[Value; 3]> {
    ensure!(args.is_array(), "agents arguments must be an array");
    let manifest = arg(&args, 0)?;
    let tabs: Vec<_> = arg(&args, 1)?;
    Ok([
        encode(panes::terminal_panes(&manifest, &tabs))?,
        encode(panes::suppressed_terminal_panes(&manifest, &tabs))?,
        encode(panes::focused_pane(&manifest, &tabs))?,
    ])
}
fn owner(value: Value) -> Result<agent_proto::store::Owner> {
    Ok(agent_proto::store::Owner::new(
        decode(value["host_id"].clone())?,
        decode(value["yinz_session_name"].clone())?,
    ))
}
pub(super) fn run(op: &str, args: Value) -> Result<Value> {
    ensure!(args.is_array(), "agents arguments must be an array");
    match op {
        "NormalizePath" => encode(paths::normalize(&arg::<String>(&args, 0)?)),
        "TerminalPanes" => encode(panes::terminal_panes(
            &arg(&args, 0)?,
            &arg::<Vec<_>>(&args, 1)?,
        )),
        "SuppressedPanes" => encode(panes::suppressed_terminal_panes(
            &arg(&args, 0)?,
            &arg::<Vec<_>>(&args, 1)?,
        )),
        "FocusedPane" => encode(panes::focused_pane(
            &arg(&args, 0)?,
            &arg::<Vec<_>>(&args, 1)?,
        )),
        "ManifestPaneIds" => encode(panes::manifest_pane_ids(&arg(&args, 0)?)),
        "AgentPane" => encode(panes::agent_pane(&arg(&args, 0)?, arg(&args, 1)?)),
        "DisplayedPane" => encode(panes::displayed(arg(&args, 0)?)),
        "ValidKind" => encode(agents::valid_kind(&arg::<String>(&args, 0)?)),
        "IsShell" => encode(agents::is_shell(&arg::<String>(&args, 0)?)),
        "KindFromCommand" => encode(agents::kind_from_command(&arg::<String>(&args, 0)?)),
        "DetectKind" => encode(agents::detect_kind(
            &arg::<String>(&args, 0)?,
            arg::<Option<String>>(&args, 1)?.as_deref(),
        )),
        "Classify" => encode(agents::classify(&arg(&args, 0)?, &arg(&args, 1)?)),
        "Fingerprint" => encode(agents::fingerprint(&arg::<String>(&args, 0)?)),
        "HookFeed" => encode(agents::HookFeed::from_agent(&arg(&args, 0)?)),
        "HookPhase" => encode(agents::hook_phase(
            &arg::<String>(&args, 0)?,
            &arg(&args, 1)?,
        )),
        "HookEvent" => encode(
            serde_json::from_str::<wire::HookEvent>(&arg::<String>(&args, 0)?)
                .map_err(|error| error.to_string()),
        ),
        "ToolEntry" => {
            let tool = arg::<yinz_types::agent::tool_capture::ToolCapture>(&args, 0)?;
            encode((
                tool.event_id(),
                tool.text(),
                if tool.status == "requested" {
                    "tool_call"
                } else {
                    "tool_result"
                },
            ))
        },
        "TextSanitize" => encode(agent_proto::text::sanitize(&arg::<String>(&args, 0)?)),
        "TruncateBytes" => {
            let text = arg::<String>(&args, 0)?;
            let mut end = arg::<usize>(&args, 1)?.min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            encode(&text[..end])
        },
        "SubmittedPrompt" => encode(agents::submitted_prompt(&arg::<String>(&args, 0)?)),
        "EnvelopeQuestion" => encode(agent_proto::envelope::question(&arg::<String>(&args, 0)?)),
        "Observe" => {
            let mut activity = arg::<agents::Activity>(&args, 0)?;
            let text = arg::<Option<String>>(&args, 2)?;
            agents::observe(
                &mut activity,
                arg(&args, 1)?,
                text.as_deref()
                    .map(agents::Sample::Text)
                    .unwrap_or(agents::Sample::Unusable),
            );
            encode(activity)
        },
        "PromptTitle" => encode(agents::prompt_title(&arg::<String>(&args, 0)?)),
        "WaitingDetail" => encode(agents::waiting_detail(&arg(&args, 0)?)),
        "WaitingToast" => encode(agents::waiting_toast(
            arg(&args, 0)?,
            arg::<Option<String>>(&args, 1)?.as_deref(),
            &arg(&args, 2)?,
        )),
        "KindCell" => encode(agents::kind_cell(
            &arg::<String>(&args, 0)?,
            arg::<Option<String>>(&args, 1)?.as_deref(),
            arg(&args, 2)?,
        )),
        "SummaryTail" => encode(agents::tail(
            &arg::<String>(&args, 0)?,
            agents::SUMMARY_LINES,
            agents::SUMMARY_CHARS,
        )),
        "WorthSummarizing" => encode(agents::worth_summarizing(&arg::<String>(&args, 0)?)),
        "LocalSummary" => encode(agents::local_summary(
            &arg::<String>(&args, 0)?,
            arg(&args, 1)?,
        )),
        "ParseSummary" => encode(agents::parse_summary(
            &arg::<Vec<u8>>(&args, 0)?,
            arg(&args, 1)?,
        )),
        "StampSummary" => {
            let mut summary = arg(&args, 0)?;
            let previous = arg::<Option<agents::Summary>>(&args, 1)?;
            agents::stamp_summary(&mut summary, previous.as_ref(), arg(&args, 2)?);
            encode(summary)
        },
        "PushSummaryEntry" => {
            let mut run = arg(&args, 0)?;
            agents::push_summary_entry(&mut run, &arg::<String>(&args, 1)?, arg(&args, 2)?);
            encode(run)
        },
        "RenameTitle" => encode(agents::rename_title(&arg(&args, 0)?)),
        "SummaryToast" => encode(agents::summary_toast(
            arg(&args, 0)?,
            arg::<Option<String>>(&args, 1)?.as_deref(),
            &arg(&args, 2)?,
        )),
        "SummaryRequestBody" => json_body(agents::summary_request_body(
            &arg::<String>(&args, 0)?,
            &arg::<String>(&args, 1)?,
            arg::<Option<String>>(&args, 2)?.as_deref(),
            arg::<Option<String>>(&args, 3)?.as_deref(),
            &arg::<String>(&args, 4)?,
        )),
        "RunSummaryRequestBody" => json_body(agents::run_summary_request_body(
            &arg::<String>(&args, 0)?,
            &arg::<String>(&args, 1)?,
            arg::<Option<String>>(&args, 2)?.as_deref(),
            arg::<Option<String>>(&args, 3)?.as_deref(),
            arg::<Option<String>>(&args, 4)?.as_deref(),
            &arg::<String>(&args, 5)?,
        )),
        "Endpoint" => {
            let config = arg(&args, 0)?;
            let env = arg(&args, 1)?;
            let endpoint = if arg(&args, 2)? {
                agent_proto::llm::Endpoint::resolve_for_completion(&config, &env)
            } else {
                agent_proto::llm::Endpoint::resolve(&config, &env)
            };
            encode(endpoint.map(|endpoint| serde_json::json!({"url":endpoint.url,"model":endpoint.model,"headers":endpoint.headers()})))
        },
        "SummariesEnabled" => encode(agent_proto::llm::summaries_enabled(&arg(&args, 0)?)),
        "CompletionsEnabled" => encode(agent_proto::llm::plugin_completions_enabled(&arg(
            &args, 0,
        )?)),
        "PinnedCompletionModel" => {
            encode(agent_proto::llm::pinned_completion_model(&arg(&args, 0)?))
        },
        "CompletionSize" => encode(agent_proto::completion::within_size(&arg::<String>(
            &args, 0,
        )?)),
        "CompletionRequest" => encode(
            serde_json::from_str::<wire::CompletionRequest>(&arg::<String>(&args, 0)?)
                .map_err(|error| error.to_string()),
        ),
        "CompletionValidate" => encode(agent_proto::completion::validate(&arg(&args, 0)?)),
        "CompletionBody" => json_body(agent_proto::completion::completion_request_body(
            &arg(&args, 0)?,
            &arg::<String>(&args, 1)?,
        )),
        "CompletionParse" => encode(agent_proto::completion::parse_completion(
            arg(&args, 0)?,
            &arg::<Vec<u8>>(&args, 1)?,
            &arg::<String>(&args, 2)?,
        )),
        "CompletionError" => encode(agent_proto::completion::error_reply(
            &arg::<String>(&args, 0)?,
            &arg::<String>(&args, 1)?,
        )),
        "CompletionReplyString" => encode(
            serde_json::to_string(&arg::<wire::CompletionReply>(&args, 0)?)
                .context("serializing completion reply")?,
        ),
        "MutationParse" => encode(
            serde_json::from_str::<wire::Mutation>(&arg::<String>(&args, 0)?)
                .map_err(|error| error.to_string()),
        ),
        "SummarizeRequest" => encode(
            serde_json::from_str::<wire::SummarizeRequest>(&arg::<String>(&args, 0)?)
                .map_err(|error| error.to_string()),
        ),
        "HookDetail" => encode(agents::hook_detail_for(
            &arg::<String>(&args, 0)?,
            &arg(&args, 1)?,
        )),
        "RegisteredAgent" => encode(agents::registered_agent(
            &arg::<String>(&args, 0)?,
            arg(&args, 1)?,
        )),
        "StoredRunId" => encode(agents::stored_run_id(&arg(&args, 0)?)),
        "RunId" => encode(agents::run_id(
            &arg::<String>(&args, 0)?,
            arg(&args, 1)?,
            &arg::<String>(&args, 2)?,
            arg(&args, 3)?,
            &arg(&args, 4)?,
        )),
        "RunHeader" => encode(arg::<agents::AgentRun>(&args, 0)?.header()),
        "RunEntry" => encode(agents::run_entry(&arg(&args, 0)?)),
        "RunEventId" => encode(agents::run_event_id(
            &arg(&args, 0)?,
            &arg::<String>(&args, 1)?,
        )),
        "EntryEventId" => encode(agents::entry_event_id(&arg(&args, 0)?)),
        "ActiveRunsRequest" => encode(agents::rows::active_runs_request(&owner(args[0].clone())?)),
        "RunByIdRequest" => encode(agents::rows::run_by_id_request(&arg::<String>(&args, 0)?)),
        "EntriesRequest" => encode(agents::rows::entries_request(arg::<Vec<String>>(&args, 0)?)),
        "NewestEntriesRequest" => encode(agents::rows::newest_entries_request(&arg::<String>(
            &args, 0,
        )?)),
        "EntriesFromRows" => encode(agents::rows::entries_from_rows(&arg(&args, 0)?)),
        "RunsFromRows" => encode(agents::rows::runs_from_rows(&arg(&args, 0)?)),
        "RunHeadersFromRows" => encode(agents::rows::run_headers_from_rows(&arg(&args, 0)?)),
        "RunMutation" => encode(agents::rows::run_mutation(&arg(&args, 0)?)),
        "RunMutations" => encode(agents::rows::run_mutations(&arg(&args, 0)?)),
        "RunOwnedMutation" => encode(agents::rows::run_owned_mutation(
            &arg(&args, 0)?,
            &owner(args[1].clone())?,
        )),
        "EntryMutation" => encode(agents::rows::entry_mutation(
            &arg::<String>(&args, 0)?,
            &arg(&args, 1)?,
            &arg(&args, 2)?,
        )),
        "LatchesFromRows" => encode(agents::rows::latches_from_rows(&arg(&args, 0)?)),
        "LatchMutation" => encode(agents::rows::latch_mutation(
            &owner(args[0].clone())?,
            arg(&args, 1)?,
            &arg::<String>(&args, 2)?,
        )),
        "LatchDeleteMutation" => encode(agents::rows::latch_delete_mutation(
            &owner(args[0].clone())?,
            arg::<Vec<u32>>(&args, 1)?,
        )),
        "SummariesFromRows" => encode(agents::rows::summaries_from_rows(&arg(&args, 0)?)),
        "SummaryMutation" => encode(agents::rows::summary_mutation(
            &owner(args[0].clone())?,
            arg(&args, 1)?,
            &arg(&args, 2)?,
        )),
        "SummaryDeleteMutation" => encode(agents::rows::summary_delete_mutation(
            &owner(args[0].clone())?,
            arg::<Vec<u32>>(&args, 1)?,
        )),
        "ClaimRuns" => {
            let mut runs = arg::<Vec<agents::AgentRun>>(&args, 0)?;
            agents::rows::claim_unowned_runs(&mut runs, &owner(args[1].clone())?);
            encode(runs)
        },
        "AdoptRun" => encode(
            arg::<agents::AgentRun>(&args, 0)?.adopted_by(arg(&args, 1)?, &owner(args[2].clone())?),
        ),
        "PollReplyString" => encode(
            serde_json::to_string(&arg::<wire::PollReply>(&args, 0)?)
                .context("serializing poll reply")?,
        ),
        "PollReply" => encode(arg::<wire::PollReply>(&args, 0)?),
        "PollReplyParse" => encode(
            serde_json::from_str::<wire::PollReply>(&arg::<String>(&args, 0)?)
                .map_err(|error| error.to_string()),
        ),
        "StripRows" => encode(agents::strip_rows(
            &arg::<Vec<_>>(&args, 0)?,
            &arg(&args, 1)?,
            &arg(&args, 2)?,
            &arg(&args, 3)?,
            &arg(&args, 4)?,
            &arg(&args, 5)?,
            &arg(&args, 6)?,
        )),
        "WorktreeCell" => encode(agents::worktree_cell(&arg(&args, 0)?)),
        "RunPayloadFingerprint" => {
            encode(agents::run_payload_fingerprint(&arg::<String>(&args, 0)?))
        },
        "AgentPaneContext" => encode(agent_proto::launch::agent_pane_context()),
        "PaneId" => encode(
            arg::<String>(&args, 0)?
                .parse::<yinz_types::pane_id::PaneId>()
                .map_err(|error| error.to_string()),
        ),
        "PlanSwap" => encode(agent_proto::pane_operations::plan_swap(
            &arg::<Vec<_>>(&args, 0)?,
            arg(&args, 1)?,
            arg(&args, 2)?,
        )),
        "CheckoutLocation" => encode(agents::checkout_location(
            arg::<Option<String>>(&args, 0)?.as_deref(),
            arg::<Option<String>>(&args, 1)?.as_deref(),
        )),
        "PaneCheckoutLocation" => encode(arg::<agents::AgentPane>(&args, 0)?.checkout_location()),
        "TabProjectLabels" => encode(agents::tab_project_labels(
            &arg::<Vec<agents::AgentPane>>(&args, 0)?,
            &arg(&args, 1)?,
        )),
        "TabProjectLabel" => encode(agents::tab_project_label(&arg::<Vec<_>>(&args, 0)?)),
        "LocationParts" => {
            let location = arg::<agent_proto::location::Location>(&args, 0)?;
            encode((location.host, location.path))
        },
        "CheckoutLabel" => {
            encode(arg::<agent_proto::projects::Repository>(&args, 0)?.checkout_label())
        },
        "RepositoryFolder" => encode(
            agent_proto::projects::Repository::folder(arg::<String>(&args, 0)?).on_host(arg::<
                Option<String>,
            >(
                &args, 1,
            )?),
        ),
        "RepositoryOnHost" => encode(
            arg::<agent_proto::projects::Repository>(&args, 0)?
                .on_host(arg::<Option<String>>(&args, 1)?),
        ),
        "GitProbe" => git_probe(
            arg(&args, 0)?,
            &arg::<Vec<u8>>(&args, 1)?,
            &arg::<Vec<u8>>(&args, 2)?,
        ),
        _ => bail!("unknown agents model operation {op:?}"),
    }
}

fn json_body(bytes: Vec<u8>) -> Result<Value> {
    let mut value: Value =
        serde_json::from_slice(&bytes).context("decoding shared JSON request body")?;
    value.sort_all_objects();
    encode(serde_json::to_vec(&value).context("encoding shared JSON request body")?)
}

pub(super) fn git_probe(exit_code: Option<i32>, metadata: &[u8], config: &[u8]) -> Result<Value> {
    match agent_proto::git_probe::repository(exit_code, metadata, config) {
        agent_proto::git_probe::ProbeOutcome::Checkout(repository) => {
            encode(serde_json::json!({"Checkout":repository}))
        },
        agent_proto::git_probe::ProbeOutcome::NotARepository => encode("NotARepository"),
        agent_proto::git_probe::ProbeOutcome::Failed => encode("Failed"),
    }
}
