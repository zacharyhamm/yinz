use crate::args::{CliArgs, Command};
use clap::Parser;
use yinz_types::{input::actions::Action, internal_ids::TabNumber};

fn parse(args: &[&str]) -> Result<CliArgs, clap::Error> {
    let args: Vec<String> = ["yinz", "action"]
        .into_iter()
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect();
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || CliArgs::try_parse_from(args))
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn tab_group_commands_parse_and_convert() {
    let cases: Vec<(Vec<&str>, Action)> = vec![
        (
            vec!["new-tab-group"],
            Action::NewNamedTabGroup { name: None },
        ),
        (
            vec!["new-tab-group", "-n", "work"],
            Action::NewNamedTabGroup {
                name: Some("work".into()),
            },
        ),
        (
            vec!["list-tab-groups", "--json"],
            Action::ListTabGroups { output_json: true },
        ),
        (
            vec!["current-tab-group-info"],
            Action::CurrentTabGroupInfo { output_json: false },
        ),
        (
            vec!["go-to-tab-group-by-id", "0"],
            Action::SwitchTabGroup { id: 0 },
        ),
        (
            vec!["go-to-tab-group-name", "work", "--create"],
            Action::GoToTabGroupName {
                name: "work".into(),
                create: true,
            },
        ),
        (
            vec!["rename-tab-group", "work"],
            Action::RenameCurrentTabGroup {
                name: "work".into(),
            },
        ),
        (
            vec!["rename-tab-group", "work", "--group-id", "3"],
            Action::RenameTabGroup {
                id: 3,
                name: "work".into(),
            },
        ),
        (
            vec!["rename-tab-group-by-id", "3", "work"],
            Action::RenameTabGroup {
                id: 3,
                name: "work".into(),
            },
        ),
        (vec!["close-tab-group"], Action::CloseCurrentTabGroup),
        (
            vec!["close-tab-group", "--group-id", "3"],
            Action::CloseTabGroup { id: 3 },
        ),
        (
            vec!["close-tab-group-by-id", "3"],
            Action::CloseTabGroup { id: 3 },
        ),
        (vec!["go-to-next-tab-group"], Action::GoToNextTabGroup),
        (
            vec!["go-to-previous-tab-group"],
            Action::GoToPreviousTabGroup,
        ),
        (
            vec!["go-to-tab-in-group", "2"],
            Action::GoToTabInGroup {
                index: TabNumber::new(2),
            },
        ),
        (
            vec!["move-pane-to-next-tab-group"],
            Action::MovePaneToNextTabGroup,
        ),
        (
            vec!["move-pane-to-previous-tab-group"],
            Action::MovePaneToPreviousTabGroup,
        ),
    ];
    for (args, expected) in cases {
        let parsed = parse(&args).unwrap();
        let Some(Command::Action(action)) = parsed.command else {
            panic!("expected action");
        };
        let actions =
            crate::actions::actions_from_cli(*action, Box::new(|| "/tmp".into()), None).unwrap();
        assert_eq!(actions, vec![expected]);
    }
}

#[test]
fn tab_group_ids_reject_negative_overflow_and_text() {
    for command in ["go-to-tab-group-by-id", "close-tab-group-by-id"] {
        for invalid in ["-1", "4294967296", "work"] {
            assert!(parse(&[command, invalid]).is_err());
        }
    }
}
