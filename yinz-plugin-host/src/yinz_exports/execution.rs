//! Plugin timers, subprocess jobs, and HTTP requests.

use super::pane_creation::translate_plugin_path;
use crate::plugin_map::PluginEnv;
use crate::PluginInstruction;
use log::warn;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process;
use std::time::{Duration, Instant};
use yinz_bus::background_jobs::BackgroundJob;
use yinz_bus::global_async_runtime::get_tokio_runtime;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{Event, HttpVerb};
use yinz_types::units::FractionalSeconds;

pub(super) fn timeout_duration(secs: FractionalSeconds) -> Result<Duration> {
    Duration::try_from_secs_f64(secs.get()).map_err(anyError::new)
}

pub(super) fn set_timeout(env: &PluginEnv, secs: FractionalSeconds) {
    let duration = match timeout_duration(secs) {
        Ok(duration) => duration,
        Err(e) => {
            Err::<(), _>(e)
                .with_context(|| format!("invalid timeout of {secs} s for plugin {}", env.name()))
                .non_fatal();
            return;
        },
    };
    let senders = yinz_bus::thread_bus::ThreadSenders {
        to_plugin: env.senders.to_plugin.clone(),
        should_silently_fail: env.senders.should_silently_fail,
        ..Default::default()
    };
    let update_target = env.plugin_id;
    let generation = env.generation;
    let client_id = env.client_id;
    let plugin_name = env.name();
    // Use tokio runtime for async I/O (timer operation)
    let start_time = Instant::now();
    get_tokio_runtime().spawn(async move {
        tokio::time::sleep(duration).await;
        // This measures the host timer alone. The `Timer` event then waits on the plugin bus, in
        // `cached_events_for_pending_plugins` while any instance of this plugin id is loading,
        // reloading or awaiting permission, and behind earlier work on the plugin's executor or
        // plugin worker, which can add seconds. A plugin that needs to know how long it really
        // waited reads its own clock.
        let elapsed_time = Instant::now().duration_since(start_time).as_secs_f64();

        senders
            .send_to_plugin(PluginInstruction::for_plugin_event(
                update_target,
                client_id,
                generation,
                Event::Timer(FractionalSeconds::new(elapsed_time)),
            ))
            .with_context(|| {
                format!(
                    "failed to set host timeout of {secs} s for plugin {}",
                    plugin_name
                )
            })
            .non_fatal();
    });
}

pub(super) fn exec_cmd(env: &PluginEnv, mut command_line: Vec<String>) {
    log::warn!("The ExecCmd plugin command is deprecated and will be removed in a future version. Please use RunCmd instead (it has all the things and can even show you STDOUT/STDERR and an exit code!)");
    if command_line.is_empty() {
        log::warn!("Cannot execute an empty command");
        return;
    }
    let err_context = || {
        format!(
            "failed to execute command on host for plugin '{}'",
            env.name()
        )
    };
    let command = command_line.remove(0);

    // Bail out if we're forbidden to run command
    if !env.plugin_config._allow_exec_host_cmd {
        warn!("This plugin isn't allow to run command in host side, skip running this command: '{cmd} {args}'.",
            cmd = command, args = command_line.join(" "));
        return;
    }

    // Here, we don't wait the command to finish
    process::Command::new(command)
        .args(command_line)
        .spawn()
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn run_command(
    env: &PluginEnv,
    mut command_line: Vec<String>,
    env_variables: BTreeMap<String, String>,
    cwd: PathBuf,
    context: BTreeMap<String, String>,
    max_output_bytes: Option<usize>,
    stream_output: Option<bool>,
) {
    if command_line.is_empty() {
        log::error!("Command cannot be empty");
    } else {
        let command = command_line.remove(0);
        let cwd = translate_plugin_path(env, cwd);
        let _ = env
            .senders
            .send_to_background_jobs(BackgroundJob::RunCommand(
                env.plugin_id,
                env.client_id,
                command,
                command_line,
                env_variables,
                cwd,
                context,
                max_output_bytes,
                stream_output,
                env.generation,
            ));
    }
}

pub(super) fn web_request(
    env: &PluginEnv,
    url: String,
    verb: HttpVerb,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    context: BTreeMap<String, String>,
    stream_id: Option<String>,
    proxy: Option<String>,
) {
    let _ = env
        .senders
        .send_to_background_jobs(BackgroundJob::WebRequest(
            env.plugin_id,
            env.client_id,
            url,
            verb,
            headers,
            body,
            context,
            stream_id,
            proxy,
            env.generation,
        ));
}

pub(super) fn cancel_web_request(env: &PluginEnv, stream_id: String) {
    let _ = env
        .senders
        .send_to_background_jobs(BackgroundJob::CancelWebRequest(
            env.plugin_id,
            env.client_id,
            stream_id,
        ));
}
