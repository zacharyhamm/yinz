use super::*;
use agent_proto::{projects, run_history, store};
use store::Backend;
use yinz_types::plugin_data::{FetchRequest, Pred, SortDir};

impl State {
    fn store(&self) -> store::Host {
        store::Host::shared()
    }
    pub(super) fn subscribe_to_service(&self) {
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
    }
    pub(super) fn discover_service(&mut self, sessions: &[SessionInfo]) {
        let ids: Vec<_> = sessions
            .iter()
            .filter(|s| s.is_current_session)
            .flat_map(|s| s.plugins.iter())
            .filter(|(_, p)| p.location == "yinz:agent-service")
            .map(|(id, _)| *id)
            .collect();
        let id = if ids.len() == 1 { Some(ids[0]) } else { None };
        if self.service_id != id {
            self.service_id = id;
            self.service_seq = 0;
            self.subscribe_to_service();
        }
    }
    pub(super) fn receive_snapshot(&mut self, message: &PipeMessage) -> bool {
        if self.service_id.is_none() {
            if let Ok(sessions) = get_session_list() {
                self.discover_service(&sessions.live_sessions);
            }
        }
        if self.service_id.is_none()
            || message.source != PipeSource::Plugin(self.service_id.unwrap_or_default())
        {
            return false;
        }
        let seq = message
            .args
            .get("seq")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        if seq <= self.service_seq {
            return false;
        }
        let Some(snapshot) = message
            .payload
            .as_deref()
            .and_then(|s| serde_json::from_str::<wire::PollReply>(s).ok())
        else {
            return false;
        };
        self.service_seq = seq;
        for run in &snapshot.runs {
            if run.ended_at_ms.is_none() {
                self.archived_runs.retain(|old| old.id != run.id);
            } else if let Some(existing) =
                self.archived_runs.iter_mut().find(|old| old.id == run.id)
            {
                *existing = run.clone();
            } else {
                if self
                    .archive_loaded_through
                    .as_ref()
                    .is_some_and(|(time, id)| {
                        (std::cmp::Reverse(run.started_at_ms), run.id.as_str())
                            < (std::cmp::Reverse(*time), id.as_str())
                    })
                {
                    self.archive_offset = self.archive_offset.saturating_add(1);
                }
                self.archived_runs.push(run.clone());
            }
        }
        self.archived_runs.sort_by(|a, b| {
            b.started_at_ms
                .cmp(&a.started_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        self.snapshot = Some(snapshot);
        let first = !std::mem::replace(&mut self.strip_seen, true);
        self.refresh_strip() || first
    }
    pub(super) fn refresh_strip(&mut self) -> bool {
        let Some(snapshot) = &self.snapshot else {
            return false;
        };
        let panes = snapshot.panes.as_deref().unwrap_or_default();
        let registry = self
            .store()
            .fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))
            .map(|rows| projects::from_rows(&rows))
            .unwrap_or_default();
        let projects = panes
            .iter()
            .filter_map(|p| {
                let location = p.checkout_location()?;
                let project =
                    projects::project_for_cwd(&snapshot.repositories, &registry, &location)?;
                Some((p.pane_id, project.name.clone()))
            })
            .collect();
        let branches = panes
            .iter()
            .filter_map(|p| {
                Some((
                    p.pane_id,
                    snapshot
                        .repositories
                        .get(&p.checkout_location()?)?
                        .branch
                        .clone()?,
                ))
            })
            .collect();
        let mut rows = agents::strip_rows(
            panes,
            &snapshot.manual_kinds,
            &snapshot.activity,
            &projects,
            &branches,
            &snapshot.summaries,
            &snapshot.hooks,
        );
        for row in &mut rows {
            if let StripRow::Agent { pane_id, text, .. } = row {
                if snapshot.runs.iter().any(|run| {
                    run.pane_id == *pane_id
                        && run.ended_at_ms.is_none()
                        && run.previous_run_id.is_some()
                }) {
                    text.insert_str(text.len() - text.trim_start().len(), "↳ ");
                }
            }
        }
        if !self.archived_runs.is_empty() || self.archive_more {
            rows.push(StripRow::Project("archive".into()));
            rows.extend(self.archived_runs.iter().map(|run| StripRow::Run {
                run_id: run.id.clone(),
                kind: run.kind.clone(),
                title: run.title.clone(),
                worktree: agents::worktree_cell(run),
                started_at_ms: Some(run.started_at_ms),
                continuation: run.previous_run_id.is_some(),
            }));
            if self.archive_more {
                rows.push(StripRow::MoreRuns);
            }
        }
        let strip = Strip {
            rows,
            focused: snapshot.focused,
            summaries: snapshot.summaries.clone(),
        };
        let selected = self.selection_key();
        let changed = self.set_rows(strip);
        self.follow_selection(selected);
        let revision = self.run_revision;
        let error = self.run_error.clone();
        self.load_selected_history();
        let answered = self.summary_in_the_strip();
        changed || answered || revision != self.run_revision || error != self.run_error
    }
    pub(super) fn load_selected_history(&mut self) {
        let run_id = self.selected_run_id().or_else(|| {
            self.selection().and_then(|pane| {
                self.snapshot
                    .as_ref()?
                    .runs
                    .iter()
                    .find(|r| r.pane_id == pane && r.ended_at_ms.is_none())
                    .map(|r| r.id.clone())
            })
        });
        let Some(run_id) = run_id else {
            return;
        };
        let result = (|| {
            let rows = self
                .store()
                .fetch(&agents::rows::run_by_id_request(&run_id))?;
            let mut run = agents::rows::runs_from_rows(&rows)
                .into_iter()
                .next()
                .ok_or("the run is no longer in the archive")?;
            if let Some(revision) = self.selected_revision() {
                let page = run_history::search_detail(&self.store(), &run_id, revision)?;
                run.omitted_entries = usize::from(page.earlier);
                run.entries = page.entries;
            } else {
                agents::rows::hydrate_newest_entries(&self.store(), &mut run);
            }
            serde_json::to_string(&run).map_err(|e| e.to_string())
        })();
        match result {
            Ok(payload) => {
                let fingerprint = agents::run_payload_fingerprint(&payload);
                if self
                    .run
                    .as_ref()
                    .is_none_or(|r| r.fingerprint != fingerprint)
                {
                    if let Ok(run) = serde_json::from_str(&payload) {
                        self.load_run(run, fingerprint);
                    }
                }
            },
            Err(error) => {
                self.run_error = self.selection_key().map(|key| (key, error));
            },
        }
    }
    #[cfg(not(test))]
    pub(super) fn agent_predecessors(
        &self,
        request: wire::PredecessorsRequest,
    ) -> wire::PredecessorsResult {
        let anchor = self
            .snapshot
            .as_ref()
            .and_then(|s| {
                s.runs
                    .iter()
                    .find(|r| r.pane_id == request.pane_id && r.ended_at_ms.is_none())
            })
            .cloned();
        let (runs, diagnostic) = anchor
            .as_ref()
            .map(|a| run_history::predecessors(a, |id| run_history::run_header(&self.store(), id)))
            .unwrap_or_else(|| (vec![], "No recorded active run for this agent".into()));
        wire::PredecessorsResult {
            request_id: request.request_id,
            pane_id: request.pane_id,
            anchor,
            runs,
            diagnostic,
        }
    }
    #[cfg(not(test))]
    pub(super) fn agent_search_result(
        &self,
        request: wire::SearchRequest,
        _: bool,
    ) -> wire::SearchResult {
        let mut result = wire::SearchResult {
            request_id: request.request_id,
            ..Default::default()
        };
        if let Err(error) = self.search_agent_runs(&request.query, &mut result) {
            result.error = error;
        }
        result
    }
    #[cfg(not(test))]
    pub(super) fn resume_run(&mut self, run_id: &str) {
        let result = self
            .stored_run_header(run_id)
            .and_then(|run| {
                if run.ended_at_ms.is_none() {
                    return Err("that run is live again".into());
                }
                launch::resume_command(
                    &run.kind,
                    &run.session_id,
                    run.cwd.as_deref(),
                    run.host.as_deref(),
                    run.bubblewrap.as_deref(),
                )
            })
            .and_then(|command| {
                open_command_pane_near_plugin(command, launch::agent_pane_context())
                    .ok_or_else(|| "the pane could not be opened".to_owned())
            });
        match result {
            Ok(pane) => {
                self.hide_the_panel();
                focus_pane_with_id(pane, false, false);
                self.resume_note = None;
            },
            Err(error) => self.resume_note = Some((format!("run:{run_id}"), error)),
        }
    }
    pub(crate) fn load_more_agent_runs(&mut self) {
        const PAGE: u32 = 50;
        let Ok(rows) = self.store().fetch(
            &FetchRequest::entity(agents::rows::RUN_ENTITY)
                .filter(Pred::eq("active", false))
                .filter(Pred::not_null("header"))
                .sort_by("started_at_ms", SortDir::Desc)
                .sort_by("uid", SortDir::Asc)
                .select(["header"])
                .limit(PAGE)
                .offset(self.archive_offset),
        ) else {
            return;
        };
        let count = rows.len() as u32;
        let headers = agents::rows::run_headers_from_rows(&rows);
        if let Some(last) = headers.last() {
            self.archive_loaded_through = Some((last.started_at_ms, last.id.clone()));
        }
        for header in headers {
            if !self.archived_runs.iter().any(|run| run.id == header.id) {
                self.archived_runs.push(header);
            }
        }
        self.archive_offset = self.archive_offset.saturating_add(count);
        self.archive_more = rows.truncated;
    }

    #[cfg(not(test))]
    pub(crate) fn stored_run_header(&self, run_id: &str) -> Result<agents::AgentRunHeader, String> {
        let rows = self
            .store()
            .fetch(&agents::rows::run_by_id_request(run_id))
            .map_err(|_| "that run's record could not be read".to_owned())?;
        agents::rows::runs_from_rows(&rows)
            .into_iter()
            .next()
            .map(|run| run.header())
            .ok_or_else(|| "that run is no longer in the archive".to_owned())
    }

    #[cfg(not(test))]
    fn search_agent_runs(
        &self,
        query: &str,
        result: &mut wire::SearchResult,
    ) -> Result<(), String> {
        let found = run_history::search_runs(&self.store(), query)?;
        result.truncated = found.truncated;
        for hit in found.hits {
            let Some(header) = found.runs.get(&hit.run_id) else {
                continue;
            };
            result.runs.push(wire::SearchRun {
                run_id: hit.run_id,
                kind: header.kind.clone(),
                title: header.title.clone(),
                revision: hit.revision,
                snippet: hit.snippet,
                started_at_ms: header.started_at_ms,
                worktree: agents::worktree_cell(header),
            });
        }
        Ok(())
    }
}
