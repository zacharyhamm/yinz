//! Starting a coding agent: what to run, and what Git setup to do first.
//!
//! Two callers ask for the same thing by two different routes — the `hocus-new-agent`
//! modal, where a person fills in a form, and hocus's `agent_spawn` tool, where a model
//! fills in a JSON object. Both mean "start `claude` on a fresh worktree off `main`", and
//! neither has any business deciding separately what that means: a default base branch
//! that drifted, or a `cursor` that ran `cursor` on one path and `cursor-agent` on the
//! other, would be a difference nobody asked for and nobody would find until it bit.
//!
//! So the decision lives here, once. [`Spec`] is the request in terms both callers can
//! fill in, [`plan`] turns it into the pair Zellij wants, and [`open`] is the only place
//! in the tree that calls the host to do it. A caller's job is to build a `Spec` and hand
//! it over; everything after that is the same code for both.
//!
//! [`plan`] is pure and tests on the host triple. [`open`] cannot be — it is a host call —
//! so it is gated the way [`crate::store::Host`] is, with a stub on the host triple that
//! fails loudly rather than quietly answering nothing.

use zellij_tile::prelude::{CommandToRun, GitCheckout, PaneId};

/// The branch a new one is cut from when the caller does not say.
///
/// A default rather than a probe of the repository's own HEAD: the modal has to
/// show *something* in the field before it knows which project is selected, and a
/// value that changed as you moved the project select would be worse than one you
/// can read and correct.
pub const DEFAULT_BASE: &str = "main";

/// Where worktrees go, relative to the repository's primary checkout.
pub const WORKTREE_PREFIX: &str = ".worktrees/";

/// The agents a chooser offers, in the order it should offer them.
///
/// Not every kind [`crate::agents`] can *recognise* — that list is what detection
/// knows about, which is deliberately wider than what we will start on request. This
/// one is shared so the modal's select and the tool's schema enum cannot come to
/// disagree about which agents you may ask for.
pub const OFFERED: &[&str] = &["codex", "claude", "cursor"];

/// Everything it takes to start an agent, in terms both callers have.
///
/// Every field is optional because both callers have partial inputs — a form with
/// blank fields, a tool call with omitted keys — and the defaults belong in [`plan`]
/// where both get them, not in a form's initial values where only one would.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Spec {
    /// The agent to run, by kind: `claude`, `codex`, `cursor`, or any other name
    /// [`crate::agents`] would recognise. Resolved to a program by [`binary_for`].
    pub kind: Option<String>,
    /// A program to run instead, for whatever no kind names. Ignored when `kind` is
    /// set; one of the two is required.
    pub command: Option<String>,
    /// Arguments before the ones this module adds.
    pub args: Vec<String>,
    /// Passed as `--model <model>`, the one flag every agent here spells the same.
    pub model: Option<String>,
    /// An opening prompt, passed as the last positional argument — which is how
    /// `claude`, `codex` and `cursor-agent` all take one.
    pub prompt: Option<String>,
    /// The branch to create. Absent means no Git work at all: run where we are.
    pub branch: Option<String>,
    /// What `branch` is cut from. Defaults to [`DEFAULT_BASE`].
    pub base: Option<String>,
    /// Whether `branch` lives in a new worktree rather than in this checkout.
    /// Defaults to *true* once a branch is named, which is the safer of the two:
    /// a worktree leaves the checkout the caller is looking at alone.
    pub worktree: Option<bool>,
    /// Where that worktree goes, relative to the repository's primary checkout.
    /// Defaults to [`worktree_path_for`] the branch.
    pub worktree_path: Option<String>,
}

/// The program a kind runs.
///
/// Identity for all but one: `cursor` ships its agent as `cursor-agent`, and the bare
/// `cursor` on the path is the editor. The mapping is here rather than at each call
/// site because there used to be two copies of it and they were one release apart from
/// disagreeing.
pub fn binary_for(kind: &str) -> &str {
    match kind {
        "cursor" => "cursor-agent",
        other => other,
    }
}

/// Where a branch's worktree goes when the caller does not say.
pub fn worktree_path_for(branch: &str) -> String {
    format!("{WORKTREE_PREFIX}{branch}")
}

/// What to run, and what to do to the repository before running it.
///
/// The error strings are the ones shown to whoever asked — the modal puts them in its
/// status row, hocus hands them back to the model as the tool result — so they say what
/// is missing rather than that something is.
pub fn plan(spec: &Spec) -> Result<(CommandToRun, GitCheckout), String> {
    let path = match (trimmed(&spec.kind), trimmed(&spec.command)) {
        (Some(kind), _) => binary_for(&kind.to_lowercase()).to_owned(),
        (None, Some(command)) => command.to_owned(),
        (None, None) => return Err("an agent or a command is required".to_owned()),
    };

    let mut args = spec.args.clone();
    if let Some(model) = trimmed(&spec.model) {
        args.push("--model".to_owned());
        args.push(model.to_owned());
    }
    // Last, and after `--model`: a positional argument that came before a flag would
    // be read by some of these programs as the flag's value.
    if let Some(prompt) = trimmed(&spec.prompt) {
        args.push(prompt.to_owned());
    }

    let command = CommandToRun {
        path: path.into(),
        args,
        ..Default::default()
    };

    let Some(branch) = trimmed(&spec.branch) else {
        // A worktree asked for with nothing to put in it. Saying so beats silently
        // running in the current checkout, which is the one thing the caller said
        // they did not want.
        if spec.worktree == Some(true) {
            return Err("a branch is required to create a worktree".to_owned());
        }
        return Ok((command, GitCheckout::Current));
    };

    let base = trimmed(&spec.base).unwrap_or(DEFAULT_BASE);
    if !spec.worktree.unwrap_or(true) {
        return Ok((
            command,
            GitCheckout::NewBranch {
                branch: branch.to_owned(),
                base: base.to_owned(),
            },
        ));
    }
    let path = trimmed(&spec.worktree_path)
        .map(str::to_owned)
        .unwrap_or_else(|| worktree_path_for(branch));
    Ok((
        command,
        GitCheckout::NewWorktree {
            branch: branch.to_owned(),
            base: base.to_owned(),
            path: path.into(),
        },
    ))
}

/// A field the caller filled in, or `None` for one they left blank.
///
/// Whitespace-only counts as blank: a form field the user tabbed through and a JSON
/// key the model sent as `""` both mean "I did not say", and neither should reach Git
/// as a branch name.
fn trimmed(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

// --------------------------------------------------------------- wasm: the host call

/// Do the Git setup and open the pane. The one call site of the host API that does it.
#[cfg(target_family = "wasm")]
pub fn open(spec: &Spec, source_directory: std::path::PathBuf) -> Result<PaneId, String> {
    let (command, checkout) = plan(spec)?;
    zellij_tile::shim::open_command_pane_in_git_checkout_near_plugin(
        command,
        source_directory,
        checkout,
    )
}

/// [`open`] on the host triple, where there is no host to call.
///
/// Both callers are compiled natively to run their tests, so this has to exist on
/// both sides. It fails rather than pretending, for the same reason
/// [`crate::store::Host`]'s stub does: a test that reaches here has left the pure
/// half, and should say so instead of passing against a pane that was never opened.
#[cfg(not(target_family = "wasm"))]
pub fn open(_spec: &Spec, _source_directory: std::path::PathBuf) -> Result<PaneId, String> {
    Err("agents cannot be started outside zellij".to_owned())
}

/// The plugin alias for each half of the hocus column.
///
/// Aliases rather than `zellij:` urls: plugin identity is (location, configuration), an alias
/// contributes its own block - `shared true` - to that configuration, and the keybindings open
/// both halves by alias. A second spelling would open a second pane rather than reach the one
/// already there.
pub const HOCUS_ALIAS: &str = "hocus";
pub const HOCUS_AGENTS_ALIAS: &str = "hocus-agents";

/// The configuration that makes a launched hocus pane an overlay.
///
/// One definition for both halves and both directions. `LaunchOrFocusPlugin { overlay true }`
/// writes exactly this key, so a launch from a plugin has to match it character for character or
/// the keybinding and the plugin address two different instances - and the failure looks like a
/// duplicate pane rather than an error.
pub fn overlay_configuration() -> std::collections::BTreeMap<String, String> {
    let mut configuration = std::collections::BTreeMap::new();
    configuration.insert("overlay".to_owned(), "true".to_owned());
    configuration
}

/// Whether this instance is a *legacy* hocus half: one restored as a global pane.
///
/// hocus used to be a pair of global panes and is now a pair of overlays. Overlays are not
/// serialized at all, but global panes are, so a session dump written before that change brings
/// both halves back as global panes configured `shared true` and nothing else. Plugin identity is
/// `(location, configuration)` and both halves now address each other with
/// [`overlay_configuration`], so a restored pane matches nothing anybody asks for: the keybinding
/// opens a second, overlay column beside a strip that will not go away on its own.
///
/// The manifest is the authority rather than the absent `overlay` key, because the two questions
/// are not the same one. "My configuration says no overlay" is true of any deliberate non-overlay
/// launch; "the server is drawing me as a global pane" is true of exactly the shape being migrated
/// away from. The configuration is still worth reading first - it is known at `load`, before
/// anything has been opened - but only as a suspicion for this to confirm.
pub fn is_legacy_global_instance(
    own_plugin_id: u32,
    manifest: &zellij_tile::prelude::PaneManifest,
) -> bool {
    manifest
        .global_panes
        .iter()
        .any(|pane| pane.is_plugin && pane.id == own_plugin_id)
}

/// Whether a half launched with this configuration might be a legacy one - the cheap half of
/// [`is_legacy_global_instance`], answerable at `load` before any pane has been opened.
///
/// A suspicion and not a verdict: it is also true of a hocus someone put in a tiled layout on
/// purpose. What it buys is that a suspect defers the launches it would otherwise make on load, so
/// a legacy instance cannot seed an orphan half before it finds out what it is.
pub fn may_be_legacy(configuration: &std::collections::BTreeMap<String, String>) -> bool {
    configuration.get("overlay").map(|value| value.trim()) != Some("true")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use zellij_tile::prelude::PaneManifest;

    fn spec(kind: &str) -> Spec {
        Spec {
            kind: Some(kind.to_owned()),
            ..Default::default()
        }
    }

    /// The modal's own defaults, which are now this module's: a worktree off `main`
    /// at `.worktrees/<branch>`, with nothing but the branch name typed.
    #[test]
    fn a_branch_alone_means_a_worktree_off_main() {
        let (command, checkout) = plan(&Spec {
            branch: Some("fix-parser".to_owned()),
            ..spec("claude")
        })
        .unwrap();
        assert_eq!(command.path, PathBuf::from("claude"));
        assert!(command.args.is_empty());
        assert_eq!(
            checkout,
            GitCheckout::NewWorktree {
                branch: "fix-parser".to_owned(),
                base: DEFAULT_BASE.to_owned(),
                path: PathBuf::from(".worktrees/fix-parser"),
            }
        );
    }

    #[test]
    fn no_branch_means_no_git_work() {
        let (_, checkout) = plan(&spec("codex")).unwrap();
        assert_eq!(checkout, GitCheckout::Current);
    }

    #[test]
    fn a_branch_without_a_worktree_switches_this_checkout() {
        let (_, checkout) = plan(&Spec {
            branch: Some("topic".to_owned()),
            base: Some("develop".to_owned()),
            worktree: Some(false),
            ..spec("claude")
        })
        .unwrap();
        assert_eq!(
            checkout,
            GitCheckout::NewBranch {
                branch: "topic".to_owned(),
                base: "develop".to_owned(),
            }
        );
    }

    /// The one place a worktree path is *not* derived: the caller said where.
    #[test]
    fn a_given_worktree_path_is_used_as_it_stands() {
        let (_, checkout) = plan(&Spec {
            branch: Some("topic".to_owned()),
            worktree_path: Some("/elsewhere/topic".to_owned()),
            ..spec("claude")
        })
        .unwrap();
        assert!(matches!(
            checkout,
            GitCheckout::NewWorktree { path, .. } if path == *"/elsewhere/topic"
        ));
    }

    #[test]
    fn a_worktree_with_no_branch_says_so_rather_than_running_here() {
        assert_eq!(
            plan(&Spec {
                worktree: Some(true),
                ..spec("claude")
            })
            .unwrap_err(),
            "a branch is required to create a worktree",
        );
    }

    /// `--model` before the prompt, because the prompt is positional and would
    /// otherwise be read as the flag's value.
    #[test]
    fn the_model_flag_comes_before_the_prompt() {
        let (command, _) = plan(&Spec {
            model: Some("opus".to_owned()),
            prompt: Some("fix the parser".to_owned()),
            args: vec!["--verbose".to_owned()],
            ..spec("claude")
        })
        .unwrap();
        assert_eq!(
            command.args,
            vec![
                "--verbose".to_owned(),
                "--model".to_owned(),
                "opus".to_owned(),
                "fix the parser".to_owned(),
            ]
        );
    }

    #[test]
    fn cursor_is_the_one_kind_that_is_not_its_own_program() {
        assert_eq!(binary_for("cursor"), "cursor-agent");
        for kind in ["claude", "codex", "gemini", "aider"] {
            assert_eq!(binary_for(kind), kind);
        }
    }

    /// The modal's select says `Codex`; the tool's schema says `codex`. Both arrive
    /// here and mean the same agent.
    #[test]
    fn a_kind_is_matched_however_it_was_capitalised() {
        let (command, _) = plan(&spec("Cursor")).unwrap();
        assert_eq!(command.path, PathBuf::from("cursor-agent"));
    }

    #[test]
    fn a_command_stands_in_for_a_kind_but_one_of_them_is_required() {
        let (command, _) = plan(&Spec {
            command: Some("zsh".to_owned()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(command.path, PathBuf::from("zsh"));
        assert_eq!(
            plan(&Spec::default()).unwrap_err(),
            "an agent or a command is required",
        );
    }

    fn global_manifest(panes: Vec<zellij_tile::prelude::PaneInfo>) -> PaneManifest {
        PaneManifest {
            global_panes: panes,
            ..Default::default()
        }
    }

    fn plugin_pane(id: u32) -> zellij_tile::prelude::PaneInfo {
        zellij_tile::prelude::PaneInfo {
            id,
            is_plugin: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_half_the_server_draws_as_a_global_pane_is_a_legacy_one() {
        assert!(is_legacy_global_instance(
            7,
            &global_manifest(vec![plugin_pane(7)])
        ));
    }

    /// The overlay column's own panes live on a tab and never appear in this list, which is the
    /// whole reason the list is the discriminator.
    #[test]
    fn a_half_that_is_not_a_global_pane_is_not_a_legacy_one() {
        assert!(!is_legacy_global_instance(7, &global_manifest(vec![])));
        assert!(!is_legacy_global_instance(
            7,
            &global_manifest(vec![plugin_pane(9)])
        ));
    }

    /// Terminal and plugin pane ids are separate number lines, so the kind has to be checked or a
    /// global terminal that happens to share our number reads as us.
    #[test]
    fn a_global_terminal_with_our_number_is_not_us() {
        let terminal = zellij_tile::prelude::PaneInfo {
            id: 7,
            is_plugin: false,
            ..Default::default()
        };
        assert!(!is_legacy_global_instance(
            7,
            &global_manifest(vec![terminal])
        ));
    }

    #[test]
    fn only_a_half_configured_as_an_overlay_is_above_suspicion() {
        assert!(!may_be_legacy(&overlay_configuration()));
        assert!(may_be_legacy(&BTreeMap::new()));
        assert!(may_be_legacy(&BTreeMap::from([(
            "shared".to_owned(),
            "true".to_owned()
        )])));
    }

    /// A form field tabbed through and a JSON key sent as `""` are the same thing:
    /// nothing said. Neither may reach Git as a branch name.
    #[test]
    fn blank_fields_are_the_same_as_absent_ones() {
        let (command, checkout) = plan(&Spec {
            branch: Some("   ".to_owned()),
            model: Some("".to_owned()),
            prompt: Some(" ".to_owned()),
            ..spec("claude")
        })
        .unwrap();
        assert!(command.args.is_empty());
        assert_eq!(checkout, GitCheckout::Current);
    }
}
