//! `yinz:<tag>` works only while the runtime's name list and the compiled-in files agree. A
//! listed tag without a file fails to load as "not compiled in"; a file without a listed tag
//! never resolves as a plugin ("Failed to resolve plugin").
use super::*;
use yinz_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES;

/// Top-level modules the builtins import that are not plugins themselves.
const SHARED_MODULES: &[&str] = &[];

#[test]
fn every_listed_builtin_has_a_compiled_in_source() {
    for tag in BUILTIN_DENO_PLUGIN_NAMES {
        assert!(
            builtin_deno_plugin(tag).is_some(),
            "BUILTIN_DENO_PLUGIN_NAMES lists '{tag}', but there is no UTF-8 \
             assets/plugins/{tag}.ts to compile in"
        );
    }
}

#[test]
fn every_compiled_in_builtin_is_listed() {
    for tag in BUILTIN_DENO_FILES.files().filter_map(entry_tag) {
        assert!(
            BUILTIN_DENO_PLUGIN_NAMES.contains(&tag) || SHARED_MODULES.contains(&tag),
            "assets/plugins/{tag}.ts is compiled in, but yinz:{tag} will not resolve until \
             BUILTIN_DENO_PLUGIN_NAMES lists it (or SHARED_MODULES here, if it is not a plugin)"
        );
    }
    for module in SHARED_MODULES {
        assert!(
            !BUILTIN_DENO_PLUGIN_NAMES.contains(module),
            "'{module}' is listed as a builtin, so it is no longer only a shared module"
        );
        assert!(
            builtin_deno_plugin(module).is_some(),
            "the shared module '{module}' is no longer compiled in"
        );
    }
}

#[test]
fn only_top_level_modules_resolve() {
    assert!(builtin_deno_plugin("status-bar").is_some());
    for tag in [
        "status-bar.test",
        "status-bar.integration.test",
        "status-bar/render",
        "",
    ] {
        assert_eq!(builtin_deno_plugin(tag), None, "{tag:?}");
    }
}
