use dialoguer::Confirm;
use std::{
    path::{Path, PathBuf},
    process,
    time::Duration,
};
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TerminalId;
use yinz_types::units::{ElapsedMicros, UnixMicros};

use yinz_client::{
    os_input_output::get_client_os_input, start_client as start_client_impl, ClientInfo,
};

use {
    yinz_cli::sessions::assert_dead_session, yinz_cli::sessions::assert_session_ne,
    yinz_cli::sessions::delete_session as delete_session_impl,
    yinz_cli::sessions::denied_session_message, yinz_cli::sessions::generate_unique_session_name,
    yinz_cli::sessions::get_active_session, yinz_cli::sessions::no_active_session_message,
    yinz_cli::sessions::print_sessions, yinz_cli::sessions::print_sessions_with_index,
    yinz_cli::sessions::session_listing_error_message,
    yinz_cli::sessions::undeliverable_kill_message,
    yinz_cli::sessions::unresponsive_session_message, yinz_cli::sessions::ActiveSession,
    yinz_cli::sessions::SessionListing, yinz_ipc::probe::SessionProbe,
    yinz_session::sessions::get_resurrectable_sessions, yinz_session::sessions::get_sessions,
    yinz_session::sessions::get_sessions_sorted_by_mtime,
    yinz_session::sessions::get_sessions_with_status,
    yinz_session::sessions::kill_session as kill_session_impl,
    yinz_session::sessions::match_session_name, yinz_session::sessions::resurrection_layout,
    yinz_session::sessions::session_status, yinz_session::sessions::validate_session_name,
    yinz_session::sessions::SessionNameMatch,
};

use yinz_runtime::consts::session_layout_cache_file_name;

use miette::{Report, Result};
use yinz_server::start_server as start_server_impl;
use yinz_server_os::get_server_os_input_from_termios_arg;
use {
    yinz_cli::args::CliArgs, yinz_cli::args::Command, yinz_cli::args::SessionCommand,
    yinz_cli::args::Sessions, yinz_cli::setup::Setup, yinz_config::config::Config,
    yinz_config::config::ConfigError, yinz_types::data::SessionConnection,
    yinz_types::input::actions::Action, yinz_types::input::options::Options,
};

pub(crate) fn list_sessions(no_formatting: bool, short: bool, reverse: bool, exited: bool) {
    yinz_cli::sessions::list_sessions(
        no_formatting,
        short,
        reverse,
        exited,
        current_session().as_ref(),
    );
}

/// Whether a nameless `yinz attach` may ask which session to attach to: only
/// a person at a terminal can answer.
fn can_pick_a_session() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal()
}

/// A nameless `yinz attach` with several sessions running: a menu that
/// attaches to the session picked, or switches to it when run from inside a
/// session.
fn pick_session_to_attach(config_options: Options, config: &Config) -> ClientInfo {
    use yinz_cli::session_picker::{Outcome, PickerRow};
    let mut sessions = yinz_cli::sessions::collect_sessions(false).unwrap_or_else(|e| {
        eprintln!("Error occurred: {:?}", e);
        process::exit(1);
    });
    if sessions.is_empty() {
        eprintln!("No active yinz sessions found.");
        process::exit(1);
    }
    // Newest first, as the listing this replaces printed them.
    yinz_cli::sessions::sort_sessions(&mut sessions, true);
    let current = current_session();
    let rows = sessions
        .into_iter()
        .map(|(name, age, listing)| PickerRow {
            current: current.as_ref() == Some(&name),
            name,
            age,
            listing,
        })
        .collect();
    let verb = if current.is_some() {
        "switch"
    } else {
        "attach"
    };
    match crate::session_menu::run(rows, verb) {
        Ok(Outcome::Picked(name)) if current.is_some() => {
            let switch = yinz_cli::args::CliAction::SwitchSession {
                name: name.into_string(),
                tab_position: None,
                pane_id: None,
                layout: None,
                layout_string: None,
                layout_dir: None,
                cwd: None,
            };
            send_action_to_session(switch, None, Some(config.clone()));
            process::exit(0);
        },
        Ok(Outcome::Picked(name)) => ClientInfo::Attach(name, config_options),
        Ok(Outcome::Cancelled(message)) => {
            if let Some(message) = message {
                eprintln!("{message}");
            }
            process::exit(0);
        },
        Ok(Outcome::Interrupted) => process::exit(130),
        Ok(Outcome::Continue) => process::exit(0),
        Err(e) => {
            eprintln!("Error occurred: {e}");
            process::exit(1);
        },
    }
}

fn sessions_to_kill(
    get_sessions: impl FnOnce()
        -> Result<Vec<(SessionName, Duration, SessionProbe)>, std::io::ErrorKind>,
) -> Result<Vec<(SessionName, Duration)>, std::io::ErrorKind> {
    Ok(get_sessions()?
        .into_iter()
        .filter_map(|(name, age, status)| {
            (!matches!(status, SessionProbe::Absent)).then_some((name, age))
        })
        .collect())
}

pub(crate) fn firecracker(
    command: yinz_cli::args::Firecracker,
    config: &Config,
) -> yinz_runtime::errors::prelude::Result<()> {
    let provider = yinz_server::firecracker::provider_from_options(
        config.options.firecracker_provider.as_deref(),
        config.options.firecracker_provider_timeout_secs,
    )?;
    run_firecracker(
        command,
        &provider,
        &git_branch,
        &live_vm_session_panes,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

/// The branch a host worktree has out, or `None` when Git cannot say.
///
/// A VM mirrors a worktree, so "what is this VM working on" is answered by the branch
/// that worktree has out. That is a Git question about a path on this machine, not one
/// a VM provider should be asked for, so it is resolved here rather than added to the
/// provider protocol.
///
/// An explicit Git directory prevents discovery of an unrelated enclosing repository.
/// Detached and unborn checkouts, invalid repositories and a missing Git return `None`.
fn git_branch(worktree: &Path) -> Option<String> {
    let output = process::Command::new("git")
        .arg("--git-dir")
        .arg(worktree.join(".git"))
        .arg("--work-tree")
        .arg(worktree)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!branch.is_empty() && branch != "HEAD").then_some(branch)
}

/// The listing as the CLI prints it: the provider's own answer, with the branch each
/// VM's host worktree has out folded in.
///
/// A wrapper rather than a field on `VmEntry` because the branch is not part of the
/// provider protocol - it is what this command works out about the paths the provider
/// named. `recoveries` is borrowed through unchanged.
#[derive(serde::Serialize)]
struct ListedVms<'a> {
    vms: Vec<ListedVm<'a>>,
    recoveries: &'a [yinz_server::firecracker::RecoveryEntry],
}

#[derive(serde::Serialize)]
struct ListedVm<'a> {
    #[serde(flatten)]
    entry: &'a yinz_server::firecracker::VmEntry,
    /// Absent, rather than null, when there is no branch to name - the same shape the
    /// entry's own optional fields take, so a reader has one rule for all three.
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    panes: Vec<yinz_types::firecracker::Pane>,
    #[serde(skip_serializing_if = "Option::is_none")]
    panes_error: Option<String>,
}

fn live_vm_session_panes(
    session: &str,
) -> std::result::Result<Vec<yinz_types::data::PaneListEntry>, String> {
    if matches!(
        session_status(&SessionName::from(session)),
        Ok(SessionProbe::Absent)
    ) {
        return Ok(vec![]);
    }
    yinz_session::firecracker::list_panes(session, Duration::from_secs(3))
        .map_err(|error| format!("Live pane list unavailable: {error:#}"))
}

/// The session a `yinz firecracker` command acts on: `--session` if given, else the
/// session this command runs inside, else the only active one.
pub(crate) fn resolve_firecracker_session(
    explicit: Option<String>,
    env_session: Option<SessionName>,
    active: ActiveSession,
) -> yinz_runtime::errors::prelude::Result<String> {
    use yinz_runtime::errors::prelude::*;

    explicit
        .or(env_session.map(SessionName::into_string))
        .or(match active {
            ActiveSession::One(name) => Some(name.into_string()),
            ActiveSession::None | ActiveSession::Many => None,
        })
        .context("no session selected; pass --session SESSION")
}

/// Run one `yinz firecracker` command against `provider`, writing the listing to
/// `out` and each merge warning to `err`.
///
/// `branch_of` is injected rather than called directly so the tests stay off whatever
/// checkouts happen to exist on the machine running them; production passes
/// [`git_branch`].
pub(crate) fn run_firecracker(
    command: yinz_cli::args::Firecracker,
    provider: &yinz_server::firecracker::Provider,
    branch_of: &dyn Fn(&Path) -> Option<String>,
    panes_of: &dyn Fn(&str) -> std::result::Result<Vec<yinz_types::data::PaneListEntry>, String>,
    out: &mut impl std::io::Write,
    err: &mut impl std::io::Write,
) -> yinz_runtime::errors::prelude::Result<()> {
    use yinz_cli::args::Firecracker;
    use yinz_runtime::errors::prelude::anyhow;

    fn selected_session(explicit: Option<String>) -> yinz_runtime::errors::prelude::Result<String> {
        // The active-session scan is only worth doing without an explicit choice.
        match explicit {
            Some(session) => Ok(session),
            None => resolve_firecracker_session(
                None,
                yinz_process::pane_context::target(None)?.0,
                get_active_session(),
            ),
        }
    }

    match command {
        Firecracker::List { session, json } => {
            let listing = provider.list(session.as_deref())?;
            // One resolution per distinct worktree: two sessions leasing one worktree VM
            // are two entries naming the same directory, and asking Git twice about it
            // would be two processes for one answer.
            let mut branches: std::collections::BTreeMap<&Path, Option<String>> =
                std::collections::BTreeMap::new();
            for entry in &listing.vms {
                if let Some(worktree) = entry.worktree.as_deref() {
                    branches
                        .entry(worktree)
                        .or_insert_with(|| branch_of(worktree));
                }
            }
            let branch_for = |entry: &yinz_server::firecracker::VmEntry| {
                entry
                    .worktree
                    .as_deref()
                    .and_then(|worktree| branches.get(worktree).cloned().flatten())
            };
            if json {
                let mut manifests = std::collections::BTreeMap::new();
                for entry in &listing.vms {
                    manifests
                        .entry(&entry.session)
                        .or_insert_with(|| panes_of(&entry.session));
                }
                writeln!(
                    out,
                    "{}",
                    serde_json::to_string(&ListedVms {
                        vms: listing
                            .vms
                            .iter()
                            .map(|entry| ListedVm {
                                entry,
                                branch: branch_for(entry),
                                panes: manifests
                                    .get(&entry.session)
                                    .and_then(|result| result.as_ref().ok())
                                    .into_iter()
                                    .flatten()
                                    .filter(|pane| !pane.pane_info.is_plugin()
                                        && pane.pane_info.target.firecracker_vm()
                                            == Some(entry.name.as_str()))
                                    .map(|pane| yinz_types::firecracker::Pane {
                                        id: pane.pane_info.id.legacy_parts().0,
                                        title: pane.pane_info.title.clone(),
                                        agent: pane.pane_info.agent.is_some(),
                                    })
                                    .collect(),
                                panes_error: manifests
                                    .get(&entry.session)
                                    .and_then(|result| result.as_ref().err())
                                    .cloned(),
                            })
                            .collect(),
                        recoveries: &listing.recoveries,
                    })?
                )?;
            } else {
                if listing.vms.is_empty() {
                    writeln!(out, "No Firecracker VMs found.")?;
                }
                for entry in &listing.vms {
                    writeln!(
                        out,
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        entry.session,
                        entry.name,
                        entry.state,
                        entry.image.as_deref().unwrap_or("-"),
                        entry
                            .worktree
                            .as_deref()
                            .map(|worktree| worktree.display().to_string())
                            .unwrap_or_else(|| "-".to_owned()),
                        branch_for(entry).as_deref().unwrap_or("-"),
                    )?;
                }
                if !listing.recoveries.is_empty() {
                    writeln!(out, "Retained disks:")?;
                }
                for recovery in &listing.recoveries {
                    writeln!(
                        out,
                        "{}\t{}\t{}\t{}",
                        recovery.session.as_deref().unwrap_or("-"),
                        recovery.name.as_deref().unwrap_or("-"),
                        recovery.reason,
                        recovery.path.display()
                    )?;
                }
            }
        },
        Firecracker::Close {
            name,
            session,
            expected_identity,
            expected_worktree,
            expected_head,
            expected_branch,
        } => {
            let session = selected_session(session)?;
            let expected_checkout = match (expected_worktree.as_deref(), expected_head.as_deref()) {
                (Some(worktree), Some(head)) => Some((worktree, head, expected_branch.as_deref())),
                (None, None) if expected_branch.is_none() => None,
                _ => {
                    return Err(anyhow!(
                        "expected worktree cleanup requires both path and HEAD"
                    ))
                },
            };
            if expected_checkout.is_some() && expected_identity.is_none() {
                return Err(anyhow!("expected worktree cleanup requires a VM identity"));
            }
            close_firecracker_lease(
                provider,
                &session,
                &name,
                expected_identity.as_ref(),
                expected_checkout,
                out,
                err,
            )?;
        },
        Firecracker::Stop { name, session } => {
            let session = selected_session(session)?;
            for warning in provider.stop(&session, &name)? {
                writeln!(err, "warning: {warning}")?;
            }
        },
        Firecracker::Sync {
            name,
            session,
            expected_identity,
        } => {
            let session = selected_session(session)?;
            let warnings = if let Some(expected) = expected_identity {
                expected
                    .validate_target(&session, &name)
                    .map_err(|error| anyhow!(error))?;
                provider.sync_if_matches(&expected)?
            } else {
                provider.sync(&session, &name)?
            };
            for warning in warnings {
                writeln!(err, "warning: {warning}")?;
            }
        },
        Firecracker::Changes {
            name,
            session,
            expected_identity,
        } => {
            let session = selected_session(session)?;
            let changes = if let Some(expected) = expected_identity {
                expected
                    .validate_target(&session, &name)
                    .map_err(|error| anyhow!(error))?;
                provider.changes_if_matches(&expected)?
            } else {
                provider.changes(&session, &name)?
            };
            writeln!(out, "{changes}")?;
        },
    }
    Ok(())
}

fn close_firecracker_lease(
    provider: &yinz_server::firecracker::Provider,
    session: &str,
    name: &str,
    expected: Option<&yinz_types::firecracker::Identity>,
    expected_checkout: Option<(&Path, &str, Option<&str>)>,
    out: &mut impl std::io::Write,
    err: &mut impl std::io::Write,
) -> yinz_runtime::errors::prelude::Result<()> {
    use yinz_runtime::errors::prelude::*;
    validate_session_name(session).map_err(|error| anyhow!(error))?;
    yinz_types::command::validate_firecracker_vm_name(name).map_err(|error| anyhow!(error))?;
    if let Some(expected) = expected {
        expected
            .validate_target(session, name)
            .map_err(|error| anyhow!(error))?;
    }
    if let Some((path, head, branch)) = expected_checkout {
        let expected = expected.context("expected worktree cleanup requires a VM identity")?;
        for warning in
            provider.close_and_remove_worktree_if_matches(expected, path, head, branch)?
        {
            writeln!(err, "warning: {warning}")?;
        }
        writeln!(
            out,
            "Lease released and worktree removed: {name} in {session}."
        )?;
        return Ok(());
    }
    let before = provider.list(None)?;
    let mut leases = before
        .vms
        .iter()
        .filter(|vm| vm.session == session && vm.name == name);
    let lease = leases
        .next()
        .context("VM lease no longer exists; refresh the listing")?;
    if leases.next().is_some() {
        bail!("VM lease is ambiguous; refresh the listing");
    }
    let listed_identity = lease.identity();
    let expected = expected.unwrap_or(&listed_identity);
    expected
        .validate_target(session, name)
        .map_err(|error| anyhow!(error))?;
    if *expected != listed_identity {
        bail!("VM lease changed; refresh and confirm before closing");
    }
    for warning in provider.close_if_matches(expected)? {
        writeln!(err, "warning: {warning}")?;
    }
    writeln!(
        out,
        "Lease released: {name} in {session}. Pane closure confirmed (or owning server absent)."
    )?;
    let after = provider.list(None)?;
    if lease
        .vm_id
        .as_ref()
        .is_some_and(|id| after.vms.iter().any(|vm| vm.vm_id.as_ref() == Some(id)))
    {
        writeln!(out, "VM remains leased by another session.")?;
    } else {
        writeln!(out, "No remaining lease was reported for this VM. Check retained disks and merge warnings; release does not guarantee a successful merge.")?;
    }
    Ok(())
}

pub(crate) fn kill_all_sessions(yes: bool) {
    match sessions_to_kill(get_sessions_with_status) {
        Ok(sessions) if sessions.is_empty() => {
            eprintln!("No active yinz sessions found.");
            process::exit(1);
        },
        Ok(sessions) => {
            if !yes {
                println!("WARNING: this action will kill all sessions.");
                if !Confirm::new()
                    .with_prompt("Do you want to continue?")
                    .interact()
                    .unwrap()
                {
                    println!("Abort.");
                    process::exit(1);
                }
            }
            let mut all_delivered = true;
            for session in &sessions {
                if let Err(e) = kill_session_impl(&session.0) {
                    // One unreachable server must not stop the rest being
                    // killed, but it must not be reported as killed either.
                    log::error!("failed to kill session {:?}: {e}", session.0);
                    eprintln!("{}", undeliverable_kill_message(session.0.as_str()));
                    all_delivered = false;
                }
            }
            process::exit(if all_delivered { 0 } else { 1 });
        },
        Err(e) => {
            eprintln!("Error occurred: {:?}", e);
            process::exit(1);
        },
    }
}

pub(crate) fn delete_all_sessions(yes: bool, force: bool) {
    use std::collections::BTreeMap;
    use yinz_server::background_jobs::scan_session_list_default_dirs;

    let (_live_sessions, resurrectable_map) =
        scan_session_list_default_dirs(&SessionName::default(), &[], &BTreeMap::new());
    let mut resurrectable_sessions: Vec<(SessionName, Duration)> =
        resurrectable_map.into_iter().collect();
    if force {
        for entry in get_resurrectable_sessions() {
            if !resurrectable_sessions.iter().any(|(n, _)| *n == entry.0) {
                resurrectable_sessions.push(entry);
            }
        }
    }
    let dead_sessions = match sessions_to_delete(force, resurrectable_sessions, get_sessions) {
        Ok(dead_sessions) => dead_sessions,
        Err(kind) => {
            eprintln!("{}", session_listing_error_message(kind));
            process::exit(1);
        },
    };
    if !yes {
        println!("WARNING: this action will delete all resurrectable sessions.");
        if !Confirm::new()
            .with_prompt("Do you want to continue?")
            .interact()
            .unwrap()
        {
            println!("Abort.");
            process::exit(1);
        }
    }
    for session in &dead_sessions {
        delete_session_impl(session.0.as_str(), force);
    }
    process::exit(0);
}

fn sessions_to_delete(
    force: bool,
    resurrectable_sessions: Vec<(SessionName, Duration)>,
    get_active_sessions: impl FnOnce() -> Result<Vec<(SessionName, Duration)>, std::io::ErrorKind>,
) -> Result<Vec<(SessionName, Duration)>, std::io::ErrorKind> {
    if force {
        return Ok(resurrectable_sessions);
    }
    let active_sessions: Vec<SessionName> = get_active_sessions()?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    Ok(resurrectable_sessions
        .into_iter()
        .filter(|(name, _)| !active_sessions.contains(name))
        .collect())
}

/// Success requires a completion reply, or an absent session with reclaimed orphans.
fn kill_outcome(
    probe: Result<SessionProbe, std::io::ErrorKind>,
    complete: impl FnOnce() -> std::io::Result<()>,
    cleanup: impl FnOnce() -> std::result::Result<usize, String>,
) -> std::result::Result<(), String> {
    let shutdown = match probe {
        Ok(SessionProbe::Absent) => None,
        Ok(_) => Some(complete().map_err(|e| format!("server shutdown unconfirmed: {e}"))),
        Err(kind) => Some(Err(session_listing_error_message(kind))),
    };
    if matches!(shutdown, Some(Ok(()))) {
        return Ok(());
    }
    let cleaned = cleanup();
    match (shutdown, cleaned) {
        (None, Ok(count)) if count > 0 => Ok(()),
        (None, Ok(_)) => Err("No session or orphaned pane supervisors found.".to_owned()),
        (None, Err(e)) => Err(format!("orphan cleanup incomplete: {e}")),
        (Some(Err(e)), Ok(_)) => Err(e),
        (Some(Err(e)), Err(cleanup)) => Err(format!("{e}; orphan cleanup incomplete: {cleanup}")),
        (Some(Ok(())), _) => unreachable!(),
    }
}

pub(crate) fn kill_session(target_session: &Option<String>) {
    let Some(name) = target_session else {
        eprintln!("Please specify the session name to kill.");
        process::exit(1);
    };
    if let Err(e) = validate_session_name(name) {
        eprintln!("{e}");
        process::exit(1);
    }
    let outcome = kill_outcome(
        session_status(&SessionName::new(name.clone())),
        || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(10),
                    yinz_ipc::ipc::async_kill_session_with_completion(
                        &yinz_runtime::consts::YINZ_SOCK_DIR.join(name),
                    ),
                )
                .await
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "waiting for shutdown completion",
                    )
                })?
            })
        },
        || {
            yinz_process::pty_host::cli::terminate_orphans_of_session(name)
                .map(|terminated| terminated.len())
                .map_err(|e| format!("{e:#}"))
        },
    );
    match outcome {
        Ok(()) => process::exit(0),
        Err(e) => {
            eprintln!("Failed to kill session {name:?}: {e}");
            process::exit(1);
        },
    }
}

pub(crate) fn delete_session(target_session: &Option<String>, force: bool) {
    match target_session {
        Some(target_session) => {
            if let Err(e) = validate_session_name(target_session) {
                eprintln!("{}", e);
                process::exit(1);
            }
            assert_dead_session(target_session, force);
            delete_session_impl(target_session, force);
            process::exit(0);
        },
        None => {
            println!("Please specify the session name to delete.");
            process::exit(1);
        },
    }
}

fn get_os_input<OsInputOutput>(
    fn_get_os_input: impl FnOnce() -> Result<OsInputOutput, std::io::Error>,
) -> OsInputOutput {
    match fn_get_os_input() {
        Ok(os_input) => os_input,
        Err(e) => {
            eprintln!("failed to open terminal:\n{}", e);
            process::exit(1);
        },
    }
}

pub(crate) fn start_server(path: PathBuf, debug: bool, termios: Option<String>) {
    // Set instance-wide debug mode
    yinz_runtime::consts::DEBUG_MODE.set(debug).unwrap();
    let os_input = get_os_input(|| get_server_os_input_from_termios_arg(termios.as_deref()));
    start_server_impl(Box::new(os_input), path);
}

fn create_new_client() -> ClientInfo {
    ClientInfo::New(generate_unique_session_name_or_exit(), None, None)
}

fn find_indexed_session(
    sessions: Vec<SessionName>,
    config_options: Options,
    index: usize,
    create: bool,
) -> ClientInfo {
    match sessions.get(index) {
        Some(session) => ClientInfo::Attach(session.clone(), config_options),
        None if create => create_new_client(),
        None => {
            println!(
                "No session indexed by {} found. The following sessions are active:",
                index
            );
            print_sessions_with_index(sessions, current_session().as_ref());
            process::exit(1);
        },
    }
}

/// Client entrypoint for all [`yinz_cli::args::CliAction`]
///
/// Checks session to send the action to and attaches with client
pub(crate) fn send_action_to_session(
    mut cli_action: yinz_cli::args::CliAction,
    requested_session_name: Option<String>,
    config: Option<Config>,
) {
    let (requested_session_name, caller) = command_target(requested_session_name);
    fill_caller_pane(&mut cli_action, caller);
    if let Some(session_name) = requested_session_name {
        wait_for_command_session(&session_name);
        attach_with_cli_client(cli_action, &session_name, config, caller);
        return;
    }
    let named_store_action = matches!(
        cli_action,
        yinz_cli::args::CliAction::NotesTodos(_)
            | yinz_cli::args::CliAction::Operations(_)
            | yinz_cli::args::CliAction::Wait(_)
    );
    match get_active_session() {
        ActiveSession::None => {
            eprintln!("{}", no_active_session_message());
            std::process::exit(1);
        },
        ActiveSession::One(session_name) => {
            attach_with_cli_client(cli_action, &session_name, config, caller);
        },
        ActiveSession::Many => {
            let existing_sessions: Vec<SessionName> = get_sessions()
                .unwrap_or_default()
                .iter()
                .map(|s| s.0.clone())
                .collect();
            eprintln!("Please specify the session name to send actions to. The following sessions are active:");
            if named_store_action {
                for name in &existing_sessions {
                    eprintln!("{name}");
                }
            } else {
                list_sessions(false, false, true, false);
            }
            std::process::exit(1);
        },
    };
}

pub(crate) fn command_target(
    explicit: Option<String>,
) -> (Option<SessionName>, Option<TerminalId>) {
    yinz_process::pane_context::target(explicit.map(SessionName::new)).unwrap_or_else(|error| {
        eprintln!("{error:#}");
        process::exit(1);
    })
}

pub(crate) fn fill_caller_pane(action: &mut yinz_cli::args::CliAction, caller: Option<TerminalId>) {
    match action {
        yinz_cli::args::CliAction::AgentReport { pane_id, .. }
        | yinz_cli::args::CliAction::SetPaneColor { pane_id, .. }
            if pane_id.is_none() =>
        {
            *pane_id = caller.map(|id| id.to_string());
        },
        _ => {},
    }
}

pub(crate) fn current_session() -> Option<SessionName> {
    yinz_process::pane_context::current_pane()
        .ok()
        .flatten()
        .map(|(session, _)| session)
}

/// The session a `yinz subscribe` / `yinz events` client attaches to: the one running, the
/// one asked for, or the one this shell is inside. Anything else is an error naming the choices,
/// with `purpose` finishing the sentence ("subscribe to", "stream events from").
pub(crate) fn resolve_session_for_stream(
    requested_session_name: Option<String>,
    purpose: &str,
) -> SessionName {
    resolve_session_for_command(requested_session_name, purpose).0
}

pub(crate) fn resolve_session_for_command(
    requested_session_name: Option<String>,
    purpose: &str,
) -> (SessionName, Option<TerminalId>) {
    let (requested_session_name, caller) = command_target(requested_session_name);
    if let Some(session_name) = requested_session_name {
        wait_for_command_session(&session_name);
        return (session_name, caller);
    }
    let session = match get_active_session() {
        ActiveSession::None => {
            eprintln!("{}", no_active_session_message());
            std::process::exit(1);
        },
        ActiveSession::One(session_name) => session_name,
        ActiveSession::Many => {
            eprintln!(
                "Please specify the session name to {purpose}. The following sessions are active:"
            );
            list_sessions(false, false, true, false);
            std::process::exit(1);
        },
    };
    (session, caller)
}

fn wait_for_command_session(session_name: &SessionName) {
    command_session_ready(session_name, Duration::from_secs(30), session_status).unwrap_or_else(
        |error| {
            eprintln!("{error}");
            process::exit(1);
        },
    );
}

fn command_session_ready(
    session_name: &SessionName,
    within: Duration,
    mut probe: impl FnMut(&SessionName) -> Result<SessionProbe, std::io::ErrorKind>,
) -> Result<(), String> {
    validate_session_name(session_name.as_str())?;
    let deadline = std::time::Instant::now() + within;
    loop {
        let status = probe(session_name);
        if status != Ok(SessionProbe::Unresponsive) {
            return match named_session_exists_or_error(session_name, status)? {
                true => Ok(()),
                false => Err(format!("Session '{session_name}' not found.")),
            };
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(unresponsive_session_message(session_name.as_str()));
        }
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
        if std::time::Instant::now() >= deadline {
            return Err(unresponsive_session_message(session_name.as_str()));
        }
    }
}

#[cfg(test)]
#[path = "unit/command_session.rs"]
mod command_session_tests;

pub(crate) fn subscribe_to_session(
    subscribe_cli: yinz_cli::args::SubscribeCli,
    requested_session_name: Option<String>,
    _config: Option<Config>,
) {
    let session_name = resolve_session_for_stream(requested_session_name, "subscribe to");
    let os_input = get_os_input(yinz_client::os_input_output::get_cli_client_os_input);
    yinz_client::cli_client::start_subscribe_client(
        Box::new(os_input),
        &session_name,
        subscribe_cli,
    );
}

pub(crate) fn stream_session_events(
    events_cli: yinz_cli::args::EventsCli,
    requested_session_name: Option<String>,
    _config: Option<Config>,
) {
    let session_name = resolve_session_for_stream(requested_session_name, "stream events from");
    let os_input = get_os_input(yinz_client::os_input_output::get_cli_client_os_input);
    yinz_client::cli_client::start_events_client(Box::new(os_input), &session_name, events_cli);
}

fn attach_with_cli_client(
    cli_action: yinz_cli::args::CliAction,
    session_name: &SessionName,
    config: Option<Config>,
    caller: Option<TerminalId>,
) {
    if let yinz_cli::args::CliAction::Wait(request) = cli_action {
        match yinz_client::wait_client::wait(session_name, request) {
            Ok(result) => {
                println!("{result}");
                std::process::exit(0);
            },
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(1);
            },
        }
    }
    let service_action = match &cli_action {
        yinz_cli::args::CliAction::NotesTodos(command) => {
            Some(command.action(&mut std::io::stdin()))
        },
        yinz_cli::args::CliAction::Operations(command) => {
            Some(command.action(&mut std::io::stdin()))
        },
        _ => None,
    };
    if let Some(action) = service_action {
        let result = action
            .map_err(anyhow::Error::msg)
            .and_then(|action| yinz_client::store_client::request(session_name, action, caller));
        match result {
            Ok(data) => {
                println!("{data}");
                std::process::exit(0);
            },
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(1);
            },
        }
    }
    let os_input = get_os_input(yinz_client::os_input_output::get_cli_client_os_input);
    let get_current_dir = || std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match yinz_cli::actions::actions_from_cli(cli_action, Box::new(get_current_dir), config) {
        Ok(actions) => {
            let exit_status = yinz_client::cli_client::start_cli_client(
                Box::new(os_input),
                session_name,
                actions,
                caller,
            );
            std::process::exit(exit_status);
        },
        Err(e) => {
            eprintln!("{}", e);
            log::error!("Error sending action: {}", e);
            std::process::exit(2);
        },
    }
}

fn attach_with_session_index(config_options: Options, index: usize, create: bool) -> ClientInfo {
    // Ignore the session_name when `--index` is provided
    match get_sessions_sorted_by_mtime() {
        Ok(sessions) if sessions.is_empty() => {
            if create {
                create_new_client()
            } else {
                eprintln!("No active yinz sessions found.");
                process::exit(1);
            }
        },
        Ok(sessions) => find_indexed_session(sessions, config_options, index, create),
        Err(e) => {
            eprintln!("Error occurred: {:?}", e);
            process::exit(1);
        },
    }
}

fn named_session_exists_or_error(
    session_name: &SessionName,
    probe: Result<SessionProbe, std::io::ErrorKind>,
) -> Result<bool, String> {
    match probe {
        Ok(SessionProbe::Alive) => Ok(true),
        Ok(SessionProbe::Absent) => Ok(false),
        Ok(SessionProbe::Unresponsive) => Err(unresponsive_session_message(session_name.as_str())),
        Ok(SessionProbe::Denied) => Err(denied_session_message(session_name.as_str())),
        Err(kind) => Err(session_listing_error_message(kind)),
    }
}

/// `picker` is the config to switch sessions with when no name is given and
/// the choice is put to the person; `None` lists the sessions and exits instead.
fn attach_with_session_name(
    session_name: Option<SessionName>,
    config_options: Options,
    create: bool,
    picker: Option<&Config>,
) -> ClientInfo {
    match &session_name {
        Some(session) if create => {
            let session_exists = named_session_exists_or_error(session, session_status(session))
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    process::exit(1);
                });
            if session_exists {
                ClientInfo::Attach(session_name.unwrap(), config_options)
            } else {
                ClientInfo::New(session_name.unwrap(), None, None)
            }
        },
        Some(prefix) => match match_session_name(prefix.as_str()) {
            Ok(SessionNameMatch::UniquePrefix(s)) | Ok(SessionNameMatch::Exact(s)) => {
                ClientInfo::Attach(s, config_options)
            },
            Ok(SessionNameMatch::AmbiguousPrefix(sessions)) => {
                println!(
                    "Ambiguous selection: multiple sessions names start with '{}':",
                    prefix
                );
                print_sessions(
                    sessions
                        .iter()
                        .map(|s| (s.clone(), Duration::default(), SessionListing::Running))
                        .collect(),
                    false,
                    false,
                    true,
                    current_session().as_ref(),
                );
                process::exit(1);
            },
            Ok(SessionNameMatch::None) => {
                eprintln!("No session with the name '{}' found!", prefix);
                process::exit(1);
            },
            Err(kind) => {
                eprintln!("{}", session_listing_error_message(kind));
                process::exit(1);
            },
        },
        None => match get_active_session() {
            ActiveSession::None if create => create_new_client(),
            ActiveSession::None => {
                eprintln!("No active yinz sessions found.");
                process::exit(1);
            },
            ActiveSession::One(session_name) => ClientInfo::Attach(session_name, config_options),
            ActiveSession::Many => match picker {
                Some(config) => pick_session_to_attach(config_options, config),
                None => {
                    println!("Please specify the session to attach to, either by using the full name or a unique prefix.\nThe following sessions are active:");
                    list_sessions(false, false, true, false);
                    process::exit(1);
                },
            },
        },
    }
}

fn handle_client_outcome(outcome: yinz_client::ClientOutcome) -> Option<SessionConnection> {
    if outcome.exit_status != 0 {
        std::process::exit(outcome.exit_status);
    }
    outcome.reconnect_to_session
}

pub(crate) fn start_client(opts: CliArgs) {
    let (
        config,
        client_layout_info,
        config_options,
        mut config_without_layout,
        mut config_options_without_layout,
    ) = match Setup::from_cli_args(&opts) {
        Ok(results) => results,
        Err(e) => {
            if let ConfigError::KdlError(error) = e {
                let report: Report = error.into();
                eprintln!("{:?}", report);
            } else {
                eprintln!("{}", e);
            }
            process::exit(1);
        },
    };

    let mut reconnect_to_session: Option<SessionConnection> = None;
    let os_input = get_os_input(get_client_os_input);
    loop {
        let os_input = os_input.clone();
        let mut config = config.clone();
        let mut config_options = config_options.clone();
        let mut opts = opts.clone();
        let mut is_a_reconnect = false;
        let mut should_create_detached = false;
        let mut layout_info = client_layout_info.clone();
        let mut new_session_cwd = None;

        if let Some(reconnect_to_session) = &reconnect_to_session {
            // this is integration code to make session reconnects work with this existing,
            // untested and pretty involved function
            //
            // ideally, we should write tests for this whole function and refctor it
            reload_config_from_disk(
                &mut config_without_layout,
                &mut config_options_without_layout,
                &opts,
            );
            if reconnect_to_session.name.is_some() {
                opts.command = Some(Command::Sessions(Sessions::Attach {
                    session_name: reconnect_to_session
                        .name
                        .clone()
                        .map(SessionName::into_string),
                    create: true,
                    create_background: false,
                    force_run_commands: false,
                    index: None,
                    options: None,
                }));
            } else {
                opts.command = None;
                opts.session = None;
                config_options.attach_to_session = None;
            }

            if let Some(reconnect_layout) = &reconnect_to_session.layout {
                layout_info = Some(reconnect_layout.clone());
            }
            if let Some(cwd) = &reconnect_to_session.cwd {
                new_session_cwd = Some(cwd.clone());
            }
            config = config_without_layout.clone();
            config_options = config_options_without_layout.clone();
            is_a_reconnect = true;
        }

        let start_client_plan = |session_name: SessionName| {
            assert_session_ne(session_name.as_str());
        };

        if let Some(Command::Sessions(Sessions::Attach {
            session_name,
            create,
            create_background,
            force_run_commands,
            index,
            options,
        })) = opts.command.clone()
        {
            let config_options = match options.as_deref() {
                Some(SessionCommand::Options(o)) => {
                    yinz_cli::options::merge_from_cli(&config_options, o.to_owned().into())
                },
                None => config_options,
            };
            let session_name = session_name.map(SessionName::new);
            should_create_detached = create_background;

            let mut client = if let Some(idx) = index {
                attach_with_session_index(
                    config_options.clone(),
                    idx,
                    create || should_create_detached,
                )
            } else {
                let session_exists = session_name
                    .as_ref()
                    .map(|session_name| {
                        named_session_exists_or_error(session_name, session_status(session_name))
                            .unwrap_or_else(|error| {
                                eprintln!("{error}");
                                process::exit(1);
                            })
                    })
                    .unwrap_or(false);
                let resurrection_layout =
                    session_name
                        .as_ref()
                        .and_then(|s| match resurrection_layout(s) {
                            Ok(layout) => layout,
                            Err(e) => {
                                eprintln!("{}", e);
                                process::exit(2);
                            },
                        });
                if (create || should_create_detached)
                    && !session_exists
                    && resurrection_layout.is_none()
                {
                    session_name.clone().map(start_client_plan);
                }
                match (session_name.as_ref(), resurrection_layout) {
                    (Some(session_name), Some(mut resurrection_layout)) if !session_exists => {
                        if force_run_commands {
                            resurrection_layout.recursively_add_start_suspended(Some(false));
                        }
                        ClientInfo::Resurrect(
                            session_name.clone(),
                            session_layout_cache_file_name(session_name),
                            force_run_commands,
                            new_session_cwd.clone(),
                        )
                    },
                    _ => attach_with_session_name(
                        session_name,
                        config_options.clone(),
                        create || should_create_detached,
                        (!should_create_detached && can_pick_a_session()).then_some(&config),
                    ),
                }
            };

            if let Some(val) = current_session() {
                if &val == client.get_session_name() {
                    panic!("You are trying to attach to the current session (\"{}\"). This is not supported.", val);
                }
            }

            if let Some(layout_info) = layout_info {
                client.set_layout_info(layout_info);
            }

            if let Some(new_session_cwd) = new_session_cwd {
                client.set_cwd(new_session_cwd);
            }

            let tab_number_to_focus = reconnect_to_session.as_ref().and_then(|r| r.tab_position);
            let pane_id_to_focus = reconnect_to_session.as_ref().and_then(|r| r.pane_id);
            reconnect_to_session = handle_client_outcome(start_client_impl(
                Box::new(os_input),
                opts,
                config,
                config_options,
                client,
                tab_number_to_focus,
                pane_id_to_focus,
                is_a_reconnect,
                should_create_detached,
            ));
        } else {
            if let Some(session_name) = opts.session.clone().map(SessionName::new) {
                start_client_plan(session_name.clone());
                reconnect_to_session = handle_client_outcome(start_client_impl(
                    Box::new(os_input),
                    opts,
                    config,
                    config_options,
                    ClientInfo::New(session_name, layout_info, new_session_cwd),
                    None,
                    None,
                    is_a_reconnect,
                    should_create_detached,
                ));
            } else {
                if let Some(session_name) = config_options.session_name.as_ref() {
                    if let Some(val) = current_session() {
                        // This prevents the same type of recursion as above, only that here we
                        // don't get the command to "attach", but to start a new session instead.
                        // This occurs for example when declaring the session name inside a layout
                        // file and then, from within this session, trying to open a new yinz
                        // session with the same layout. This causes an infinite recursion in the
                        // `yinz_pty::terminal_bytes::listen` task, flooding the server and
                        // clients with infinite `Render` requests.
                        if *session_name == val {
                            eprintln!("You are trying to attach to the current session (\"{}\"). Yinz does not support nesting a session in itself.", session_name);
                            process::exit(1);
                        }
                    }
                    match config_options.attach_to_session {
                        Some(true) => {
                            let client = attach_with_session_name(
                                Some(session_name.clone()),
                                config_options.clone(),
                                true,
                                None,
                            );
                            reconnect_to_session = handle_client_outcome(start_client_impl(
                                Box::new(os_input),
                                opts,
                                config,
                                config_options,
                                client,
                                None,
                                None,
                                is_a_reconnect,
                                should_create_detached,
                            ));
                        },
                        _ => {
                            start_client_plan(session_name.clone());
                            reconnect_to_session = handle_client_outcome(start_client_impl(
                                Box::new(os_input),
                                opts,
                                config,
                                config_options.clone(),
                                ClientInfo::New(session_name.clone(), layout_info, new_session_cwd),
                                None,
                                None,
                                is_a_reconnect,
                                should_create_detached,
                            ));
                        },
                    }
                    if reconnect_to_session.is_some() {
                        continue;
                    }
                    // after we detach, this happens and so we need to exit before the rest of the
                    // function happens
                    process::exit(0);
                }

                let session_name = generate_unique_session_name_or_exit();
                start_client_plan(session_name.clone());
                reconnect_to_session = handle_client_outcome(start_client_impl(
                    Box::new(os_input),
                    opts,
                    config,
                    config_options,
                    ClientInfo::New(session_name, layout_info, new_session_cwd),
                    None,
                    None,
                    is_a_reconnect,
                    should_create_detached,
                ));
            }
        }
        if reconnect_to_session.is_none() {
            break;
        }
    }
}

fn generate_unique_session_name_or_exit() -> SessionName {
    let Some(unique_session_name) = generate_unique_session_name() else {
        eprintln!("Failed to generate a unique session name, giving up");
        process::exit(1);
    };
    unique_session_name
}

pub(crate) fn list_aliases(opts: CliArgs) {
    let (config, _layout, _config_options, _config_without_layout, _config_options_without_layout) =
        match Setup::from_cli_args(&opts) {
            Ok(results) => results,
            Err(e) => {
                if let ConfigError::KdlError(error) = e {
                    let report: Report = error.into();
                    eprintln!("{:?}", report);
                } else {
                    eprintln!("{}", e);
                }
                process::exit(1);
            },
        };
    for alias in config.plugins.list() {
        println!("{}", alias);
    }
    process::exit(0);
}

pub(crate) fn watch_session(session_name: Option<String>, opts: CliArgs) {
    let (config, _, config_options, _, _) = match Setup::from_cli_args(&opts) {
        Ok(results) => results,
        Err(e) => {
            if let ConfigError::KdlError(error) = e {
                let report: Report = error.into();
                eprintln!("{:?}", report);
            } else {
                eprintln!("{}", e);
            }
            process::exit(1);
        },
    };

    let session_name = session_name.or(opts.session.clone());
    // Resolve the session name to watch
    let client_info = match &session_name {
        Some(prefix) => match match_session_name(prefix.as_str()).unwrap() {
            SessionNameMatch::UniquePrefix(s) | SessionNameMatch::Exact(s) => {
                ClientInfo::Watch(s, config_options.clone())
            },
            SessionNameMatch::AmbiguousPrefix(sessions) => {
                eprintln!(
                    "Ambiguous selection: multiple sessions names start with '{}':",
                    prefix
                );
                print_sessions(
                    sessions
                        .iter()
                        .map(|s| (s.clone(), Duration::default(), SessionListing::Running))
                        .collect(),
                    false,
                    false,
                    true,
                    current_session().as_ref(),
                );
                process::exit(1);
            },
            SessionNameMatch::None => {
                eprintln!("No session with the name '{}' found!", prefix);
                process::exit(1);
            },
        },
        None if yinz_process::pane_context::inside_pane() => ClientInfo::Watch(
            resolve_session_for_stream(None, "watch"),
            config_options.clone(),
        ),
        None => match get_active_session() {
            ActiveSession::None => {
                eprintln!("No active yinz sessions found.");
                process::exit(1);
            },
            ActiveSession::One(name) => ClientInfo::Watch(name, config_options.clone()),
            ActiveSession::Many => {
                eprintln!("Please specify the session name to watch.");
                process::exit(1);
            },
        },
    };

    let mut opts = opts.clone();
    opts.session = Some(client_info.get_session_name().to_string());

    let os_input = get_os_input(get_client_os_input);

    // Start the watcher client
    handle_client_outcome(start_client_impl(
        Box::new(os_input),
        opts,
        config,
        config_options,
        client_info,
        None,  // tab_number_to_focus
        None,  // pane_id_to_focus
        false, // is_a_reconnect
        false, // should_create_detached
    ));
}

fn reload_config_from_disk(
    config_without_layout: &mut Config,
    config_options_without_layout: &mut Options,
    opts: &CliArgs,
) {
    match Setup::from_cli_args(opts) {
        Ok((_, _, _, reloaded_config_without_layout, reloaded_config_options_without_layout)) => {
            *config_without_layout = reloaded_config_without_layout;
            *config_options_without_layout = reloaded_config_options_without_layout;
        },
        Err(e) => {
            log::error!("Failed to reload config: {}", e);
        },
    };
}

#[cfg(unix)]
pub(crate) fn pty_hosts(command: yinz_cli::args::PtyHosts) -> anyhow::Result<()> {
    use yinz_cli::args::PtyHosts as PtyHostsCli;
    use yinz_server::pty_host::cli;
    match command {
        PtyHostsCli::List { json, host } => match host {
            Some(host) => cli::remote_list(&host, json),
            None => cli::list(json),
        },
        PtyHostsCli::Kill { host_id } => cli::kill(&host_id),
        PtyHostsCli::KillAll => cli::kill_all(),
        // Handled by `pty_host_adopt`: it needs a session, which the rest of this
        // command deliberately does not.
        PtyHostsCli::Adopt { .. } => Ok(()),
    }
}

/// `yinz pty-host adopt` — put a surviving pane back on screen.
///
/// Resolves the supervisor here, where the failures are legible without a
/// session, then hands the pane to one as an ordinary CLI action.
#[cfg(unix)]
pub(crate) fn pty_host_adopt(
    command: yinz_cli::args::PtyHosts,
    requested_session_name: Option<String>,
    config: Option<Config>,
) {
    use yinz_cli::args::PtyHosts as PtyHostsCli;
    use yinz_server::pty_host::cli;
    use yinz_types::input::actions::warp_placement;

    let PtyHostsCli::Adopt {
        host_id,
        host,
        session,
        focused_tab,
        floating,
    } = command
    else {
        return;
    };

    let (session, caller) = command_target(session.or(requested_session_name));
    let session = session.map(SessionName::into_string);
    // With `--host`, `host_id` is a broker pane token rather than a local
    // supervisor id, and the broker on that machine is the only thing that can
    // resolve it.
    let resolved = match &host {
        Some(host) => cli::remote_adopt_request(host, &host_id, session),
        None => cli::adopt_request(&host_id, session),
    };
    let (session, payload) = match resolved {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        },
    };

    let os_input = get_os_input(yinz_client::os_input_output::get_cli_client_os_input);
    let exit_status = yinz_client::cli_client::start_cli_client(
        Box::new(os_input),
        &SessionName::new(session),
        vec![Action::AdoptPane {
            payload: Box::new(payload),
            placement: warp_placement(focused_tab, floating),
        }],
        caller,
    );
    let _ = config;
    std::process::exit(exit_status);
}

/// `yinz plugins ...`: the plugin folder is the session's - `user_plugin_dir` in the config,
/// or `<config dir>/plugins` - so what is written here is what `plugin_reload` and a layout's
/// `file:` location find.
pub(crate) fn plugins(command: yinz_cli::args::Plugins, opts: &CliArgs) -> anyhow::Result<()> {
    use anyhow::Context;
    use yinz_cli::args::Plugins as PluginsCli;
    let config = if matches!(
        command,
        PluginsCli::Install { .. }
            | PluginsCli::Update { .. }
            | PluginsCli::Status { .. }
            | PluginsCli::Reload { .. }
            | PluginsCli::Check { .. }
    ) {
        yinz_config::config::Config::validate_plugin_overlays(opts.config_dir.as_deref())
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Some(
            yinz_cli::setup::load_config(opts)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?,
        )
    } else {
        yinz_cli::setup::load_config(opts).ok()
    };
    let plugin_dir = || {
        yinz_runtime::home::user_plugin_dir_or_default(
            config
                .as_ref()
                .and_then(|config| config.options.user_plugin_dir.clone()),
            opts.config_dir.clone(),
        )
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no plugin folder is configured; set `user_plugin_dir` in the config or pass \
             --config-dir"
            )
        })
    };
    if let PluginsCli::Test {
        name,
        file,
        kind,
        filter,
        timeout_ms,
        json,
    } = command
    {
        let options = yinz_types::authoring::PluginTestOptions {
            kind: serde_json::from_value(serde_json::Value::String(kind))?,
            filter,
            timeout_ms,
        };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signals = [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM]
            .map(|signal| signal_hook::flag::register(signal, cancel.clone()))
            .into_iter()
            .collect::<std::io::Result<Vec<_>>>()?;
        let report = match (name, file) {
            (_, Some(path)) => yinz_server::plugin_testing::run(&path, &options, None, &cancel)?,
            (Some(name), None) => {
                let directory = plugin_dir().ok();
                let directory = directory.as_deref().unwrap_or(std::path::Path::new(""));
                yinz_server::plugin_testing::run_named(directory, &name, &options, None, &cancel)?
            },
            _ => anyhow::bail!("give a plugin name or --file"),
        };
        for signal in signals {
            signal_hook::low_level::unregister(signal);
        }
        if json {
            println!("{}", serde_json::to_string(&report)?);
        } else {
            for test in &report.tests {
                println!("{} {} ({}ms)", test.status, test.name, test.duration_ms);
                if let Some(error) = &test.error {
                    eprintln!("{error}\n{}", test.output);
                }
            }
        }
        if !report.passed() {
            anyhow::bail!("plugin tests failed");
        }
        return Ok(());
    }
    let update_repositories = matches!(command, PluginsCli::Update { .. });
    let repository_status = matches!(command, PluginsCli::Status { .. });
    match command {
        PluginsCli::Install { aliases }
        | PluginsCli::Update { aliases }
        | PluginsCli::Status { aliases } => {
            use yinz_types::input::layout::RunPluginLocation;
            let config = config
                .as_ref()
                .context("repository commands require configuration")?;
            let repositories = config
                .plugins
                .aliases
                .iter()
                .filter_map(|(name, plugin)| match &plugin.location {
                    RunPluginLocation::Git(source) => Some((name, source)),
                    _ => None,
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            let selected = if aliases.is_empty() {
                repositories.keys().map(|name| (*name).clone()).collect()
            } else {
                aliases
            };
            for name in &selected {
                if !repositories.contains_key(name) {
                    anyhow::bail!("{name} is not a configured repository plugin");
                }
            }
            let data_dir = opts
                .data_dir
                .clone()
                .unwrap_or_else(yinz_runtime::home::get_default_data_dir);
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let signals = [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM]
                .map(|signal| signal_hook::flag::register(signal, cancel.clone()))
                .into_iter()
                .collect::<std::io::Result<Vec<_>>>()?;
            let result = (|| -> anyhow::Result<()> {
                let mut completed = std::collections::BTreeSet::new();
                for name in selected {
                    let source = repositories[&name];
                    if !repository_status
                        && completed.insert((source.repo.clone(), source.revision.clone()))
                    {
                        let entries = repositories
                            .values()
                            .filter(|other| {
                                other.repo == source.repo && other.revision == source.revision
                            })
                            .map(|other| (*other).clone())
                            .collect::<Vec<_>>();
                        yinz_server::plugin_authoring::git::install(
                            &data_dir,
                            source,
                            &entries,
                            update_repositories,
                            &cancel,
                        )?;
                    }
                    let installed = yinz_server::plugin_authoring::git::status(&data_dir, source)?;
                    let selector = source
                        .revision
                        .as_ref()
                        .map(|value| {
                            let (key, value) = value.property();
                            format!("{key}={value}")
                        })
                        .unwrap_or_else(|| {
                            if source.is_local() {
                                "local".into()
                            } else {
                                "default branch".into()
                            }
                        });
                    let revision = installed
                        .as_ref()
                        .map(|value| value.commit.as_deref().unwrap_or("user-managed"))
                        .unwrap_or("not installed");
                    println!(
                        "{name}: {} ({selector}), entrypoint={}, revision={revision}",
                        source.repo,
                        source.entrypoint.display()
                    );
                }
                Ok(())
            })();
            for signal in signals {
                signal_hook::low_level::unregister(signal);
            }
            result
        },
        PluginsCli::Test { .. } => unreachable!("tests return above"),
        PluginsCli::Docs { section } => {
            let content = match section.as_str() {
                "index" => "guide: plugin authoring guide\nsdk: TypeScript API declarations\ntesting: plugin test API\nrepositories: Git repository plugins",
                "guide" => concat!("Author plugins with yinz:plugin and confined relative imports. Use plugins write, check --json, test --json, reload, then log. Read sdk for the full API. Permissions must be requested explicitly. Complete executable example:\n\n", include_str!("../example-plugins/clock.ts"), "\n\nBroader host API example:\n\n", include_str!("../example-plugins/capabilities.ts")),
                "repositories" => include_str!("../docs/GIT_PLUGINS.md"),
                "sdk" => include_str!("../yinz-plugin-host/src/deno/sdk.d.ts"),
                "testing" => include_str!("../yinz-plugin-host/src/deno/testing/testing.d.ts"),
                _ => anyhow::bail!("unknown docs section; use index, guide, sdk, testing or repositories"),
            };
            println!(
                "{}",
                serde_json::json!({"section":section,"content":content})
            );
            Ok(())
        },
        PluginsCli::List => {
            fn sources(root: &Path, paths: &mut Vec<PathBuf>) -> anyhow::Result<()> {
                if !root.exists() {
                    return Ok(());
                }
                for entry in std::fs::read_dir(root)? {
                    let entry = entry?;
                    let kind = entry.file_type()?;
                    if kind.is_dir() {
                        sources(&entry.path(), paths)?;
                    } else if kind.is_file()
                        && yinz_plugin_api::authoring::is_deno_plugin(&entry.path())
                    {
                        paths.push(entry.path());
                    }
                }
                Ok(())
            }
            let directory = plugin_dir()?;
            let mut files = Vec::new();
            sources(&directory, &mut files)?;
            files.sort();
            println!(
                "{}",
                serde_json::json!({"directory":directory,"files":files})
            );
            Ok(())
        },
        PluginsCli::Edit { name, file } => {
            let edits: Vec<yinz_plugin_api::authoring::edits::EditPair> =
                serde_json::from_str(&read_source(Some(&file))?)?;
            let written = yinz_server::plugin_authoring::edit(&plugin_dir()?, &name, &edits)?;
            println!(
                "{}",
                serde_json::json!({"path":written.path,"backup":written.replaced})
            );
            Ok(())
        },
        PluginsCli::Dir => {
            println!("{}", plugin_dir()?.display());
            Ok(())
        },
        PluginsCli::Write {
            name,
            file,
            overwrite,
            no_check,
        } => {
            let source = read_source(file.as_deref())?;
            let written = yinz_server::plugin_authoring::write(
                &plugin_dir()?,
                &name,
                &source,
                overwrite,
                !no_check,
            )?;
            println!("{}", written.path.display());
            if let Some(replaced) = written.replaced {
                eprintln!("kept the version it replaced as {}", replaced.display());
            }
            // On stderr, and after the path: the path is the answer a script reads, and a
            // warning is for the person watching. Never a refusal - see `plugin_authoring::lint`.
            // The write already checked syntax, so only the warnings are wanted here.
            print_plugin_findings(
                &yinz_server::plugin_authoring::check_source(
                    &written.path,
                    &source,
                    yinz_types::authoring::PluginCheckMode::Syntax,
                )
                .warnings,
            );
            Ok(())
        },
        PluginsCli::Check { name, file, json } => {
            if file.is_none() {
                if let Some(yinz_types::input::layout::RunPluginLocation::Git(source)) = name
                    .as_ref()
                    .and_then(|name| config.as_ref()?.plugins.aliases.get(name))
                    .map(|plugin| &plugin.location)
                {
                    let data_dir = opts
                        .data_dir
                        .clone()
                        .unwrap_or_else(yinz_runtime::home::get_default_data_dir);
                    let installed = yinz_server::plugin_authoring::git::status(&data_dir, source)?
                        .context("repository is not installed; use plugins install")?;
                    let report = yinz_server::plugin_authoring::check_repository(&installed)?;
                    if json {
                        println!("{}", serde_json::to_string(&report)?);
                    }
                    if !report.diagnostics.is_empty() {
                        anyhow::bail!("{}", report.diagnostics.join("\n"));
                    }
                    if !json {
                        println!("{} checks successfully", installed.entrypoint.display());
                        print_plugin_findings(&report.warnings);
                    }
                    return Ok(());
                }
            }

            let path = match (name, file) {
                (_, Some(file)) => file,
                (Some(name), None) => {
                    yinz_plugin_api::authoring::resolve_plugin_file(&plugin_dir()?, &name)
                        .map_err(|e| anyhow::anyhow!(e))?
                },
                (None, None) => anyhow::bail!("give a plugin name, or --file"),
            };
            if json {
                let source = read_source(Some(&path))?;
                let report = yinz_server::plugin_authoring::check_source(
                    &path,
                    &source,
                    yinz_types::authoring::PluginCheckMode::Semantic,
                );
                println!("{}", serde_json::to_string(&report)?);
                if !report.diagnostics.is_empty() {
                    anyhow::bail!("plugin checks failed");
                }
                return Ok(());
            }
            let report = yinz_server::plugin_authoring::check_plugin_report(&path)?;
            println!("{} checks successfully", path.display());
            print_plugin_findings(&report.warnings);
            Ok(())
        },
        PluginsCli::Format { name, file, check } => {
            let path = match (name, file) {
                (_, Some(file)) => file,
                (Some(name), None) => {
                    yinz_plugin_api::authoring::resolve_plugin_file(&plugin_dir()?, &name)
                        .map_err(|e| anyhow::anyhow!(e))?
                },
                (None, None) => anyhow::bail!("give a plugin name, or --file"),
            };
            yinz_server::plugin_authoring::format_file(&path, check)?;
            println!("{} is formatted", path.display());
            Ok(())
        },
        PluginsCli::Reload { name, headless } => {
            let url = if config
                .as_ref()
                .is_some_and(|config| config.plugins.aliases.contains_key(&name))
            {
                name
            } else {
                let path = yinz_plugin_api::authoring::resolve_plugin_file(&plugin_dir()?, &name)
                    .map_err(|e| anyhow::anyhow!(e))?;
                if !path.is_file() {
                    anyhow::bail!("{} is not there; write it first", path.display());
                }
                format!("file:{}", path.display())
            };
            if headless {
                let (session, caller) = resolve_session_for_command(
                    opts.session.clone(),
                    "reload a headless plugin in",
                );
                let action = yinz_cli::notes_todos::pipe_action(
                    "agent_action",
                    serde_json::json!({"op":"plugin_reload","args":{"url":url},"structured_reply":true}),
                );
                println!(
                    "{}",
                    yinz_client::store_client::request(&session, action, caller)?
                );
                return Ok(());
            }
            send_action_to_session(
                yinz_cli::args::CliAction::StartOrReloadPlugin {
                    url,
                    configuration: None,
                },
                opts.session.clone(),
                config,
            );
            Ok(())
        },
        PluginsCli::Log { name, lines } => {
            let path = yinz_plugin_api::authoring::resolve_plugin_file(&plugin_dir()?, &name)
                .map_err(|e| anyhow::anyhow!(e))?;
            let log_file = yinz_runtime::consts::YINZ_TMP_LOG_FILE.clone();
            print!(
                "{}",
                yinz_server::plugin_authoring::log_tail(&log_file, &path, lines)?
            );
            Ok(())
        },
    }
}

/// What the source is going to do wrong at run time, on stderr, under whatever the command
/// printed. Compiling is the verdict; these are the traps a compile cannot see.
fn print_plugin_findings(findings: &[String]) {
    for finding in findings {
        eprintln!("warning: {finding}");
    }
}

pub(crate) fn read_source(file: Option<&Path>) -> anyhow::Result<String> {
    match file {
        Some(file) if file != Path::new("-") => std::fs::read_to_string(file)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", file.display())),
        _ => {
            use std::io::Read;
            let mut source = String::new();
            std::io::stdin()
                .read_to_string(&mut source)
                .map_err(|e| anyhow::anyhow!("failed to read the plugin source from stdin: {e}"))?;
            Ok(source)
        },
    }
}

pub(crate) fn recordings(
    command: yinz_cli::args::Recordings,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    use yinz_cli::args::Recordings as RecordingsCli;
    let data_access = yinz_storage::data_access::DataAccess::sqlite(&data_dir);
    match command {
        RecordingsCli::List { json } => {
            let summaries = data_access.recordings().list()?;
            if json {
                println!("{}", serde_json::to_string(&summaries)?);
            } else {
                print!("{}", format_recordings_list(&summaries));
            }
            Ok(())
        },
        RecordingsCli::Export {
            recording_id,
            output,
        } => {
            if let Some(ref path) = output {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map_err(|e| {
                        if e.kind() == std::io::ErrorKind::AlreadyExists {
                            anyhow::anyhow!("output file {:?} already exists", path)
                        } else {
                            e.into()
                        }
                    })?;
                data_access.recordings().export(recording_id, &mut file)?;
            } else {
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                data_access.recordings().export(recording_id, &mut handle)?;
            }
            Ok(())
        },
        RecordingsCli::Search { query, limit, json } => {
            let hits = data_access.recordings().search(&query, limit)?;
            if json {
                println!("{}", serde_json::to_string(&hits)?);
            } else {
                print!("{}", format_search_results(&hits));
            }
            Ok(())
        },
        RecordingsCli::Compact => {
            let result = data_access.recordings().compact_all()?;
            print!("{}", format_compaction(&result));
            Ok(())
        },
    }
}

pub(crate) fn format_compaction(
    result: &yinz_storage::session_recording::StoreCompaction,
) -> String {
    let moved = result.compaction;
    let mut out = if moved.events == 0 {
        "Nothing new to compress.\n".to_string()
    } else {
        format!(
            "Compressed {} events ({}) into {} segments ({}).\n",
            moved.events,
            format_bytes(moved.raw_bytes),
            moved.segments,
            format_bytes(moved.stored_bytes),
        )
    };
    out.push_str(&format!(
        "Recording database: {} -> {}\n",
        format_bytes(result.bytes_before),
        format_bytes(result.bytes_after),
    ));
    out
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub(crate) fn format_search_results(hits: &[yinz_storage::session_recording::SearchHit]) -> String {
    if hits.is_empty() {
        return "No matches.\n".to_string();
    }
    let mut out = String::from("ID  TIME  SESSION  TAB  PANE  TEXT\n");
    for hit in hits {
        let pane = if hit.pane_title.is_empty() {
            "-"
        } else {
            &hit.pane_title
        };
        out.push_str(&format!(
            "{}  {}  {}  {}  {}  {}\n",
            hit.recording_id,
            format_duration(hit.elapsed_us),
            hit.session_name,
            hit.tab_name,
            pane,
            truncate_chars(&hit.text, 120),
        ));
    }
    out
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

pub(crate) fn format_recordings_list(
    summaries: &[yinz_storage::session_recording::RecordingSummary],
) -> String {
    // Header as specified in plan
    let mut out = String::new();
    out.push_str("ID  SESSION  TAB  PANE  STARTED  DURATION  INPUT  STATE\n");
    for s in summaries {
        let started = format_started(s.started_at_us);
        let duration = if s.is_running {
            "running".to_string()
        } else {
            format_duration(s.duration_us)
        };
        let input = if s.capture_input { "yes" } else { "no" };
        let state = if s.is_running {
            "running"
        } else if s.error.is_some() {
            "incomplete"
        } else {
            "complete"
        };
        // PANE column: terminal_id plus title
        let pane_col = if s.pane_title.is_empty() {
            format!("{}", s.terminal_id)
        } else {
            format!("{} {}", s.terminal_id, s.pane_title)
        };
        // Use simple space-separated formatting; header checks only contain substrings
        out.push_str(&format!(
            "{}  {}  {}  {}  {}  {}  {}  {}\n",
            s.id, s.session_name, s.tab_name, pane_col, started, duration, input, state
        ));
    }
    out
}

fn format_started(started_at_us: UnixMicros) -> String {
    let t = std::time::UNIX_EPOCH + Duration::from_micros(started_at_us.get().max(0) as u64);
    humantime::format_rfc3339_seconds(t).to_string()
}

fn format_duration(duration_us: ElapsedMicros) -> String {
    let secs = duration_us.get() as f64 / 1_000_000.0;
    if secs < 60.0 {
        format!("{:.1}s", secs)
    } else if secs < 3600.0 {
        format!("{:.1}m", secs / 60.0)
    } else {
        format!("{:.1}h", secs / 3600.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        kill_outcome, named_session_exists_or_error, sessions_to_delete, sessions_to_kill,
    };
    use std::{cell::Cell, io, time::Duration};
    use {
        yinz_cli::sessions::session_listing_error_message,
        yinz_cli::sessions::unresponsive_session_message, yinz_ipc::probe::SessionProbe,
    };

    fn session(name: &str) -> (yinz_types::internal_ids::SessionName, Duration) {
        (name.into(), Duration::default())
    }

    #[test]
    fn session_deletion_fails_when_live_session_discovery_fails() {
        let result = sessions_to_delete(false, vec![session("dead")], || {
            Err(io::ErrorKind::PermissionDenied)
        });

        assert_eq!(result.unwrap_err(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn session_deletion_excludes_live_sessions() {
        let sessions = sessions_to_delete(false, vec![session("live"), session("dead")], || {
            Ok(vec![session("live")])
        })
        .unwrap();

        assert_eq!(sessions, vec![session("dead")]);
    }

    #[test]
    fn forced_session_deletion_skips_live_session_discovery() {
        let discovery_called = Cell::new(false);
        let eligible = vec![session("live"), session("dead")];
        let sessions = sessions_to_delete(true, eligible.clone(), || {
            discovery_called.set(true);
            Ok(vec![session("live")])
        })
        .unwrap();

        assert_eq!(sessions, eligible);
        assert!(!discovery_called.get());
    }

    #[test]
    fn session_killing_includes_every_non_absent_socket() {
        let sessions = sessions_to_kill(|| {
            Ok(vec![
                (
                    "alive".to_owned().into(),
                    Duration::default(),
                    SessionProbe::Alive,
                ),
                (
                    "wedged".to_owned().into(),
                    Duration::default(),
                    SessionProbe::Unresponsive,
                ),
                (
                    "absent".to_owned().into(),
                    Duration::default(),
                    SessionProbe::Absent,
                ),
            ])
        })
        .unwrap();

        assert_eq!(sessions, vec![session("alive"), session("wedged")]);
    }

    #[test]
    fn a_kill_requires_completion_and_always_cleans_up_unconfirmed_shutdowns() {
        use std::cell::Cell;
        for probe in [SessionProbe::Alive, SessionProbe::Unresponsive] {
            assert!(kill_outcome(Ok(probe), || Ok(()), || panic!("already complete")).is_ok());
            for kind in [
                io::ErrorKind::TimedOut,
                io::ErrorKind::UnexpectedEof,
                io::ErrorKind::ConnectionRefused,
                io::ErrorKind::BrokenPipe,
                io::ErrorKind::InvalidData,
                io::ErrorKind::Other,
            ] {
                let swept = Cell::new(false);
                let result = kill_outcome(
                    Ok(probe),
                    || Err(kind.into()),
                    || {
                        swept.set(true);
                        Ok(1)
                    },
                );
                assert!(swept.get());
                assert!(result.is_err(), "fallback must not confirm server shutdown");
            }
        }
        assert!(kill_outcome(Ok(SessionProbe::Absent), || panic!("absent"), || Ok(1)).is_ok());
        assert!(kill_outcome(Ok(SessionProbe::Absent), || panic!("absent"), || Ok(0)).is_err());
        for probe in [
            Ok(SessionProbe::Absent),
            Err(io::ErrorKind::PermissionDenied),
        ] {
            let result = kill_outcome(
                probe,
                || panic!("not deliverable"),
                || Err("cleanup failed".into()),
            );
            assert!(result.unwrap_err().contains("cleanup failed"));
        }
    }

    #[test]
    fn only_an_absent_named_session_allows_creation() {
        let creation_allowed = |probe| {
            named_session_exists_or_error(
                &yinz_types::internal_ids::SessionName::new("named"),
                probe,
            )
            .map(|session_exists| !session_exists)
        };

        assert_eq!(creation_allowed(Ok(SessionProbe::Alive)), Ok(false));
        assert_eq!(creation_allowed(Ok(SessionProbe::Absent)), Ok(true));
        assert_eq!(
            creation_allowed(Ok(SessionProbe::Unresponsive)),
            Err(unresponsive_session_message("named"))
        );
        assert_eq!(
            creation_allowed(Err(io::ErrorKind::PermissionDenied)),
            Err(session_listing_error_message(
                io::ErrorKind::PermissionDenied
            ))
        );
    }
}

#[cfg(test)]
mod firecracker_command_tests {
    use super::{close_firecracker_lease, resolve_firecracker_session, run_firecracker};
    use serde_json::json;
    use std::{
        fs,
        path::{Path, PathBuf},
        time::Duration,
    };
    use tempfile::TempDir;
    use yinz_cli::args::Firecracker;
    use yinz_cli::sessions::ActiveSession;
    use yinz_server::firecracker::Provider;
    use yinz_test_support::write_warm_script;

    #[test]
    fn branch_lookup_stays_at_the_supplied_worktree_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-b", "root-branch"]);
        assert_eq!(super::git_branch(&root), None, "unborn branch");
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ]);
        assert_eq!(super::git_branch(&root).as_deref(), Some("root-branch"));
        let nested = root.join("not-a-repo");
        fs::create_dir(&nested).unwrap();
        assert_eq!(
            super::git_branch(&nested),
            None,
            "must not discover the enclosing repo"
        );
        assert_eq!(super::git_branch(&dir.path().join("missing")), None);
        let linked = dir.path().join("linked");
        git(&[
            "worktree",
            "add",
            "-b",
            "linked-branch",
            linked.to_str().unwrap(),
        ]);
        assert_eq!(super::git_branch(&linked).as_deref(), Some("linked-branch"));
        git(&["checkout", "--detach"]);
        assert_eq!(super::git_branch(&root), None, "detached HEAD");
    }

    /// A provider that logs each request to `requests` beside itself and answers
    /// every one with `response`.
    fn stub_provider(response: &str) -> (TempDir, Provider) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider");
        write_warm_script(
            &path,
            &format!("cat >> \"$(dirname \"$0\")/requests\"\nprintf '%s' '{response}'\n"),
        );
        let provider = Provider::new(path, Duration::from_secs(5)).unwrap();
        (dir, provider)
    }

    fn requests(dir: &TempDir) -> Vec<serde_json::Value> {
        serde_json::Deserializer::from_str(
            &fs::read_to_string(dir.path().join("requests")).unwrap(),
        )
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    /// Every worktree is on a branch named after its last path component, except
    /// `/detached`, which has none. Enough to tell "resolved", "no worktree" and
    /// "worktree but no branch" apart without going near a real checkout.
    fn stub_branch(worktree: &Path) -> Option<String> {
        if worktree == Path::new("/detached") {
            return None;
        }
        Some(worktree.file_name()?.to_string_lossy().into_owned())
    }

    fn run(command: Firecracker, provider: &Provider) -> (String, String) {
        run_with_branches(command, provider, &stub_branch)
    }

    fn run_with_branches(
        command: Firecracker,
        provider: &Provider,
        branch_of: &dyn Fn(&Path) -> Option<String>,
    ) -> (String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        run_firecracker(
            command,
            provider,
            branch_of,
            &|_| Ok(vec![]),
            &mut out,
            &mut err,
        )
        .unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn list(session: Option<&str>, json: bool) -> Firecracker {
        Firecracker::List {
            session: session.map(str::to_owned),
            json,
        }
    }

    #[test]
    fn an_explicit_session_wins_over_the_environment_and_the_active_one() {
        assert_eq!(
            resolve_firecracker_session(
                Some("explicit".to_owned()),
                Some("env".to_owned().into()),
                ActiveSession::One("active".to_owned().into()),
            )
            .unwrap(),
            "explicit"
        );
    }

    #[test]
    fn the_environment_session_wins_over_the_active_one() {
        assert_eq!(
            resolve_firecracker_session(
                None,
                Some("env".to_owned().into()),
                ActiveSession::One("active".to_owned().into()),
            )
            .unwrap(),
            "env"
        );
    }

    #[test]
    fn a_single_active_session_is_used_when_nothing_else_selects_one() {
        assert_eq!(
            resolve_firecracker_session(None, None, ActiveSession::One("active".to_owned().into()))
                .unwrap(),
            "active"
        );
    }

    #[test]
    fn no_session_and_an_ambiguous_one_are_both_refused() {
        for active in [ActiveSession::None, ActiveSession::Many] {
            let error = resolve_firecracker_session(None, None, active)
                .unwrap_err()
                .to_string();
            assert_eq!(error, "no session selected; pass --session SESSION");
        }
    }

    #[test]
    fn list_json_reads_each_sessions_live_panes_once_and_matches_vm_names() {
        use yinz_types::data::{PaneInfo, PaneListEntry};
        let (_dir, provider) = stub_provider(
            r#"{"version":2,"vms":[{"session":"s","name":"api","state":"running"},{"session":"s","name":"other","state":"running"}]}"#,
        );
        let calls = std::cell::Cell::new(0);
        let panes = |session: &str| {
            assert_eq!(session, "s");
            calls.set(calls.get() + 1);
            Ok(vec![PaneListEntry {
                pane_info: PaneInfo {
                    title: "API shell".into(),
                    target: yinz_types::command::PaneTarget::Firecracker {
                        vm: "api".into(),
                        image: None,
                        worktree: None,
                    },
                    ..Default::default()
                },
                tab_id: Default::default(),
                tab_position: Default::default(),
                tab_name: "work".into(),
                pane_command: None,
                pane_cwd: None,
            }])
        };
        let mut out = vec![];
        run_firecracker(
            list(None, true),
            &provider,
            &|_| None,
            &panes,
            &mut out,
            &mut vec![],
        )
        .unwrap();
        let result: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(result["vms"][0]["panes"][0]["title"], "API shell");
        assert!(result["vms"][1].get("panes").is_none());
    }

    #[test]
    fn list_says_so_when_there_are_no_vms() {
        let (dir, provider) = stub_provider(r#"{"version":2,"vms":[]}"#);
        let (out, err) = run(list(None, false), &provider);
        assert_eq!(out, "No Firecracker VMs found.\n");
        assert_eq!(err, "");
        assert_eq!(
            requests(&dir),
            vec![json!({"version": 2, "action": "list", "session": null})]
        );
    }

    #[test]
    fn list_prints_one_tab_separated_line_per_vm() {
        let (dir, provider) = stub_provider(
            r#"{"version":2,"vms":[{"session":"work","name":"review","state":"running"},{"session":"other","name":"build","state":"provisioning"}]}"#,
        );
        let (out, err) = run(list(Some("work"), false), &provider);
        assert_eq!(
            out,
            "work\treview\trunning\t-\t-\t-\nother\tbuild\tprovisioning\t-\t-\t-\n"
        );
        assert_eq!(err, "");
        assert_eq!(
            requests(&dir),
            vec![json!({"version": 2, "action": "list", "session": "work"})]
        );
    }

    #[test]
    fn list_prints_retained_disks_after_the_vms() {
        let (_dir, provider) = stub_provider(
            r#"{"version":2,"vms":[{"session":"work","name":"review","state":"running"}],"recoveries":[{"path":"/state/recovery/abc-1-sync-failed","reason":"sync-failed","retained_at":1,"session":"work","name":"review","worktree":"/repo"},{"path":"/state/recovery/def-2-crashed","reason":"crashed","retained_at":2}]}"#,
        );
        let (out, _) = run(list(None, false), &provider);
        assert_eq!(
            out,
            "work\treview\trunning\t-\t-\t-\n\
             Retained disks:\n\
             work\treview\tsync-failed\t/state/recovery/abc-1-sync-failed\n\
             -\t-\tcrashed\t/state/recovery/def-2-crashed\n"
        );
    }

    #[test]
    fn list_json_is_one_object_with_vms_and_recoveries() {
        let (_dir, provider) = stub_provider(
            r#"{"version":2,"vms":[{"session":"work","name":"review","state":"running"}],"recoveries":[{"path":"/state/recovery/def-2-crashed","reason":"crashed","retained_at":2}]}"#,
        );
        let (out, err) = run(list(None, true), &provider);
        assert_eq!(err, "");
        assert_eq!(out.matches('\n').count(), 1, "{out:?}");
        let printed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            printed,
            json!({
                "vms": [{"session": "work", "name": "review", "state": "running"}],
                "recoveries": [{
                    "path": "/state/recovery/def-2-crashed",
                    "reason": "crashed",
                    "retained_at": 2,
                    "session": null,
                    "name": null,
                    "worktree": null,
                }],
            })
        );

        let (_dir, provider) = stub_provider(r#"{"version":2,"vms":[]}"#);
        let (out, _) = run(list(None, true), &provider);
        assert_eq!(out, "{\"vms\":[],\"recoveries\":[]}\n");
    }

    #[test]
    fn list_names_the_image_worktree_and_branch_of_each_vm() {
        let (_dir, provider) = stub_provider(
            r#"{"version":2,"vms":[{"session":"work","name":"review","state":"running","image":"rust-dev","worktree":"/src/api"},{"session":"work","name":"detached","state":"running","worktree":"/detached"},{"session":"work","name":"bare","state":"provisioning"}]}"#,
        );
        let (out, err) = run(list(None, false), &provider);
        assert_eq!(err, "");
        // Three shapes, and a `-` for each thing that cannot be named: a VM with
        // everything, one whose worktree has no branch out, and one with no worktree
        // at all - the VM an agent got to itself.
        assert_eq!(
            out,
            "work\treview\trunning\trust-dev\t/src/api\tapi\n\
             work\tdetached\trunning\t-\t/detached\t-\n\
             work\tbare\tprovisioning\t-\t-\t-\n"
        );
    }

    #[test]
    fn list_json_carries_the_branch_beside_the_provider_fields() {
        let (_dir, provider) = stub_provider(
            r#"{"version":2,"vms":[{"session":"work","name":"review","state":"running","image":"rust-dev","worktree":"/src/api"},{"session":"work","name":"bare","state":"running"}]}"#,
        );
        let (out, _) = run(list(None, true), &provider);
        let printed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            printed["vms"],
            json!([
                {
                    "session": "work",
                    "name": "review",
                    "state": "running",
                    "image": "rust-dev",
                    "worktree": "/src/api",
                    "branch": "api",
                },
                // What cannot be named is left out rather than sent as null, so a
                // reader has one rule for all three optional fields.
                {"session": "work", "name": "bare", "state": "running"},
            ])
        );
    }

    /// Two sessions leasing one worktree VM are two entries naming one directory.
    /// Resolving the branch is a process launch, so it happens once.
    #[test]
    fn a_worktree_shared_by_two_leases_is_resolved_once() {
        let (_dir, provider) = stub_provider(
            r#"{"version":2,"vms":[{"session":"work","name":"review","state":"running","worktree":"/src/api"},{"session":"other","name":"review","state":"running","worktree":"/src/api"}]}"#,
        );
        let calls = std::cell::RefCell::new(Vec::new());
        let (out, _) = run_with_branches(list(None, false), &provider, &|worktree: &Path| {
            calls.borrow_mut().push(worktree.to_path_buf());
            Some("api".to_owned())
        });
        assert_eq!(calls.into_inner(), vec![PathBuf::from("/src/api")]);
        assert_eq!(
            out,
            "work\treview\trunning\t-\t/src/api\tapi\n\
             other\treview\trunning\t-\t/src/api\tapi\n"
        );
    }

    #[test]
    fn guarded_vm_dispatch_sends_the_confirmed_identity_unchanged() {
        let expected: yinz_types::firecracker::Identity = serde_json::from_value(json!({
            "session": "work", "name": "review", "vm_id": "/vm/socket",
            "generation": "42:100", "worktree": "/repo with spaces",
        }))
        .unwrap();
        for (command, action) in [
            (
                Firecracker::Sync {
                    name: "review".into(),
                    session: Some("work".into()),
                    expected_identity: Some(expected.clone()),
                },
                "sync_if_matches",
            ),
            (
                Firecracker::Changes {
                    name: "review".into(),
                    session: Some("work".into()),
                    expected_identity: Some(expected.clone()),
                },
                "changes_if_matches",
            ),
        ] {
            let (dir, provider) = stub_provider(r#"{"version":2,"changes":3}"#);
            run(command, &provider);
            assert_eq!(
                requests(&dir),
                [json!({"version": 2, "action": action, "expected": expected})]
            );
        }
    }

    #[test]
    fn close_refuses_incomplete_identities_before_any_pane_close_request() {
        let complete: yinz_types::firecracker::VmEntry = serde_json::from_value(json!({
            "session": "work", "name": "review", "state": "running", "vm_id": "/vm/socket",
            "metrics": {"generation": "42:100", "source": "process", "sample_time_us": 0},
        }))
        .unwrap();
        for field in [
            "missing_id",
            "empty_id",
            "missing_generation",
            "empty_generation",
        ] {
            let mut lease = complete.clone();
            match field {
                "missing_id" => lease.vm_id = None,
                "empty_id" => lease.vm_id = Some(String::new()),
                "missing_generation" => lease.metrics = None,
                _ => lease.metrics.as_mut().unwrap().generation.clear(),
            }
            for explicit in [false, true] {
                let (dir, provider) =
                    stub_provider(&json!({"version": 2, "vms": [lease]}).to_string());
                let identity = explicit.then(|| lease.identity());
                let result = close_firecracker_lease(
                    &provider,
                    "work",
                    "review",
                    identity.as_ref(),
                    None,
                    &mut vec![],
                    &mut vec![],
                );
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("identity and generation"),
                    "{field}"
                );
                if explicit {
                    assert!(!dir.path().join("requests").exists());
                } else {
                    assert_eq!(
                        requests(&dir),
                        [json!({"version": 2, "action": "list", "session": null})]
                    );
                }
            }
        }
    }

    #[test]
    fn close_compares_every_confirmed_identity_field_before_closing_panes() {
        let lease: yinz_types::firecracker::VmEntry = serde_json::from_value(json!({
            "session": "work", "name": "review", "state": "running", "vm_id": "/vm/socket",
            "metrics": {"generation": "42:100", "source": "process", "sample_time_us": 0},
            "worktree": "/repo",
        }))
        .unwrap();
        for field in ["session", "name", "vm_id", "generation", "worktree"] {
            let mut expected = lease.identity();
            match field {
                "session" => expected.session = "other".into(),
                "name" => expected.name = "other".into(),
                "vm_id" => expected.vm_id = Some("other".into()),
                "generation" => expected.generation = Some("replacement".into()),
                _ => expected.worktree = Some("/other".into()),
            }
            let (dir, provider) = stub_provider(&json!({"version": 2, "vms": [lease]}).to_string());
            let result = close_firecracker_lease(
                &provider,
                "work",
                "review",
                Some(&expected),
                None,
                &mut vec![],
                &mut vec![],
            );
            assert!(result.is_err(), "{field}");
            if dir.path().join("requests").exists() {
                assert!(requests(&dir)
                    .iter()
                    .all(|request| request["action"] == "list"));
            }
        }
    }

    #[test]
    fn close_passes_the_original_lease_to_guarded_close_and_prints_warnings() {
        let session = format!("absent-close-{}", std::process::id());
        let lease = json!({
            "session": session, "name": "api", "state": "running",
            "vm_id": "/vm/socket", "metrics": {
                "generation": "42:100", "source": "process", "sample_time_us": 0,
            },
            "worktree": "/repo",
        });
        let (dir, provider) = stub_provider(
            &json!({
                "version": 2, "vms": [lease.clone()], "warnings": ["kept host copy"],
            })
            .to_string(),
        );
        let (out, err) = run(
            Firecracker::Close {
                expected_identity: None,
                expected_worktree: None,
                expected_head: None,
                expected_branch: None,
                name: "api".into(),
                session: Some(session),
            },
            &provider,
        );
        assert!(out.contains("Lease released: api"));
        assert_eq!(err, "warning: kept host copy\n");
        let sent = requests(&dir);
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0]["action"], "list");
        assert_eq!(sent[1]["action"], "close_if_matches");
        assert_eq!(sent[1]["expected"]["vm_id"], lease["vm_id"]);
        assert_eq!(
            sent[1]["expected"]["generation"],
            lease["metrics"]["generation"]
        );
        assert_eq!(sent[1]["expected"]["worktree"], lease["worktree"]);
        assert_eq!(sent[2]["action"], "list");
    }

    #[test]
    fn close_dispatches_guarded_worktree_removal_as_one_provider_request() {
        let lease = json!({
            "session": "work", "name": "review", "state": "running",
            "vm_id": "/vm/socket", "metrics": {
                "generation": "42:100", "source": "process", "sample_time_us": 0,
            },
            "worktree": "/repo/topic",
        });
        let expected: yinz_types::firecracker::Identity = serde_json::from_value(json!({
            "session": "work", "name": "review", "vm_id": "/vm/socket",
            "generation": "42:100", "worktree": "/repo/topic",
        }))
        .unwrap();
        let (dir, provider) =
            stub_provider(&json!({"version": 2, "vms": [lease], "warnings": []}).to_string());

        run(
            Firecracker::Close {
                name: "review".into(),
                session: Some("work".into()),
                expected_identity: Some(expected),
                expected_worktree: Some("/repo/topic".into()),
                expected_head: Some("0123456789".into()),
                expected_branch: Some("topic".into()),
            },
            &provider,
        );

        let sent = requests(&dir);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["action"], "close_and_remove_worktree_if_matches");
        assert_eq!(
            sent[0]["checkout"],
            json!({"path": "/repo/topic", "head": "0123456789", "branch": "topic"})
        );
    }

    #[test]
    fn stop_prints_each_merge_warning_to_stderr() {
        let (dir, provider) = stub_provider(
            r#"{"version":2,"warnings":["/repo/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/a.rs.guest","/repo/b.rs: changed on both sides; kept the host copy, the guest copy is /repo/b.rs.guest"]}"#,
        );
        let (out, err) = run(
            Firecracker::Stop {
                name: "review".to_owned(),
                session: Some("work".to_owned()),
            },
            &provider,
        );
        assert_eq!(out, "");
        assert_eq!(
            err,
            "warning: /repo/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/a.rs.guest\n\
             warning: /repo/b.rs: changed on both sides; kept the host copy, the guest copy is /repo/b.rs.guest\n"
        );
        assert_eq!(
            requests(&dir),
            vec![json!({"version": 2, "action": "stop", "session": "work", "name": "review"})]
        );
    }

    #[test]
    fn sync_prints_each_merge_warning_to_stderr() {
        let (dir, provider) = stub_provider(
            r#"{"version":2,"warnings":["/repo/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/a.rs.guest"]}"#,
        );
        let (out, err) = run(
            Firecracker::Sync {
                expected_identity: None,
                name: "review".to_owned(),
                session: Some("work".to_owned()),
            },
            &provider,
        );
        assert_eq!(out, "");
        assert_eq!(
            err,
            "warning: /repo/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/a.rs.guest\n"
        );
        assert_eq!(
            requests(&dir),
            vec![json!({"version": 2, "action": "sync", "session": "work", "name": "review"})]
        );
    }

    #[test]
    fn changes_prints_the_guest_change_count() {
        let (dir, provider) = stub_provider(r#"{"version":2,"changes":3}"#);
        let (out, err) = run(
            Firecracker::Changes {
                expected_identity: None,
                name: "review".to_owned(),
                session: Some("work".to_owned()),
            },
            &provider,
        );
        assert_eq!(out, "3\n");
        assert_eq!(err, "");
        assert_eq!(
            requests(&dir),
            vec![json!({"version": 2, "action": "changes", "session": "work", "name": "review"})]
        );
    }

    #[test]
    fn a_clean_stop_or_sync_prints_nothing() {
        let (_dir, provider) = stub_provider(r#"{"version":2}"#);
        for command in [
            Firecracker::Stop {
                name: "review".to_owned(),
                session: Some("work".to_owned()),
            },
            Firecracker::Sync {
                expected_identity: None,
                name: "review".to_owned(),
                session: Some("work".to_owned()),
            },
        ] {
            assert_eq!(run(command, &provider), (String::new(), String::new()));
        }
    }
}

pub(crate) fn data(command: yinz_cli::args::Data, opts: &CliArgs) -> anyhow::Result<()> {
    use anyhow::Context;
    use yinz_cli::args::Data;
    use yinz_types::cli::DataRequest;
    let request = match command {
        Data::Catalog { namespace } => DataRequest::Catalog { namespace },
        Data::Apply { namespace, file } => DataRequest::Apply {
            namespace,
            mutations: serde_json::from_str(&read_source(Some(&file))?)
                .context("expected a Mutation JSON array")?,
        },
        command @ (Data::Fetch { .. } | Data::Count { .. }) => {
            let count = matches!(command, Data::Count { .. });
            let (Data::Fetch { namespace, file } | Data::Count { namespace, file }) = command
            else {
                unreachable!()
            };
            let mut request: yinz_types::plugin_data::FetchRequest =
                serde_json::from_str(&read_source(Some(&file))?)
                    .context("expected FetchRequest JSON")?;
            if request
                .namespace
                .as_ref()
                .is_some_and(|value| value != &namespace)
            {
                anyhow::bail!("--namespace conflicts with the request namespace");
            }
            request.namespace = Some(namespace);
            if count {
                DataRequest::Count(request)
            } else {
                DataRequest::Fetch(request)
            }
        },
    };
    let session = resolve_session_for_stream(opts.session.clone(), "query data in");
    println!(
        "{}",
        yinz_client::store_client::data_request(&session, request)?
    );
    Ok(())
}
