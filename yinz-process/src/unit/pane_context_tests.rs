use super::*;

fn status() -> protocol::HostStatus {
    protocol::HostStatus {
        supports_relabel: true,
        child_group_terminated: false,
        host_id: SupervisorId::new("abcdef012345"),
        session_name: Some("after-warp".into()),
        terminal_id: 42,
        command: "codex".into(),
        cwd: None,
        child_pid: None,
        host_pid: yinz_types::internal_ids::ProcessId::new(11),
        started_at_unix: yinz_types::units::UnixSeconds::new(0),
        attached: true,
        child_exit_status: None,
        buffered_bytes: 0,
    }
}

#[test]
fn reference_requires_a_matching_attached_owner() {
    assert_eq!(
        resolve_with("abcdef012345", |_| Ok(status())).unwrap(),
        (SessionName::new("after-warp"), TerminalId::new(42))
    );
    for reference in ["", "invalid", "../abcdef012", "abcdefgh1234"] {
        assert!(resolve_with(reference, |_| panic!("invalid reference queried")).is_err());
    }
    for case in 0..4 {
        assert!(resolve_with("abcdef012345", |_| {
            let mut status = status();
            match case {
                0 => status.host_id = SupervisorId::new("000000000000"),
                1 => status.attached = false,
                2 => status.session_name = None,
                _ => bail!("unreachable supervisor"),
            }
            Ok(status)
        })
        .is_err());
    }
}
