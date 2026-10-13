//! Filesystem modules share the checker boundary; builtin modules resolve only from embedded assets.
use super::limits;
use deno_ast::{MediaType, ParseParams, ParsedSource};
use deno_core::{
    error::ModuleLoaderError, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse,
    ModuleLoader, ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, RequestedModuleType,
    ResolutionKind,
};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};
use yinz_runtime::errors::prelude::*;

pub const SDK_SPECIFIER: &str = "yinz:plugin";
pub(super) const EMBEDDED_ROOT: &str = "/__yinz_builtin__";

fn embedded_url(path: &Path) -> Result<ModuleSpecifier> {
    let url = file_specifier(&Path::new("/").join(path))?;
    ModuleSpecifier::parse(&format!("yinz-builtin://{}", url.path()))
        .context("invalid embedded module path")
}

pub(super) fn compiler_specifier(path: &Path) -> Result<ModuleSpecifier> {
    if let Ok(relative) = path.strip_prefix(EMBEDDED_ROOT) {
        embedded_url(relative)
    } else {
        file_specifier(path)
    }
}

pub struct PluginModuleLoader {
    root: PathBuf,
    embedded: bool,
    entrypoint: ModuleSpecifier,
    source_maps: RefCell<HashMap<String, Vec<u8>>>,
    sources: RefCell<HashMap<String, String>>,
    entry_source: Option<String>,
    testing: bool,
}

impl PluginModuleLoader {
    pub fn new(path: &Path, source: &str) -> Result<Self> {
        Self::new_with_root(path, source, None)
    }

    pub fn new_with_root(path: &Path, source: &str, root: Option<&Path>) -> Result<Self> {
        let mut loader = Self::for_check_with_root(path, true, root)?;
        media_type(path, false)?;
        let entry = loader
            .entrypoint
            .to_file_path()
            .map_err(|_| anyhow!("invalid plugin entrypoint"))?;
        media_type(&entry, false)?;
        loader.set_entry_source(source)?;
        Ok(loader)
    }

    pub fn embedded(name: &str, source: &str) -> Result<Self> {
        let entrypoint = embedded_url(Path::new(&format!("{name}.ts")))?;
        let mut loader = Self {
            root: PathBuf::from(EMBEDDED_ROOT),
            embedded: true,
            entrypoint,
            source_maps: RefCell::new(HashMap::new()),
            sources: RefCell::new(HashMap::new()),
            entry_source: None,
            testing: false,
        };
        loader.set_entry_source(source)?;
        Ok(loader)
    }

    pub(super) fn checked_sources(&self) -> Result<HashMap<PathBuf, String>> {
        self.sources
            .borrow()
            .iter()
            .map(|(url, source)| {
                let path = self.module_path(&ModuleSpecifier::parse(url)?, true)?;
                Ok((path, source.clone()))
            })
            .collect()
    }

    pub fn entrypoint(&self) -> &ModuleSpecifier {
        &self.entrypoint
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn set_entry_source(&mut self, source: &str) -> Result<()> {
        limits::check_source(&self.root, source)?;
        self.entry_source = Some(source.to_owned());
        Ok(())
    }

    pub(super) fn read_source(&self, url: &ModuleSpecifier, declarations: bool) -> Result<String> {
        if let Some(source) = self.sources.borrow().get(url.as_str()) {
            return Ok(source.clone());
        }
        let source = match self
            .entry_source
            .as_ref()
            .filter(|_| url == self.entrypoint())
        {
            Some(source) => source.clone(),
            None if url.scheme() == "yinz-builtin" => {
                let path = self.module_path(url, declarations)?;
                let relative = path.strip_prefix(EMBEDDED_ROOT)?;
                yinz_deno_builtins::BUILTIN_DENO_FILES
                    .get_file(relative)
                    .and_then(|file| file.contents_utf8())
                    .with_context(|| format!("embedded module is not registered: {url}"))?
                    .to_owned()
            },
            None => limits::read_source(&self.local_path(url, declarations)?)?,
        };
        limits::check_source(&self.module_path(url, declarations)?, &source)?;
        let mut sources = self.sources.borrow_mut();
        let total: usize = sources.values().map(String::len).sum();
        if source.len() > limits::MAX_GRAPH_BYTES - total {
            bail!(
                "Deno plugin {} exceeds 64 MiB module-graph budget",
                self.entrypoint
            );
        }
        sources.insert(url.to_string(), source.clone());
        Ok(source)
    }

    pub(super) fn for_check(path: &Path, unsaved: bool) -> Result<Self> {
        Self::for_check_with_root(path, unsaved, None)
    }

    pub(super) fn for_check_with_root(
        path: &Path,
        unsaved: bool,
        root: Option<&Path>,
    ) -> Result<Self> {
        media_type(path, true)?;
        let absolute = absolute_path(path)?;
        let parent = absolute
            .parent()
            .context("plugin has no parent directory")?;
        let root = root
            .unwrap_or(parent)
            .canonicalize()
            .context("resolve plugin directory")?;
        // Only a genuinely absent leaf may be supplied by an editor. A dangling
        // symlink must still fail canonicalization.
        let entry = if unsaved
            && matches!(std::fs::symlink_metadata(&absolute), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        {
            parent
                .canonicalize()
                .context("resolve plugin directory")?
                .join(absolute.file_name().context("plugin has no filename")?)
        } else {
            absolute
                .canonicalize()
                .context("resolve plugin entrypoint")?
        };
        if !entry.starts_with(&root) {
            return Err(anyhow!(
                "plugin entrypoint escapes its directory: {}",
                path.display()
            ));
        }
        media_type(&entry, true)?;
        Ok(Self {
            root,
            embedded: false,
            entrypoint: file_specifier(&entry)?,
            source_maps: RefCell::new(HashMap::new()),
            sources: RefCell::new(HashMap::new()),
            entry_source: None,
            testing: false,
        })
    }

    pub(super) fn local_path(&self, url: &ModuleSpecifier, declarations: bool) -> Result<PathBuf> {
        if self.embedded {
            bail!(
                "embedded plugin {} cannot load module {url}; only registered embedded modules are supported",
                self.entrypoint
            );
        }
        if url.scheme() != "file" || url.query().is_some() || url.fragment().is_some() {
            return Err(anyhow!(
                "only local module paths without query or fragment are supported: {url}"
            ));
        }
        let path = url
            .to_file_path()
            .map_err(|_| anyhow!("invalid local module path: {url}"))?;
        media_type(&path, declarations)?;
        let canonical = path
            .canonicalize()
            .with_context(|| format!("resolve module {}", path.display()))?;
        if !canonical.starts_with(&self.root) {
            return Err(anyhow!(
                "module escapes plugin directory: {}",
                path.display()
            ));
        }
        media_type(&canonical, declarations)?;
        if !canonical.is_file() {
            return Err(anyhow!(
                "module is not a regular file: {}",
                canonical.display()
            ));
        }
        Ok(canonical)
    }

    pub(super) fn module_path(&self, url: &ModuleSpecifier, declarations: bool) -> Result<PathBuf> {
        if url.scheme() != "yinz-builtin" {
            if url == self.entrypoint() && self.entry_source.is_some() {
                return url
                    .to_file_path()
                    .map_err(|_| anyhow!("invalid plugin entrypoint"));
            }
            return self.local_path(url, declarations);
        }
        if url.host_str().is_some() || url.query().is_some() || url.fragment().is_some() {
            bail!("invalid embedded module path: {url}");
        }
        // Parse as a file URL to decode escaped names, then look up only compiled-in files.
        let file_url = ModuleSpecifier::parse(&format!("file://{}", url.path()))?;
        let path = file_url
            .to_file_path()
            .map_err(|_| anyhow!("invalid embedded module path"))?;
        let relative = path.strip_prefix("/")?;
        media_type(relative, declarations)?;
        if url != self.entrypoint()
            && yinz_deno_builtins::BUILTIN_DENO_FILES
                .get_file(relative)
                .is_none()
        {
            bail!("embedded module is not registered: {url}");
        }
        Ok(Path::new(EMBEDDED_ROOT).join(relative))
    }

    pub(super) fn for_tests(path: &Path, root: &Path) -> Result<Self> {
        let mut loader = Self::for_check(path, false)?;
        loader.root = root.canonicalize().context("resolve test root")?;
        loader.local_path(loader.entrypoint(), true)?;
        loader.testing = true;
        Ok(loader)
    }

    pub(super) fn testing(&self) -> bool {
        self.testing
    }

    pub(super) fn resolve_local(
        &self,
        specifier: &str,
        referrer: &str,
        declarations: bool,
    ) -> Result<ModuleSpecifier> {
        if self.testing && specifier.starts_with("file:") {
            let url = ModuleSpecifier::parse(specifier).context("resolve test module")?;
            return file_specifier(&self.local_path(&url, declarations)?);
        }
        if specifier == "yinz:testing" && self.testing {
            return ModuleSpecifier::parse(specifier).context("resolve testing SDK");
        }
        if specifier == SDK_SPECIFIER {
            return ModuleSpecifier::parse(SDK_SPECIFIER).context("resolve embedded plugin SDK");
        }
        if !(specifier.starts_with("./") || specifier.starts_with("../"))
            || specifier.contains(['\\', '?', '#'])
        {
            return Err(anyhow!("unsupported import {specifier:?} from {referrer}: use an explicit relative .js, .mjs, .ts or .mts path, or yinz:plugin"));
        }
        let referrer = ModuleSpecifier::parse(referrer).context("parse module referrer")?;
        // Referrers normally originate here. Validate again for callers of
        // import.meta.resolve and for the offline compiler host.
        if referrer != self.entrypoint {
            self.module_path(&referrer, declarations)?;
        }
        let url = referrer
            .join(specifier)
            .context("resolve relative module import")?;
        if referrer.scheme() == "yinz-builtin" {
            return compiler_specifier(&self.module_path(&url, declarations)?);
        }
        file_specifier(&self.local_path(&url, declarations)?)
    }

    fn load_source(&self, url: &ModuleSpecifier) -> Result<ModuleSource> {
        let code = if url.as_str() == SDK_SPECIFIER {
            super::SDK_MODULE.to_owned()
        } else if url.as_str() == "yinz:testing" && self.testing {
            include_str!("testing/module.js").to_owned()
        } else {
            // Resolve again at load time: a symlink may have changed since resolve.
            let path = self.module_path(url, false)?;
            let source = self.read_source(url, false)?;
            // Builtin sources are compiled in, so their emitted text is worth keeping for the
            // life of the process.
            let pinned = url.scheme() == "yinz-builtin";
            let emitted = transpile_cached(&path, &source, false, pinned)?;
            if let Some(mut map) = emitted.source_map.clone() {
                if pinned {
                    let mut mapped: serde_json::Value =
                        serde_json::from_str(&map).context("decode embedded plugin source map")?;
                    mapped["sources"] = serde_json::json!([url.as_str()]);
                    map = serde_json::to_string(&mapped)
                        .context("encode embedded plugin source map")?;
                }
                self.source_maps
                    .borrow_mut()
                    .insert(url.to_string(), map.into_bytes());
            }
            emitted.text.clone()
        };
        Ok(ModuleSource::new(
            ModuleType::JavaScript,
            ModuleSourceCode::String(code.into()),
            url,
            None,
        ))
    }
}

impl ModuleLoader for PluginModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        kind: ResolutionKind,
    ) -> std::result::Result<ModuleSpecifier, ModuleLoaderError> {
        // Deno asks the loader to resolve the already-canonical main URL.
        if matches!(kind, ResolutionKind::MainModule) && specifier == self.entrypoint.as_str() {
            if self.entry_source.is_some() {
                return Ok(self.entrypoint.clone());
            }
            return self
                .local_path(&self.entrypoint, false)
                .and_then(|p| file_specifier(&p))
                .map_err(loader_error);
        }
        self.resolve_local(specifier, referrer, false)
            .with_context(|| format!("resolve import {specifier:?} from {referrer}"))
            .map_err(loader_error)
    }

    fn load(
        &self,
        specifier: &ModuleSpecifier,
        _referrer: Option<&ModuleLoadReferrer>,
        options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        if options.requested_module_type != RequestedModuleType::None {
            return ModuleLoadResponse::Sync(Err(ModuleLoaderError::generic(
                "module type attributes are not supported",
            )));
        }
        ModuleLoadResponse::Sync(self.load_source(specifier).map_err(loader_error))
    }

    fn get_source_map(&self, specifier: &str) -> Option<Cow<'_, [u8]>> {
        self.source_maps
            .borrow()
            .get(specifier)
            .cloned()
            .map(Cow::Owned)
    }

    fn get_source_mapped_source_line(&self, specifier: &str, line: usize) -> Option<String> {
        self.sources
            .borrow()
            .get(specifier)?
            .lines()
            .nth(line)
            .map(str::to_owned)
    }
}

fn loader_error(error: anyhow::Error) -> ModuleLoaderError {
    ModuleLoaderError::generic(format!("{error:#}"))
}

pub(super) fn absolute_path(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .context("locate plugin working directory")?
            .join(path)
    })
}

pub(super) fn file_specifier(path: &Path) -> Result<ModuleSpecifier> {
    ModuleSpecifier::from_file_path(path)
        .map_err(|_| anyhow!("invalid module path: {}", path.display()))
}

pub(super) fn media_type(path: &Path, declarations: bool) -> Result<MediaType> {
    let media = MediaType::from_path(path);
    match media {
        MediaType::JavaScript | MediaType::Mjs | MediaType::TypeScript | MediaType::Mts => {
            Ok(media)
        },
        MediaType::Dts if declarations => Ok(media),
        _ => Err(anyhow!(
            "unsupported module {}: expected .js, .mjs, .ts or .mts (.d.ts is check-only)",
            path.display()
        )),
    }
}

pub(super) fn parse(path: &Path, source: &str, declarations: bool) -> Result<ParsedSource> {
    limits::check_source(path, source)?;
    let parsed = deno_ast::parse_module(ParseParams {
        specifier: file_specifier(&absolute_path(path)?)?,
        text: source.to_owned().into(),
        media_type: media_type(path, declarations)?,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })
    .with_context(|| format!("parse plugin {}", path.display()))?;
    if !parsed.diagnostics().is_empty() {
        return Err(anyhow!(
            "{}",
            parsed
                .diagnostics()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok(parsed)
}

/// Emitted text and source map, keyed by the source that produced it.
///
/// Every transpile is a compilation-worker subprocess (see `worker.rs` for why the parse must
/// happen out of process), and nothing above this layer remembers a result: each connected
/// client gets its own instance, a reload re-reads every module, and the checker's syntax pass
/// re-transpiles the buffer it is given. So the same bytes were being handed to a fresh process
/// once per module per client per reload. The key is the content, so an edited file always
/// misses and an identical reload always hits; the path is part of it so a source map is never
/// attributed to another file with the same text, and `declarations` because `.d.ts` is
/// accepted by the checker's pass only.
///
/// Errors are not cached: a broken file is expected to be edited next.
const MAX_CACHE_ENTRIES: usize = 256;
const MAX_CACHE_BYTES: usize = limits::MAX_GRAPH_BYTES;

type CacheKey = (PathBuf, [u8; 32], bool);

struct CacheEntry {
    emitted: Arc<deno_ast::EmittedSourceText>,
    bytes: usize,
    /// Compiled-in builtins are never evicted: their text cannot change for the life of the
    /// process, and they are loaded by every session.
    pinned: bool,
    last_used: u64,
}

#[derive(Default)]
struct TranspileCache {
    entries: HashMap<CacheKey, CacheEntry>,
    total_bytes: usize,
    clock: u64,
}

impl TranspileCache {
    fn get(&mut self, key: &CacheKey) -> Option<Arc<deno_ast::EmittedSourceText>> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.last_used = self.clock;
        Some(entry.emitted.clone())
    }

    fn insert(&mut self, key: CacheKey, emitted: Arc<deno_ast::EmittedSourceText>, pinned: bool) {
        let bytes = emitted.text.len() + emitted.source_map.as_ref().map_or(0, String::len);
        if bytes > MAX_CACHE_BYTES {
            return;
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.total_bytes -= previous.bytes;
        }
        self.clock += 1;
        self.total_bytes += bytes;
        self.entries.insert(
            key,
            CacheEntry {
                emitted,
                bytes,
                pinned,
                last_used: self.clock,
            },
        );
        while self.entries.len() > MAX_CACHE_ENTRIES || self.total_bytes > MAX_CACHE_BYTES {
            let Some(victim) = self
                .entries
                .iter()
                .filter(|(_, entry)| !entry.pinned)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&victim) {
                self.total_bytes -= removed.bytes;
            }
        }
    }
}

static TRANSPILE_CACHE: LazyLock<Mutex<TranspileCache>> =
    LazyLock::new(|| Mutex::new(TranspileCache::default()));

fn cache_key(path: &Path, source: &str, declarations: bool) -> CacheKey {
    (
        path.to_owned(),
        Sha256::digest(source.as_bytes()).into(),
        declarations,
    )
}

/// The number of cached emissions and the bytes they hold.
#[cfg(test)]
pub(crate) fn cache_stats() -> (usize, usize) {
    let cache = TRANSPILE_CACHE.lock().unwrap();
    (cache.entries.len(), cache.total_bytes)
}

/// Fill the cache with `count` distinct unpinned entries without spawning a worker for each.
#[cfg(test)]
pub(crate) fn cache_fill_for_test(count: usize) {
    let mut cache = TRANSPILE_CACHE.lock().unwrap();
    for index in 0..count {
        let path = PathBuf::from(format!("/__test_fill__/{index}.ts"));
        let emitted = Arc::new(deno_ast::EmittedSourceText {
            text: format!("export const fill = {index};"),
            source_map: None,
        });
        cache.insert(cache_key(&path, &emitted.text, false), emitted, false);
    }
}

pub(super) fn transpile(
    path: &Path,
    source: &str,
    declarations: bool,
) -> Result<Arc<deno_ast::EmittedSourceText>> {
    transpile_cached(path, source, declarations, false)
}

pub(super) fn transpile_cached(
    path: &Path,
    source: &str,
    declarations: bool,
    pinned: bool,
) -> Result<Arc<deno_ast::EmittedSourceText>> {
    let key = cache_key(path, source, declarations);
    if let Some(emitted) = TRANSPILE_CACHE.lock().unwrap().get(&key) {
        return Ok(emitted);
    }
    // The worker runs unlocked: a spawn is milliseconds, and two executor threads compiling
    // the same content at once merely both insert it.
    let emitted = Arc::new(super::worker::transpile(path, source, declarations)?);
    TRANSPILE_CACHE
        .lock()
        .unwrap()
        .insert(key, emitted.clone(), pinned);
    Ok(emitted)
}

pub(super) fn transpile_native(
    path: &Path,
    source: &str,
    declarations: bool,
) -> Result<deno_ast::EmittedSourceText> {
    let parsed = parse(path, source, declarations)?;
    if !matches!(
        media_type(path, declarations)?,
        MediaType::TypeScript | MediaType::Mts
    ) {
        return Ok(deno_ast::EmittedSourceText {
            text: source.to_owned(),
            source_map: None,
        });
    }
    Ok(parsed
        .transpile(
            &deno_ast::TranspileOptions {
                jsx: None,
                ..Default::default()
            },
            &deno_ast::TranspileModuleOptions { module_kind: None },
            &deno_ast::EmitOptions {
                source_map: deno_ast::SourceMapOption::Separate,
                inline_sources: true,
                ..Default::default()
            },
        )
        .with_context(|| format!("transpile plugin {}", path.display()))?
        .into_source())
}

#[cfg(test)]
#[path = "unit/loader.rs"]
mod tests;
