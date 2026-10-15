use anyhow::{Context, Result};
use steel::compiler::modules::SourceModuleResolver;
use steel::steel_vm::{
    builtin::BuiltInModule, engine::Engine, primitives::register_builtin_modules,
};

// Resolve every external module before Steel can fall back to the filesystem.
struct DenyExternalModules;
impl SourceModuleResolver for DenyExternalModules {
    fn exists(&self, _: &str) -> bool {
        true
    }
    fn resolve(&self, _: &str) -> Option<String> {
        Some("(error \"External modules must be loaded by the Yinz host\")".to_owned())
    }
}

pub fn engine() -> Result<Engine> {
    // Steel's sandbox constructor still exposes process/meta modules and a separate macro VM.
    let mut primitives = Engine::new_raw_no_kernel();
    register_builtin_modules(&mut primitives, true);
    let mut engine = Engine::new_raw_no_kernel();
    engine.register_source_module_resolver(DenyExternalModules);
    let mut base = BuiltInModule::new("steel/base");
    for name in [
        "steel/hash",
        "steel/sets",
        "steel/lists",
        "steel/strings",
        "steel/vectors",
        "steel/immutable-vectors",
        "steel/identity",
        "steel/numbers",
        "steel/equality",
        "steel/ord",
        "steel/transducers",
        "steel/symbols",
        "steel/constants",
        "steel/bytevectors",
        "steel/core/result",
        "steel/core/option",
    ] {
        let module = primitives
            .builtin_modules()
            .get(name)
            .with_context(|| format!("missing pure Steel module {name}"))?;
        for (name, value) in module.inner_map().iter() {
            engine.register_value(name, value.clone());
        }
        base = base.with_module(module.clone());
        engine.register_module(module);
    }
    let ports = primitives
        .builtin_modules()
        .get("steel/ports")
        .context("finding Steel ports")?;
    let mut safe_ports = BuiltInModule::new("steel/ports");
    for name in [
        "#%raw-display",
        "#%raw-write",
        "#%raw-write-char",
        "#%raw-write-string",
        "write-line!",
        "open-output-string",
        "get-output-string",
        "open-input-string",
        "read-line-from-port",
        "#%read-port-to-string",
        "read-char",
        "peek-char",
        "eof-object?",
        "eof-object",
    ] {
        if let Some(value) = ports.inner_map().get(name) {
            safe_ports.register_value(name, value.clone());
            engine.register_value(
                &format!("yinz-raw-{}", name.trim_start_matches("#%raw-")),
                value.clone(),
            );
        }
    }
    engine.register_module(safe_ports);
    let meta = primitives
        .builtin_modules()
        .get("steel/meta")
        .context("finding Steel exception support")?;
    let mut safe_meta = BuiltInModule::new("steel/meta");
    for name in [
        "call-with-exception-handler",
        "error-object-message",
        "raise-error",
        "value->string",
        "call/cc",
    ] {
        let value = meta
            .inner_map()
            .get(name)
            .with_context(|| format!("missing pure Steel function {name}"))?
            .clone();
        safe_meta.register_value(name, value.clone());
        engine.register_value(name, value);
    }
    engine.register_module(safe_meta);
    engine.register_module(base);
    engine
        .run(include_str!("prelude.scm"))
        .context("loading Scheme prelude")?;
    Ok(engine)
}
