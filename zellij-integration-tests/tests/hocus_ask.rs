#![cfg(unix)]

//! Lifecycle coverage for the `hocus-ask` popup.
//!
//! The popup's layout maths and its phase machine are unit-tested inside the
//! crate, and hocus's queue is unit-tested inside hocus. What neither can prove
//! is the part welded to the host, and it is the part the whole design rests
//! on: that a broadcast pipe actually reaches the running hocus, that hocus's
//! reply finds its way back to the pane that asked, and that closing the popup
//! really does put the cursor back where it came from.
//!
//! No API is involved. hocus is configured against a dead local port with its
//! key taken from `PATH`, so its config resolves — which is all these tests
//! need — while any request it makes is refused on the loopback interface.

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, PROMPT, TERMINAL_SIZE};

const POPUP_TITLE: &str = "ask hocus";
const POPUP_PROMPT: &str = "ask> ";

/// Just the popup, with no hocus anywhere to hear it.
///
/// `load_plugins clear-defaults=true` keeps the default background plugins from
/// spawning work of their own while the test watches for pty spawns.
const ASK_ONLY: &str = r#"plugins {
  hocus-ask location="zellij:hocus-ask"
}
load_plugins clear-defaults=true {
}
"#;

/// The popup and a hocus for it to reach.
///
/// `zellij:` rather than `file:` even though the two point at the same wasm:
/// neither plugin asks for `MessageAndLaunchOtherPlugins`, relying on a builtin
/// being granted it implicitly, and a `file:` load is not a builtin — every
/// pipe between them would be denied. `shared true` because hocus warns without
/// it, and the warning would be the first thing on screen.
///
/// `api_key_env "PATH"` is the trick that makes hocus's config resolve without
/// a secret: there is deliberately no KDL key for the API key, only for which
/// variable holds it, and `PATH` is one the test process is guaranteed to have.
/// `base_url` then points the resolved endpoint at a port nothing listens on,
/// so the request hocus makes is refused locally rather than leaving the
/// machine. The question is in the transcript by then either way.
const WITH_HOCUS: &str = r#"plugins {
  hocus location="zellij:hocus" {
    shared true
    api_key_env "PATH"
    base_url "http://127.0.0.1:1"
    tools "false"
  }
  hocus-ask location="zellij:hocus-ask"
}
load_plugins clear-defaults=true {
}
"#;

/// hocus in a pane rather than in `load_plugins`, because the assertions are
/// made by reading its transcript off the grid. The second pane is an ordinary
/// shell, so the popup has something other than a plugin to cover.
///
/// `focus=true` on the plugin because hocus's permission prompt is drawn in its
/// own pane and answered with a keystroke; without it the `y` goes to the shell.
const HOCUS_AND_SHELL: &str = r#"layout {
    pane focus=true {
        plugin location="hocus"
    }
    pane
}
"#;

/// A session with one shell pane and the popup available.
fn start_without_hocus() -> TestSession {
    let zellij = TestRunner::new(TERMINAL_SIZE).with_config(ASK_ONLY).start();
    let shell = zellij.expect_pty_spawn();
    shell.output(PROMPT);
    zellij.wait_for_app_load();
    zellij
}

fn open_popup(zellij: &TestSession) {
    zellij.send_stdin(&keys::alt('a'));
    zellij.wait_until("the popup is up", |grid_snapshot| {
        grid_snapshot.contains(POPUP_PROMPT)
    });
}

/// Type into the popup and send it.
///
/// Pasted rather than typed: a burst of keystrokes outruns the render and the
/// line arrives half finished. One `PastedText` event does not.
fn ask(zellij: &TestSession, question: &str) {
    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(question.as_bytes());
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the question is in the box", |grid_snapshot| {
        grid_snapshot.contains(question)
    });
    zellij.send_stdin(&keys::ENTER);
}

#[test]
fn the_popup_opens_small_and_takes_a_question() {
    let mut zellij = start_without_hocus();
    open_popup(&zellij);

    let snapshot = zellij.snapshot();
    assert!(
        snapshot.contains(POPUP_TITLE),
        "the popup should name itself in its frame"
    );

    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(b"why is the build red?");
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the question is on screen", |grid_snapshot| {
        grid_snapshot.contains("why is the build red?")
    });

    zellij.quit();
}

/// The whole feature in one assertion, and the only one that exercises the real
/// broadcast: hocus is reached without being addressed by url, takes the
/// question, and answers the pane that asked — which is what closes the popup.
#[test]
fn a_submitted_question_reaches_hocus_and_dismisses_the_popup() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(WITH_HOCUS)
        .with_layout(LayoutInfo::Stringified(HOCUS_AND_SHELL.to_owned()))
        .start();
    let shell = zellij.expect_pty_spawn();
    shell.output(PROMPT);

    zellij.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    open_popup(&zellij);
    ask(&zellij, "why is the build red?");

    zellij.wait_until("the popup is dismissed", |grid_snapshot| {
        !grid_snapshot.contains(POPUP_PROMPT)
    });
    zellij.wait_until("the question is in hocus's transcript", |grid_snapshot| {
        grid_snapshot.contains("why is the build red?")
    });

    zellij.quit();
}

/// The claim the popup makes no attempt to implement: closing the last floating
/// pane is what puts the user back in the pane they came from. Asserted against
/// the real host rather than against a reading of `Tab::close_pane`.
#[test]
fn dismissing_the_popup_returns_focus_to_the_pane_it_covered() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE).with_config(ASK_ONLY).start();
    let shell = zellij.expect_pty_spawn();
    shell.output(PROMPT);
    zellij.wait_for_app_load();

    open_popup(&zellij);
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("the popup is gone", |grid_snapshot| {
        !grid_snapshot.contains(POPUP_PROMPT)
    });

    // Focus lands back on the shell for one instant somewhere in the teardown,
    // and a keystroke issued before it does is genuinely lost. Re-offer it
    // rather than assert on a single moment.
    let mut reached_the_shell = false;
    for _ in 0..50 {
        zellij.send_stdin(b"q");
        if !shell.stdin_bytes().is_empty() {
            reached_the_shell = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        reached_the_shell,
        "typing after the popup closes should reach the shell it covered"
    );

    zellij.quit();
}

/// A broadcast into a session with no hocus in it looks exactly like a
/// successful one from the popup's side. This is the test that the
/// acknowledgement is doing real work rather than being a formality.
#[test]
fn a_question_nobody_hears_is_reported_rather_than_swallowed() {
    let mut zellij = start_without_hocus();
    open_popup(&zellij);
    ask(&zellij, "why is the build red?");

    zellij.wait_until("the popup says nothing heard it", |grid_snapshot| {
        grid_snapshot.contains("hocus is not running")
    });
    let snapshot = zellij.snapshot();
    assert!(
        snapshot.contains("why is the build red?"),
        "the question should still be there to retry or copy"
    );

    zellij.quit();
}

/// The popup closes rather than hiding, so the next one starts empty. A hidden
/// instance would be found and re-focused by `LaunchOrFocusPlugin` with its old
/// text still in it.
#[test]
fn reopening_the_popup_gives_a_fresh_prompt() {
    let mut zellij = start_without_hocus();
    open_popup(&zellij);

    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(b"abandoned question");
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the question is on screen", |grid_snapshot| {
        grid_snapshot.contains("abandoned question")
    });

    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("the popup is gone", |grid_snapshot| {
        !grid_snapshot.contains("abandoned question")
    });

    open_popup(&zellij);
    let snapshot = zellij.snapshot();
    assert!(
        !snapshot.contains("abandoned question"),
        "a reopened popup should be empty, not the last one focused again"
    );

    zellij.quit();
}
