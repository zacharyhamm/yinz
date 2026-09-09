//! Filesystem modules share the checker boundary; embedded entrypoints can import only explicitly registered modules.
use super::limits;
use deno_ast::{MediaType, ParseParams, ParsedSource};
use deno_core::{
    error::ModuleLoaderError, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse,
    ModuleLoader, ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, RequestedModuleType,
    ResolutionKind,
};
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
};
use zellij_runtime::errors::prelude::*;

pub const SDK_SPECIFIER: &str = "zellij:plugin";
pub(super) const WORKFLOW_SPECIFIER: &str = "zellij:workflow-view";
pub(super) const WORKFLOW_PATH: &str = "/__zellij_builtin__/workflow-view.ts";
pub(super) const WORKFLOW_SOURCE: &str =
    include_str!("../../../../zellij-config/assets/plugins/workflow-view.ts");

pub struct PluginModuleLoader {
    root: PathBuf,
    embedded: bool,
    entrypoint: ModuleSpecifier,
    source_maps: RefCell<HashMap<String, Vec<u8>>>,
    sources: RefCell<HashMap<String, String>>,
    entry_source: Option<String>,
}

impl PluginModuleLoader {
    pub fn new(path: &Path, source: &str) -> Result<Self> {
        let mut loader = Self::for_check(path, true)?;
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
        let entrypoint = ModuleSpecifier::parse(&format!("zellij-builtin:///{name}.ts"))
            .context("resolve embedded plugin entrypoint")?;
        let mut loader = Self {
            root: PathBuf::from(format!("/__zellij_builtin__/{name}.ts")),
            embedded: true,
            entrypoint,
            source_maps: RefCell::new(HashMap::new()),
            sources: RefCell::new(HashMap::new()),
            entry_source: None,
        };
        loader.set_entry_source(source)?;
        Ok(loader)
    }

    pub fn entrypoint(&self) -> &ModuleSpecifier {
        &self.entrypoint
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
        let source = if url.as_str() == WORKFLOW_SPECIFIER {
            WORKFLOW_SOURCE.to_owned()
        } else {
            match self
                .entry_source
                .as_ref()
                .filter(|_| url == self.entrypoint())
            {
                Some(source) => source.clone(),
                None => limits::read_source(&self.local_path(url, declarations)?)?,
            }
        };
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
        media_type(path, true)?;
        let absolute = absolute_path(path)?;
        let root = absolute
            .parent()
            .context("plugin has no parent directory")?
            .canonicalize()
            .context("resolve plugin directory")?;
        // Only a genuinely absent leaf may be supplied by an editor. A dangling
        // symlink must still fail canonicalization.
        let entry = if unsaved
            && matches!(std::fs::symlink_metadata(&absolute), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        {
            root.join(absolute.file_name().context("plugin has no filename")?)
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

    pub(super) fn resolve_local(
        &self,
        specifier: &str,
        referrer: &str,
        declarations: bool,
    ) -> Result<ModuleSpecifier> {
        if specifier == WORKFLOW_SPECIFIER {
            return ModuleSpecifier::parse(WORKFLOW_SPECIFIER)
                .context("resolve embedded workflow view");
        }
        if specifier == SDK_SPECIFIER {
            return ModuleSpecifier::parse(SDK_SPECIFIER).context("resolve embedded plugin SDK");
        }
        if self.embedded || referrer == WORKFLOW_SPECIFIER {
            bail!("unsupported import {specifier:?} from {referrer}: embedded plugins may import only registered modules (zellij:plugin, zellij:workflow-view)");
        }
        if !(specifier.starts_with("./") || specifier.starts_with("../"))
            || specifier.contains(['\\', '?', '#'])
        {
            return Err(anyhow!("unsupported import {specifier:?} from {referrer}: use an explicit relative .js, .mjs, .ts or .mts path, or zellij:plugin"));
        }
        let referrer = ModuleSpecifier::parse(referrer).context("parse module referrer")?;
        // Referrers normally originate here. Validate again for callers of
        // import.meta.resolve and for the offline compiler host.
        if referrer != self.entrypoint {
            self.local_path(&referrer, declarations)?;
        }
        let url = referrer
            .join(specifier)
            .context("resolve relative module import")?;
        file_specifier(&self.local_path(&url, declarations)?)
    }

    fn load_source(&self, url: &ModuleSpecifier) -> Result<ModuleSource> {
        let code = if url.as_str() == SDK_SPECIFIER {
            super::SDK_MODULE.to_owned()
        } else {
            // Resolve again at load time: a symlink may have changed since resolve.
            let path = if url.as_str() == WORKFLOW_SPECIFIER {
                PathBuf::from(WORKFLOW_PATH)
            } else if self.embedded && url == self.entrypoint() {
                self.root.clone()
            } else if url == self.entrypoint() && self.entry_source.is_some() {
                url.to_file_path()
                    .map_err(|_| anyhow!("invalid plugin entrypoint"))?
            } else {
                self.local_path(url, false)?
            };
            let source = self.read_source(url, false)?;
            let emitted = transpile(&path, &source, false)?;
            if let Some(mut map) = emitted.source_map {
                if self.embedded {
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
            emitted.text
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

pub(super) fn transpile(
    path: &Path,
    source: &str,
    declarations: bool,
) -> Result<deno_ast::EmittedSourceText> {
    super::worker::transpile(path, source, declarations)
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
