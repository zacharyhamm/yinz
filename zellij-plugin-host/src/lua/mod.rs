//! The Lua plugin runtime.
//!
//! Zellij's original plugins are wasm modules, which are excellent for isolation and terrible for
//! casual customization: writing one means a Rust crate and a `wasm32-wasip1` toolchain. Lua
//! plugins exist so that a user can drop a `.lua` file on disk and have it run.
//!
//! The design deliberately mirrors the wasm host rather than inventing a parallel API. There,
//! exactly one function is imported by the guest (`host_run_plugin_command`), and every
//! capability is a `PluginCommand` sent through it. Here, exactly one function gives Lua access to host authority
//! (`__dispatch`), and it builds the very same `PluginCommand` and hands it to the very same
//! [`run_plugin_command`]. No capability is implemented twice.
//!
//! Everything a plugin author sees - `zellij.subscribe`, `zellij.on`, event normalization - is
//! built on top of `__dispatch` by [`prelude.lua`](./prelude.lua), in Lua. Keeping the Rust
//! boundary this thin means the API can grow ergonomics without growing this file.

mod command;
mod loader;
mod response;
mod ui;
#[cfg(test)]
pub(crate) mod unit_tests;

pub use loader::{check_lua_plugin, load_lua_plugin};

use std::cell::RefCell;

use mlua::{Lua, Table, Value as LuaValue};
use zellij_runtime::errors::prelude::*;
use {
    zellij_types::data::CommandType, zellij_types::data::Event,
    zellij_types::data::PermissionStatus, zellij_types::data::PipeMessage,
};

use crate::plugin_map::{PluginEnv, ResponseSink};
use crate::zellij_exports::{
    check_command_permission, permission_answer_pending, run_plugin_command,
};
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
        let config = to_lua_value(&self.lua, configuration)?;
        self.call_entrypoint("__load", |plugin| Ok((plugin, config)))
            .context("failed to run the plugin's `load`")
    }

    pub fn update(&mut self, event: &Event) -> Result<bool> {
        // Live output is binary text. Avoid constructing and walking a Lua table per byte.
        let event = match event {
            Event::RunCommandOutput {
                data,
                stderr,
                context,
            } => {
                let payload = self.lua.create_table().map_err(lua_err)?;
                payload
                    .set("data", self.lua.create_string(data).map_err(lua_err)?)
                    .map_err(lua_err)?;
                payload.set("stderr", *stderr).map_err(lua_err)?;
                payload
                    .set("context", to_lua_value(&self.lua, context)?)
                    .map_err(lua_err)?;
                let event = self.lua.create_table().map_err(lua_err)?;
                event.set("RunCommandOutput", payload).map_err(lua_err)?;
                LuaValue::Table(event)
            },
            Event::PaneOutput {
                data,
                pane_id,
                context,
            } => {
                let payload = self.lua.create_table().map_err(lua_err)?;
                payload
                    .set("data", self.lua.create_string(data).map_err(lua_err)?)
                    .map_err(lua_err)?;
                payload.set("pane_id", *pane_id).map_err(lua_err)?;
                payload
                    .set("context", to_lua_value(&self.lua, context)?)
                    .map_err(lua_err)?;
                let event = self.lua.create_table().map_err(lua_err)?;
                event.set("PaneOutput", payload).map_err(lua_err)?;
                LuaValue::Table(event)
            },
            _ => to_lua_value(&self.lua, event)?,
        };
        self.call_entrypoint("__update", |plugin| Ok((plugin, event)))
            .context("failed to run the plugin's `update`")
    }

    pub fn pipe(&mut self, message: &PipeMessage) -> Result<bool> {
        let message = to_lua_value(&self.lua, message)?;
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

    // Kept for the reply: `run_plugin_command` consumes the command, and decoding its answer
    // needs to know which command asked.
    let command_type = CommandType::from(&command);

    // `try_borrow_mut` rather than `borrow_mut`: a re-entrant call would otherwise panic, and a
    // panic here would take down the plugin thread rather than just this plugin.
    let mut env = env.try_borrow_mut().map_err(|_| {
        mlua::Error::runtime("zellij commands cannot be called re-entrantly".to_owned())
    })?;

    // Asked here rather than left to `run_plugin_command`, whose denied branch only logs: a wasm
    // guest is mid-call and cannot be handed a refusal, but a Lua plugin's command is an ordinary
    // function call, and silently doing nothing is the worst answer it could get.
    if let (PermissionStatus::Denied, Some(permission)) = check_command_permission(&env, &command) {
        // Two different failures wear the same denial, and they have opposite fixes. A plugin
        // that has asked and not yet been *answered* - which is every plugin that asks in `load`,
        // for the whole of `load`, even one whose permissions were granted long ago, because the
        // grant is carried by an event that has not arrived - is refused everything. Telling that
        // author to ask in `load` names what they already did; what they need is to wait for the
        // answer. A plugin that never asked gets the ordinary denial below, which is its actual
        // fix. See `permission_answer_pending`.
        if permission_answer_pending(&env) {
            return Err(mlua::Error::runtime(format!(
                "'{spelling}' needs the '{permission}' permission, and no permission has been \
                 answered for this plugin yet - a grant arrives as an event, so nothing gated \
                 works during `load`. Ask in `load` with zellij.request_permission {{ \
                 \"{permission}\" }}, and do this from a \
                 zellij.on(\"permission_request_result\", ...) handler"
            )));
        }
        return Err(mlua::Error::runtime(format!(
            "'{spelling}' needs the '{permission}' permission, which this plugin has not been \
             granted - ask for it in `load` with zellij.request_permission {{ \"{permission}\" }}"
        )));
    }
    // Replies are written through the same `wasi_write_object` the wasm host uses; capturing
    // them here is what lets the handlers stay runtime-agnostic.
    env.response_sink = ResponseSink::Capture(None);
    let outcome = run_plugin_command(&mut env, command);
    let captured = env.response_sink.take();
    env.response_sink = ResponseSink::WasiStdin;

    outcome.map_err(|e| mlua::Error::runtime(format!("{:#}", e)))?;

    // The reply is protobuf bytes, JSON-encoded by `wasi_write_object`, because that is what the
    // wasm guest reads off its stdin pipe. `response_to_lua` decodes the ones it knows into a
    // table; anything else is handed back as the bytes it always was.
    match captured {
        Some(reply) => match serde_json::from_str::<Vec<u8>>(&reply) {
            Ok(bytes) => match response::response_to_lua(lua, command_type, &bytes) {
                Some(decoded) => decoded.map_err(|e| mlua::Error::runtime(format!("{:#}", e))),
                None => lua.create_string(&bytes).map(LuaValue::String),
            },
            Err(_) => lua.create_string(&reply).map(LuaValue::String),
        },
        None => Ok(LuaValue::Nil),
    }
}

/// Serialize a host value for Lua. An absent `Option` becomes `nil`, not mlua's `null`
/// userdata: a plugin checking `if event.payload then` must get the answer Lua would give.
///
/// It is serialized as `null` first and turned into `nil` afterwards, rather than as `nil`
/// directly, because of where a `None` can sit: mlua builds a tuple variant's fields with
/// `raw_push`, and pushing `nil` is a no-op, so `CommandPaneExited(7, None, context)` would
/// arrive as `{ 7, context }` with the context in the exit code's place. A `null` holds the
/// slot; clearing it afterwards leaves the slot empty and everything after it where it was.
pub(crate) fn to_lua_value<T: serde::Serialize + ?Sized>(lua: &Lua, value: &T) -> Result<LuaValue> {
    use mlua::{serde::SerializeOptions, LuaSerdeExt};
    let value = lua
        .to_value_with(
            value,
            SerializeOptions::new()
                .serialize_none_to_null(true)
                .serialize_unit_to_null(false),
        )
        .map_err(lua_err)?;
    nulls_to_nil(lua, value).map_err(lua_err)
}

/// Replace every mlua `null` in `value`, at any depth, with `nil`. Clearing a table slot keeps
/// the slots after it in place, which is the whole point - see [`to_lua_value`].
fn nulls_to_nil(lua: &Lua, value: LuaValue) -> mlua::Result<LuaValue> {
    use mlua::LuaSerdeExt;
    if value == lua.null() {
        return Ok(LuaValue::Nil);
    }
    if let LuaValue::Table(table) = &value {
        let entries: Vec<(LuaValue, LuaValue)> = table
            .pairs::<LuaValue, LuaValue>()
            .collect::<mlua::Result<_>>()?;
        for (key, entry) in entries {
            let is_null = entry == lua.null();
            let is_table = matches!(entry, LuaValue::Table(_));
            if is_null {
                table.raw_set(key, LuaValue::Nil)?;
            } else if is_table {
                table.raw_set(key, nulls_to_nil(lua, entry)?)?;
            }
        }
    }
    Ok(value)
}
