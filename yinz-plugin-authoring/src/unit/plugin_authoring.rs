use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn unchecked_overwrite_backs_up_source_permissions() {
    for (existing_backup, mode) in [(true, 0o600), (false, 0o754)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.scm");
        let backup = dir.path().join("keys.bak.scm");
        let original = "(define before #t)\n";
        let replacement = "(define after #t)\n";
        std::fs::write(&path, original).unwrap();
        let permissions = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(&path, permissions).unwrap();
        let permissions = std::fs::metadata(&path).unwrap().permissions();
        if existing_backup {
            std::fs::write(&backup, "older backup").unwrap();
            std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        let written = write(dir.path(), "keys", replacement, true, false).unwrap();

        assert_eq!(written.replaced.as_ref(), Some(&backup));
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&backup).unwrap().permissions(),
            permissions
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), replacement);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions(), permissions);
    }
}

#[test]
fn formatting_backs_up_source_permissions() {
    for (existing_backup, mode) in [(true, 0o600), (false, 0o754)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.scm");
        let backup = dir.path().join("keys.bak.scm");
        let original = "(define before #t)";
        std::fs::write(&path, original).unwrap();
        let permissions = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(&path, permissions).unwrap();
        let permissions = std::fs::metadata(&path).unwrap().permissions();
        if existing_backup {
            std::fs::write(&backup, "older backup").unwrap();
            std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        format_file(&path, false).unwrap();

        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&backup).unwrap().permissions(),
            permissions
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "(define before #t)\n"
        );
        assert_eq!(std::fs::metadata(&path).unwrap().permissions(), permissions);
    }
}

#[test]
fn formatting_does_not_stop_at_a_malformed_pane_declaration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("twice.scm");
    std::fs::write(&path, "  (yinz-declare-pane)\n(yinz-declare-pane)").unwrap();

    let warnings = format_file(&path, false).unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "(yinz-declare-pane)\n(yinz-declare-pane)\n"
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("line 2: the pane is declared more than once")),
        "{warnings:?}"
    );
}

#[test]
fn plugin_replacement_preserves_hard_linked_sources_and_backups() {
    let dir = tempfile::tempdir().unwrap();
    let original = "(define before #t)\n";
    let replacement = "(define after #t)\n";
    let written = write(dir.path(), "keys", original, false, false).unwrap();
    let alias = dir.path().join("alias.scm");
    let backup = dir.path().join("keys.bak.scm");
    std::fs::hard_link(&written.path, &alias).unwrap();
    std::fs::hard_link(&written.path, &backup).unwrap();

    write(dir.path(), "keys", replacement, true, false).unwrap();

    assert_eq!(std::fs::read_to_string(&written.path).unwrap(), replacement);
    assert_eq!(std::fs::read_to_string(&alias).unwrap(), original);
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
}

#[test]
fn concurrent_plugin_creators_never_replace_the_winner() {
    let dir = tempfile::tempdir().unwrap();
    let gate = std::sync::Barrier::new(16);
    std::thread::scope(|scope| {
        let writers: Vec<_> = (0..16)
            .map(|index| {
                let gate = &gate;
                let dir = dir.path();
                scope.spawn(move || {
                    let source = format!("(define value {index})\n");
                    gate.wait();
                    write(dir, "keys", &source, false, false).map(|_| source)
                })
            })
            .collect();
        let successful: Vec<_> = writers
            .into_iter()
            .filter_map(|writer| writer.join().unwrap().ok())
            .collect();
        assert_eq!(successful.len(), 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("keys.scm")).unwrap(),
            successful[0]
        );
        assert!(!dir.path().join("keys.bak.scm").exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    });
}

#[test]
fn check_plugin_report_carries_computed_permission_warnings() {
    let dir = tempfile::tempdir().unwrap();
    // A computed permission request typechecks and cannot be inferred: a warning, not an error.
    let plugin = dir.path().join("computed.scm");
    std::fs::write(
            &plugin,
            r#"(define wanted '("ReadFiles")) (define (load config) (yinz-wait-for-permissions wanted))"#,
        )
        .unwrap();
    let report = check_plugin_report(&plugin).unwrap();
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert!(
        report.warnings.iter().any(|w| w.contains("computed")),
        "{report:?}"
    );
    std::fs::write(&plugin, "(define value").unwrap();
    assert!(check_plugin_report(&plugin).is_err());
}

#[test]
fn scheme_writes_check_before_replacing_and_preserve_backups() {
    let dir = tempfile::tempdir().unwrap();
    for extension in ["scm", "ss", "sld"] {
        let name = format!("example.{extension}");
        let source = "; original\n(define (render rows columns) \"hello\")\n";
        let written = write(dir.path(), &name, source, false, true).unwrap();
        assert_eq!(std::fs::read_to_string(&written.path).unwrap(), source);
        assert!(write(dir.path(), &name, "(define (broken", true, true).is_err());
        assert_eq!(std::fs::read_to_string(&written.path).unwrap(), source);
        let replacement = "; replacement\n(define (render rows columns) \"updated\")\n";
        let written = write(dir.path(), &name, replacement, true, true).unwrap();
        assert_eq!(std::fs::read_to_string(&written.path).unwrap(), replacement);
        assert_eq!(
            std::fs::read_to_string(written.replaced.unwrap()).unwrap(),
            source
        );
        assert!(check_plugin_report(&written.path)
            .unwrap()
            .outline
            .iter()
            .any(|entry| entry.text == "(render rows columns)"));
    }
}

#[test]
fn scheme_import_writes_preserve_previous_versions() {
    for extension in ["scm", "ss", "sld"] {
        assert_scheme_writes_preserve_previous_versions(extension);
    }
}

fn assert_scheme_writes_preserve_previous_versions(extension: &str) {
    let dir = tempfile::tempdir().unwrap();
    let name = format!("app/keys.{extension}");
    // Dependencies must exist before authoring their importer.
    assert!(write(
        dir.path(),
        &name,
        r#"(require "./missing.scm")"#,
        false,
        true
    )
    .is_err());
    assert!(!dir.path().join(&name).exists());
    std::fs::create_dir_all(dir.path().join("app")).unwrap();
    std::fs::write(
        dir.path().join("app/dep.scm"),
        "(provide dependency) (define dependency 1)\n",
    )
    .unwrap();
    let source = r#"(require "./dep.scm")
(define before #t)"#;
    let written = write(dir.path(), &name, source, false, true).unwrap();
    assert_eq!(written.path, dir.path().join(&name));
    assert!(
        check_plugin(&written.path).is_ok(),
        "semantic check resolves existing imports"
    );
    assert!(write(dir.path(), &name, "(define value 1)", false, true).is_err());
    assert!(write(dir.path(), &name, "(define value", true, true).is_err());
    assert_eq!(
        std::fs::read_to_string(&written.path).unwrap(),
        format!("{source}\n")
    );
    let replaced = write(dir.path(), &name, "(define value 1)", true, true).unwrap();
    let backup = dir.path().join(format!("app/keys.bak.{extension}"));
    assert_eq!(replaced.replaced, Some(backup.clone()));
    assert_eq!(
        std::fs::read_to_string(&backup).unwrap(),
        format!("{source}\n")
    );
    assert!(write(dir.path(), &name, "(define value 1)\n\n", true, true)
        .unwrap()
        .replaced
        .is_none());
    assert_eq!(
        std::fs::read_to_string(&backup).unwrap(),
        format!("{source}\n")
    );
    let recursive = format!("{}0{}", "(".repeat(10_000), ")".repeat(10_000));
    assert!(write(dir.path(), &name, &recursive, true, true).is_err());
    assert_eq!(
        std::fs::read_to_string(&written.path).unwrap(),
        "(define value 1)\n"
    );
    assert_eq!(
        std::fs::read_to_string(&backup).unwrap(),
        format!("{source}\n")
    );
    #[cfg(unix)]
    {
        let linked = format!("linked.{extension}");
        std::os::unix::fs::symlink(&written.path, dir.path().join(&linked)).unwrap();
        assert!(write(dir.path(), &linked, "(define value 1)", true, true)
            .unwrap_err()
            .to_string()
            .contains("symlink"));
        let protected = format!("protected.{extension}");
        std::fs::copy(dir.path().join("app/dep.scm"), dir.path().join("dep.scm")).unwrap();
        let original = write(dir.path(), &protected, source, false, true).unwrap();
        std::os::unix::fs::symlink(
            &written.path,
            dir.path().join(format!("protected.bak.{extension}")),
        )
        .unwrap();
        assert!(
            write(dir.path(), &protected, "(define value 1)", true, true)
                .unwrap_err()
                .to_string()
                .contains("backup")
        );
        assert_eq!(
            std::fs::read_to_string(original.path).unwrap(),
            format!("{source}\n")
        );
    }
}

#[test]
fn unchecked_writes_preserve_invalid_source_for_later_editing() {
    let dir = tempfile::tempdir().unwrap();
    let written = write(dir.path(), "broken.scm", "(define value", false, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(written.path).unwrap(),
        "(define value\n"
    );
}

#[test]
fn lua_names_are_rejected_even_without_a_syntax_check() {
    let dir = tempfile::tempdir().unwrap();
    for check in [false, true] {
        let error = write(dir.path(), "old.lua", "return {}", false, check).unwrap_err();
        assert!(error
            .to_string()
            .contains("Lua plugins are no longer supported"));
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert!(check_plugin(&dir.path().join("missing.lua"))
        .unwrap_err()
        .to_string()
        .contains("Lua plugins are no longer supported"));
    assert!(check_source(
        Path::new("old.lua"),
        "return {}",
        yinz_types::authoring::PluginCheckMode::Syntax
    )
    .diagnostics[0]
        .contains("Lua plugins are no longer supported"));
}

#[test]
fn a_write_compiles_first_and_never_clobbers_or_follows_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    let written = write(&plugins, "keys", "(define value 1)\n\n", false, true).unwrap();
    assert_eq!(written.path, plugins.join("keys.scm"));
    assert!(written.replaced.is_none(), "nothing was there to replace");
    assert_eq!(
        std::fs::read_to_string(&written.path).unwrap(),
        "(define value 1)\n"
    );
    let refused = write(&plugins, "keys", "(define value 1)", false, true).unwrap_err();
    assert!(refused.to_string().contains("--overwrite"), "{refused}");
    let replaced = write(&plugins, "keys", "(define x 1)", true, true).unwrap();
    assert_eq!(
        replaced.replaced,
        Some(plugins.join("keys.bak.scm")),
        "an overwrite keeps what it replaced"
    );
    assert_eq!(
        std::fs::read_to_string(plugins.join("keys.bak.scm")).unwrap(),
        "(define value 1)\n"
    );
    // A rewrite that changes nothing must not overwrite the backup: what that would throw
    // away is the last version that actually differed.
    let identical = write(&plugins, "keys", "(define x 1)\n\n", true, true).unwrap();
    assert!(identical.replaced.is_none(), "nothing was replaced");
    assert_eq!(
        std::fs::read_to_string(plugins.join("keys.bak.scm")).unwrap(),
        "(define value 1)\n",
        "the backup still holds the version before this one"
    );
    let refused = write(&plugins, "broken", "(define value", false, true).unwrap_err();
    assert!(
        refused.to_string().contains("does not compile"),
        "{refused}"
    );
    assert!(!plugins.join("broken.scm").exists());
    write(&plugins, "broken", "(define value", false, false).unwrap();
    let module = write(&plugins, "app/ui", "(define value 1)", false, true).unwrap();
    assert_eq!(module.path, plugins.join("app").join("ui.scm"));
    assert!(write(&plugins, "../escape", "(define value 1)", false, true).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path(), plugins.join("linked")).unwrap();
        let refused = write(&plugins, "linked/x", "(define value 1)", false, true).unwrap_err();
        assert!(refused.to_string().contains("symlink"), "{refused}");
        // A backup that cannot be kept refuses the write: the alternative is destroying the
        // only copy of the file and saying nothing about it.
        write(&plugins, "linked-bak", "(define first 1)", false, true).unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("elsewhere.scm"),
            plugins.join("linked-bak.bak.scm"),
        )
        .unwrap();
        let refused = write(&plugins, "linked-bak", "(define second 2)", true, true).unwrap_err();
        assert!(refused.to_string().contains("backup"), "{refused}");
        assert_eq!(
            std::fs::read_to_string(plugins.join("linked-bak.scm")).unwrap(),
            "(define first 1)\n",
            "the file it could not back up is still there"
        );
        assert!(!dir.path().join("elsewhere.scm").exists());
    }
}

#[test]
fn the_log_tail_is_the_plugins_lines_only() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("yinz.log");
    std::fs::write(
        &log,
        "a [steel:keys.scm] one\nb other\nc [steel:keys.scm] two\nd [steel:keys.scm] three\n",
    )
    .unwrap();
    let tail = log_tail(&log, Path::new("/p/keys.scm"), 2).unwrap();
    assert_eq!(tail, "c [steel:keys.scm] two\nd [steel:keys.scm] three\n");
    // A builtin logs under its bare tag; asking for it by name finds those lines and not
    // a written plugin's that happens to share the prefix.
    std::fs::write(
            &log,
            "a [steel:agent-policy] hook\nb [steel:agent-policy-mine.scm] x\nc [steel:agent-policy] two\n",
        )
        .unwrap();
    assert_eq!(
        log_tail(&log, Path::new("/p/agent-policy.scm"), 5).unwrap(),
        "a [steel:agent-policy] hook\nc [steel:agent-policy] two\n"
    );
    assert_eq!(log_tail(&log, Path::new("/p/other.scm"), 5).unwrap(), "");
    assert!(log_tail(&dir.path().join("missing"), Path::new("/p/keys.scm"), 5).is_err());
}
