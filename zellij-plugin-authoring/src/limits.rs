//! The per-file source budget, applied before a source reaches a worker, the loader or the
//! checker, and again inside the worker.
use zellij_runtime::errors::prelude::*;

pub const MAX_SOURCE_BYTES: usize = zellij_runtime::consts::MAX_DENO_SOURCE_BYTES;

pub fn check_source(path: &std::path::Path, source: &str) -> Result<()> {
    if source.len() > MAX_SOURCE_BYTES {
        bail!(
            "Deno plugin {} exceeds 8 MiB source-file budget",
            path.display()
        );
    }
    Ok(())
}

pub fn read_source(path: &std::path::Path) -> Result<String> {
    use std::io::Read;
    let mut source = String::new();
    std::fs::File::open(path)
        .with_context(|| format!("open Deno module {}", path.display()))?
        .take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_string(&mut source)
        .with_context(|| format!("read Deno module {}", path.display()))?;
    check_source(path, &source)?;
    Ok(source)
}
