use yinz_cli::{
    session_picker::{Destination, Outcome, PickerRow},
    sessions::SessionListing,
};
use yinz_runtime::errors::prelude::*;
use yinz_runtime::session_recovery::Candidate;

pub(crate) fn choose(candidates: &[Candidate]) -> Result<Outcome> {
    let mut rows: Vec<_> = candidates
        .iter()
        .map(|candidate| PickerRow {
            label: candidate.name.clone(),
            destination: Some(Destination::Local(candidate.name.clone().into())),
            status: Some("interrupted".into()),
            age: candidate.age,
            listing: SessionListing::Exited,
            current: false,
        })
        .collect();
    rows.push(PickerRow {
        label: "Start fresh".into(),
        destination: Some(Destination::StartFresh),
        status: None,
        age: Default::default(),
        listing: SessionListing::Running,
        current: false,
    });
    crate::session_menu::run(rows, "recover", Vec::new(), Default::default())
        .context("choose session recovery")
}

pub(crate) fn local() -> Result<Option<String>> {
    use yinz_session::sessions::{dismiss_recovery, recovery_candidates};
    let candidates = recovery_candidates()?;
    if candidates.is_empty() {
        return Ok(None);
    }
    match choose(&candidates)? {
        Outcome::Picked(Destination::Local(name)) => Ok(Some(name.into_string())),
        Outcome::Picked(Destination::StartFresh) => {
            dismiss_recovery(&candidates)?;
            Ok(None)
        },
        Outcome::Interrupted => crate::remote_commands::exit(yinz_client::INTERRUPTED_EXIT_STATUS),
        _ => crate::remote_commands::exit(0),
    }
}
