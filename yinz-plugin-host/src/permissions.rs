//! One permission FIFO for the parent and all of its workers.
use crate::plugin_map::{PluginEnv, CANCELLED_BY_HOST};
use crate::yinz_exports::run_plugin_command;
use crate::{background_worker, ClientId, PluginInstruction};
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::sync::{atomic::Ordering, Arc};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::PermissionType;
use yinz_types::data::{PermissionStatus, PluginCommand};

const MAX_REQUESTS: usize = 1_024;
const MAX_COMPLETION_ID_BYTES: usize = 128;

struct Reservation(Arc<AtomicUsize>);

impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub(crate) struct Request {
    pub generation: u64,
    pub client: ClientId,
    pub permissions: Vec<PermissionType>,
    pub id: String,
    // Shared by dispatch, the active request and its completion, including worker inboxes.
    reservation: Option<Arc<Reservation>>,
}

impl Request {
    pub fn new(
        generation: u64,
        client: ClientId,
        permissions: Vec<PermissionType>,
        id: String,
    ) -> Self {
        Self {
            generation,
            client,
            permissions,
            id,
            reservation: None,
        }
    }
}

pub(crate) struct Completion {
    pub request: Request,
    pub result: std::result::Result<PermissionStatus, String>,
}

#[derive(Default)]
pub(crate) struct Permissions {
    queue: VecDeque<Request>,
    active: Option<Request>,
    granted: Vec<PermissionType>,
    answer: Option<PermissionStatus>,
    completions: VecDeque<Completion>,
    outstanding: Arc<AtomicUsize>,
    stopped: bool,
}

impl Permissions {
    pub fn requested(&self) -> Option<Vec<PermissionType>> {
        self.active
            .as_ref()
            .map(|request| request.permissions.clone())
    }

    pub fn enqueue(&mut self, mut request: Request) -> Result<()> {
        if self.stopped {
            bail!(CANCELLED_BY_HOST);
        }
        if request.id.len() > MAX_COMPLETION_ID_BYTES {
            bail!("Plugin permission completion ID exceeds 128 bytes");
        }
        self.outstanding
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                (used < MAX_REQUESTS).then_some(used + 1)
            })
            .map_err(|_| {
                anyhow!("Plugin permission requests exceed the 1024 outstanding request limit")
            })?;
        request.reservation = Some(Arc::new(Reservation(self.outstanding.clone())));
        // Do not retain the input allocation: duplicate lists can have arbitrarily large capacity.
        let mut normalized = Vec::new();
        for permission in request.permissions {
            if !normalized.contains(&permission) {
                normalized.push(permission);
            }
        }
        request.permissions = normalized.into_boxed_slice().into_vec();
        request.id = request.id.into_boxed_str().into_string();
        self.queue.push_back(request);
        Ok(())
    }

    pub fn next(&mut self) -> Option<Request> {
        if self.stopped || self.active.is_some() {
            return None;
        }
        let mut request = self.queue.pop_front()?;
        let mut permissions = self.granted.clone();
        for permission in request.permissions {
            if !permissions.contains(&permission) {
                permissions.push(permission);
            }
        }
        request.permissions = permissions;
        self.active = Some(request.clone());
        Some(request)
    }

    // The host answer is recorded before its ordinary event is broadcast/replayed. Observers
    // cannot settle a request, and a repeated broadcast cannot answer the next FIFO entry.
    pub fn answered(&mut self, permissions: &[PermissionType], status: PermissionStatus) -> bool {
        if self.stopped || self.answer.is_some() {
            return false;
        }
        if self.active.as_ref().is_some_and(|request| {
            request.permissions.len() == permissions.len()
                && request.permissions.iter().all(|p| permissions.contains(p))
        }) {
            self.answer = Some(status);
            true
        } else {
            false
        }
    }

    pub fn finish(
        &mut self,
        result: std::result::Result<PermissionStatus, String>,
    ) -> Option<Completion> {
        let request = self.active.take()?;
        if result == Ok(PermissionStatus::Granted) {
            self.granted = request.permissions.clone();
        }
        Some(Completion { request, result })
    }

    pub fn take_answer(&mut self) -> Option<Completion> {
        self.answer
            .take()
            .and_then(|status| self.finish(Ok(status)))
    }

    pub fn forget(&mut self, generation: u64) {
        self.queue
            .retain(|request| request.generation != generation);
        self.completions
            .retain(|completion| completion.request.generation != generation);
        // An already dispatched request still owns the next host answer, even if its
        // worker stopped. Keep it until answered so that no sibling consumes its reply.
    }

    pub fn stop(&mut self) {
        *self = Self {
            stopped: true,
            outstanding: self.outstanding.clone(),
            ..Self::default()
        };
    }
}

impl background_worker::Workers {
    pub(crate) fn forget_permission_requests(&self, generation: u64) {
        self.permissions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .forget(generation);
    }

    pub fn request_permissions(&self, env: &mut PluginEnv, request: Request) -> Result<()> {
        let immediate = (request.generation, request.id.clone());
        self.permissions.lock().to_anyhow()?.enqueue(request)?;
        self.dispatch_permissions(env, Some(immediate))
    }

    fn dispatch_permissions(
        &self,
        env: &mut PluginEnv,
        immediate: Option<(u64, String)>,
    ) -> Result<()> {
        let mut failure = None;
        loop {
            let request = self.permissions.lock().to_anyhow()?.next();
            let Some(request) = request else { break };
            // Never hold the coordinator lock across the shared command dispatcher.
            let previous = std::mem::replace(&mut env.client_id, request.client);
            let result = run_plugin_command(
                env,
                PluginCommand::RequestPluginPermissions(request.permissions.clone()),
            );
            env.client_id = previous;
            if let Err(error) = result {
                let completion = self
                    .permissions
                    .lock()
                    .to_anyhow()?
                    .finish(Err(format!("{error:#}")));
                if immediate.as_ref() == Some(&(request.generation, request.id)) {
                    failure = Some(error);
                } else if let Some(completion) = completion {
                    self.complete_permission(env, completion);
                }
            } else {
                break;
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub fn permission_answer(
        &self,
        env: &PluginEnv,
        permissions: &[PermissionType],
        status: PermissionStatus,
    ) {
        let accepted = self
            .permissions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .answered(permissions, status);
        if accepted {
            self.wake_permissions(env);
        }
    }

    fn wake_permissions(&self, env: &PluginEnv) {
        env.senders
            .send_to_plugin(PluginInstruction::PluginTick(
                env.plugin_id,
                env.owner_client_id,
                self.parent_generation,
            ))
            .context("waking Plugin permission coordinator")
            .non_fatal();
    }

    fn complete_permission(&self, env: &PluginEnv, completion: Completion) {
        if completion.request.generation == self.parent_generation {
            self.permissions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .completions
                .push_back(completion);
            self.wake_permissions(env);
        } else {
            self.send(
                completion.request.generation,
                background_worker::Message::Permission(completion),
            );
        }
    }

    pub fn take_permission_completions(&self, env: &mut PluginEnv) -> Result<Vec<Completion>> {
        let completion = { self.permissions.lock().to_anyhow()?.take_answer() };
        if let Some(completion) = completion {
            self.complete_permission(env, completion);
            self.dispatch_permissions(env, None)?;
        }
        Ok(self
            .permissions
            .lock()
            .to_anyhow()?
            .completions
            .drain(..)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use yinz_types::data::Event;

    fn request(generation: u64, id: usize) -> Request {
        Request::new(
            generation,
            ClientId::new(1),
            vec![PermissionType::RunCommands],
            id.to_string(),
        )
    }

    #[test]
    fn enqueue_compacts_duplicates_and_bounds_completion_ids_in_bytes() {
        let mut permissions = Permissions::default();
        let mut input = request(1, 0);
        input.permissions = vec![PermissionType::RunCommands; 100_000];
        input.permissions.push(PermissionType::ReadFiles);
        input.id = String::with_capacity(100_000);
        input.id.push_str(&"é".repeat(64));
        permissions.enqueue(input).unwrap();
        let queued = permissions.queue.front().unwrap();
        assert_eq!(
            queued.permissions,
            vec![PermissionType::RunCommands, PermissionType::ReadFiles]
        );
        assert_eq!(queued.permissions.capacity(), 2);
        assert_eq!(queued.id.capacity(), MAX_COMPLETION_ID_BYTES);
        let mut oversized = request(1, 1);
        oversized.id = "é".repeat(65);
        assert!(permissions
            .enqueue(oversized)
            .unwrap_err()
            .to_string()
            .contains("128 bytes"));
        assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn request_limit_spans_dispatch_and_completion_delivery_and_preserves_fifo_grants() {
        let mut permissions = Permissions::default();
        for id in 0..MAX_REQUESTS {
            let mut input = request(1, id);
            if id > 0 {
                input.permissions = vec![PermissionType::ReadFiles];
            }
            permissions.enqueue(input).unwrap();
        }
        assert!(permissions.enqueue(request(1, MAX_REQUESTS)).is_err());
        let dispatch = permissions.next().unwrap();
        assert!(permissions.enqueue(request(1, MAX_REQUESTS)).is_err());
        assert!(permissions.answered(&dispatch.permissions, PermissionStatus::Granted));
        let completion = permissions.take_answer().unwrap();
        assert!(permissions.enqueue(request(1, MAX_REQUESTS)).is_err());
        // Delivery can outlive both the active slot and the dispatch clone.
        drop(dispatch);
        assert!(permissions.enqueue(request(1, MAX_REQUESTS)).is_err());
        drop(completion);
        permissions.enqueue(request(1, MAX_REQUESTS)).unwrap();
        for id in 1..=MAX_REQUESTS {
            let dispatch = permissions.next().unwrap();
            assert_eq!(dispatch.id, id.to_string());
            assert_eq!(
                dispatch.permissions,
                vec![PermissionType::RunCommands, PermissionType::ReadFiles]
            );
            drop(permissions.finish(Ok(PermissionStatus::Granted)));
        }
        assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn reservations_drop_on_dispatch_failure_cancelled_queues_and_shutdown() {
        let mut permissions = Permissions::default();
        permissions.enqueue(request(1, 0)).unwrap();
        drop(permissions.next());
        let failure = permissions.finish(Err("dispatch failed".into())).unwrap();
        assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 1);
        drop(failure);
        assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 0);
        permissions.enqueue(request(1, 1)).unwrap();
        permissions.enqueue(request(1, 2)).unwrap();
        drop(permissions.next());
        let completion = permissions.finish(Ok(PermissionStatus::Granted)).unwrap();
        permissions.completions.push_back(completion);
        permissions.forget(1);
        assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 0);

        permissions.enqueue(request(2, 3)).unwrap();
        permissions.enqueue(request(2, 4)).unwrap();
        let dispatch = permissions.next().unwrap();
        permissions.forget(2);
        assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 1);
        permissions.stop();
        assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 1);
        drop(dispatch);
        assert_eq!(permissions.outstanding.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn dispatcher_failures_release_immediate_and_deferred_requests() {
        let workers = background_worker::Workers::new(1);
        let mut env = crate::test_env::test_env();
        env.senders = Default::default(); // No host receiver: both dispatch branches fail.
        assert!(workers
            .request_permissions(&mut env, request(1, 0))
            .is_err());
        assert_eq!(
            workers
                .permissions
                .lock()
                .unwrap()
                .outstanding
                .load(Ordering::Relaxed),
            0
        );
        workers
            .permissions
            .lock()
            .unwrap()
            .enqueue(request(1, 1))
            .unwrap();
        workers.dispatch_permissions(&mut env, None).unwrap();
        let completions = workers.take_permission_completions(&mut env).unwrap();
        assert_eq!(completions.len(), 1);
        assert!(completions[0].result.is_err());
        assert_eq!(
            workers
                .permissions
                .lock()
                .unwrap()
                .outstanding
                .load(Ordering::Relaxed),
            1
        );
        drop(completions);
        assert_eq!(
            workers
                .permissions
                .lock()
                .unwrap()
                .outstanding
                .load(Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn undeliverable_worker_completions_release_the_reservation() {
        let workers = background_worker::Workers::new(1);
        let outstanding = {
            let mut permissions = workers.permissions.lock().unwrap();
            permissions.enqueue(request(2, 0)).unwrap();
            drop(permissions.next());
            permissions.outstanding.clone()
        };
        let completion = workers
            .permissions
            .lock()
            .unwrap()
            .finish(Ok(PermissionStatus::Granted))
            .unwrap();
        workers.complete_permission(&crate::test_env::test_env(), completion);
        assert_eq!(outstanding.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn stopped_workers_keep_their_inflight_answer_and_disposal_clears_the_fifo() {
        let request = |generation| Request {
            generation,
            client: ClientId::new(1),
            permissions: vec![PermissionType::RunCommands],
            id: "1".into(),
            reservation: None,
        };
        let mut permissions = Permissions::default();
        permissions.enqueue(request(2)).unwrap();
        permissions.enqueue(request(2)).unwrap();
        permissions.enqueue(request(3)).unwrap();
        assert_eq!(permissions.next().unwrap().generation, 2);
        permissions.forget(2);
        assert!(permissions.next().is_none());
        assert!(permissions.answered(&[PermissionType::RunCommands], PermissionStatus::Granted));
        assert!(!permissions.answered(&[PermissionType::RunCommands], PermissionStatus::Granted));
        assert_eq!(permissions.take_answer().unwrap().request.generation, 2);
        assert_eq!(permissions.next().unwrap().generation, 3);
        permissions.stop();
        assert!(!permissions.answered(&[PermissionType::RunCommands], PermissionStatus::Granted));
        assert!(permissions.finish(Err("dispatch stopped".into())).is_none());
        assert!(permissions.take_answer().is_none());
        assert!(permissions.next().is_none());
        assert!(permissions.enqueue(request(4)).is_err());
    }

    #[test]
    fn cancellation_leaves_answered_permissions_queued_for_ticks_and_events() {
        let cancel_client = Arc::new(AtomicBool::new(false));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("permissions.scm");
        std::fs::write(&path, "(void)").unwrap();
        let mut plugin = crate::steel::SteelPlugin::new(
            "(void)",
            &path,
            crate::test_env::test_env(),
            cancel_client.clone(),
        )
        .unwrap();
        let workers = plugin.env().workers.clone().unwrap();
        let first = Request {
            generation: plugin.generation(),
            client: ClientId::new(1),
            permissions: vec![PermissionType::RunCommands],
            id: "first".into(),
            reservation: None,
        };
        let second = Request {
            id: "second".into(),
            ..first.clone()
        };
        {
            let mut permissions = workers.permissions.lock().unwrap();
            permissions.enqueue(first).unwrap();
            permissions.enqueue(second).unwrap();
            let active = permissions.next().unwrap();
            assert!(permissions.answered(&active.permissions, PermissionStatus::Granted));
        }

        plugin.env().cancel.store(true, Ordering::Release);
        for result in [
            plugin.tick().map(|_| ()),
            plugin
                .update(&Event::PermissionRequestResult(PermissionStatus::Granted))
                .map(|_| ()),
        ] {
            assert!(result.unwrap_err().to_string().contains(CANCELLED_BY_HOST));
        }
        assert!(!plugin.env().cancel_tripped.load(Ordering::Acquire));
        let permissions = workers.permissions.lock().unwrap();
        assert_eq!(
            permissions
                .active
                .as_ref()
                .map(|request| request.id.as_str()),
            Some("first")
        );
        assert_eq!(
            permissions.queue.front().map(|request| request.id.as_str()),
            Some("second")
        );
        assert_eq!(permissions.answer, Some(PermissionStatus::Granted));
        drop(permissions);

        plugin.env().cancel.store(false, Ordering::Release);
        cancel_client.store(true, Ordering::Release);
        assert!(!plugin.tick().unwrap());
        assert!(!plugin
            .update(&Event::PermissionRequestResult(PermissionStatus::Granted))
            .unwrap());
    }
}
