//! A fuzzy file finder for the project of the focused pane.
//!
//! One keypress puts fzf in a floating pane rooted at the git toplevel of wherever the
//! user currently is. Picking a file replaces fzf *in that same pane* with an editor or
//! a viewer chosen by file type, and quitting that takes the pane with it. The overlay
//! appears, does one job, and leaves nothing behind.
//!
//! The plugin itself is never visible: it hides on load and acts purely as the state
//! machine driving the pane it opened.
//!
//! ## Why the title names a tree the script also resolves
//!
//! The overlay looks the same in every project, so its frame title says which tree fzf is
//! listing: the git root's folder and its branch, or the plain folder outside a checkout.
//! The script resolves that root too, but it does so a process away and after the pane is
//! already named, so `project` walks for `.git` through the `/host` mount instead —
//! synchronously, in time for the one rename.
//!
//! ## Why a shell script and a temp file
//!
//! Plugins cannot read a command pane's stdout — there is no such API — so fzf's
//! selection comes back through a file the plugin reads via its `/host` mount.
//!
//! ## Why the pane survives fzf exiting
//!
//! Every plugin-opened command pane is created with `hold_on_close = true` (hardcoded
//! in `zellij-server/src/plugins/zellij_exports.rs`). So when fzf exits the pane is
//! *held* rather than closed, `Event::CommandPaneExited` arrives while the pane still
//! exists, and it can be replaced in place. That is what makes "the same floating pane"
//! literally true rather than a new pane at the same coordinates.

mod opener;
mod project;
mod script;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use opener::{opener_for, Opener};
use zellij_tile::prelude::*;

/// Config keys for the floating pane's geometry, and the defaults if unset.
///
/// `DEFAULT_X` is empty rather than a percentage: a fixed `DEFAULT_WIDTH` needs `x` left
/// unset so `PaneGeom::apply_floating_pane_position` auto-centers it against whatever the
/// viewport turns out to be, rather than pinning it to a percentage that assumed the old
/// percentage-based width.
const DEFAULT_X: &str = "";
const DEFAULT_Y: &str = "15%";
const DEFAULT_WIDTH: &str = "120";
const DEFAULT_HEIGHT: &str = "70%";

/// Frame title while the picker is up, before the tree being searched is named.
const FINDER_PANE_TITLE: &str = "Find file";

/// Where `change_host_folder` puts the real filesystem root for this plugin.
const HOST_MOUNT: &str = "/host";

/// Which pane the plugin is currently driving, and what stage it is at.
#[derive(Debug, Default, PartialEq, Eq)]
enum Stage {
    /// Nothing opened yet, or the flow is over and we are on our way out.
    #[default]
    Idle,
    /// fzf is running in `pane_id`.
    Finding { pane_id: u32 },
    /// An editor or viewer is running in `pane_id`.
    Opened { pane_id: u32 },
}

impl Stage {
    fn pane_id(&self) -> Option<u32> {
        match self {
            Stage::Idle => None,
            Stage::Finding { pane_id } | Stage::Opened { pane_id } => Some(*pane_id),
        }
    }
}

#[derive(Default)]
struct State {
    /// The plugin's whole KDL block, kept so `PluginConfigurationChanged` can refresh it.
    config: BTreeMap<String, String>,
    /// Where the finder script writes the selected path.
    out_path: PathBuf,
    /// This plugin's own pane id, used to notice when focus lands back on it.
    plugin_id: u32,
    stage: Stage,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        // No `request_permission` here, deliberately. A builtin plugin — one loaded from
        // a `zellij:` location — is granted every gated call unconditionally by
        // `check_command_permission`. Asking anyway would strand this plugin: the
        // permission dialog is drawn *in the requesting plugin's pane*, and this plugin
        // hides itself immediately, so the answer would never come.
        self.config = configuration;

        // Map /host to the real filesystem root so the selection file can be read back.
        change_host_folder(PathBuf::from("/"));

        let plugin_ids = get_plugin_ids();
        self.plugin_id = plugin_ids.plugin_id;
        self.out_path = selection_path(plugin_ids.zellij_pid, self.plugin_id);

        subscribe(&[
            EventType::CommandPaneExited,
            EventType::EditPaneExited,
            EventType::PaneClosed,
            EventType::PaneUpdate,
            EventType::PluginConfigurationChanged,
        ]);

        hide_self();
        self.open_finder();
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::CommandPaneExited(pane_id, _exit_code, _context) => {
                self.handle_pane_exited(pane_id);
            },
            Event::EditPaneExited(pane_id, _exit_code, _context) => {
                self.handle_pane_exited(pane_id);
            },
            Event::PaneClosed(pane_id) => {
                // The user closed our pane by hand. Without this the plugin would linger
                // hidden for the rest of the session, and the next keypress would focus
                // that corpse instead of starting a new search.
                if Some(pane_id) == self.stage.pane_id().map(PaneId::Terminal) {
                    self.finish();
                }
            },
            Event::PaneUpdate(_) => {
                self.bounce_focus_off_self();
            },
            Event::PluginConfigurationChanged(configuration) => {
                // A running plugin's KDL block is otherwise frozen at server start, so
                // without this an opener fix needs a whole Zellij restart.
                self.config = configuration;
            },
            _ => {},
        }
        false // never render — the plugin has no UI of its own
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        // Hidden driver plugin. Never rendered. Intentionally empty.
    }
}

impl State {
    /// Launch fzf in a floating pane rooted at the focused pane's project.
    fn open_finder(&mut self) {
        // Clear any selection left by a previous run, so a crashed fzf cannot make us
        // reopen the last file instead of reporting a cancel.
        let _ = std::fs::remove_file(self.host_out_path());

        // `PaneInfo` carries no cwd, so ask the host for the focused pane's live working
        // directory. The script walks up to the git root from there, and so does the
        // title — asked once, so the two can never name different directories.
        let cwd = focused_cwd();
        let title = finder_title(project::label(Path::new(HOST_MOUNT), &cwd));

        let command = CommandToRun {
            path: PathBuf::from("/bin/sh"),
            args: vec![
                "-c".to_owned(),
                script::build(&self.config, &self.out_path.to_string_lossy()),
            ],
            cwd: Some(cwd),
            ..Default::default()
        };

        match open_command_pane_overlay(
            command,
            OverlayPlacement::dismissible(self.floating_geometry()),
            BTreeMap::new(),
        ) {
            Some(PaneId::Terminal(pane_id)) => {
                rename_pane_with_id(PaneId::Terminal(pane_id), &title);
                self.stage = Stage::Finding { pane_id };
            },
            _ => {
                // Nothing was opened, so there is no pane to report the failure in.
                eprintln!("file-finder: failed to open the finder pane");
                close_self();
            },
        }
    }

    fn handle_pane_exited(&mut self, pane_id: u32) {
        match self.stage {
            Stage::Finding { pane_id: finder } if finder == pane_id => {
                self.handle_selection(pane_id);
            },
            Stage::Opened { pane_id: opened } if opened == pane_id => {
                // The editor or viewer quit. Take the pane with it.
                close_pane_with_id(PaneId::Terminal(pane_id));
                self.finish();
            },
            _ => {},
        }
    }

    /// fzf has exited: read what it chose and hand the pane over, or tidy up.
    fn handle_selection(&mut self, finder_pane_id: u32) {
        let contents = std::fs::read_to_string(self.host_out_path()).unwrap_or_default();
        let _ = std::fs::remove_file(self.host_out_path());

        let selection = match script::parse_selection(&contents) {
            Some(selection) => PathBuf::from(selection),
            None => {
                // Cancelled — Esc, an empty list, or fzf failing to start.
                close_pane_with_id(PaneId::Terminal(finder_pane_id));
                self.finish();
                return;
            },
        };

        let cwd = selection.parent().map(Path::to_path_buf);
        let target = PaneId::Terminal(finder_pane_id);

        // `close_replaced_pane: true` retires the held fzf pane for good; the new pane
        // inherits its floating slot and geometry.
        let opened = match opener_for(&self.config, &selection) {
            Opener::ZellijEditor => {
                let mut file = FileToOpen::new(&selection);
                if let Some(cwd) = cwd {
                    file = file.with_cwd(cwd);
                }
                // Routing through the host means the server picks the editor
                // (scrollback_editor → $EDITOR → $VISUAL → vi) exactly as it does for
                // every other "open a file" path in Zellij.
                open_edit_pane_in_place_of_pane_id(target, file, true, BTreeMap::new())
            },
            Opener::Command { path, args } => {
                let command = CommandToRun {
                    path: PathBuf::from(path),
                    args,
                    cwd,
                    ..Default::default()
                };
                open_command_pane_in_place_of_pane_id(target, command, true, BTreeMap::new())
            },
        };

        match opened {
            Some(PaneId::Terminal(pane_id)) => {
                self.stage = Stage::Opened { pane_id };
                // These in-place-of-pane-id calls deliberately do not move focus, and
                // the pane they replaced was the focused one.
                focus_pane_with_id(PaneId::Terminal(pane_id), true, false);
            },
            _ => {
                eprintln!(
                    "file-finder: failed to open {}",
                    selection.to_string_lossy()
                );
                close_pane_with_id(target);
                self.finish();
            },
        }
    }

    /// Send focus back to the pane being driven if it has landed on this plugin.
    ///
    /// Pressing the finder keybinding again while a search is up does not start a second
    /// search: `LaunchOrFocusPlugin` matches on `(location, configuration)`, finds this
    /// instance, and focuses it — a pane that renders nothing. So the keypress would
    /// appear to do nothing at all. Put the user back on the pane they meant.
    ///
    /// Two things that look like they would work here do not, and each cost a debugging
    /// session, so they are worth naming:
    ///
    /// - `Event::Visible` reports *tab* visibility, and only to plugins in `tiled_panes`
    ///   or `floating_panes`. `hide_self()` extracts us into `suppressed_panes`, which
    ///   is neither, so it never arrives.
    /// - The `PaneManifest` cannot answer it either: `Tab::pane_infos` hardcodes
    ///   `is_focused = false` for every suppressed pane, and `hide_self()` suppresses
    ///   us — so we always look unfocused there, wherever focus actually is.
    ///
    /// `get_focused_pane_info` asks the host for the client's active pane id, which is
    /// the one answer that stays true while we are hidden. `PaneUpdate` is only the
    /// trigger to go and ask.
    fn bounce_focus_off_self(&self) {
        let Some(driving) = self.stage.pane_id() else {
            return;
        };
        let focused = get_focused_pane_info().ok().map(|(_, pane)| pane);
        if focused == Some(PaneId::Plugin(self.plugin_id)) {
            hide_self();
            focus_pane_with_id(PaneId::Terminal(driving), true, false);
        }
    }

    /// The flow is over: stop tracking a pane and shut down.
    fn finish(&mut self) {
        self.stage = Stage::Idle;
        let _ = std::fs::remove_file(self.host_out_path());
        close_self();
    }

    /// The selection file as this plugin sees it, through the `/host` mount.
    fn host_out_path(&self) -> PathBuf {
        Path::new(HOST_MOUNT).join(self.out_path.strip_prefix("/").unwrap_or(&self.out_path))
    }

    /// Where the finder pane sits on the session surface. Dismissible rather than modal: the
    /// user can click away from a file picker.
    fn floating_geometry(&self) -> Option<FloatingPaneCoordinates> {
        let Geometry {
            x,
            y,
            width,
            height,
        } = geometry(&self.config);
        FloatingPaneCoordinates::new(Some(x), Some(y), Some(width), Some(height), None, None)
    }
}

/// The finder pane's frame title, naming the tree being searched where there is a name
/// for it.
///
/// The constant stays the prefix rather than being replaced: it is what the eye finds
/// first, and it is what tells the user this frame is the picker at all.
fn finder_title(label: Option<String>) -> String {
    match label {
        Some(label) => format!("{FINDER_PANE_TITLE} — {label}"),
        None => FINDER_PANE_TITLE.to_owned(),
    }
}

/// Where the finder script leaves the path the user picked.
///
/// Both halves of the name are load-bearing. `plugin_id` alone is not unique: it comes
/// from a counter that restarts at 0 in every server, and the default layout loads
/// tab-bar and status-bar ahead of us, so this plugin is `2` in more or less every
/// session there has ever been. Two Zellij sessions open on one machine would then read
/// and unlink one another's selection file, and an unlink that lands between the script
/// writing a pick and this plugin reading it is indistinguishable from a cancel: the
/// overlay just closes and the file never opens.
///
/// `zellij_pid` is the server's own pid, so it separates sessions the way the name needs.
/// It also separates the integration tests, which are separate processes and were losing
/// each other's selections exactly this way.
fn selection_path(zellij_pid: u32, plugin_id: u32) -> PathBuf {
    PathBuf::from(format!(
        "/tmp/zellij-file-finder-{}-{}.sel",
        zellij_pid, plugin_id
    ))
}

/// The overlay's position and size, as the strings `FloatingPaneCoordinates` parses.
#[derive(Debug, PartialEq, Eq)]
struct Geometry {
    x: String,
    y: String,
    width: String,
    height: String,
}

/// Read the geometry out of the plugin's config, falling back to a centred overlay.
///
/// `x`/`y`/`width`/`height` are not reserved words, so they reach `load` untouched.
/// Values are passed through verbatim; `FloatingPaneCoordinates::new` is what decides
/// whether `"80%"` or `"100"` is meaningful, and drops anything it cannot parse.
fn geometry(config: &BTreeMap<String, String>) -> Geometry {
    let dimension = |key: &str, default: &str| {
        config
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .unwrap_or(default)
            .to_owned()
    };
    Geometry {
        x: dimension("x", DEFAULT_X),
        y: dimension("y", DEFAULT_Y),
        width: dimension("width", DEFAULT_WIDTH),
        height: dimension("height", DEFAULT_HEIGHT),
    }
}

/// The cwd the finder should start from. The plugin's own cwd is `/host`, which is never
/// what the user means by "this project".
fn focused_cwd() -> PathBuf {
    get_focused_pane_info()
        .ok()
        .and_then(|(_, pane)| get_pane_cwd(pane).ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_reports_the_pane_it_is_driving() {
        assert_eq!(Stage::Idle.pane_id(), None);
        assert_eq!(Stage::Finding { pane_id: 3 }.pane_id(), Some(3));
        assert_eq!(Stage::Opened { pane_id: 9 }.pane_id(), Some(9));
    }

    #[test]
    fn the_title_names_the_tree_being_searched() {
        assert_eq!(
            finder_title(Some("zellij (main)".to_owned())),
            "Find file — zellij (main)"
        );
    }

    /// Every integration assertion matches on the bare title, and a nameless tree is
    /// exactly the case where there is nothing to add to it.
    #[test]
    fn a_tree_with_no_name_keeps_the_bare_title() {
        assert_eq!(finder_title(None), "Find file");
    }

    #[test]
    fn host_out_path_is_the_selection_file_under_the_host_mount() {
        let state = State {
            out_path: PathBuf::from("/tmp/zellij-file-finder-4.sel"),
            ..Default::default()
        };
        assert_eq!(
            state.host_out_path(),
            PathBuf::from("/host/tmp/zellij-file-finder-4.sel")
        );
    }

    /// The whole point of the name: `plugin_id` is very nearly always 2, so the pid is
    /// the only thing keeping two sessions off each other's selection file.
    #[test]
    fn two_sessions_do_not_share_a_selection_file() {
        assert_ne!(selection_path(1000, 2), selection_path(1001, 2));
    }

    /// And the plugin id still separates two finders within one session.
    #[test]
    fn two_finders_in_one_session_do_not_share_a_selection_file() {
        assert_ne!(selection_path(1000, 2), selection_path(1000, 5));
    }

    #[test]
    fn geometry_defaults_to_a_centred_overlay() {
        assert_eq!(
            geometry(&BTreeMap::new()),
            Geometry {
                x: "".to_owned(),
                y: "15%".to_owned(),
                width: "120".to_owned(),
                height: "70%".to_owned(),
            }
        );
    }

    #[test]
    fn geometry_honours_configured_dimensions() {
        let config = BTreeMap::from([
            ("width".to_owned(), "100".to_owned()),
            ("height".to_owned(), "50%".to_owned()),
        ]);
        let geometry = geometry(&config);
        assert_eq!(geometry.width, "100");
        assert_eq!(geometry.height, "50%");
        // Unset keys keep their defaults.
        assert_eq!(geometry.x, "");
        assert_eq!(geometry.y, "15%");
    }

    #[test]
    fn blank_geometry_values_fall_back_to_the_defaults() {
        let config = BTreeMap::from([("width".to_owned(), "   ".to_owned())]);
        assert_eq!(geometry(&config).width, "120");
    }
}
