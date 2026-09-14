use crate::pty::{held_command_on_exit, terminal_exit_instruction};
use crate::screen::ScreenInstruction;
use crate::PaneId;
use zellij_types::input::command::RunCommand;
use zellij_types::internal_ids::TerminalId;

#[test]
fn terminal_exit_policy_preserves_command_and_optional_status() {
    let pane_id = PaneId::Terminal(TerminalId::new(42));
    for hold_on_close in [false, true] {
        let command = RunCommand {
            command: "worker".into(),
            hold_on_close,
            ..Default::default()
        };
        for child_exit_status in [None, Some(None), Some(Some(3))] {
            assert_eq!(
                held_command_on_exit(&command, child_exit_status),
                (hold_on_close && child_exit_status.is_some()).then(|| command.clone())
            );
        }
        for exit_status in [None, Some(3)] {
            // The caller's policy wins even when the received command disagrees.
            for policy in [false, true] {
                match terminal_exit_instruction(pane_id, exit_status, command.clone(), policy) {
                    ScreenInstruction::HoldPane(id, status, held) => {
                        assert!(policy);
                        assert_eq!((id, status, held), (pane_id, exit_status, command.clone()));
                    },
                    ScreenInstruction::ClosePane(id, client, completion, status) => {
                        assert!(!policy);
                        assert_eq!((id, status), (pane_id, exit_status));
                        assert!(client.is_none() && completion.is_none());
                    },
                    instruction => panic!("unexpected exit instruction: {instruction:?}"),
                }
            }
        }
    }
}
