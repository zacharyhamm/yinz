#![cfg(unix)]

use yinz_integration_tests::{LayoutInfo, TestRunner, TERMINAL_SIZE};
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
use yinz_types::input::actions::Action;
use yinz_types::pane_id::PaneId;

#[test]
fn waiting_agent_bells_render_in_both_tab_bars_pane_titles_and_stack_labels() {
    for bar in ["tab-bar", "compact-bar"] {
        for panes in [
            "pane name=\"alpha\"",
            "pane name=\"alpha\"; pane name=\"beta\"",
            "pane stacked=true { pane name=\"alpha\"; pane name=\"beta\"; }",
        ] {
            let single = !panes.contains("beta");
            let layout = format!(
                r#"layout {{
                pane size=1 borderless=true {{ plugin location="{bar}"; }}
                {panes}
            }}"#
            );
            let mut yinz = TestRunner::new(TERMINAL_SIZE)
                .with_config("pane_frame_style \"titles\"\nload_plugins clear-defaults=true {}")
                .with_layout(LayoutInfo::Stringified(layout))
                .start();
            let first = yinz.expect_pty_spawn();
            first.output(b"first-ready\r\n");
            if !single {
                yinz.expect_pty_spawn().output(b"second-ready\r\n");
            }
            yinz.wait_until("pane titles are rendered", |snapshot| {
                snapshot.contains("alpha")
            });
            let pane_id = PaneId::Terminal(first.terminal_id());
            let waiting = AgentInfo {
                kind: "codex".into(),
                state: AgentState::Waiting,
                source: AgentSource::Reported,
                detail: None,
                waiting_on: None,
                wait_needs_a_person: true,
                cwd: None,
                session_id: None,
                run_id: None,
            };
            let (code, output) = yinz.run_actions_reporting(vec![Action::SetPaneAgent {
                pane_id,
                agent: Some(waiting.clone()),
            }]);
            assert_eq!(code, 0, "{output}");
            let bell_title = format!("🔔 [{}] alpha", first.terminal_id());
            yinz.wait_until(
                "waiting bells reach the tab bar and visible pane title",
                |snapshot| {
                    let lines = snapshot.lines();
                    lines[0].contains('🔔')
                        && if single {
                            lines[0].contains(&bell_title)
                                && snapshot.text.matches('🔔').count() == 1
                        } else {
                            lines.iter().skip(1).any(|line| line.contains(&bell_title))
                        }
                },
            );
            // The action is for the attached client, which may already be focused on alpha.
            let (code, output) =
                yinz.run_actions_reporting(vec![Action::FocusPaneByPaneId { pane_id }]);
            assert!(
                code == 0 || output.contains("is already focused"),
                "{code}: {output}"
            );
            let (code, output) = yinz.run_actions_reporting(vec![Action::RenameTerminalPane {
                pane_id: first.terminal_id(),
                name: "long-agent-name-".repeat(8).into_bytes(),
            }]);
            assert_eq!(code, 0, "{output}");
            yinz.send_stdin(b"y");
            first.wait_for_stdin("answer reaches the waiting pane", |bytes| {
                bytes.contains(&b'y')
            });
            yinz.wait_until(
                "focus and typing preserve bells on truncated titles",
                |snapshot| {
                    let lines = snapshot.lines();
                    lines[0].contains('🔔')
                        && snapshot.contains("long-agent")
                        && if single {
                            snapshot.text.matches('🔔').count() == 1
                        } else {
                            lines
                                .iter()
                                .skip(1)
                                .any(|line| line.contains('🔔') && line.contains("long-agent"))
                        }
                },
            );
            let (code, output) = yinz.run_actions_reporting(vec![Action::SetPaneAgent {
                pane_id,
                agent: Some(AgentInfo {
                    state: AgentState::Busy,
                    ..waiting
                }),
            }]);
            assert_eq!(code, 0, "{output}");
            yinz.wait_until("ending the wait clears every bell", |snapshot| {
                snapshot.contains("long-agent") && !snapshot.contains("🔔")
            });
            yinz.quit();
        }
    }
}
