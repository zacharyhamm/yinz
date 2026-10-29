#![cfg(unix)]

use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, split_right_and_wait_for_prompt, start_yinz, Size,
    TestRunner,
};
use yinz_types::input::options::PaneFrameStyle;

const FULL_FRAME_CORNER: &str = "┌";

fn ctrl_y() -> [u8; 1] {
    [0x19]
}

#[test]
fn set_pane_frame_style_through_the_cli() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.wait_until("lone titles pane shows its title", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("Pane #1")
            && !grid_snapshot.contains(FULL_FRAME_CORNER)
    });

    let exit_code = yinz.run_cli_action(CliAction::SetPaneFrameStyle {
        style: PaneFrameStyle::Full,
    });
    assert_eq!(exit_code, 0, "set-pane-frame-style exited cleanly");

    yinz.wait_until(
        "full frames drawn after switching to full",
        |grid_snapshot| {
            grid_snapshot.contains(FULL_FRAME_CORNER) && grid_snapshot.contains("Pane #1")
        },
    );

    let exit_code = yinz.run_cli_action(CliAction::SetPaneFrameStyle {
        style: PaneFrameStyle::Titles,
    });
    assert_eq!(exit_code, 0, "set-pane-frame-style exited cleanly");

    yinz.wait_until(
        "title remains after switching back to titles",
        |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && !grid_snapshot.contains(FULL_FRAME_CORNER)
                && grid_snapshot.contains("Pane #1")
        },
    );

    yinz.quit();
}

#[test]
fn set_pane_frame_style_none_through_the_cli() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.wait_until(
        "two titled panes start without full frames",
        |grid_snapshot| {
            grid_snapshot.contains("Pane #1")
                && grid_snapshot.contains("Pane #2")
                && !grid_snapshot.contains(FULL_FRAME_CORNER)
        },
    );

    let exit_code = yinz.run_cli_action(CliAction::SetPaneFrameStyle {
        style: PaneFrameStyle::None,
    });
    assert_eq!(exit_code, 0, "set-pane-frame-style none exited cleanly");

    yinz.wait_until(
        "titles and frames both removed in none mode",
        |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && !grid_snapshot.contains("Pane #1")
                && !grid_snapshot.contains("Pane #2")
                && !grid_snapshot.contains(FULL_FRAME_CORNER)
        },
    );

    let exit_code = yinz.run_cli_action(CliAction::SetPaneFrameStyle {
        style: PaneFrameStyle::Titles,
    });
    assert_eq!(exit_code, 0, "set-pane-frame-style titles exited cleanly");

    yinz.wait_until("pane titles return in titles mode", |grid_snapshot| {
        grid_snapshot.contains("Pane #1") && !grid_snapshot.contains(FULL_FRAME_CORNER)
    });

    yinz.quit();
}

#[test]
fn set_pane_frame_style_through_a_keybinding() {
    let mut yinz = TestRunner::new(Size {
        cols: 120,
        rows: 24,
    })
    .with_config("keybinds {\n normal {\n  bind \"Ctrl y\" { SetPaneFrameStyle \"full\"; }\n }\n}")
    .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.wait_until("lone titles pane shows its title", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("Pane #1")
            && !grid_snapshot.contains(FULL_FRAME_CORNER)
    });

    yinz.send_stdin(&ctrl_y());

    yinz.wait_until(
        "keybinding switched the pane to full frames",
        |grid_snapshot| {
            grid_snapshot.contains(FULL_FRAME_CORNER) && grid_snapshot.contains("Pane #1")
        },
    );

    yinz.quit();
}
