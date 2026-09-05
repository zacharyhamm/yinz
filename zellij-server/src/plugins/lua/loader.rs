//! Creating a Lua interpreter for one plugin and running its source.

use std::path::Path;

use mlua::{Lua, StdLib, Table, Value as LuaValue};
use uuid::Uuid;
use zellij_utils::errors::prelude::*;

use super::{LuaPlugin, PRELUDE};
use crate::plugins::plugin_map::PluginEnv;

/// The standard library a plugin gets.
///
/// A wasm plugin reaches the filesystem only through the four directories zellij preopens for it,
/// and only through paths the host has rewritten - so the permission system is the whole of its
/// authority. Lua has no such boundary, and stock `io`, `os`, and `package.loadlib` would hand a
/// plugin the server process's full authority and make the permission prompts meaningless.
///
/// So the parts of the stdlib that reach outside the interpreter stay shut, and the capabilities
/// worth having arrive as host commands instead - the `Fs*` family, which goes through the same
/// four roots the wasm side sees and is gated on `ReadFiles`/`WriteFiles` like every other
/// capability.
///
/// What is opened here is what cannot reach outside the VM on its own: `string`, `table`, `math`,
/// `utf8`, `coroutine` and the base library, plus the harmless part of `os`, which
/// [`restrict_table`] cuts down below. `io` and `package` are never opened at all, which is what
/// keeps `package.loadlib` - and `package.loaded`, through which the discarded original `os`
/// could otherwise be recovered - out of reach.
///
/// `debug` is absent because mlua will not open it in a safe interpreter at all: the library can
/// reach upvalues and userdata metatables, which is memory-unsafety rather than merely too much
/// authority. Having `debug.traceback` would mean building the whole interpreter unsafely, which
/// is a poor trade for a nicer error message.
fn plugin_stdlib() -> StdLib {
    StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE | StdLib::OS
}

/// The `os` functions a plugin keeps: the clock, and nothing else.
///
/// A wasm plugin reads the clock through WASI with no permission at all, so time is not authority
/// and asking the user to approve it would be noise. Everything else `os` offers is: `execute`
/// and `exit` are the process, `getenv` is the server's environment, `remove`/`rename` are the
/// filesystem without the confinement the `Fs*` commands apply, `tmpname` hands out a path
/// outside every root, and `setlocale` mutates process-global C locale for the whole server -
/// a plugin changing how the *application* formats numbers.
const ALLOWED_OS: &[&str] = &["time", "date", "difftime", "clock"];

const DISALLOWED_GLOBALS: &[&str] = &["dofile", "loadfile", "load", "loadstring", "require"];

/// Load a Lua plugin: build its interpreter, run the prelude, then run the plugin's own source
/// and keep the table it returns.
/// Compile `source` as a plugin chunk without running it, answering with the compile error.
///
/// This is the same interpreter shape `load_lua_plugin` builds, so what compiles here loads
/// there — but only *compiles*: a plugin that parses and then fails in `load`, or returns
/// something other than a table, is a runtime error this cannot see. The chunk is named as the
/// loader would name it, so the error reads `keys.lua:3: ...` either way.
pub fn check_lua_plugin(source: &str, chunk_name: &str) -> std::result::Result<(), String> {
    let lua = Lua::new_with(plugin_stdlib(), mlua::LuaOptions::default())
        .map_err(|e| format!("failed to create a Lua interpreter: {}", e))?;
    lua.load(source)
        .set_name(chunk_name)
        .into_function()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// How many VM instructions run between looks at the plugin's cancel token. Low enough that a
/// loop is stopped within milliseconds, high enough that the hook is not what a render costs.
const CANCEL_CHECK_EVERY_N_INSTRUCTIONS: u32 = 10_000;

pub fn load_lua_plugin(source: &str, chunk_name: &str, env: PluginEnv) -> Result<LuaPlugin> {
    let lua = Lua::new_with(plugin_stdlib(), mlua::LuaOptions::default())
        .map_err(|e| anyhow!("failed to create a Lua interpreter: {}", e))?;

    // The Lua half of `PluginEnv::cancel`: an instruction-count hook, so a loop that never calls
    // the host is stopped too. The hook owns its own handles on the flags because `PluginEnv` is
    // borrowed by `Lua::scope` for the length of every entrypoint call.
    //
    // A *global* hook, not `set_hook`: a coroutine inherits the C hook of the thread that
    // created it, but mlua's per-thread hook looks its callback up by thread and, finding none
    // for the coroutine, uninstalls itself — so a loop inside `coroutine.wrap` would run
    // unhooked. The global hook has one callback for every thread of this interpreter.
    let cancel = env.cancel.clone();
    let cancel_tripped = env.cancel_tripped.clone();
    lua.set_global_hook(
        mlua::HookTriggers::new().every_nth_instruction(CANCEL_CHECK_EVERY_N_INSTRUCTIONS),
        move |_lua, _debug| {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                cancel_tripped.store(true, std::sync::atomic::Ordering::Relaxed);
                Err(mlua::Error::runtime(
                    crate::plugins::plugin_map::CANCELLED_BY_HOST,
                ))
            } else {
                Ok(mlua::VmState::Continue)
            }
        },
    )
    .map_err(|e| anyhow!("failed to install the cancel hook: {}", e))?;

    let globals = lua.globals();
    for global in DISALLOWED_GLOBALS {
        globals
            .set(*global, LuaValue::Nil)
            .map_err(|e| anyhow!("failed to restrict '{}': {}", global, e))?;
    }

    restrict_table(&lua, &globals, "os", ALLOWED_OS)?;

    // The prelude wraps `pcall`, `xpcall` and `coroutine.resume` to re-raise this error rather
    // than return it: the hook above is an ordinary Lua error, and a loop around a `pcall`
    // would otherwise swallow the cancel and keep going. Handed over as a global the prelude
    // takes and nils, so the message is spelled once, in Rust.
    globals
        .set(
            "__zellij_cancelled_by_host",
            crate::plugins::plugin_map::CANCELLED_BY_HOST,
        )
        .map_err(|e| anyhow!("failed to hand the cancel message to the prelude: {}", e))?;
    globals
        .set(
            "__zellij_interpreter_id",
            Uuid::new_v4().simple().to_string(),
        )
        .map_err(|e| anyhow!("failed to hand the interpreter id to the prelude: {}", e))?;

    let zellij: Table = lua
        .load(PRELUDE)
        .set_name("zellij:prelude")
        .eval()
        .map_err(|e| anyhow!("failed to load zellij's Lua prelude: {}", e))?;

    install_json(&lua, &zellij)?;
    // `zellij.null`: the one value a Lua table cannot hold that a tuple command may need in the
    // middle - serde counts a tuple variant's elements, a Lua table cannot hold `nil`, and mlua
    // deserializes this as `None`. A constant, not a capability.
    zellij
        .set("null", mlua::LuaSerdeExt::null(&lua))
        .map_err(|e| anyhow!("failed to define zellij.null: {}", e))?;
    install_log(&lua, &zellij, chunk_name)?;
    install_text_width(&lua, &zellij)?;
    super::ui::install(&lua, &zellij).map_err(|e| anyhow!("failed to install shared UI: {}", e))?;

    // `require("zellij")` is the module itself; anything else is a file next to the plugin.
    let require = {
        let zellij = zellij.clone();
        let plugin_dir = env
            .resolved_source_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf);
        let loaded = lua
            .create_table()
            .map_err(|e| anyhow!("failed to define require: {}", e))?;
        // The in-flight marker, made once per interpreter and never handed to Lua, so no module
        // can return a value equal to it. See `require_sibling`.
        let loading = lua
            .create_table()
            .map_err(|e| anyhow!("failed to define require: {}", e))?;
        lua.create_function(move |lua, name: String| {
            if name == "zellij" {
                return Ok(LuaValue::Table(zellij.clone()));
            }
            require_sibling(lua, plugin_dir.as_deref(), &loaded, &loading, &name)
        })
        .map_err(|e| anyhow!("failed to define require: {}", e))?
    };
    globals
        .set("require", require)
        .map_err(|e| anyhow!("failed to define require: {}", e))?;
    globals
        .set("zellij", zellij.clone())
        .map_err(|e| anyhow!("failed to define the zellij global: {}", e))?;

    let plugin: Table = lua
        .load(source)
        .set_name(chunk_name)
        .eval()
        .map_err(|e| anyhow!("{}", e))
        .with_context(|| {
            format!(
                "failed to load the Lua plugin at '{}'. A plugin must return a table, eg. \
                 `local M = {{}} ... return M`",
                chunk_name
            )
        })?;

    Ok(LuaPlugin {
        lua,
        plugin,
        zellij,
        env,
    })
}

/// `require("<name>")` for a file beside the plugin: `<name>.lua`, or `<name>/init.lua`, with
/// `.` in the name meaning `/` as Lua's own `require` reads it.
///
/// This is the one place a plugin runs code it did not arrive with, so it is host-mediated and
/// confined rather than `package.path`: the file has to be under the plugin's own folder *after*
/// symlinks are followed, so a link out of the folder is refused rather than followed, and a
/// name that climbs (`..`) or starts at the root never reaches the disk. The confinement is the
/// same argument `zellij.fs` makes, and it is what lets a plugin be split across files without
/// reopening `package`.
///
/// Modules load once per interpreter and are cached, as `require` does; a module that returns
/// nothing is remembered as `true`, as `require` does. A module still loading when it is asked
/// for again is a cycle, and says so rather than recursing until the stack goes.
///
/// The in-flight mark is `loading`, a table the host made and Lua never sees, compared by
/// identity: any value a module could *return* - the string "loading" included - is a value a
/// module may legitimately return, and marking with one would turn that module into a permanent
/// false cycle.
fn require_sibling(
    lua: &Lua,
    plugin_dir: Option<&Path>,
    loaded: &Table,
    loading: &Table,
    name: &str,
) -> mlua::Result<LuaValue> {
    let cached: LuaValue = loaded.get(name)?;
    if cached == LuaValue::Table(loading.clone()) {
        return Err(mlua::Error::runtime(format!(
            "require(\"{name}\") is circular: it is still being loaded"
        )));
    }
    if cached != LuaValue::Nil {
        return Ok(cached);
    }
    let Some(plugin_dir) = plugin_dir else {
        return Err(mlua::Error::runtime(format!(
            "plugins may only require(\"zellij\") here, not \"{name}\": this plugin was not \
             loaded from a file, so it has no folder to find a module in"
        )));
    };
    let relative = name.replace('.', "/");
    let relative = Path::new(&relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(mlua::Error::runtime(format!(
            "require(\"{name}\"): a module name is a file beside the plugin, not a path"
        )));
    }
    let root = plugin_dir.canonicalize().map_err(|e| {
        mlua::Error::runtime(format!(
            "require(\"{name}\"): the plugin's folder cannot be read: {e}"
        ))
    })?;
    let candidates = [
        plugin_dir.join(relative).with_extension("lua"),
        plugin_dir.join(relative).join("init.lua"),
    ];
    let Some(path) = candidates.iter().find(|path| path.is_file()) else {
        return Err(mlua::Error::runtime(format!(
            "require(\"{name}\"): no module found; looked for {} and {}",
            candidates[0].display(),
            candidates[1].display()
        )));
    };
    let real = path.canonicalize().map_err(|e| {
        mlua::Error::runtime(format!("require(\"{name}\"): {}: {e}", path.display()))
    })?;
    if !real.starts_with(&root) {
        return Err(mlua::Error::runtime(format!(
            "require(\"{name}\"): {} leads outside the plugin's folder and is refused",
            path.display()
        )));
    }
    let source = std::fs::read_to_string(&real).map_err(|e| {
        mlua::Error::runtime(format!("require(\"{name}\"): {}: {e}", real.display()))
    })?;
    loaded.set(name, loading.clone())?;
    let result: mlua::Result<LuaValue> = lua
        .load(&source)
        .set_name(real.display().to_string())
        .eval();
    let value = match result {
        Ok(LuaValue::Nil) => LuaValue::Boolean(true),
        Ok(value) => value,
        Err(e) => {
            loaded.set(name, LuaValue::Nil)?;
            return Err(e);
        },
    };
    loaded.set(name, value.clone())?;
    Ok(value)
}

/// Install `zellij.text.width`: a string's width in terminal columns.
///
/// In Rust because the answer is a table of Unicode properties the server already links, and
/// because it has to agree with what the server draws: this is the same measurement the grid
/// uses. `zellij.text.pad` and friends in the prelude are built on it.
fn install_text_width(lua: &Lua, zellij: &Table) -> Result<()> {
    use unicode_width::UnicodeWidthStr;
    let text: Table = zellij
        .get("text")
        .map_err(|e| anyhow!("failed to read zellij.text: {}", e))?;
    let width = lua
        .create_function(|_, s: mlua::LuaString| Ok(s.to_str()?.width()))
        .map_err(|e| anyhow!("failed to define zellij.text.width: {}", e))?;
    text.set("width", width)
        .map_err(|e| anyhow!("failed to define zellij.text.width: {}", e))?;
    Ok(())
}

/// Replace the global named `name` with a fresh table holding only `allowed`, taken from it.
///
/// A whitelist rather than a deny-list on purpose. Nilling out the names known to be dangerous
/// leaves the next Lua release free to add one this file has never heard of, and the cost of
/// missing a single name here is the whole sandbox - `os.setlocale` alone would let a plugin
/// change how the entire server formats numbers. Building the replacement from the names that
/// were vetted means anything unlisted is gone whether or not it was thought of.
fn restrict_table(lua: &Lua, globals: &Table, name: &str, allowed: &[&str]) -> Result<()> {
    let original: Table = globals
        .get(name)
        .map_err(|e| anyhow!("failed to read the '{}' library: {}", name, e))?;
    let restricted = lua
        .create_table()
        .map_err(|e| anyhow!("failed to restrict '{}': {}", name, e))?;
    for entry in allowed {
        let value: LuaValue = original
            .get(*entry)
            .map_err(|e| anyhow!("failed to read '{}.{}': {}", name, entry, e))?;
        restricted
            .set(*entry, value)
            .map_err(|e| anyhow!("failed to restrict '{}.{}': {}", name, entry, e))?;
    }
    globals
        .set(name, restricted)
        .map_err(|e| anyhow!("failed to restrict '{}': {}", name, e))?;
    Ok(())
}

/// Install `zellij.log(...)`, and point `print` at it.
///
/// A plugin has no terminal of its own: stock `print` writes to the server process's stdout,
/// which is nowhere a plugin author can look. Both go to zellij's log instead, tagged with the
/// plugin's file so one plugin's lines can be told from another's (`[lua:<chunk>]`), which is
/// also what a tool tailing the log for a given plugin greps for. A wasm plugin gets the same
/// treatment for free, since the host captures its stderr.
///
/// Values are joined with tabs the way `print` does, so `print(a, b)` reads as expected.
fn install_log(lua: &Lua, zellij: &Table, chunk_name: &str) -> Result<()> {
    let tag = format!("[lua:{}]", chunk_name);
    let log = lua
        .create_function(move |_, values: mlua::MultiValue| {
            let line = values
                .iter()
                .map(|value| match value {
                    LuaValue::String(s) => s.to_string_lossy().to_string(),
                    // `tostring` semantics, so a number or a table reads the way it would in Lua.
                    other => other.to_string().unwrap_or_else(|_| format!("{:?}", other)),
                })
                .collect::<Vec<_>>()
                .join("\t");
            log::info!("{} {}", tag, line);
            Ok(())
        })
        .map_err(|e| anyhow!("failed to define zellij.log: {}", e))?;
    zellij
        .set("log", log.clone())
        .map_err(|e| anyhow!("failed to define zellij.log: {}", e))?;
    lua.globals()
        .set("print", log)
        .map_err(|e| anyhow!("failed to redirect print: {}", e))?;
    Ok(())
}

/// Install `zellij.json.encode` / `zellij.json.decode`.
///
/// In Rust rather than the prelude, unusually for this runtime: a JSON parser written in Lua
/// would be slower and larger than the one already linked in, and this widens no authority -
/// it converts values that are already in the plugin's hands.
///
/// Both directions go through mlua's serde bridge with the same options [`to_lua_value`] uses,
/// so a JSON `null` decodes to a real `nil` rather than mlua's null userdata, and a plugin
/// checking `if value then` gets the answer Lua would give. The one thing a caller has to know
/// is that Lua cannot tell an empty array from an empty table, so `encode {}` is `[]`.
fn install_json(lua: &Lua, zellij: &Table) -> Result<()> {
    use mlua::LuaSerdeExt;

    let json = lua
        .create_table()
        .map_err(|e| anyhow!("failed to define zellij.json: {}", e))?;

    let decode = lua
        .create_function(|lua, text: mlua::LuaString| {
            let text = text.to_str()?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| mlua::Error::runtime(format!("could not decode JSON: {}", e)))?;
            super::to_lua_value(lua, &value).map_err(|e| mlua::Error::runtime(format!("{:#}", e)))
        })
        .map_err(|e| anyhow!("failed to define zellij.json.decode: {}", e))?;

    let encode = lua
        .create_function(|lua, value: LuaValue| {
            let value: serde_json::Value = lua.from_value(value).map_err(|e| {
                mlua::Error::runtime(format!("could not encode this as JSON: {}", e))
            })?;
            serde_json::to_string(&value)
                .map_err(|e| mlua::Error::runtime(format!("could not encode JSON: {}", e)))
        })
        .map_err(|e| anyhow!("failed to define zellij.json.encode: {}", e))?;

    json.set("decode", decode)
        .and_then(|_| json.set("encode", encode))
        .map_err(|e| anyhow!("failed to define zellij.json: {}", e))?;
    zellij
        .set("json", json)
        .map_err(|e| anyhow!("failed to define zellij.json: {}", e))?;
    Ok(())
}
