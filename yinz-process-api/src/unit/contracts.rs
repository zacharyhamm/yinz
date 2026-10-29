use super::*;

/// ssh tokenizes an `-o Name=value` string on whitespace even when it arrives
/// as one argv entry, so a path with a space has to be double-quoted or the
/// value is cut short — which for `UserKnownHostsFile` means every strict
/// host-key check fails.
#[test]
fn ssh_option_paths_are_quoted_for_the_option_tokenizer() {
    assert_eq!(
        ssh_option_value(Path::new("/home/a user/known_hosts")),
        r#""/home/a user/known_hosts""#
    );
    assert_eq!(
        ssh_option_value(Path::new(r#"/odd "dir"\here"#)),
        r#""/odd \"dir\"\\here""#
    );
}

#[test]
fn child_signals_keep_the_remote_wire_names() {
    for (signal, wire) in [
        (ChildSignal::Hup, "Hup"),
        (ChildSignal::Int, "Int"),
        (ChildSignal::Kill, "Kill"),
        (ChildSignal::Cont, "Cont"),
    ] {
        let encoded = serde_json::to_value(signal).unwrap();
        assert_eq!(encoded, wire);
        assert_eq!(
            serde_json::from_value::<ChildSignal>(encoded).unwrap(),
            signal
        );
    }
}
