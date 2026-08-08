//! A development plugin for exercising "shared" plugins - plugins that run as a single wasm
//! instance for the whole session rather than one instance per client.
//!
//! Launch it with:
//!   zellij plugin --floating -c shared=true -- zellij:shared-test
//!
//! Attach a second client and the same instance renders a different view for each of them.
use std::collections::BTreeMap;
use std::path::PathBuf;
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    plugin_id: u32,
    presses: BTreeMap<ClientId, u32>,
    total: u32,
    last_presser: Option<ClientId>,
    /// The client ids `render` was called for so far in the current render sweep, in order. The
    /// host always renders the owner of a shared plugin first, so `sweep[0]` is the current owner.
    sweep: Vec<ClientId>,
    /// The previous, complete sweep - the clients this plugin was last rendered for. We display
    /// this rather than `sweep`, which is by definition partial while we're rendering.
    last_sweep: Vec<ClientId>,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        subscribe(&[EventType::Key]);
        request_permission(&[PermissionType::ChangeApplicationState]);
        self.plugin_id = get_plugin_ids().plugin_id;
        rename_plugin_pane(self.plugin_id, "shared-test");
    }

    fn update(&mut self, event: Event) -> bool {
        let Event::Key(key) = event else {
            return false;
        };
        // for a shared plugin the host sets the acting client to whoever sent the event, so this
        // is the client that actually pressed the key - not necessarily the instance's owner
        let client_id = get_plugin_ids().client_id;
        *self.presses.entry(client_id).or_default() += 1;
        self.total += 1;
        self.last_presser = Some(client_id);

        match key.bare_key {
            // both of these act on a single client - they should affect the presser only
            BareKey::Char('m') => switch_to_input_mode(&InputMode::Locked),
            BareKey::Char('f') => {
                open_terminal_floating(PathBuf::from("."), None);
            },
            BareKey::Esc => close_self(),
            _ => {},
        }
        true
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        let me = get_plugin_ids().client_id;
        // seeing a client twice means a new sweep started, and the one we had is complete
        if self.sweep.contains(&me) {
            self.last_sweep = std::mem::take(&mut self.sweep);
        }
        self.sweep.push(me);
        let owner = self.sweep[0];
        let rendered_for = if self.last_sweep.is_empty() {
            &self.sweep
        } else {
            &self.last_sweep
        };

        let per_client: Vec<String> = self
            .presses
            .iter()
            .map(|(client_id, count)| format!("{} -> {}", client_id, count))
            .collect();
        let last_presser = self
            .last_presser
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".to_owned());

        println!(
            "\u{1b}[1mshared-test\u{1b}[0m   plugin_id={}",
            self.plugin_id
        );
        println!(
            "you are client {}\towner: {}\trendered for: {:?}",
            me, owner, rendered_for
        );
        println!(
            "your presses: {}\ttotal: {}\tlast presser: {}",
            self.presses.get(&me).copied().unwrap_or(0),
            self.total,
            last_presser
        );
        println!("per client: {}", per_client.join("   "));
        println!("[m] locked mode   [f] floating pane   [Esc] close");
    }
}
