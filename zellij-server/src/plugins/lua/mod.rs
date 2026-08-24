//! The Lua plugin runtime.
//!
//! Zellij's original plugins are wasm modules, which are excellent for isolation and terrible for
//! casual customization: writing one means a Rust crate and a `wasm32-wasip1` toolchain. Lua
//! plugins exist so that a user can drop a `.lua` file on disk and have it run.
//!
//! The design deliberately mirrors the wasm host rather than inventing a parallel API. There,
//! exactly one function is imported by the guest (`host_run_plugin_command`), and every
//! capability is a `PluginCommand` sent through it. Here, exactly one function is exposed to Lua
//! (`__dispatch`), and it builds the very same `PluginCommand` and hands it to the very same
//! [`run_plugin_command`]. No capability is implemented twice.
//!
//! Everything a plugin author sees - `zellij.subscribe`, `zellij.on`, event normalization - is
//! built on top of `__dispatch` by [`prelude.lua`](./prelude.lua), in Lua. Keeping the Rust
//! boundary this thin means the API can grow ergonomics without growing this file.

mod command;
mod loader;
#[cfg(test)]
mod unit_tests;

pub use loader::load_lua_plugin;

use std::cell::RefCell;

use mlua::{Lua, LuaSerdeExt, Table, Value as LuaValue};
use zellij_utils::data::{Event, PipeMessage};
use zellij_utils::errors::prelude::*;

use crate::plugins::plugin_map::{PluginEnv, ResponseSink};
use crate::plugins::zellij_exports::run_plugin_command;
use command::plugin_command_from_lua;

/// The Lua source shipped with zellij and run before every plugin. See the module docs.
const PRELUDE: &str = include_str!("prelude.lua");

/// A loaded Lua plugin: its interpreter, the table its source file returned, and the environment
/// host commands act on.
pub struct LuaPlugin {
    lua: Lua,
    /// The table returned by the plugin's source file, holding its `load`/`update`/`render`/
    /// `pipe` entrypoints.
    plugin: Table,
    /// The `zellij` module from the prelude, which owns the entrypoint wrappers.
    zellij: Table,
    env: PluginEnv,
}

impl LuaPlugin {
    pub fn env(&self) -> &PluginEnv {
        &self.env
    }

    pub fn env_mut(&mut self) -> &mut PluginEnv {
        &mut self.env
    }

    pub fn load(
        &mut self,
        configuration: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        let config = self.lua.to_value(configuration).map_err(lua_err)?;
        self.call_entrypoint("__load", |plugin| Ok((plugin, config)))
            .context("failed to run the plugin's `load`")
    }

    pub fn update(&mut self, event: &Event) -> Result<bool> {
        let event = self.lua.to_value(event).map_err(lua_err)?;
        self.call_entrypoint("__update", |plugin| Ok((plugin, event)))
            .context("failed to run the plugin's `update`")
    }

    pub fn pipe(&mut self, message: &PipeMessage) -> Result<bool> {
        let message = self.lua.to_value(message).map_err(lua_err)?;
        self.call_entrypoint("__pipe", |plugin| Ok((plugin, message)))
            .context("failed to run the plugin's `pipe`")
    }

    pub fn render(&mut self, rows: usize, columns: usize) -> Result<Vec<u8>> {
        let rendered: String = self
            .call_entrypoint("__render", |plugin| Ok((plugin, rows, columns)))
            .context("failed to run the plugin's `render`")?;
        // Plugins write plain `\n`, but the pane's vte parser needs a carriage return too - the
        // wasm host does the same fixup when it drains the guest's stdout.
        Ok(rendered.replace('\n', "\n\r").into_bytes())
    }

    /// Call one of the prelude's entrypoint wrappers with `zellij.__dispatch` bound to this
    /// plugin's environment for the duration of the call.
    ///
    /// Host commands mutate `PluginEnv`, so the dispatch function needs `&mut PluginEnv` while
    /// Lua holds it. `Lua::scope` is what makes that sound: the function it creates is torn down
    /// when the scope ends, so the borrow cannot outlive this call and nothing has to be
    /// reference-counted, cloned, or made `'static`.
    fn call_entrypoint<A, R>(
        &mut self,
        entrypoint: &str,
        args: impl FnOnce(Table) -> mlua::Result<A>,
    ) -> Result<R>
    where
        A: mlua::IntoLuaMulti,
        R: mlua::FromLuaMulti,
    {
        // Split the borrows so the scoped closure can hold `env` mutably while `lua` is used.
        let LuaPlugin {
            lua,
            plugin,
            zellij,
            env,
        } = self;
        let env = RefCell::new(env);

        let result = lua.scope(|scope| {
            let entry: mlua::Function = zellij.get(entrypoint)?;
            // Built before `__dispatch` is installed so that a failure here cannot leave it
            // installed with nothing to clear it.
            let args = args(plugin.clone())?;
            let dispatch_fn = scope.create_function(
                |lua, (command_name, args, spelling): (String, LuaValue, Option<String>)| {
                    dispatch(lua, &env, command_name, args, spelling)
                },
            )?;
            zellij.set("__dispatch", dispatch_fn)?;
            let out = entry.call::<R>(args);
            // Cleared whether or not the call succeeded: leaving it installed would let a plugin
            // stash the function and call it later, after the borrow it closes over is gone.
            zellij.set("__dispatch", LuaValue::Nil)?;
            out
        });

        result.map_err(|e| anyhow!("{}", e))
    }
}

fn lua_err(e: mlua::Error) -> anyhow::Error {
    anyhow!("{}", e)
}

/// Build the `zellij.__dispatch` implementation: Lua's single door into the host.
fn dispatch(
    lua: &Lua,
    env: &RefCell<&mut PluginEnv>,
    command_name: String,
    args: LuaValue,
    // How the plugin spelled the command. Carried only so that errors quote it back the way it
    // was written, rather than in the pascal-cased spelling the prelude hands over.
    spelling: Option<String>,
) -> mlua::Result<LuaValue> {
    let spelling = spelling.unwrap_or_else(|| command_name.clone());
    let command = plugin_command_from_lua(lua, &command_name, &spelling, args)
        .map_err(|e| mlua::Error::runtime(format!("{:#}", e)))?;

    // `try_borrow_mut` rather than `borrow_mut`: a re-entrant call would otherwise panic, and a
    // panic here would take down the plugin thread rather than just this plugin.
    let mut env = env.try_borrow_mut().map_err(|_| {
        mlua::Error::runtime("zellij commands cannot be called re-entrantly".to_owned())
    })?;
    // Replies are written through the same `wasi_write_object` the wasm host uses; capturing
    // them here is what lets the handlers stay runtime-agnostic.
    env.response_sink = ResponseSink::Capture(None);
    let outcome = run_plugin_command(&mut env, command);
    let captured = env.response_sink.take();
    env.response_sink = ResponseSink::WasiStdin;

    outcome.map_err(|e| mlua::Error::runtime(format!("{:#}", e)))?;

    match captured {
        // The reply is protobuf bytes, JSON-encoded by `wasi_write_object`. Handing them back as
        // an opaque byte array is enough for now; decoding each response type into a Lua table
        // comes with the rest of the command surface.
        Some(reply) => match serde_json::from_str::<Vec<u8>>(&reply) {
            Ok(bytes) => lua.create_string(&bytes).map(LuaValue::String),
            Err(_) => lua.create_string(&reply).map(LuaValue::String),
        },
        None => Ok(LuaValue::Nil),
    }
}
