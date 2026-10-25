//! Translate native agent reports while retaining hook validation and input limits.
use super::parse_pane_id;
use std::path::PathBuf;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
use yinz_types::input::actions::{hook_report_actions, Action};

pub(super) fn agent_report(
    pane_id: Option<String>,
    kind: Option<String>,
    state: Option<String>,
    detail: Option<String>,
    cwd: Option<PathBuf>,
    hook_json: bool,
    clear: bool,
    run_id: Option<String>,
) -> Result<Vec<Action>, String> {
    let pane_id_str = match pane_id {
        Some(id) => id,
        None => return Err("No --pane-id provided and no resolved caller pane".to_string()),
    };
    let pane_id = parse_pane_id(&pane_id_str)?;
    let kind = kind
        .map(|kind| kind.trim().to_lowercase())
        .filter(|kind| !kind.is_empty())
        .unwrap_or_else(|| "claude".to_owned());
    // A hook event does two things, from the one process the hook may spend: it
    // sets the pane's record, and it is broadcast whole to the plugins, because
    // what a record holds is deliberately less than what an event carries. The
    // agent's own last message, the user's prompt and the permission mode are
    // read off the broadcast by whoever wants them - see `docs/AGENT_HOOKS.md`.
    if hook_json {
        use std::io::Read;
        let mut json = String::new();
        std::io::stdin()
            .take(yinz_types::agent::NATIVE_HOOK_MAX_BYTES as u64 + 1)
            .read_to_string(&mut json)
            .map_err(|e| format!("failed to read the hook event from stdin: {e}"))?;
        if json.len() > yinz_types::agent::NATIVE_HOOK_MAX_BYTES {
            return Err("hook input exceeds 2097152 bytes".into());
        }
        let (event, json) = yinz_runtime::agent::hooks::parse_hook_event(&kind, json)
            .map_err(|e| format!("the hook event is not valid agent hook JSON: {e}"))?;
        return Ok(hook_report_actions(&kind, pane_id, &event, json));
    }
    let agent = if clear {
        None
    } else {
        let state = state
            .ok_or_else(|| "one of --state, --hook-json or --clear is required".to_string())?;
        let state = AgentState::parse_lenient(&state).ok_or_else(|| {
            format!("Unknown agent state '{state}': busy, waiting, idle or exited")
        })?;
        Some(AgentInfo {
            kind,
            state,
            source: AgentSource::Reported,
            detail: detail.filter(|detail| !detail.trim().is_empty()),
            // `--detail` is the tool hint; a script that wants to say what it is
            // blocked on says it there, and the hook road fills this in itself.
            waiting_on: None,
            // A script that troubles itself to say `waiting` is saying a person
            // is wanted; there is no permission mode here to answer for it.
            wait_needs_a_person: state == AgentState::Waiting,
            cwd: cwd.map(|cwd| cwd.display().to_string()),
            session_id: None,
            // `None` is not "no run": a record that does not name one inherits
            // the pane's, so a script that sets it once need not repeat it on
            // every report. See `AgentInfo::run_id`.
            run_id: run_id.filter(|run_id| !run_id.trim().is_empty()),
        })
    };
    Ok(vec![Action::SetPaneAgent { pane_id, agent }])
}
