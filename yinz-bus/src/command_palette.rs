//! Private host routing for Deno palettes. Guests cannot supply an owner or a delivery.
use std::sync::{atomic::AtomicBool, Arc};
use yinz_types::command_palette::{Command, Message, Scope};
use yinz_types::internal_ids::{ClientId, PluginId};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Owner {
    pub plugin_id: PluginId,
    pub generation: u64,
    /// Shared instances serve every client; ordinary instances serve their owner only.
    pub client_id: Option<ClientId>,
}

#[derive(Clone, Debug)]
pub struct Delivery {
    pub owner: Owner,
    pub client: ClientId,
    pub message: Message,
    /// Invalidated synchronously by screen input, before debounce or executor delivery.
    pub active: Option<Arc<AtomicBool>>,
}

#[derive(Clone, Debug)]
pub enum Instruction {
    Command {
        owner: Owner,
        client: ClientId,
        unavailable: Arc<AtomicBool>,
        command: Command,
    },
    Dispose {
        plugin_id: PluginId,
        generation: u64,
    },
    Discover {
        client: ClientId,
        opening: u64,
        revision: u64,
    },
    Timeout {
        owner: Owner,
        client: ClientId,
        scope: Scope,
    },
}
