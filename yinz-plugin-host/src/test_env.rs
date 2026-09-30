use crate::plugin_limits::PluginResourceLimits;
use crate::plugin_map::{PluginEnv, ResponseSink};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use yinz_bus::thread_bus::ThreadSenders;
use yinz_config::plugins::PluginConfig;
use yinz_storage::data_access::DataAccess;
use yinz_types::input::layout::RunPluginLocation;
use yinz_types::internal_ids::TabId;

pub(crate) fn test_env() -> PluginEnv {
    PluginEnv {
        response_sink: ResponseSink::WasiStdin,
        plugin_id: crate::PluginId::new(1),
        instance_id: Default::default(),
        unavailable: Default::default(),
        cancel: Default::default(),
        client_id: crate::ClientId::new(1),
        owner_client_id: crate::ClientId::new(1),
        plugin_config: PluginConfig {
            repository_root: None,
            installed_commit: None,
            path: PathBuf::from("test.ts"),
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from("test.ts")),
            initial_userspace_configuration: Default::default(),
            initial_cwd: None,
            relative_path: None,
        },
        permissions: Arc::new(Mutex::new(None)),
        permissions_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        senders: ThreadSenders::default().silently_fail_on_send(),
        wasi_ctx: None,
        plugin_own_data_dir: PathBuf::from("."),
        temporary_data_dir: None,
        data_access: Arc::new(DataAccess::sqlite(&PathBuf::from("."))),
        deno_watch_generations: Default::default(),
        queues: Default::default(),
        plugin_own_cache_dir: PathBuf::from("."),
        tab_index: Some(TabId::new(0)),
        path_to_default_shell: PathBuf::from("."),
        default_shell: None,
        plugin_cwd: PathBuf::from("."),
        filesystem_watcher: None,
        session_env_vars: BTreeMap::new(),
        // Never the process environment: tests run on threads that share it.
        server_session: crate::ServerSession::Fixed(Default::default()),
        plugin_aliases: Arc::new(Mutex::new(Default::default())),
        input_pipes_to_unblock: Arc::new(Mutex::new(HashSet::new())),
        input_pipes_to_block: Arc::new(Mutex::new(HashSet::new())),
        layout_dir: None,
        user_plugin_dir: None,
        resolved_source_path: None,
        default_mode: Default::default(),
        subscriptions: Arc::new(Mutex::new(HashSet::new())),
        keybinds: Default::default(),
        intercepting_key_presses: false,
        cancel_tripped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        deno_generation: None,
        deno_workers: None,
        stdin_pipe: Arc::new(Mutex::new(VecDeque::new())),
        stdout_pipe: Arc::new(Mutex::new(VecDeque::new())),
        store_limits: PluginResourceLimits::new(crate::PluginId::new(1)),
    }
}
