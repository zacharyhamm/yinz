//! Scheduled prompts as rows in `shared:hocus`, so a hocus reload restores them.
//!
//! hocus is the only writer. The row is what `schedule` was told plus when the next run is due
//! as a wall-clock second: the countdown itself stays in memory, and the row moves only when a
//! job is armed or fires, so the store is not written once a second per job.

use zellij_tile::prelude::{DataRows, DataValue, Mutation, Pred};

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
pub fn to_mutation(job: &JobRow) -> Mutation {
    Mutation::put(ENTITY)
        .uid(job.job_id.to_string())
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

/// The write that forgets the named jobs.
pub fn delete_mutation(job_ids: impl IntoIterator<Item = u64>) -> Mutation {
    Mutation::delete(ENTITY, Pred::is_in("job_id", job_ids))
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
            .apply(vec![to_mutation(&job(1)), to_mutation(&job(2))])
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
        local.apply(vec![to_mutation(&moved)]).unwrap();
        assert_eq!(
            from_rows(&local.fetch(&fetch).unwrap()),
            vec![moved, job(2)]
        );

        local.apply(vec![delete_mutation([1])]).unwrap();
        assert_eq!(from_rows(&local.fetch(&fetch).unwrap()), vec![job(2)]);
    }
}
