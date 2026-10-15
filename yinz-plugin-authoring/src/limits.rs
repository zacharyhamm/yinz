//! The per-file source budget, applied before a source reaches a worker, the loader or the
//! checker, and again inside the worker.
use yinz_runtime::errors::prelude::*;

pub const MAX_SOURCE_BYTES: usize = yinz_runtime::consts::MAX_PLUGIN_SOURCE_BYTES;

pub fn check_source(path: &std::path::Path, source: &str) -> Result<()> {
    if source.len() > MAX_SOURCE_BYTES {
        bail!(
            "Scheme plugin {} exceeds 8 MiB source-file budget",
            path.display()
        );
    }
    Ok(())
}

pub fn read_source(path: &std::path::Path) -> Result<String> {
    use std::io::Read;
    let mut source = String::new();
    std::fs::File::open(path)
        .with_context(|| format!("open Scheme module {}", path.display()))?
        .take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_string(&mut source)
        .with_context(|| format!("read Scheme module {}", path.display()))?;
    check_source(path, &source)?;
    Ok(source)
}
