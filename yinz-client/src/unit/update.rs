use super::*;

fn finished(updates: &mut Updates, token: &str, request: ParticipantRequest) {
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Ok(serde_json::json!({"phase": "published"})))
        .unwrap();
    updates.jobs.insert(
        token.into(),
        Job {
            request,
            result: None,
            applied: false,
            receiver,
        },
    );
}

#[test]
fn repeated_requests_return_original_result_and_conflicting_tokens_are_rejected() {
    let mut updates = Updates::new(None);
    let request = ParticipantRequest::Publish {
        operation: "operation-1".into(),
    };
    finished(&mut updates, "publish", request.clone());
    let first = updates
        .handle("publish", ClientRequest::Participant(request.clone()))
        .unwrap();
    assert_eq!(
        first,
        updates
            .handle("publish", ClientRequest::Participant(request))
            .unwrap()
    );
    assert_eq!(updates.published.as_deref(), Some("operation-1"));
    assert!(updates
        .handle(
            "publish",
            ClientRequest::Participant(ParticipantRequest::Rollback {
                operation: "operation-1".into(),
                prepare_unconfirmed: false,
            })
        )
        .is_err());
    assert_eq!(updates.jobs.len(), 1);
}

#[test]
fn polling_a_disconnected_worker_preserves_uncertain_ownership() {
    let mut updates = Updates::new(None);
    let (_sender, receiver) = mpsc::channel();
    drop(_sender);
    updates.jobs.insert(
        "publish".into(),
        Job {
            request: ParticipantRequest::Publish {
                operation: "operation-1".into(),
            },
            result: None,
            applied: false,
            receiver,
        },
    );
    assert!(updates
        .poll("publish")
        .unwrap_err()
        .to_string()
        .contains("journal"));
    assert!(updates.published.is_none());
    assert!(updates.poll("unknown").is_err());
}

#[test]
fn restart_requires_exact_published_operation_installation_and_existing_session_target() {
    let mut updates = Updates::new(None);
    let executable = std::fs::canonicalize("/usr/bin/true").unwrap();
    updates.executable = Ok(executable.clone());
    let mut target = SessionConnection {
        name: Some("same-session".into()),
        ..Default::default()
    };
    assert!(updates
        .validate_restart("operation-1", &executable, &target)
        .is_err());
    updates.published = Some("operation-1".into());
    assert!(updates
        .validate_restart("operation-2", &executable, &target)
        .is_err());
    assert!(updates
        .validate_restart("operation-1", Path::new("/other/executable"), &target)
        .is_err());
    assert!(updates
        .validate_restart("operation-1", &executable, &target)
        .is_ok());
    target.cwd = Some("/tmp".into());
    assert!(updates
        .validate_restart("operation-1", &executable, &target)
        .is_err());
    target.cwd = None;
    target.name = None;
    assert!(updates
        .validate_restart("operation-1", &executable, &target)
        .is_err());
}

#[test]
fn resume_preserves_remote_destination_configuration_and_focus() {
    let resume = Resume {
        operation: "operation-1".into(),
        source_client: yinz_types::data::ClientId::new(1),
        cli_args: yinz_cli::args::CliArgs {
            config: Some("/custom/config.kdl".into()),
            data_dir: Some("/custom/data".into()),
            ..Default::default()
        },
        config: yinz_config::config::Config::from_default_assets().unwrap(),
        options: Default::default(),
        remote_host: Some("user@host".into()),
        target: SessionConnection {
            name: Some("session".into()),
            tab_position: Some(yinz_types::internal_ids::TabNumber::new(2)),
            pane_id: Some(yinz_types::pane_id::PaneId::Terminal(
                yinz_types::internal_ids::TerminalId::new(42),
            )),
            ..Default::default()
        },
    };
    let restored: Resume = serde_json::from_slice(&serde_json::to_vec(&resume).unwrap()).unwrap();
    assert_eq!(restored.remote_host, resume.remote_host);
    assert_eq!(restored.cli_args.config, resume.cli_args.config);
    assert_eq!(restored.cli_args.data_dir, resume.cli_args.data_dir);
    assert_eq!(restored.target, resume.target);
    assert_eq!(restored.config.keybinds, resume.config.keybinds);
}

#[test]
fn resume_rejects_symlinks_and_group_readable_files_before_consuming_them() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(".yinz-client-resume-test");
    let real = directory.path().join("contents");
    std::fs::write(&real, "{}").unwrap();
    symlink(&real, &path).unwrap();
    assert!(resume_client(&path).is_err());
    assert!(path.is_symlink());
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "{}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    assert!(resume_client(&path)
        .unwrap_err()
        .to_string()
        .contains("private"));
    assert!(path.exists());
}

#[test]
fn helper_output_limit_never_silently_truncates_a_reply() {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"12345").unwrap();
    assert!(read_bounded(file, 4).is_err());
}

#[test]
fn old_publication_reply_cannot_rearm_restart_after_rollback() {
    let mut updates = Updates::new(None);
    finished(
        &mut updates,
        "publish",
        ParticipantRequest::Publish {
            operation: "op".into(),
        },
    );
    updates.poll("publish").unwrap();
    finished(
        &mut updates,
        "rollback",
        ParticipantRequest::Rollback {
            operation: "op".into(),
            prepare_unconfirmed: false,
        },
    );
    updates.poll("rollback").unwrap();
    updates.poll("publish").unwrap();
    assert!(updates.published.is_none());
}

#[test]
fn lost_helper_acknowledgement_is_settled_before_later_rollback() {
    let mut updates = Updates::new(None);
    let executable = std::fs::canonicalize("/usr/bin/true").unwrap();
    updates.executable = Ok(executable.clone());
    finished(
        &mut updates,
        "publish",
        ParticipantRequest::Publish {
            operation: "old".into(),
        },
    );
    // No coordinator poll arrives. Starting the next helper still processes
    // the earlier local result in order, while retaining its cached response.
    updates.settle_previous().unwrap();
    finished(
        &mut updates,
        "rollback",
        ParticipantRequest::Rollback {
            operation: "old".into(),
            prepare_unconfirmed: false,
        },
    );
    updates.poll("rollback").unwrap();
    updates.poll("publish").unwrap();
    assert!(updates.published.is_none());
    finished(
        &mut updates,
        "new",
        ParticipantRequest::Publish {
            operation: "new".into(),
        },
    );
    updates.poll("new").unwrap();
    assert!(updates
        .validate_restart(
            "new",
            &executable,
            &SessionConnection {
                name: Some("session".into()),
                ..Default::default()
            }
        )
        .is_ok());
}

#[test]
fn acknowledged_failure_does_not_block_a_later_successfully_published_operation() {
    let mut updates = Updates::new(None);
    let executable = std::fs::canonicalize("/usr/bin/true").unwrap();
    updates.executable = Ok(executable.clone());
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Err("prepare failed before publication".into()))
        .unwrap();
    updates.jobs.insert(
        "failed".into(),
        Job {
            request: ParticipantRequest::Prepare {
                operation: "old".into(),
                release: serde_json::Value::Null,
            },
            result: None,
            applied: false,
            receiver,
        },
    );
    assert!(updates.poll("failed").is_err());
    assert!(updates.published.is_none());
    finished(
        &mut updates,
        "new",
        ParticipantRequest::Publish {
            operation: "new".into(),
        },
    );
    updates.poll("new").unwrap();
    assert!(updates
        .validate_restart(
            "new",
            &executable,
            &SessionConnection {
                name: Some("session".into()),
                ..Default::default()
            }
        )
        .is_ok());
}

#[test]
fn inventory_reports_running_client_separately_from_installed_helper() {
    let mut inventory = serde_json::json!({"running_version":"new-helper", "installed_version":"new-helper",
            "capabilities": {"source_binary_sha256":"new-helper-hash"}});
    stamp_running_version(&mut inventory).unwrap();
    assert_eq!(inventory["running_version"], yinz_runtime::consts::VERSION);
    assert_eq!(inventory["installed_version"], "new-helper");
    assert_eq!(
        inventory["capabilities"]["source_binary_sha256"],
        serde_json::to_value(yinz_runtime::session_update::running_executable_digest().ok())
            .unwrap()
    );
    assert_ne!(
        inventory["capabilities"]["source_binary_sha256"],
        "new-helper-hash"
    );
}

#[test]
fn replacement_readiness_requires_advertised_server_protocol_and_is_sent_once() {
    let args = yinz_cli::args::CliArgs {
        update_resume: Some("operation".into()),
        update_source_client: Some(44),
        ..Default::default()
    };
    let mut config = crate::session_switch::LocalConfig::default();
    let mut sessions =
        crate::session_switch::Sessions::new(None, "session".into(), &args, &mut config);
    let os = crate::test_support::FakeClientOsApi::new();
    let ready = Request {
        token: "ready".into(),
        request: ClientRequest::Ready {
            operation: "operation".into(),
            source_client: yinz_types::data::ClientId::new(44),
        },
    };
    assert!(sessions.update(ready.clone()).0.is_err());
    sessions.set_update_protocol(PROTOCOL_VERSION);
    assert_eq!(
        sessions.update(ready.clone()).0.unwrap(),
        ClientReply::Pending
    );
    assert!(os.sent_messages().is_empty());
    sessions.update_rendered(&os);
    sessions.update_rendered(&os);
    assert_eq!(
        os.sent_messages(),
        vec![yinz_ipc::ipc::ClientToServerMsg::UpdateReply(Reply {
            token: "operation".into(),
            result: Ok(ClientReply::Resumed {
                operation: "operation".into(),
                source_client: yinz_types::data::ClientId::new(44),
            }),
        })]
    );
    assert_eq!(sessions.update(ready).0.unwrap(), ClientReply::Pending);
}

#[test]
fn installation_identity_survives_replacement_and_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let installed = directory.path().join("yinz");
    let displaced = directory.path().join("old-yinz");
    std::fs::write(&installed, b"old").unwrap();
    let cache = std::sync::OnceLock::new();
    let first = installation_path(&cache, || Ok(installed.clone())).unwrap();
    std::fs::rename(&installed, &displaced).unwrap();
    std::fs::write(&installed, b"new").unwrap();
    std::fs::remove_file(&displaced).unwrap();
    let next = installation_path(&cache, || Ok(displaced.clone())).unwrap();
    assert_eq!(first, next);
    assert_eq!(std::fs::read(next).unwrap(), b"new");
}

#[test]
fn source_chunk_helper_input_batches_writes_and_preserves_the_request() {
    #[derive(Default)]
    struct Writes {
        bytes: Vec<u8>,
        calls: usize,
    }
    impl Write for Writes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.calls += 1;
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let invocation = ParticipantInvocation {
        executable: "/fixture/yinz".into(),
        data_dir: Some("/fixture/data".into()),
        request: ParticipantRequest::SourceChunk {
            operation: "operation".into(),
            artifact: SourceArtifact {
                target: "x86_64-unknown-linux-gnu".into(),
                sha256: "a".repeat(64),
                binary_sha256: "b".repeat(64),
                size: 65536,
            },
            offset: 0,
            bytes: (0..65536).map(|i| (i % 256) as u8).collect(),
        },
    };
    let mut output = Writes::default();
    write_participant_input(&mut output, &invocation).unwrap();
    let decoded: ParticipantInvocation = serde_json::from_slice(&output.bytes).unwrap();
    assert_eq!(decoded.executable, invocation.executable);
    assert_eq!(decoded.data_dir, invocation.data_dir);
    assert_eq!(decoded.request, invocation.request);
    // A full chunk must use block writes. Direct serde_json serialization makes
    // over 100,000 tiny file writes per chunk, dominating real source transfers.
    assert!(
        output.calls <= 64,
        "{} writes for {} bytes",
        output.calls,
        output.bytes.len()
    );
}

#[test]
fn participant_input_flush_failure_is_reported_before_launch() {
    struct BrokenFlush;
    impl Write for BrokenFlush {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("fixture flush failure"))
        }
    }
    let invocation = ParticipantInvocation {
        executable: "/fixture/yinz".into(),
        data_dir: None,
        request: ParticipantRequest::Inventory,
    };
    let error = write_participant_input(BrokenFlush, &invocation).unwrap_err();
    assert!(format!("{error:#}").contains("fixture flush failure"));
}
