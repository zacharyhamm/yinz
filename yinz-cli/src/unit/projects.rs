use super::*;
use crate::args::{CliAction, CliArgs, Command};
use clap::Parser;

fn parse(args: &[&str]) -> Result<Projects, clap::Error> {
    let argv: Vec<String> = ["yinz", "action"]
        .into_iter()
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect();
    // The full debug clap builder needs a larger stack (see args.rs tests).
    let parsed = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(move || CliArgs::try_parse_from(argv))
        .unwrap()
        .join()
        .unwrap()?;
    match parsed.command {
        Some(Command::Action(action)) => match *action {
            CliAction::Projects(command) => Ok(command),
            other => panic!("unexpected action: {other:?}"),
        },
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn project_actions_keep_the_store_protocol() {
    let cases: &[(&[&str], &str, Value, bool)] = &[
        (&["project-list"], "project_list", json!({}), true),
        (
            &["project-remember", "work", "--root", "/work"],
            "project_remember",
            json!({"name":"work","root":"/work"}),
            false,
        ),
        (
            &["project-forget", "work"],
            "project_forget",
            json!({"name":"work"}),
            false,
        ),
        (
            &["project-rename", "work", "new"],
            "project_rename",
            json!({"name":"work","new_name":"new"}),
            false,
        ),
        (
            &["project-merge", "work", "new"],
            "project_merge",
            json!({"source":"work","target":"new"}),
            false,
        ),
    ];
    for (argv, op, args, query) in cases {
        let Action::CliPipe {
            name,
            payload,
            plugin,
            launch_new,
            ..
        } = parse(argv).unwrap().action().unwrap()
        else {
            panic!("not a pipe")
        };
        assert_eq!(
            name.as_deref(),
            Some(if *query {
                "store_query"
            } else {
                "store_mutate"
            })
        );
        assert!(plugin.is_none() && !launch_new);
        assert_eq!(
            serde_json::from_str::<Value>(&payload.unwrap()).unwrap(),
            json!({"op":op,"args":args,"structured_reply":true})
        );
    }
}

#[test]
fn removed_actions_are_unknown() {
    for action in [
        "todo-list",
        "todo-search",
        "todo-add",
        "todo-edit",
        "todo-done",
        "todo-delete",
        "todo-move",
        "note-list",
        "note-search",
        "note-get",
        "note-add",
        "note-edit",
        "note-archive",
        "note-delete",
        "note-link",
    ] {
        assert_eq!(
            parse(&[action]).unwrap_err().kind(),
            clap::error::ErrorKind::InvalidSubcommand
        );
    }
}
