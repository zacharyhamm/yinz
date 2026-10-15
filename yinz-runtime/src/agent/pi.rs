use std::path::{Path, PathBuf};

/// Bumped whenever [`PI_EXTENSION_TEMPLATE`]'s substitutions-bearing body changes, or the
/// set of events it wires does, so a data-dir copy from an older yinz is replaced rather
/// than pointed at. The same discipline `src/agent_hooks.rs` keeps for its script.
pub const PI_EXTENSION_VERSION: u32 = 7;

/// What marks the generated file as ours, for install/uninstall/status and for nothing
/// else: a file without it is a person's own extension and is never touched.
pub const PI_EXTENSION_MARKER: &str = "yinz-pi-agent-report";

pub use yinz_types::agent::{
    is_pi_extension_argument as is_extension_argument, PI_EXTENSION_ARGUMENT as EXTENSION_ARGUMENT,
    PI_EXTENSION_FILE,
};

/// The env var a remote pane's token arrives in.
///
/// Spelled here because `yinz-process`'s broker owns the canonical constant and this
/// crate cannot reach it (the broker reads this crate, not the other way round). The
/// spelling is the one the broker, the codex and muse launchers and Claude Code's own
/// report command all share; a name that drifted would be a silently missing route.
pub const PANE_TOKEN_ENV: &str = "YINZ_REMOTE_PANE_TOKEN";

/// The pi events the extension reports, in wiring order.
///
/// The count is what `yinz hooks install --agent pi` says it wired, and the
/// installer's tests assert the template carries every one of them - the same contract
/// `OPENCODE_HOOK_EVENTS` keeps.
pub const PI_HOOK_EVENTS: &[&str] = &[
    "session_start",
    "session_before_compact",
    "session_compact",
    "session_compact_failed",
    "before_agent_start",
    "tool_execution_start",
    "tool_execution_end",
    "message_end",
    "agent_settled",
    "ui_prompt_start",
    "ui_prompt_end",
    "session_shutdown",
];

/// The extension a pi pane loads, with `__YINZ_BIN__`, `__TOKEN_ENV__` and
/// `__VERSION__` left for [`render_pi_extension`] to fill in.
///
/// A template rather than a `format!` for the same reason the opencode plugin is one:
/// the body is TypeScript, whose braces would all have to be doubled, and a missed
/// substitution has to fail a test rather than write an extension pi cannot parse.
pub const PI_EXTENSION_TEMPLATE: &str = include_str!("pi-extension.ts");

pub fn pi_extension_path() -> PathBuf {
    crate::home::xdg_data_dir()
        .join("pi")
        .join(EXTENSION_ARGUMENT)
        .join(PI_EXTENSION_FILE)
}

/// Resolve only Yinz's injected argument. Remote agents use remotely installed hooks.
pub fn prepare_command(command: &mut yinz_types::command::CommandToRun) -> Result<(), String> {
    if command.target.is_remote() {
        strip_extension(&mut command.args);
    } else if command
        .args
        .windows(2)
        .any(|pair| pair == ["--extension", EXTENSION_ARGUMENT])
    {
        let binary = crate::own_binary::server_binary()
            .ok_or("cannot resolve Yinz binary for Pi reporting")?;
        let path = ensure_pi_extension(binary).map_err(|error| {
            format!(
                "cannot create Pi reporting extension at {}: {error}",
                pi_extension_path().display()
            )
        })?;
        for index in 1..command.args.len() {
            if command.args[index - 1] == "--extension" && command.args[index] == EXTENSION_ARGUMENT
            {
                command.args[index] = path.to_string_lossy().into_owned();
            }
        }
    }
    Ok(())
}

/// [`PI_EXTENSION_TEMPLATE`] with `yinz`'s absolute path and the reporting version
/// substituted. The path is JSON-quoted - TypeScript string literals, not shell ones -
/// and baked rather than looked up on PATH for the same reason every other generated
/// artifact bakes it: a pane's environment is not always one where `yinz` is on the
/// path, and the template keeps its own PATH fallback for a binary an upgrade moved.
pub fn render_pi_extension(yinz: &Path) -> String {
    let bin = serde_json::to_string(&yinz.display().to_string()).expect("a path serializes");
    let token_env = serde_json::to_string(PANE_TOKEN_ENV).expect("a name serializes");
    PI_EXTENSION_TEMPLATE
        .replace("__YINZ_BIN__", &bin)
        .replace("__TOKEN_ENV__", &token_env)
        .replace("__VERSION__", &PI_EXTENSION_VERSION.to_string())
}

/// Remove the `--extension <path>` pair [`super::launch`] adds for a local pi, from a
/// command that will run on another machine.
///
/// The file lives in *this* machine's data dir, and a path that does not exist where
/// the command runs is a pi that refuses to start. A far machine with yinz installed
/// reports through its own copy - `yinz hooks install --agent pi` - which is the
/// same shape every install-required agent keeps. Only the planner's own pair goes: a
/// caller's `--extension` of their own is theirs, wherever the pane opens.
pub fn strip_extension(args: &mut Vec<String>) {
    let mut index = 0;
    while index + 1 < args.len() {
        if args[index] == "--extension" && args[index + 1] == EXTENSION_ARGUMENT {
            args.drain(index..=index + 1);
        } else {
            index += 1;
        }
    }
}

/// Write the data-dir copy a spawned pane's `--extension` names, if it is not already
/// the one this binary renders.
///
/// Through a temporary file and a rename, because pi reads the file while loading: a
/// half-written extension would be one that does not parse. Content rather than
/// version-only comparison, so a version number forgotten in a bump still repairs
/// itself. A failure rejects the spawn before Pi is started.
pub fn ensure_pi_extension(yinz: &Path) -> std::io::Result<PathBuf> {
    write_extension(&pi_extension_path(), yinz)
}

fn write_extension(path: &Path, yinz: &Path) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let rendered = render_pi_extension(yinz);
    if std::fs::read_to_string(path).is_ok_and(|current| current == rendered) {
        return Ok(path.to_owned());
    }
    let folder = path
        .parent()
        .ok_or_else(|| std::io::Error::other("the extension path has no folder"))?;
    std::fs::create_dir_all(folder)?;
    let mut temporary = tempfile::NamedTempFile::new_in(folder)?;
    temporary.write_all(rendered.as_bytes())?;
    temporary.persist(path)?;
    Ok(path.to_owned())
}

/// The events a template wires, from the template itself: every entry in
/// [`PI_HOOK_EVENTS`] must appear as a `pi.on("...")` registration, so a table edited
/// here and a template edited there cannot drift apart.
pub fn template_wires_every_event() -> bool {
    PI_HOOK_EVENTS
        .iter()
        .all(|event| PI_EXTENSION_TEMPLATE.contains(&format!("pi.on(\"{event}\"")))
}

#[cfg(test)]
#[path = "unit/pi_tests.rs"]
mod tests;
