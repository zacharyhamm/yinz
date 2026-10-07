use super::*;
use yinz_types::data::LayoutInfo;

fn check_host_paths(custom_opener: bool) {
    let root = tempfile::Builder::new()
        .prefix("finder-")
        .tempdir_in("/tmp")
        .unwrap();
    let cwd = root.path().join("project");
    let chosen = cwd.join("sub/book, one.pdf");
    std::fs::create_dir_all(chosen.parent().unwrap()).unwrap();
    let opener = if custom_opener {
        "open_pdf \"/tmp/my-viewer --literal {}\""
    } else {
        ""
    };
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!("{NO_BACKGROUND_PLUGINS}plugins {{\n file-finder location=\"yinz:file-finder\" cwd=\"{}\" {{\n {opener}\n }}\n}}", root.path().display()))
        .with_layout(LayoutInfo::Stringified(format!("layout {{ pane cwd=\"{}\"; }}", cwd.display())))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell prompt is visible", |screen| screen.contains("$ "));
    let (finder, selection_file) = open_finder(&yinz);
    match finder.terminal_action() {
        Some(TerminalAction::RunCommand(command)) => assert_eq!(command.cwd.as_ref(), Some(&cwd)),
        other => panic!("expected finder command: {other:?}"),
    }
    choose(&finder, &selection_file, chosen.to_str().unwrap());
    let viewer = yinz.expect_pty_spawn();
    match viewer.terminal_action() {
        Some(TerminalAction::RunCommand(command)) if custom_opener => {
            assert_eq!(command.cwd.as_deref(), chosen.parent());
            assert_eq!(command.command, Path::new("/tmp/my-viewer"));
            assert_eq!(command.args, ["--literal", chosen.to_str().unwrap()]);
        },
        Some(TerminalAction::OpenFile(file)) if !custom_opener => {
            assert_eq!(file.path, chosen);
            assert_eq!(file.cwd.as_deref(), chosen.parent());
        },
        other => panic!("unexpected viewer: {other:?}"),
    }
    viewer.exit(Some(0));
    yinz.quit();
}

#[test]
fn finder_and_custom_opener_keep_host_cwds_and_literal_arguments() {
    check_host_paths(true);
}

#[test]
fn finder_and_editor_keep_host_paths() {
    check_host_paths(false);
}

#[test]
fn real_finder_and_editor_use_the_tmp_project() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("finder.integration.test.ts");
    std::fs::write(&path, r#"
import {test, session, assert} from 'yinz:testing';
test('finder host paths through real PTYs', {grants:['RunCommands']}, async () => {
  const home = session.fixture({'project/sub/book, one.pdf':'finder-host-path-sentinel'}, {
    fzf: `set -eu
expected=$(cd "$HOME/project" && pwd -P)
[ "$(pwd -P)" = "$expected" ] || exit 1
input=$(cat)
[ "$input" = 'sub/book, one.pdf' ] || exit 2
printf '%s\\n' "$HOME/project/sub/book, one.pdf"
`,
    vi: `set -eu
[ "$#" -eq 1 ] || exit 3
[ "$1" = "$HOME/project/sub/book, one.pdf" ] || exit 4
[ "$(pwd -P)" = "$(cd "$HOME/project/sub" && pwd -P)" ] || exit 5
printf 'EDITOR_CWD=%s\\nEDITOR_ARG=%s\\n' "$(pwd -P)" "$1"
cat "$1"
printf '\\nEDITOR_READY\\n'
read release
`
  });
  session.start({columns:240});
  await session.waitForText('yinz-test>');
  session.text(`cd "$HOME/project" && printf 'PROJECT_%s\\n' READY`);
  session.key('Enter');
  await session.waitForText('PROJECT_READY');
  session.key('Alt e');
  await session.waitForText('EDITOR_READY');
  assert(session.screen.includes('EDITOR_CWD=' + home + '/project/sub'));
  assert(/EDITOR_ARG=\/tmp\/ypt-[^\s]+\/home\/project\/sub\/book, one.pdf/.test(session.screen));
  assert(session.screen.includes('finder-host-path-sentinel'));
  session.key('Enter');
  await session.waitFor('finder and editor cleaned up', () => session.panes.filter(pane => !pane.is_plugin).length === 1 && !session.panes.some(pane => pane.plugin_url === 'yinz:file-finder') && !session.screen.includes('EDITOR_READY'));
});
"#).unwrap();
    let report = yinz_server::plugin_testing::run(
        &path,
        &Default::default(),
        None,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}
