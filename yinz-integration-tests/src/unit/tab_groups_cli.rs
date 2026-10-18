use crate::{claim_first_terminal_and_wait_for_prompt, start_yinz, TestSession};
use serde_json::Value;
use yinz_cli::args::CliAction;

fn query(session: &TestSession, action: CliAction) -> Value {
    let (status, output) = session.run_cli_action_reporting(action);
    assert_eq!(status, 0, "{output}");
    serde_json::from_str(output.trim()).unwrap()
}

#[test]
fn tab_groups_cli_lifecycle_and_errors() {
    let mut session = start_yinz();
    let _first = claim_first_terminal_and_wait_for_prompt(&session);
    let original = query(&session, CliAction::CurrentTabGroupInfo { json: true });
    let (status, output) = session.run_cli_action_reporting(CliAction::NewTabGroup {
        name: Some("CLI work".into()),
    });
    assert_eq!(status, 0, "{output}");
    let id: u32 = output
        .trim()
        .parse()
        .expect("creation prints only the group ID");
    let current = query(&session, CliAction::CurrentTabGroupInfo { json: true });
    assert_eq!(current["id"], id);
    assert_eq!(current["name"], "CLI work");
    assert_eq!(current["active"], true);
    assert_eq!(current["tab_ids"].as_array().unwrap().len(), 1);
    let (status, output) =
        session.run_cli_action_reporting(CliAction::CurrentTabGroupInfo { json: false });
    assert_eq!(status, 0);
    assert_eq!(output.trim(), format!("name: CLI work\nid: {id}"));
    let (status, output) =
        session.run_cli_action_reporting(CliAction::ListTabGroups { json: false });
    assert_eq!(status, 0);
    assert!(output.contains("ID  NAME  ACTIVE  TAB_IDS"), "{output}");
    assert!(
        output.contains(&format!("{id}  CLI work  true  ")),
        "{output}"
    );

    assert_eq!(
        session.run_cli_action(CliAction::RenameTabGroup {
            name: "CLI renamed".into(),
            group_id: None,
        }),
        0
    );
    assert_eq!(session.run_cli_action(CliAction::GoToPreviousTabGroup), 0);
    assert_eq!(
        query(&session, CliAction::CurrentTabGroupInfo { json: true })["id"],
        original["id"]
    );
    let (status, output) = session.run_cli_action_reporting(CliAction::GoToTabGroupName {
        name: "CLI renamed".into(),
        create: true,
    });
    assert_eq!((status, output.trim()), (0, ""));
    assert_eq!(
        query(&session, CliAction::CurrentTabGroupInfo { json: true })["id"],
        id
    );
    assert_eq!(
        query(&session, CliAction::ListTabGroups { json: true })
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let (status, output) = session.run_cli_action_reporting(CliAction::GoToTabGroupName {
        name: "CLI created".into(),
        create: true,
    });
    assert_eq!(status, 0, "{output}");
    let created: u32 = output.trim().parse().unwrap();
    assert_ne!(created, id);
    assert_eq!(
        session.run_cli_action(CliAction::CloseTabGroup { group_id: None }),
        0
    );
    assert_eq!(
        session.run_cli_action(CliAction::GoToTabGroupById { id }),
        0
    );
    assert_eq!(
        session.run_cli_action(CliAction::CloseTabGroupById { id }),
        0
    );
    assert_eq!(
        query(&session, CliAction::ListTabGroups { json: true })
            .as_array()
            .unwrap()
            .len(),
        1
    );

    for action in [
        CliAction::GoToTabGroupById { id },
        CliAction::RenameTabGroupById {
            id,
            name: "gone".into(),
        },
        CliAction::CloseTabGroupById { id },
        CliAction::GoToTabGroupName {
            name: "missing".into(),
            create: false,
        },
    ] {
        let (status, output) = session.run_cli_action_reporting(action);
        assert_ne!(status, 0, "{output}");
        assert!(output.contains("tab group"), "{output}");
    }
    session.quit();
}
