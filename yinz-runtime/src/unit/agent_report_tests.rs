use super::*;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

#[test]
fn report_diagnostics_uses_the_supplied_session_identity() {
    let report = AgentReport {
        report_id: "00000000-0000-4000-8000-000000000001".into(),
        pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
        state: AgentReportState::Clear,
        event: None,
        trace: false,
        process_id: ProcessId::new(42),
        kind: "claude".into(),
    };
    let session = crate::envs::SessionState::new("original".into());
    assert_eq!(
        report_diagnostics(&report, &session.current()).session,
        "original"
    );
    session.set("renamed".into());
    assert_eq!(
        report_diagnostics(&report, &session.current()).session,
        "renamed"
    );
}

#[test]
fn forwarding_uses_only_the_bound_routing_environment() {
    for (token, remote) in [
        (None, false),
        (Some(""), false),
        (Some("bound-token"), true),
    ] {
        let mut env = HashMap::from([("YINZ_PANE_REF".into(), "bound-pane".into())]);
        if let Some(token) = token {
            env.insert("YINZ_REMOTE_PANE_TOKEN".into(), token.into());
        }
        let command = forwarding_command(Path::new("yinz"), "codex", &env);
        let args: Vec<_> = command.get_args().collect();
        let expected: &[&str] = if remote {
            &["--pty-broker", "report", "codex"]
        } else {
            &["action", "agent-report", "--hook-json", "--kind", "codex"]
        };
        assert_eq!(args, expected.iter().map(OsStr::new).collect::<Vec<_>>());
        let changes: HashMap<_, _> = command.get_envs().collect();
        for key in ROUTE_ENV {
            assert_eq!(
                changes.get(OsStr::new(key)).copied(),
                Some(env.get(*key).map(OsStr::new))
            );
        }
    }
}
