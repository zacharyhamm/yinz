//! hocus-agentd — watches for coding agents, whether or not hocus is open.
//!
//! Background-only plugin. Never rendered.
//!
//! hocus can do this poll itself and still does when the daemon is not there.
//! What it cannot do is keep doing it once its pane closes, which is the whole
//! reason this exists: `agents::observe` counts *consecutive* unchanged polls,
//! so busy/idle is only meaningful across an unbroken series of them. A hocus
//! that reopens onto a cold snapshot has to rebuild that history from zero.

mod poll;

use hocus_proto::agents;
use hocus_proto::wire::{self, PollReply, PollRequest};
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

/// How often the snapshot is refreshed. Matches hocus's own `AGENT_SECONDS` —
/// the two polls feed one `activity` map, and a daemon on a different cadence
/// would make the idle counts mean something different depending on who polled.
const AGENT_SECONDS: f64 = 5.0;

#[derive(Default)]
struct State {
    /// Overrides from hocus's `agent_register`, refreshed by every subscribe.
    /// hocus stays authoritative; this is only an input to `classify`.
    manual_kinds: BTreeMap<u32, String>,
    activity: agents::Activity,
    /// Who to push snapshots to — hocus, and hocus-store for its project
    /// discovery. A list rather than a single slot because both want the same
    /// snapshot, and broadcasting it instead would hand every plugin in the
    /// session the title and working folder of every pane.
    subscribers: Vec<u32>,
    seq: u64,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        subscribe(&[EventType::Timer]);
        // No `request_permission` here, deliberately. A builtin plugin — one
        // loaded from a `zellij:` location — is granted every gated call
        // unconditionally by `check_command_permission`, which is why almost no
        // other default plugin asks either.
        //
        // Asking anyway would not merely be redundant, it would strand this
        // plugin: the permission dialog is drawn *in the requesting plugin's
        // pane*, and a background plugin has none, so the request is cached
        // against a pane that never arrives and the answer never comes.

        // One chain only. The alias sets `shared true` precisely so there is a
        // single instance per session — without it Zellij builds one per
        // client, and each would poll the same session on its own timer.
        set_timeout(AGENT_SECONDS);
        // A reload drops the in-memory subscriber list. Ask existing watchers
        // to subscribe again instead of silently stopping their snapshots.
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_HELLO));
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::Timer(_) => {
                self.poll_and_publish();
                // One-shot timer, so the chain has to re-arm itself.
                set_timeout(AGENT_SECONDS);
            },
            _ => {},
        }
        // Never rendered, so never asks to be.
        false
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name == wire::AGENT_SUBSCRIBE {
            // A CLI pipe has no plugin id to reply to; it can still refresh the
            // overrides and force a poll, which is what makes the daemon
            // pokeable from a terminal for debugging.
            if let PipeSource::Plugin(id) = pipe_message.source {
                if !self.subscribers.contains(&id) {
                    self.subscribers.push(id);
                }
            }
            // Only when the subscriber actually sent overrides. hocus-store
            // subscribes with no payload, and treating that as "no manual
            // kinds" would throw away hocus's registrations.
            if let Some(request) = pipe_message
                .payload
                .as_deref()
                .and_then(|json| serde_json::from_str::<PollRequest>(json).ok())
            {
                self.manual_kinds = request.manual_kinds;
            }
            // Answer immediately rather than making the caller wait out the
            // heartbeat — a hocus that just opened wants the snapshot now.
            self.poll_and_publish();
        }
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
    fn poll_and_publish(&mut self) {
        let panes = poll::gather(&self.manual_kinds, &mut self.activity);
        self.seq += 1;
        let reply = PollReply {
            panes,
            activity: self.activity.clone(),
            manual_kinds: self.manual_kinds.clone(),
        };
        let Ok(payload) = serde_json::to_string(&reply) else {
            return;
        };
        // Addressed by plugin id, never by url: `with_plugin_url("zellij:hocus")`
        // would *launch* hocus, which is precisely what a background monitor
        // must not do. No subscribers means nobody has asked yet — keep polling
        // anyway, because the point is that the history is already warm when
        // someone finally does.
        for id in &self.subscribers {
            pipe_message_to_plugin(
                MessageToPlugin::new(wire::AGENT_SNAPSHOT)
                    .with_destination_plugin_id(*id)
                    .with_payload(payload.clone())
                    .with_args(BTreeMap::from([("seq".to_owned(), self.seq.to_string())])),
            );
        }
    }
}
