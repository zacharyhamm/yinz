//! Scheduled prompts as rows in `shared:hocus`, so a hocus reload restores them.
//!
//! hocus is the only writer. The row is what `schedule` was told plus when the next run is due
//! as a wall-clock second: the countdown itself stays in memory, and the row moves only when a
//! job is armed or fires, so the store is not written once a second per job.

use crate::agents::rows::Owner;
use zellij_tile::prelude::{DataRows, DataValue, FetchRequest, Mutation, Pred};

pub const ENTITY: &str = "job";

pub const ROW_COLUMNS: &[&str] = &[
    "job_id",
    "chat_id",
    "every",
    "next_due_at",
    "prompt",
    "show",
    "auto_confirm",
];

/// One job as the store holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct JobRow {
    pub job_id: u64,
    /// The chat the job was armed in, once that chat has been saved.
    pub chat_id: Option<u64>,
    /// Seconds between runs.
    pub every: f64,
    /// Unix seconds at which the next run is due.
    pub next_due_at: i64,
    pub prompt: String,
    pub show: bool,
    pub auto_confirm: bool,
}

fn int_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<i64> {
    match rows.get(row, column) {
        Some(DataValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// The jobs these rows describe. A row without an id, interval, due time or prompt is skipped
/// rather than guessed at; all four are `required`, so that should not happen.
pub fn from_rows(rows: &DataRows) -> Vec<JobRow> {
    rows.rows
        .iter()
        .filter_map(|row| {
            let job_id = u64::try_from(int_at(rows, row, "job_id")?).ok()?;
            let every = match rows.get(row, "every") {
                Some(DataValue::Real(every)) => *every,
                Some(DataValue::Int(every)) => *every as f64,
                _ => return None,
            };
            let next_due_at = int_at(rows, row, "next_due_at")?;
            let prompt = match rows.get(row, "prompt") {
                Some(DataValue::Text(prompt)) => prompt.clone(),
                _ => return None,
            };
            Some(JobRow {
                job_id,
                chat_id: int_at(rows, row, "chat_id").and_then(|id| u64::try_from(id).ok()),
                every,
                next_due_at,
                prompt,
                show: matches!(rows.get(row, "show"), Some(DataValue::Bool(true))),
                auto_confirm: matches!(rows.get(row, "auto_confirm"), Some(DataValue::Bool(true))),
            })
        })
        .collect()
}

/// The write that stores one job. Every attribute is set, as `projects::to_mutation` does:
/// an upsert only writes what it names.
///
/// The uid names the owner as well as the job id, and has to: `job_id` comes from a counter each
/// hocus keeps in its own memory, so two servers both allocate 1, 2, 3. On a shared store a bare
/// id put their jobs into one row.
pub fn to_mutation(owner: &Owner, job: &JobRow) -> Mutation {
    Mutation::put(ENTITY)
        .uid(job_uid(owner, job.job_id))
        .set("host_id", owner.host_id.clone())
        .set("zellij_session_name", owner.zellij_session_name.clone())
        .set("job_id", job.job_id)
        .set(
            "chat_id",
            job.chat_id.map(DataValue::from).unwrap_or(DataValue::Null),
        )
        .set("every", job.every)
        .set("next_due_at", job.next_due_at)
        .set("prompt", job.prompt.clone())
        .set("show", job.show)
        .set("auto_confirm", job.auto_confirm)
        .build()
}

/// The write that forgets the named jobs. Scoped to this machine, so cancelling job 3 here does
/// not cancel another machine's job 3.
pub fn delete_mutation(owner: &Owner, job_ids: impl IntoIterator<Item = u64>) -> Mutation {
    Mutation::delete(ENTITY, Pred::is_in("job_id", job_ids).and(owned_by(owner)))
}

/// A job's row key: the machine that armed it, and its id there.
///
/// The machine and not the session, deliberately. Jobs are restored once at load, before the
/// first snapshot has named the session, so a session-scoped read could not run at all — and
/// sharing jobs between the sessions on one machine is what has always happened, since the uid
/// used to be the bare id. This keeps that exactly and stops only the new leak, which is one
/// machine restoring another's jobs and firing its prompts here.
fn job_uid(owner: &Owner, job_id: u64) -> String {
    format!("{}:{}", owner.host_id, job_id)
}

/// Only this machine's jobs.
fn owned_by(owner: &Owner) -> Pred {
    Pred::eq("host_id", owner.host_id.clone())
}

/// The jobs this machine armed.
///
/// Scoped, and that is the whole point: an unscoped read on a shared store restores another
/// machine's jobs and fires its scheduled prompts into this session.
///
/// Jobs written before the store could be shared have an empty `host_id` and are deliberately
/// left where they are. They name a server that no longer exists — the `chat_id` on one points
/// into an archive this machine may not have — and reviving them under a new owner would fire
/// somebody's old prompts at whoever upgraded first.
pub fn owned_request(owner: &Owner) -> FetchRequest {
    FetchRequest::entity(ENTITY).filter(owned_by(owner))
}

/// The jobs as the rows a fetch would return; the inverse of [`from_rows`], for tests.
pub fn to_rows(jobs: &[JobRow]) -> DataRows {
    DataRows {
        columns: ROW_COLUMNS.iter().map(|c| (*c).to_owned()).collect(),
        rows: jobs
            .iter()
            .map(|job| {
                vec![
                    DataValue::Int(job.job_id as i64),
                    job.chat_id
                        .map(|id| DataValue::Int(id as i64))
                        .unwrap_or(DataValue::Null),
                    DataValue::Real(job.every),
                    DataValue::Int(job.next_due_at),
                    DataValue::Text(job.prompt.clone()),
                    DataValue::Bool(job.show),
                    DataValue::Bool(job.auto_confirm),
                ]
            })
            .collect(),
        next_cursor: None,
        truncated: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{self, Backend};
    use zellij_tile::prelude::{FetchRequest, SortDir};

    /// The machine these jobs were armed on.
    fn owner() -> Owner {
        Owner::new(zellij_tile::prelude::HostId::new("host-a"), "zellij".into())
    }

    fn job(id: u64) -> JobRow {
        JobRow {
            job_id: id,
            chat_id: id.is_multiple_of(2).then_some(40 + id),
            every: 60.0,
            next_due_at: 1_700_000_000 + id as i64,
            prompt: format!("check {id}"),
            show: id == 1,
            auto_confirm: id == 2,
        }
    }

    /// Two machines sharing one Postgres, each with its own job counter starting at 1.
    ///
    /// The counter lives in each hocus's memory, so both mint job 1. With the bare id as the uid
    /// they were one row; with an unscoped restore each machine took over the other's jobs and
    /// fired its prompts.
    #[test]
    fn jobs_belong_to_the_machine_that_armed_them() {
        let laptop = store::Local::shared("hocus");
        laptop.define_all(&store::shared_defs()).unwrap();
        let build_box = laptop.joined(store::SHARED, "hocus");

        let on_laptop = Owner::new(
            zellij_tile::prelude::HostId::new("host-laptop"),
            "main".into(),
        );
        let on_build_box = Owner::new(
            zellij_tile::prelude::HostId::new("host-build-box"),
            "main".into(),
        );

        let mut theirs = job(1);
        theirs.prompt = "run the build".to_owned();
        laptop
            .apply(vec![to_mutation(&on_laptop, &job(1))])
            .unwrap();
        build_box
            .apply(vec![to_mutation(&on_build_box, &theirs)])
            .unwrap();

        let restored = |backend: &dyn Backend, owner: &Owner| {
            from_rows(&backend.fetch(&owned_request(owner)).unwrap())
        };
        assert_eq!(restored(&laptop, &on_laptop), vec![job(1)]);
        assert_eq!(
            restored(&build_box, &on_build_box),
            vec![theirs.clone()],
            "one machine's job 1 overwrote another's"
        );

        // And cancelling job 1 here leaves job 1 there alone.
        laptop
            .apply(vec![delete_mutation(&on_laptop, [1])])
            .unwrap();
        assert!(restored(&laptop, &on_laptop).is_empty());
        assert_eq!(restored(&build_box, &on_build_box), vec![theirs]);
    }

    /// A job armed before the store could be shared has no owner, and is left where it is rather
    /// than adopted: it names a server that is gone, and its `chat_id` points into an archive the
    /// machine reading it may not have.
    #[test]
    fn a_job_with_no_owner_is_not_restored() {
        let local = store::Local::shared("hocus");
        local.define_all(&store::shared_defs()).unwrap();
        // The old shape: a bare job id for a uid, and no owner columns.
        local
            .apply(vec![zellij_tile::prelude::Mutation::put(ENTITY)
                .uid("1")
                .set("job_id", 1_u64)
                .set("every", 60.0)
                .set("next_due_at", 1_700_000_000_i64)
                .set("prompt", "an old prompt")
                .build()])
            .unwrap();
        let owner = Owner::new(zellij_tile::prelude::HostId::new("host-a"), "main".into());
        assert!(from_rows(&local.fetch(&owned_request(&owner)).unwrap()).is_empty());
    }

    #[test]
    fn a_job_round_trips_through_its_rows() {
        let jobs = vec![job(1), job(2)];
        assert_eq!(from_rows(&to_rows(&jobs)), jobs);
    }

    #[test]
    fn a_job_round_trips_through_the_store_and_a_delete_forgets_it() {
        let local = store::Local::shared("hocus");
        local.define_all(&store::shared_defs()).unwrap();
        local
            .apply(vec![
                to_mutation(&owner(), &job(1)),
                to_mutation(&owner(), &job(2)),
            ])
            .unwrap();
        let fetch = FetchRequest::entity(ENTITY).sort_by("job_id", SortDir::Asc);
        assert_eq!(
            from_rows(&local.fetch(&fetch).unwrap()),
            vec![job(1), job(2)]
        );

        // A second put with the same id is the same row, moved.
        let mut moved = job(1);
        moved.next_due_at += 60;
        moved.chat_id = None;
        local.apply(vec![to_mutation(&owner(), &moved)]).unwrap();
        assert_eq!(
            from_rows(&local.fetch(&fetch).unwrap()),
            vec![moved, job(2)]
        );

        local.apply(vec![delete_mutation(&owner(), [1])]).unwrap();
        assert_eq!(from_rows(&local.fetch(&fetch).unwrap()), vec![job(2)]);
    }
}
