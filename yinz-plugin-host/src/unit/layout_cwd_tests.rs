use super::*;
use yinz_runtime::channels::{self, SenderWithContext};

#[test]
fn layout_tab_commands_preserve_kdl_cwds_without_a_tab_override() {
    for (raw_layout, expected_base) in [
        (
            r#"layout {
                pane cwd="/project/tiled"
                floating_panes { pane cwd="/project/floating"; }
            }"#,
            "/project",
        ),
        (
            r#"layout cwd="/project" {
                pane cwd="tiled"
                floating_panes { pane cwd="floating"; }
            }"#,
            "/project",
        ),
        (
            r#"layout cwd="/project" {
                tab cwd="work" {
                    pane cwd="tiled"
                    floating_panes { pane cwd="floating"; }
                }
            }"#,
            "/project/work",
        ),
    ] {
        for use_layout_info in [false, true] {
            let mut env = crate::test_env::test_env();
            env.plugin_cwd = PathBuf::from("/caller");
            env.response_sink = ResponseSink::Capture(None);
            let (to_screen, screen_receiver) = channels::unbounded();
            env.senders.to_screen = Some(SenderWithContext::new(to_screen));
            let screen_thread = thread::spawn(move || {
                let (instruction, _) = screen_receiver
                    .recv_timeout(Duration::from_secs(30))
                    .unwrap();
                let ScreenInstruction::NewTab(cwd, _, tiled, floating, ..) = instruction else {
                    panic!("expected a new tab, got {instruction:?}");
                };
                // Dropping the remaining instruction fields completes the routed action.
                (cwd, tiled.unwrap(), floating)
            });
            if use_layout_info {
                new_tabs_with_layout_info(&mut env, LayoutInfo::Stringified(raw_layout.to_owned()))
                    .unwrap();
            } else {
                new_tabs_with_layout(&mut env, raw_layout).unwrap();
            }
            let (cwd, tiled, floating) = screen_thread.join().unwrap();
            assert_eq!(cwd, None);
            assert_eq!(
                tiled.extract_run_instructions(),
                vec![Some(Run::Cwd(PathBuf::from(expected_base).join("tiled")))],
            );
            assert_eq!(floating.len(), 1);
            assert_eq!(
                floating[0].run,
                Some(Run::Cwd(PathBuf::from(expected_base).join("floating"))),
            );
        }
    }
}
