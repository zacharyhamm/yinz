//! The TypeScript builtin plugins, compiled into the server and transpiled by its Deno worker.
//!
//! Only the plugin host depends on this crate (the whole-app tests also use it as a
//! dev-dependency), so an edit to a builtin's source rebuilds the host and what links it, not the
//! configuration crate or the wasm plugins. This crate holds only files. The tags of the
//! builtins are listed once, in `yinz_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES`, and the
//! builtin tagged `x` is `x.ts` at the top of `assets/plugins`. The tests here hold the list and
//! the files to each other.
use include_dir::{include_dir, Dir, File};

/// Module sources and adjacent tests shipped with TypeScript builtins.
pub static BUILTIN_DENO_FILES: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets/plugins");

/// The source of the builtin tagged `tag`: `<tag>.ts` at the top of [`BUILTIN_DENO_FILES`].
/// This finds a file, not a builtin: a shared module has one too, so
/// check a tag against `BUILTIN_DENO_PLUGIN_NAMES` before loading it as a plugin.
pub fn builtin_deno_plugin(tag: &str) -> Option<&'static str> {
    BUILTIN_DENO_FILES
        .files()
        .find(|file| entry_tag(file) == Some(tag))?
        .contents_utf8()
}

/// The tag a top-level module loads under; `None` for tests (`x.test.ts`,
/// `x.integration.test.ts`) and anything that is not TypeScript.
fn entry_tag(file: &File<'static>) -> Option<&'static str> {
    let tag = file.path().to_str()?.strip_suffix(".ts")?;
    (!tag.contains('.')).then_some(tag)
}

#[cfg(test)]
#[path = "unit/names_tests.rs"]
mod names_tests;
