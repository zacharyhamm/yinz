use super::*;
use yinz_types::pane_size::PaneGeom;

fn make_command_pane(
    terminal_id: TerminalId,
    command: &str,
    args: Vec<&str>,
) -> PaneLayoutMetadata {
    let mut run_command = RunCommand::new(PathBuf::from(command));
    run_command.args = args.into_iter().map(|s| s.to_string()).collect();
    PaneLayoutMetadata::new(
        PaneId::Terminal(terminal_id),
        PaneGeom::default(),
        false,
        Some(Run::Command(run_command)),
        None,
        false,
        None,
        vec![],
        None,
        None,
    )
}

#[test]
fn bubblewrap_commands_survive_running_metadata_and_session_roundtrip() {
    use yinz_types::command::PaneTarget;
    for (program, args) in [
        ("claude", vec!["--resume".into()]),
        ("/bin/sh", vec![]),
        ("nvim", vec!["notes.md".into()]),
    ] {
        for floating in [false, true] {
            let command = RunCommand {
                command: program.into(),
                args: args.clone(),
                hold_on_close: true,
                target: PaneTarget::bubblewrap(std::path::Path::new("/repo")).unwrap(),
                ..Default::default()
            };
            let mut pane = make_command_pane(TerminalId::new(1), "claude", vec![]);
            pane.run = Some(Run::Command(command.clone()));
            pane.geom.rows = yinz_types::pane_size::Dimension::fixed(10);
            pane.geom.cols = yinz_types::pane_size::Dimension::fixed(10);
            let mut metadata = SessionLayoutMetadata::default();
            let (tiled, floats) = if floating {
                (vec![], vec![pane])
            } else {
                (vec![pane], vec![])
            };
            metadata.add_tab("sandbox".into(), true, false, false, tiled, floats);
            metadata.update_terminal_commands(HashMap::from([(
                TerminalId::new(1),
                vec!["bwrap".into(), "--args".into(), "17".into()],
            )]));
            metadata.update_default_shell("/bin/sh".into());
            metadata.update_default_editor(&Some("nvim".into()));
            metadata.detect_editor_panes();
            let pane = if floating {
                &metadata.tabs[0].floating_panes[0]
            } else {
                &metadata.tabs[0].tiled_panes[0]
            };
            assert_eq!(pane.run, Some(Run::Command(command.clone())));
            let (source, _) =
                yinz_session::session_serialization::serialize_session_layout(metadata.into())
                    .unwrap();
            let layout = yinz_config::kdl::layout::from_kdl(&source, None, None, None).unwrap();
            let (_, tiled, floats) = &layout.tabs[0];
            let mut panes = vec![tiled];
            let mut commands = floats
                .iter()
                .filter_map(|pane| pane.run.as_ref())
                .collect::<Vec<_>>();
            while let Some(pane) = panes.pop() {
                commands.extend(pane.run.as_ref());
                panes.extend(&pane.children);
            }
            let mut restored = command;
            restored.hold_on_start = true;
            assert!(commands.contains(&&Run::Command(restored)), "{source}");
        }
    }
}

fn make_edit_file_pane(
    terminal_id: TerminalId,
    path: &str,
    line_number: Option<usize>,
) -> PaneLayoutMetadata {
    PaneLayoutMetadata::new(
        PaneId::Terminal(terminal_id),
        PaneGeom::default(),
        false,
        Some(Run::EditFile(PathBuf::from(path), line_number, None)),
        None,
        false,
        None,
        vec![],
        None,
        None,
    )
}

fn session_with_editor(editor: &str, panes: Vec<PaneLayoutMetadata>) -> SessionLayoutMetadata {
    let mut meta = SessionLayoutMetadata {
        default_editor: Some(PathBuf::from(editor)),
        ..Default::default()
    };
    meta.add_tab("tab1".to_string(), true, false, false, panes, vec![]);
    meta
}

fn get_first_tiled_run(meta: &SessionLayoutMetadata) -> Option<&Run> {
    meta.tabs[0].tiled_panes[0].run.as_ref()
}

// detects_editor_pane_no_line_number: covered by the group-0 draw with
// with_line = false. The two line-number cases below are kept as the
// documentation of the two argument syntaxes.

#[test]
fn detects_editor_pane_with_line_number_vim_family() {
    let pane = make_command_pane(crate::TerminalId::new(1), "nvim", vec!["+50", "file.txt"]);
    let mut meta = session_with_editor("nvim", vec![pane]);
    meta.detect_editor_panes();
    assert_eq!(
        get_first_tiled_run(&meta),
        Some(&Run::EditFile(PathBuf::from("file.txt"), Some(50), None))
    );
}

#[test]
fn detects_editor_pane_with_line_number_helix() {
    let pane = make_command_pane(crate::TerminalId::new(1), "hx", vec!["file.txt:50"]);
    let mut meta = session_with_editor("hx", vec![pane]);
    meta.detect_editor_panes();
    assert_eq!(
        get_first_tiled_run(&meta),
        Some(&Run::EditFile(PathBuf::from("file.txt"), Some(50), None))
    );
}

// detects_editor_pane_helix_no_line_number: covered by the group-1 draw
// with with_line = false.

#[test]
fn skips_non_editor_command() {
    let pane = make_command_pane(
        crate::TerminalId::new(1),
        "grep",
        vec!["pattern", "file.txt"],
    );
    let mut meta = session_with_editor("nvim", vec![pane]);
    meta.detect_editor_panes();
    // Run::Command(grep ...) unchanged
    match get_first_tiled_run(&meta) {
        Some(Run::Command(rc)) => {
            assert_eq!(rc.command, PathBuf::from("grep"));
        },
        other => panic!("expected Command, got {:?}", other),
    }
}

#[test]
fn skips_editor_with_no_args() {
    let pane = make_command_pane(crate::TerminalId::new(1), "nvim", vec![]);
    let mut meta = session_with_editor("nvim", vec![pane]);
    meta.detect_editor_panes();
    match get_first_tiled_run(&meta) {
        Some(Run::Command(rc)) => {
            assert_eq!(rc.command, PathBuf::from("nvim"));
            assert!(rc.args.is_empty());
        },
        other => panic!("expected Command, got {:?}", other),
    }
}

#[test]
fn skips_editor_with_multiple_files() {
    let pane = make_command_pane(crate::TerminalId::new(1), "nvim", vec!["a.txt", "b.txt"]);
    let mut meta = session_with_editor("nvim", vec![pane]);
    meta.detect_editor_panes();
    match get_first_tiled_run(&meta) {
        Some(Run::Command(rc)) => {
            assert_eq!(rc.command, PathBuf::from("nvim"));
            assert_eq!(rc.args, vec!["a.txt", "b.txt"]);
        },
        other => panic!("expected Command, got {:?}", other),
    }
}

#[test]
fn detects_editor_matching_by_binary_name() {
    // configured as /usr/bin/nvim, running as nvim
    let pane = make_command_pane(crate::TerminalId::new(1), "nvim", vec!["file.txt"]);
    let mut meta = session_with_editor("/usr/bin/nvim", vec![pane]);
    meta.detect_editor_panes();
    assert_eq!(
        get_first_tiled_run(&meta),
        Some(&Run::EditFile(PathBuf::from("file.txt"), None, None))
    );
}

#[test]
fn does_not_affect_existing_edit_file_run() {
    let pane = make_edit_file_pane(crate::TerminalId::new(1), "file.txt", Some(10));
    let mut meta = session_with_editor("nvim", vec![pane]);
    meta.detect_editor_panes();
    assert_eq!(
        get_first_tiled_run(&meta),
        Some(&Run::EditFile(PathBuf::from("file.txt"), Some(10), None))
    );
}

// detects_vi_when_editor_is_vim and detects_nvim_when_editor_is_vi: the
// group-0 draw picks the configured editor and the pane's binary
// independently from {vi, vim, nvim}, so it produces both pairings. The
// negative case below stays - the generator never crosses the groups.

#[test]
fn does_not_cross_match_vim_with_emacs() {
    // configured as emacs, pane running vim — NOT cross-matched
    let pane = make_command_pane(crate::TerminalId::new(1), "vim", vec!["file.txt"]);
    let mut meta = session_with_editor("emacs", vec![pane]);
    meta.detect_editor_panes();
    match get_first_tiled_run(&meta) {
        Some(Run::Command(rc)) => assert_eq!(rc.command, PathBuf::from("vim")),
        other => panic!("expected Command, got {:?}", other),
    }
}

#[test]
fn list_clients_header_lines_up_with_its_rows() {
    let clients = BTreeMap::from([(
        ClientId::new(1),
        ClientMetadata {
            pane_id: PaneId::Terminal(crate::TerminalId::new(2)),
            command: Some(Run::Command(RunCommand::new(PathBuf::from("htop")))),
        },
    )]);
    let rendered = ClientMetadata::render_many(clients, &None);
    let lines: Vec<&str> = rendered.lines().collect();
    let [header, row] = lines[..] else {
        panic!("expected a header and one row:\n{rendered}");
    };
    let column = |line: &str, cell: &str| {
        line.find(cell)
            .unwrap_or_else(|| panic!("{cell:?} missing from {line:?}"))
    };
    assert_eq!(
        column(header, "YINZ_PANE_ID"),
        column(row, "terminal_2"),
        "pane id column misaligned:\n{rendered}"
    );
    assert_eq!(
        column(header, "RUNNING_COMMAND"),
        column(row, "htop"),
        "running command column misaligned:\n{rendered}"
    );
}

// detects_hx_when_editor_is_helix: the group-1 draw pairs the configured
// editor and the pane binary independently across {hx, helix}.
//
// absolute_file_path_preserved: "/home/user/file.txt" is one of the
// generator's file_paths, drawn on every group.

mod props {
    use super::*;
    use hegel::generators;

    /// File paths a pane could plausibly have been launched on. None start
    /// with `-`, which `detect_editor_panes` treats as a flag rather than a
    /// file.
    fn file_paths() -> Vec<&'static str> {
        vec![
            "file.txt",
            "src/main.rs",
            "/home/user/file.txt",
            "/tmp/a.log",
            "a-b.txt",
            "x",
            "Cargo.toml",
            "./relative.rs",
        ]
    }

    /// Paths that themselves contain a `:`, so that appending `:N` for
    /// helix produces more than one colon. The parser splits on the *last*
    /// one; these are what make that distinguishable from the first.
    fn file_paths_with_colons() -> Vec<&'static str> {
        vec!["weird:name.txt", "a:b:c.rs", "/tmp/ns:1/file.txt", "x:9"]
    }

    /// A command invocation `detect_editor_panes` must turn into an
    /// `EditFile` run, as `(configured editor, pane command, args, expected
    /// path, expected line)`.
    ///
    /// The three groups mirror the three ways the function decides a pane
    /// is an editor: the vi/vim/nvim lineage and the helix pair each
    /// cross-match within themselves, everything else has to match its own
    /// binary name exactly. Which argument syntax is legal depends on the
    /// *pane's* binary, not the configured one - `vi` is outside
    /// `is_vim_family`, so it never gets the `+N` form.
    #[hegel::composite]
    fn editor_invocation(
        tc: &hegel::TestCase,
    ) -> (String, String, Vec<String>, PathBuf, Option<usize>) {
        let line = tc.draw(generators::integers::<usize>().max_value(999_999));
        let group = tc.draw(generators::integers::<u8>().max_value(2));
        let (configured, command) = match group {
            0 => {
                let lineage = vec!["vi", "vim", "nvim"];
                (
                    tc.draw(generators::sampled_from(lineage.clone())),
                    tc.draw(generators::sampled_from(lineage)),
                )
            },
            1 => {
                let helix = vec!["hx", "helix"];
                (
                    tc.draw(generators::sampled_from(helix.clone())),
                    tc.draw(generators::sampled_from(helix)),
                )
            },
            _ => {
                let solo = tc.draw(generators::sampled_from(vec!["emacs", "nano", "kak"]));
                (solo, solo)
            },
        };

        let is_vim_family = matches!(command, "vim" | "nvim" | "emacs" | "nano" | "kak");
        let is_helix = matches!(command, "hx" | "helix");
        let with_line = tc.draw(generators::booleans());

        // Only the helix `file:N` form can carry a colon in the path: for
        // everything else the path is passed through untouched, so a colon
        // would add nothing.
        let file = if with_line && is_helix && tc.draw(generators::booleans()) {
            tc.draw(generators::sampled_from(file_paths_with_colons()))
        } else {
            tc.draw(generators::sampled_from(file_paths()))
        };

        let (args, expected_line) = if with_line && is_vim_family {
            (vec![format!("+{}", line), file.to_string()], Some(line))
        } else if with_line && is_helix {
            (vec![format!("{}:{}", file, line)], Some(line))
        } else {
            (vec![file.to_string()], None)
        };

        (
            configured.to_string(),
            command.to_string(),
            args,
            PathBuf::from(file),
            expected_line,
        )
    }

    #[hegel::test]
    fn any_known_editor_invocation_becomes_an_edit_file_run(tc: hegel::TestCase) {
        let (configured, command, args, path, line) = tc.draw(editor_invocation());
        let pane = make_command_pane(
            crate::TerminalId::new(1),
            &command,
            args.iter().map(|s| s.as_str()).collect(),
        );
        let mut meta = session_with_editor(&configured, vec![pane]);
        meta.detect_editor_panes();
        assert_eq!(
            get_first_tiled_run(&meta),
            Some(&Run::EditFile(path.clone(), line, None)),
            "editor {:?} running {:?} {:?} did not become EditFile({:?}, {:?})",
            configured,
            command,
            args,
            path,
            line
        );
    }

    /// `detect_editor_panes` mutates the metadata in place, so running it
    /// again on its own output must be a no-op - the second pass sees an
    /// `EditFile` run rather than a `Command` and has to leave it alone.
    #[hegel::test]
    fn detecting_editor_panes_twice_changes_nothing(tc: hegel::TestCase) {
        let (configured, command, args, _, _) = tc.draw(editor_invocation());
        let pane = make_command_pane(
            crate::TerminalId::new(1),
            &command,
            args.iter().map(|s| s.as_str()).collect(),
        );
        let mut meta = session_with_editor(&configured, vec![pane]);
        meta.detect_editor_panes();
        let after_once = get_first_tiled_run(&meta).cloned();
        meta.detect_editor_panes();
        assert_eq!(
            get_first_tiled_run(&meta),
            after_once.as_ref(),
            "second detect_editor_panes changed {:?} {:?}",
            command,
            args
        );
    }

    /// Commands that are not the configured editor keep their `Run::Command`
    /// verbatim. Cross-matching is deliberately narrow, so e.g. an `emacs`
    /// pane under a `vim` config must not be rewritten.
    #[hegel::test]
    fn a_non_editor_command_is_never_rewritten(tc: hegel::TestCase) {
        let configured = tc.draw(generators::sampled_from(vec![
            "vim", "nvim", "vi", "hx", "helix", "emacs",
        ]));
        let command = tc.draw(generators::sampled_from(vec![
            "grep", "cat", "tail", "ls", "cargo", "git", "bash",
        ]));
        let file = tc.draw(generators::sampled_from(file_paths()));

        let pane = make_command_pane(crate::TerminalId::new(1), command, vec![file]);
        let mut meta = session_with_editor(configured, vec![pane]);
        meta.detect_editor_panes();

        match get_first_tiled_run(&meta) {
            Some(Run::Command(run_command)) => {
                assert_eq!(run_command.command, PathBuf::from(command));
                assert_eq!(run_command.args, vec![file.to_string()]);
            },
            other => panic!(
                "{:?} under editor {:?} became {:?}",
                command, configured, other
            ),
        }
    }
}
