use super::*;
use yinz_cli::projects::Projects;
use yinz_integration_tests::test_env;

#[test]
fn last_project_survives_restart_and_yields_to_a_known_folder() {
    let config = r#"
        pane_animations false
        plugins {
            project-store location="yinz:project-store"
            agent-service location="yinz:agent-service"
        }
        load_plugins clear-defaults=true { "project-store"; "agent-service"; }
    "#;
    let root = test_env::init();
    let alpha = root.join("alpha");
    let subfolder = alpha.join("src");
    let unrelated = root.join("unrelated");
    std::fs::create_dir_all(&subfolder).unwrap();
    std::fs::create_dir_all(&unrelated).unwrap();
    let start = |cwd: &std::path::Path| {
        let yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config(config)
            .with_layout(LayoutInfo::Stringified(format!(
                "layout {{ pane cwd=\"{}\"; }}",
                cwd.display()
            )))
            .start();
        yinz.expect_pty_spawn().output(PROMPT);
        yinz.wait_until("shell ready", |grid| grid.contains("$"));
        yinz
    };
    let mut first = start(&unrelated);
    for (name, folder) in [("alpha", alpha), ("beta", root.join("beta"))] {
        let action = Projects::ProjectRemember {
            name: name.into(),
            root: folder.display().to_string(),
            worktree_dir: None,
        }
        .action()
        .unwrap();
        yinz_client::store_client::request(&first.session_name().into(), action, None).unwrap();
    }
    first.send_stdin(DO);
    first.wait_until("alphabetical fallback", |grid| {
        grid.contains("Project: alpha")
    });
    first.send_stdin(b"Switch current project\r");
    first.wait_until("project picker", |grid| grid.contains("beta"));
    first.send_stdin(b"beta\r");
    first.wait_until("explicit selection", |grid| grid.contains("Project: beta"));
    first.quit();
    let saved = root.join("data/last-project.json");
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "\"beta\"");

    let mut restarted = start(&unrelated);
    restarted.send_stdin(DO);
    restarted.wait_until("remembered project in a fresh session", |grid| {
        grid.contains("Project: beta")
    });
    restarted.quit();

    let mut in_project = start(&subfolder);
    in_project.send_stdin(DO);
    in_project.wait_until("focused subfolder overrides the saved project", |grid| {
        grid.contains("Project: alpha")
    });
    in_project.quit();
    assert_eq!(std::fs::read_to_string(saved).unwrap(), "\"alpha\"");
}
