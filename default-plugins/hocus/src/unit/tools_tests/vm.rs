//! The pure half of the VM tools: the argv each one sends, and which panes
//! `vm_close` will close before it sends it.

use super::*;

fn vm_argv_for(name: &str, arguments: &str) -> Result<Vec<String>, String> {
    vm_argv(
        name,
        &Args::parse(arguments).unwrap(),
        "/usr/bin/zellij",
        "work",
    )
}

fn pane(id: u32, host: Option<&str>) -> zellij_tile::prelude::PaneInfo {
    zellij_tile::prelude::PaneInfo {
        id: zellij_tile::prelude::PaneId::from_legacy(id, false),
        target: PaneTarget::parse_display(host),
        ..Default::default()
    }
}

#[test]
fn listing_is_machine_wide_and_names_no_session() {
    // Deliberate: a VM another session holds, or one a crashed server left behind,
    // is still a VM the user has running. `--session` would hide both.
    assert_eq!(
        vm_argv_for("vm_list", "{}").unwrap(),
        ["/usr/bin/zellij", "firecracker", "list", "--json"]
    );
}

#[test]
fn each_per_vm_tool_sends_its_own_cli_verb() {
    for (tool, verb) in [
        ("vm_changes", "changes"),
        ("vm_sync", "sync"),
        // `vm_close` is `stop` plus the pane closing the CLI does not do.
        ("vm_close", "stop"),
    ] {
        assert_eq!(
            vm_argv_for(tool, r#"{"vm":"review"}"#).unwrap(),
            [
                "/usr/bin/zellij",
                "firecracker",
                verb,
                "review",
                "--session",
                "work"
            ],
            "{tool}"
        );
    }
}

#[test]
fn an_explicit_session_is_the_callers_to_choose() {
    // The session is resolved before `vm_argv`, so this pins that whatever the
    // caller resolved is what the CLI is told - see `start_vm_tool`.
    assert_eq!(
        vm_argv(
            "vm_sync",
            &Args::parse(r#"{"vm":"review"}"#).unwrap(),
            "zellij",
            "other",
        )
        .unwrap()
        .last()
        .unwrap(),
        "other"
    );
}

#[test]
fn a_per_vm_tool_without_a_vm_is_refused_before_it_runs() {
    for arguments in ["{}", r#"{"vm":"   "}"#] {
        let error = vm_argv_for("vm_sync", arguments).unwrap_err();
        assert_eq!(error, "vm is required; call vm_list for the names");
    }
}

/// A name Zellij would refuse is a mistake the model can fix from the message.
/// Finding that out should not cost a process.
#[test]
fn a_vm_name_zellij_would_refuse_never_reaches_the_cli() {
    let error = vm_argv_for("vm_close", r#"{"vm":"a/b"}"#).unwrap_err();
    assert!(!error.is_empty(), "{error}");
    assert!(vm_argv_for("vm_close", r#"{"vm":""}"#).is_err());
}

#[test]
fn only_the_panes_running_in_that_vm_are_selected() {
    let mut panes = vec![
        pane(1, Some("firecracker:review")),
        // Local: the manifest gives it no host at all.
        pane(2, None),
        // An ordinary ssh pane, which is on a machine rather than in a VM.
        pane(3, Some("build.example.com")),
        pane(4, Some("firecracker:other")),
        pane(5, Some("firecracker:review")),
    ];
    let mut plugin = pane(6, Some("firecracker:review"));
    plugin.id = zellij_tile::prelude::PaneId::from_legacy(plugin.id.legacy_parts().0, true);
    panes.push(plugin);
    let current = SessionInfo {
        name: "work".to_owned(),
        panes: zellij_tile::prelude::PaneManifest {
            panes: std::collections::HashMap::from([(0, panes)]),
        },
        ..Default::default()
    };
    assert_eq!(firecracker_panes("review", "work", &current), vec![1, 5]);
    assert_eq!(firecracker_panes("other", "work", &current), vec![4]);
    assert!(firecracker_panes("absent", "work", &current).is_empty());
    assert!(firecracker_panes("review", "other", &current).is_empty());
}

#[test]
fn padded_vm_names_use_the_validated_name() {
    assert_eq!(
        vm_argv_for("vm_close", r#"{"vm":"  review  "}"#).unwrap(),
        vm_argv_for("vm_close", r#"{"vm":"review"}"#).unwrap(),
    );
}
