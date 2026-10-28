use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use yinz_bus::command_palette::{Delivery, Instruction, Owner};
use yinz_bus::plugins::PluginInstruction;
use yinz_bus::screen::ScreenInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::command_palette::{self as wire, Command, Message, Reply, Scope, View};
use yinz_types::internal_ids::ClientId;

use super::{CommandPaletteState, Screen};
use crate::ui::command_palette::{
    plain_text, Entry, EntryKind, Field, FieldValue, Form, Outcome, PluginSection,
};

type ProviderKey = (Owner, String);

pub(crate) struct Provider {
    id: String,
    title: String,
    static_actions: bool,
    unavailable: Arc<AtomicBool>,
}

#[derive(Default)]
pub(crate) struct Registry {
    providers: BTreeMap<ProviderKey, Provider>,
    next_opening: u64,
    closed: HashMap<(Owner, ClientId, Scope), String>,
}

#[derive(Default)]
struct Results {
    actions: Vec<wire::Action>,
    status: Option<String>,
}

struct Nested {
    provider: ProviderKey,
    display: View,
}

struct Execution {
    handle: String,
    values: Option<BTreeMap<String, String>>,
    label: String,
}

struct Pending {
    provider: ProviderKey,
    scope: Scope,
    active: Arc<AtomicBool>,
    execution: bool,
    label: String,
}

#[derive(Default)]
pub(super) struct State {
    opening: u64,
    revision: u64,
    next_view: u64,
    results: BTreeMap<ProviderKey, Results>,
    views: BTreeMap<u64, Nested>,
    pending: Vec<Pending>,
}

fn entry_id(provider: &ProviderKey, id: &str) -> String {
    // Length-prefixed registration avoids ambiguity even when IDs contain punctuation.
    format!(
        "plugin:{}:{}:{}:{}",
        provider.0.generation,
        provider.1.len(),
        provider.1,
        id
    )
}

fn entries(provider: &ProviderKey, actions: &[wire::Action]) -> Vec<Entry> {
    actions
        .iter()
        .map(|action| Entry {
            id: entry_id(provider, &action.id),
            label: action.label.clone(),
            description: action.description.clone(),
            shortcut: String::new(),
            aliases: action.aliases.clone(),
            kind: EntryKind::Plugin,
        })
        .collect()
}

fn deliver(
    senders: &yinz_bus::thread_bus::ThreadSenders,
    owner: &Owner,
    client: ClientId,
    message: Message,
    active: Option<Arc<AtomicBool>>,
) {
    if matches!(&message, Message::Release { handles } if handles.is_empty()) {
        return;
    }
    senders
        .send_to_plugin(PluginInstruction::PluginPalette(Delivery {
            owner: owner.clone(),
            client,
            message,
            active,
        }))
        .context("delivering private palette message")
        .non_fatal();
}

impl CommandPaletteState {
    pub(super) fn cancel_requests(
        &mut self,
        senders: &yinz_bus::thread_bus::ThreadSenders,
        client: ClientId,
    ) {
        self.plugins.revision += 1;
        for pending in self.plugins.pending.drain(..) {
            pending.active.store(false, Ordering::Release);
            deliver(
                senders,
                &pending.provider.0,
                client,
                Message::Cancel {
                    scope: pending.scope,
                },
                None,
            );
        }
        // A query can be cancelled before its debounce has created any requests.
        for results in self.plugins.results.values_mut() {
            if results.status.as_deref() == Some("Loading…") {
                results.status = None;
            }
        }
        self.widget.busy = false;
        self.widget.pending = false;
    }

    fn release(
        &self,
        senders: &yinz_bus::thread_bus::ThreadSenders,
        client: ClientId,
        provider: &ProviderKey,
        reply: &Reply,
    ) {
        let handles = reply.handles();
        if !handles.is_empty() {
            deliver(
                senders,
                &provider.0,
                client,
                Message::Release { handles },
                None,
            );
        }
    }

    fn sections(&mut self, registry: &Registry) {
        if self.agents.owns_view(self.widget.view) {
            return;
        }
        if let Some(nested) = self.plugins.views.get(&self.widget.view) {
            if let View::Actions {
                title,
                actions,
                source,
                ..
            } = &nested.display
            {
                self.widget.set_sections(vec![PluginSection {
                    title: title.clone(),
                    entries: entries(&nested.provider, actions),
                    semantic: source.is_some(),
                    status: if self.widget.pending {
                        Some("Loading…".into())
                    } else {
                        actions.is_empty().then(|| "No actions".into())
                    },
                }]);
            }
            return;
        }
        let mut sections: Vec<_> = self
            .plugins
            .results
            .iter()
            .filter_map(|(key, results)| {
                let provider = registry.providers.get(key)?;
                Some((
                    provider.title.clone(),
                    key,
                    PluginSection {
                        title: provider.title.clone(),
                        entries: entries(key, &results.actions),
                        semantic: !provider.static_actions,
                        status: results.status.clone(),
                    },
                ))
            })
            .collect();
        sections.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        self.widget.set_sections(
            sections
                .into_iter()
                .map(|(_, _, section)| section)
                .collect(),
        );
    }

    fn request(
        &mut self,
        senders: &yinz_bus::thread_bus::ThreadSenders,
        client: ClientId,
        provider: ProviderKey,
        execution: Option<Execution>,
        source: Option<String>,
    ) {
        let scope = Scope {
            registration: provider.1.clone(),
            opening: self.plugins.opening,
            view: self.widget.view,
            revision: self.plugins.revision,
        };
        let context = wire::Context {
            client_id: client.get(),
            pane_id: self.context.pane_id,
            tab_id: self.context.tab_id.get() as u64,
            current_project: self.context.current_project.clone(),
        };
        let active = Arc::new(AtomicBool::new(true));
        let label = execution
            .as_ref()
            .map(|execution| execution.label.clone())
            .unwrap_or_default();
        let is_execution = execution.is_some();
        if is_execution {
            self.widget.clear_error();
        }
        let message = if let Some(Execution { handle, values, .. }) = execution {
            Message::Execute {
                scope: scope.clone(),
                context,
                handle,
                values,
            }
        } else {
            Message::Discover {
                scope: scope.clone(),
                context,
                query: self.widget.query().into(),
                source,
            }
        };
        deliver(senders, &provider.0, client, message, Some(active.clone()));
        if !is_execution {
            let senders = senders.clone();
            let owner = provider.0.clone();
            let scope = scope.clone();
            let active = active.clone();
            crate::global_async_runtime::get_tokio_runtime().spawn(async move {
                tokio::time::sleep(Duration::from_secs(10)).await;
                if active.load(Ordering::Acquire) {
                    senders
                        .send_to_screen(ScreenInstruction::CommandPalette(Instruction::Timeout {
                            owner,
                            client,
                            scope,
                        }))
                        .context("timing out palette discovery")
                        .non_fatal();
                }
            });
        }
        self.plugins.pending.push(Pending {
            provider,
            scope,
            active,
            execution: is_execution,
            label,
        });
        self.widget.busy = is_execution;
        self.widget.pending = !is_execution;
    }
}

impl Screen {
    pub(crate) fn disconnect_palette_client(&mut self, client: ClientId) {
        let owners: HashSet<_> = self
            .palette_providers
            .providers
            .keys()
            .map(|(owner, _)| owner.clone())
            .collect();
        for owner in owners {
            deliver(&self.bus.senders, &owner, client, Message::Disconnect, None);
        }
        self.palette_providers
            .closed
            .retain(|(_, origin, _), _| *origin != client);
    }

    pub(super) fn open_plugin_palette(&mut self, client: ClientId) {
        self.palette_providers.next_opening += 1;
        if let Some(state) = self.command_palettes.get_mut(&client) {
            state.plugins.opening = self.palette_providers.next_opening;
        }
        self.refresh_plugin_palette(client, false);
    }

    pub(super) fn dismiss_plugin_palette(
        &mut self,
        client: ClientId,
        state: &mut CommandPaletteState,
    ) {
        state.cancel_requests(&self.bus.senders, client);
        let owners: HashSet<_> = state
            .plugins
            .results
            .keys()
            .map(|key| key.0.clone())
            .chain(
                state
                    .plugins
                    .views
                    .values()
                    .map(|view| view.provider.0.clone()),
            )
            .collect();
        for owner in owners {
            deliver(
                &self.bus.senders,
                &owner,
                client,
                Message::Dismiss {
                    opening: state.plugins.opening,
                },
                None,
            );
        }
    }

    pub(super) fn refresh_plugin_palette(&mut self, client: ClientId, debounce: bool) {
        self.remove_unavailable_palette_providers();
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        if state.agents.owns_view(state.widget.view) || state.agents.pending() {
            return;
        }
        state.cancel_requests(&self.bus.senders, client);
        // Discard forward history and its closures when returning to an earlier view.
        let discarded: Vec<_> = state
            .plugins
            .views
            .keys()
            .copied()
            .filter(|view| *view > state.widget.view)
            .collect();
        for view in discarded {
            if let Some(nested) = state.plugins.views.remove(&view) {
                state.release(
                    &self.bus.senders,
                    client,
                    &nested.provider,
                    &Reply::View {
                        view: nested.display,
                    },
                );
            }
        }
        let dynamic = if let Some(nested) = state.plugins.views.get(&state.widget.view) {
            matches!(
                &nested.display,
                View::Actions {
                    source: Some(_),
                    ..
                }
            )
        } else if state.widget.is_root() {
            state
                .plugins
                .results
                .retain(|key, _| self.palette_providers.providers.contains_key(key));
            for (key, provider) in &self.palette_providers.providers {
                if key.0.client_id.is_none_or(|owner| owner == client)
                    && !provider.unavailable.load(Ordering::Acquire)
                {
                    state.plugins.results.entry(key.clone()).or_default().status =
                        Some("Loading…".into());
                }
            }
            !state.plugins.results.is_empty()
        } else {
            false
        };
        state.widget.pending = dynamic;
        state.sections(&self.palette_providers);
        if !dynamic {
            return;
        }
        let opening = state.plugins.opening;
        let revision = state.plugins.revision;
        if debounce {
            let senders = self.bus.senders.clone();
            crate::global_async_runtime::get_tokio_runtime().spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                senders
                    .send_to_screen(ScreenInstruction::CommandPalette(Instruction::Discover {
                        client,
                        opening,
                        revision,
                    }))
                    .context("debouncing palette discovery")
                    .non_fatal();
            });
        } else {
            self.discover_plugin_palette(client, opening, revision);
        }
    }

    fn discover_plugin_palette(&mut self, client: ClientId, opening: u64, revision: u64) -> bool {
        let changed = self.remove_unavailable_palette_providers();
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return changed;
        };
        if state.plugins.opening != opening || state.plugins.revision != revision {
            return changed;
        }
        let requests = if let Some(Nested {
            provider,
            display:
                View::Actions {
                    source: Some(source),
                    ..
                },
        }) = state.plugins.views.get(&state.widget.view)
        {
            vec![(provider.clone(), Some(source.clone()))]
        } else if state.widget.is_root() {
            state
                .plugins
                .results
                .keys()
                .map(|key| (key.clone(), None))
                .collect()
        } else {
            vec![]
        };
        for (provider, source) in requests {
            state.request(&self.bus.senders, client, provider, None, source);
        }
        changed
    }

    pub(super) fn plugin_palette_outcome(&mut self, client: ClientId, outcome: &Outcome) -> bool {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return false;
        };
        if *outcome == Outcome::Cancel {
            state.cancel_requests(&self.bus.senders, client);
            state.sections(&self.palette_providers);
            return true;
        }
        let (id, values) = match outcome {
            Outcome::Activate(id) => (id, None),
            Outcome::Submit { id, values } => (id, Some(values)),
            _ => return false,
        };
        let activation = if let Some(nested) = state.plugins.views.get(&state.widget.view) {
            match &nested.display {
                View::Actions { actions, .. } => actions
                    .iter()
                    .find(|a| entry_id(&nested.provider, &a.id) == *id)
                    .map(|a| {
                        (
                            nested.provider.clone(),
                            a.handle.clone(),
                            None,
                            a.label.clone(),
                        )
                    }),
                View::Form {
                    fields,
                    handle,
                    title,
                    actions,
                    ..
                } => values.map(|values| {
                    (
                        nested.provider.clone(),
                        actions
                            .iter()
                            .find(|a| a.handle == *id)
                            .map(|a| a.handle.clone())
                            .unwrap_or_else(|| handle.clone()),
                        Some(
                            fields
                                .iter()
                                .zip(values)
                                .map(|(f, v)| (f.id().to_owned(), v.clone()))
                                .collect(),
                        ),
                        title.clone(),
                    )
                }),
                _ => None,
            }
        } else {
            state
                .plugins
                .results
                .iter()
                .find_map(|(provider, results)| {
                    results
                        .actions
                        .iter()
                        .find(|a| entry_id(provider, &a.id) == *id)
                        .map(|a| (provider.clone(), a.handle.clone(), None, a.label.clone()))
                })
        };
        let Some((provider, handle, values, label)) = activation else {
            return state.widget.view != 0;
        };
        if state.widget.busy
            || self
                .palette_providers
                .providers
                .get(&provider)
                .is_none_or(|p| p.unavailable.load(Ordering::Acquire))
        {
            return true;
        }
        state.cancel_requests(&self.bus.senders, client);
        state.request(
            &self.bus.senders,
            client,
            provider,
            Some(Execution {
                handle,
                values,
                label: plain_text(&label),
            }),
            None,
        );
        state.sections(&self.palette_providers);
        true
    }

    pub(crate) fn plugin_palette_instruction(&mut self, instruction: Instruction) -> bool {
        match instruction {
            Instruction::Refresh { client, scope } => {
                if self.command_palettes.get(&client).is_some_and(|state| {
                    state.plugins.opening == scope.opening
                        && state.widget.view == scope.view
                        && state.plugins.revision == scope.revision
                        && !state.widget.busy
                }) {
                    self.refresh_plugin_palette(client, false);
                    return true;
                }
                false
            },
            Instruction::AgentReply {
                client,
                opening,
                step,
                reply,
            } => {
                self.agent_palette_reply(client, opening, step, reply);
                true
            },
            Instruction::UsageReply {
                client,
                opening,
                view,
                usage,
            } => {
                self.palette_usage_reply(client, opening, view, usage);
                true
            },
            Instruction::HistoryTick {
                client,
                opening,
                view,
                generation,
            } => {
                self.palette_history_tick(client, opening, view, generation);
                true
            },
            Instruction::Discover {
                client,
                opening,
                revision,
            } => self.discover_plugin_palette(client, opening, revision),
            Instruction::Timeout {
                owner,
                client,
                scope,
            } => self.palette_reply(
                owner,
                client,
                scope,
                Err("Provider timed out after 10 seconds".into()),
                true,
            ),
            Instruction::Dispose {
                plugin_id,
                generation,
            } => {
                let keys: Vec<_> = self
                    .palette_providers
                    .providers
                    .keys()
                    .filter(|(owner, _)| {
                        owner.plugin_id == plugin_id && owner.generation == generation
                    })
                    .cloned()
                    .collect();
                let mut changed = false;
                for key in keys {
                    changed |= self.remove_palette_provider(&key);
                }
                self.palette_providers.closed.retain(|(owner, _, _), _| {
                    owner.plugin_id != plugin_id || owner.generation != generation
                });
                changed
            },
            Instruction::Command {
                owner,
                client,
                unavailable,
                command,
            } => {
                if unavailable.load(Ordering::Acquire)
                    || owner.client_id.is_some_and(|id| id != client)
                {
                    return false;
                }
                match command {
                    Command::Suspend { scope, suspended } => {
                        let Some(state) = self.command_palettes.get_mut(&client) else {
                            return false;
                        };
                        if !state
                            .plugins
                            .pending
                            .iter()
                            .any(|p| p.execution && p.provider.0 == owner && p.scope == scope)
                        {
                            return false;
                        }
                        // Keep the server's saved mode until the palette actually closes.
                        state.suspended = suspended;
                        for tab in self.tabs.values_mut() {
                            tab.set_should_clear_display_before_rendering();
                            tab.set_force_render();
                        }
                        true
                    },
                    Command::Register {
                        id,
                        title,
                        registration,
                        static_actions,
                    } => {
                        if self.palette_providers.providers.iter().any(
                            |((existing, _), provider)| *existing == owner && provider.id == id,
                        ) {
                            return false;
                        }
                        let owning_client = owner.client_id;
                        self.palette_providers.providers.insert(
                            (owner, registration),
                            Provider {
                                id,
                                title,
                                static_actions,
                                unavailable,
                            },
                        );
                        let clients: Vec<_> = self
                            .command_palettes
                            .iter()
                            .filter(|(client, state)| {
                                owning_client.is_none_or(|owner| owner == **client)
                                    && state.widget.is_root()
                                    && !state.widget.busy
                            })
                            .map(|(client, _)| *client)
                            .collect();
                        let changed = !clients.is_empty();
                        for client in clients {
                            self.refresh_plugin_palette(client, false);
                        }
                        changed
                    },
                    Command::Unregister { registration } => {
                        self.remove_palette_provider(&(owner, registration))
                    },
                    Command::Reply { scope, result } => {
                        self.palette_reply(owner, client, scope, result, false)
                    },
                    Command::Close { scope } => {
                        if let Some(state) = self.command_palettes.get_mut(&client)
                            && let Some(index) = state.plugins.pending.iter().position(|p| {
                                p.execution && p.provider.0 == owner && p.scope == scope
                            })
                        {
                            let pending = state.plugins.pending.remove(index);
                            pending.active.store(false, Ordering::Release);
                            self.palette_providers
                                .closed
                                .insert((owner, client, scope), pending.label);
                            self.close_command_palette(client);
                            return true;
                        }
                        false
                    },
                }
            },
        }
    }

    pub(super) fn remove_unavailable_palette_providers(&mut self) -> bool {
        let keys: Vec<_> = self
            .palette_providers
            .providers
            .iter()
            .filter(|(_, provider)| provider.unavailable.load(Ordering::Acquire))
            .map(|(key, _)| key.clone())
            .collect();
        let mut changed = false;
        for key in keys {
            changed |= self.remove_palette_provider(&key);
        }
        changed
    }

    fn remove_palette_provider(&mut self, key: &ProviderKey) -> bool {
        let mut changed = false;
        self.palette_providers.providers.remove(key);
        self.palette_providers
            .closed
            .retain(|(owner, _, scope), _| *owner != key.0 || scope.registration != key.1);
        let mut restored = Vec::new();
        for (client, state) in &mut self.command_palettes {
            if !state.plugins.results.contains_key(key)
                && !state
                    .plugins
                    .views
                    .values()
                    .any(|view| view.provider == *key)
                && !state.plugins.pending.iter().any(|p| p.provider == *key)
            {
                continue;
            }
            changed = true;
            if let Some(results) = state.plugins.results.remove(key) {
                state.release(
                    &self.bus.senders,
                    *client,
                    key,
                    &Reply::Actions {
                        actions: results.actions,
                    },
                );
            }
            if state
                .plugins
                .views
                .values()
                .any(|view| view.provider == *key)
            {
                state.cancel_requests(&self.bus.senders, *client);
                state.suspended = false;
                self.bus
                    .senders
                    .send_to_server(yinz_bus::ServerInstruction::SetCommandPaletteOpen(
                        *client, true,
                    ))
                    .context("restoring palette input")
                    .non_fatal();
                state.widget.reset_to_root();
                for (_, nested) in std::mem::take(&mut state.plugins.views) {
                    state.release(
                        &self.bus.senders,
                        *client,
                        &nested.provider,
                        &Reply::View {
                            view: nested.display,
                        },
                    );
                }
                state
                    .widget
                    .set_error("Palette provider was removed or reloaded".into());
                restored.push(*client);
            } else {
                state.plugins.pending.retain(|p| {
                    if p.provider != *key {
                        return true;
                    }
                    p.active.store(false, Ordering::Release);
                    deliver(
                        &self.bus.senders,
                        &key.0,
                        *client,
                        Message::Cancel {
                            scope: p.scope.clone(),
                        },
                        None,
                    );
                    false
                });
                state.widget.busy =
                    state.agents.pending() || state.plugins.pending.iter().any(|p| p.execution);
                state.widget.pending = state.plugins.pending.iter().any(|p| !p.execution)
                    || state
                        .plugins
                        .results
                        .values()
                        .any(|r| r.status.as_deref() == Some("Loading…"));
            }
            state.sections(&self.palette_providers);
        }
        for client in restored {
            self.refresh_plugin_palette(client, false);
        }
        changed
    }

    fn palette_reply(
        &mut self,
        owner: Owner,
        client: ClientId,
        scope: Scope,
        mut result: Result<Reply, String>,
        timeout: bool,
    ) -> bool {
        if !timeout
            && let Some(label) =
                self.palette_providers
                    .closed
                    .remove(&(owner.clone(), client, scope.clone()))
        {
            if let Ok(reply) = &result {
                deliver(
                    &self.bus.senders,
                    &owner,
                    client,
                    Message::Release {
                        handles: reply.handles(),
                    },
                    None,
                );
                if let Err(error) = reply.validate() {
                    result = Err(error);
                }
            }
            let message = match result {
                Ok(Reply::Complete { notification, .. }) => notification,
                Ok(Reply::Actions { .. }) => {
                    format!("Could not {}: invalid palette reply", label.to_lowercase())
                },
                Ok(_) => format!("Completed: {label}"),
                Err(error) => {
                    log::error!(
                        "[deno:{}] Palette callback failed: {error}",
                        owner.plugin_id
                    );
                    format!("Could not {}: {error}", label.to_lowercase())
                },
            };
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::Notify(
                    message,
                    Some(yinz_types::data::PaneId::Plugin(owner.plugin_id)),
                    None,
                    None,
                ))
                .context("notifying palette completion")
                .non_fatal();
            return true;
        }
        let Some(state) = self.command_palettes.get_mut(&client) else {
            if let Ok(reply) = result {
                deliver(
                    &self.bus.senders,
                    &owner,
                    client,
                    Message::Release {
                        handles: reply.handles(),
                    },
                    None,
                );
            }
            return false;
        };
        let Some(index) =
            state.plugins.pending.iter().position(|p| {
                p.provider.0 == owner && p.scope == scope && (!timeout || !p.execution)
            })
        else {
            if let Ok(reply) = result {
                state.release(
                    &self.bus.senders,
                    client,
                    &(owner, scope.registration),
                    &reply,
                );
            }
            return false;
        };
        let pending = state.plugins.pending.remove(index);
        pending.active.store(false, Ordering::Release);
        if timeout {
            deliver(
                &self.bus.senders,
                &owner,
                client,
                Message::Cancel {
                    scope: scope.clone(),
                },
                None,
            );
        }
        if let Ok(reply) = &result
            && let Err(error) = reply.validate()
        {
            state.release(&self.bus.senders, client, &pending.provider, reply);
            result = Err(error);
        }
        state.widget.busy = false;
        state.widget.pending = state.plugins.pending.iter().any(|p| !p.execution);
        if pending.execution {
            if let Ok(reply @ Reply::Actions { .. }) = &result {
                state.release(&self.bus.senders, client, &pending.provider, reply);
                result = Err("Action returned an invalid palette reply".into());
            }
            let notification = match &result {
                Ok(Reply::Complete { notification, .. }) => Some(notification.clone()),
                Ok(Reply::Close) => Some(format!("Completed: {}", pending.label)),
                Err(error) => {
                    log::error!(
                        "[deno:{}] Palette callback failed: {error}",
                        owner.plugin_id
                    );
                    Some(format!(
                        "Could not {}: {error}",
                        pending.label.to_lowercase()
                    ))
                },
                _ => None,
            };
            if let Some(message) = notification {
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::Notify(
                        message,
                        Some(yinz_types::data::PaneId::Plugin(owner.plugin_id)),
                        None,
                        None,
                    ))
                    .context("notifying palette completion")
                    .non_fatal();
            }
            if let Ok(Reply::Complete { view, .. }) = result {
                state.widget.clear_searches();
                result = Ok(view
                    .map(|view| Reply::View { view })
                    .unwrap_or(Reply::Close));
            }
            match result {
                Ok(Reply::Fields { values }) => {
                    if let Some(Nested {
                        display: View::Form { fields, .. },
                        ..
                    }) = state.plugins.views.get(&state.widget.view)
                    {
                        state.widget.update_form_values(
                            &fields
                                .iter()
                                .map(|f| values.get(f.id()).cloned().unwrap_or_default())
                                .collect::<Vec<_>>(),
                        );
                    }
                },
                Ok(Reply::Close) => {
                    self.close_command_palette(client);
                    return true;
                },
                Ok(Reply::View { view }) => {
                    state.plugins.next_view += 1;
                    let id = state.plugins.next_view;
                    let title = match &view {
                        View::Actions { title, .. }
                        | View::Form { title, .. }
                        | View::Text { title, .. } => title.clone(),
                    };
                    state.widget.push_view(id, title.clone());
                    state.widget.description = match &view {
                        View::Actions { description, .. } | View::Form { description, .. } => {
                            description.clone()
                        },
                        View::Text { .. } => String::new(),
                    };
                    match &view {
                        View::Form {
                            fields,
                            submit_label,
                            handle,
                            actions,
                            confirmation,
                            ..
                        } => {
                            let fields = fields
                                .iter()
                                .map(|field| match field {
                                    wire::Field::Input {
                                        label,
                                        initial_value,
                                        options,
                                        ..
                                    } => {
                                        let mut field = Field::text(label, initial_value);
                                        field.input_options = Some(options.clone());
                                        field
                                    },
                                    wire::Field::Directory {
                                        label,
                                        initial_value,
                                        ..
                                    }
                                    | wire::Field::Text {
                                        label,
                                        initial_value,
                                        ..
                                    } => Field::text(label, initial_value),
                                    wire::Field::Choice {
                                        label,
                                        options,
                                        initial_value,
                                        ..
                                    } => Field {
                                        input_options: None,
                                        capturing: false,
                                        label: label.clone(),
                                        value: FieldValue::Choice {
                                            options: options.clone(),
                                            selected: initial_value
                                                .as_ref()
                                                .and_then(|v| options.iter().position(|o| o == v))
                                                .unwrap_or(0),
                                        },
                                    },
                                })
                                .collect();
                            let mut form = Form::new(
                                handle.clone(),
                                title,
                                fields,
                                submit_label,
                                *confirmation,
                            );
                            form.actions = actions
                                .iter()
                                .map(|a| (a.handle.clone(), a.label.clone()))
                                .collect();
                            if *confirmation {
                                state.widget.metadata = Some(form.title.clone());
                            }
                            state.widget.open_form(form);
                        },
                        View::Text { text, .. } => state.widget.open_text(text.clone()),
                        View::Actions { initial_query, .. } => {
                            state.widget.insert_text(initial_query)
                        },
                    }
                    state.plugins.views.insert(
                        id,
                        Nested {
                            provider: pending.provider,
                            display: view,
                        },
                    );
                    self.refresh_plugin_palette(client, false);
                    return true;
                },
                Err(error) => state.widget.set_error(error),
                Ok(reply) => {
                    state.release(&self.bus.senders, client, &pending.provider, &reply);
                    state
                        .widget
                        .set_error("Action returned an invalid palette reply".into());
                },
            }
        } else {
            match result {
                Ok(Reply::Actions { actions }) => {
                    let old = if scope.view == 0 {
                        state
                            .plugins
                            .results
                            .get(&pending.provider)
                            .map(|r| &r.actions)
                    } else {
                        state.plugins.views.get(&scope.view).and_then(|nested| {
                            match &nested.display {
                                View::Actions { actions, .. } => Some(actions),
                                _ => None,
                            }
                        })
                    };
                    if let Some(old) = old {
                        deliver(
                            &self.bus.senders,
                            &owner,
                            client,
                            Message::Release {
                                handles: old.iter().map(|a| a.handle.clone()).collect(),
                            },
                            None,
                        );
                    }
                    if scope.view == 0 {
                        state.plugins.results.insert(
                            pending.provider,
                            Results {
                                status: actions.is_empty().then(|| "No actions".into()),
                                actions,
                            },
                        );
                    } else if let Some(Nested {
                        display:
                            View::Actions {
                                actions: current, ..
                            },
                        ..
                    }) = state.plugins.views.get_mut(&scope.view)
                    {
                        *current = actions;
                    }
                },
                result => {
                    if let Ok(reply) = &result {
                        state.release(&self.bus.senders, client, &pending.provider, reply);
                    }
                    let error = result
                        .err()
                        .unwrap_or_else(|| "Provider returned an invalid palette reply".into());
                    if scope.view == 0 {
                        state
                            .plugins
                            .results
                            .entry(pending.provider)
                            .or_default()
                            .status = Some(error);
                    } else {
                        state.widget.set_error(error);
                    }
                },
            }
        }
        if matches!(
            state.plugins.views.get(&scope.view),
            Some(Nested {
                display: View::Actions {
                    source: Some(_),
                    ..
                },
                ..
            })
        ) {
            let senders = self.bus.senders.clone();
            crate::global_async_runtime::get_tokio_runtime().spawn(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                senders
                    .send_to_screen(ScreenInstruction::CommandPalette(Instruction::Refresh {
                        client,
                        scope,
                    }))
                    .context("refreshing visible palette source")
                    .non_fatal();
            });
        }
        state.sections(&self.palette_providers);
        true
    }
}
