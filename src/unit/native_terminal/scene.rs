//! The native scene from a real session: the headless capture records every production frame,
//! a live `TerminalState` applies them with the agreement the capture negotiated, and the test
//! drives pane and overlay transitions through `yinz action` while the capture runs.
use super::*;
use yinz_types::render_negotiation as caps;
use yinz_types::structured_render::{PaneRect, PANE_FLOATING, PANE_POPUP, PANE_UNDER_FLOATING};

const SESSION: &str = "native-scene";
const LEFT: &str = "scene-left";
const RIGHT: &str = "scene-right";
const FLOAT: &str = "scene-float";
const TOAST: &str = "scene-toast";

/// Every frame the capture wrote, applied to a viewer in live mode: frames must arrive in
/// sequence, start with a full repaint, and carry only negotiated sections, and replacement
/// geometry must leave every image an owning pane.
struct LiveRecording {
    state: Option<TerminalState>,
    frames: usize,
    applied_bytes: usize,
}

impl LiveRecording {
    fn new() -> Self {
        LiveRecording {
            state: None,
            frames: 0,
            applied_bytes: 0,
        }
    }

    fn read(&mut self, path: &Path) {
        let Ok(bytes) = fs::read(path) else { return };
        let mut rest = &bytes[self.applied_bytes..];
        while let Some(length) = rest.get(..4) {
            let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
            let Some(frame) = rest.get(4..4 + length) else {
                break;
            };
            let header = *decode(frame).unwrap().header();
            let state = self.state.get_or_insert_with(|| {
                let mut state = TerminalState::new(header.rows.into(), header.cols.into());
                // The headless capture offers `Offer::reader` without a shared media proof.
                let agreement = caps::Offer::reader(None).select(None).unwrap();
                state.negotiate(agreement, None);
                state
            });
            if let Err(error) = state.apply_frame(frame) {
                panic!("the live viewer rejected frame {}: {error:?}", header.seq);
            }
            self.frames += 1;
            rest = &rest[4 + length..];
        }
        self.applied_bytes = bytes.len() - rest.len();
    }

    fn state(&self) -> Option<&TerminalState> {
        self.state.as_ref()
    }

    fn text(&self) -> String {
        let Some(state) = self.state() else {
            return String::new();
        };
        let size = state.size();
        (0..size.rows)
            .map(|row| {
                (0..size.cols)
                    .map(|col| state.screen().cell(row, col).character())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn column_of(&self, marker: &str) -> Option<usize> {
        self.text()
            .lines()
            .find_map(|line| line.find(marker).map(|byte| line[..byte].chars().count()))
    }

    fn panes(&self) -> Vec<PaneRect> {
        self.state()
            .map(|state| state.geometry().panes.clone())
            .unwrap_or_default()
    }

    fn tiled(&self) -> Vec<PaneRect> {
        self.panes()
            .into_iter()
            .filter(|rect| rect.flags & (PANE_FLOATING | PANE_POPUP) == 0)
            .collect()
    }

    fn floats(&self) -> Vec<PaneRect> {
        self.panes()
            .into_iter()
            .filter(|rect| rect.floating())
            .collect()
    }

    fn popups(&self) -> Vec<PaneRect> {
        self.panes()
            .into_iter()
            .filter(|rect| rect.popup())
            .collect()
    }

    /// The cursor is visible inside the focused pane's content: never stale after a change.
    fn cursor_in_focused_pane(&self) -> bool {
        let Some(state) = self.state() else {
            return false;
        };
        let (row, col) = state.cursor_position();
        let focused: Vec<_> = self
            .panes()
            .into_iter()
            .filter(|rect| rect.focused() && !rect.popup())
            .collect();
        state.cursor_is_visible()
            && focused.len() == 1
            && focused[0].content_contains(col as u16, row as u16)
    }

    /// Floating rects and `PANE_UNDER_FLOATING` on every tiled rect come and go together.
    fn under_floating_matches_floats(&self) -> bool {
        let floats = !self.floats().is_empty();
        let tiled = self.tiled();
        !tiled.is_empty()
            && tiled
                .iter()
                .all(|rect| (rect.flags & PANE_UNDER_FLOATING != 0) == floats)
    }
}

fn scene_layout() -> String {
    let pane = |name: &str, marker: &str| {
        format!(
            r#"pane name="{name}" command="/bin/sh" {{ args "-c" "stty -echo; printf {marker}; exec /bin/sh"; }}"#
        )
    };
    format!(
        "layout {{\n    pane split_direction=\"vertical\" {{\n        {}\n        {}\n    }}\n    floating_panes {{\n        {}\n    }}\n}}\n",
        pane("left", LEFT),
        pane("right", RIGHT),
        pane("float", FLOAT),
    )
}

/// The terminal ids of the named tiled panes, from the session itself.
fn pane_id(fixture: &NativeFixture, title: &str) -> u32 {
    let output = fixture
        .command()
        .args(["--session", SESSION, "action", "list-panes", "--json"])
        .output()
        .unwrap();
    let panes: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    panes
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["title"] == title && pane["is_plugin"] != true)
        .and_then(|pane| pane["id"].as_u64())
        .unwrap_or_else(|| panic!("no pane titled {title}: {panes}")) as u32
}

fn start(fixture: &NativeFixture, create: bool, path: &Path) -> Child {
    fixture
        .terminal(SESSION, create, path)
        .arg("--headless")
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}

/// Wait for the recorded scene. The bound leaves the whole test inside nextest's kill, and a
/// timeout reports what the viewer last held.
fn wait(
    child: &mut Child,
    recording: &mut LiveRecording,
    path: &Path,
    what: &str,
    ready: impl Fn(&LiveRecording) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        recording.read(path);
        if ready(recording) {
            return;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("native client exited with {status} before {what}");
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            let cursor = recording
                .state()
                .map(|state| (state.cursor_is_visible(), state.cursor_position()));
            panic!(
                "timed out waiting for {what} after {} frames\ncursor (visible, (row, col)) = {cursor:?}\npanes = {:#?}\n{}",
                recording.frames,
                recording.panes(),
                recording.text()
            );
        }
        std::thread::park_timeout(Duration::from_millis(20));
    }
}

#[test]
fn native_scene_tracks_panes_overlays_and_cursor_across_transitions_and_reattach() {
    let fixture = NativeFixture::new();
    fs::write(&fixture.layout, scene_layout()).unwrap();
    fs::write(
        &fixture.config,
        // Classic stacks, whose collapsed members keep one-row titles on screen; the stack
        // list instead suppresses them, leaving no cells or rect to check.
        format!(
            "{}notification_timeout 1\nstacked_pane_list false\n",
            native_config(NATIVE_PADDING)
        ),
    )
    .unwrap();
    let path = fixture.fixture.path().join("scene.frames");
    let mut child = start(&fixture, true, &path);
    let mut recording = LiveRecording::new();
    wait(
        &mut child,
        &mut recording,
        &path,
        "both tiled markers",
        |r| {
            let text = r.text();
            text.contains(LEFT)
                && text.contains(RIGHT)
                && r.tiled().len() == 2
                && r.under_floating_matches_floats()
                && r.cursor_in_focused_pane()
        },
    );

    // Floating layer: whichever way it started, toggling flips rects, flags and cells together.
    for _ in 0..2 {
        let shown = !recording.floats().is_empty();
        fixture.action(SESSION, &["toggle-floating-panes"]);
        wait(
            &mut child,
            &mut recording,
            &path,
            if shown {
                "floating layer hidden"
            } else {
                "floating layer shown"
            },
            |r| {
                r.floats().is_empty() == shown
                    && r.under_floating_matches_floats()
                    && r.text().contains(FLOAT) != shown
                    && r.cursor_in_focused_pane()
            },
        );
    }
    if !recording.floats().is_empty() {
        fixture.action(SESSION, &["toggle-floating-panes"]);
        wait(&mut child, &mut recording, &path, "floats hidden", |r| {
            r.floats().is_empty() && !r.text().contains(FLOAT) && r.cursor_in_focused_pane()
        });
    }
    assert_eq!(recording.panes().len(), 2);

    // Fullscreen: one rect and one marker, then both again with nothing stale.
    fixture.action(SESSION, &["toggle-fullscreen"]);
    wait(&mut child, &mut recording, &path, "fullscreen", |r| {
        let text = r.text();
        r.panes().len() == 1
            && (text.contains(LEFT) != text.contains(RIGHT))
            && r.cursor_in_focused_pane()
    });
    fixture.action(SESSION, &["toggle-fullscreen"]);
    wait(&mut child, &mut recording, &path, "fullscreen off", |r| {
        let text = r.text();
        r.panes().len() == 2
            && text.contains(LEFT)
            && text.contains(RIGHT)
            && r.cursor_in_focused_pane()
    });

    // Moving the focused pane swaps the markers' columns; each still appears once.
    let (left, right) = (
        recording.column_of(LEFT).unwrap(),
        recording.column_of(RIGHT).unwrap(),
    );
    fixture.action(SESSION, &["move-pane"]);
    wait(&mut child, &mut recording, &path, "panes swapped", |r| {
        r.column_of(LEFT) == Some(right)
            && r.column_of(RIGHT) == Some(left)
            && r.cursor_in_focused_pane()
    });
    let text = recording.text();
    assert_eq!(text.matches(LEFT).count(), 1, "{text}");
    assert_eq!(text.matches(RIGHT).count(), 1, "{text}");

    // Stacking collapses one member to a title row, which keeps its rect.
    let ids = [pane_id(&fixture, "left"), pane_id(&fixture, "right")];
    fixture.action(
        SESSION,
        &[
            "stack-panes",
            "--",
            &format!("terminal_{}", ids[0]),
            &format!("terminal_{}", ids[1]),
        ],
    );
    wait(
        &mut child,
        &mut recording,
        &path,
        "a collapsed title rect",
        |r| {
            let tiled = r.tiled();
            let text = r.text();
            tiled.len() == 2
                && tiled.iter().filter(|rect| rect.rows == 1).count() == 1
                && (text.contains(LEFT) != text.contains(RIGHT))
                && r.cursor_in_focused_pane()
        },
    );

    // A toast is a popup while it shows, and leaves no rect or cell behind once it expires.
    fixture.action(SESSION, &["notify", TOAST]);
    wait(&mut child, &mut recording, &path, "the toast popup", |r| {
        r.popups().len() == 1
            && r.panes()
                .last()
                .is_some_and(|rect| rect.popup() && !rect.focused())
            && r.text().contains(TOAST)
    });
    wait(
        &mut child,
        &mut recording,
        &path,
        "the toast expires",
        |r| r.popups().is_empty() && !r.text().contains(TOAST) && r.cursor_in_focused_pane(),
    );
    let scene = recording.panes();
    fixture.action(SESSION, &["detach"]);
    assert_success(child.wait_with_output().unwrap());
    recording.read(&path);
    assert!(recording.frames > 0);

    // Reattach: a new attachment rebuilds the same scene and cursor from a fresh sequence.
    let path = fixture.fixture.path().join("scene-reattach.frames");
    let mut child = start(&fixture, false, &path);
    let mut reattached = LiveRecording::new();
    wait(
        &mut child,
        &mut reattached,
        &path,
        "the reattached scene",
        |r| r.panes() == scene && r.cursor_in_focused_pane(),
    );
    fixture.action(SESSION, &["detach"]);
    assert_success(child.wait_with_output().unwrap());
}
